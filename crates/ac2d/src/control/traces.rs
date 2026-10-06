//! The trace store and the `trace.*` commands.
//!
//! Metadata lives in the mirrored state (so every client sees it through events); the
//! columns and grid of every trace live here and leave only by `trace.get`, `trace.export`
//! and `file.save`.

use std::collections::BTreeMap;

use ac2_core::mic_curve::Correction;
use ac2_proto::event::{Change, Patch};
use ac2_proto::frame::{Frame, MathState, OperandStatus};
use ac2_proto::model::{
    AverageMethod, CalState, DelayReference, ExportFormat, ImportFormat, ImportRole, MathConfig,
    MathExpr, MeasKind, Measurement, MicCurveId, MicState, NamedOperand, Operand, Smoothing,
    SmoothingMode, SweepData, TraceEdit, TraceKind, TraceMeta, TraceMicCurve, TraceSource,
};
use ac2_proto::units::{Hz, MeasId, Seconds, TraceId, WallNs};
use ac2_proto::{ErrorCode, ErrorDetail, FrameData, GridDef, ProtoError, ReplyBody};
use ac2_traces::columns::{Columns, StoredTrace};
use ac2_traces::meta;
use ac2_traces::mic::{self, MicCurveError};
use ac2_traces::ops::{self, Derived, OpError};
use ac2_traces::text::{self, ImportError};

use super::Control;
use crate::util::{perr, perr_detail, wall_ns};

/// Columns and grids of the stored traces, by id.
#[derive(Debug, Default)]
pub(crate) struct TraceStore {
    data: BTreeMap<TraceId, (GridDef, Columns)>,
    /// Distortion and IR of sweep traces.
    sweeps: BTreeMap<TraceId, SweepData>,
    /// Mic curves applied after capture.
    mic_curves: BTreeMap<TraceId, Correction>,
    next_id: u32,
}

impl TraceStore {
    pub(crate) fn alloc(&mut self) -> TraceId {
        self.next_id = self.next_id.max(1);
        let id = TraceId(self.next_id);
        self.next_id += 1;
        id
    }

    pub(crate) fn insert(
        &mut self,
        id: TraceId,
        grid: GridDef,
        columns: Columns,
        sweep: Option<SweepData>,
        mic_curve: Option<Correction>,
    ) {
        self.next_id = self.next_id.max(id.0.saturating_add(1));
        self.data.insert(id, (grid, columns));
        match sweep {
            Some(s) => {
                self.sweeps.insert(id, s);
            }
            None => {
                self.sweeps.remove(&id);
            }
        }
        self.set_mic_curve(id, mic_curve);
    }

    pub(crate) fn set_mic_curve(&mut self, id: TraceId, c: Option<Correction>) {
        match c {
            Some(c) => {
                self.mic_curves.insert(id, c);
            }
            None => {
                self.mic_curves.remove(&id);
            }
        }
    }

    pub(crate) fn remove(&mut self, id: TraceId) {
        self.data.remove(&id);
        self.sweeps.remove(&id);
        self.mic_curves.remove(&id);
    }

    pub(crate) fn clear(&mut self) {
        self.data.clear();
        self.sweeps.clear();
        self.mic_curves.clear();
    }

    pub(crate) fn get(&self, id: TraceId) -> Option<&(GridDef, Columns)> {
        self.data.get(&id)
    }
}

fn op_err(e: &OpError) -> ProtoError {
    perr(ErrorCode::Invalid, e.to_string())
}

pub(crate) fn import_err(e: ImportError) -> ProtoError {
    perr_detail(
        ErrorCode::Invalid,
        e.to_string(),
        ErrorDetail::Import {
            line: e.line,
            problem: e.problem,
        },
    )
}

/// Smoothing a derived trace of `kind` shows: its inputs' common smoothing. The inputs are
/// combined unsmoothed either way.
fn common_smoothing(inputs: &[&StoredTrace], kind: TraceKind) -> Option<Smoothing> {
    let first = inputs.first()?.meta.edit.smoothing;
    (ac2_traces::smooth::smoothable(kind) && inputs.iter().all(|t| t.meta.edit.smoothing == first))
        .then_some(first)
        .flatten()
}

fn no_trace(id: TraceId) -> ProtoError {
    perr(ErrorCode::NotFound, format!("no trace {id}"))
}

impl Control {
    fn trace_meta(&self, id: TraceId) -> Result<&TraceMeta, ProtoError> {
        self.store
            .state()
            .traces
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| no_trace(id))
    }

    /// Metadata, grid and columns of a stored trace.
    pub(super) fn stored(&self, id: TraceId) -> Result<StoredTrace, ProtoError> {
        let meta = self.trace_meta(id)?.clone();
        let (grid, columns) = self
            .traces
            .get(id)
            .cloned()
            .ok_or_else(|| perr(ErrorCode::Internal, format!("trace {id} has no data")))?;
        Ok(StoredTrace {
            meta,
            grid,
            columns,
            sweep: self.traces.sweeps.get(&id).cloned(),
            mic_curve: self.traces.mic_curves.get(&id).cloned(),
        })
    }

    /// Commits a new trace (taking its slot from any other trace) and stores its data.
    pub(super) fn add_trace(
        &mut self,
        meta: TraceMeta,
        grid: GridDef,
        columns: Columns,
        sweep: Option<SweepData>,
    ) -> ReplyBody {
        let gid = self.register_grid(grid.clone());
        debug_assert_eq!(gid, meta.grid_id);
        for other in meta::take_slot(&self.store.state().traces, meta.id, meta.edit.slot) {
            self.commit(Change::Trace(Patch::Set(other)));
        }
        self.traces.insert(meta.id, grid, columns, sweep, None);
        self.commit(Change::Trace(Patch::Set(meta.clone())));
        ReplyBody::Trace(meta)
    }

    /// Calibration and mic of `input` at capture (`docs/design/q7-calibration.md` §3): the
    /// input setup's mic name with the curve applied to it, and the sensitivity calibration
    /// the matching rules pick. A calibration of another mic or input keeps its own key, so
    /// the trace shows that it was not this mic's.
    pub(super) fn cal_and_mic(&self, input: u16) -> (CalState, Option<MicState>) {
        let Some(rt) = self.session.as_ref() else {
            return (CalState::Uncalibrated, None);
        };
        let c = self.input_cal(rt, input);
        let cal = match c.spl_entry {
            Some(e) => CalState::Calibrated {
                key: e.key,
                sensitivity: e.spl.sensitivity,
                calibrated_at: e.spl.calibrated_at,
            },
            None => CalState::Uncalibrated,
        };
        let mic = ac2_proto::cal::input_setup(&self.store.state().inputs, input)
            .mic
            .map(|name| MicState {
                name,
                curve: c.curve,
            });
        (cal, mic)
    }

    pub(super) fn trace_capture(
        &mut self,
        meas: MeasId,
        name: String,
        slot: Option<u8>,
    ) -> Result<ReplyBody, ProtoError> {
        meta::check_edit(&name, slot).map_err(|e| perr(ErrorCode::Invalid, e))?;
        let m = self.meas(meas)?.clone();
        let frame = self
            .jobs
            .get(&meas)
            .and_then(super::JobHandle::capture)
            .ok_or_else(|| {
                perr(
                    ErrorCode::Invalid,
                    format!(
                        "{}: no result to capture yet (the measurement must be running)",
                        m.config.name
                    ),
                )
            })?;
        if frame.stamp.session_epoch != self.epoch() {
            return Err(perr(
                ErrorCode::Invalid,
                "the newest result is from an earlier session epoch",
            ));
        }
        let (kind, columns) = ops::capture_columns(&frame.data).ok_or_else(|| {
            perr(
                ErrorCode::Invalid,
                "only transfer, spectrum and RTA measurements can be captured",
            )
        })?;
        let grid = frame
            .stamp
            .grid_id
            .and_then(|g| self.grids.get(&g).cloned())
            .ok_or_else(|| perr(ErrorCode::Internal, "the frame's grid is not registered"))?;
        if let MeasKind::Math { config } = &m.config.kind {
            return self.capture_math(&m, config, &frame, kind, grid, columns, name, slot);
        }
        let (delay, smoothing, depth, input): (Seconds, Option<Smoothing>, _, u16) =
            match (&frame.data, &m.config.kind) {
                (FrameData::Tf(f), MeasKind::Transfer { config }) => (
                    f.meta.delay,
                    f.meta.smoothing,
                    Some(config.depth),
                    config.measurement_input,
                ),
                (FrameData::Spec(f), MeasKind::Spectrum { config }) => (
                    Seconds(0.0),
                    f.meta.smoothing.map(|fraction| Smoothing {
                        fraction,
                        mode: SmoothingMode::Magnitude,
                    }),
                    None,
                    config.input,
                ),
                (_, MeasKind::Rta { config }) => (Seconds(0.0), None, None, config.input),
                _ => {
                    return Err(perr(
                        ErrorCode::Invalid,
                        "the measurement changed kind since its last result",
                    ));
                }
            };
        let (cal, mic) = self.cal_and_mic(input);
        // A transfer function is a ratio of two inputs: no calibration applies to it.
        let cal = if kind == TraceKind::Transfer {
            CalState::Uncalibrated
        } else {
            cal
        };
        let id = self.traces.alloc();
        let mut edit = meta::new_edit(id, name, slot);
        // The capture holds the unsmoothed curve and shows it as the measurement did.
        edit.smoothing = smoothing;
        let t = TraceMeta {
            id,
            edit,
            kind,
            source: TraceSource::Captured {
                meas,
                meas_name: m.config.name.clone(),
                epoch: frame.stamp.session_epoch,
                at_sample: frame.stamp.audio_sample,
            },
            grid_id: grid.id(),
            delay,
            depth,
            cal,
            mic,
            mic_curve: None,
            created_at: WallNs(wall_ns()),
        };
        tracing::info!("trace {id} captured from measurement {meas}");
        Ok(self.add_trace(t, grid, columns, None))
    }

    /// The name of math operand `o` as the state names it now.
    pub(super) fn operand_name(&self, o: Operand) -> String {
        match o {
            Operand::Meas { meas } => self
                .lookup(meas)
                .map_or_else(|| format!("measurement {meas}"), |(n, _)| n.to_owned()),
            Operand::Trace { trace } => self
                .store
                .state()
                .traces
                .iter()
                .find(|t| t.id == trace)
                .map_or_else(|| format!("trace {trace}"), |t| t.edit.name.clone()),
        }
    }

    /// `trace.capture` of a math channel: the trace names the expression and the operands
    /// that went into it (an average's operands left out at that moment are not named). A
    /// combination of several inputs, it names no single mic or calibration.
    #[allow(clippy::too_many_arguments)]
    fn capture_math(
        &mut self,
        m: &Measurement,
        config: &MathConfig,
        frame: &Frame,
        kind: TraceKind,
        grid: GridDef,
        columns: Columns,
        name: String,
        slot: Option<u8>,
    ) -> Result<ReplyBody, ProtoError> {
        let (state, delay, smoothing): (Option<&MathState>, Seconds, Option<Smoothing>) =
            match &frame.data {
                FrameData::Tf(f) => (f.meta.math.as_deref(), f.meta.delay, f.meta.smoothing),
                FrameData::Spec(f) => (
                    f.meta.math.as_deref(),
                    Seconds(0.0),
                    f.meta.smoothing.map(|fraction| Smoothing {
                        fraction,
                        mode: SmoothingMode::Magnitude,
                    }),
                ),
                FrameData::Rta(f) => (f.meta.math.as_deref(), Seconds(0.0), None),
                _ => (None, Seconds(0.0), None),
            };
        let state = state.ok_or_else(|| {
            perr(
                ErrorCode::Internal,
                "the math channel's frame does not say what it combined",
            )
        })?;
        let included: Vec<NamedOperand> = state
            .operands
            .iter()
            .filter(|s| s.status == OperandStatus::Included)
            .map(|s| NamedOperand {
                operand: s.operand,
                name: self.operand_name(s.operand),
            })
            .collect();
        let enough = match &config.expr {
            MathExpr::Binary { .. } => included.len() == 2,
            MathExpr::Average { .. } => included.len() >= MathConfig::MIN_AVERAGE,
        };
        if !enough || columns.mag_db.iter().all(|v| v.is_nan()) {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "{}: its operands have no usable result together; nothing to capture",
                    m.config.name
                ),
            ));
        }
        let id = self.traces.alloc();
        let mut edit = meta::new_edit(id, name, slot);
        edit.smoothing = smoothing;
        let t = TraceMeta {
            id,
            edit,
            kind,
            source: TraceSource::Math {
                meas: m.id,
                meas_name: m.config.name.clone(),
                epoch: frame.stamp.session_epoch,
                at_sample: frame.stamp.audio_sample,
                expr: config.expr.clone(),
                operands: included,
                phase: state.phase,
            },
            grid_id: grid.id(),
            delay,
            depth: None,
            cal: CalState::Uncalibrated,
            mic: None,
            mic_curve: None,
            created_at: WallNs(wall_ns()),
        };
        tracing::info!("trace {id} captured from math channel {}", m.id);
        Ok(self.add_trace(t, grid, columns, None))
    }

    pub(super) fn trace_get(&self, id: TraceId) -> Result<ReplyBody, ProtoError> {
        Ok(ReplyBody::TraceData(Box::new(self.stored(id)?.data())))
    }

    pub(super) fn trace_update(
        &mut self,
        id: TraceId,
        edit: TraceEdit,
    ) -> Result<ReplyBody, ProtoError> {
        meta::check_values(&edit).map_err(|e| perr(ErrorCode::Invalid, e))?;
        let mut t = self.trace_meta(id)?.clone();
        if edit.smoothing.is_some() && !ac2_traces::smooth::smoothable(t.kind) {
            return Err(perr(
                ErrorCode::Invalid,
                "smoothing applies to transfer and spectrum traces only",
            ));
        }
        if !meta::lock_allows(&t.edit, &edit) {
            return Err(perr(
                ErrorCode::Refused,
                format!("trace {id} is locked; unlock it to change its curve"),
            ));
        }
        for other in meta::take_slot(&self.store.state().traces, id, edit.slot) {
            self.commit(Change::Trace(Patch::Set(other)));
        }
        t.edit = edit;
        self.commit(Change::Trace(Patch::Set(t.clone())));
        Ok(ReplyBody::Trace(t))
    }

    pub(super) fn trace_delete(&mut self, id: TraceId) -> Result<ReplyBody, ProtoError> {
        let t = self.trace_meta(id)?;
        if t.edit.locked {
            return Err(perr(
                ErrorCode::Refused,
                format!("trace {id} is locked; unlock it first"),
            ));
        }
        self.check_trace_operand_delete(id)?;
        self.traces.remove(id);
        let rev = self.commit(Change::Trace(Patch::Deleted(id)));
        Ok(ReplyBody::Ack { rev })
    }

    fn derived(
        &mut self,
        name: String,
        source: TraceSource,
        d: Derived,
        inputs: &[&StoredTrace],
        smoothing: Option<Smoothing>,
    ) -> Result<ReplyBody, ProtoError> {
        meta::check_edit(&name, None).map_err(|e| perr(ErrorCode::Invalid, e))?;
        let first = inputs.first().map(|t| &t.meta);
        let same = |f: &dyn Fn(&TraceMeta) -> bool| inputs.iter().all(|t| f(&t.meta));
        let cal = first.map_or(CalState::Uncalibrated, |m| m.cal.clone());
        let cal = if !ops::transfer_like(d.kind) && same(&|m| m.cal == cal) {
            cal
        } else {
            CalState::Uncalibrated
        };
        let mic = first.and_then(|m| m.mic.clone());
        let mic = if same(&|m| m.mic == mic) { mic } else { None };
        let id = self.traces.alloc();
        let mut edit = meta::new_edit(id, name, None);
        edit.smoothing = smoothing;
        let t = TraceMeta {
            id,
            edit,
            kind: d.kind,
            source,
            grid_id: d.grid.id(),
            delay: d.delay,
            depth: None,
            cal,
            mic,
            mic_curve: None,
            created_at: WallNs(wall_ns()),
        };
        Ok(self.add_trace(t, d.grid, d.columns, None))
    }

    pub(super) fn trace_average(
        &mut self,
        ids: &[TraceId],
        method: AverageMethod,
        reference: DelayReference,
        name: String,
    ) -> Result<ReplyBody, ProtoError> {
        // Applied mic curves go into the inputs' columns (and their `mic`), so the result
        // says which curve its columns carry.
        let inputs = ids
            .iter()
            .map(|id| self.stored(*id).map(|t| mic::bake(&t)))
            .collect::<Result<Vec<_>, _>>()?;
        let refs: Vec<&StoredTrace> = inputs.iter().collect();
        let d = ops::average(&refs, method, reference).map_err(|e| op_err(&e))?;
        let source = TraceSource::Average {
            traces: ids.to_vec(),
            method,
            reference,
        };
        let smoothing = common_smoothing(&refs, d.kind);
        self.derived(name, source, d, &refs, smoothing)
    }

    pub(super) fn trace_import(
        &mut self,
        file_name: String,
        format: ImportFormat,
        role: ImportRole,
        content: &[u8],
    ) -> Result<ReplyBody, ProtoError> {
        let imp = text::import(content, format, role).map_err(import_err)?;
        let name = imp.name.clone().unwrap_or_else(|| {
            std::path::Path::new(&file_name)
                .file_stem()
                .map_or_else(|| file_name.clone(), |s| s.to_string_lossy().into_owned())
        });
        let name: String = name.chars().take(meta::MAX_NAME).collect();
        let name = if name.trim().is_empty() {
            "imported".to_owned()
        } else {
            name
        };
        let id = self.traces.alloc();
        let t = TraceMeta {
            id,
            edit: meta::new_edit(id, name, None),
            kind: imp.kind,
            source: TraceSource::Imported {
                file_name,
                format: imp.format,
                notes: imp.notes,
            },
            grid_id: imp.grid.id(),
            delay: imp.delay.unwrap_or(Seconds(0.0)),
            depth: None,
            cal: CalState::Uncalibrated,
            mic: None,
            mic_curve: None,
            created_at: WallNs(wall_ns()),
        };
        tracing::info!("trace {id} imported ({} rows)", imp.rows);
        Ok(self.add_trace(t, imp.grid, imp.columns, imp.sweep))
    }

    /// `trace.mic_curve`: puts the curve the calibration store holds for `mic` on the
    /// trace (a display edit), or takes the applied one off (`None`).
    pub(super) fn trace_mic_curve(
        &mut self,
        id: TraceId,
        curve: Option<MicCurveId>,
    ) -> Result<ReplyBody, ProtoError> {
        let mut t = self.trace_meta(id)?.clone();
        mic::check(&t).map_err(|e| {
            let code = match e {
                MicCurveError::Target => ErrorCode::Invalid,
                MicCurveError::InColumns { .. } | MicCurveError::Locked => ErrorCode::Refused,
            };
            perr(code, format!("trace {id}: {e}"))
        })?;
        let Some(m) = curve else {
            if t.mic_curve.is_none() {
                return Err(perr(
                    ErrorCode::NotFound,
                    format!("trace {id} has no mic curve applied after capture"),
                ));
            }
            t.mic_curve = None;
            self.traces.set_mic_curve(id, None);
            self.commit(Change::Trace(Patch::Set(t.clone())));
            self.restart_maths_naming(Operand::Trace { trace: id });
            tracing::info!("trace {id}: mic curve removed");
            return Ok(ReplyBody::Trace(t));
        };
        let st = self.store.state();
        let curve_ref = ac2_proto::cal::curve(&st.mics, &m.mic, &m.label)
            .ok_or_else(|| {
                let stored = ac2_proto::cal::mic(&st.mics, &m.mic).map_or_else(
                    || "none".to_owned(),
                    |x| {
                        x.curves
                            .iter()
                            .map(|c| c.label.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                );
                perr(
                    ErrorCode::NotFound,
                    format!(
                        "no mic curve {:?} for mic {:?} in the mic library (stored: {stored}; \
                         import one with `ac2 cal curve import`)",
                        m.label, m.mic
                    ),
                )
            })?
            .clone();
        let Some(points) = self.cal.curve(&m.mic, &m.label).cloned() else {
            return Err(perr(
                ErrorCode::Internal,
                format!("the mic curve {:?} of {:?} has no points", m.label, m.mic),
            ));
        };
        // Normalised where the trace's level calibration was read (the calibrator tone was
        // read uncorrected, so 0 dB there counts nothing twice), else where the mic's newest
        // calibration was, else at 1 kHz — the live rule (Q7 §3).
        let trace_cal = match &t.cal {
            CalState::Calibrated { key, .. } => st.calibrations.iter().find(|e| e.key == *key),
            CalState::Uncalibrated => None,
        };
        let f_norm = crate::calstore::f_norm(trace_cal.or_else(|| {
            st.calibrations
                .iter()
                .filter(|e| e.key.mic == m.mic)
                .max_by_key(|e| e.spl.calibrated_at)
        }));
        t.mic_curve = Some(Box::new(TraceMicCurve {
            mic: m.mic.clone(),
            curve: curve_ref,
            f_norm: Hz(f_norm),
        }));
        self.traces
            .set_mic_curve(id, Some(points.normalised(f_norm)));
        self.commit(Change::Trace(Patch::Set(t.clone())));
        // The curve corrects the trace's columns as a math channel combines them.
        self.restart_maths_naming(Operand::Trace { trace: id });
        tracing::info!("trace {id}: mic curve of {m:?} applied (0 dB at {f_norm} Hz)");
        Ok(ReplyBody::Trace(t))
    }

    pub(super) fn trace_export(
        &self,
        id: TraceId,
        format: ExportFormat,
    ) -> Result<ReplyBody, ProtoError> {
        let t = self.stored(id)?;
        match format {
            ExportFormat::Ac2Csv => Ok(ReplyBody::Export {
                file_name: meta::file_name(&t.meta.edit.name, "csv"),
                content: ac2_proto::units::Blob(text::export_csv(&t).into_bytes()),
            }),
        }
    }
}
