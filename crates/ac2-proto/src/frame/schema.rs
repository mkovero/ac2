//! Frame kinds and the array descriptors of the payload.

use serde::{Deserialize, Serialize};

use crate::model::{LevelScale, SpecLevel};

/// Frame kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    /// Transfer function.
    Tf,
    /// Impulse response view.
    Ir,
    /// RTA.
    Rta,
    /// Spectrum.
    Spec,
    /// SPL meter.
    Spl,
    /// Rolling Leq windows of an SPL meter.
    Leq,
    /// 1/3-octave band Leq of an SPL meter's band meter.
    BandLeq,
    /// Input meters.
    Levels,
    /// Input meters of the open session.
    SessionLevels,
    /// Input meters of a device preview.
    PreviewLevels,
    /// Goniometer points of the open session's pairs.
    SessionGonio,
    /// Goniometer points of a device preview's pairs.
    PreviewGonio,
    /// Loopback timing.
    Timing,
    /// Keepalive.
    Ka,
}

/// Array name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrayName {
    /// TF magnitude.
    Mag,
    /// TF phase.
    Phase,
    /// TF coherence γ².
    Coh,
    /// An alignment's phase difference as measured.
    AlignPhaseBefore,
    /// An alignment's phase difference with its fit applied.
    AlignPhaseAfter,
    /// An alignment's summation as measured.
    AlignSumBefore,
    /// An alignment's summation with its fit applied.
    AlignSumAfter,
    /// Validity bitmask.
    Validity,
    /// IR samples, linear.
    IrLinear,
    /// IR energy-time curve.
    IrEtc,
    /// Band / bin level.
    Level,
    /// Input peak.
    Peak,
    /// Input RMS.
    Rms,
    /// Input clip flags.
    Clip,
    /// Leq of each window.
    Leq,
    /// Seconds of each window elapsed.
    Elapsed,
    /// Seconds of each window measured.
    Measured,
    /// Headroom: steady level allowed over the horizon (NaN: none).
    Allowed,
    /// Seconds to recover at the limit (NaN unless the window cannot recover within the
    /// horizon).
    Recover,
    /// Leq window state.
    LeqFlags,
    /// Leq a window ends at if the rest of it is silent (its Leq once full).
    Least,
    /// Seconds until a filling window on course spends its budget (NaN otherwise).
    OverIn,
    /// A band window's limit in force (NaN: none).
    Limit,
    /// The chosen ballistic's programme-meter reading per input.
    Ppm,
    /// True peak per input (BS.1770 Annex 2).
    TruePeak,
    /// Highest reading of the chosen ballistic since the last reset, per input.
    Max,
    /// Over events since the last reset, per input.
    Overs,
    /// Goniometer point x (`(r − l)/√2` after auto gain), every pair's points in turn.
    GonioX,
    /// Goniometer point y (`(l + r)/√2` after auto gain), every pair's points in turn.
    GonioY,
}

/// Array unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// dB (ratio).
    Db,
    /// dBFS.
    Dbfs,
    /// dB true peak (an inter-sample peak re full scale).
    Dbtp,
    /// dB SPL.
    DbSpl,
    /// dBFS per hertz (power spectral density).
    DbfsPerHz,
    /// dB SPL per hertz (power spectral density).
    DbSplPerHz,
    /// Degrees.
    Deg,
    /// γ², 0…1.
    Coherence,
    /// Linear, full scale = 1.
    FullScale,
    /// Seconds.
    Seconds,
    /// Bitmask (u32 elements).
    Bitmask,
    /// An event count (u32 elements).
    Count,
    /// Goniometer coordinate, −127 … 127 of the pair's auto gain (i8 elements).
    GonioUnit,
}

/// Element type of an array part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Elem {
    /// IEEE-754 binary32, little-endian.
    F32,
    /// Unsigned 32-bit, little-endian.
    U32,
    /// Hundredths of the unit, signed 16-bit, as neighbour differences in byte planes;
    /// degrees wrapped to [−180, 180). Levels, magnitudes and phase of the curves a display
    /// draws.
    I16,
    /// Ten-thousandths, unsigned 16-bit (0 … 10 000), as neighbour differences in byte
    /// planes. Coherence.
    U16,
    /// Block floating point: one exponent byte `e`, then signed 16-bit mantissas as
    /// neighbour differences in byte planes, value = mantissa · 2^e; within 1/32 765 of the
    /// array's peak. The linear IR.
    Bfp16,
    /// Signed 8-bit. The goniometer's quantised coordinates.
    I8,
}

/// Describes one array part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrayDesc {
    /// Name.
    pub name: ArrayName,
    /// Unit.
    pub unit: Unit,
    /// Element type.
    pub elem: Elem,
}

pub(super) fn desc(name: ArrayName, unit: Unit) -> ArrayDesc {
    // The curves a display draws at frame rate go in 16 bits (precision budget in
    // `crate::packing`), the linear IR relative to its peak; meters and Leq windows are read
    // as numbers or summed further and stay f32.
    let elem = match (name, unit) {
        (_, Unit::Bitmask | Unit::Count) => Elem::U32,
        (_, Unit::GonioUnit) => Elem::I8,
        (
            ArrayName::Mag
            | ArrayName::Phase
            | ArrayName::Level
            | ArrayName::IrEtc
            | ArrayName::AlignPhaseBefore
            | ArrayName::AlignPhaseAfter
            | ArrayName::AlignSumBefore
            | ArrayName::AlignSumAfter,
            _,
        ) => Elem::I16,
        (ArrayName::Coh, _) => Elem::U16,
        (ArrayName::IrLinear, _) => Elem::Bfp16,
        _ => Elem::F32,
    };
    ArrayDesc { name, unit, elem }
}

pub(super) fn level_unit(scale: LevelScale) -> Unit {
    match scale {
        LevelScale::Dbfs => Unit::Dbfs,
        LevelScale::DbSpl => Unit::DbSpl,
    }
}

pub(super) fn spec_unit(scale: LevelScale, level: SpecLevel) -> Unit {
    match (scale, level) {
        (_, SpecLevel::Tone) => level_unit(scale),
        (LevelScale::Dbfs, SpecLevel::Density) => Unit::DbfsPerHz,
        (LevelScale::DbSpl, SpecLevel::Density) => Unit::DbSplPerHz,
    }
}
