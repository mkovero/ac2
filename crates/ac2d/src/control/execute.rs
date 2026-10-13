//! [`Control::execute`]: one command against the state store, the dispatch from each
//! [`Command`] to the handler that runs it.

use std::time::Instant;

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{ColourFamily, DelayState, GenAction, MeasKind, Measurement, SweepFailure};
use ac2_proto::units::{ClientId, MeasId, Rev, Seconds};
use ac2_proto::{Command, ErrorCode, ErrorDetail, ProtoError, ReplyBody, Welcome};

use crate::jobs::JobCmd;
use crate::util::{perr, perr_detail};

use super::validate::{not_a_sweep, smoothing_only, spl_in_place, validate_meas};
use super::{Control, DelaySource, cal, owners};

impl Control {
    pub(super) fn execute(
        &mut self,
        client: &ClientId,
        cmd: Command,
    ) -> Result<ReplyBody, ProtoError> {
        let ack = |rev: Rev| Ok(ReplyBody::Ack { rev });
        match cmd {
            Command::Hello { client: software } => {
                tracing::info!("hello from {} ({software})", client.0);
                Ok(ReplyBody::Welcome(Welcome {
                    server: format!(
                        "ac2d {} (build {})",
                        env!("CARGO_PKG_VERSION"),
                        env!("AC2_BUILD_ID")
                    ),
                    client_id: client.clone(),
                    daemon_incarnation: self.s.incarnation,
                    session_epoch: self.epoch(),
                    rev: self.store.rev(),
                }))
            }
            Command::SessionDevices => Ok(ReplyBody::Backends(self.backend_infos())),
            Command::SessionPreview { backend, device } => self.session_preview(backend, device),
            Command::SessionPreviewStop => {
                self.close_preview();
                ack(self.store.rev())
            }
            Command::SessionDetectLoopback { .. } => Err(perr(
                ErrorCode::Internal,
                "session.detect_loopback is answered asynchronously",
            )),
            Command::SessionControlPanel { .. } => Err(perr(
                ErrorCode::Internal,
                "session.control_panel is answered asynchronously",
            )),
            Command::SessionOpen { config } => self.session_open(client, config),
            Command::SessionClose => {
                self.session_close(client);
                ack(self.store.rev())
            }
            Command::SessionStatus => Ok(ReplyBody::Session(self.store.state().session.clone())),

            Command::GenAcquire { force } => self.gen_acquire(client, force),
            Command::GenSet {
                lease_token,
                desired,
            } => self.gen_set(client, lease_token, desired),
            Command::GenRefresh { lease_token } => {
                self.lease_check(client, lease_token)?;
                let deadline = Instant::now() + self.s.lease_expiry;
                if let Some(l) = &mut self.lease {
                    l.deadline = deadline;
                }
                if self.store.state().generator.firing {
                    self.gate.open_until(deadline);
                }
                Ok(ReplyBody::Lease(self.wire_lease(lease_token)))
            }
            Command::GenRelease { lease_token } => {
                self.lease_check(client, lease_token)?;
                self.abort_sweep(SweepFailure::Stopped, "the stimulus lease was released");
                self.stop_output();
                self.lease = None;
                let mut g = self.store.state().generator.clone();
                g.owner = None;
                g.armed = false;
                g.firing = false;
                self.audit(&mut g, GenAction::Release, Some(client));
                ack(self.commit(Change::Generator(g)))
            }
            Command::GenCeiling {
                ceiling,
                confirm_raise,
            } => self.gen_ceiling(client, ceiling, confirm_raise),
            Command::SessionOutputs { outputs } => self.session_outputs(outputs),
            Command::ServerInfo => Ok(ReplyBody::Server(self.server_info())),
            Command::ServerAuthorize { name, key } => self.server_authorize(client, &name, &key),
            Command::ServerRevoke { name } => self.server_revoke(client, &name),
            Command::GenStop => {
                self.abort_sweep(SweepFailure::Stopped, "the stimulus was stopped");
                self.stop_output();
                let mut g = self.store.state().generator.clone();
                g.armed = false;
                g.firing = false;
                self.audit(&mut g, GenAction::Stop, Some(client));
                ack(self.commit(Change::Generator(g)))
            }

            Command::MeasCreate { config } => {
                validate_meas(&config)?;
                self.check_meas_owner(None, &config.kind)?;
                let grid = self.grid_of(None, &config.kind)?;
                let id = MeasId(self.next_meas);
                self.next_meas += 1;
                let grid_id = grid.map(|g| self.register_grid(g));
                let delay = matches!(config.kind, MeasKind::Transfer { .. }).then(|| DelayState {
                    applied: Seconds(0.0),
                    applied_samples: 0.0,
                    nudged: Seconds(0.0),
                    nudged_samples: 0.0,
                    tracking: false,
                    awaiting_pick: false,
                    last_finding: None,
                });
                let m = Measurement {
                    id,
                    config,
                    config_rev: Rev(self.store.rev().0 + 1),
                    running: false,
                    delay,
                    grid_id,
                    colour: None,
                    layout: None,
                };
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                self.ensure_spl_log(&m, None);
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasUpdate { meas, config } => {
                validate_meas(&config)?;
                let mut m = self.meas(meas)?.clone();
                self.check_meas_owner(Some(meas), &config.kind)?;
                self.check_operand_update(meas, &config.kind)?;
                if owners::owner_only(&m.config, &config) {
                    // Moving a math channel changes where it is listed, not what it
                    // computes: its job and its frames' config rev go on.
                    m.config = config;
                    self.commit(Change::Measurement(Patch::Set(m.clone())));
                    return Ok(ReplyBody::Measurement(m));
                }
                let grid = self.grid_of(Some(meas), &config.kind)?;
                let old_leq = match &m.config.kind {
                    MeasKind::Spl { config } => Some(config.leq.clone()),
                    _ => None,
                };
                if let Some(spl) = spl_in_place(&m.config.kind, &config.kind) {
                    m.config = config;
                    m.config_rev = Rev(self.store.rev().0 + 1);
                    if let Some(j) = self.jobs.get(&meas) {
                        j.send(JobCmd::Spl {
                            config: Box::new(spl),
                            rev: m.config_rev,
                        });
                    }
                    self.commit(Change::Measurement(Patch::Set(m.clone())));
                    self.ensure_spl_log(&m, old_leq.as_ref());
                    return Ok(ReplyBody::Measurement(m));
                }
                if let Some(change) = smoothing_only(&m.config.kind, &config.kind) {
                    // Display smoothing changes in place: averaging goes on, and the next
                    // frame carries the new setting under the new rev.
                    m.config = config;
                    m.config_rev = Rev(self.store.rev().0 + 1);
                    if let Some(j) = self.jobs.get(&meas) {
                        j.send(JobCmd::Smoothing {
                            change,
                            rev: m.config_rev,
                        });
                    }
                    self.commit(Change::Measurement(Patch::Set(m.clone())));
                    return Ok(ReplyBody::Measurement(m));
                }
                let was_transfer = matches!(m.config.kind, MeasKind::Transfer { .. });
                let is_transfer = matches!(config.kind, MeasKind::Transfer { .. });
                m.config = config;
                m.config_rev = Rev(self.store.rev().0 + 1);
                if !is_transfer {
                    m.delay = None;
                } else if !was_transfer {
                    m.delay = Some(DelayState {
                        applied: Seconds(0.0),
                        applied_samples: 0.0,
                        nudged: Seconds(0.0),
                        nudged_samples: 0.0,
                        tracking: false,
                        awaiting_pick: false,
                        last_finding: None,
                    });
                }
                m.grid_id = grid.map(|g| self.register_grid(g));
                // The log and the entity follow the new configuration before the job
                // starts, so a restarted meter reports against its new windows; the
                // measurement's own commit comes after, under the rev it names.
                self.ensure_spl_log(&m, old_leq.as_ref());
                m.config_rev = Rev(self.store.rev().0 + 1);
                if m.running && self.session.is_some() {
                    self.stop_job(meas);
                    if let Some(g) = self.start_job(&m)? {
                        m.grid_id = Some(self.register_grid(g));
                    }
                }
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasColour { meas, colour } => {
                let mut m = self.meas(meas)?.clone();
                if matches!(m.config.kind, MeasKind::Math { .. }) {
                    return Err(perr(
                        ErrorCode::Invalid,
                        format!(
                            "{} is a math channel: it is drawn in its owner's colours",
                            m.config.name
                        ),
                    ));
                }
                if colour.is_some_and(|c| !c.is_valid()) {
                    return Err(perr(
                        ErrorCode::Invalid,
                        format!(
                            "colour families are 0 … {} and neutral",
                            ColourFamily::HUES - 1
                        ),
                    ));
                }
                // Only how its curves are drawn changes: the job and its frames go on.
                m.colour = colour;
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasLayout { meas, layout } => {
                let mut m = self.meas(meas)?.clone();
                if let Some(l) = &layout {
                    l.check().map_err(|e| perr(ErrorCode::Invalid, e))?;
                }
                // Only what clients show changes: the job and its frames go on.
                m.layout = layout;
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasDelete { meas, traces } => self.meas_delete(meas, traces),
            Command::MeasStart { meas } => {
                let mut m = self.meas(meas)?.clone();
                not_a_sweep(&m)?;
                if !m.running
                    && self.session.is_some()
                    && let Some(g) = self.start_job(&m)?
                {
                    m.grid_id = Some(self.register_grid(g));
                }
                m.running = true;
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasStop { meas } => {
                let mut m = self.meas(meas)?.clone();
                not_a_sweep(&m)?;
                self.stop_job(meas);
                m.running = false;
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasReset { meas } => {
                not_a_sweep(self.meas(meas)?)?;
                if let MeasKind::Math { .. } = self.meas(meas)?.config.kind {
                    // Its operands hold the averages; resetting one is the operator's choice.
                    return Err(perr(
                        ErrorCode::Invalid,
                        "a math channel holds no averages of its own; reset its operands",
                    ));
                }
                if let Some(j) = self.jobs.get(&meas) {
                    j.send(JobCmd::Reset);
                }
                ack(self.store.rev())
            }

            Command::DelayFind { .. } => Err(perr(
                ErrorCode::Internal,
                "delay.find is answered asynchronously",
            )),
            Command::DelayInsert { meas, pick } => {
                let d = self.transfer_delay(meas)?;
                let Some(f) = &d.last_finding else {
                    return Err(perr(
                        ErrorCode::Invalid,
                        "no delay finding to insert; run delay.find or use delay.set",
                    ));
                };
                if let Some(reasons) = f.no_estimate() {
                    return Err(perr(
                        ErrorCode::Refused,
                        format!(
                            "the last finding has no estimate ({reasons:?}); find again or use delay.set"
                        ),
                    ));
                }
                let delay = f
                    .arrival(pick)
                    .ok_or_else(|| {
                        perr(
                            ErrorCode::NotFound,
                            "the last finding has no such arrival (ranked picks need an ambiguous finding)",
                        )
                    })?
                    .delay;
                self.set_delay(meas, delay, DelaySource::Insert)
            }
            Command::DelaySet { meas, delay } => self.set_delay(meas, delay, DelaySource::Typed),
            Command::DelayNudge { meas, by } => {
                let d = self.transfer_delay(meas)?;
                if !by.0.is_finite() {
                    return Err(perr(ErrorCode::Invalid, "the step must be finite"));
                }
                let delay = Seconds(d.applied.0 + by.0);
                self.set_delay(meas, delay, DelaySource::Nudge)
            }
            Command::DelayTrack { meas, enabled } => {
                self.transfer_delay(meas)?;
                let mut m = self.meas(meas)?.clone();
                if let Some(d) = &mut m.delay {
                    d.tracking = enabled;
                }
                if let Some(j) = self.jobs.get(&meas) {
                    j.send(JobCmd::Track { enabled });
                }
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }

            Command::TraceList => Ok(ReplyBody::Traces(self.store.state().traces.clone())),
            Command::TraceCapture { meas, name, slot } => self.trace_capture(meas, name, slot),
            Command::TraceGet { trace } => self.trace_get(trace),
            Command::TraceUpdate { trace, edit } => self.trace_update(trace, edit),
            Command::TraceDelete { trace } => self.trace_delete(trace),
            Command::TraceAverage {
                traces,
                method,
                reference,
                name,
            } => self.trace_average(&traces, method, reference, name),
            Command::TraceImport {
                file_name,
                format,
                role,
                ir,
                content,
            } => self.trace_import(file_name, format, role, ir, &content.0),
            Command::TraceExport { trace, format } => self.trace_export(trace, format),
            Command::TraceMicCurve { trace, curve } => self.trace_mic_curve(trace, curve),

            Command::CalSpl {
                input,
                mic,
                calibrator_level,
                calibrator_freq,
            } => self.cal_spl(input, mic, calibrator_level, calibrator_freq),
            Command::CalSplElectrical {
                input,
                mic,
                connection,
                volts,
                freq,
                mic_sensitivity,
                loading,
                uncertainty,
                replace_acoustic,
            } => self.cal_spl_electrical(cal::ElectricalArgs {
                input,
                mic,
                connection,
                volts,
                freq,
                mic_sensitivity,
                loading,
                uncertainty,
                replace_acoustic,
            }),
            Command::CalCurveImport {
                mic,
                label,
                file_name,
                content,
                input,
            } => self.cal_curve_import(mic, label, file_name, content, input),
            Command::CalCurveRename { curve, label } => self.cal_curve_rename(curve, label),
            Command::CalCurveDelete { curve } => self.cal_curve_delete(curve),
            Command::CalList => self.cal_list(),
            Command::CalDelete { key } => self.cal_delete(&key),
            Command::SessionInputs { inputs } => self.session_inputs(inputs),

            Command::SplLogGet {
                meas,
                log,
                from,
                max,
            } => self.spl_log_get(meas, log, from, max),
            Command::SplLogNew { meas } => self.spl_log_new(meas),
            Command::SplBandTransfer {
                meas,
                foh,
                at_place,
                background,
                place,
            } => self.spl_band_transfer(client, meas, &foh, &at_place, background.as_ref(), &place),
            Command::SplHistoryGet { meas, seconds } => self.spl_history_get(meas, seconds),
            Command::SplBandLogGet {
                meas,
                from,
                until,
                step,
            } => self.spl_band_log_get(meas, from, until, step),

            Command::SweepRun {
                lease_token,
                meas,
                name,
            } => self.sweep_run(client, lease_token, meas, name),

            Command::StateSnapshot => Ok(ReplyBody::Snapshot(Box::new(
                self.store.snapshot(self.s.incarnation),
            ))),
            Command::StateSince { rev } => match self.store.since(rev, Instant::now()) {
                Ok(evs) => Ok(ReplyBody::Events(evs)),
                Err(oldest) => Err(perr_detail(
                    ErrorCode::ResyncRequired,
                    format!(
                        "events after rev {} are no longer replayable; take a snapshot",
                        rev.0
                    ),
                    ErrorDetail::Resync { oldest },
                )),
            },
            Command::GridGet { grid_id } => self
                .grids
                .get(&grid_id)
                .cloned()
                .map(ReplyBody::Grid)
                .ok_or_else(|| perr(ErrorCode::NotFound, format!("no grid {grid_id}"))),
            Command::FileSave { .. }
            | Command::FileLoad { .. }
            | Command::FileNew { .. }
            | Command::FileList
            | Command::FileImport { .. }
            | Command::FileDownloadBegin { .. } => Err(perr(
                ErrorCode::Internal,
                "session file work is answered asynchronously",
            )),
            Command::FileRecover => self.file_recover(client),
            Command::FileSalvage { session } => self.file_salvage(client, &session),
            Command::FileUploadBegin {
                file,
                bytes,
                sha256,
            } => self.file_upload_begin(client, file, bytes, sha256),
            Command::FileUploadChunk {
                transfer,
                offset,
                data,
            } => self.file_upload_chunk(client, transfer, offset, &data),
            Command::FileDownloadChunk { transfer, offset } => {
                self.file_download_chunk(client, transfer, offset)
            }
            Command::FileDownloadEnd { transfer, saved_as } => {
                self.file_download_end(client, transfer, saved_as)
            }
            Command::RecStart { request } => self.rec_start(client, request),
            Command::RecStop => self.rec_stop(),
            Command::RecList => self.rec_list(),
            Command::SessionReplay { recording, pace } => {
                self.session_replay(client, &recording, pace)
            }
        }
    }
}
