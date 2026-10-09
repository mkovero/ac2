//! Stored traces: kinds, sources, import/export, mic curve and calibration state, metadata, session files.

use serde::{Deserialize, Serialize};

use super::{
    AverageMethod, CalKey, DelayReference, DepthPolicy, EssSpec, LevelScale, LfHarmonics, MathExpr,
    MathOp, MicCurveRef, Operand, PhaseBasis, Smoothing, SweepData, TraceIr, TraceOwner,
};
use crate::grid::GridId;
use crate::units::{
    Db, Dbfs, Hz, MeasId, SampleIndex, Seconds, SessionEpoch, SweepId, TraceId, WallNs,
};

/// Polarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    /// As measured.
    Normal,
    /// Inverted.
    Inverted,
}

/// Imported file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportFormat {
    /// ac2 CSV (what `trace.export` writes; its header is checked).
    Ac2Csv,
    /// Analyzer text export: columns `freq mag [phase] [coherence]`, separated by commas,
    /// semicolons, tabs or spaces, with optional comment and header lines.
    AnalyzerText,
    /// ac2 CSV when the file starts with the ac2 header, analyzer text otherwise.
    Auto,
}

/// What an imported file becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportRole {
    /// A measured trace (independent time base, decision 8a).
    Trace,
    /// A target curve: magnitude only, drawn on the transfer pane.
    Target,
}

/// What a trace holds, and so where it is drawn and which operations apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceKind {
    /// Transfer function: magnitude dB, phase, coherence.
    Transfer,
    /// Target curve: magnitude dB only.
    Target,
    /// Narrowband spectrum (tone level).
    Spectrum {
        /// Level unit.
        scale: LevelScale,
    },
    /// Fractional-octave RTA (band power).
    Rta {
        /// Level unit.
        scale: LevelScale,
    },
    /// Sweep measurement: the fundamental's magnitude dB and phase (drawn like a transfer
    /// function), harmonic distortion per order and the impulse response
    /// ([`TraceData::sweep`]).
    Sweep,
}

/// Export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// ac2 CSV.
    Ac2Csv,
}

/// Where a trace came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceSource {
    /// Captured live.
    Captured {
        /// Source measurement.
        meas: MeasId,
        /// Its name at capture.
        meas_name: String,
        /// Epoch it was captured in (shared time reference within it).
        epoch: SessionEpoch,
        /// Capture sample index.
        at_sample: SampleIndex,
    },
    /// Imported from a file; independent time reference (decision 8a).
    Imported {
        /// Original file name.
        file_name: String,
        /// Format actually parsed (never `auto`).
        format: ImportFormat,
        /// What the file held that the trace does not keep.
        notes: Vec<ImportNote>,
    },
    /// Captured from a math channel: the expression and the operands that went into it.
    Math {
        /// The math channel.
        meas: MeasId,
        /// Its name at capture.
        meas_name: String,
        /// Epoch it was captured in.
        epoch: SessionEpoch,
        /// Capture sample index.
        at_sample: SampleIndex,
        /// The expression.
        expr: MathExpr,
        /// The operands that went into the capture, named as at capture, in expression
        /// order (an average's operands left out at that moment are not listed).
        operands: Vec<NamedOperand>,
        /// What the phase is relative to.
        phase: PhaseBasis,
    },
    /// Average of other traces.
    Average {
        /// Inputs.
        traces: Vec<TraceId>,
        /// Method.
        method: AverageMethod,
        /// Phase reference.
        reference: DelayReference,
    },
    /// A run of a sweep measurement (`sweep.run`).
    Sweep {
        /// The sweep measurement.
        meas: MeasId,
        /// Its name at the run.
        meas_name: String,
        /// The run that made it.
        run: SweepId,
        /// Run number within the measurement (1, 2 …): one more than the highest of its
        /// runs stored when it ran.
        number: u32,
        /// Epoch (shared time reference within it, like a capture).
        epoch: SessionEpoch,
        /// Sweep played.
        sweep: EssSpec,
        /// Level played.
        level: Dbfs,
        /// Sweeps averaged.
        repeats: u8,
        /// Harmonic windows at the lowest columns.
        lf_harmonics: LfHarmonics,
        /// Reference input.
        reference_input: u16,
        /// Measurement input.
        measurement_input: u16,
    },
}

impl TraceSource {
    /// The session epoch whose time base the trace's phase is in: captures (live or sweep)
    /// share their epoch's, and so does a math capture whose phase is a sum, difference or
    /// average of operands in that time base (a ratio or a cascade is relative, in no time
    /// base); every other source is independent (decision 8a).
    pub fn shared_epoch(&self) -> Option<SessionEpoch> {
        match self {
            TraceSource::Captured { epoch, .. } | TraceSource::Sweep { epoch, .. } => Some(*epoch),
            TraceSource::Math {
                epoch, expr, phase, ..
            } => {
                let relative = matches!(
                    expr,
                    MathExpr::Binary {
                        op: MathOp::Divide | MathOp::Multiply,
                        ..
                    }
                );
                (*phase == PhaseBasis::SharedTimeBase && !relative).then_some(*epoch)
            }
            TraceSource::Imported { .. } | TraceSource::Average { .. } => None,
        }
    }
}

/// A math capture's operand and its name at capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedOperand {
    /// The operand.
    pub operand: Operand,
    /// Its name at capture.
    pub name: String,
}

/// Something an imported file held that the trace does not keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportNote {
    /// A sweep export without its analysis facts and impulse response (written before
    /// they were exported): imported as its transfer function, the distortion dropped.
    SweepWithoutAnalysis,
    /// A sweep export whose rows are off the grid in its header: the response was
    /// resampled, and distortion is never resampled, so it was dropped.
    SweepOffGrid,
    /// The export showed a mic curve applied afterwards as a display edit; its columns are
    /// as measured, without the curve (apply it again with `trace.mic_curve`).
    MicCurveNotApplied,
}

/// Display colour, 8-bit sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

/// Calibration state of a trace at capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalState {
    /// Uncalibrated (dBFS).
    Uncalibrated,
    /// Calibrated with this entry.
    Calibrated {
        /// The calibration used.
        key: CalKey,
        /// Its sensitivity.
        sensitivity: Db,
        /// When the calibration was taken.
        calibrated_at: WallNs,
    },
}

/// Mic in use for a trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicState {
    /// Mic name.
    pub name: String,
    /// The curve the live measurement had subtracted from the captured columns, `None`
    /// when none applied.
    pub curve: Option<MicCurveRef>,
}

/// A mic curve applied to a stored trace after capture (`trace.mic_curve`): a display
/// edit. The stored columns stay as measured; the daemon subtracts the curve, normalised to
/// 0 dB at `f_norm`, when it serves the trace's data (after the display smoothing), and
/// keeps the curve's points with the trace, so it survives a later change to the
/// calibration store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceMicCurve {
    /// Mic name of the calibration entry the curve came from.
    pub mic: String,
    /// The curve as the calibration store had it.
    pub curve: MicCurveRef,
    /// Normalisation frequency: the correction is 0 dB here.
    pub f_norm: Hz,
}

/// Operator-editable trace properties (`trace.update` replaces all of them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceEdit {
    /// Name.
    pub name: String,
    /// Colour.
    pub color: Rgb,
    /// Shown.
    pub visible: bool,
    /// Protected from edits and deletion.
    pub locked: bool,
    /// Display order (lower first).
    pub order: u32,
    /// Magnitude offset.
    pub offset: Db,
    /// Polarity.
    pub polarity: Polarity,
    /// Per-trace delay nudge on top of the measured delay (decision 8a).
    pub delay_nudge: Seconds,
    /// Slot 1…9 the trace occupies (Ctrl+1…9 in the UI); a slot holds at most one trace.
    pub slot: Option<u8>,
    /// Display smoothing (transfer and spectrum traces): applied when the daemon serves the
    /// trace's data. The stored columns stay unsmoothed, so it can be changed at any time; a
    /// capture starts with the smoothing its measurement had. A spectrum has no phase: its
    /// power is smoothed in either mode.
    pub smoothing: Option<Smoothing>,
    /// The measurement it is listed under (a capture: the one it came from; a sweep run: its
    /// sweep measurement), or the imported group. Moving it changes nothing else.
    pub owner: TraceOwner,
}

/// Trace metadata entity (mandatory metadata of PLAN §3.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceMeta {
    /// Id.
    pub id: TraceId,
    /// Editable properties.
    pub edit: TraceEdit,
    /// Content.
    pub kind: TraceKind,
    /// Origin.
    pub source: TraceSource,
    /// Grid of the stored data.
    pub grid_id: GridId,
    /// Delay the phase is referred to: the measured delay at capture; for an average the
    /// common reference delay; for an import the delay its ac2 CSV header states (0 for
    /// other files).
    pub delay: Seconds,
    /// Averaging depth policy at capture (transfer captures).
    pub depth: Option<DepthPolicy>,
    /// Calibration at capture.
    pub cal: CalState,
    /// Mic at capture.
    pub mic: Option<MicState>,
    /// A mic curve applied after capture (display edit, [`TraceMicCurve`]); never set
    /// while `mic.curve` is (the columns carry that curve already).
    pub mic_curve: Option<Box<TraceMicCurve>>,
    /// When captured / created.
    pub created_at: WallNs,
}

/// A session saved by `file.save` (reply to `file.save` / `file.load`, rows of `file.list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionFile {
    /// Name (the directory name).
    pub name: String,
    /// Directory on the daemon host.
    pub path: String,
    /// When it was saved.
    pub saved_at: WallNs,
    /// Measurements in it.
    pub measurements: u32,
    /// Traces in it.
    pub traces: u32,
}

/// Which session directory `file.save` / `file.load` use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionRef {
    /// A name in the daemon's session directory (letters, digits, ` `, `-`, `_`, `.`).
    Name {
        /// Name.
        name: String,
    },
    /// A directory path on the daemon host (local transports only).
    Path {
        /// Path.
        path: String,
    },
}

/// Stored trace data (reply to `trace.get`): the stored columns with the trace's display
/// smoothing (`meta.edit.smoothing`) and then its mic curve (`meta.mic_curve`) applied; the
/// curve also corrects a sweep's distortion levels and floors (each order re the
/// fundamental at its own frequency). Column order = grid order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceData {
    /// Metadata.
    pub meta: TraceMeta,
    /// Magnitude, dB.
    pub mag_db: Vec<f32>,
    /// Phase, degrees; `None` when the trace has no phase.
    pub phase_deg: Option<Vec<f32>>,
    /// Coherence γ²; `None` when not available.
    pub coherence: Option<Vec<f32>>,
    /// Distortion and impulse response of a [`TraceKind::Sweep`] trace.
    pub sweep: Option<SweepData>,
    /// The impulse response a [`TraceKind::Transfer`] trace was captured with; `None` when
    /// the measurement had none to show at capture (no reference yet) and for traces that
    /// were not captured from a transfer measurement.
    pub ir: Option<TransferIr>,
}

/// The impulse response a transfer trace was captured with: the samples the IR pane drew at
/// that moment, unsmoothed and without the trace's display edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferIr {
    /// Sample rate of the measurement.
    pub sample_rate: Hz,
    /// The samples, time zero at the trace's `delay` (the inserted delay at capture).
    pub ir: TraceIr,
}
