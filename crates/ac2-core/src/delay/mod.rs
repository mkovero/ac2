//! Delay finder and tracking on the raw, unaligned pair (PLAN.md §5.2). The design note
//! `docs/design/q1-delay-finder.md` is normative; section numbers below refer to it.
//!
//! - Estimator: regularised H1 in two passes. Pass 1 tiles the signed search range; pass 2
//!   refines around the strongest arrival (and around the first-arrival pick when it lies
//!   outside). An analytic IR comes out of each tile (§4).
//! - Candidates are envelope maxima above a statistical floor, not explained by a stronger
//!   arrival's pulse skirt, timed by a deblended lobe fit (§5).
//! - The answer is the **first significant arrival**: the earliest candidate within the
//!   threshold (−12 dB) of the strongest (§6). Every refusal reason is typed (§7); unsure
//!   picks come back `Ambiguous` with at most three ranked candidates (§8).
//! - [`Tracker`] moves the held delay only on two agreeing `Accepted` results from windows
//!   that share no samples (§10.2). [`DelayStream`] cuts back-to-back windows from
//!   sample-indexed blocks.
//!
//! Sign: positive delay = measurement late, `meas[i] ≈ (h ∗ ref)[i − D]`. Delays are
//! absolute (from the block sample indices); the held delay is never added.
//!
//! Runs on worker threads, never in the audio callback: a call allocates through a reusable
//! [`FinderScratch`], which keeps FFT plans and buffers between calls.
//!
//! Where the note leaves a detail open, this module decides:
//! - A lag is valid when its ref covers at least half of the meas segments of the grid
//!   (§4.1 counts against the grid, not against the best-covered lag).
//! - The full-range tolerance is 1 sample at any rate; mid and sub tolerances are times.
//! - Tracking agreement compares fractional first-arrival delays (±1 sample, sub ±0.1 ms).
//! - A bin with zero regularised denominator contributes 0 to H, never NaN.
//! - The 1/16-sample pulse model is evaluated as 16 phase-ramped inverse FFTs of the tile
//!   size, which equals one 16× zero-padded inverse FFT without forming it.

mod candidates;
mod estimator;
mod stream;
mod track;

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use arrayvec::ArrayVec;
use num_complex::Complex64;

use candidates::{Lobe, RAYLEIGH_MEDIAN, deblend, detection_floor, local_maxima, parabolic};
use estimator::{
    Ffts, GridShape, MeasSpectra, Model, Pair, Pulse, Reg, Shape, Tile, band_hi, band_snr,
    detect_period,
};

pub use stream::DelayStream;
pub use track::{Agreement, Tracker};

/// A run of samples at an absolute stream index (from the block headers).
#[derive(Debug, Clone, Copy)]
pub struct Block<'a> {
    /// Absolute sample index of `samples[0]`.
    pub start: u64,
    pub samples: &'a [f32],
}

/// Analysis band (§9). Edges are −6 dB points of a 1-octave raised-cosine taper in log f.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Band {
    /// 2 kHz – 16 kHz: full-range boxes.
    FullRange,
    /// 300 Hz – 3 kHz.
    Mid,
    /// 20 Hz – 120 Hz.
    Sub,
    /// Operator edges. Segment lengths, tolerance and merged-lobe limit come from the
    /// preset whose lower edge regime it falls in ([`Band::class`]).
    Custom { lo_hz: f64, hi_hz: f64 },
}

/// The preset a band takes its analysis parameters from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BandClass {
    FullRange,
    Mid,
    Sub,
}

impl Band {
    /// Parameter class. A custom band's resolution is set by its lower edge: ≥ 1 kHz is
    /// treated as full range, ≥ 150 Hz as mid, anything lower as sub.
    pub fn class(self) -> BandClass {
        match self {
            Band::FullRange => BandClass::FullRange,
            Band::Mid => BandClass::Mid,
            Band::Sub => BandClass::Sub,
            Band::Custom { lo_hz, .. } if lo_hz >= 1000.0 => BandClass::FullRange,
            Band::Custom { lo_hz, .. } if lo_hz >= 150.0 => BandClass::Mid,
            Band::Custom { .. } => BandClass::Sub,
        }
    }

    /// Nominal −6 dB edges, Hz.
    pub fn nominal_edges(self) -> (f64, f64) {
        match self {
            Band::FullRange => (2000.0, 16_000.0),
            Band::Mid => (300.0, 3000.0),
            Band::Sub => (20.0, 120.0),
            Band::Custom { lo_hz, hi_hz } => (lo_hz, hi_hz),
        }
    }

    /// Edges used at `fs`: the upper edge is clipped so its taper ends at or below Nyquist.
    pub fn edges(self, fs: f64) -> (f64, f64) {
        let (lo, hi) = self.nominal_edges();
        (lo, band_hi(hi, fs))
    }
}

impl BandClass {
    /// Acquisition segment N₁ at 48 kHz; scaled with fs to the nearest power of two.
    fn seg_48k(self) -> usize {
        match self {
            BandClass::FullRange => 4096,
            BandClass::Mid => 8192,
            BandClass::Sub => 32768,
        }
    }

    /// Acquisition segment N₁ at `fs` (refinement N₂ = 2 N₁).
    pub fn segment(self, fs: f64) -> usize {
        let n = self.seg_48k() as f64 / 48_000.0 * fs;
        1usize << n.log2().round().max(1.0) as u32
    }

    /// Default observation length, seconds.
    pub fn default_observation_s(self) -> f64 {
        match self {
            BandClass::FullRange => 0.25,
            BandClass::Mid => 0.5,
            BandClass::Sub => 4.0,
        }
    }

    /// Accuracy tolerance of an accepted delay, samples at `fs` (§9, D1).
    pub fn tolerance_samples(self, fs: f64) -> f64 {
        match self {
            BandClass::FullRange => 1.0,
            BandClass::Mid => 0.05e-3 * fs,
            BandClass::Sub => 0.1e-3 * fs,
        }
    }

    /// Tracking agreement, samples at `fs` (§9, D3).
    pub fn agreement_samples(self, fs: f64) -> f64 {
        match self {
            BandClass::FullRange | BandClass::Mid => 1.0,
            BandClass::Sub => 0.1e-3 * fs,
        }
    }

    /// Lobe misfit above which the pick is a merged lobe (μ_band).
    fn merged_misfit(self) -> f64 {
        match self {
            BandClass::FullRange => 0.10,
            BandClass::Mid => 0.05,
            BandClass::Sub => 0.03,
        }
    }
}

/// Transfer estimator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Estimator {
    /// H = Gxy / (Gxx + ε·mean_band Gxx). The default and the only automatic choice.
    RegularisedH1,
    /// GCC-PHAT, Gxy/|Gxy|. Diagnostic only: it discards the relative levels the −12 dB
    /// rule depends on (§11.3).
    Phat,
}

/// Signed lag range searched, samples. Positive = measurement late.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SearchRange {
    pub min: i64,
    pub max: i64,
}

impl SearchRange {
    /// ±`seconds` at `fs`.
    pub fn symmetric_s(fs: f64, seconds: f64) -> Self {
        let n = (seconds * fs).round() as i64;
        Self { min: -n, max: n }
    }

    /// Number of lags.
    pub fn len(&self) -> u64 {
        (self.max - self.min + 1).max(0) as u64
    }

    /// True for an empty range.
    pub fn is_empty(&self) -> bool {
        self.max < self.min
    }

    /// Lag span Dmax − Dmin.
    pub fn span(&self) -> u64 {
        (self.max - self.min).max(0) as u64
    }
}

/// Shared constants of §9 (Default = the note's values).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    /// Regularisation ε, fraction of the B-weighted in-band mean Gxx.
    pub eps: f64,
    /// Candidates are listed down to this level re the strongest, dB.
    pub list_depth_db: f64,
    /// False-peak probability per region for the detection floor.
    pub p_fa: f64,
    /// Refinement PSR gate, dB (|T| + 3 dB).
    pub psr_min_db: f64,
    /// Acquisition PSR gate, dB.
    pub psr_acq_min_db: f64,
    /// ± band around the threshold that makes an earlier candidate borderline, dB.
    pub borderline_db: f64,
    /// Merged-lobe misfit also must exceed this multiple of σ_n/|A|.
    pub merged_noise_k: f64,
    /// A candidate within this many pulse widths of the pick ...
    pub close_k: f64,
    /// ... and at least this strong makes the pick ambiguous, dB.
    pub close_depth_db: f64,
    /// A candidate must beat a stronger one's pulse skirt by this margin, dB.
    pub sidelobe_margin_db: f64,
    /// Neighbours within this many pulse widths are deblended.
    pub deblend_k: f64,
    /// Lags within this many pulse widths of a peak are excluded from the floor median.
    pub floor_excl_k: f64,
    /// σ_τ = coef · w_p · σ_n / |A|.
    pub uncertainty_coef: f64,
    /// Floor on σ_τ (fractional-interpolation bias), samples.
    pub uncertainty_floor: f64,
    /// Refuse when k·σ_τ exceeds the band tolerance.
    pub precision_k: f64,
    /// Minimum excited fraction of the band's octave span.
    pub min_excited_fraction: f64,
    /// Band SNR sanity guard, dB.
    pub band_snr_min_db: f64,
    /// Whitened ref autocorrelation peak that marks periodic excitation, dB.
    pub periodic_db: f64,
    /// A pick (strongest) this close to Dmin (Dmax) is refused, samples.
    pub edge_guard: i64,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            eps: 0.01,
            list_depth_db: -20.0,
            p_fa: 1e-3,
            psr_min_db: 15.0,
            psr_acq_min_db: 10.0,
            borderline_db: 2.0,
            merged_noise_k: 3.0,
            close_k: 2.0,
            close_depth_db: -20.0,
            sidelobe_margin_db: 6.0,
            deblend_k: 6.0,
            floor_excl_k: 3.0,
            uncertainty_coef: 0.35,
            uncertainty_floor: 0.1,
            precision_k: 2.5,
            min_excited_fraction: 0.5,
            band_snr_min_db: -10.0,
            periodic_db: -10.0,
            edge_guard: 2,
        }
    }
}

/// Finder configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FinderConfig {
    pub fs: f64,
    pub band: Band,
    /// Signed search range, samples (default ±1 s).
    pub search: SearchRange,
    /// First-arrival threshold re the strongest, dB (−12, operator-adjustable).
    pub threshold_db: f64,
    /// Stimulus period from the generator when known; otherwise detected from the ref.
    pub excitation_period: Option<u64>,
    /// Response tail allowance for periodic excitation, seconds.
    pub tail_s: f64,
    pub estimator: Estimator,
    /// Observation (meas window) length for [`DelayStream`], seconds; `None` = the band's
    /// default (§9; sub: operator choice 2/4/8 s, default 4 s). [`find`] uses whatever
    /// meas block it is given.
    pub observation_s: Option<f64>,
    pub tuning: Tuning,
}

impl FinderConfig {
    /// Defaults of §9 for `band` at `fs`.
    pub fn new(fs: f64, band: Band) -> Self {
        Self {
            fs,
            band,
            search: SearchRange::symmetric_s(fs, 1.0),
            threshold_db: -12.0,
            excitation_period: None,
            tail_s: 1.0,
            estimator: Estimator::RegularisedH1,
            observation_s: None,
            tuning: Tuning::default(),
        }
    }

    /// Meas window length used by [`DelayStream`], samples.
    pub fn observation_len(&self) -> usize {
        let s = self
            .observation_s
            .unwrap_or_else(|| self.band.class().default_observation_s());
        (s * self.fs).round() as usize
    }

    /// Minimum observation N₂, samples.
    pub fn min_observation_len(&self) -> usize {
        2 * self.band.class().segment(self.fs)
    }

    /// Check the parts that make the configuration meaningless.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(self.fs.is_finite() && self.fs > 0.0) {
            return Err(ConfigError::SampleRate(self.fs));
        }
        let (lo, hi) = self.band.edges(self.fs);
        if !(lo.is_finite() && hi.is_finite() && lo > 0.0 && hi > lo) {
            return Err(ConfigError::BandEdges {
                lo_hz: lo,
                hi_hz: hi,
            });
        }
        if self.search.is_empty() {
            return Err(ConfigError::SearchRange(self.search));
        }
        if !self.threshold_db.is_finite() || self.threshold_db > 0.0 {
            return Err(ConfigError::Threshold(self.threshold_db));
        }
        if let Some(s) = self.observation_s
            && !(s.is_finite() && s > 0.0)
        {
            return Err(ConfigError::Observation(s));
        }
        Ok(())
    }
}

/// Configuration the finder cannot run with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConfigError {
    SampleRate(f64),
    /// Band edges after clipping at Nyquist are not 0 < lo < hi.
    BandEdges {
        lo_hz: f64,
        hi_hz: f64,
    },
    /// min > max.
    SearchRange(SearchRange),
    /// Threshold must be finite and ≤ 0 dB.
    Threshold(f64),
    Observation(f64),
    /// A block start does not fit a signed 64-bit lag computation.
    SampleIndex(u64),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::SampleRate(fs) => write!(f, "invalid sample rate {fs}"),
            ConfigError::BandEdges { lo_hz, hi_hz } => {
                write!(f, "invalid band edges {lo_hz}–{hi_hz} Hz")
            }
            ConfigError::SearchRange(r) => write!(f, "empty search range {}..={}", r.min, r.max),
            ConfigError::Threshold(t) => write!(f, "invalid threshold {t} dB"),
            ConfigError::Observation(s) => write!(f, "invalid observation length {s} s"),
            ConfigError::SampleIndex(i) => write!(f, "sample index {i} out of range"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// One arrival.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrival {
    /// Integer delay (rounded), samples at fs.
    pub delay: i64,
    /// Fractional delay, samples at fs (the full value, e.g. 137.37).
    pub delay_frac: f64,
    /// Level re the strongest arrival, dB.
    pub level_db: f64,
    /// Analytic phase, degrees: ≈ 0 in phase, ≈ ±180 inverted, otherwise dispersive.
    pub phase_deg: f64,
    /// 1-σ timing uncertainty, samples.
    pub uncertainty: f64,
    /// Relative RMS lobe-shape misfit against the pulse model (0 outside refinement).
    pub misfit: f64,
    /// Measured inside a refinement window.
    pub refined: bool,
}

/// Confidence record; present with every outcome. Fields not reached before a refusal
/// are NaN / `None`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Confidence {
    /// Strongest envelope over its region's detection floor, dB.
    pub psr_db: f64,
    /// Same on the acquisition envelope, dB.
    pub psr_acq_db: f64,
    /// Coherent-to-incoherent band power at the strongest alignment, dB.
    pub band_snr_db: f64,
    /// Octave fraction of the band with W ≥ 0.5.
    pub excited_fraction: f64,
    /// −6 dB pulse width under this excitation, samples.
    pub pulse_width: f64,
    /// −6 dB pulse width with flat excitation, samples.
    pub nominal_width: f64,
    /// Excitation period (given or detected), samples.
    pub period: Option<u64>,
    /// First refinement window, lags (inclusive).
    pub refinement_window: Option<(i64, i64)>,
}

impl Confidence {
    fn unknown() -> Self {
        Self {
            psr_db: f64::NAN,
            psr_acq_db: f64::NAN,
            band_snr_db: f64::NAN,
            excited_fraction: f64::NAN,
            pulse_width: f64::NAN,
            nominal_width: f64::NAN,
            period: None,
            refinement_window: None,
        }
    }
}

/// Why there is no estimate (§7). All that apply are reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoEstimateReason {
    NoReference,
    NoSignal,
    /// Meas block shorter than N₂.
    ObservationTooShort,
    /// No lag has ref coverage for at least half the meas segments.
    InsufficientOverlap,
    /// Excited fraction below the minimum.
    InsufficientExcitation,
    /// Excitation repeats within the search span plus the tail allowance.
    PeriodicExcitation {
        period: u64,
    },
    LowPsr,
    /// k·σ_τ of the pick exceeds the band tolerance.
    LowPrecision,
    PeakAtSearchEdge,
    LowBandSnr,
}

/// Why the pick is not certain (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AmbiguityReason {
    /// A candidate before the clear pick is within ±m_b of the threshold.
    BorderlineLevel,
    /// Another candidate ≥ −20 dB lies within 2 pulse widths of the pick.
    CloseArrivals,
    /// The pick's lobe does not fit one arrival.
    MergedLobe,
    /// The pick has only acquisition evidence.
    OutsideRefinement,
}

/// Finder outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Accepted {
        first: Arrival,
        strongest: Arrival,
    },
    /// The rule pick is `ranked[0]`.
    Ambiguous {
        reasons: Vec<AmbiguityReason>,
        ranked: ArrayVec<Arrival, 3>,
        strongest: Arrival,
    },
    NoEstimate {
        reasons: Vec<NoEstimateReason>,
    },
}

/// Everything one finder run reports.
#[derive(Debug, Clone, PartialEq)]
pub struct FinderResult {
    pub outcome: Outcome,
    /// All candidates, by delay (IR panel), also with `NoEstimate`.
    pub candidates: Vec<Arrival>,
    pub confidence: Confidence,
    /// Band analysed (the resolved one in auto mode).
    pub band: Band,
    /// Absolute sample range of the meas block.
    pub meas_window: Range<u64>,
}

impl FinderResult {
    /// The accepted first arrival, if any.
    pub fn accepted(&self) -> Option<&Arrival> {
        match &self.outcome {
            Outcome::Accepted { first, .. } => Some(first),
            _ => None,
        }
    }

    /// The first-arrival rule pick (accepted or ambiguous).
    pub fn pick(&self) -> Option<&Arrival> {
        match &self.outcome {
            Outcome::Accepted { first, .. } => Some(first),
            Outcome::Ambiguous { ranked, .. } => ranked.first(),
            Outcome::NoEstimate { .. } => None,
        }
    }
}

/// Reusable FFT plans and buffers. One per worker; results never depend on its history.
pub struct FinderScratch {
    ffts: Ffts,
    shapes: Vec<Shape>,
    w: Work,
}

impl fmt::Debug for FinderScratch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinderScratch")
            .field("grids", &self.shapes.len())
            .finish_non_exhaustive()
    }
}

impl Default for FinderScratch {
    fn default() -> Self {
        Self::new()
    }
}

impl FinderScratch {
    /// Empty scratch; plans are built on first use.
    pub fn new() -> Self {
        Self {
            ffts: Ffts::new(),
            shapes: Vec::new(),
            w: Work::default(),
        }
    }

    /// Scratch with the FFT plans and grids of `cfg` built up front.
    pub fn for_config(cfg: &FinderConfig) -> Result<Self, ConfigError> {
        cfg.validate()?;
        let mut s = Self::new();
        let n1 = cfg.band.class().segment(cfg.fs);
        let (lo, hi) = cfg.band.edges(cfg.fs);
        s.shape(n1, n1 / 2, cfg.fs, lo, hi);
        s.shape(2 * n1, n1 / 2, cfg.fs, lo, hi);
        s.ffts.plan(n1);
        Ok(s)
    }

    fn shape(&mut self, nseg: usize, hop: usize, fs: f64, lo: f64, hi: f64) -> Shape {
        if let Some(s) = self
            .shapes
            .iter()
            .find(|s| s.matches(nseg, hop, fs, lo, hi))
        {
            return Arc::clone(s);
        }
        let s = Arc::new(GridShape::new(&mut self.ffts, nseg, hop, fs, lo, hi));
        self.shapes.push(Arc::clone(&s));
        s
    }
}

/// Per-call working arrays, kept between calls to avoid reallocating.
#[derive(Debug, Default)]
struct Work {
    r: Vec<f64>,
    m: Vec<f64>,
    y1: MeasSpectra,
    y2: MeasSpectra,
    tile: Tile,
    hcx: Vec<Complex64>,
    env: Vec<f64>,
    det: Vec<f64>,
    med: Vec<f64>,
    kseg: Vec<usize>,
    valid: Vec<bool>,
    in_win: Vec<bool>,
    tmp: Vec<f64>,
    maxima: Vec<usize>,
}

/// Find the delay of `meas` re `ref_` in `cfg.band` (§4–§8).
pub fn find(
    ref_: Block<'_>,
    meas: Block<'_>,
    cfg: &FinderConfig,
) -> Result<FinderResult, ConfigError> {
    find_with(&mut FinderScratch::new(), ref_, meas, cfg)
}

/// [`find`] reusing `scratch`.
pub fn find_with(
    scratch: &mut FinderScratch,
    ref_: Block<'_>,
    meas: Block<'_>,
    cfg: &FinderConfig,
) -> Result<FinderResult, ConfigError> {
    cfg.validate()?;
    let a = i64::try_from(ref_.start).map_err(|_| ConfigError::SampleIndex(ref_.start))?;
    let b = i64::try_from(meas.start).map_err(|_| ConfigError::SampleIndex(meas.start))?;
    let mut w = std::mem::take(&mut scratch.w);
    w.r.clear();
    w.r.extend(ref_.samples.iter().map(|&v| f64::from(v)));
    w.m.clear();
    w.m.extend(meas.samples.iter().map(|&v| f64::from(v)));
    let r = std::mem::take(&mut w.r);
    let m = std::mem::take(&mut w.m);
    let pair = Pair { r: &r, a, m: &m, b };
    let mut run = Run {
        s: scratch,
        w: &mut w,
        cfg,
        pair,
    };
    let (outcome, candidates, confidence) = run.find();
    w.r = r;
    w.m = m;
    scratch.w = w;
    Ok(FinderResult {
        outcome,
        candidates,
        confidence,
        band: cfg.band,
        meas_window: meas.start..meas.start + meas.samples.len() as u64,
    })
}

/// Auto band (§9.1): full → mid → sub, the first result that is not `NoEstimate`. When
/// every band refuses, the full-range result is returned. `cfg.band` is ignored.
pub fn find_auto(
    ref_: Block<'_>,
    meas: Block<'_>,
    cfg: &FinderConfig,
) -> Result<FinderResult, ConfigError> {
    find_auto_with(&mut FinderScratch::new(), ref_, meas, cfg)
}

/// [`find_auto`] reusing `scratch`.
pub fn find_auto_with(
    scratch: &mut FinderScratch,
    ref_: Block<'_>,
    meas: Block<'_>,
    cfg: &FinderConfig,
) -> Result<FinderResult, ConfigError> {
    let mut first = None;
    for band in [Band::FullRange, Band::Mid, Band::Sub] {
        let c = FinderConfig { band, ..*cfg };
        let r = find_with(scratch, ref_, meas, &c)?;
        if !matches!(r.outcome, Outcome::NoEstimate { .. }) {
            return Ok(r);
        }
        first.get_or_insert(r);
    }
    Ok(first.unwrap_or_else(|| unreachable!("three bands were tried")))
}

/// A candidate while the decision is made.
#[derive(Debug, Clone, Copy)]
struct Cand {
    delay: f64,
    delay_int: i64,
    level_db: f64,
    phase_deg: f64,
    misfit: f64,
    refined: bool,
    uncertainty: f64,
    /// σ_n / |A|.
    noise_ratio: f64,
}

impl Cand {
    fn arrival(&self) -> Arrival {
        Arrival {
            delay: self.delay_int,
            delay_frac: self.delay,
            level_db: self.level_db,
            phase_deg: self.phase_deg,
            uncertainty: self.uncertainty,
            misfit: self.misfit,
            refined: self.refined,
        }
    }
}

/// Pulse model in use (from the first refinement tile, else from acquisition).
struct ModelSrc {
    pulse: Pulse,
    grid: Shape,
}

struct Run<'s, 'd> {
    s: &'s mut FinderScratch,
    w: &'s mut Work,
    cfg: &'s FinderConfig,
    pair: Pair<'d>,
}

/// Python-style round half to even, so integer delays match the reference prototype.
fn round_lag(x: f64) -> i64 {
    x.round_ties_even() as i64
}

impl Run<'_, '_> {
    fn find(&mut self) -> (Outcome, Vec<Arrival>, Confidence) {
        let cfg = self.cfg;
        let t = cfg.tuning;
        let reg = Reg {
            estimator: cfg.estimator,
            eps: t.eps,
        };
        let fs = cfg.fs;
        let class = cfg.band.class();
        let (lo, hi) = cfg.band.edges(fs);
        let (d_min, d_max) = (cfg.search.min, cfg.search.max);
        let mut conf = Confidence::unknown();
        let mut reasons = Vec::new();
        let refuse = |reasons, conf| (Outcome::NoEstimate { reasons }, Vec::new(), conf);

        if !self.pair.r.iter().any(|v| *v != 0.0) {
            reasons.push(NoEstimateReason::NoReference);
        }
        if !self.pair.m.iter().any(|v| *v != 0.0) {
            reasons.push(NoEstimateReason::NoSignal);
        }
        if !reasons.is_empty() {
            return refuse(reasons, conf);
        }
        let n1 = class.segment(fs);
        let n2 = 2 * n1;
        if self.pair.m.len() < n2 {
            return refuse(vec![NoEstimateReason::ObservationTooShort], conf);
        }

        // ---- pass 1: acquisition over the whole search range (§4.3) ----------------------
        let g1 = self.s.shape(n1, n1 / 2, fs, lo, hi);
        let w = &mut *self.w;
        let ffts = &mut self.s.ffts;
        w.y1.compute(ffts, &g1, self.pair.m);
        let n_lags = (d_max - d_min + 1) as usize;
        w.hcx.clear();
        w.hcx.resize(n_lags, Complex64::new(f64::NAN, f64::NAN));
        w.kseg.clear();
        w.kseg.resize(n_lags, 0);
        let mut gxx1 = vec![0.0; g1.bins()];
        let step = (n1 / 4) as i64;
        let half = step / 2;
        let mut d0 = d_min + half;
        while d0 < d_max + step {
            w.tile.compute(ffts, &g1, &w.y1, self.pair, d0, reg);
            if w.tile.k > 0 {
                for d in (d0 - half).max(d_min)..=(d0 - half + step - 1).min(d_max) {
                    let i = (d - d_min) as usize;
                    w.hcx[i] = w.tile.at(&g1, d - d0);
                    w.kseg[i] = w.tile.k;
                }
                for (acc, v) in gxx1.iter_mut().zip(&w.tile.gxx) {
                    *acc += v;
                }
            }
            d0 += step;
        }
        // a lag whose ref is missing for more than half the meas segments is invalid (§4.1)
        let k_need = w.y1.segments().div_ceil(2).max(1);
        w.valid.clear();
        w.valid.extend(w.kseg.iter().map(|&k| k >= k_need));
        let n_valid = w.valid.iter().filter(|v| **v).count();
        if n_valid == 0 {
            return refuse(vec![NoEstimateReason::InsufficientOverlap], conf);
        }
        w.env.clear();
        w.env.extend(
            w.hcx
                .iter()
                .zip(&w.valid)
                .map(|(h, &v)| if v { h.norm() } else { 0.0 }),
        );

        let pulse1 = Pulse::new(ffts, &g1, Some(&gxx1), reg);
        let env_valid: Vec<f64> = w
            .env
            .iter()
            .zip(&w.valid)
            .filter_map(|(e, &v)| v.then_some(*e))
            .collect();
        let (med1, det1) = detection_floor(
            &env_valid,
            pulse1.b_eff(g1.df()),
            fs,
            t.p_fa,
            t.floor_excl_k * pulse1.width,
            &mut w.tmp,
        );
        let i_s1 = argmax(&w.env);
        let e_s1 = w.env[i_s1];
        conf.psr_acq_db = if det1 > 0.0 {
            20.0 * (e_s1 / det1).log10()
        } else {
            f64::INFINITY
        };
        if e_s1 <= 0.0 || e_s1.is_nan() {
            return refuse(vec![NoEstimateReason::NoSignal], conf);
        }

        // ---- pass 2: refinement tiles (§4.4) ------------------------------------------------
        w.det.clear();
        w.det.resize(n_lags, det1);
        w.med.clear();
        w.med.resize(n_lags, med1);
        w.in_win.clear();
        w.in_win.resize(n_lags, false);
        let g2 = self.s.shape(n2, n2 / 4, fs, lo, hi);
        self.w.y2.compute(&mut self.s.ffts, &g2, self.pair.m);
        let mut model = ModelSrc {
            pulse: pulse1,
            grid: Arc::clone(&g1),
        };
        let mut windows: Vec<(i64, i64)> = Vec::new();
        self.refine_at(d_min + i_s1 as i64, &g2, &mut model, &mut windows);

        let width = model.pulse.width;
        conf.refinement_window = windows.first().copied();
        conf.pulse_width = width;
        let flat = Pulse::new(&mut self.s.ffts, &model.grid, None, reg);
        conf.nominal_width = flat.width;
        conf.excited_fraction = model.pulse.excited_fraction(&model.grid);

        // ---- refusals that do not depend on the IR (§7) -------------------------------------
        if conf.excited_fraction < t.min_excited_fraction {
            reasons.push(NoEstimateReason::InsufficientExcitation);
        }
        let period = cfg
            .excitation_period
            .or_else(|| detect_period(&mut self.s.ffts, self.pair.r, fs, t.eps, t.periodic_db));
        conf.period = period;
        if let Some(p) = period
            && (p as f64) <= cfg.search.span() as f64 + cfg.tail_s * fs
        {
            reasons.push(NoEstimateReason::PeriodicExcitation { period: p });
        }

        // ---- candidates (§5, §6) -------------------------------------------------------------
        let reach = t.deblend_k * width;
        let om = Model::new(&mut self.s.ffts, &model.pulse.pw, reach + 1.5 * width + 3.0);
        let mut cands = self.candidates(&model.pulse, &om);
        if let Some(first) = cands.iter().find(|c| c.level_db >= cfg.threshold_db)
            && !first.refined
        {
            let d = first.delay_int;
            self.refine_at(d, &g2, &mut model, &mut windows);
            cands = self.candidates(&model.pulse, &om);
        }

        let w = &*self.w;
        let i_max = argmax(&w.env);
        let e_max = w.env[i_max];
        let det_s = w.det[i_max];
        conf.psr_db = if det_s > 0.0 {
            20.0 * (e_max / det_s).log10()
        } else {
            f64::INFINITY
        };
        if conf.psr_acq_db < t.psr_acq_min_db || conf.psr_db < t.psr_min_db {
            reasons.push(NoEstimateReason::LowPsr);
        }
        let arrivals: Vec<Arrival> = cands.iter().map(Cand::arrival).collect();
        let Some(si) = argmax_by(&cands, |c| c.level_db) else {
            if !reasons.contains(&NoEstimateReason::LowPsr) {
                reasons.push(NoEstimateReason::LowPsr);
            }
            return (Outcome::NoEstimate { reasons }, arrivals, conf);
        };
        let strongest = cands[si];
        let pi = cands
            .iter()
            .position(|c| c.level_db >= cfg.threshold_db)
            .unwrap_or(si);
        let pick = cands[pi];

        if t.precision_k * pick.uncertainty > class.tolerance_samples(fs) {
            reasons.push(NoEstimateReason::LowPrecision);
        }
        if pick.delay_int - d_min < t.edge_guard || d_max - strongest.delay_int < t.edge_guard {
            reasons.push(NoEstimateReason::PeakAtSearchEdge);
        }
        conf.band_snr_db = band_snr(
            &mut self.s.ffts,
            self.pair,
            strongest.delay_int,
            n1,
            lo,
            hi,
            fs,
        );
        if conf.band_snr_db.is_nan() || conf.band_snr_db < t.band_snr_min_db {
            reasons.push(NoEstimateReason::LowBandSnr);
        }
        if !reasons.is_empty() {
            return (Outcome::NoEstimate { reasons }, arrivals, conf);
        }

        // ---- ambiguity (§8) ------------------------------------------------------------------
        let mut amb = Vec::new();
        let clear_lvl = cfg.threshold_db + t.borderline_db;
        let ci = cands
            .iter()
            .position(|c| c.level_db >= clear_lvl)
            .unwrap_or(si);
        let pick_clear = cands[ci];
        let unsure: Vec<usize> = (0..cands.len())
            .filter(|&i| {
                let c = &cands[i];
                c.level_db >= cfg.threshold_db - t.borderline_db
                    && c.level_db < clear_lvl
                    && c.delay < pick_clear.delay
            })
            .collect();
        if !unsure.is_empty() {
            amb.push(AmbiguityReason::BorderlineLevel);
        }
        let close: Vec<usize> = (0..cands.len())
            .filter(|&i| {
                let c = &cands[i];
                i != pi
                    && (c.delay - pick.delay).abs() < t.close_k * width
                    && c.level_db >= t.close_depth_db
            })
            .collect();
        if !close.is_empty() {
            amb.push(AmbiguityReason::CloseArrivals);
        }
        if pick.misfit
            > class
                .merged_misfit()
                .max(t.merged_noise_k * pick.noise_ratio)
        {
            amb.push(AmbiguityReason::MergedLobe);
        }
        if !pick.refined {
            amb.push(AmbiguityReason::OutsideRefinement);
        }
        if amb.is_empty() {
            return (
                Outcome::Accepted {
                    first: pick.arrival(),
                    strongest: strongest.arrival(),
                },
                arrivals,
                conf,
            );
        }
        // ranked: the rule pick, pick_clear, the strongest, then unsure and close by level
        let mut rest: Vec<usize> = unsure.into_iter().chain(close).collect();
        rest.sort_by(|&x, &y| cands[y].level_db.total_cmp(&cands[x].level_db));
        let mut order = vec![pi];
        for i in [ci, si].into_iter().chain(rest) {
            if !order.contains(&i) {
                order.push(i);
            }
        }
        let ranked: ArrayVec<Arrival, 3> =
            order.iter().take(3).map(|&i| cands[i].arrival()).collect();
        (
            Outcome::Ambiguous {
                reasons: amb,
                ranked,
                strongest: strongest.arrival(),
            },
            arrivals,
            conf,
        )
    }

    /// Refinement tile at lag `d0` (§4.4): replaces the acquisition values in its window,
    /// sets its floor and, the first time, the pulse model.
    fn refine_at(
        &mut self,
        d0: i64,
        g2: &Shape,
        model: &mut ModelSrc,
        windows: &mut Vec<(i64, i64)>,
    ) {
        let cfg = self.cfg;
        let t = cfg.tuning;
        let reg = Reg {
            estimator: cfg.estimator,
            eps: t.eps,
        };
        let (d_min, d_max) = (cfg.search.min, cfg.search.max);
        let w = &mut *self.w;
        let ffts = &mut self.s.ffts;
        w.tile.compute(ffts, g2, &w.y2, self.pair, d0, reg);
        if w.tile.k == 0 {
            return;
        }
        let r2 = (g2.nseg / 8) as i64;
        let lo = (d0 - r2).max(d_min);
        let hi = (d0 + r2).min(d_max);
        if lo > hi {
            return;
        }
        for d in lo..=hi {
            let i = (d - d_min) as usize;
            let h = w.tile.at(g2, d - d0);
            w.hcx[i] = h;
            w.env[i] = h.norm();
        }
        if windows.is_empty() {
            model.pulse = Pulse::new(ffts, g2, Some(&w.tile.gxx), reg);
            model.grid = Arc::clone(g2);
        }
        let (i0, i1) = ((lo - d_min) as usize, (hi - d_min) as usize);
        let (med, det) = detection_floor(
            &w.env[i0..=i1],
            model.pulse.b_eff(g2.df()),
            cfg.fs,
            t.p_fa,
            t.floor_excl_k * model.pulse.width,
            &mut w.tmp,
        );
        w.det[i0..=i1].fill(det);
        w.med[i0..=i1].fill(med);
        w.in_win[i0..=i1].fill(true);
        windows.push((lo, hi));
    }

    /// Candidate list, by delay (§5.2, §5.3).
    fn candidates(&mut self, pulse: &Pulse, om: &Model) -> Vec<Cand> {
        let cfg = self.cfg;
        let t = cfg.tuning;
        let d_min = cfg.search.min;
        let w = &mut *self.w;
        let width = pulse.width;
        let e_max = w.env.iter().copied().fold(0.0, f64::max);
        let depth = e_max * 10f64.powf(t.list_depth_db / 20.0);
        let margin = 10f64.powf(t.sidelobe_margin_db / 20.0);
        local_maxima(&w.env, &mut w.maxima);
        let mut idx: Vec<usize> = w
            .maxima
            .iter()
            .copied()
            .filter(|&j| w.valid[j] && w.env[j] >= w.det[j].max(depth))
            .collect();
        idx.sort_by(|&x, &y| w.env[y].total_cmp(&w.env[x]));
        let mut kept: Vec<usize> = Vec::new();
        for &j in &idx {
            let explained = kept.iter().any(|&s| {
                let dl = j as i64 - s as i64;
                (dl.abs() as f64) < width || w.env[j] <= w.env[s] * pulse.env_at(dl) * margin
            });
            if !explained {
                kept.push(j);
            }
        }
        let mut taus: Vec<f64> = kept
            .iter()
            .map(|&j| {
                let e = &w.env;
                let frac = if j > 0 && j + 1 < e.len() && e[j - 1] > 0.0 && e[j + 1] > 0.0 {
                    parabolic(e[j - 1].ln(), e[j].ln(), e[j + 1].ln())
                } else {
                    0.0
                };
                j as f64 + frac
            })
            .collect();
        let mut amps: Vec<Complex64> = kept.iter().map(|&j| w.hcx[j]).collect();
        let mut misfit = vec![0.0; kept.len()];
        let win: Vec<usize> = (0..kept.len()).filter(|&a| w.in_win[kept[a]]).collect();
        if !win.is_empty() {
            let mut lobes: Vec<Lobe> = win
                .iter()
                .map(|&a| Lobe {
                    j: kept[a],
                    tau: taus[a],
                    amp: amps[a],
                    misfit: 0.0,
                })
                .collect();
            deblend(&w.hcx, &mut lobes, om, width, t.deblend_k * width);
            for (&a, l) in win.iter().zip(&lobes) {
                taus[a] = l.tau;
                amps[a] = l.amp;
                misfit[a] = l.misfit;
            }
        }
        let lv: Vec<f64> = kept
            .iter()
            .enumerate()
            .map(|(a, &j)| {
                if w.in_win[j] {
                    amps[a].norm()
                } else {
                    w.env[j]
                }
            })
            .collect();
        let l_max = if lv.is_empty() {
            e_max
        } else {
            lv.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        };
        let mut out: Vec<Cand> = kept
            .iter()
            .enumerate()
            .map(|(a, &j)| {
                let delay = d_min as f64 + taus[a];
                let l = lv[a].max(1e-300);
                let noise_ratio = (w.med[j] / RAYLEIGH_MEDIAN) / l;
                Cand {
                    delay,
                    delay_int: round_lag(delay),
                    level_db: 20.0 * (l / l_max).log10(),
                    phase_deg: amps[a].arg().to_degrees(),
                    misfit: misfit[a],
                    refined: w.in_win[j],
                    uncertainty: (t.uncertainty_coef * width * noise_ratio)
                        .max(t.uncertainty_floor),
                    noise_ratio,
                }
            })
            .collect();
        out.sort_by(|x, y| x.delay.total_cmp(&y.delay));
        out
    }
}

/// Index of the first maximum.
fn argmax(v: &[f64]) -> usize {
    let mut best = 0;
    for (i, x) in v.iter().enumerate() {
        if *x > v[best] {
            best = i;
        }
    }
    best
}

fn argmax_by<T>(v: &[T], key: impl Fn(&T) -> f64) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, x) in v.iter().enumerate() {
        if best.is_none_or(|b| key(x) > key(&v[b])) {
            best = Some(i);
        }
    }
    best
}

#[cfg(test)]
mod tests;
