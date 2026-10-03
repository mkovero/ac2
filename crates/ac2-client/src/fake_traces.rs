//! Trace and session commands of the fake daemon, on the same `ac2-traces` code the real
//! daemon uses. Captures are synthetic: a flat −6 dB response with a 2nd-order roll-off
//! below 80 Hz, the measurement's delay compensated, coherence 0.95.

use std::collections::HashMap;
use std::path::PathBuf;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, ErrorCode, ErrorDetail, GridDef, Patch, ProtoError, ReplyBody};
use ac2_traces::columns::{Columns, StoredTrace, frequencies};
use ac2_traces::meta;
use ac2_traces::ops::{self, Derived};
use ac2_traces::session::{self, SavedDelay, SavedMeasurement, SessionError};
use ac2_traces::text;

use super::{Shared, err};

/// Trace data held by the fake.
#[derive(Debug, Default)]
pub(super) struct FakeTraces {
    pub(super) data: HashMap<TraceId, (GridDef, Columns)>,
}

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

/// Synthetic transfer columns on `grid`.
fn synthetic(grid: &GridDef) -> Columns {
    let f = frequencies(grid);
    Columns {
        mag_db: f
            .iter()
            .map(|f| {
                let x = (f / 80.0).powi(2);
                (-6.0 + 10.0 * (x * x / (1.0 + x * x)).log10()) as f32
            })
            .collect(),
        phase_deg: Some(
            f.iter()
                .map(|f| (180.0 * 80.0 / (f + 80.0)) as f32)
                .collect(),
        ),
        coherence: Some(vec![0.95; f.len()]),
    }
}

impl Shared {
    fn trace(&self, id: TraceId) -> Result<StoredTrace, ProtoError> {
        let meta = self
            .state
            .traces
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no trace {id}")))?;
        let (grid, columns) = self
            .traces
            .data
            .get(&id)
            .cloned()
            .ok_or_else(|| err(ErrorCode::Internal, format!("trace {id} has no data")))?;
        Ok(StoredTrace {
            meta,
            grid,
            columns,
            sweep: None,
        })
    }

    fn add_trace(&mut self, meta: TraceMeta, grid: GridDef, columns: Columns) -> ReplyBody {
        self.grids.insert(grid.id(), grid.clone());
        for other in meta::take_slot(&self.state.traces, meta.id, meta.edit.slot) {
            self.commit(Change::Trace(Patch::Set(other)));
        }
        self.traces.data.insert(meta.id, (grid, columns));
        self.commit(Change::Trace(Patch::Set(meta.clone())));
        ReplyBody::Trace(meta)
    }

    fn alloc_trace(&mut self) -> TraceId {
        let id = TraceId(self.next_id);
        self.next_id += 1;
        id
    }

    fn new_meta(
        id: TraceId,
        name: String,
        slot: Option<u8>,
        kind: TraceKind,
        source: TraceSource,
        grid: &GridDef,
        delay: Seconds,
    ) -> TraceMeta {
        TraceMeta {
            id,
            edit: meta::new_edit(id, name, slot),
            kind,
            source,
            grid_id: grid.id(),
            delay,
            depth: None,
            cal: CalState::Uncalibrated,
            mic: None,
            created_at: WallNs(1_790_000_000_000_000_000),
        }
    }

    pub(super) fn trace_capture(
        &mut self,
        meas: MeasId,
        name: String,
        slot: Option<u8>,
    ) -> Result<ReplyBody, ProtoError> {
        meta::check_edit(&name, slot).map_err(|e| err(ErrorCode::Invalid, e))?;
        let m = self.meas(meas)?;
        let MeasKind::Transfer { config } = &m.config.kind else {
            return Err(err(
                ErrorCode::Invalid,
                "the fake captures transfer measurements only",
            ));
        };
        let grid = GridDef::Log {
            ppo: config.grid.ppo,
            k_min: config.grid.k_min,
            k_max: config.grid.k_max,
        };
        let id = self.alloc_trace();
        let mut t = Self::new_meta(
            id,
            name,
            slot,
            TraceKind::Transfer,
            TraceSource::Captured {
                meas,
                meas_name: m.config.name.clone(),
                epoch: self.state.session.epoch,
                at_sample: SampleIndex(48_000),
            },
            &grid,
            m.delay.as_ref().map_or(Seconds(0.0), |d| d.applied),
        );
        t.edit.smoothing = config.smoothing;
        t.depth = Some(config.depth);
        let c = synthetic(&grid);
        Ok(self.add_trace(t, grid, c))
    }

    pub(super) fn trace_get(&self, id: TraceId) -> Result<ReplyBody, ProtoError> {
        Ok(ReplyBody::TraceData(self.trace(id)?.data()))
    }

    pub(super) fn trace_update(
        &mut self,
        id: TraceId,
        edit: TraceEdit,
    ) -> Result<ReplyBody, ProtoError> {
        meta::check_values(&edit).map_err(|e| err(ErrorCode::Invalid, e))?;
        let mut t = self.trace(id)?.meta;
        if !meta::lock_allows(&t.edit, &edit) {
            return Err(err(ErrorCode::Refused, format!("trace {id} is locked")));
        }
        for other in meta::take_slot(&self.state.traces, id, edit.slot) {
            self.commit(Change::Trace(Patch::Set(other)));
        }
        t.edit = edit;
        self.commit(Change::Trace(Patch::Set(t.clone())));
        Ok(ReplyBody::Trace(t))
    }

    pub(super) fn trace_delete(&mut self, id: TraceId) -> Result<ReplyBody, ProtoError> {
        let t = self.trace(id)?;
        if t.meta.edit.locked {
            return Err(err(ErrorCode::Refused, format!("trace {id} is locked")));
        }
        self.traces.data.remove(&id);
        let rev = self.commit(Change::Trace(Patch::Deleted(id)));
        Ok(ReplyBody::Ack { rev })
    }

    fn derived(
        &mut self,
        name: String,
        source: TraceSource,
        d: Derived,
    ) -> Result<ReplyBody, ProtoError> {
        meta::check_edit(&name, None).map_err(|e| err(ErrorCode::Invalid, e))?;
        let id = self.alloc_trace();
        let t = Self::new_meta(id, name, None, d.kind, source, &d.grid, d.delay);
        Ok(self.add_trace(t, d.grid, d.columns))
    }

    pub(super) fn trace_average(
        &mut self,
        ids: Vec<TraceId>,
        method: AverageMethod,
        reference: DelayReference,
        name: String,
    ) -> Result<ReplyBody, ProtoError> {
        let inputs = ids
            .iter()
            .map(|i| self.trace(*i))
            .collect::<Result<Vec<_>, _>>()?;
        let refs: Vec<&StoredTrace> = inputs.iter().collect();
        let d = ops::average(&refs, method, reference)
            .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
        self.derived(
            name,
            TraceSource::Average {
                traces: ids,
                method,
                reference,
            },
            d,
        )
    }

    pub(super) fn trace_math(
        &mut self,
        a: TraceId,
        b: TraceId,
        op: MathOp,
        name: String,
    ) -> Result<ReplyBody, ProtoError> {
        let d = ops::math(&self.trace(a)?, &self.trace(b)?, op)
            .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
        self.derived(name, TraceSource::Math { a, b, op }, d)
    }

    pub(super) fn trace_import(
        &mut self,
        file_name: String,
        format: ImportFormat,
        role: ImportRole,
        content: &[u8],
    ) -> Result<ReplyBody, ProtoError> {
        let imp = text::import(content, format, role).map_err(|e| ProtoError {
            code: ErrorCode::Invalid,
            msg: e.to_string(),
            detail: Some(ErrorDetail::Import {
                line: e.line,
                problem: e.problem,
            }),
        })?;
        let name = imp.name.clone().unwrap_or_else(|| {
            std::path::Path::new(&file_name)
                .file_stem()
                .map_or_else(|| file_name.clone(), |s| s.to_string_lossy().into_owned())
        });
        let id = self.alloc_trace();
        let t = Self::new_meta(
            id,
            name,
            None,
            imp.kind,
            TraceSource::Imported {
                file_name,
                format: imp.format,
            },
            &imp.grid,
            Seconds(0.0),
        );
        Ok(self.add_trace(t, imp.grid, imp.columns))
    }

    pub(super) fn trace_export(&self, id: TraceId) -> Result<ReplyBody, ProtoError> {
        let t = self.trace(id)?;
        Ok(ReplyBody::Export {
            file_name: meta::file_name(&t.meta.edit.name, "csv"),
            content: Blob(text::export_csv(&t).into_bytes()),
        })
    }

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
                    frozen: m.frozen,
                    delay: m.delay.as_ref().map(|d| SavedDelay {
                        applied: d.applied,
                        tracking: d.tracking,
                    }),
                })
                .collect(),
            traces,
        };
        let m = session::save(&dir, &s).map_err(session_err)?;
        Ok(ReplyBody::SessionFile(Self::file_info(name, &dir, &m)))
    }

    pub(super) fn file_load(
        &mut self,
        client: &ClientId,
        r: &SessionRef,
    ) -> Result<ReplyBody, ProtoError> {
        let (name, dir) = self.session_path(r)?;
        let m = session::read_manifest(&dir).map_err(session_err)?;
        let s = session::load(&dir).map_err(session_err)?;
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
            self.commit(Change::Measurement(Patch::Deleted(id)));
        }
        for id in self.state.traces.iter().map(|t| t.id).collect::<Vec<_>>() {
            self.commit(Change::Trace(Patch::Deleted(id)));
        }
        self.traces.data.clear();
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
        };
        self.commit(Change::Session(session));
        for sm in s.measurements {
            self.next_id = self.next_id.max(sm.id.0 + 1);
            let grid_id = match &sm.config.kind {
                MeasKind::Transfer { config } => {
                    let g = GridDef::Log {
                        ppo: config.grid.ppo,
                        k_min: config.grid.k_min,
                        k_max: config.grid.k_max,
                    };
                    let id = g.id();
                    self.grids.insert(id, g);
                    Some(id)
                }
                _ => None,
            };
            let delay = sm.delay.map(|d| DelayState {
                applied: d.applied,
                applied_samples: Samples((d.applied.0 * 48_000.0).round() as i64),
                tracking: d.tracking,
                awaiting_pick: false,
                last_finding: None,
            });
            let meas = Measurement {
                id: sm.id,
                config: sm.config,
                config_rev: Rev(self.rev.0 + 1),
                running: sm.running,
                frozen: sm.frozen,
                delay,
                grid_id,
            };
            self.commit(Change::Measurement(Patch::Set(meas)));
        }
        for t in s.traces {
            self.next_id = self.next_id.max(t.meta.id.0 + 1);
            self.grids.insert(t.grid.id(), t.grid.clone());
            self.traces.data.insert(t.meta.id, (t.grid, t.columns));
            self.commit(Change::Trace(Patch::Set(t.meta)));
        }
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
