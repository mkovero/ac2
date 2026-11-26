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
//! own noise and the path's fast jitter, `φ` being the position the servo handed out for
//! that output, which it remembers. Because `w` is zero-mean and `b` moves only slowly, the
//! stamps pin the long term and audio carries the short term: a complementary filter
//! crossing over at a few minutes. Without audio it is a stamps-only tracker: it removes the
//! stamp wander faster than about three minutes and passes most of the slower wander
//! through as phase, since nothing in the stamps tells slow wander from the device's own
//! phase. Its output follows a step in the stamps to 63 % in about three minutes (184–186 s
//! with the defaults).
//!
//! The figures behind [`ServoConfig::default`]:
//!
//! | setting | default | from |
//! |---|---|---|
//! | `stamp_white_s` | 5 µs | white part of a one-second stamp mean, 3–7 µs measured |
//! | `wander_sd_s` | 20 µs | wander sd 15.5 and 27.8 µs on the two hosts |
//! | `wander_tau_s` | 100 s | rate sd 0.22–0.47 ppm per 60 s and 0.04–0.14 per 300 s: a Gauss–Markov wander of 20 µs and 100 s reads 0.28 and 0.09 ppm |
//! | `rate_walk_ppm_per_h` | 0.1 ppm | the CM4's crystal drifts 0.07–0.25 ppm/h, a steady trend the filter's rate follows anyway; a faster walk lets the stamp wander through as rate (real stamps: rate sd 0.081 ppm at 0.5, 0.031 at 0.1) |
//! | `stamp_gate` | 4 sd | a white stamp passes it once in 16 000; one 1 ms late at a stamp a second would move the phase 16 µs and the step 2.2 ppm, gated 0.14 µs and 0.013 ppm |
//! | `path_walk_s_per_h` | 50 µs | the crossover: short-term error 0.9–2.0 µs at the hosts' wander, while a path change over 20 min stays 81 % in the bias |
//! | `path_jitter_s` | 5 µs | an indoor path up to 20 m: air moving along it at `v` changes the travel time by `L·v/c²`, 5 µs for draughts of 3 cm/s, and a path-averaged temperature change of 0.05 °C does the same; outdoors, wind gusts of 0.1–0.2 m/s over 30 m make it 25–50 µs |
//! | `steer_s` | 60 s | the step changes at most 0.08 ppm in a second on the real stamps of the software-PTP host (0.33 at the earlier 30 s and faster rate walk) |
//! | `audio_gate` | 5 sd | a re-anchored tracker or a moved path jumps by far more |
//! | `coast_after_s` | 3 s | three times the spacing of one-second stamp means, the sparsest a caller feeds |
//! | `acquire_*` | 100 µs, 1 ppm, 1 s | a phase and rate good enough to start; reached in 11.3 s from cold |
//!
//! # What it delivers
//!
//! The simulated rig in the tests (96 kHz, an hour, the device +7 ppm drifting 0.25 ppm/h),
//! five seeds at each host's stamp wander, stamps alone, from five minutes on:
//!
//! | stamp wander | phase error sd | peak-to-peak | rate error sd | step change per second |
//! |---|---|---|---|---|
//! | 15.5 µs | 6.5–13.0 µs | 34–52 µs | 0.026–0.041 ppm | ≤ 0.025 ppm |
//! | 27.8 µs | 11.5–18.1 µs | 48–76 µs | 0.031–0.045 ppm | ≤ 0.034 ppm |
//!
//! On the rigs' real hour-long stamp logs: the locked host with software PTP (where all of
//! the stamps' wander is error) 15.9 µs sd, 60 µs peak-to-peak, the step changing at most
//! 0.08 ppm in a second; the free-running USB mic with hardware PTP, against a smooth fit of
//! its own stamps, 7.9 µs sd and 40 µs peak-to-peak, 0.02 ppm per second. In both, 2 µs or
//! less of that is faster than three minutes. 16 µs is 0.6° at 100 Hz, 6° at 1 kHz and 57°
//! at 10 kHz: magnitude and coherence are unaffected, phase at high frequency is not
//! measurement grade.
//!
//! With audio throughout (path jitter 5 µs per observation): the phase error faster than
//! three minutes is 0.89–1.14 µs sd at the 15.5 µs wander and 1.57–1.99 µs at 27.8, under
//! the stated [`Status::audio_sd_s`] (2.1 µs); the long-term mean is still the stamps'.
//!
//! What it does not know:
//! - the constant path delay: audio gives `θ` only up to a constant, which the stamps place;
//! - how the path moves: slow path drift goes into `b`, faster path changes partly into
//!   `θ`. A path change of 160 µs (1 °C over 30 m) puts at its peak 58 µs (36 %) into `θ`
//!   when it happens over a minute, 53 µs (33 %) over five minutes and 31 µs (19 %) over
//!   twenty; the stamps pull the phase back after that with their own time constant.
//!
//! # Behaviour
//!
//! - **Acquire**: from cold the first stamp sets the line and a wide prior on the rate (1 %),
//!   and the filter is a weighted line fit through the first stamps until the phase and rate
//!   sd fall under the `acquire_*` bounds; until then [`Servo::block`] returns
//!   [`NotReady::Acquiring`], never a guess. A stamp beyond the gate is set aside while
//!   acquiring, and stamps that stay beyond it for `acquire_min_s` restart the line. A
//!   configuration whose stamps could never reach the bounds is refused by [`Servo::new`].
//! - **Stamp outliers**: once acquired, a stamp beyond [`ServoConfig::stamp_gate`] counts as
//!   one at the gate (Huber), so a wild stamp moves little and a genuine step is still
//!   followed at the time constant above.
//! - **Stamps stop**: blocks are still handed out on the held rate, [`Status::phase_sd_s`]
//!   grows, and after [`ServoConfig::coast_after_s`] the mode is [`Mode::Coasting`] (60 s
//!   without stamps moved the phase 0.5–5 µs on the simulated rig, 10 µs on real stamps).
//! - **Audio lost**: `b`'s uncertainty grows as its random walk, and [`Status::audio_sd_s`]
//!   with it. An observation outside the innovation gate (the tracker re-anchored, the path
//!   changed), or one flagged as a restart, sets `b` afresh from itself; the phase does not
//!   move, and later audio tells changes from there on. Only observations that moved the
//!   estimate make [`Status::audio`] true; gated ones are counted in
//!   [`Status::audio_refused`].
//! - **Gap or device restart**: [`Servo::reset_phase`] keeps the rate (the crystal did not
//!   change), its variance grown by the rate walk over the outage, and re-acquires the phase
//!   from a second of stamps; [`Servo::reset`] forgets everything.
//! - **Step limits**: the step is clamped to ±[`Step::MAX_DEVIATION`] and the status says
//!   [`Mode::Saturated`]: a wrong nominal rate or a broken clock is visible, never an error
//!   or an out-of-range step.
//! - **Smoothness**: blocks that follow each other continue exactly from the last one's end;
//!   the step is the estimated rate plus a correction that closes the gap to the estimate in
//!   `steer_s`, itself low-passed (critically damped), so the stamps' per-block noise does
//!   not reach the step: at most 0.025 ppm change per second on the simulated rig at 15.5
//!   µs of wander and 0.034 at 27.8, 0.02 and 0.08 on the two hosts' real stamps.
//!
//! # Numeric range
//!
//! Media time and sample indices run for years at 96 kHz, past where an `f64` keeps a
//! sub-microsecond. Absolute positions are 64.64 fixed point in `i128` (media time converts
//! as `ns · rate / 10⁹`, whole samples exactly and the fraction floored to 2⁻⁶⁴, for rates
//! up to [`Servo::MAX_NOMINAL_RATE`]). The filter's state refers to one
//! media position and a whole-sample offset of the nominal line, both fixed point; its `f64`
//! states are small: `θ` is kept within a sample by moving whole samples into the line after
//! each update, the rest are offsets and rates. Only differences of nearby positions become
//! `f64`.

mod config;
mod filter;
mod handed;

pub use config::{BadConfig, ServoConfig};
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

/// What [`Servo::observe_stamp`] did with a stamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampUse {
    /// The first since a (re)start: it placed the nominal line.
    Started,
    /// It moved the estimate with its full weight.
    Used,
    /// Its innovation lay beyond [`ServoConfig::stamp_gate`]: it moved the estimate with its
    /// variance inflated, as far as a stamp at the gate would; while acquiring, it was set
    /// aside.
    Outlier,
    /// Its input index is not after the newest stamp's (out of order, or repeated): ignored,
    /// since the white noise of every sample up to the newest stamp is already counted.
    OutOfOrder,
}

/// What [`Servo::observe_audio`] did with an observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioUse {
    /// It moved the estimate.
    Used,
    /// It (re)set the audio bias: the first one since a (re)start, one flagged as a restart,
    /// or one outside the innovation gate (counted in [`Status::audio_refused`]). The phase
    /// did not move.
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
    /// Acquired, but the newest stamp is older than [`ServoConfig::coast_after_s`]: blocks
    /// are still handed out on the held rate, and the phase sd grows with the time since.
    Coasting,
    /// Acquired, but the step needed is outside the resampler's range and was clamped: the
    /// nominal rate is wrong or a clock is broken.
    Saturated,
}

/// Plain figures for the UI; times in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status {
    pub mode: Mode,
    /// The device's rate against the media clock, ppm (positive: the device runs fast).
    pub rate_ppm: Option<f64>,
    /// The model's 1-sigma of [`Status::rate_ppm`], ppm, from the filter's covariance: what
    /// the rate's error would be if the stamps were as the config describes, not a
    /// measurement of it. It starts at the 1 % prior, is at most
    /// [`ServoConfig::acquire_rate_ppm`] once acquired and falls with the span of stamps
    /// (at [`ServoConfig::cross_rig`], one stamp a second: 1.0 ppm at acquisition after
    /// 23 s, 0.88 after 28 s, 0.38 after 100 s, 0.16 after 280 s); over a [`Servo::reset_phase`] it is kept with the
    /// rate and grows by the rate walk until stamps return. `None` with no rate: before the
    /// first stamp or after [`Servo::reset`].
    pub rate_sd_ppm: Option<f64>,
    /// The model's 1-sigma of the phase against the stamps' long-term line, now: what the
    /// error would be if the stamps were as the config describes. It is close to a constant
    /// of the config while stamps arrive (14.1 µs at the defaults) and grows while coasting;
    /// stamps worse than configured show in [`Status::stamp_scatter_s`], not here.
    pub phase_sd_s: Option<f64>,
    /// Measured: the rms of one-second means of the stamps about the tracked phase,
    /// exponentially weighted over the last ten minutes; `None` until a second of stamps
    /// after acquiring. With stamps as the config describes it reads about the part of the
    /// wander faster than the tracker follows (13–15 µs at the defaults on the simulated
    /// rig); well above that, the stamps are worse than configured and the phase error is
    /// larger than [`Status::phase_sd_s`] by about the same factor. Wander slower than the
    /// tracker's few minutes is followed, and invisible to stamps alone.
    pub stamp_scatter_s: Option<f64>,
    /// 1-sigma of the phase against the audio path (the short-term figure), now; `None`
    /// until audio has anchored the bias since the last (re)start.
    pub audio_sd_s: Option<f64>,
    /// Whether audio moved the estimate within [`ServoConfig::audio_hold_s`].
    pub audio: bool,
    /// Audio observations the innovation gate refused since the last (re)start: rising
    /// while [`Status::audio`] is false, audio arrives but does not fit the clock.
    pub audio_refused: u64,
    /// Seconds since the newest stamp's media time.
    pub since_stamp_s: Option<f64>,
}

/// A running measure of the stamps' scatter about the tracked phase: one-second means
/// (so the stamps' white part counts as in [`ServoConfig::stamp_white_s`]), their squares
/// averaged with an exponential window whose weights are normalised, so the figure is a
/// plain mean of what there is while the window fills.
#[derive(Clone, Copy, Debug, Default)]
struct Scatter {
    sum: f64,
    span: f64,
    mean_square: f64,
    weight: f64,
}

/// The scatter's window: many wander correlation times, so it reads the wander's variance
/// rather than its last few excursions.
const SCATTER_WINDOW_S: f64 = 1800.0;

impl Scatter {
    /// One stamp's residual (seconds) standing for `spacing` seconds of stamps.
    fn add(&mut self, residual: f64, spacing: f64) {
        self.sum += residual * spacing;
        self.span += spacing;
        if self.span >= 1.0 {
            let mean = self.sum / self.span;
            self.weight = self.weight * (-self.span / SCATTER_WINDOW_S).exp() + self.span;
            self.mean_square += self.span / self.weight * (mean * mean - self.mean_square);
            self.sum = 0.0;
            self.span = 0.0;
        }
    }

    fn rms(&self) -> Option<f64> {
        (self.weight > 0.0).then(|| self.mean_square.sqrt())
    }
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
    /// Media position of the newest stamp.
    last_stamp: i128,
    /// Input index of the stamp with the highest index.
    last_index: u64,
    /// Seconds of input between the last two stamps taken.
    last_spacing: Option<f64>,
    acquired: bool,
    bias_set: bool,
    last_audio: Option<i128>,
    audio_refused: u64,
    scatter: Scatter,
    /// While acquiring: media position of the first of the latest run of stamps beyond the
    /// gate.
    outlier_since: Option<i128>,
}

/// The rate kept over [`Servo::reset_phase`], and the media position of the newest stamp
/// before it, from which its variance ages.
#[derive(Clone, Copy, Debug)]
struct Kept {
    rate: f64,
    var: f64,
    at: i128,
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
    kept: Option<Kept>,
    handed: Handed,
}

/// The rate's sd before any stamp: a percent, far wider than any crystal, so that a wrong
/// nominal rate is still found (and reported as saturated).
const RATE_PRIOR: f64 = 1e-2;

impl Servo {
    /// The highest nominal rate [`Servo::new`] takes: media positions of every `u64` of
    /// nanoseconds then stay under 2⁶² samples (`2⁶⁴ ns · rate / 10⁹`), so 64.64 positions
    /// and their sums with device offsets fit an `i128`.
    pub const MAX_NOMINAL_RATE: u32 = 250_000_000;

    /// A servo for a device of nominal rate `nominal_rate` Hz, which is also the media
    /// clock's rate.
    pub fn new(cfg: ServoConfig, nominal_rate: u32) -> Result<Self, BadConfig> {
        cfg.check()?;
        if nominal_rate == 0 || nominal_rate > Self::MAX_NOMINAL_RATE {
            return Err(BadConfig::NominalRate);
        }
        let fs = f64::from(nominal_rate);
        Ok(Self {
            cfg,
            fs,
            rate: nominal_rate,
            model: cfg.model(fs),
            track: None,
            kept: None,
            handed: Handed::new(nominal_rate),
        })
    }

    pub fn config(&self) -> &ServoConfig {
        &self.cfg
    }

    /// Forgets everything, the rate included: a different device, or a fresh start.
    pub fn reset(&mut self) {
        self.track = None;
        self.kept = None;
        self.handed.clear();
    }

    /// After a gap in the input or a device restart: the phase is gone (and so is the audio
    /// bias, since the interface's constants may have changed), but the crystal is the same,
    /// so the rate is kept, its variance grown by the rate walk over the time until the
    /// next stamp, and the phase re-acquires from about [`ServoConfig::acquire_min_s`] of
    /// stamps. Positions handed out before refer to the old sample numbering; the next block
    /// starts afresh.
    pub fn reset_phase(&mut self) {
        if let Some(t) = &self.track {
            self.kept = Some(Kept {
                rate: t.f.x[RATE],
                var: t.f.p[RATE][RATE],
                at: t.at.max(t.last_stamp),
            });
        }
        self.track = None;
        self.handed.clear();
    }

    /// Media time as a 64.64 media position, `ns · rate / 10⁹`: whole samples exactly, the
    /// fraction floored to 2⁻⁶⁴.
    fn media(&self, at: MediaTime) -> i128 {
        let n = u128::from(at.0) * u128::from(self.rate);
        let (q, r) = (n / 1_000_000_000, n % 1_000_000_000);
        // q < 2⁶² by MAX_NOMINAL_RATE and r < 2³⁰: both shifts fit, and so does the sum.
        ((q << 64) + (r << 64) / 1_000_000_000) as i128
    }

    fn seconds(&self, media_diff: i128) -> f64 {
        fix::to_f64(media_diff) / self.fs
    }

    /// One capture stamp: device input sample `input_index` was captured at media time `at`.
    /// Feed every block's stamp: the white noise is modelled as a density, each stamp
    /// standing for the input since the one before (by index, so the weight does not depend
    /// on the block length), and a stamp after missing ones for no more than the spacing
    /// before them, since it is still one block's capture time.
    pub fn observe_stamp(&mut self, input_index: u64, at: MediaTime) -> StampUse {
        let m = self.media(at);
        let Some(t) = self.track.as_mut() else {
            self.start(input_index, m);
            return StampUse::Started;
        };
        if input_index <= t.last_index {
            return StampUse::OutOfOrder;
        }
        let gap = (input_index - t.last_index) as f64 / self.fs;
        let spacing = t.last_spacing.map_or(gap, |s| gap.min(s));
        t.last_spacing = Some(gap);
        t.last_index = input_index;
        let r = (self.cfg.stamp_white_s * self.fs).powi(2) / spacing;
        let lag = advance(t, m, self.fs, &self.model);
        let device = fix::from_int(i128::from(input_index));
        let z = fix::to_f64(device - m - t.line);
        // The state is at `m` or later; θ at the stamp is θ − rate·lag. The wander is taken
        // as unchanged over the lag (seconds against its correlation time of minutes).
        let h = [1.0, -lag, 1.0, 0.0];
        let (nu, s) = t.f.innovation(&h, z, r);
        if t.acquired {
            // The stamp about the tracked phase: the innovation plus the wander estimate.
            t.scatter.add((nu + t.f.x[WANDER]) / self.fs, spacing);
        }
        let c = self.cfg.stamp_gate * s.sqrt();
        let outlier = nu.abs() > c;
        if !t.acquired {
            // While the line is still being found a stamp at the gate would bend it for
            // minutes, so one beyond the gate is set aside; stamps that stay beyond it for
            // `acquire_min_s` are a step, and the line starts again from them.
            if outlier {
                let since = *t.outlier_since.get_or_insert(m);
                if fix::to_f64(m - since) / self.fs >= self.cfg.acquire_min_s {
                    self.start(input_index, m);
                    return StampUse::Started;
                }
                return StampUse::Outlier;
            }
            t.outlier_since = None;
        }
        // Huber: beyond the gate the variance grows with the innovation, so the stamp moves
        // the state exactly as one at the gate would, and a real step is followed a few
        // stamps later.
        let (r, used) = if outlier {
            (s * nu.abs() / c - (s - r), StampUse::Outlier)
        } else {
            (r, StampUse::Used)
        };
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
        used
    }

    /// The first stamp after a (re)start: the nominal line through it, the phase from it.
    fn start(&mut self, input_index: u64, m: i128) {
        let device = fix::from_int(i128::from(input_index));
        let line = fix::round(device - m);
        let theta = fix::to_f64(device - m - line);
        let wander = self.model.wander_var;
        // Its block length is unknown: taken as one sample, the least it can be, so the first
        // stamp weighs least and the next ones place the phase.
        let white = (self.cfg.stamp_white_s * self.fs).powi(2) * self.fs;
        let (rate, rate_var) = self
            .kept
            .map_or((0.0, (RATE_PRIOR * self.fs).powi(2)), |k| {
                let since = self.seconds(m - k.at).max(0.0);
                (k.rate, k.var + self.model.q_rate * since)
            });
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
            last_index: input_index,
            last_spacing: None,
            acquired: false,
            bias_set: false,
            last_audio: None,
            audio_refused: 0,
            scatter: Scatter::default(),
            outlier_since: None,
        });
    }

    /// One delay from the tracker that compares this servo's output with a stream on the
    /// reference clock: `delay_s` is how much later the servo's output carries the same
    /// content than the reference stream does (positive: the output lags), measured over
    /// output around media time `at`. A tracker that reports the delay of the reference
    /// against the measured channel — the re-timed stream being the reference channel of
    /// the transfer function — reports minus this; the caller negates it.
    ///
    /// `sigma_s` is the tracker's own 1-sigma; the path's fast jitter
    /// ([`ServoConfig::path_jitter_s`]) is added to it here. Only the delay's changes carry
    /// clock information; the constant (path delay and interface latencies) is learnt as the
    /// bias. `restart` says the tracker re-anchored or the path changed: the bias is set
    /// afresh and the phase does not move.
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
        let r = (sigma_s.powi(2) + self.cfg.path_jitter_s.powi(2)) * self.fs.powi(2);
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
                t.audio_refused += 1;
                AudioUse::BiasReset
            } else {
                t.f.update(&h, z, r);
                t.last_audio = Some(t.last_audio.map_or(m, |a| a.max(m)));
                AudioUse::Used
            }
        };
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
            .record(first, len, pos, next, continuing.is_none());
        self.handed.clamped = dev != wanted;
        self.handed.correction = correction.clamp(-Step::MAX_DEVIATION, Step::MAX_DEVIATION);
        Ok((Pos::from_raw(raw), step))
    }

    /// The servo's state at media time `now` (a time before the newest observation reads as
    /// that observation's time).
    pub fn status(&self, now: MediaTime) -> Status {
        let Some(t) = &self.track else {
            let kept_sd = |k: Kept| {
                let since = self.seconds(self.media(now) - k.at).max(0.0);
                (k.var + self.model.q_rate * since).sqrt() / self.fs * 1e6
            };
            return Status {
                mode: Mode::Acquiring,
                rate_ppm: self.kept.map(|k| k.rate / self.fs * 1e6),
                rate_sd_ppm: self.kept.map(kept_sd),
                phase_sd_s: None,
                stamp_scatter_s: None,
                audio_sd_s: None,
                audio: false,
                audio_refused: 0,
                since_stamp_s: None,
            };
        };
        let m = self.media(now);
        let mut f = t.f;
        f.predict(self.seconds(m - t.at).max(0.0), &self.model);
        let rate = f.x[RATE] / self.fs;
        let since_stamp = self.seconds(m - t.last_stamp).max(0.0);
        let mode = if !t.acquired {
            Mode::Acquiring
        } else if self.handed.clamped || rate.abs() > Step::MAX_DEVIATION {
            Mode::Saturated
        } else if since_stamp > self.cfg.coast_after_s {
            Mode::Coasting
        } else {
            Mode::Tracking
        };
        let audio_sd_s = t
            .bias_set
            .then(|| f.variance(&[1.0, 0.0, 0.0, 1.0]).sqrt() / self.fs);
        Status {
            mode,
            rate_ppm: Some(rate * 1e6),
            rate_sd_ppm: Some(f.p[RATE][RATE].sqrt() / self.fs * 1e6),
            phase_sd_s: Some(f.p[THETA][THETA].sqrt() / self.fs),
            stamp_scatter_s: t.scatter.rms(),
            audio_sd_s,
            audio: t
                .last_audio
                .is_some_and(|a| self.seconds(m - a) <= self.cfg.audio_hold_s),
            audio_refused: t.audio_refused,
            since_stamp_s: Some(since_stamp),
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
