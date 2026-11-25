//! The clock servo: where each media-clock output sample falls in a free-running device's
//! input-sample time, as a [`Pos`] and [`Step`] per output block for the [`Resampler`].
//!
//! The media clock is the system clock (disciplined by PTP) times the nominal sample rate:
//! output sample `k` is media time `k / rate`. A device whose crystal runs free (a USB
//! measurement mic at +7 ppm, drifting 0.25 ppm/h) is re-timed onto it. A device locked to the
//! house reference needs none of this and stays bit-exact; choosing between the two is the
//! caller's policy, not the servo's.
//!
//! [`Resampler`]: crate::Resampler
//!
//! ```
//! use ac2_resampler::servo::{MediaTime, Servo, ServoConfig};
//!
//! let mut servo = Servo::new(ServoConfig::default(), 96_000)?;
//! // A device 5 ppm fast, one stamp per 9600-sample block, 20 s of them.
//! for b in 0..200u64 {
//!     let index = b * 9600;
//!     let ns = (index as f64 / 96_000.0 / (1.0 + 5e-6) * 1e9) as u64;
//!     servo.observe_stamp(index, MediaTime::from_nanos(1_000_000_000 + ns));
//! }
//! let (pos, step) = servo.block(96_000 * 20, 512)?;
//! assert!((step.get() - (1.0 + 5e-6)).abs() < 1e-6);
//! # let _ = pos;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Two kinds of evidence
//!
//! **Stamps**: pairs (device input index, capture time on the system clock). They hold the
//! long term but err in a coloured way. Measured over an hour on the rigs, as the error of the
//! capture time against the true line: a white part of 3–7 µs in a one-second mean; a slow
//! wander of 15.5 µs sd / 94 µs peak-to-peak (a USB mic on a host with hardware PTP) to 27.8
//! µs sd / 242 µs p-p (a host with software PTP whose interface is locked, so all of it is
//! stamp-path error); the rate read from windows moves 0.22–0.47 ppm sd over 60 s and
//! 0.04–0.14 ppm over 300 s. The wander lives at tens of seconds to minutes and averages
//! out only slowly.
//!
//! **Audio**: while a transfer function runs between this (resampled) stream and one on the
//! reference clock, its delay tracker measures the delay between them to well under 1 µs
//! within seconds. That delay holds the path (acoustic or cable) and interface constants as
//! well, so only its changes carry clock information, and an acoustic path drifts by itself
//! (0.16 ms per °C over 30 m; wind). It is measured on the servo's own output, so it sees
//! the servo's residual error plus that bias. It comes and goes with signal and coherence.
//!
//! # Model
//!
//! A Kalman filter in device samples, four states:
//! - `θ`, the device's phase against the nominal line (what the resampler needs);
//! - `r`, its rate (samples per second): a random walk, the crystal's drift;
//! - `w`, the stamp-path wander: first-order Gauss–Markov, zero mean;
//! - `b`, the audio bias (path delay and constants): a random walk slow enough to follow
//!   path drift on a scale of minutes.
//!
//! A stamp measures `θ + w` plus white noise; an audio delay measures `θ − φ + b` plus its
//! own noise, `φ` being the position the servo handed out for that output, which it
//! remembers. Because `w` is zero-mean and `b` moves only slowly, the stamps pin the long
//! term and audio carries the short term: a complementary filter crossing over at a few
//! minutes. Without audio it is a stamps-only tracker whose output follows a step in the
//! stamps to 63 % in about a minute (54 s with the defaults).
//!
//! The figures behind [`ServoConfig::default`]:
//!
//! | setting | default | from |
//! |---|---|---|
//! | `stamp_white_s` | 5 µs | white part of a one-second stamp mean, 3–7 µs measured |
//! | `wander_sd_s` | 20 µs | wander sd 15.5 and 27.8 µs on the two hosts |
//! | `wander_tau_s` | 100 s | rate sd 0.22–0.47 ppm per 60 s and 0.04–0.14 per 300 s: a Gauss–Markov wander of 20 µs and 100 s reads 0.28 and 0.09 ppm |
//! | `rate_walk_ppm_per_h` | 0.5 ppm | twice the measured crystal trend of 0.25 ppm/h, so a warming room is followed |
//! | `path_walk_s_per_h` | 50 µs | the crossover: short-term error under 2 µs at the 15.5 µs wander, while a path change over 20 min stays 78 % in the bias |
//! | `steer_s` | 30 s | step change under 0.1 ppm per second with stamps alone |
//! | `audio_gate` | 5 sd | a re-anchored tracker or a moved path jumps by far more |
//! | `acquire_*` | 100 µs, 1 ppm, 1 s | a phase and rate good enough to start; reached in about 11 s from cold |
//!
//! # What it delivers (the simulated rig in the tests, 96 kHz, an hour)
//!
//! With the 15.5 µs wander, stamps alone: phase error −5.4 ± 8.6 µs, rate error 0.04 ppm
//! sd, the step changing at most 0.06 ppm in any second. 8.6 µs is 0.3° at 100 Hz, 3° at 1 kHz, 31° at 10 kHz: magnitude
//! and coherence are unaffected, phase at high frequency is not measurement grade.
//! With audio throughout: phase error faster than three minutes 1.8 µs sd, the long-term
//! mean still the stamps' (−3.4 µs over the hour); at the other host's 27.8 µs wander the
//! short-term figure is 2.6–3.3 µs.
//!
//! What it does not know:
//! - the constant path delay: audio gives `θ` only up to a constant, which the stamps place;
//! - how the path moves: slow path drift goes into `b`, faster path changes partly into
//!   `θ`. A path change of 160 µs (1 °C over 30 m) puts at its peak 107 µs (67 %) into `θ`
//!   when it happens over a minute, 82 µs (51 %) over five minutes and 35 µs (22 %) over
//!   twenty; the stamps pull the phase back after that with their own time constant.
//!
//! # Behaviour
//!
//! - **Acquire**: from cold the first stamp sets the line and a wide prior on the rate (1 %),
//!   and the filter is a weighted line fit through the first stamps until the phase and rate
//!   sd fall under the `acquire_*` bounds; until then [`Servo::block`] returns
//!   [`NotReady::Acquiring`], never a guess.
//! - **Audio lost**: `b`'s uncertainty grows as its random walk, and [`Status::audio_sd_s`]
//!   with it. An observation outside the innovation gate (the tracker re-anchored, the path
//!   changed), or one flagged as a restart, sets `b` afresh from itself; the phase does not
//!   move, and later audio tells changes from there on.
//! - **Gap or device restart**: [`Servo::reset_phase`] keeps the rate (the crystal did not
//!   change) and re-acquires the phase from a second of stamps; [`Servo::reset`] forgets
//!   everything.
//! - **Step limits**: the step is clamped to ±[`Step::MAX_DEVIATION`] and the status says
//!   [`Mode::Saturated`]: a wrong nominal rate or a broken clock is visible, never an error
//!   or an out-of-range step.
//! - **Smoothness**: blocks that follow each other continue exactly from the last one's end;
//!   the step is the estimated rate plus a correction that closes the gap to the estimate in
//!   `steer_s`, itself low-passed (critically damped), so the stamps' per-block noise does
//!   not reach the step: under 0.1 ppm change per second while tracking.
//!
//! # Numeric range
//!
//! Media time and sample indices run for years at 96 kHz, past where an `f64` keeps a
//! sub-microsecond. Absolute positions are 64.64 fixed point in `i128` (media time converts
//! exactly: `ns · rate / 10⁹` as integer and remainder). The filter's state refers to one
//! media position and a whole-sample offset of the nominal line, both fixed point; its `f64`
//! states are small: `θ` is kept within a sample by moving whole samples into the line after
//! each update, the rest are offsets and rates. Only differences of nearby positions become
//! `f64`.

mod filter;
mod handed;

use filter::{Filter, Model, RATE, THETA, WANDER};
use handed::Handed;

use crate::{Pos, Step};

/// 64.64 fixed point in `i128`: absolute media and device positions keep 2⁻⁶⁴ of a sample
/// at any index, and only differences of nearby positions become `f64`.
mod fix {
    const ONE: f64 = 18_446_744_073_709_551_616.0;

    pub const fn from_int(n: i128) -> i128 {
        n << 64
    }

    /// Exact to the nearest `f64` for differences, whose size is seconds of samples.
    pub fn to_f64(v: i128) -> f64 {
        v as f64 / ONE
    }

    /// Rounded to 2⁻⁶⁴; saturates far outside any position.
    pub fn from_f64(v: f64) -> i128 {
        (v * ONE).round() as i128
    }

    /// The nearest whole sample.
    pub fn round(v: i128) -> i128 {
        (v + (1 << 63)) >> 64 << 64
    }
}

/// A time on the media clock — the system clock the caller keeps disciplined (PTP) — in
/// nanoseconds since that clock's epoch. Output sample `k` of the media clock is media time
/// `k / nominal rate` seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MediaTime(u64);

impl MediaTime {
    pub const fn from_nanos(ns: u64) -> Self {
        Self(ns)
    }

    pub const fn nanos(self) -> u64 {
        self.0
    }
}

/// Noise figures and loop settings; [`Default`] gives the figures measured on the rigs (see
/// the module docs for where each comes from).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ServoConfig {
    /// White part of the stamp error: the sd of a one-second mean of stamps, seconds.
    pub stamp_white_s: f64,
    /// Stamp-path wander: stationary sd, seconds.
    pub wander_sd_s: f64,
    /// Stamp-path wander: correlation time, seconds.
    pub wander_tau_s: f64,
    /// Crystal rate random walk: sd after one hour, ppm.
    pub rate_walk_ppm_per_h: f64,
    /// Audio path random walk: sd after one hour, seconds.
    pub path_walk_s_per_h: f64,
    /// Time constant with which handed-out positions close on the estimate, seconds.
    pub steer_s: f64,
    /// Audio innovation beyond this many sd re-initialises the bias instead of moving the
    /// phase.
    pub audio_gate: f64,
    /// Audio counts as contributing this long after its last accepted observation, seconds.
    pub audio_hold_s: f64,
    /// Acquired when the phase sd is below this, seconds …
    pub acquire_phase_s: f64,
    /// … the rate sd below this, ppm …
    pub acquire_rate_ppm: f64,
    /// … and stamps have spanned at least this long since the (re)start, seconds.
    pub acquire_min_s: f64,
}

impl Default for ServoConfig {
    fn default() -> Self {
        Self {
            stamp_white_s: 5e-6,
            wander_sd_s: 20e-6,
            wander_tau_s: 100.0,
            rate_walk_ppm_per_h: 0.5,
            path_walk_s_per_h: 50e-6,
            steer_s: 30.0,
            audio_gate: 5.0,
            audio_hold_s: 5.0,
            acquire_phase_s: 100e-6,
            acquire_rate_ppm: 1.0,
            acquire_min_s: 1.0,
        }
    }
}

/// The field of a [`ServoConfig`] (or the nominal rate) that was refused: each must be
/// finite and positive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadConfig {
    NominalRate,
    StampWhite,
    WanderSd,
    WanderTau,
    RateWalk,
    PathWalk,
    Steer,
    AudioGate,
    AudioHold,
    AcquirePhase,
    AcquireRate,
    AcquireMin,
}

impl std::fmt::Display for BadConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "servo setting {self:?} must be finite and positive")
    }
}

impl std::error::Error for BadConfig {}

impl ServoConfig {
    fn check(&self) -> Result<(), BadConfig> {
        use BadConfig as B;
        for (v, e) in [
            (self.stamp_white_s, B::StampWhite),
            (self.wander_sd_s, B::WanderSd),
            (self.wander_tau_s, B::WanderTau),
            (self.rate_walk_ppm_per_h, B::RateWalk),
            (self.path_walk_s_per_h, B::PathWalk),
            (self.steer_s, B::Steer),
            (self.audio_gate, B::AudioGate),
            (self.audio_hold_s, B::AudioHold),
            (self.acquire_phase_s, B::AcquirePhase),
            (self.acquire_rate_ppm, B::AcquireRate),
            (self.acquire_min_s, B::AcquireMin),
        ] {
            if !(v.is_finite() && v > 0.0) {
                return Err(e);
            }
        }
        Ok(())
    }
}

/// Why [`Servo::block`] has no position to give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotReady {
    /// No stamps yet, or not enough to place a sample within the acquisition bounds.
    Acquiring,
    /// The position falls before device index 0 or past the last representable one.
    OutOfRange,
}

impl std::fmt::Display for NotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Acquiring => write!(f, "clock servo still acquiring"),
            Self::OutOfRange => write!(f, "clock servo position outside the device's samples"),
        }
    }
}

impl std::error::Error for NotReady {}

/// What [`Servo::observe_audio`] did with an observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioUse {
    /// It moved the estimate.
    Used,
    /// It (re)set the audio bias: the first one since a (re)start, one flagged as a restart,
    /// or one outside the innovation gate. The phase did not move.
    BiasReset,
    /// It refers to output the servo has not handed out (or handed out before a
    /// non-contiguous block or a reset), or is not finite: ignored.
    Unplaced,
}

/// The servo's mode, for the UI and the gate upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Acquiring,
    Tracking,
    /// Tracking, but the step needed is outside the resampler's range and was clamped: the
    /// nominal rate is wrong or a clock is broken.
    Saturated,
}

/// Plain figures for the UI; times in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status {
    pub mode: Mode,
    /// The device's rate against the media clock, ppm (positive: the device runs fast).
    pub rate_ppm: Option<f64>,
    /// 1-sigma of the phase against the stamps' long-term line, now.
    pub phase_sd_s: Option<f64>,
    /// 1-sigma of the phase against the audio path (the short-term figure), now; `None`
    /// until audio has anchored the bias since the last (re)start.
    pub audio_sd_s: Option<f64>,
    /// Whether audio was accepted within [`ServoConfig::audio_hold_s`].
    pub audio: bool,
    /// Seconds since the newest stamp's media time.
    pub since_stamp_s: Option<f64>,
}

/// Everything known since the first stamp after a (re)start.
#[derive(Clone, Copy, Debug)]
struct Track {
    f: Filter,
    /// Media position (64.64) the filter state refers to.
    at: i128,
    /// Whole-sample offset (64.64) of the nominal line: device position = media position +
    /// `line` + θ.
    line: i128,
    first_stamp: i128,
    last_stamp: i128,
    acquired: bool,
    bias_set: bool,
    last_audio: Option<i128>,
}

/// Re-times a free-running device onto the media clock: from stamps and audio delays, the
/// device position of each media-clock output sample. See the module docs.
#[derive(Clone, Debug)]
pub struct Servo {
    cfg: ServoConfig,
    fs: f64,
    rate: u32,
    model: Model,
    track: Option<Track>,
    /// Rate and its variance kept over [`Servo::reset_phase`].
    kept_rate: Option<(f64, f64)>,
    handed: Handed,
}

/// The rate's sd before any stamp: a percent, far wider than any crystal, so that a wrong
/// nominal rate is still found (and reported as saturated).
const RATE_PRIOR: f64 = 1e-2;

/// Stamps closer than this are not taken to average the white noise further.
const MIN_STAMP_SPACING_S: f64 = 1e-3;

impl Servo {
    /// A servo for a device of nominal rate `nominal_rate` Hz, which is also the media
    /// clock's rate.
    pub fn new(cfg: ServoConfig, nominal_rate: u32) -> Result<Self, BadConfig> {
        cfg.check()?;
        if nominal_rate == 0 {
            return Err(BadConfig::NominalRate);
        }
        let fs = f64::from(nominal_rate);
        let q_rate = (cfg.rate_walk_ppm_per_h * 1e-6 * fs).powi(2) / 3600.0;
        let model = Model {
            q_rate,
            wander_var: (cfg.wander_sd_s * fs).powi(2),
            wander_tau: cfg.wander_tau_s,
            q_bias: (cfg.path_walk_s_per_h * fs).powi(2) / 3600.0,
        };
        Ok(Self {
            cfg,
            fs,
            rate: nominal_rate,
            model,
            track: None,
            kept_rate: None,
            handed: Handed::new(nominal_rate),
        })
    }

    pub fn config(&self) -> &ServoConfig {
        &self.cfg
    }

    /// Forgets everything, the rate included: a different device, or a fresh start.
    pub fn reset(&mut self) {
        self.track = None;
        self.kept_rate = None;
        self.handed.clear();
    }

    /// After a gap in the input or a device restart: the phase is gone (and so is the audio
    /// bias, since the interface's constants may have changed), but the crystal is the same,
    /// so the rate and its uncertainty are kept and the phase re-acquires from about
    /// [`ServoConfig::acquire_min_s`] of stamps. Positions handed out before refer to the old
    /// sample numbering; the next block starts afresh.
    pub fn reset_phase(&mut self) {
        if let Some(t) = &self.track {
            self.kept_rate = Some((t.f.x[RATE], t.f.p[RATE][RATE]));
        }
        self.track = None;
        self.handed.clear();
    }

    /// Media time as a 64.64 media position: `ns · rate / 10⁹` exactly.
    fn media(&self, at: MediaTime) -> i128 {
        let n = u128::from(at.0) * u128::from(self.rate);
        let (q, r) = (n / 1_000_000_000, n % 1_000_000_000);
        // q < 2⁵² for any u64 of nanoseconds, r < 2³⁰: both shifts fit.
        ((q << 64) + (r << 64) / 1_000_000_000) as i128
    }

    fn seconds(&self, media_diff: i128) -> f64 {
        fix::to_f64(media_diff) / self.fs
    }

    /// One capture stamp: device input sample `input_index` was captured at media time `at`.
    /// Feed every block's stamp (the white noise is modelled as a density, so denser stamps
    /// weigh more); stamps out of order are taken at their own time.
    pub fn observe_stamp(&mut self, input_index: u64, at: MediaTime) {
        let m = self.media(at);
        let device = fix::from_int(i128::from(input_index));
        let Some(t) = self.track.as_mut() else {
            self.start(device, m);
            return;
        };
        let spacing = (fix::to_f64(m - t.last_stamp) / self.fs).clamp(MIN_STAMP_SPACING_S, 1.0);
        let r = (self.cfg.stamp_white_s * self.fs).powi(2) / spacing;
        let lag = advance(t, m, self.fs, &self.model);
        let z = fix::to_f64(device - m - t.line);
        // The state is at `m` or later; θ at the stamp is θ − rate·lag. The wander is taken
        // as unchanged over the lag (seconds against its correlation time of minutes).
        let h = [1.0, -lag, 1.0, 0.0];
        t.f.update(&h, z, r);
        t.last_stamp = t.last_stamp.max(m);
        recentre(t);
        if !t.acquired
            && fix::to_f64(t.last_stamp - t.first_stamp) / self.fs >= self.cfg.acquire_min_s
            && t.f.p[THETA][THETA].sqrt() / self.fs <= self.cfg.acquire_phase_s
            && t.f.p[RATE][RATE].sqrt() / self.fs * 1e6 <= self.cfg.acquire_rate_ppm
        {
            t.acquired = true;
        }
    }

    /// The first stamp after a (re)start: the nominal line through it, the phase from it.
    fn start(&mut self, device: i128, m: i128) {
        let line = fix::round(device - m);
        let theta = fix::to_f64(device - m - line);
        let wander = self.model.wander_var;
        let white = (self.cfg.stamp_white_s * self.fs).powi(2) / MIN_STAMP_SPACING_S;
        let (rate, rate_var) = self
            .kept_rate
            .unwrap_or((0.0, (RATE_PRIOR * self.fs).powi(2)));
        let mut p = [[0.0; 4]; 4];
        // θ̂ = θ + wander + white, and the wander's estimate is 0: their errors are opposed.
        p[THETA][THETA] = wander + white;
        p[THETA][WANDER] = -wander;
        p[WANDER][THETA] = -wander;
        p[WANDER][WANDER] = wander;
        p[RATE][RATE] = rate_var;
        self.track = Some(Track {
            f: Filter {
                x: [theta, rate, 0.0, 0.0],
                p,
            },
            at: m,
            line,
            first_stamp: m,
            last_stamp: m,
            acquired: false,
            bias_set: false,
            last_audio: None,
        });
    }

    /// One delay from the tracker that compares this servo's output with a stream on the
    /// reference clock: `delay_s` measured over output around media time `at`, with 1-sigma
    /// `sigma_s`. Only its changes carry clock information; the constant (path delay and
    /// interface latencies) is learnt as the bias. `restart` says the tracker re-anchored or
    /// the path changed: the bias is set afresh and the phase does not move.
    pub fn observe_audio(
        &mut self,
        delay_s: f64,
        sigma_s: f64,
        at: MediaTime,
        restart: bool,
    ) -> AudioUse {
        let m = self.media(at);
        let Some(t) = self.track.as_mut() else {
            return AudioUse::Unplaced;
        };
        let Some(handed) = self.handed.position_at(m) else {
            return AudioUse::Unplaced;
        };
        if !(delay_s.is_finite() && sigma_s.is_finite() && sigma_s > 0.0) {
            return AudioUse::Unplaced;
        }
        let lag = advance(t, m, self.fs, &self.model);
        // The output at `m` was taken from device position `handed`; the device's true
        // position there is the line plus θ. Output taken e samples behind the truth carries
        // content e samples older: the tracker sees the path delay plus e/fs. So
        // delay·fs + (handed − line − m) = θ + bias.
        let z = delay_s * self.fs + fix::to_f64(handed - m - t.line);
        let r = (sigma_s * self.fs).powi(2);
        let g = [1.0, -lag, 0.0, 0.0];
        let h = [1.0, -lag, 0.0, 1.0];
        let used = if restart || !t.bias_set {
            t.f.reset_bias(&g, z, r);
            t.bias_set = true;
            AudioUse::BiasReset
        } else {
            let (nu, s) = t.f.innovation(&h, z, r);
            if nu * nu > self.cfg.audio_gate.powi(2) * s {
                t.f.reset_bias(&g, z, r);
                AudioUse::BiasReset
            } else {
                t.f.update(&h, z, r);
                AudioUse::Used
            }
        };
        t.last_audio = Some(t.last_audio.map_or(m, |a| a.max(m)));
        recentre(t);
        used
    }

    /// Position and step for `len` output samples from media-clock output index `first`.
    /// A block that starts where the last one ended continues from its exact end, so the
    /// output never jumps; any other block starts on the estimate.
    pub fn block(&mut self, first: u64, len: usize) -> Result<(Pos, Step), NotReady> {
        let Some(t) = self.track.as_ref().filter(|t| t.acquired) else {
            return Err(NotReady::Acquiring);
        };
        let k = fix::from_int(i128::from(first));
        let ahead = self.seconds(k - t.at);
        let estimate = k + t.line + fix::from_f64(t.f.x[THETA] + t.f.x[RATE] * ahead);
        let continuing = self.handed.continuing(first);
        let pos = continuing.unwrap_or(estimate);
        let gap = fix::to_f64(estimate - pos);
        // The correction that closes the gap in `steer_s` is itself low-passed over a
        // quarter of that, which damps the loop critically: the step then carries no trace
        // of the per-stamp corrections the estimate makes.
        let target = gap / (self.cfg.steer_s * self.fs);
        let mut correction = if continuing.is_some() {
            self.handed.correction
        } else {
            0.0
        };
        let blend = (len as f64 / self.fs / (self.cfg.steer_s / 4.0)).min(1.0);
        correction += (target - correction) * blend;
        let wanted = t.f.x[RATE] / self.fs + correction;
        let dev = if wanted.is_finite() {
            wanted.clamp(-Step::MAX_DEVIATION, Step::MAX_DEVIATION)
        } else {
            0.0
        };
        let step = Step::from_deviation(dev).map_err(|_| NotReady::OutOfRange)?;
        let raw = u128::try_from(pos).map_err(|_| NotReady::OutOfRange)?;
        let next = i128::try_from(step.raw())
            .ok()
            .and_then(|s| s.checked_mul(len as i128))
            .and_then(|d| pos.checked_add(d))
            .ok_or(NotReady::OutOfRange)?;
        self.handed
            .record(first, len, pos, step.get(), next, continuing.is_none());
        self.handed.clamped = dev != wanted;
        self.handed.correction = correction.clamp(-Step::MAX_DEVIATION, Step::MAX_DEVIATION);
        Ok((Pos::from_raw(raw), step))
    }

    /// The servo's state at media time `now` (a time before the newest observation reads as
    /// that observation's time).
    pub fn status(&self, now: MediaTime) -> Status {
        let Some(t) = &self.track else {
            return Status {
                mode: Mode::Acquiring,
                rate_ppm: self.kept_rate.map(|(r, _)| r / self.fs * 1e6),
                phase_sd_s: None,
                audio_sd_s: None,
                audio: false,
                since_stamp_s: None,
            };
        };
        let m = self.media(now);
        let mut f = t.f;
        f.predict(self.seconds(m - t.at).max(0.0), &self.model);
        let rate = f.x[RATE] / self.fs;
        let mode = if !t.acquired {
            Mode::Acquiring
        } else if self.handed.clamped || rate.abs() > Step::MAX_DEVIATION {
            Mode::Saturated
        } else {
            Mode::Tracking
        };
        let audio_sd_s = t
            .bias_set
            .then(|| f.variance(&[1.0, 0.0, 0.0, 1.0]).sqrt() / self.fs);
        Status {
            mode,
            rate_ppm: Some(rate * 1e6),
            phase_sd_s: Some(f.p[THETA][THETA].sqrt() / self.fs),
            audio_sd_s,
            audio: t
                .last_audio
                .is_some_and(|a| self.seconds(m - a) <= self.cfg.audio_hold_s),
            since_stamp_s: Some(self.seconds(m - t.last_stamp).max(0.0)),
        }
    }
}

/// Moves the filter forward to media position `m` if it is ahead; returns how far (seconds)
/// the state is past `m`.
fn advance(t: &mut Track, m: i128, fs: f64, model: &Model) -> f64 {
    let dt = fix::to_f64(m - t.at) / fs;
    if dt > 0.0 {
        t.f.predict(dt, model);
        t.at = m;
        0.0
    } else {
        -dt
    }
}

/// Keeps θ within a sample of the nominal line by moving whole samples into the line: θ
/// grows by the rate (0.7 samples a second at 7 ppm and 96 kHz) and would otherwise carry an
/// absolute offset into the `f64` state.
fn recentre(t: &mut Track) {
    let whole = t.f.x[THETA].round();
    if whole != 0.0 {
        t.f.x[THETA] -= whole;
        t.line += fix::from_int(whole as i128);
    }
}
