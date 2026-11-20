//! Dynamics measurement (`docs/design/dynamics.md`): the tone-burst programme a dynamics
//! processor is driven with, and the analysis of its reference loop and outputs into attack,
//! overshoot, release, distortion while the gain moves and control-voltage thump.
//!
//! The programme is one sine carrier whose period is a whole number of samples; every level
//! change lands at an onset point (one per period), so the waveform stays continuous at a
//! zero-crossing onset and every boundary falls on the period grid. Timeline, in periods from
//! the programme's first sample:
//!
//! 1. the linearity check: `pilot − 6 dB` for [`Programme::check_periods`], then `pilot` for
//!    as long;
//! 2. per step (levels outer, durations inner): a burst at the step's level for its whole
//!    number of periods, then a rest at `pilot` that ends when the analysis releases it
//!    ([`Release`]), at [`Programme::rest_max_periods`] at the latest, or after the replayed
//!    rest's fixed length;
//! 3. a fade to silence after the last rest.
//!
//! Pure: no I/O, no threads. The analysis ([`Analyser`]) runs on whatever thread feeds it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

mod analyse;

pub use analyse::{Analyser, max_envelope_points};

/// Length of each half of the linearity check, s.
pub const CHECK_SECONDS: f64 = 2.0;

/// Shortest rest, s: a rest ends no earlier, released or not.
pub const REST_MIN_SECONDS: f64 = 2.0;

/// Default longest rest, s: at τ ≈ 2.7 s exponential in dB, 20 dB of gain reduction takes
/// about 12 s to come within 0.2 dB, so 30 s leaves a slow detector room.
pub const REST_MAX_SECONDS: f64 = 30.0;

/// Longest burst, s: a burst plus its longest rest is the most audio the analysis keeps.
pub const MAX_BURST_SECONDS: f64 = 10.0;

/// Default burst durations, s: the release knee of a programme-dependent detector sits
/// between 10 ms and 1 s. A carrier plays those of at least [`MIN_BURST_PERIODS`] of its
/// periods ([`default_durations`]).
pub const DEFAULT_DURATIONS: [f64; 9] = [0.001, 0.003, 0.01, 0.03, 0.1, 0.3, 1.0, 3.0, 10.0];

/// Shortest burst, carrier periods. A burst's edges are found in the reference's centred
/// one-period envelope, which ramps for a period around each edge, and each edge's midpoint
/// is refined against the levels two periods either side of it: two periods after the start
/// must still be on the burst's plateau, half a period before its end, so the burst lasts at
/// least 2.5 periods (3 whole ones). A shorter burst's envelope is a ramp that never
/// plateaus: neither edge nor any time inside the burst can be read.
pub const MIN_BURST_PERIODS: u32 = 3;

/// Whole carrier periods a burst or rest of `seconds` plays at a period of `period_s` (at
/// least one).
pub fn whole_periods(seconds: f64, period_s: f64) -> u32 {
    ((seconds / period_s).round() as u32).max(1)
}

/// The shortest burst, s, at a carrier period of `period_s`.
pub fn shortest_burst_seconds(period_s: f64) -> f64 {
    f64::from(MIN_BURST_PERIODS) * period_s
}

/// Whether a burst of `seconds` plays at least [`MIN_BURST_PERIODS`] at a period of
/// `period_s`.
pub fn burst_long_enough(seconds: f64, period_s: f64) -> bool {
    whole_periods(seconds, period_s) >= MIN_BURST_PERIODS
}

/// The default durations a carrier of `carrier_hz` plays: [`DEFAULT_DURATIONS`] without
/// those shorter than [`MIN_BURST_PERIODS`] of its periods (1 kHz: 3 ms … 10 s; 8 kHz: all;
/// 100 Hz: 30 ms … 10 s).
pub fn default_durations(carrier_hz: f64) -> Vec<f64> {
    DEFAULT_DURATIONS
        .into_iter()
        .filter(|d| burst_long_enough(*d, 1.0 / carrier_hz))
        .collect()
}

/// Most burst levels in one programme.
pub const MAX_LEVELS: usize = 8;

/// Most burst durations in one programme.
pub const MAX_DURATIONS: usize = 16;

/// Shortest carrier period, samples: the highest carrier is `fs / 12`.
pub const MIN_PERIOD: u32 = 12;

/// Lowest carrier, Hz.
pub const MIN_CARRIER_HZ: f64 = 20.0;

/// Where in the carrier's cycle a level change lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Onset {
    /// At a positive-going zero crossing: the waveform stays continuous.
    #[default]
    Zero,
    /// At a positive peak: the hardest case for a peak detector.
    Peak,
}

impl Onset {
    /// Samples from the start of a carrier period (its positive-going zero crossing) to the
    /// onset point.
    pub fn offset(self, period: u32) -> u32 {
        match self {
            Onset::Zero => 0,
            Onset::Peak => period / 4,
        }
    }
}

/// The carrier period, samples, nearest to `freq_hz` that is whole, even for a zero onset
/// and a multiple of 4 for a peak onset (so the peak lands on a sample).
pub fn snap_period(freq_hz: f64, fs: f64, onset: Onset) -> Result<u32, ProgrammeError> {
    if !freq_hz.is_finite() || !fs.is_finite() || fs <= 0.0 {
        return Err(ProgrammeError::Carrier);
    }
    if freq_hz < MIN_CARRIER_HZ || freq_hz > fs / f64::from(MIN_PERIOD) {
        return Err(ProgrammeError::Carrier);
    }
    let step = match onset {
        Onset::Zero => 2.0,
        Onset::Peak => 4.0,
    };
    let p = ((fs / freq_hz / step).round() * step).max(f64::from(MIN_PERIOD));
    // A 20 Hz carrier at 768 kHz is 38 400 samples: far inside u32.
    Ok(p as u32)
}

/// How the rests of a programme end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rests {
    /// When the analysis releases them (the gain recovered), at the longest rest at the
    /// latest.
    Adaptive,
    /// Fixed lengths in periods, one per step: a previous run's rests replayed, so an A/B
    /// pair plays an identical programme.
    Replay(Vec<u32>),
}

/// What a programme is built from, in the operator's units.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgrammeConfig {
    /// Requested carrier, Hz (snapped by [`snap_period`]).
    pub carrier_hz: f64,
    /// Onset point.
    pub onset: Onset,
    /// Rest level, dBFS RMS: below the processor's threshold.
    pub pilot_dbfs: f64,
    /// Burst levels, dBFS RMS, in play order; each above the pilot.
    pub bursts_dbfs: Vec<f64>,
    /// Burst durations, s, in play order (rounded to whole periods, at least
    /// [`MIN_BURST_PERIODS`]).
    pub durations_s: Vec<f64>,
    /// Longest rest, s.
    pub rest_max_s: f64,
    /// Replayed rest lengths, s, one per step; `None`: adaptive.
    pub replay_rests_s: Option<Vec<f64>>,
}

/// Why a programme cannot be built.
#[derive(Debug, Clone, PartialEq)]
pub enum ProgrammeError {
    /// The carrier is not finite or outside 20 Hz … `fs / 12`.
    Carrier,
    /// No burst level, more than [`MAX_LEVELS`], a non-finite level or one not above the
    /// pilot.
    Levels,
    /// No duration, more than [`MAX_DURATIONS`], or one not in (0, [`MAX_BURST_SECONDS`]].
    Durations,
    /// A duration plays fewer than [`MIN_BURST_PERIODS`] carrier periods.
    ShortBurst {
        /// The duration, s.
        duration_s: f64,
        /// The shortest burst at the carrier played, s.
        shortest_s: f64,
        /// The carrier played, Hz.
        carrier_hz: f64,
    },
    /// The longest rest is not in [[`REST_MIN_SECONDS`], [`REST_MAX_SECONDS`]].
    RestMax,
    /// Replayed rests are not one per step, or one is shorter than [`REST_MIN_SECONDS`] or
    /// longer than [`REST_MAX_SECONDS`].
    Replay,
}

impl std::fmt::Display for ProgrammeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Carrier => write!(f, "the carrier must be 20 Hz … sample rate / 12"),
            Self::Levels => write!(
                f,
                "1 … {MAX_LEVELS} finite burst levels, each above the pilot, are needed"
            ),
            Self::Durations => write!(
                f,
                "1 … {MAX_DURATIONS} burst durations, each above 0 and at most {MAX_BURST_SECONDS} s, are needed"
            ),
            Self::ShortBurst {
                duration_s,
                shortest_s,
                carrier_hz,
            } => write!(
                f,
                "a {} ms burst is shorter than {MIN_BURST_PERIODS} carrier periods: the shortest at {carrier_hz:.2} Hz is {} ms",
                ms(*duration_s),
                ms(*shortest_s)
            ),
            Self::RestMax => write!(
                f,
                "the longest rest must be {REST_MIN_SECONDS} … {REST_MAX_SECONDS} s"
            ),
            Self::Replay => write!(
                f,
                "replayed rests must be one per step, each {REST_MIN_SECONDS} … {REST_MAX_SECONDS} s"
            ),
        }
    }
}

impl std::error::Error for ProgrammeError {}

/// `s` in ms to the µs, without trailing zeros.
fn ms(s: f64) -> f64 {
    (s * 1e6).round() / 1e3
}

/// A planned programme, all lengths in whole carrier periods.
#[derive(Debug, Clone, PartialEq)]
pub struct Programme {
    /// Sample rate, Hz.
    pub sample_rate: f64,
    /// Carrier period, samples.
    pub period: u32,
    /// Onset point.
    pub onset: Onset,
    /// Rest level, dBFS RMS.
    pub pilot_dbfs: f64,
    /// Burst levels, dBFS RMS (outer loop).
    pub bursts_dbfs: Vec<f64>,
    /// Burst lengths, periods (inner loop).
    pub burst_periods: Vec<u32>,
    /// Each half of the linearity check, periods.
    pub check_periods: u32,
    /// Shortest rest, periods.
    pub rest_min_periods: u32,
    /// Longest adaptive rest, periods.
    pub rest_max_periods: u32,
    /// How rests end.
    pub rests: Rests,
}

impl Programme {
    /// Plans `cfg` at `fs`.
    pub fn new(cfg: &ProgrammeConfig, fs: f64) -> Result<Self, ProgrammeError> {
        let period = snap_period(cfg.carrier_hz, fs, cfg.onset)?;
        let p_s = f64::from(period) / fs;
        let periods = |s: f64| whole_periods(s, p_s);
        if !cfg.pilot_dbfs.is_finite()
            || cfg.bursts_dbfs.is_empty()
            || cfg.bursts_dbfs.len() > MAX_LEVELS
            || cfg
                .bursts_dbfs
                .iter()
                .any(|b| !b.is_finite() || *b <= cfg.pilot_dbfs)
        {
            return Err(ProgrammeError::Levels);
        }
        if cfg.durations_s.is_empty()
            || cfg.durations_s.len() > MAX_DURATIONS
            || cfg
                .durations_s
                .iter()
                .any(|d| !d.is_finite() || *d <= 0.0 || *d > MAX_BURST_SECONDS)
        {
            return Err(ProgrammeError::Durations);
        }
        if let Some(&d) = cfg
            .durations_s
            .iter()
            .find(|d| !burst_long_enough(**d, p_s))
        {
            return Err(ProgrammeError::ShortBurst {
                duration_s: d,
                shortest_s: shortest_burst_seconds(p_s),
                carrier_hz: fs / f64::from(period),
            });
        }
        if !(REST_MIN_SECONDS..=REST_MAX_SECONDS).contains(&cfg.rest_max_s) {
            return Err(ProgrammeError::RestMax);
        }
        let steps = cfg.bursts_dbfs.len() * cfg.durations_s.len();
        let rests = match &cfg.replay_rests_s {
            None => Rests::Adaptive,
            Some(r) => {
                // A replayed rest is as long as it was played; the bounds catch a corrupted
                // file, with one period of slack for rounding.
                let slack = p_s;
                if r.len() != steps
                    || r.iter().any(|s| {
                        !s.is_finite()
                            || *s < REST_MIN_SECONDS - slack
                            || *s > REST_MAX_SECONDS + slack
                    })
                {
                    return Err(ProgrammeError::Replay);
                }
                Rests::Replay(r.iter().map(|s| periods(*s)).collect())
            }
        };
        Ok(Self {
            sample_rate: fs,
            period,
            onset: cfg.onset,
            pilot_dbfs: cfg.pilot_dbfs,
            bursts_dbfs: cfg.bursts_dbfs.clone(),
            burst_periods: cfg.durations_s.iter().map(|d| periods(*d)).collect(),
            check_periods: periods(CHECK_SECONDS),
            rest_min_periods: periods(REST_MIN_SECONDS),
            rest_max_periods: periods(cfg.rest_max_s),
            rests,
        })
    }

    /// The carrier actually played, Hz.
    pub fn carrier_hz(&self) -> f64 {
        self.sample_rate / f64::from(self.period)
    }

    /// Number of steps (levels × durations).
    pub fn steps(&self) -> usize {
        self.bursts_dbfs.len() * self.burst_periods.len()
    }

    /// Level and duration index of step `i` (durations inner).
    pub fn step(&self, i: usize) -> (usize, usize) {
        let n = self.burst_periods.len();
        (i / n, i % n)
    }

    /// Burst level of step `i`, dBFS RMS.
    pub fn step_level_dbfs(&self, i: usize) -> f64 {
        self.bursts_dbfs[self.step(i).0]
    }

    /// Burst length of step `i`, periods.
    pub fn step_periods(&self, i: usize) -> u32 {
        self.burst_periods[self.step(i).1]
    }

    /// The highest burst level, dBFS RMS: the generator's level (its unit-RMS
    /// normalisation is taken there).
    pub fn highest_dbfs(&self) -> f64 {
        self.bursts_dbfs
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
    }

    /// Longest the programme can play, s: every rest at its longest (or its replayed
    /// length), plus the closing fade.
    pub fn longest_seconds(&self) -> f64 {
        let p = u64::from(self.period);
        let check = 2 * u64::from(self.check_periods);
        let bursts: u64 = (0..self.steps())
            .map(|i| u64::from(self.step_periods(i)))
            .sum();
        let rests: u64 = match &self.rests {
            Rests::Adaptive => self.steps() as u64 * u64::from(self.rest_max_periods),
            Rests::Replay(r) => r.iter().map(|&x| u64::from(x)).sum(),
        };
        ((check + bursts + rests) * p) as f64 / self.sample_rate
            + crate::generator::FADE_OUT_SECONDS
    }
}

/// The "released through step n" counter: the analysis publishes how many rests it has
/// found recovered, the programme source reads it (one atomic load per `fill`). Monotonic, so
/// the job thread can only shorten a rest, never lengthen one.
#[derive(Debug, Clone, Default)]
pub struct Release(Arc<AtomicU32>);

/// Two handles are equal when they are the same counter.
impl PartialEq for Release {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Release {
    /// A counter at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rests `1 ..= n` (1-based) may end.
    pub fn publish(&self, n: u32) {
        self.0.fetch_max(n, Ordering::Release);
    }

    /// Rests released so far.
    pub fn released(&self) -> u32 {
        self.0.load(Ordering::Acquire)
    }
}

// ---------------------------------------------------------------------------------------
// Analysis results

/// A time read from a gain envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Time {
    /// Seconds from the edge.
    Seconds(f64),
    /// The crossing lies inside the window skipped after the edge: one carrier period or
    /// less.
    WithinPeriod,
    /// Never happened, or the level has no settled burst to refer to.
    CantTell,
    /// The level's settled gain reduction is under 1 dB: there is nothing to time.
    NoGainReduction,
}

/// A level that may be lost under its floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading {
    /// dB.
    Db(f64),
    /// Not 6 dB above its floor.
    BelowFloor,
    /// Not measurable here.
    CantTell,
}

/// Which edge of a burst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    /// The burst's first sample.
    Start,
    /// The burst's last sample.
    End,
}

/// Harmonics `h2 … h5` and THD over one window, re the fundamental.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowHarmonics {
    /// `h2 … h5`.
    pub h: [Reading; 4],
    /// THD over `h2 … h5`.
    pub thd: Reading,
}

/// The harmonic windows of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HarmonicWindow {
    /// The first 50 ms of the burst.
    Attack,
    /// The last 10 % of the burst.
    Settled,
    /// The first 1 s after the burst.
    Release,
    /// The last second of the rest.
    Rest,
}

/// Thump after one edge: the output low-passed at 200 Hz, re the burst peak.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thump {
    /// Peak, dB.
    pub peak: Reading,
    /// RMS, dB.
    pub rms: Reading,
}

/// Overshoot in a burst.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overshoot {
    /// Largest |y| in the burst over the level's settled peak, dB.
    pub db: f64,
    /// When, s from the burst's first sample.
    pub at: f64,
    /// The reference's own overshoot the same way, dB: the floor.
    pub reference_db: f64,
}

/// One step of one measured input.
#[derive(Debug, Clone, PartialEq)]
pub struct StepRow {
    /// Gain reduction over the burst's last period, dB.
    pub gr_end: f64,
    /// The level's settled gain reduction, dB; `None` when no burst at the level settled.
    pub gr_settled: Option<f64>,
    /// The burst's gain changed less than 0.05 dB over its last 10 %.
    pub settled: bool,
    /// First time GR reaches 63 % of the settled GR.
    pub attack_tau: Time,
    /// First time after which the output stays within 2 dB of its settled level.
    pub attack_2db: Time,
    /// The same within 0.5 dB.
    pub settle_0_5db: Time,
    /// `None` when the level has no settled burst.
    pub overshoot: Option<Overshoot>,
    /// Time after the burst for GR to fall to 37 % of `gr_end`.
    pub release_tau: Time,
    /// Time after the burst until GR stays under 2 dB.
    pub recovery_2db: Time,
    /// The same under 0.5 dB.
    pub recovery_0_5db: Time,
    /// Per [`HarmonicWindow`], in its declaration order.
    pub harmonics: [WindowHarmonics; 4],
    /// Per [`Edge`]: start, end.
    pub thump: [Thump; 2],
    /// Final gain of the rest after the burst re `g₀`, dB.
    pub drift: f64,
}

/// One point of a step's envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvelopePoint {
    /// The edge `t` counts from.
    pub edge: Edge,
    /// Seconds from the edge (negative before the burst).
    pub t: f64,
    /// Gain reduction, dB.
    pub gr_db: f32,
    /// `h2 … h5` re the fundamental, dB; NaN where blind (a window holding an edge) or not
    /// 6 dB above the floor.
    pub h_db: [f32; 4],
}

/// One measured input of a run.
#[derive(Debug, Clone, PartialEq)]
pub struct InputResult {
    /// The processor's envelope delay, samples (whole samples are compensated).
    pub delay_samples: f64,
    /// Baseline gain at the pilot, dB.
    pub g0_db: f64,
    /// One row per step.
    pub rows: Vec<StepRow>,
    /// One envelope per step.
    pub envelopes: Vec<Vec<EnvelopePoint>>,
}

/// What every input shares about a step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepInfo {
    /// Burst level, dBFS RMS.
    pub level_dbfs: f64,
    /// Burst length as found in the reference, samples.
    pub burst_samples: u64,
    /// Rest length after it as played, samples.
    pub rest_samples: u64,
    /// The rest ended at the longest rest without recovering.
    pub recovered: bool,
}

/// A whole run's analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicsResult {
    /// Per step.
    pub steps: Vec<StepInfo>,
    /// Per measured input, in the order fed.
    pub inputs: Vec<InputResult>,
}

/// Where the analysis is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the programme to start.
    Waiting,
    /// In the linearity check.
    Check,
    /// In the burst of step `n` (0-based).
    Burst(u32),
    /// In the rest after step `n`.
    Rest(u32),
    /// The last rest is over: the run is complete.
    Done,
}

/// Why the analysis gave up.
#[derive(Debug, Clone, PartialEq)]
pub enum DynamicsError {
    /// The reference carries no programme.
    NoReference,
    /// The gain at the pilot differs from the gain 6 dB below it.
    PilotNotBelowThreshold {
        /// Difference, dB, of the worst input.
        diff_db: f64,
        /// That input, in feed order.
        input: usize,
    },
    /// The edges in the reference do not match the programme.
    Mismatch(String),
}

impl std::fmt::Display for DynamicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoReference => write!(f, "the reference input carries no programme"),
            Self::PilotNotBelowThreshold { diff_db, .. } => write!(
                f,
                "pilot is not below threshold: its gain differs by {diff_db:.2} dB from 6 dB below it"
            ),
            Self::Mismatch(m) => write!(f, "the reference does not match the programme: {m}"),
        }
    }
}

impl std::error::Error for DynamicsError {}

#[cfg(test)]
mod tests;
