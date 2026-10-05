//! Generator-to-loopback timing estimator (design Q3).
//!
//! The mapping watched here is `capture_index = output_index + offset`. Three layers, all
//! pure (no threads, no I/O):
//!
//! - [`GccPhat`]: GCC-PHAT between a capture window and the matching slice of generator
//!   history, with parabolic sub-sample peak and peak-to-sidelobe ratio.
//! - [`GccPhat::measure`]: applies the stimulus, loopback-level and confidence floors and
//!   turns one window into a [`WindowMeasurement`].
//! - [`TimingTracker`]: the NoStimulus / Acquiring / Locked / Jumped / Lost state machine and
//!   the drift line ([`drift`]), fed one measurement per hop.
//!
//! [`LoopbackTiming`] bundles the three for the daemon job.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

pub mod drift;

pub use drift::{DriftEstimate, DriftLine};

/// Window length at 48 kHz; other rates use the nearest power of two to the same duration.
pub const WINDOW_AT_48K: usize = 1 << 15;

/// Hop between windows, s.
pub const HOP_SECONDS: f64 = 0.25;

/// Analysis window length for a sample rate: W = 2^15 at 48 kHz, scaled with the rate to the
/// nearest power of two.
pub fn window_for_rate(sample_rate: f64) -> usize {
    let exact = WINDOW_AT_48K as f64 * sample_rate / 48_000.0;
    1usize << exact.log2().round().max(10.0) as u32
}

/// Inclusive range of offsets (samples) a window searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LagRange {
    /// Smallest offset searched.
    pub min: i64,
    /// Largest offset searched.
    pub max: i64,
}

impl LagRange {
    /// Number of offsets beyond the first: `max − min`.
    pub fn span(&self) -> usize {
        (self.max - self.min).max(0) as usize
    }

    /// Length of the generator-history slice a window of `window` samples needs.
    pub fn reference_len(&self, window: usize) -> usize {
        window + self.span()
    }

    /// Output index of the first history sample for a window starting at capture index
    /// `capture_start`: the slice covers output indices
    /// `[capture_start − max, capture_start + window − min)`. May be negative before the
    /// stream has run long enough; such samples are zero.
    pub fn reference_start(&self, capture_start: u64) -> i64 {
        capture_start as i64 - self.max
    }

    /// Output index of the first of the newest `window` samples of that slice, the ones
    /// stimulus presence is judged on: `capture_start − min`.
    pub fn newest_start(&self, capture_start: u64) -> i64 {
        capture_start as i64 - self.min
    }
}

/// Parameters of the monitor. [`TimingConfig::for_rate`] gives the Q3 defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimingConfig {
    /// Sample rate, Hz.
    pub sample_rate: f64,
    /// Window length W, samples.
    pub window: usize,
    /// Hop between windows, samples.
    pub hop: usize,
    /// Offsets searched while acquiring (default 0 … 1 s).
    pub acquisition: LagRange,
    /// Half-width of the tracking search around the locked offset.
    pub track_radius: i64,
    /// Minimum peak-to-sidelobe ratio (peak over RMS of all other lags), dB.
    pub psr_floor_db: f64,
    /// Generator-history level below which there is no stimulus, dBFS RMS.
    pub stimulus_floor_dbfs: f64,
    /// Loopback level below which a window is not measured, dBFS RMS.
    pub loopback_floor_dbfs: f64,
    /// Windows of one acquisition or jump candidate agree when each lies within this many
    /// samples of the candidate's line.
    pub agree_samples: f64,
    /// A locked window further than this from the drift line's prediction (or three times
    /// the prediction's uncertainty, if larger) is a jump candidate, samples.
    pub jump_samples: f64,
    /// Largest slope a candidate line may have, ppm. The stimulus slides by `W·ε` within
    /// one window, which smears the correlation peak over that many lags (6.5 samples at
    /// 200 ppm with 0.68 s windows), so single windows scatter along the smear. Drift is
    /// followed reliably up to about 200 ppm; around 300 ppm the lock comes and goes, and
    /// far beyond the bound the monitor reports Lost rather than a line through scatter.
    pub max_drift_ppm: f64,
    /// Agreeing windows needed to lock; the first and last must not overlap.
    pub acquire_windows: usize,
    /// Consecutive unmeasurable windows after which the state is Lost.
    pub lost_after_windows: usize,
    /// Drift regression length, s.
    pub drift_window_s: f64,
    /// Shortest regression span on which drift is judged, s.
    pub drift_min_span_s: f64,
    /// Drift above which input and output are considered on different clocks, ppm.
    pub drift_threshold_ppm: f64,
}

impl TimingConfig {
    /// Q3 defaults for a sample rate.
    pub fn for_rate(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            window: window_for_rate(sample_rate),
            hop: (HOP_SECONDS * sample_rate).round() as usize,
            acquisition: LagRange {
                min: 0,
                max: sample_rate.round() as i64,
            },
            track_radius: 64,
            psr_floor_db: 20.0,
            stimulus_floor_dbfs: -100.0,
            loopback_floor_dbfs: -80.0,
            agree_samples: 1.0,
            jump_samples: 0.5,
            max_drift_ppm: 500.0,
            acquire_windows: 3,
            lost_after_windows: 4,
            drift_window_s: 30.0,
            drift_min_span_s: 10.0,
            drift_threshold_ppm: 2.0,
        }
    }
}

/// Why a window could not be processed (a caller bug, not a measurement outcome).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingError {
    /// Capture length differs from the configured window.
    CaptureLength,
    /// History length differs from `range.reference_len(window)`.
    ReferenceLength,
    /// The search range is wider than the estimator was built for.
    RangeTooWide,
}

impl std::fmt::Display for TimingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CaptureLength => write!(f, "capture slice length differs from the window"),
            Self::ReferenceLength => write!(f, "history slice length does not match the range"),
            Self::RangeTooWide => write!(f, "search range wider than the estimator supports"),
        }
    }
}

impl std::error::Error for TimingError {}

/// Correlation peak of one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    /// Integer offset (capture − output), samples.
    pub offset: i64,
    /// Parabolic sub-sample correction in (−0.5, 0.5); diagnostics only.
    pub fraction: f64,
    /// Peak over RMS of all other lags (excluding the main lobe), dB.
    pub psr_db: f64,
}

/// Why a window with stimulus present produced no offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoEstimate {
    /// Loopback level below the floor.
    LowLoopbackLevel,
    /// Correlation peak not clear enough.
    LowConfidence,
}

/// Outcome of one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// Generator history below the stimulus floor.
    NoStimulus,
    /// Stimulus present but no trustworthy offset.
    NoEstimate(NoEstimate),
    /// A confident offset.
    Offset(Peak),
}

/// One window's measurement, the tracker's only input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowMeasurement {
    /// Capture index of the window's first sample.
    pub capture_start: u64,
    /// What the window showed.
    pub outcome: Outcome,
    /// Loopback (capture) level, dBFS RMS.
    pub loopback_dbfs: f64,
    /// Level of the newest W generator-history samples, dBFS RMS.
    pub stimulus_dbfs: f64,
}

struct Plan {
    n: usize,
    fwd: Arc<dyn RealToComplex<f64>>,
    inv: Arc<dyn ComplexToReal<f64>>,
    a: Vec<f64>,
    b: Vec<f64>,
    sa: Vec<Complex<f64>>,
    sb: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan").field("n", &self.n).finish()
    }
}

/// GCC-PHAT estimator with all FFT plans and buffers allocated up front.
#[derive(Debug)]
pub struct GccPhat {
    window: usize,
    max_span: usize,
    plans: Vec<Plan>,
    /// Capture taper, see [`capture_taper`].
    taper: Vec<f64>,
}

/// Fraction of the capture window tapered at each end.
const TAPER_FRACTION: f64 = 0.125;

/// Raised-cosine (Tukey) taper for the capture window. PHAT whitening gives every bin unit
/// weight, so when the stimulus is narrowband (a sweep's first second is a 20–35 Hz tone)
/// the bins outside its band carry only the leakage of the window's truncation steps. Those
/// steps then correlate with the reference slice's own ends and the peak lands exactly on
/// an end of the searched range (offset `min` or `max`) with a high peak-to-sidelobe ratio.
/// Fading the capture in and out removes its steps, so out-of-band bins are noise and stay
/// incoherent; the flat middle keeps most of the window's energy for the true peak.
fn capture_taper(window: usize) -> Vec<f64> {
    let edge = ((window as f64 * TAPER_FRACTION) as usize).max(1);
    (0..window)
        .map(|i| {
            let d = i.min(window - 1 - i);
            if d >= edge {
                1.0
            } else {
                0.5 - 0.5 * (std::f64::consts::PI * (d as f64 + 0.5) / edge as f64).cos()
            }
        })
        .collect()
}

/// Lags within this distance of the peak are its main lobe and excluded from the sidelobe
/// statistic.
const MAIN_LOBE: usize = 8;

impl GccPhat {
    /// Estimator for windows of `window` samples and search spans up to `max_span`.
    /// Allocates one plan per power-of-two FFT size the spans can need.
    pub fn new(window: usize, max_span: usize) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        // Zero-padded to window + span so the searched lags never wrap.
        let smallest = (window + 1).next_power_of_two();
        let largest = (window + max_span + 1).next_power_of_two();
        let mut plans = Vec::new();
        let mut n = smallest;
        while n <= largest {
            let fwd = planner.plan_fft_forward(n);
            let inv = planner.plan_fft_inverse(n);
            let scratch_len = fwd.get_scratch_len().max(inv.get_scratch_len());
            plans.push(Plan {
                n,
                a: vec![0.0; n],
                b: vec![0.0; n],
                sa: fwd.make_output_vec(),
                sb: fwd.make_output_vec(),
                scratch: vec![Complex::new(0.0, 0.0); scratch_len],
                fwd,
                inv,
            });
            n *= 2;
        }
        Self {
            window,
            max_span,
            plans,
            taper: capture_taper(window),
        }
    }

    /// Window length.
    pub fn window(&self) -> usize {
        self.window
    }

    /// GCC-PHAT peak of `capture` (W samples from `capture_start`) against `reference`
    /// (generator history from output index `range.reference_start(capture_start)`).
    /// Returns `None` if either input is silent.
    pub fn correlate(
        &mut self,
        capture: &[f32],
        reference: &[f32],
        range: LagRange,
    ) -> Result<Option<Peak>, TimingError> {
        if capture.len() != self.window {
            return Err(TimingError::CaptureLength);
        }
        let span = range.span();
        if span > self.max_span {
            return Err(TimingError::RangeTooWide);
        }
        if reference.len() != range.reference_len(self.window) {
            return Err(TimingError::ReferenceLength);
        }
        let need = (self.window + span + 1).next_power_of_two();
        let Some(p) = self.plans.iter_mut().find(|p| p.n >= need) else {
            return Err(TimingError::RangeTooWide);
        };
        let n = p.n;
        p.a.fill(0.0);
        p.b.fill(0.0);
        for ((d, &s), w) in p.a.iter_mut().zip(capture).zip(&self.taper) {
            *d = f64::from(s) * w;
        }
        for (d, &s) in p.b.iter_mut().zip(reference) {
            *d = f64::from(s);
        }
        // Sizes match the plan, so the transforms cannot fail.
        let _ = p
            .fwd
            .process_with_scratch(&mut p.a, &mut p.sa, &mut p.scratch);
        let _ = p
            .fwd
            .process_with_scratch(&mut p.b, &mut p.sb, &mut p.scratch);
        // r[k] = Σ c[i]·ref[i + k]  ↔  conj(C)·REF; PHAT keeps only the phase.
        let mut mean_mag = 0.0;
        for (x, y) in p.sa.iter_mut().zip(&p.sb) {
            *x = x.conj() * y;
            mean_mag += x.norm();
        }
        mean_mag /= p.sa.len() as f64;
        if mean_mag <= 0.0 || !mean_mag.is_finite() {
            return Ok(None);
        }
        // A small floor keeps empty bins (band-limited stimulus) from being amplified.
        let floor = 1e-6 * mean_mag;
        for x in p.sa.iter_mut() {
            *x /= x.norm() + floor;
        }
        let last = p.sa.len() - 1;
        p.sa[0] = Complex::new(0.0, 0.0);
        p.sa[last] = Complex::new(0.0, 0.0);
        let _ = p
            .inv
            .process_with_scratch(&mut p.sa, &mut p.a, &mut p.scratch);
        let r = &p.a;

        let mut k_best = 0;
        let mut best = f64::MIN;
        for (k, v) in r.iter().enumerate().take(span + 1) {
            if v.abs() > best {
                best = v.abs();
                k_best = k;
            }
        }
        let mut side_e = 0.0;
        let mut side_n = 0usize;
        for (k, v) in r.iter().enumerate() {
            let dist = k.abs_diff(k_best).min(n - k.abs_diff(k_best));
            if dist > MAIN_LOBE {
                side_e += v * v;
                side_n += 1;
            }
        }
        let side_rms = (side_e / side_n.max(1) as f64).sqrt();
        let psr_db = 20.0 * (best / side_rms).log10();
        let delta = if k_best > 0 && k_best < span {
            let (ym, y0, yp) = (r[k_best - 1].abs(), best, r[k_best + 1].abs());
            let den = ym - 2.0 * y0 + yp;
            if den < 0.0 {
                (0.5 * (ym - yp) / den).clamp(-0.5, 0.5)
            } else {
                0.0
            }
        } else {
            0.0
        };
        // Lag k corresponds to offset range.max − k.
        Ok(Some(Peak {
            offset: range.max - k_best as i64,
            fraction: -delta,
            psr_db,
        }))
    }

    /// Correlates one window and applies the floors in `cfg`.
    pub fn measure(
        &mut self,
        cfg: &TimingConfig,
        capture_start: u64,
        capture: &[f32],
        reference: &[f32],
        range: LagRange,
    ) -> Result<WindowMeasurement, TimingError> {
        if capture.len() != self.window {
            return Err(TimingError::CaptureLength);
        }
        if reference.len() != range.reference_len(self.window) {
            return Err(TimingError::ReferenceLength);
        }
        let newest = range.span();
        if let Some(m) =
            self.measure_no_stimulus(cfg, capture_start, capture, &reference[newest..])?
        {
            return Ok(m);
        }
        let stimulus_dbfs = level_dbfs(&reference[newest..]);
        let loopback_dbfs = level_dbfs(capture);
        let outcome = if loopback_dbfs < cfg.loopback_floor_dbfs {
            Outcome::NoEstimate(NoEstimate::LowLoopbackLevel)
        } else {
            match self.correlate(capture, reference, range)? {
                Some(p) if p.psr_db >= cfg.psr_floor_db => Outcome::Offset(p),
                _ => Outcome::NoEstimate(NoEstimate::LowConfidence),
            }
        };
        Ok(WindowMeasurement {
            capture_start,
            outcome,
            loopback_dbfs,
            stimulus_dbfs,
        })
    }

    /// The window's measurement if its stimulus is below the floor, judged from `newest`
    /// alone: the W history samples from `range.newest_start(capture_start)`. A wide search
    /// range reaches up to a second back and would otherwise see a generator that has
    /// stopped, so presence never depends on the older part of the slice, and a window
    /// without stimulus needs nothing else. `None`: the stimulus is present and the window
    /// needs [`Self::measure`] with its whole slice.
    pub fn measure_no_stimulus(
        &self,
        cfg: &TimingConfig,
        capture_start: u64,
        capture: &[f32],
        newest: &[f32],
    ) -> Result<Option<WindowMeasurement>, TimingError> {
        // Lengths are checked even in silence so a caller bug is never masked by it.
        if capture.len() != self.window {
            return Err(TimingError::CaptureLength);
        }
        if newest.len() != self.window {
            return Err(TimingError::ReferenceLength);
        }
        let stimulus_dbfs = level_dbfs(newest);
        Ok(
            (stimulus_dbfs < cfg.stimulus_floor_dbfs).then(|| WindowMeasurement {
                capture_start,
                outcome: Outcome::NoStimulus,
                loopback_dbfs: level_dbfs(capture),
                stimulus_dbfs,
            }),
        )
    }
}

/// RMS level in dBFS (0 dBFS = full-scale sine RMS, design Q4); −∞ for silence.
fn level_dbfs(x: &[f32]) -> f64 {
    if x.is_empty() {
        return f64::NEG_INFINITY;
    }
    let p = x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64;
    10.0 * (2.0 * p).log10()
}

// ---------------------------------------------------------------------------------------
// State machine

/// Monitor state (Q3 diagram).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimingState {
    /// No generator output; [`TimingTracker::last_lock`] still reports the last lock.
    NoStimulus,
    /// New epoch or stimulus restart; collecting agreeing windows.
    Acquiring,
    /// Offset validated.
    Locked {
        /// Current offset, samples.
        offset: i64,
    },
    /// A jump was confirmed by the last window; the next window reports Locked.
    Jumped {
        /// Offset before the jump.
        from: i64,
        /// Offset after the jump.
        to: i64,
    },
    /// Stimulus present but no confident offset for several windows; searching wide.
    Lost,
}

/// Events the daemon publishes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimingEvent {
    /// First lock in an epoch, or re-lock at the epoch's previous offset.
    Locked {
        /// Offset epoch.
        epoch: u64,
        /// Offset, samples.
        offset: i64,
        /// Capture index of the window that completed the lock.
        at_capture_sample: u64,
    },
    /// The offset changed within an epoch (`timing.jump`).
    Jump {
        /// Offset epoch.
        epoch: u64,
        /// Offset before, samples.
        from: i64,
        /// Offset after, samples.
        to: i64,
        /// Start of the first capture window showing the new offset; the jump lies at or
        /// before the end of that window.
        at_capture_sample: u64,
    },
    /// Confidence lost while the stimulus is present.
    Lost {
        /// Offset epoch.
        epoch: u64,
        /// Capture index of the window that declared it.
        at_capture_sample: u64,
    },
    /// The generator stopped.
    StimulusOff {
        /// Offset epoch.
        epoch: u64,
        /// Capture index of the first window without stimulus.
        at_capture_sample: u64,
    },
    /// Drift exceeded the threshold: input and output are on different clocks.
    DriftWarning {
        /// Offset epoch.
        epoch: u64,
        /// Estimated drift, ppm (positive: offset grows).
        ppm: f64,
    },
}

/// Up to two events from one window (for example a lock completing while drift is judged).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TimingEvents {
    slots: [Option<TimingEvent>; 2],
}

impl TimingEvents {
    fn push(&mut self, e: TimingEvent) {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.is_none()) {
            *slot = Some(e);
        }
    }

    /// The events, in order.
    pub fn iter(&self) -> impl Iterator<Item = &TimingEvent> {
        self.slots.iter().flatten()
    }

    /// True if no event happened.
    pub fn is_empty(&self) -> bool {
        self.slots[0].is_none()
    }
}

/// The most recent lock, kept across stimulus gaps and epochs for display with its age.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LastLock {
    /// Epoch it belongs to.
    pub epoch: u64,
    /// Offset, samples.
    pub offset: i64,
    /// Capture index of the last window that confirmed it.
    pub at_capture_sample: u64,
}

/// Windows that agree on one offset line: an acquisition, or the new offset after a jump.
#[derive(Debug, Clone)]
struct Candidate {
    /// Integer offset of the newest window.
    offset: i64,
    /// Capture start of the first window.
    first_start: u64,
    /// (window centre, offset) of each window, oldest first.
    points: Vec<(f64, f64)>,
}

impl Candidate {
    fn new(x: f64, y: f64, offset: i64, start: u64) -> Self {
        let mut points = Vec::with_capacity(8);
        points.push((x, y));
        Self {
            offset,
            first_start: start,
            points,
        }
    }

    fn count(&self) -> usize {
        self.points.len()
    }

    /// Whether a window at (`x`, `y`) lies on this candidate's line: its own slope once it
    /// has two windows, else `slope` (the drift known so far); with neither, any drift up
    /// to the largest followed is allowed. The candidate's own windows take precedence, so
    /// a drift line fitted on too short a span cannot keep a correct candidate out.
    fn agrees(&self, x: f64, y: f64, slope: Option<f64>, cfg: &TimingConfig) -> bool {
        let (&(x0, y0), &(x1, y1)) = match (self.points.first(), self.points.last()) {
            (Some(a), Some(b)) => (a, b),
            _ => return false,
        };
        let own = (x1 > x0).then(|| (y1 - y0) / (x1 - x0));
        let max_slope = cfg.max_drift_ppm * 1e-6;
        match own.or(slope) {
            Some(s) if s.abs() <= max_slope + cfg.agree_samples / (x1 - x0).max(1.0) => {
                (y - (y1 + s * (x - x1))).abs() <= cfg.agree_samples
            }
            Some(_) => false,
            None => (y - y1).abs() <= cfg.agree_samples + max_slope * (x - x1).abs(),
        }
    }

    fn add(&mut self, x: f64, y: f64, offset: i64) {
        self.points.push((x, y));
        self.offset = offset;
    }
}

/// Q3 state machine and drift model; pure logic over [`WindowMeasurement`]s.
///
/// Every window is judged against the offset the epoch's [`DriftLine`] predicts for it, so
/// a drifting offset is followed however fast it moves (up to
/// [`TimingConfig::max_drift_ppm`]) and a step is told from drift by its size against the
/// prediction's uncertainty (`docs/design/multi-device.md` §5).
#[derive(Debug)]
pub struct TimingTracker {
    cfg: TimingConfig,
    epoch: u64,
    state: TimingState,
    last_lock: Option<LastLock>,
    acquire: Option<Candidate>,
    jump: Option<Candidate>,
    misses: usize,
    /// Consecutive locked windows off the line that did not confirm a jump.
    off_line: usize,
    wide_next: bool,
    /// Offsets of the epoch's validated windows; kept through Lost and NoStimulus so a
    /// re-lock elsewhere than the line predicts is reported as a jump instead of passing
    /// silently.
    line: DriftLine,
    drift_warning: bool,
}

impl TimingTracker {
    /// Tracker in epoch 0, state NoStimulus.
    pub fn new(cfg: TimingConfig) -> Self {
        let capacity =
            (cfg.drift_window_s * cfg.sample_rate / cfg.hop.max(1) as f64).ceil() as usize + 4;
        Self {
            cfg,
            epoch: 0,
            state: TimingState::NoStimulus,
            last_lock: None,
            acquire: None,
            jump: None,
            misses: 0,
            off_line: 0,
            wide_next: false,
            line: DriftLine::new(
                cfg.sample_rate,
                cfg.drift_window_s,
                cfg.drift_min_span_s,
                cfg.drift_threshold_ppm,
                capacity,
            ),
            drift_warning: false,
        }
    }

    /// Current state.
    pub fn state(&self) -> TimingState {
        self.state
    }

    /// Current offset epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Last validated lock, possibly from an earlier epoch.
    pub fn last_lock(&self) -> Option<LastLock> {
        self.last_lock
    }

    /// The generator may stand in for the reference only when locked on one clock.
    pub fn internal_reference_allowed(&self) -> bool {
        matches!(self.state, TimingState::Locked { .. }) && !self.drift_warning
    }

    /// Starts a new offset epoch (stream open, device/rate/buffer change, continuity break,
    /// xrun). The offset may legitimately change, so the next lock is not a jump. The
    /// judged drift stays: the clocks are those of the same stream.
    pub fn new_epoch(&mut self) {
        self.epoch += 1;
        self.line.clear();
        self.acquire = None;
        self.jump = None;
        self.misses = 0;
        self.off_line = 0;
        self.wide_next = false;
        if self.state != TimingState::NoStimulus {
            self.state = TimingState::Acquiring;
        }
    }

    /// Range the next window should search.
    pub fn search_range(&self) -> LagRange {
        let locked = match self.state {
            TimingState::Locked { offset } | TimingState::Jumped { to: offset, .. } => offset,
            _ => return self.cfg.acquisition,
        };
        let r = self.cfg.track_radius;
        let candidate_far = self
            .jump
            .as_ref()
            .is_some_and(|c| (c.offset - locked).abs() as f64 > r as f64 - self.cfg.agree_samples);
        if self.wide_next || candidate_far {
            self.cfg.acquisition
        } else {
            LagRange {
                min: locked - r,
                max: locked + r,
            }
        }
    }

    /// Drift between the output and the input clock: the current regression once it spans
    /// enough to judge, else the last judged one of this stream, else the short current one.
    pub fn drift(&self) -> Option<DriftEstimate> {
        self.line.estimate()
    }

    /// Feeds one window's measurement.
    pub fn observe(&mut self, m: &WindowMeasurement) -> TimingEvents {
        let mut ev = TimingEvents::default();
        if let TimingState::Jumped { to, .. } = self.state {
            self.state = TimingState::Locked { offset: to };
        }
        match m.outcome {
            Outcome::NoStimulus => {
                if self.state != TimingState::NoStimulus {
                    self.state = TimingState::NoStimulus;
                    ev.push(TimingEvent::StimulusOff {
                        epoch: self.epoch,
                        at_capture_sample: m.capture_start,
                    });
                }
                self.reset_search();
            }
            Outcome::NoEstimate(_) => {
                if self.state == TimingState::NoStimulus {
                    self.state = TimingState::Acquiring;
                }
                self.misses += 1;
                self.wide_next = true;
                if self.misses >= self.cfg.lost_after_windows && self.state != TimingState::Lost {
                    self.state = TimingState::Lost;
                    self.reset_search();
                    ev.push(TimingEvent::Lost {
                        epoch: self.epoch,
                        at_capture_sample: m.capture_start,
                    });
                }
            }
            Outcome::Offset(p) => {
                if self.state == TimingState::NoStimulus {
                    self.state = TimingState::Acquiring;
                }
                self.misses = 0;
                match self.state {
                    TimingState::Locked { offset } => self.track(offset, p, m, &mut ev),
                    _ => self.acquire(p, m, &mut ev),
                }
            }
        }
        let drift = self.drift().filter(|d| d.warning);
        if let Some(d) = drift
            && !self.drift_warning
        {
            ev.push(TimingEvent::DriftWarning {
                epoch: self.epoch,
                ppm: d.ppm,
            });
        }
        self.drift_warning = drift.is_some();
        ev
    }

    fn reset_search(&mut self) {
        self.acquire = None;
        self.jump = None;
        self.off_line = 0;
    }

    /// Capture index a window's offset belongs to: its centre.
    fn centre(&self, m: &WindowMeasurement) -> f64 {
        m.capture_start as f64 + self.cfg.window as f64 / 2.0
    }

    /// Distance from the line's prediction beyond which a window is a jump candidate:
    /// three standard deviations of a window's offset around the line, never less than
    /// [`TimingConfig::jump_samples`]. Under drift a window's stimulus slides by `W·ε`
    /// while it is captured, and its peak can sit anywhere along that smear (σ of a uniform
    /// spread, `W·ε/√12`). That scatter is a property of the drift, so it holds even right
    /// after a lock, when a few windows cannot yet show it in their residuals.
    fn jump_threshold(&self, sigma: f64) -> f64 {
        let smear = self.line.slope().map_or(0.0, |s| s.abs()) * self.cfg.window as f64;
        let sigma = sigma.max(smear / 12f64.sqrt());
        self.cfg.jump_samples.max(3.0 * sigma)
    }

    fn acquire(&mut self, p: Peak, m: &WindowMeasurement, ev: &mut TimingEvents) {
        let (x, y) = (self.centre(m), p.offset as f64 + p.fraction);
        let slope = self.line.slope();
        let c = match self.acquire.take() {
            Some(mut c) if c.agrees(x, y, slope, &self.cfg) => {
                c.add(x, y, p.offset);
                c
            }
            _ => Candidate::new(x, y, p.offset, m.capture_start),
        };
        let disjoint = m.capture_start >= c.first_start + self.cfg.window as u64;
        if c.count() < self.cfg.acquire_windows || !disjoint {
            self.acquire = Some(c);
            return;
        }
        self.wide_next = false;
        // A re-lock within the epoch (after a gap or a loss) is judged against the drift line
        // extrapolated across the gap, at the newest window: the first may straddle the
        // stimulus onset.
        let (xn, yn) = c.points[c.count() - 1];
        match self.line.predict(xn) {
            Some(q) if (yn - q.offset).abs() > self.jump_threshold(q.sigma) => {
                let from = q.offset.round() as i64;
                self.line.shift(yn - q.offset);
                self.state = TimingState::Jumped { from, to: p.offset };
                ev.push(TimingEvent::Jump {
                    epoch: self.epoch,
                    from,
                    to: p.offset,
                    at_capture_sample: c.first_start,
                });
            }
            _ => {
                self.state = TimingState::Locked { offset: p.offset };
                ev.push(TimingEvent::Locked {
                    epoch: self.epoch,
                    offset: p.offset,
                    at_capture_sample: m.capture_start,
                });
            }
        }
        for &(px, py) in &c.points {
            self.line.push(px, py);
        }
        self.confirm(p, m);
    }

    fn track(&mut self, locked: i64, p: Peak, m: &WindowMeasurement, ev: &mut TimingEvents) {
        let (x, y) = (self.centre(m), p.offset as f64 + p.fraction);
        let on_line = match self.line.predict(x) {
            Some(q) => (y - q.offset).abs() <= self.jump_threshold(q.sigma),
            None => (y - locked as f64).abs() <= self.cfg.agree_samples,
        };
        if on_line {
            // Clock drift moves the offset steadily; the line follows it.
            self.jump = None;
            self.off_line = 0;
            self.wide_next = false;
            self.state = TimingState::Locked { offset: p.offset };
            self.line.push(x, y);
            self.confirm(p, m);
            return;
        }
        let slope = self.line.slope();
        let c = match self.jump.take() {
            Some(mut c) if c.agrees(x, y, slope, &self.cfg) => {
                c.add(x, y, p.offset);
                c
            }
            _ => Candidate::new(x, y, p.offset, m.capture_start),
        };
        if m.capture_start < c.first_start + self.cfg.window as u64 {
            self.jump = Some(c);
            self.off_line += 1;
            // Off the line, yet no new line forms: the windows scatter more than the model
            // allows (drift beyond the largest followed, or a stimulus the estimator cannot
            // time). Locked would claim an offset nothing confirms any more.
            let confirm_windows = self.cfg.window.div_ceil(self.cfg.hop.max(1));
            if self.off_line > confirm_windows + self.cfg.lost_after_windows {
                self.state = TimingState::Lost;
                self.reset_search();
                self.wide_next = true;
                ev.push(TimingEvent::Lost {
                    epoch: self.epoch,
                    at_capture_sample: m.capture_start,
                });
            }
            return;
        }
        self.off_line = 0;
        // The step is measured against the drifted offset at the newest window: the first
        // one may straddle the step and read part of it.
        let (xn, yn) = c.points[c.count() - 1];
        if let Some(q) = self.line.predict(xn) {
            self.line.shift(yn - q.offset);
        } else {
            self.line.clear();
        }
        for &(px, py) in &c.points {
            self.line.push(px, py);
        }
        self.wide_next = false;
        self.state = TimingState::Jumped {
            from: locked,
            to: p.offset,
        };
        ev.push(TimingEvent::Jump {
            epoch: self.epoch,
            from: locked,
            to: p.offset,
            at_capture_sample: c.first_start,
        });
        self.confirm(p, m);
    }

    /// Records a validated offset as the last lock.
    fn confirm(&mut self, p: Peak, m: &WindowMeasurement) {
        self.last_lock = Some(LastLock {
            epoch: self.epoch,
            offset: p.offset,
            at_capture_sample: m.capture_start,
        });
    }
}

/// Estimator plus tracker: the whole Q3 monitor minus threads and I/O.
///
/// Per hop: ask [`LoopbackTiming::search_range`], slice W capture samples from
/// `capture_start` and `range.reference_len(W)` history samples from output index
/// `range.reference_start(capture_start)`, then call [`LoopbackTiming::process_window`].
#[derive(Debug)]
pub struct LoopbackTiming {
    cfg: TimingConfig,
    estimator: GccPhat,
    tracker: TimingTracker,
}

impl LoopbackTiming {
    /// Builds the monitor; allocates FFT plans for tracking and acquisition spans.
    pub fn new(cfg: TimingConfig) -> Self {
        let max_span = cfg
            .acquisition
            .span()
            .max(2 * cfg.track_radius.max(0) as usize);
        Self {
            estimator: GccPhat::new(cfg.window, max_span),
            tracker: TimingTracker::new(cfg),
            cfg,
        }
    }

    /// Configuration.
    pub fn config(&self) -> &TimingConfig {
        &self.cfg
    }

    /// The state machine.
    pub fn tracker(&self) -> &TimingTracker {
        &self.tracker
    }

    /// Range the next window should search.
    pub fn search_range(&self) -> LagRange {
        self.tracker.search_range()
    }

    /// Starts a new offset epoch.
    pub fn new_epoch(&mut self) {
        self.tracker.new_epoch();
    }

    /// Measures one window and feeds the tracker.
    pub fn process_window(
        &mut self,
        capture_start: u64,
        capture: &[f32],
        reference: &[f32],
        range: LagRange,
    ) -> Result<(WindowMeasurement, TimingEvents), TimingError> {
        let m = self
            .estimator
            .measure(&self.cfg, capture_start, capture, reference, range)?;
        Ok((m, self.tracker.observe(&m)))
    }

    /// Feeds the tracker a window whose stimulus is below the floor, judged from only the
    /// newest W history samples (see [`GccPhat::measure_no_stimulus`]); the outcome, levels
    /// and events are exactly those [`Self::process_window`] gives for the same window.
    /// `None`, with nothing fed: the stimulus is present, so the window needs
    /// [`Self::process_window`] with its whole history slice.
    pub fn process_if_no_stimulus(
        &mut self,
        capture_start: u64,
        capture: &[f32],
        newest: &[f32],
    ) -> Result<Option<(WindowMeasurement, TimingEvents)>, TimingError> {
        let m = self
            .estimator
            .measure_no_stimulus(&self.cfg, capture_start, capture, newest)?;
        Ok(m.map(|m| (m, self.tracker.observe(&m))))
    }
}

#[cfg(test)]
mod tests;
