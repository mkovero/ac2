//! [`Control::execute`]: one command against the state store, the dispatch from each
//! [`Command`] to the handler that runs it.

use std::time::Instant;

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    ClientRole, ColourFamily, GenAction, MeasConfig, MeasKind, Measurement, SweepFailure,
};
use ac2_proto::units::{Db, MeasId, Rev, Seconds};
use ac2_proto::{
    CalSplElectricalArgs, Command, ErrorCode, ErrorDetail, ProtoError, ReplyBody, TraceImportArgs,
    Welcome,
};

use crate::jobs::JobCmd;
use crate::route::Peer;
use crate::util::{internal, invalid, missing, perr_detail, refused};

use super::validate::{
    check_meas_name, display_only, not_a_sweep, smoothing_only, spl_in_place, stale_meas,
    validate_meas,
};
use super::{Control, DelaySource, cal, owners};

impl Control {
    pub(super) fn execute(&mut self, peer: &Peer, cmd: Command) -> Result<ReplyBody, ProtoError> {
        let client = peer.id();
        let ack = |rev: Rev| Ok(ReplyBody::Ack { rev });
        self.rig_preflight(&cmd)?;
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
                    stream_epoch: self.epoch(),
                    rev: self.store.rev(),
                    rig_id: self.s.rig_id,
                    name: self.s.name.clone(),
                    role: self.peer_role(peer).unwrap_or(ClientRole::Viewer),
                }))
            }
            Command::StreamDevices => Err(internal("stream.devices is answered asynchronously")),
            Command::StreamPreview { .. } => {
                Err(internal("stream.preview is answered asynchronously"))
            }
            Command::StreamPreviewStop => {
                self.stop_preview();
                ack(self.store.rev())
            }
            Command::StreamDetectLoopback { .. } => Err(internal(
                "stream.detect_loopback is answered asynchronously",
            )),
            Command::StreamControlPanel { .. } => {
                Err(internal("stream.control_panel is answered asynchronously"))
            }
            Command::StreamOpen { config } => self.rig_stream_open(peer, config),
            Command::StreamClose => {
                self.rig_stream_closing();
                self.stream_close(peer);
                ack(self.store.rev())
            }
            Command::StreamStatus => Ok(ReplyBody::Stream(self.store.state().stream.clone())),

            Command::GenAcquire { force } => self.gen_acquire(peer, force),
            Command::GenSet {
                lease_token,
                desired,
            } => self.gen_set(peer, lease_token, desired),
            Command::GenRefresh { lease_token } => {
                self.lease_check(peer, lease_token)?;
                let deadline = Instant::now() + self.s.lease_expiry;
                if let Some(l) = &mut self.out.lease {
                    l.deadline = deadline;
                }
                if self.store.state().generator.firing {
                    self.out.gate.open_until(deadline);
                }
                Ok(ReplyBody::Lease(self.wire_lease(lease_token)))
            }
            Command::GenRelease { lease_token } => {
                self.lease_check(peer, lease_token)?;
                self.abort_runs(SweepFailure::Stopped, "the stimulus lease was released");
                self.stop_output();
                self.out.lease = None;
                let mut g = self.store.state().generator.clone();
                g.owner = None;
                g.owner_local = false;
                g.armed = false;
                g.firing = false;
                self.audit(&mut g, GenAction::Release, Some(peer));
                ack(self.commit(Change::Generator(g)))
            }
            Command::GenCeiling {
                ceiling,
                confirm_raise,
            } => self.gen_ceiling(peer, ceiling, confirm_raise),
            Command::StreamOutputs { outputs } => self.stream_outputs(outputs),
            Command::ServerInfo => Ok(ReplyBody::Server(self.server_info(peer))),
            Command::ServerNetwork {
                enabled,
                bind,
                port,
            } => self.server_network(peer, enabled, bind, port),
            Command::AuthAuthorize { name, key, role } => {
                self.server_authorize(peer, &name, &key, role)
            }
            Command::AuthRevoke { name } => self.server_revoke(peer, &name),
            Command::AuthAdmitRig { key, rig_id, name } => {
                self.admit_rig(peer, &name, &key, rig_id)
            }
            Command::AuthRevokeRig { rig_id } => self.auth_revoke_rig(peer, rig_id),
            Command::RigPair { peer: p } => self.rig_pair(p),
            Command::RigUnpair { rig_id: rig } => self.rig_unpair(rig),
            Command::SourceState => Ok(ReplyBody::SourceState(Box::new(self.source_state()))),
            Command::RigMonitorAtStart { base, on } => self.rig_monitor_at_start(base, on),
            Command::OscTarget { base, addr } => self.osc_target(base, addr),
            Command::OscTest { msg } => self.osc_test(&msg),
            Command::ArtnetTarget { base, target } => self.artnet_target(base, target),
            Command::ArtnetFade { base, seconds } => self.artnet_fade(base, seconds),
            Command::ArtnetTest { channels } => self.artnet_test(&channels),
            Command::LevelsSet {
                base,
                ballistic,
                correlation_window,
                over_threshold,
                correlation_gate,
                loudness_target,
            } => self.levels_set(
                base,
                ballistic,
                correlation_window,
                over_threshold,
                correlation_gate,
                loudness_target,
            ),
            Command::LevelsReset { channels } => self.levels_reset(channels),
            Command::GenStop => {
                // Anyone may stop the output; a stop of nothing changes nothing, so a client
                // repeating it can neither flood every client with events nor write over
                // who last acted on the generator.
                if !self.generator_active() {
                    return ack(self.store.rev());
                }
                self.abort_runs(SweepFailure::Stopped, "the stimulus was stopped");
                self.stop_output();
                let mut g = self.store.state().generator.clone();
                g.armed = false;
                g.firing = false;
                self.audit(&mut g, GenAction::Stop, Some(peer));
                ack(self.commit(Change::Generator(g)))
            }

            Command::MeasCreate { config } => self.meas_create(config).map(ReplyBody::Measurement),
            Command::MeasUpdate { meas, base, config } => self.meas_update(meas, base, config),
            Command::MeasColour { meas, colour } => {
                let mut m = self.meas(meas)?.clone();
                if matches!(m.config.kind, MeasKind::Math { .. }) {
                    return Err(invalid(format!(
                        "{} is a math trace: it is drawn in its owner's colours",
                        m.config.name
                    )));
                }
                if colour.is_some_and(|c| !c.is_valid()) {
                    return Err(invalid(format!(
                        "colour families are 0 … {} and neutral",
                        ColourFamily::HUES - 1
                    )));
                }
                // Only how its curves are drawn changes: the job and its frames go on.
                m.colour = colour;
                Ok(self.reply_meas(m))
            }
            Command::MeasLayout { meas, layout } => {
                let mut m = self.meas(meas)?.clone();
                if let Some(l) = &layout {
                    l.check().map_err(invalid)?;
                    l.check_groups(|w| self.drawn_by_group(w))
                        .map_err(invalid)?;
                }
                // Only what clients show changes: the job and its frames go on.
                m.layout = layout;
                Ok(self.reply_meas(m))
            }
            Command::MeasOffset { meas, offset } => {
                let mut m = self.meas(meas)?.clone();
                if !Measurement::offset_ok(offset) {
                    return Err(invalid(format!(
                        "the offset must be finite and within ±{} dB",
                        Measurement::OFFSET_MAX_DB
                    )));
                }
                // Display only: the job and its frames go on, levels as measured.
                m.offset = offset;
                Ok(self.reply_meas(m))
            }
            Command::MeasDelete { meas, traces } => self.meas_delete(meas, traces),
            Command::MeasEq { meas, edit } => self.meas_eq(peer, meas, edit),
            Command::MeasStart { meas } => self.meas_start(meas).map(ReplyBody::Measurement),
            Command::MeasStop { meas } => {
                let mut m = self.meas(meas)?.clone();
                not_a_sweep(&m)?;
                self.stop_job(meas);
                m.running = false;
                Ok(self.reply_meas(m))
            }
            Command::MeasReset { meas } => {
                not_a_sweep(self.meas(meas)?)?;
                if let MeasKind::Math { .. } = self.meas(meas)?.config.kind {
                    // Its operands hold the averages; resetting one is the operator's choice.
                    return Err(invalid(
                        "a math trace holds no averages of its own; reset its operands",
                    ));
                }
                if let Some(j) = self.jobs.get(&meas) {
                    j.send(JobCmd::Reset);
                }
                ack(self.store.rev())
            }

            Command::DelayFind { .. } => Err(internal("delay.find is answered asynchronously")),
            Command::DelayInsert { meas, pick } => {
                self.transfer_delay(meas)?;
                let Some(f) = self.store.state().delay_finding(meas) else {
                    return Err(invalid(
                        "no delay finding to insert; run delay.find or use delay.set",
                    ));
                };
                if let Some(reasons) = f.no_estimate() {
                    return Err(refused(format!(
                        "the last finding has no estimate ({reasons:?}); find again or use delay.set"
                    )));
                }
                let delay = f
                    .arrival(pick)
                    .ok_or_else(|| {
                        missing("the last finding has no such arrival (ranked picks need an ambiguous finding)",
                        )
                    })?
                    .delay;
                self.set_delay(meas, delay, DelaySource::Insert)
            }
            Command::DelaySet { meas, delay } => self.set_delay(meas, delay, DelaySource::Typed),
            Command::DelayNudge { meas, by } => {
                let d = self.transfer_delay(meas)?;
                if !by.0.is_finite() {
                    return Err(invalid("the step must be finite"));
                }
                let delay = Seconds(d.applied.0 + by.0);
                self.set_delay(meas, delay, DelaySource::Nudge)
            }
            Command::DelayTrack { meas, enabled } => {
                // Tracking on or off moves neither `applied` nor `nudged`: the delay's rev
                // stays, and so does the stamp of the frames computed under it.
                let mut d = self.transfer_delay(meas)?.clone();
                d.tracking = enabled;
                if let Some(j) = self.jobs.get(&meas) {
                    j.send(JobCmd::Track { enabled });
                }
                Ok(self.reply_delay(d))
            }

            Command::TraceList => Ok(ReplyBody::Traces(self.store.state().traces.clone())),
            Command::TraceCapture { meas, name, slot } => {
                self.trace_capture(meas, name, slot, None)
            }
            Command::TraceGet { trace } => self.trace_get(trace),
            Command::DynamicsTable { trace } => self.dynamics_table(trace),
            Command::DynamicsEnvelope(a) => self.dynamics_envelope(a.trace, a.input, a.step),
            Command::TraceUpdate { trace, edit } => self.trace_update(trace, edit),
            Command::TraceDelete { trace } => self.trace_delete(trace),
            Command::TraceAverage {
                traces,
                method,
                reference,
                name,
            } => self.trace_average(&traces, method, reference, name),
            Command::TraceImport(TraceImportArgs {
                transfer,
                format,
                role,
                ir,
                owner,
            }) => self.trace_import(peer, transfer, format, role, ir, owner),
            Command::TraceTarget { curve, name, owner } => self.trace_target(curve, name, owner),
            Command::TraceExport { trace, format } => self.trace_export(peer, trace, format),
            Command::TraceMicCurve { trace, curve } => self.trace_mic_curve(trace, curve),

            Command::CalSpl {
                channel: input,
                mic,
                calibrator_level,
                calibrator_freq,
            } => self.cal_spl(input, mic, calibrator_level, calibrator_freq),
            Command::CalSplElectrical(CalSplElectricalArgs {
                channel: input,
                mic,
                connection,
                volts,
                freq,
                mic_sensitivity,
                loading,
                uncertainty,
                replace_acoustic,
            }) => self.cal_spl_electrical(cal::ElectricalArgs {
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
                channel: input,
            } => self.cal_curve_import(mic, label, file_name, content, input),
            Command::CalCurveRename { curve, label } => self.cal_curve_rename(curve, label),
            Command::CalCurveDelete { curve } => self.cal_curve_delete(curve),
            Command::CalList => self.cal_list(),
            Command::CalLine { channel, volts } => self.cal_line(channel, volts),
            Command::CalDelete { key } => self.cal_delete(&key),
            Command::StreamInputs { inputs, pairs } => self.stream_inputs(inputs, pairs),

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
            } => self.spl_band_transfer(peer, meas, &foh, &at_place, background.as_ref(), &place),
            Command::SplHistoryGet { meas, span } => self.spl_history_get(meas, span),
            Command::SplBandLogGet {
                meas,
                from,
                until,
                step,
            } => self.spl_band_log_get(meas, from, until, step),

            Command::SweepRun(ac2_proto::SweepRunArgs {
                lease_token,
                meas,
                name,
            }) => self.sweep_run(peer, lease_token, meas, name),
            Command::DynamicsRun(ac2_proto::DynamicsRunArgs {
                lease_token,
                meas,
                name,
                rests_from,
            }) => self.dynamics_run(peer, lease_token, meas, name, rests_from),

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
                .ok_or_else(|| missing(format!("no grid {grid_id}"))),
            Command::FileSave { .. }
            | Command::FileLoad { .. }
            | Command::FileNew { .. }
            | Command::FileRecover
            | Command::FileSalvage { .. }
            | Command::FileList
            | Command::FileImport { .. }
            | Command::FileDownloadBegin { .. } => {
                Err(internal("session file work is answered asynchronously"))
            }
            Command::FileUploadBegin {
                file,
                bytes,
                sha256,
            } => self.file_upload_begin(peer, file, bytes, sha256),
            Command::FileUploadChunk {
                transfer,
                offset,
                data,
            } => self.file_upload_chunk(peer, transfer, offset, &data),
            Command::FileDownloadChunk { transfer, offset } => {
                self.file_download_chunk(peer, transfer, offset)
            }
            Command::FileDownloadEnd { transfer, saved_as } => {
                self.file_download_end(peer, transfer, saved_as)
            }
            Command::RecStart { request } => self.rec_start(peer, request),
            Command::RecStop => self.rec_stop(),
            Command::RecList => self.rec_list(),
            Command::StreamReplay { recording, pace } => self.stream_replay(peer, &recording, pace),
            Command::PlanSave { plan } => self.plan_save(plan),
            Command::PlanList => Ok(ReplyBody::Plans(self.store.state().plans.clone())),
            Command::PlanDelete { name } => self.plan_delete(&name),
            Command::WalkNew {
                name,
                plan,
                kind,
                meas,
                points,
            } => self.walk_new(name, plan, kind, meas, points),
            Command::WalkUpdate { walk, name, points } => self.walk_update(walk, name, points),
            Command::WalkGoto { walk, step } => self.walk_goto(walk, step),
            Command::WalkCapture { walk } => self.walk_capture(walk),
            Command::WalkSkip { walk, point } => self.walk_skip(walk, point),
            Command::WalkRedo { walk, point } => self.walk_redo(walk, point),
            Command::WalkSwap { walk, points } => self.walk_swap(walk, &points),
            Command::WalkDelete { walk } => self.walk_delete(walk),
            Command::SimRun { request } => self.sim_run(request),
            Command::SimFit { request } => self.sim_fit(request),
            Command::SimMap { request } => self.sim_map(request),
            Command::SimCancel => self.sim_cancel(),
        }
    }

    /// `meas.create`: the new measurement, stopped.
    pub(super) fn meas_create(&mut self, config: MeasConfig) -> Result<Measurement, ProtoError> {
        check_meas_name(&config)?;
        validate_meas(&config)?;
        super::maths::new_average_has_two(&config.kind)?;
        self.spl_needs_mic(&config)?;
        self.check_meas_owner(None, &config.kind)?;
        let grid = self.grid_of(None, &config.kind)?;
        let id = self.ids.meas();
        let grid_id = grid.map(|g| self.register_grid(g));
        let m = Measurement {
            id,
            config,
            config_rev: Rev(self.store.rev().0 + 1),
            edit_rev: Rev(self.store.rev().0 + 1),
            running: false,
            grid_id,
            colour: None,
            layout: None,
            offset: Db(0.0),
            eq: Vec::new(),
        };
        let m = self.commit_meas_with_delay(m, None)?;
        self.ensure_spl_log(&m, None);
        Ok(self.meas(m.id)?.clone())
    }

    /// `meas.update`: the measurement with `config`, if `base` is its `edit_rev`.
    pub(super) fn meas_update(
        &mut self,
        meas: MeasId,
        base: Rev,
        config: MeasConfig,
    ) -> Result<ReplyBody, ProtoError> {
        check_meas_name(&config)?;
        validate_meas(&config)?;
        self.spl_needs_mic(&config)?;
        let mut m = self.meas(meas)?.clone();
        if base != m.edit_rev {
            return Err(stale_meas(meas, m.edit_rev));
        }
        self.check_dynamics_idle(meas)?;
        self.check_meas_owner(Some(meas), &config.kind)?;
        self.check_operand_update(meas, &config.kind)?;
        self.check_walk_meas_update(meas, &config.kind)?;
        self.check_eq_update(meas, &config.kind)?;
        if owners::owner_only(&m.config, &config) {
            // Moving a math trace changes where it is listed, not what it
            // computes: its job and its frames' config rev go on.
            m.config = config;
            return Ok(self.reply_meas(m));
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
            let m = self.commit_meas_read(m);
            self.ensure_spl_log(&m, old_leq.as_ref());
            return Ok(ReplyBody::Measurement(self.meas(meas)?.clone()));
        }
        if display_only(&m.config.kind, &config.kind) {
            // Only how clients draw it changes: the job, its averages and the
            // config rev its frames carry go on.
            m.config = config;
            return Ok(self.reply_meas(m));
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
            return Ok(self.reply_meas(m));
        }
        m.config = config;
        m.grid_id = grid.map(|g| self.register_grid(g));
        // The log and the entity follow the new configuration before the job
        // starts, so a restarted meter reports against its new windows. A kind
        // change from a transfer drops its finding and delay first; the
        // measurement's own commit comes next, under the rev it names (one more
        // for each of those deletes), and a delay made with it after.
        self.ensure_spl_log(&m, old_leq.as_ref());
        let drops = match (self.delay_of(meas).is_some(), &m.config.kind) {
            (true, MeasKind::Transfer { .. }) | (false, _) => 0,
            (true, _) => 1 + u64::from(self.store.state().delay_finding(meas).is_some()),
        };
        m.config_rev = Rev(self.store.rev().0 + 1 + drops);
        if m.running && self.stream.is_some() {
            self.stop_job(meas);
            if let Some(g) = self.start_job(&m)? {
                m.grid_id = Some(self.register_grid(g));
            }
        }
        self.commit_meas_with_delay(m, None)?;
        // Its grid may have changed: the sets' curves follow it.
        self.eq_refresh(meas);
        Ok(ReplyBody::Measurement(self.meas(meas)?.clone()))
    }

    /// `meas.start`: its job runs while a stream is open.
    pub(super) fn meas_start(&mut self, meas: MeasId) -> Result<Measurement, ProtoError> {
        let mut m = self.meas(meas)?.clone();
        not_a_sweep(&m)?;
        if !m.running
            && self.stream.is_some()
            && let Some(g) = self.start_job(&m)?
        {
            m.grid_id = Some(self.register_grid(g));
        }
        m.running = true;
        self.commit(Change::Measurement(Patch::Set(m.clone())));
        self.resume_walks();
        Ok(self.meas(meas)?.clone())
    }
}
