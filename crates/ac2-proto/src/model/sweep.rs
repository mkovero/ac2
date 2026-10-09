//! Sweep measurements (runs, distortion, impulse response) and the room acoustics derived from them.

use serde::{Deserialize, Serialize};

use super::EssSpec;
use crate::units::{ClientId, Db, Dbfs, Hz, MeasId, Seconds, SweepId, TraceId, WallNs};

/// A sweep measurement's settings: what each `sweep.run` of it plays and records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepConfig {
    /// Reference input (the loopback), zero-based.
    pub reference_input: u16,
    /// Measurement input (the mic), zero-based.
    pub measurement_input: u16,
    /// Output channels carrying the sweep (zero-based): the speaker's and the loopback's.
    pub outputs: Vec<u16>,
    /// RMS level of the sweep's constant-envelope part, typed by the operator (there is no
    /// default level); checked against the global maximum at every run.
    pub level: Dbfs,
    /// The sweep.
    pub sweep: EssSpec,
    /// Sweeps played and averaged, 1 … [`SweepConfig::MAX_REPEATS`].
    pub repeats: u8,
    /// Linear-response gate after the arrival; `None` = the whole response up to the noise
    /// window.
    pub gate: Option<Seconds>,
    /// Silence recorded after each sweep (the room's decay and its noise; room parameters
    /// are computed up to its end), at most [`SweepConfig::MAX_TAIL`]; `None` or anything
    /// shorter = the shortest the analysis needs (1 s or more).
    pub tail: Option<Seconds>,
    /// Harmonic windows at the lowest columns; `fine` also lengthens the silence after
    /// each sweep, so it is part of the measurement, not only of its analysis.
    pub lf_harmonics: LfHarmonics,
}

/// Harmonic windows at the lowest columns (harmonics below about 1 kHz), where 1/24 octave
/// is narrower than a few of the window's resolution cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LfHarmonics {
    /// Every order in one shared window: the lowest floor, coarser resolution at the
    /// lowest columns.
    #[default]
    Standard,
    /// Each order in its own longest window: finer low-frequency resolution and columns
    /// below 20 Hz on a sweep that starts there, a higher floor there, and up to four of the
    /// longest window of silence after each sweep.
    Fine,
}

impl SweepConfig {
    /// Longest silence after each sweep.
    pub const MAX_TAIL: Seconds = Seconds(20.0);

    /// Most repeats.
    pub const MAX_REPEATS: u8 = 8;
}

/// Why a sweep run ended without a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepFailure {
    /// `gen.stop`, `gen.release` or a forced takeover stopped it.
    Stopped,
    /// The stimulus lease expired.
    LeaseExpired,
    /// The session closed or reopened.
    SessionClosed,
    /// Audio was lost while recording (an xrun or overflow).
    Dropout,
    /// The reference input carries no sweep.
    NoReference,
    /// The analysis refused the recording.
    Analysis,
}

/// Where a sweep run is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SweepStatus {
    /// Playing and recording sweep `repeat` (1-based).
    Playing {
        /// Repeat.
        repeat: u8,
    },
    /// Recorded; the analysis runs.
    Analysing,
    /// Stored as a sweep trace.
    Done {
        /// The trace.
        trace: TraceId,
    },
    /// Ended without a result; its audio is discarded.
    Failed {
        /// Why.
        reason: SweepFailure,
        /// Detail for the operator.
        msg: String,
    },
}

/// The latest sweep run (`sweep.run`), mirrored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepRun {
    /// Id.
    pub id: SweepId,
    /// The sweep measurement it runs.
    pub meas: MeasId,
    /// Client that started it.
    pub owner: ClientId,
    /// Name of the trace it makes.
    pub name: String,
    /// Reference input.
    pub reference_input: u16,
    /// Measurement input.
    pub measurement_input: u16,
    /// Outputs playing it.
    pub outputs: Vec<u16>,
    /// Level.
    pub level: Dbfs,
    /// Sweep as requested.
    pub sweep: EssSpec,
    /// Actual duration of each emitted sweep: the rate constant is rounded, and the sweep
    /// starts two octaves below the requested start at a rising level.
    pub sweep_duration: Seconds,
    /// Silence after each sweep.
    pub post_roll: Seconds,
    /// Sweeps.
    pub repeats: u8,
    /// Linear gate.
    pub gate: Option<Seconds>,
    /// Harmonic windows at the lowest columns.
    pub lf_harmonics: LfHarmonics,
    /// Status.
    pub status: SweepStatus,
    /// When it started.
    pub started_at: WallNs,
}

impl SweepRun {
    /// Total playing time: every sweep with its silence.
    pub fn total(&self) -> Seconds {
        Seconds(f64::from(self.repeats) * (self.sweep_duration.0 + self.post_roll.0))
    }

    /// Still playing or analysing.
    pub fn active(&self) -> bool {
        matches!(
            self.status,
            SweepStatus::Playing { .. } | SweepStatus::Analysing
        )
    }
}

/// A distortion curve on the trace's grid (fundamental frequency per column).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistortionCurve {
    /// Level re the fundamental, dB; NaN where not measured.
    pub level_db: Vec<f32>,
    /// Noise in the same window re the fundamental, dB; NaN where not measured.
    pub floor_db: Vec<f32>,
}

impl DistortionCurve {
    /// Column `i` counts as distortion: measured and at least `margin` above its floor
    /// ([`SweepInfo::floor_margin`]). Otherwise it reads "< floor".
    pub fn valid(&self, i: usize, margin: Db) -> bool {
        let (Some(l), Some(f)) = (self.level_db.get(i), self.floor_db.get(i)) else {
            return false;
        };
        l.is_finite() && (!f.is_finite() || f64::from(*l) >= f64::from(*f) + margin.0)
    }
}

/// One harmonic order's distortion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarmonicCurve {
    /// Order (2 = second harmonic).
    pub order: u8,
    /// Curve.
    pub curve: DistortionCurve,
}

/// The impulse response of a sweep: from the highest order's window to the end of the
/// linear window, decimated peak-preserving.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepIr {
    /// Time of point 0 re the arrival.
    pub t0: Seconds,
    /// Point spacing.
    pub dt: Seconds,
    /// Signed extreme per point (unit: the reference's level).
    pub linear: Vec<f32>,
    /// Hilbert envelope maximum per point, dB.
    pub etc_db: Vec<f32>,
}

/// What the analysis found and used.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepInfo {
    /// Sample rate.
    pub sample_rate: Hz,
    /// Rate constant L of the sweep: harmonic k's impulse sits at −L·ln k.
    pub rate: Seconds,
    /// Actual duration of each emitted sweep (from two octaves below the requested start).
    pub duration: Seconds,
    /// Sweeps averaged.
    pub repeats: u8,
    /// Arrival of the linear response re the reference (the trace's `delay`).
    pub arrival: Seconds,
    /// The reference's sweep re the emitted level (loopback gain).
    pub reference_level: Db,
    /// Harmonic window before `t_k`.
    pub window_pre: Seconds,
    /// Harmonic window after `t_k`.
    pub window_post: Seconds,
    /// Linear window before the arrival.
    pub gate_pre: Seconds,
    /// Linear window after the arrival.
    pub gate: Seconds,
    /// How far above its noise floor a distortion point must be to count.
    pub floor_margin: Db,
    /// A sample of either input reached full scale.
    pub clipped: bool,
}

/// Distortion and impulse response of a sweep trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepData {
    /// H2 … H5.
    pub harmonics: Vec<HarmonicCurve>,
    /// Total harmonic distortion (power sum of the orders in band).
    pub thd: DistortionCurve,
    /// Impulse response.
    pub ir: SweepIr,
    /// Analysis facts.
    pub info: SweepInfo,
    /// ISO 3382-1 room parameters of the impulse response; `None` for a sweep imported from
    /// an export written without them.
    pub room: Option<RoomAcoustics>,
}

// ---------------------------------------------------------------------------------------
// Room acoustics (ISO 3382-1, docs/design/room-metrics.md)

/// Why a room parameter is not given.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoomRefusal {
    /// The band carries no decay (no energy, or none falling).
    NoDecay,
    /// The decay meets the noise too soon: the parameter needs `needed` of decay range.
    InsufficientRange {
        /// Decay range measured.
        range: Db,
        /// Decay range needed.
        needed: Db,
    },
    /// The decay is too short for the band's filter (bandwidth × decay time below 8).
    FilterLimited {
        /// Bandwidth × decay time found.
        bandwidth_decay: f64,
    },
}

/// A room parameter, or why it is not given.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoomValue {
    /// The parameter (unit: the field's).
    Value {
        /// Value.
        value: f64,
    },
    /// Not given.
    Refused {
        /// Why.
        reason: RoomRefusal,
    },
}

impl RoomValue {
    /// The value, if given.
    pub fn value(self) -> Option<f64> {
        match self {
            RoomValue::Value { value } => Some(value),
            RoomValue::Refused { .. } => None,
        }
    }
}

/// One band's room parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomBand {
    /// Mid-band frequency; `None` = broadband (the impulse response as captured).
    pub centre: Option<Hz>,
    /// The band's onset, re the arrival.
    pub onset: Seconds,
    /// Where its decay meets the noise (truncation point), re the arrival.
    pub truncation: Seconds,
    /// Depth of its decay curve at the truncation point; `None` without a decay.
    pub decay_range: Option<Db>,
    /// Early decay time, seconds.
    pub edt: RoomValue,
    /// T20, seconds.
    pub t20: RoomValue,
    /// T30, seconds.
    pub t30: RoomValue,
    /// Clarity C50, dB.
    pub c50: RoomValue,
    /// Clarity C80, dB.
    pub c80: RoomValue,
    /// Definition D50, ratio 0…1.
    pub d50: RoomValue,
    /// Curvature 100·(T30/T20 − 1), percent, when both are given.
    pub curvature: Option<f64>,
}

/// ISO 3382-1 room parameters of a sweep's impulse response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomAcoustics {
    /// The impulse response as captured (band: the sweep's).
    pub broadband: RoomBand,
    /// Octave bands 63 Hz … 8 kHz inside the sweep's range.
    pub octave: Vec<RoomBand>,
    /// One-third-octave bands 50 Hz … 10 kHz inside the sweep's range.
    pub third: Vec<RoomBand>,
    /// End of the impulse response analysed (the end of the silence after the sweep), re
    /// the arrival.
    pub span_end: Seconds,
}

impl RoomAcoustics {
    /// Curvature above which a decay is not straight, percent.
    pub const CURVATURE_LIMIT: f64 = 10.0;
}
