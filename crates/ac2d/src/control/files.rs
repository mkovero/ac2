//! `file.save` / `file.load` / `file.list`: sessions on the daemon host
//! (format: `ac2_traces::session`).
//!
//! A load replaces every measurement and trace, stops and disarms the generator and drops
//! its owner, and starts a new session epoch newer than any epoch recorded in the loaded
//! traces: a trace captured in another run must never look like it shares the live time
//! base just because the epoch counters happen to match.

use std::path::{Path, PathBuf};

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    DelayState, GenAction, MeasKind, Measurement, Session, SessionFile, SessionRef, SweepFailure,
    TraceSource,
};
use ac2_proto::units::{ClientId, MeasId, Rev, Samples, Seconds, SessionEpoch, WallNs};
use ac2_proto::{ErrorCode, ErrorDetail, ProtoError, ReplyBody};
use ac2_traces::session::{
    self, Manifest, SavedDelay, SavedMeasurement, Session as SessionData, SessionError,
};

use super::{Control, static_grid, validate_meas};
use crate::util::{perr, perr_detail, wall_ns};

fn session_err(e: SessionError) -> ProtoError {
    let msg = e.to_string();
    match e {
        SessionError::NotFound(_) => perr(ErrorCode::NotFound, msg),
        SessionError::Version { found, .. } => perr_detail(
            ErrorCode::Unsupported,
            msg,
            ErrorDetail::SessionVersion {
                found,
                supported: session::VERSION,
            },
        ),
        SessionError::NotASession(_) | SessionError::BadName(_) | SessionError::Corrupt { .. } => {
            perr(ErrorCode::Invalid, msg)
        }
        SessionError::Io { .. } => perr(ErrorCode::Internal, msg),
    }
}

fn info(name: &str, dir: &Path, m: &Manifest) -> SessionFile {
    SessionFile {
        name: name.to_owned(),
        path: dir.to_string_lossy().into_owned(),
        saved_at: m.saved_at,
        measurements: u32::try_from(m.measurements.len()).unwrap_or(u32::MAX),
        traces: u32::try_from(m.traces.len()).unwrap_or(u32::MAX),
    }
}

impl Control {
    /// The directory a reference names. Paths are refused in network mode: a remote client
    /// may only use the daemon's own session directory.
    fn session_path(&self, r: &SessionRef) -> Result<(String, PathBuf), ProtoError> {
        match r {
            SessionRef::Name { name } => {
                session::validate_name(name).map_err(session_err)?;
                Ok((name.clone(), self.s.session_dir.join(name)))
            }
            SessionRef::Path { path } => {
                if self.s.network {
                    return Err(perr(
                        ErrorCode::Refused,
                        "remote clients name sessions; paths are for local clients only",
                    ));
                }
                let p = PathBuf::from(path);
                if !p.is_absolute() {
                    return Err(perr(
                        ErrorCode::Invalid,
                        "a session path must be absolute (the daemon's working directory is not the client's)",
                    ));
                }
                let name = p
                    .file_name()
                    .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                Ok((name, p))
            }
        }
    }

    /// The measurements as a session saves them.
    pub(super) fn saved_measurements(&self) -> Vec<SavedMeasurement> {
        self.store
            .state()
            .measurements
            .iter()
            .map(|m| SavedMeasurement {
                id: m.id,
                config: m.config.clone(),
                running: m.running,
                frozen: m.frozen,
                delay: m.delay.as_ref().map(|d| SavedDelay {
                    applied: d.applied,
                    tracking: d.tracking,
                }),
            })
            .collect()
    }

    /// Everything a session holds, as of now; without the SPL logs' rows unless
    /// `with_logs` (the autosave keeps its logs on disk itself).
    pub(super) fn session_data(&self, with_logs: bool) -> Result<SessionData, ProtoError> {
        let traces = self
            .store
            .state()
            .traces
            .iter()
            .map(|t| self.stored(t.id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SessionData {
            saved_at: WallNs(wall_ns()),
            measurements: self.saved_measurements(),
            spl_logs: if with_logs {
                self.saved_spl_logs()
            } else {
                Vec::new()
            },
            traces,
        })
    }

    pub(super) fn file_save(&self, r: &SessionRef) -> Result<ReplyBody, ProtoError> {
        let (name, dir) = self.session_path(r)?;
        let data = self.session_data(true)?;
        let m = session::save(&dir, &data).map_err(session_err)?;
        tracing::info!("session saved to {}", dir.display());
        Ok(ReplyBody::SessionFile(info(&name, &dir, &m)))
    }

    pub(super) fn file_list(&self) -> Result<ReplyBody, ProtoError> {
        let l = session::list(&self.s.session_dir).map_err(session_err)?;
        Ok(ReplyBody::Sessions(
            l.iter().map(|(n, p, m)| info(n, p, m)).collect(),
        ))
    }

    pub(super) fn file_load(
        &mut self,
        client: &ClientId,
        r: &SessionRef,
    ) -> Result<ReplyBody, ProtoError> {
        let (name, dir) = self.session_path(r)?;
        let m = session::read_manifest(&dir).map_err(session_err)?;
        let data = session::load(&dir).map_err(session_err)?;
        let epoch = self.replace_session(Some(client), data)?;
        tracing::info!(
            "session loaded from {} (epoch {}, disarmed)",
            dir.display(),
            epoch.0
        );
        Ok(ReplyBody::SessionFile(info(&name, &dir, &m)))
    }

    /// Replaces every measurement and trace with `data`, disarmed (module docs). Returns the
    /// new session epoch.
    pub(super) fn replace_session(
        &mut self,
        client: Option<&ClientId>,
        data: SessionData,
    ) -> Result<SessionEpoch, ProtoError> {
        // Everything is checked before anything changes: a refused load leaves the state
        // as it was.
        for sm in &data.measurements {
            validate_meas(&sm.config)?;
            if data.measurements.iter().filter(|o| o.id == sm.id).count() > 1 {
                return Err(perr(
                    ErrorCode::Invalid,
                    format!("measurement {} appears twice in the session", sm.id),
                ));
            }
        }
        for t in &data.traces {
            if data
                .traces
                .iter()
                .filter(|o| o.meta.id == t.meta.id)
                .count()
                > 1
            {
                return Err(perr(
                    ErrorCode::Invalid,
                    format!("trace {} appears twice in the session", t.meta.id),
                ));
            }
        }

        // Disarmed, no owner; a sweep run in progress is discarded.
        self.abort_sweep(SweepFailure::Stopped, "a session was loaded");
        self.stop_output();
        self.lease = None;
        let mut g = self.store.state().generator.clone();
        if g.owner.is_some() || g.armed || g.firing {
            g.owner = None;
            g.armed = false;
            g.firing = false;
            self.audit(&mut g, GenAction::Stop, client);
            self.commit(Change::Generator(g));
        }

        // Out with the old.
        let old_meas: Vec<MeasId> = self
            .store
            .state()
            .measurements
            .iter()
            .map(|m| m.id)
            .collect();
        for id in old_meas {
            self.stop_job(id);
            self.drop_spl_log(id);
            self.commit(Change::Measurement(Patch::Deleted(id)));
        }
        let old_traces: Vec<_> = self.store.state().traces.iter().map(|t| t.id).collect();
        for id in old_traces {
            self.commit(Change::Trace(Patch::Deleted(id)));
        }
        self.traces.clear();

        // A new epoch, newer than every epoch the loaded traces were captured in.
        let newest = data
            .traces
            .iter()
            .filter_map(|t| match t.meta.source {
                TraceSource::Captured { epoch, .. } | TraceSource::IrCapture { epoch, .. } => {
                    Some(epoch.0)
                }
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let epoch = SessionEpoch(self.epoch().0.max(newest) + 1);
        if self.session.is_some() {
            // The stream reopens without generator routes: nothing is armed after a load.
            // A device that fails to reopen leaves the session closed (committed by
            // `reopen_at`); the saved state still loads, its measurements waiting for one.
            if let Err(e) = self.reopen_at(&[], epoch) {
                tracing::warn!("session load: the audio stream did not reopen: {}", e.msg);
            }
        } else {
            self.commit(Change::Session(Session { epoch, open: None }));
        }

        // In with the new: the SPL logs first, so a meter's job carries on from its log.
        for l in data.spl_logs {
            self.set_spl_log(l.info.meas, crate::leq_log::LeqLog::from_rows(l.rows));
        }
        let fs = self.session.as_ref().map(|r| f64::from(r.sample_rate));
        for sm in data.measurements {
            self.next_meas = self.next_meas.max(sm.id.0.saturating_add(1));
            let delay = matches!(sm.config.kind, MeasKind::Transfer { .. }).then(|| {
                let d = sm.delay.unwrap_or(SavedDelay {
                    applied: Seconds(0.0),
                    tracking: false,
                });
                let samples = fs.map_or(0, |fs| (d.applied.0 * fs).round() as i64);
                DelayState {
                    applied: fs.map_or(d.applied, |fs| Seconds(samples as f64 / fs)),
                    applied_samples: Samples(samples),
                    tracking: d.tracking,
                    awaiting_pick: false,
                    last_finding: None,
                }
            });
            let grid_id = static_grid(&sm.config.kind).map(|g| self.register_grid(g));
            let mut meas = Measurement {
                id: sm.id,
                config: sm.config,
                config_rev: Rev(self.store.rev().0 + 1),
                running: sm.running,
                frozen: sm.frozen,
                delay,
                grid_id,
            };
            self.ensure_spl_log(&meas, None);
            meas.config_rev = Rev(self.store.rev().0 + 1);
            if meas.running && self.session.is_some() {
                match self.start_job(&meas) {
                    Ok(Some(g)) => meas.grid_id = Some(self.register_grid(g)),
                    Ok(None) => {}
                    Err(e) => tracing::warn!("measurement {} not started: {}", meas.id, e.msg),
                }
            }
            self.commit(Change::Measurement(Patch::Set(meas)));
        }
        for t in data.traces {
            self.register_grid(t.grid.clone());
            self.traces
                .insert(t.meta.id, t.grid, t.columns, t.sweep, t.mic_curve);
            self.commit(Change::Trace(Patch::Set(t.meta)));
        }
        Ok(epoch)
    }
}
