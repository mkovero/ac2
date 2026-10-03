//! The trace store and the `trace.*` commands.
//!
//! Metadata lives in the mirrored state (so every client sees it through events); the
//! columns and grid of every trace live here and leave only by `trace.get`, `trace.export`
//! and `file.save`.

use std::collections::BTreeMap;

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    AverageMethod, CalState, DelayReference, ExportFormat, ImportFormat, ImportRole, MathOp,
    MeasKind, MicState, Smoothing, SmoothingMode, TraceEdit, TraceKind, TraceMeta, TraceSource,
};
use ac2_proto::units::{MeasId, Seconds, TraceId, WallNs};
use ac2_proto::{ErrorCode, ErrorDetail, FrameData, GridDef, ProtoError, ReplyBody};
use ac2_traces::columns::{Columns, StoredTrace};
use ac2_traces::meta;
use ac2_traces::ops::{self, Derived, OpError};
use ac2_traces::text::{self, ImportError};

use super::Control;
use crate::util::{perr, perr_detail, wall_ns};

/// Columns and grids of the stored traces, by id.
#[derive(Debug, Default)]
pub(crate) struct TraceStore {
    data: BTreeMap<TraceId, (GridDef, Columns)>,
    next_id: u32,
}

impl TraceStore {
    pub(crate) fn alloc(&mut self) -> TraceId {
        self.next_id = self.next_id.max(1);
        let id = TraceId(self.next_id);
        self.next_id += 1;
        id
    }

    pub(crate) fn insert(&mut self, id: TraceId, grid: GridDef, columns: Columns) {
        self.next_id = self.next_id.max(id.0.saturating_add(1));
        self.data.insert(id, (grid, columns));
    }

    pub(crate) fn remove(&mut self, id: TraceId) {
        self.data.remove(&id);
    }

    pub(crate) fn clear(&mut self) {
        self.data.clear();
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
            sweep: None,
        })
    }

    /// Commits a new trace (taking its slot from any other trace) and stores its data.
    fn add_trace(&mut self, meta: TraceMeta, grid: GridDef, columns: Columns) -> ReplyBody {
        let gid = self.register_grid(grid.clone());
        debug_assert_eq!(gid, meta.grid_id);
        for other in meta::take_slot(&self.store.state().traces, meta.id, meta.edit.slot) {
            self.commit(Change::Trace(Patch::Set(other)));
        }
        self.traces.insert(meta.id, grid, columns);
        self.commit(Change::Trace(Patch::Set(meta.clone())));
        ReplyBody::Trace(meta)
    }

    /// Calibration and mic of `input` at capture (`docs/design/q7-calibration.md` §3): the
    /// input setup's mic name with the curve applied to it, and the sensitivity calibration
    /// the matching rules pick. A calibration of another mic or input keeps its own key, so
    /// the trace shows that it was not this mic's.
    fn cal_and_mic(&self, input: u16) -> (CalState, Option<MicState>) {
        let Some(rt) = self.session.as_ref() else {
            return (CalState::Uncalibrated, None);
        };
        let c = self.input_cal(rt, input);
        let cal = match c.spl_entry {
            Some((key, s)) => CalState::Calibrated {
                key,
                sensitivity: s.sensitivity,
                calibrated_at: s.calibrated_at,
            },
            None => CalState::Uncalibrated,
        };
        let mic = crate::calstore::input_setup(&self.store.state().inputs, input)
            .mic
            .map(|name| MicState {
                name,
                curve: c.curve_name,
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
            .and_then(super::JobHandle::latest)
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
            created_at: WallNs(wall_ns()),
        };
        tracing::info!("trace {id} captured from measurement {meas}");
        Ok(self.add_trace(t, grid, columns))
    }

    pub(super) fn trace_get(&self, id: TraceId) -> Result<ReplyBody, ProtoError> {
        Ok(ReplyBody::TraceData(self.stored(id)?.data()))
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
            created_at: WallNs(wall_ns()),
        };
        Ok(self.add_trace(t, d.grid, d.columns))
    }

    pub(super) fn trace_average(
        &mut self,
        ids: &[TraceId],
        method: AverageMethod,
        reference: DelayReference,
        name: String,
    ) -> Result<ReplyBody, ProtoError> {
        let inputs = ids
            .iter()
            .map(|id| self.stored(*id))
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

    pub(super) fn trace_math(
        &mut self,
        a: TraceId,
        b: TraceId,
        op: MathOp,
        name: String,
    ) -> Result<ReplyBody, ProtoError> {
        let (ta, tb) = (self.stored(a)?, self.stored(b)?);
        let d = ops::math(&ta, &tb, op).map_err(|e| op_err(&e))?;
        let smoothing = common_smoothing(&[&ta, &tb], d.kind);
        self.derived(name, TraceSource::Math { a, b, op }, d, &[], smoothing)
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
            },
            grid_id: imp.grid.id(),
            delay: Seconds(0.0),
            depth: None,
            cal: CalState::Uncalibrated,
            mic: None,
            created_at: WallNs(wall_ns()),
        };
        tracing::info!("trace {id} imported ({} rows)", imp.rows);
        Ok(self.add_trace(t, imp.grid, imp.columns))
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
