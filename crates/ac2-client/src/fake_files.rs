//! Session file commands of the fake daemon (`file.*`), on the same `ac2-traces` session
//! code the real daemon uses.

use std::path::PathBuf;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, ErrorCode, ErrorDetail, GridDef, Patch, ProtoError, ReplyBody};
use ac2_traces::session::{self, SavedDelay, SavedMeasurement, SessionError};

use super::{Shared, err};

fn session_err(e: SessionError) -> ProtoError {
    let msg = e.to_string();
    match e {
        SessionError::NotFound(_) => err(ErrorCode::NotFound, msg),
        SessionError::Version { found, .. } => ProtoError {
            code: ErrorCode::Unsupported,
            msg,
            detail: Some(ErrorDetail::SessionVersion {
                found,
                supported: session::VERSION,
            }),
        },
        SessionError::Io { .. } => err(ErrorCode::Internal, msg),
        _ => err(ErrorCode::Invalid, msg),
    }
}

impl Shared {
    fn session_path(&self, r: &SessionRef) -> Result<(String, PathBuf), ProtoError> {
        let root = self
            .opts
            .session_dir
            .clone()
            .ok_or_else(|| err(ErrorCode::Unsupported, "the fake has no session directory"))?;
        match r {
            SessionRef::Name { name } => {
                session::validate_name(name).map_err(session_err)?;
                Ok((name.clone(), root.join(name)))
            }
            SessionRef::Path { path } => {
                let p = PathBuf::from(path);
                let n = p
                    .file_name()
                    .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                Ok((n, p))
            }
            SessionRef::Upload { .. } => Err(err(
                ErrorCode::Unsupported,
                "the fake does not take session uploads",
            )),
        }
    }

    fn file_info(name: String, dir: &std::path::Path, m: &session::Manifest) -> SessionFile {
        SessionFile {
            name,
            path: dir.to_string_lossy().into_owned(),
            saved_at: m.saved_at,
            measurements: m.measurements.len() as u32,
            traces: m.traces.len() as u32,
        }
    }

    pub(super) fn file_save(&mut self, r: &SessionRef) -> Result<ReplyBody, ProtoError> {
        let (name, dir) = self.session_path(r)?;
        let traces = self
            .state
            .traces
            .iter()
            .map(|t| self.trace(t.id))
            .collect::<Result<Vec<_>, _>>()?;
        let s = session::Session {
            saved_at: WallNs(self.now_ns()),
            measurements: self
                .state
                .measurements
                .iter()
                .map(|m| SavedMeasurement {
                    id: m.id,
                    config: m.config.clone(),
                    running: m.running,
                    delay: m.delay.as_ref().map(|d| SavedDelay {
                        applied: d.applied,
                        nudged: d.nudged,
                        tracking: d.tracking,
                    }),
                    colour: m.colour,
                    layout: m.layout.clone(),
                })
                .collect(),
            spl_logs: self
                .state
                .measurements
                .iter()
                .filter_map(|m| match &m.config.kind {
                    MeasKind::Spl { config } => Some(session::SavedSplLog {
                        info: ac2_proto::spl_log::SplLogInfo {
                            meas: m.id,
                            name: m.config.name.clone(),
                            input: config.input,
                            mic: None,
                        },
                        rows: self.spl_rows.get(&m.id).cloned().unwrap_or_default(),
                        bands: Vec::new(),
                    }),
                    _ => None,
                })
                .collect(),
            traces,
        };
        let m = session::save(&dir, &s).map_err(session_err)?;
        self.working_clean(origin(r, &dir), m.saved_at);
        Ok(ReplyBody::SessionFile(Self::file_info(name, &dir, &m)))
    }

    /// `conflict` / `unsaved_changes` unless `discard` or clean.
    pub(super) fn guard_unsaved(&self, discard: bool) -> Result<(), ProtoError> {
        if discard || !self.state.working.dirty {
            return Ok(());
        }
        Err(ProtoError {
            code: ErrorCode::Conflict,
            msg: "unsaved changes".into(),
            detail: Some(ErrorDetail::UnsavedChanges {
                origin: self.state.working.origin.clone(),
            }),
        })
    }

    fn working_clean(&mut self, origin: Option<SessionOrigin>, saved_at: WallNs) {
        let w = WorkingSession {
            origin,
            dirty: false,
            saved_at: Some(saved_at),
            recover: None,
        };
        self.commit(Change::Working(w));
    }

    /// `file.new`: every measurement and trace deleted, untitled.
    pub(super) fn file_new(&mut self, discard: bool) -> Result<ReplyBody, ProtoError> {
        self.guard_unsaved(discard)?;
        self.marking = false;
        for id in self
            .state
            .measurements
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>()
        {
            if self.state.spl_logs.iter().any(|l| l.meas == id) {
                self.commit(Change::SplLog(Patch::Deleted(id)));
            }
            self.commit(Change::Measurement(Patch::Deleted(id)));
        }
        for id in self.state.traces.iter().map(|t| t.id).collect::<Vec<_>>() {
            self.commit(Change::Trace(Patch::Deleted(id)));
        }
        self.marking = true;
        self.commit(Change::Working(WorkingSession {
            origin: None,
            dirty: false,
            saved_at: None,
            recover: None,
        }));
        Ok(ReplyBody::Ack { rev: self.rev })
    }

    pub(super) fn file_load(
        &mut self,
        client: &ClientId,
        r: &SessionRef,
    ) -> Result<ReplyBody, ProtoError> {
        let (name, dir) = self.session_path(r)?;
        let m = session::read_manifest(&dir).map_err(session_err)?;
        let s = session::load(&dir).map_err(session_err)?;
        self.marking = false;
        self.lease = None;
        self.state.generator.owner = None;
        self.state.generator.armed = false;
        self.state.generator.firing = false;
        self.generator_changed(GenAction::Stop, Some(client.clone()));
        for id in self
            .state
            .measurements
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>()
        {
            if self.state.spl_logs.iter().any(|l| l.meas == id) {
                self.commit(Change::SplLog(Patch::Deleted(id)));
            }
            self.commit(Change::Measurement(Patch::Deleted(id)));
        }
        self.spl_rows.clear();
        for l in s.spl_logs {
            self.spl_rows.insert(l.info.meas, l.rows);
        }
        for id in self.state.traces.iter().map(|t| t.id).collect::<Vec<_>>() {
            self.commit(Change::Trace(Patch::Deleted(id)));
        }
        self.traces.data.clear();
        self.traces.sweeps.clear();
        self.traces.irs.clear();
        self.traces.mic_curves.clear();
        let newest = s
            .traces
            .iter()
            .filter_map(|t| match t.meta.source {
                TraceSource::Captured { epoch, .. } => Some(epoch.0),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let session = Session {
            epoch: SessionEpoch(self.state.session.epoch.0.max(newest) + 1),
            open: self.state.session.open.clone(),
            stopped: None,
        };
        self.commit(Change::Session(session));
        for sm in s.measurements {
            self.next_id = self.next_id.max(sm.id.0 + 1);
            let grid_id = match &sm.config.kind {
                MeasKind::Transfer { config } => {
                    let g = GridDef::Log {
                        ppo: config.grid().ppo,
                        k_min: config.grid().k_min,
                        k_max: config.grid().k_max,
                    };
                    let id = g.id();
                    self.grids.insert(id, g);
                    Some(id)
                }
                _ => None,
            };
            let delay = sm.delay.map(|d| DelayState {
                applied: d.applied,
                applied_samples: d.applied.0 * 48_000.0,
                nudged: d.nudged,
                nudged_samples: d.nudged.0 * 48_000.0,
                tracking: d.tracking,
                awaiting_pick: false,
                last_finding: None,
            });
            let meas = Measurement {
                id: sm.id,
                config: sm.config,
                config_rev: Rev(self.rev.0 + 1),
                running: sm.running,
                delay,
                grid_id,
                colour: sm.colour,
                layout: sm.layout,
            };
            self.put_meas(meas);
        }
        for t in s.traces {
            self.next_id = self.next_id.max(t.meta.id.0 + 1);
            self.grids.insert(t.grid.id(), t.grid.clone());
            if let Some(sw) = t.sweep {
                self.traces.sweeps.insert(t.meta.id, sw);
            }
            if let Some(ir) = t.ir {
                self.traces.irs.insert(t.meta.id, ir);
            }
            if let Some(k) = t.mic_curve {
                self.traces.mic_curves.insert(t.meta.id, k);
            }
            self.traces.data.insert(t.meta.id, (t.grid, t.columns));
            self.commit(Change::Trace(Patch::Set(t.meta)));
        }
        self.marking = true;
        self.working_clean(origin(r, &dir), m.saved_at);
        Ok(ReplyBody::SessionFile(Self::file_info(name, &dir, &m)))
    }

    pub(super) fn file_list(&self) -> Result<ReplyBody, ProtoError> {
        let Some(root) = self.opts.session_dir.clone() else {
            return Err(err(
                ErrorCode::Unsupported,
                "the fake has no session directory",
            ));
        };
        let l = session::list(&root).map_err(session_err)?;
        Ok(ReplyBody::Sessions(
            l.into_iter()
                .map(|(n, p, m)| Self::file_info(n, &p, &m))
                .collect(),
        ))
    }
}

/// The origin a save to, or load from, `r` gives the working state.
fn origin(r: &SessionRef, dir: &std::path::Path) -> Option<SessionOrigin> {
    Some(match r {
        SessionRef::Name { name } => SessionOrigin::Name { name: name.clone() },
        _ => SessionOrigin::Path {
            path: dir.to_string_lossy().into_owned(),
        },
    })
}
