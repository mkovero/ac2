//! Stored traces: kinds, sources, import/export, mic curve and calibration state, metadata, session files.

use serde::{Deserialize, Serialize};

use super::{
    AverageMethod, CalKey, DelayReference, DepthPolicy, EssSpec, LevelScale, LfHarmonics, MathExpr,
    MathOp, MicCurveRef, Operand, PhaseBasis, Resolution, Smoothing, SpecLevel, SweepData, TraceIr,
    TraceOwner, Unresolved,
};
use crate::grid::GridId;
use crate::units::{
    Db, Dbfs, Hz, MeasId, SampleIndex, Seconds, SessionEpoch, SweepId, TraceId, TransferId, WallNs,
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
    /// An impulse response as a WAV / RF64 file: 16-, 24- or 32-bit integer or 32-bit float
    /// samples at any rate, one channel of it ([`IrImport`]).
    IrWav,
    /// A WAV / RF64 impulse response when the file starts with `RIFF` or `RF64`; else ac2
    /// CSV when it starts with the ac2 header, analyzer text otherwise.
    Auto,
}

/// How `trace.import` reads an impulse-response WAV file ([`ImportFormat::IrWav`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrImport {
    /// Channel to read, 0-based; `None` reads a mono file and refuses a multi-channel one
    /// (`invalid`, [`ImportProblem::PickChannel`](crate::ctrl::ImportProblem::PickChannel)).
    pub channel: Option<u16>,
    /// Window the frequency response is computed through.
    pub window: IrImportWindow,
    /// Time zero of the phase.
    pub align: IrAlign,
    /// Columns per octave of the response (ten octaves around 1 kHz, as a transfer
    /// measurement's).
    pub resolution: Resolution,
}

impl Default for IrImport {
    fn default() -> Self {
        Self {
            channel: None,
            window: IrImportWindow::Whole,
            align: IrAlign::Peak,
            resolution: Resolution::default(),
        }
    }
}

/// The part of an imported impulse response its frequency response is computed from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum IrImportWindow {
    /// Every sample as the file has it (an exported IR is often windowed already).
    Whole,
    /// `pre` before and `post` after the peak, half-Hann tapered (the rise over `pre`, the
    /// fall over the last tenth of `post`): keeps later reflections out.
    Gate {
        /// Before the peak (≥ 0).
        pre: Seconds,
        /// After the peak (> 0).
        post: Seconds,
    },
}

/// Time zero of an imported impulse response's phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IrAlign {
    /// The peak of |h|, found between samples: the bulk delay is removed and becomes the
    /// trace's `delay`, so the phase shows the response's own shape.
    Peak,
    /// Sample 0 of the file: the phase keeps the whole delay (IRs exported against one
    /// timing reference keep their relative arrival). The trace's `delay` is 0.
    Start,
}

/// What an imported impulse-response file was and how it was read
/// ([`TraceSource::Imported::ir_file`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrFile {
    /// Sample rate of the file.
    pub sample_rate: Hz,
    /// Channels in the file.
    pub channels: u16,
    /// Channel read, 0-based.
    pub channel: u16,
    /// Samples per channel in the file.
    pub frames: u64,
    /// Samples kept from `first` on (fewer than `frames` when the file was longer than
    /// [`TraceIr::MAX_POINTS`]).
    pub kept: u32,
    /// First sample kept.
    pub first: u64,
    /// Peak of |h|, re the file's first sample.
    pub peak: Seconds,
    /// How the response was read.
    pub import: IrImport,
    /// Columns finer than the window resolves.
    pub unresolved: Unresolved,
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
    /// Narrowband spectrum (tone level or density).
    Spectrum {
        /// Level unit.
        scale: LevelScale,
        /// Tone level or density: spectra of different `level` are different quantities,
        /// never combined.
        level: SpecLevel,
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
    /// The trace's impulse response as a mono 32-bit float WAV at its sample rate, every
    /// stored sample (refused for a trace without one, or one stored decimated).
    IrWav,
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
        /// An impulse-response file's facts ([`ImportFormat::IrWav`]); `None` for text.
        ir_file: Option<Box<IrFile>>,
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
    /// An EQ set's filter curve, kept by the daemon (locked: the set is the truth, the
    /// columns are recomputed from it).
    Eq {
        /// The measurement holding the set.
        meas: MeasId,
        /// The set.
        set: crate::units::EqSetId,
        /// The set's rev its columns are of: a client refetches the data when it changes.
        rev: u32,
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
            TraceSource::Imported { .. } | TraceSource::Average { .. } | TraceSource::Eq { .. } => {
                None
            }
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
    /// An impulse response longer than [`TraceIr::MAX_POINTS`] samples: that many were
    /// kept, from shortly before its peak ([`IrFile::first`]).
    IrTruncated,
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
    /// The EQ sets marked "in the console" when it was captured (one per marked set of its
    /// measurement): what of them its columns contain.
    pub eq_in: Vec<super::EqSnapshot>,
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

/// Which session `file.save` / `file.load` / `file.salvage` / `file.download_begin` use.
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
    /// A session archive this client uploaded (`file.upload_begin`), complete; used up by
    /// the command. `file.load` and `file.salvage` only.
    Upload {
        /// The upload.
        transfer: TransferId,
    },
}

/// What the working state (the measurements and traces the daemon holds) belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionOrigin {
    /// A session in the daemon's session directory.
    Name {
        /// Name.
        name: String,
    },
    /// A session directory on the daemon host, by path.
    Path {
        /// Path.
        path: String,
    },
    /// A session archive on a client's machine (opened from or exported to it); only that
    /// client can write it again.
    Client {
        /// The path as that client named it.
        path: String,
    },
}

/// The working state's session and whether it has unsaved changes (`file.*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingSession {
    /// The session it was last opened from or saved to; `None`: untitled.
    pub origin: Option<SessionOrigin>,
    /// The measurements or traces differ from that save (untitled: it holds any).
    pub dirty: bool,
    /// When the content it was opened or saved as was saved; `None`: never.
    pub saved_at: Option<WallNs>,
    /// The working state the last `file.new` / `file.load` replaced while it had unsaved
    /// changes, which `file.recover` brings back.
    pub recover: Option<Recoverable>,
}

/// A replaced working state kept for `file.recover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recoverable {
    /// When it was replaced.
    pub replaced_at: WallNs,
    /// Its session; `None`: untitled.
    pub origin: Option<SessionOrigin>,
    /// Measurements in it.
    pub measurements: u32,
    /// Traces in it.
    pub traces: u32,
}

/// A session archive transfer under way (reply to `file.upload_begin`,
/// `file.download_begin`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    /// Id for the chunks.
    pub transfer: TransferId,
    /// Archive file name (download: suggested, `<session>.ac2session`).
    pub file_name: String,
    /// Archive size, bytes.
    pub bytes: u64,
    /// SHA-256 of the whole archive, lowercase hex.
    pub sha256: String,
    /// Largest chunk, bytes.
    pub chunk_bytes: u32,
}

/// What `file.salvage` did with each trace file of a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Salvage {
    /// The session's format version.
    pub version: Option<u32>,
    /// Traces imported, in file order.
    pub imported: Vec<TraceMeta>,
    /// Files not imported.
    pub refused: Vec<SalvageRefusal>,
}

/// A trace file `file.salvage` could not import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SalvageRefusal {
    /// File, relative to the session directory.
    pub file: String,
    /// Why, as `trace.import` would say.
    pub reason: String,
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
