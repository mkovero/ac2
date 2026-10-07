//! Signal generator: signals, band limits, ESS spec, settings, audit and lease.

use serde::{Deserialize, Serialize};

use crate::units::{ClientId, Dbfs, Hz, Samples, Seconds, WallNs};

/// Butterworth band-limit slope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOrder {
    /// 12 dB/oct.
    Second,
    /// 24 dB/oct.
    Fourth,
}

/// Optional band limits on noise.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandLimit {
    /// High-pass corner.
    pub highpass: Option<Hz>,
    /// Low-pass corner.
    pub lowpass: Option<Hz>,
    /// Slope.
    pub order: FilterOrder,
}

/// Exponential sine sweep.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EssSpec {
    /// Start frequency.
    pub start: Hz,
    /// End frequency.
    pub end: Hz,
    /// Requested duration.
    pub duration: Seconds,
    /// Fade-in.
    pub fade_in: Seconds,
    /// Fade-out.
    pub fade_out: Seconds,
}

impl EssSpec {
    /// Default start frequency of a sweep measurement, Hz.
    pub const DEFAULT_START_HZ: f64 = 20.0;
    /// Default end frequency, Hz.
    pub const DEFAULT_END_HZ: f64 = 20_000.0;
    /// Default requested duration, s.
    pub const DEFAULT_DURATION_S: f64 = 3.0;

    /// A sweep from `start` to `end` in about `duration`, fading in over its first 1/6
    /// octave and out over its last 1/24 octave: it starts and stops without a step, and the
    /// fades stay short enough to leave the band's ends measured.
    pub fn with_fades(start: Hz, end: Hz, duration: Seconds) -> Self {
        let rate = duration.0 / (end.0 / start.0).ln();
        let fade = |octaves: f64| {
            let s = rate * std::f64::consts::LN_2 * octaves;
            Seconds(if s.is_finite() && s > 0.0 { s } else { 0.0 })
        };
        Self {
            start,
            end,
            duration,
            fade_in: fade(1.0 / 6.0),
            fade_out: fade(1.0 / 24.0),
        }
    }
}

/// Generator signal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Signal {
    /// White noise.
    White,
    /// Pink noise.
    Pink,
    /// Periodic pink noise.
    PeriodicPink {
        /// Period (power of two).
        period: Samples,
    },
    /// Sine.
    Sine {
        /// Frequency.
        freq: Hz,
    },
    /// One exponential sine sweep.
    Ess {
        /// Sweep.
        sweep: EssSpec,
    },
}

/// Generator settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorSettings {
    /// Signal.
    pub signal: Signal,
    /// RMS level.
    pub level: Dbfs,
    /// Band limits (noise only).
    pub band: Option<BandLimit>,
    /// Output channels carrying the stimulus (zero-based).
    pub outputs: Vec<u16>,
}

/// Full desired generator state for `gen.set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorDesired {
    /// Settings.
    pub settings: GeneratorSettings,
    /// Armed (does not emit).
    pub armed: bool,
    /// Firing; requires `armed`.
    pub firing: bool,
}

/// Audited generator action (Q6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenAction {
    /// Lease acquired.
    Acquire,
    /// Lease taken over with force.
    Force,
    /// Armed.
    Arm,
    /// Fired.
    Fire,
    /// Settings changed.
    Set,
    /// Stopped (universal).
    Stop,
    /// Lease released.
    Release,
    /// Lease expired.
    Expiry,
    /// The system maximum level (`ceiling`) was lowered.
    CeilingLowered,
    /// The system maximum level (`ceiling`) was raised (an explicit confirmation).
    CeilingRaised,
}

/// Last audited action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenAudit {
    /// Action.
    pub action: GenAction,
    /// Client that caused it; for `expiry`, the owner whose lease expired.
    pub client: Option<ClientId>,
    /// When.
    pub at: WallNs,
}

/// Generator entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generator {
    /// Lease holder.
    pub owner: Option<ClientId>,
    /// Armed.
    pub armed: bool,
    /// Emitting.
    pub firing: bool,
    /// Current settings, if ever set.
    pub settings: Option<GeneratorSettings>,
    /// System maximum level, dBFS RMS: every emission above it is refused, and the output
    /// path limits samples to the matching peak. Any client may lower it; raising it needs
    /// an explicit confirmation and nothing armed or playing (`gen.ceiling`). Kept by the
    /// daemon across restarts.
    pub ceiling: Dbfs,
    /// The hard upper bound of `ceiling`, fixed when the daemon started (`ac2d
    /// --max-level`): `ceiling` never exceeds it.
    pub ceiling_bound: Dbfs,
    /// Last audited action.
    pub last_action: Option<GenAudit>,
}

/// Lease grant (reply to `gen.acquire` / `gen.refresh`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    /// Token to carry in stimulus commands.
    pub lease_token: crate::units::LeaseToken,
    /// Time until expiry without refresh.
    pub expires_in_ms: u32,
}
