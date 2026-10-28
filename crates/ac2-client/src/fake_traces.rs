//! Trace and session commands of the fake daemon, on the same `ac2-traces` code the real
//! daemon uses. Captures are synthetic: a flat −6 dB response with a 2nd-order roll-off
//! below 80 Hz, the measurement's delay compensated, coherence 0.95. A sweep run (`sweep.run`)
//! is stored at once (no audio): that response with H2 at −40 dB rising 12 dB/oct below
//! 100 Hz, H3 at −50 dB, H4 and H5 in the noise, the floor at −80 dB.

use std::collections::HashMap;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, ErrorCode, ErrorDetail, GridDef, Patch, ProtoError, ReplyBody};
use ac2_traces::columns::{Columns, StoredTrace};
use ac2_traces::meta;
use ac2_traces::ops::{self, Derived};
use ac2_traces::text;

use super::{Shared, err};

/// Trace data held by the fake.
#[derive(Debug, Default)]
pub(super) struct FakeTraces {
    pub(super) data: HashMap<TraceId, (GridDef, Columns)>,
    pub(super) sweeps: HashMap<TraceId, SweepData>,
    /// Impulse responses of transfer traces (imported IR files).
    pub(super) irs: HashMap<TraceId, TransferIr>,
    pub(super) next_sweep: u32,
    /// Mic curves applied to traces after capture.
    pub(super) mic_curves: HashMap<TraceId, ac2_traces::mic::Correction>,
    /// Points (Hz, dB) of the imported mic curves.
    pub(super) curve_points: HashMap<MicCurveId, Vec<[f64; 2]>>,
}

/// Synthetic transfer columns on `grid`.
pub(super) fn synthetic(grid: &GridDef) -> Columns {
    let f = grid.centres();
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
    pub(super) fn trace(&self, id: TraceId) -> Result<StoredTrace, ProtoError> {
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
            sweep: self.traces.sweeps.get(&id).cloned(),
            ir: self.traces.irs.get(&id).cloned(),
            mic_curve: self.traces.mic_curves.get(&id).cloned(),
        })
    }

    pub(super) fn add_trace(
        &mut self,
        meta: TraceMeta,
        grid: GridDef,
        columns: Columns,
    ) -> ReplyBody {
        self.grids.insert(grid.id(), grid.clone());
        for other in meta::take_slot(&self.state.traces, meta.id, meta.edit.slot) {
            self.commit(Change::Trace(Patch::Set(other)));
        }
        self.traces.data.insert(meta.id, (grid, columns));
        self.commit(Change::Trace(Patch::Set(meta.clone())));
        ReplyBody::Trace(meta)
    }
    /// Adds a stored trace with its columns, as a test's fixture (the daemon's own kinds of
    /// trace, such as an EQ set's curve, that the fake never makes itself).
    pub fn put_trace(&mut self, meta: TraceMeta, grid: GridDef, columns: Columns) {
        self.add_trace(meta, grid, columns);
    }

    pub(super) fn alloc_trace(&mut self) -> TraceId {
        let id = TraceId(self.next_id);
        self.next_id += 1;
        id
    }

    pub(super) fn new_meta(
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
            edit: meta::new_edit(id, name, slot, TraceOwner::Imported),
            kind,
            source,
            grid_id: grid.id(),
            delay,
            depth: None,
            cal: CalState::Uncalibrated,
            mic: None,
            mic_curve: None,
            created_at: WallNs(1_790_000_000_000_000_000),
            eq_in: Vec::new(),
            position: None,
            captured_unsettled: false,
            superseded: false,
        }
    }

    /// As the daemon: a trace is filed under a measurement that exists and is no math
    /// channel, or under Imported.
    fn check_trace_owner(&self, owner: TraceOwner) -> Result<(), ProtoError> {
        let Some(meas) = owner.meas() else {
            return Ok(());
        };
        let m = self.state.measurements.iter().find(|m| m.id == meas);
        match m.map(|m| &m.config.kind) {
            None => Err(err(
                ErrorCode::NotFound,
                format!("no measurement {meas} to file it under"),
            )),
            Some(ac2_proto::model::MeasKind::Math { .. }) => Err(err(
                ErrorCode::Invalid,
                format!("measurement {meas} is a math channel: math owns no traces"),
            )),
            Some(_) => Ok(()),
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
            ppo: config.grid().ppo,
            k_min: config.grid().k_min,
            k_max: config.grid().k_max,
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
        t.edit.delay_nudge = m.delay.as_ref().map_or(Seconds(0.0), |d| d.nudged);
        t.edit.owner = TraceOwner::Meas { meas };
        t.depth = Some(config.depth);
        let c = synthetic(&grid);
        Ok(self.add_trace(t, grid, c))
    }

    pub(super) fn trace_get(&self, id: TraceId) -> Result<ReplyBody, ProtoError> {
        Ok(ReplyBody::TraceData(Box::new(self.trace(id)?.data())))
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
        self.traces.sweeps.remove(&id);
        self.traces.irs.remove(&id);
        self.traces.mic_curves.remove(&id);
        let rev = self.commit(Change::Trace(Patch::Deleted(id)));
        let rev = self.prune_compared().unwrap_or(rev);
        Ok(ReplyBody::Ack { rev })
    }

    /// Drops what is gone from every compare list, as the daemon does.
    pub(super) fn prune_compared(&mut self) -> Option<Rev> {
        let changed: Vec<Measurement> = self
            .state
            .measurements
            .iter()
            .filter_map(|m| {
                let keep = |c: &Compared| match *c {
                    Compared::Meas { meas } => self.state.measurements.iter().any(|o| o.id == meas),
                    Compared::Trace { trace } => self.state.traces.iter().any(|t| t.id == trace),
                };
                let mut m = m.clone();
                let n = m.compared.len();
                m.compared.retain(keep);
                (m.compared.len() != n).then_some(m)
            })
            .collect();
        let mut rev = None;
        for m in changed {
            rev = Some(self.commit(Change::Measurement(Patch::Set(m))));
        }
        rev
    }

    fn derived(
        &mut self,
        name: String,
        source: TraceSource,
        d: Derived,
        owner: TraceOwner,
    ) -> Result<ReplyBody, ProtoError> {
        meta::check_edit(&name, None).map_err(|e| err(ErrorCode::Invalid, e))?;
        let id = self.alloc_trace();
        let mut t = Self::new_meta(id, name, None, d.kind, source, &d.grid, d.delay);
        t.edit.owner = owner;
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
        // As the daemon: with its inputs when they share an owner.
        let owner = refs
            .first()
            .map_or(TraceOwner::Imported, |t| t.meta.edit.owner);
        let owner = if refs.iter().all(|t| t.meta.edit.owner == owner) {
            owner
        } else {
            TraceOwner::Imported
        };
        self.derived(
            name,
            TraceSource::Average {
                traces: ids,
                method,
                reference,
            },
            d,
            owner,
        )
    }

    pub(super) fn trace_import(
        &mut self,
        file_name: String,
        format: ImportFormat,
        role: ImportRole,
        ir: Option<IrImport>,
        content: &[u8],
        owner: TraceOwner,
    ) -> Result<ReplyBody, ProtoError> {
        self.check_trace_owner(owner)?;
        let imp = ac2_traces::import_file(content, format, role, ir).map_err(|e| ProtoError {
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
        let mut t = Self::new_meta(
            id,
            name,
            None,
            imp.kind,
            TraceSource::Imported {
                file_name,
                format: imp.format,
                notes: imp.notes,
                ir_file: imp.ir_file,
            },
            &imp.grid,
            imp.delay.unwrap_or(Seconds(0.0)),
        );
        t.edit.owner = owner;
        if let Some(sw) = imp.sweep {
            self.traces.sweeps.insert(id, sw);
        }
        if let Some(ir) = imp.ir {
            self.traces.irs.insert(id, ir);
        }
        Ok(self.add_trace(t, imp.grid, imp.columns))
    }

    /// `trace.target`, as the daemon: the same columns, filed under `owner`.
    pub(super) fn trace_target(
        &mut self,
        curve: TargetCurve,
        name: String,
        owner: TraceOwner,
    ) -> Result<ReplyBody, ProtoError> {
        self.check_trace_owner(owner)?;
        let (grid, columns) =
            ac2_traces::target::generate(&curve).map_err(|e| err(ErrorCode::Invalid, e))?;
        let id = self.alloc_trace();
        let mut t = Self::new_meta(
            id,
            name,
            None,
            TraceKind::Target,
            TraceSource::Generated { curve },
            &grid,
            Seconds(0.0),
        );
        t.edit.owner = owner;
        Ok(self.add_trace(t, grid, columns))
    }

    /// `trace.mic_curve`, as the daemon: the named curve of the mic library, normalised at
    /// the trace's calibrator frequency (1 kHz without one).
    pub(super) fn trace_mic_curve(
        &mut self,
        id: TraceId,
        curve: Option<MicCurveId>,
    ) -> Result<ReplyBody, ProtoError> {
        let mut t = self.trace(id)?.meta;
        ac2_traces::mic::check(&t).map_err(|e| {
            let code = match e {
                ac2_traces::mic::MicCurveError::Target => ErrorCode::Invalid,
                _ => ErrorCode::Refused,
            };
            err(code, format!("trace {id}: {e}"))
        })?;
        match curve {
            None => {
                if t.mic_curve.take().is_none() {
                    return Err(err(
                        ErrorCode::NotFound,
                        format!("trace {id} has no mic curve applied after capture"),
                    ));
                }
                self.traces.mic_curves.remove(&id);
            }
            Some(m) => {
                let r = ac2_proto::cal::curve(&self.state.mics, &m.mic, &m.label)
                    .cloned()
                    .ok_or_else(|| {
                        err(
                            ErrorCode::NotFound,
                            format!(
                                "no mic curve {:?} of {:?} in the mic library",
                                m.label, m.mic
                            ),
                        )
                    })?;
                let points = self.traces.curve_points.get(&m).ok_or_else(|| {
                    err(
                        ErrorCode::Internal,
                        format!("the mic curve {m:?} has no points"),
                    )
                })?;
                let f_norm = match &t.cal {
                    CalState::Calibrated { key, .. } => self
                        .state
                        .calibrations
                        .iter()
                        .find(|e| e.key == *key)
                        .map(|e| ac2_proto::cal::f_norm(&e.spl).0),
                    CalState::Uncalibrated => None,
                }
                .unwrap_or(1000.0);
                let k = ac2_traces::mic::correction(points, f_norm)
                    .map_err(|x| err(ErrorCode::Internal, x))?;
                t.mic_curve = Some(Box::new(TraceMicCurve {
                    mic: m.mic,
                    curve: r,
                    f_norm: Hz(f_norm),
                }));
                self.traces.mic_curves.insert(id, k);
            }
        }
        self.commit(Change::Trace(Patch::Set(t.clone())));
        Ok(ReplyBody::Trace(t))
    }

    pub(super) fn trace_export(
        &self,
        id: TraceId,
        format: ExportFormat,
    ) -> Result<ReplyBody, ProtoError> {
        let t = self.trace(id)?;
        Ok(match format {
            ExportFormat::Ac2Csv => ReplyBody::Export {
                file_name: meta::file_name(&t.meta.edit.name, "csv"),
                content: Blob(text::export_csv(&t).into_bytes()),
            },
            ExportFormat::IrWav => ReplyBody::Export {
                file_name: meta::file_name(&t.meta.edit.name, "wav"),
                content: Blob(
                    ac2_traces::ir_wav::export(&t)
                        .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?,
                ),
            },
        })
    }
}
