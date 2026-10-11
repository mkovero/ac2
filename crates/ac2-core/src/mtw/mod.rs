//! Multi-time-window (MTW) dual-channel H1 / coherence engine (PLAN.md §5.1).
//!
//! # Pipeline
//! 1. **Alignment** at full rate by a signed delay in samples ([`Mtw::set_delay`]): the
//!    whole-sample part shifts the reference in the time domain before decimation, so the
//!    windows of both channels cover the same sound however large the delay; the fraction
//!    (|φ| ≤ ½ sample) rotates each block's cross-spectrum by `e^{j2πfφ/fs}`, which is
//!    exact in phase and leaves a window misalignment far too small to bias anything.
//!    A delay change keeps the stream running: the reference is spliced to the new
//!    alignment, blocks straddling the splice are dropped, and each stage keeps its averages
//!    (rotated to the new delay) while its held blocks' window misalignment against the new
//!    delay stays within [`KEEP_MIN_WINDOW_CORRELATION`]; stages beyond that start over, and
//!    when no stage can keep held blocks the ladder restarts
//!    (`docs/design/delay-no-resettle.md`).
//! 2. **Stages** ([`Layout`]): full rate plus ~12 kHz and ~4 kHz stages, factor
//!    `round(sr / target)`, all at NFFT 4096 with 50 / 75 / 87.5 % overlap. Each decimated
//!    stage is fed independently from the aligned full-rate pair by a Kaiser FIR (90 dB)
//!    whose two legs share one phase counter; the filter's warm-up outputs are never emitted.
//! 3. **Framing** is fixed to the stream: block `k` of a stage always covers stage samples
//!    `[k·hop, k·hop + nfft)` counted from the start of the aligned stream, however the input
//!    was chunked.
//! 4. **Averaging** of Gxx, Gyy, Gxy only ([`Averaging`]). Depth is matched across stages in
//!    model effective averages ([`estimator::OverlapModel`]) by default; [`DepthPolicy::FastLf`]
//!    instead caps how long the decimated stages average, trading a higher (reported)
//!    low-frequency coherence floor for faster settling.
//! 5. **Columns** on the shared [`LogGrid`]: H1 sums the cross- and reference auto-spectra
//!    of the bins whose centres fall inside the column and divides once. γ² is formed per
//!    bin, γ²ₖ = |Gxyₖ|² / (Gxxₖ·Gyyₖ), and averaged over the column's bins weighted by
//!    Gxxₖ. A complex sum of Gxy over a column cancels wherever the transfer phase turns
//!    across it (a delay left in the path, reflections), so |ΣGxy|²/(ΣGxx·ΣGyy) would fall
//!    with column width even when every bin is fully coherent. Per bin, γ² measures only
//!    noise and nonlinearity, so the column value does not depend on resolution; the Gxx
//!    weight keeps bins the stimulus does not excite from diluting it. Columns that contain
//!    no bin are thinned (NaN with a reason), never interpolated. Across each crossover H1
//!    is blended complex over 1/3 octave; γ² there is a labelled display blend of the two
//!    stages' estimates.
//!
//! Absolute levels never come from this engine: decimated stages carry the decimator's
//! response, which cancels in H1 and γ² but not in Gxx or Gyy.

mod align;
mod engine;
pub mod estimator;
pub mod fir;
pub mod layout;

use num_complex::Complex64;

use crate::grid::LogGrid;
pub use estimator::{Averaging, DepthPolicy, MIN_FAST_LF_BLOCKS, OverlapModel, StageAveraging};
use estimator::{StageEstimator, matched_alpha, matched_fifo_blocks};
pub use fir::{FirDesign, PairDecimator};
pub use layout::{Crossover, Ladder, Layout, LayoutError, StageSpec};

/// Engine configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct MtwConfig {
    /// Full sample rate in Hz.
    pub sample_rate_hz: f64,
    /// Stage ladder.
    pub ladder: Ladder,
    /// Spectral averaging.
    pub averaging: Averaging,
    /// Averaging depth of the decimated stages relative to the full-rate stage.
    pub depth: DepthPolicy,
    /// Output grid (48 ppo base-2 for the display).
    pub grid: LogGrid,
    /// Alignment delay in full-rate samples, fractions allowed; positive = measurement late.
    pub delay_samples: f64,
}

/// Normalised correlation `Σ w[n]·w[n+r] / Σ w²` of a stage's Hann window with itself
/// shifted by `r` that every held block must keep for a delay change to rotate the stage's
/// averages instead of starting the stage over. A block whose windows were cut `r` samples
/// off the new alignment carries its cross-spectrum scaled by this correlation (white-noise
/// model), so 0.995 bounds the bias at −0.044 dB in |H1| and ×0.990 in γ². For the NFFT 4096
/// Hann window it allows 112 stage samples (2.7 % of the window): 112 samples at full rate
/// and the decimation factor times that on the deeper stages.
pub const KEEP_MIN_WINDOW_CORRELATION: f64 = 0.995;

/// What a delay change did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelayChange {
    /// The ladder restarted: every stage settles from scratch.
    pub restarted: bool,
    /// Bit `s` set: stage `s` kept its averages, rotated to the new delay. A stage that
    /// held no blocks counts as kept.
    pub kept: u32,
}

impl DelayChange {
    /// Whether stage `stage` kept its averages.
    pub fn kept(&self, stage: usize) -> bool {
        !self.restarted && self.kept & (1 << stage) != 0
    }
}

/// Whole-sample part (nearest, so the fraction is within ±½) and fraction of a delay.
fn split_delay(d: f64) -> (i64, f64) {
    let whole = d.round();
    (whole as i64, d - whole)
}

/// Largest shift, in samples, at which `window` keeps a normalised self-correlation of at
/// least `min` (for Hann the correlation falls monotonically with the shift).
fn keep_lag(window: &[f64], min: f64) -> usize {
    let e2: f64 = window.iter().map(|v| v * v).sum();
    let rho = |r: usize| {
        window
            .iter()
            .zip(&window[r..])
            .map(|(a, b)| a * b)
            .sum::<f64>()
            / e2
    };
    let (mut lo, mut hi) = (0, window.len() - 1);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if rho(mid) >= min {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// Configuration errors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConfigError {
    /// The stage layout cannot be built.
    Layout(LayoutError),
    /// FIFO depth of zero, or a time constant that is not positive and finite.
    InvalidAveraging(Averaging),
    /// A `FastLf` span cap that is not positive and finite.
    InvalidDepthPolicy(DepthPolicy),
    /// A delay that is not finite.
    InvalidDelay(f64),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Layout(e) => write!(f, "{e}"),
            ConfigError::InvalidAveraging(a) => write!(f, "invalid averaging {a:?}"),
            ConfigError::InvalidDepthPolicy(p) => write!(f, "invalid depth policy {p:?}"),
            ConfigError::InvalidDelay(d) => write!(f, "invalid delay {d} samples"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<LayoutError> for ConfigError {
    fn from(e: LayoutError) -> Self {
        ConfigError::Layout(e)
    }
}

/// Whether a pushed block may enter the analysis. Protection logic (clip, missing
/// reference) decides; the engine only honours the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleGate {
    /// Analyse normally.
    Accept,
    /// Advance the stream but keep every analysis block that touches these samples (after
    /// alignment and decimator spread) out of the averages.
    Reject,
}

/// Errors from pushing samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    /// Reference and measurement differ in length.
    LengthMismatch {
        /// Reference length.
        reference: usize,
        /// Measurement length.
        measurement: usize,
    },
    /// Interleaved buffer layout is inconsistent.
    BadInterleave {
        /// Channels per frame.
        channels: usize,
        /// Reference channel.
        reference: usize,
        /// Measurement channel.
        measurement: usize,
    },
}

impl std::fmt::Display for PushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PushError {}

/// What a push did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PushOutcome {
    /// Blocks added to the averages (all stages).
    pub blocks_accumulated: usize,
    /// Blocks dropped because they touched rejected samples.
    pub blocks_rejected: usize,
    /// The input did not continue the previous push's sample index, so the ladder restarted.
    pub restarted: bool,
}

/// Why a column has no value, or that it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validity {
    /// H1 and γ² are estimates.
    Valid,
    /// No FFT bin of the serving stage falls inside the column: resolution has run out.
    Thinned,
    /// Above the highest frequency the ladder serves (0.45 × full rate).
    OutOfBand,
    /// The serving stage(s) hold no averaged block yet.
    Settling,
    /// The reference has no energy in the column; H1 is undefined.
    NoReference,
    /// The measurement has no energy in the column; γ² is undefined.
    NoMeasurement,
}

/// Bins `[lo, hi)` of one stage summed into a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageBins {
    /// Stage index.
    pub stage: usize,
    /// First bin.
    pub lo: usize,
    /// One past the last bin.
    pub hi: usize,
}

impl StageBins {
    /// Number of bins.
    pub fn count(&self) -> usize {
        self.hi - self.lo
    }
}

/// Where a column's value comes from. Fixed by the layout and grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnSource {
    /// No stage can serve the column (thinned or out of band).
    None,
    /// One stage.
    Stage(StageBins),
    /// Crossover blend: `value = (1 − w)·deep + w·shallow`. For H1 this is a complex blend
    /// of two estimates of the same quantity; for γ² it is a display blend, not an estimator.
    Blend {
        /// Lower-rate stage.
        deep: StageBins,
        /// Higher-rate stage.
        shallow: StageBins,
        /// Weight `w` of the shallow stage.
        shallow_weight: f64,
    },
}

/// Per-column metadata.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnInfo {
    /// Whether the column holds a value.
    pub validity: Validity,
    /// Source stage(s) and bins.
    pub source: ColumnSource,
}

/// One output frame.
#[derive(Debug, Clone, PartialEq)]
pub struct MtwFrame {
    /// Grid the columns are on.
    pub grid: LogGrid,
    /// Column centre frequencies.
    pub freq_hz: Vec<f64>,
    /// H1 = ΣGxy / ΣGxx per column (blended complex at crossovers). NaN when not valid.
    pub h1: Vec<Complex64>,
    /// 20·log10|H1|.
    pub magnitude_db: Vec<f64>,
    /// arg H1 in degrees, wrapped to (−180, 180].
    pub phase_deg: Vec<f64>,
    /// γ² = Σₖ Gxxₖ·γ²ₖ / Σₖ Gxxₖ over the column's bins, with per-bin
    /// γ²ₖ = |Gxyₖ|² / (Gxxₖ·Gyyₖ) (module doc, item 5); a display blend inside crossovers.
    pub coherence: Vec<f64>,
    /// Model effective number of averages per bin of the serving stage (stationary
    /// white-noise model of block overlap and FIFO/exponential weights). It sets γ²'s
    /// estimator bias: each γ²ₖ reads ≈ (1 − γ²)²/N_eff high, and averaging over the column's
    /// bins lowers the variance of γ² but not that bias. The Gxx weight adds some: a bin
    /// whose reference happened to read strong also reads more coherent, so a column of many
    /// independent bins tends to γ² + (1 − γ²)/N_eff. On uncorrelated input both read
    /// 1/N_eff. For blends, the smaller of the two stages' values. A model value, not a
    /// measurement.
    pub eff_avg: Vec<f64>,
    /// Validity and source per column.
    pub columns: Vec<ColumnInfo>,
    /// One past the newest measurement-channel input index that any averaged block depends
    /// on (`None` before the first block).
    pub last_block_end: Option<u64>,
}

/// Averaged spectra of one stage, scaled as one-sided densities (FS²/Hz at the stage
/// rate). For decimated stages these include the decimator's response; use them for
/// diagnostics and ratios only.
#[derive(Debug, Clone, PartialEq)]
pub struct StageSpectra {
    /// Bin frequencies.
    pub freq_hz: Vec<f64>,
    /// Reference auto-spectrum.
    pub gxx: Vec<f64>,
    /// Measurement auto-spectrum.
    pub gyy: Vec<f64>,
    /// Cross-spectrum conj(X)·Y.
    pub gxy: Vec<Complex64>,
    /// Blocks contributing.
    pub blocks: u64,
}

/// Averaging depth and timing of one stage, as configured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageDepth {
    /// Averaging the stage uses.
    pub averaging: StageAveraging,
    /// Time from the start of the aligned stream to the stage's first block (window plus
    /// decimator span); fixed by the layout.
    pub first_block_s: f64,
    /// Averaging span after the first block: FIFO `(blocks − 1)·hop`, exponential three
    /// time constants. This is what [`DepthPolicy::FastLf`] caps.
    pub settle_s: f64,
    /// `first_block_s + settle_s`: from the start of the aligned stream until the stage's
    /// average is full (FIFO) or within e⁻³ of steady state (exponential). After
    /// [`Mtw::reset_averages`] framing continues, so a refill takes about `settle_s` plus one
    /// hop instead.
    pub fill_s: f64,
    /// Model effective averages per bin once settled; its inverse is the coherence floor
    /// on uncorrelated input.
    pub steady_eff_avg: f64,
    /// Whether the depth policy made this stage shallower than equal confidence would.
    pub capped: bool,
}

#[derive(Debug)]
struct StageRuntime {
    spec: StageSpec,
    /// Largest window misalignment, in full-rate samples, at which a delay change keeps this
    /// stage's averages.
    keep_lag: f64,
    averaging: StageAveraging,
    capped: bool,
    decimator: Option<PairDecimator>,
    est: StageEstimator,
}

/// The MTW engine. Pure and push-based: no threads, no clocks.
#[derive(Debug)]
pub struct Mtw {
    config: MtwConfig,
    layout: Layout,
    stages: Vec<StageRuntime>,
    aligner: align::Aligner,
    plan: Vec<ColumnSource>,
    pair_x: Vec<f64>,
    pair_y: Vec<f64>,
    dec_x: Vec<f64>,
    dec_y: Vec<f64>,
    deint_r: Vec<f64>,
    deint_m: Vec<f64>,
    /// Per stage: per-bin model effective averages already worked out for the stage's
    /// current block count.
    neff_memo: Vec<NeffMemo>,
}

/// The effective-averages model costs a few dozen `powf`, yet it depends only on the stage's
/// block count, and a stage serves many columns: one evaluation per new block suffices.
#[derive(Debug, Default)]
struct NeffMemo {
    held: u64,
    neff: Option<f64>,
}

/// Per-stage averaging matched to the full-rate stage's model effective count, then capped
/// by the depth policy. The flag tells whether the cap made the stage shallower.
fn stage_averaging(
    layout: &Layout,
    models: &[OverlapModel],
    averaging: Averaging,
    depth: DepthPolicy,
) -> Result<Vec<(StageAveraging, bool)>, ConfigError> {
    let max_settle_s = match depth {
        DepthPolicy::EqualConfidence => None,
        DepthPolicy::FastLf { max_settle_s } => {
            if !(max_settle_s.is_finite() && max_settle_s > 0.0) {
                return Err(ConfigError::InvalidDepthPolicy(depth));
            }
            Some(max_settle_s)
        }
    };
    match averaging {
        Averaging::Fifo { blocks } => {
            if blocks == 0 {
                return Err(ConfigError::InvalidAveraging(averaging));
            }
            let target = models[0].neff_fifo(blocks as usize, 1);
            Ok(models
                .iter()
                .zip(&layout.stages)
                .map(|(m, spec)| {
                    if spec.index == 0 {
                        return (
                            StageAveraging::Fifo {
                                blocks: blocks as usize,
                            },
                            false,
                        );
                    }
                    let matched = matched_fifo_blocks(m, target);
                    let held = match max_settle_s {
                        None => matched,
                        Some(t) => {
                            // Largest count whose span (blocks − 1)·hop fits in t; the
                            // tolerance keeps an exact fit from rounding down.
                            let cap = (t / spec.hop_s() + 1e-9).floor() as usize + 1;
                            matched.min(cap.max(MIN_FAST_LF_BLOCKS))
                        }
                    };
                    (StageAveraging::Fifo { blocks: held }, held < matched)
                })
                .collect())
        }
        Averaging::Exponential { time_constant_s } => {
            if !(time_constant_s.is_finite() && time_constant_s > 0.0) {
                return Err(ConfigError::InvalidAveraging(averaging));
            }
            let alpha0 = 1.0 - (-layout.stages[0].hop_s() / time_constant_s).exp();
            let target = models[0].neff_exponential(alpha0, None, 1);
            Ok(models
                .iter()
                .zip(&layout.stages)
                .map(|(m, spec)| {
                    if spec.index == 0 {
                        return (StageAveraging::Exponential { alpha: alpha0 }, false);
                    }
                    let matched = matched_alpha(m, target);
                    // A larger α is a shallower average.
                    let alpha = match max_settle_s {
                        None => matched,
                        Some(t) => {
                            // Three time constants, 3·hop / −ln(1 − α), equal to t.
                            let cap = 1.0 - (-3.0 * spec.hop_s() / t).exp();
                            let floor = matched_alpha(m, m.neff_fifo(MIN_FAST_LF_BLOCKS, 1));
                            matched.max(cap.min(floor.max(matched)))
                        }
                    };
                    (StageAveraging::Exponential { alpha }, alpha > matched)
                })
                .collect())
        }
    }
}

/// Bins of `stage` whose centres lie in `[lo, hi)`, excluding DC and anything above the
/// stage's served band.
fn bins_in(spec: &StageSpec, lo: f64, hi: f64) -> Option<StageBins> {
    let k_lo = ((lo / spec.bin_hz).ceil() as usize).max(1);
    let top = (spec.served_hi_hz / spec.bin_hz).floor() as usize;
    let k_hi = ((hi / spec.bin_hz).ceil() as usize).min(top + 1);
    (k_hi > k_lo).then_some(StageBins {
        stage: spec.index,
        lo: k_lo,
        hi: k_hi,
    })
}

fn column_plan(layout: &Layout, grid: &LogGrid) -> Vec<ColumnSource> {
    let stages = &layout.stages;
    (0..grid.len())
        .map(|i| {
            let f = grid.frequency(i);
            let (lo, hi) = grid.edges(i);
            if f > stages[0].served_hi_hz {
                return ColumnSource::None;
            }
            // Walk down the ladder until the column is at or above a crossover.
            let mut b = 0;
            while let Some(c) = layout.crossovers.get(b) {
                if f >= c.hi_hz {
                    break;
                }
                if f > c.lo_hz {
                    let w = c.shallow_weight(f);
                    return match (
                        bins_in(&stages[c.deep], lo, hi),
                        bins_in(&stages[b], lo, hi),
                    ) {
                        (Some(deep), Some(shallow)) => ColumnSource::Blend {
                            deep,
                            shallow,
                            shallow_weight: w,
                        },
                        (Some(one), None) | (None, Some(one)) => ColumnSource::Stage(one),
                        (None, None) => ColumnSource::None,
                    };
                }
                b += 1;
            }
            bins_in(&stages[b], lo, hi).map_or(ColumnSource::None, ColumnSource::Stage)
        })
        .collect()
}
