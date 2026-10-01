//! Multi-time-window (MTW) dual-channel H1 / coherence engine (PLAN.md §5.1).
//!
//! # Pipeline
//! 1. **Alignment** at full rate by one signed integer delay ([`Mtw::set_delay`]). Changing
//!    the delay restarts the whole ladder (the decimators and block grid are defined on the
//!    aligned stream).
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
//! 5. **Columns** on the shared [`LogGrid`]: each column sums the cross- and auto-spectra of
//!    the bins whose centres fall inside it and divides once. Columns that contain no bin are
//!    thinned (NaN with a reason), never interpolated. Across each crossover H1 is blended
//!    complex over 1/3 octave; γ² there is a labelled display blend of the two stages'
//!    estimates.
//!
//! Absolute levels never come from this engine: decimated stages carry the decimator's
//! response, which cancels in H1 and γ² but not in Gxx or Gyy.

mod align;
pub mod estimator;
pub mod fir;
pub mod layout;

use num_complex::Complex64;

use crate::grid::LogGrid;
pub use estimator::{Averaging, DepthPolicy, MIN_FAST_LF_BLOCKS, OverlapModel, StageAveraging};
use estimator::{BlockFate, StageEstimator, matched_alpha, matched_fifo_blocks};
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
    /// Alignment delay in full-rate samples; positive = measurement late.
    pub delay_samples: i64,
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
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Layout(e) => write!(f, "{e}"),
            ConfigError::InvalidAveraging(a) => write!(f, "invalid averaging {a:?}"),
            ConfigError::InvalidDepthPolicy(p) => write!(f, "invalid depth policy {p:?}"),
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
    /// Blocks completed while frozen (not accumulated).
    pub blocks_frozen: usize,
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
    /// γ² = |ΣGxy|² / (ΣGxx·ΣGyy); a display blend inside crossovers.
    pub coherence: Vec<f64>,
    /// Model effective number of averages (stationary white-noise model of block overlap,
    /// FIFO/exponential weights and adjacent-bin correlation). For blends, the smaller of
    /// the two stages' values. A model value, not a measurement.
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
    frozen: bool,
    pair_x: Vec<f64>,
    pair_y: Vec<f64>,
    dec_x: Vec<f64>,
    dec_y: Vec<f64>,
    deint_r: Vec<f64>,
    deint_m: Vec<f64>,
}

impl Mtw {
    /// Build an engine.
    pub fn new(config: MtwConfig) -> Result<Self, ConfigError> {
        let layout = Layout::new(config.sample_rate_hz, config.ladder)?;
        let models: Vec<OverlapModel> = layout
            .stages
            .iter()
            .map(|s| OverlapModel::new(&crate::window::Window::Hann.coefficients(s.nfft), s.hop))
            .collect();
        let per_stage = stage_averaging(&layout, &models, config.averaging, config.depth)?;
        let stages = layout
            .stages
            .iter()
            .zip(per_stage)
            .zip(models)
            .map(|((spec, (averaging, capped)), model)| StageRuntime {
                decimator: spec
                    .decimator
                    .as_ref()
                    .map(|d| PairDecimator::new(d.taps.clone(), spec.factor)),
                est: StageEstimator::new(spec.nfft, spec.hop, averaging, model),
                averaging,
                capped,
                spec: spec.clone(),
            })
            .collect();
        let plan = column_plan(&layout, &config.grid);
        Ok(Self {
            aligner: align::Aligner::new(config.delay_samples),
            config,
            layout,
            stages,
            plan,
            frozen: false,
            pair_x: Vec::new(),
            pair_y: Vec::new(),
            dec_x: Vec::new(),
            dec_y: Vec::new(),
            deint_r: Vec::new(),
            deint_m: Vec::new(),
        })
    }

    /// The configuration in force.
    pub fn config(&self) -> &MtwConfig {
        &self.config
    }

    /// Stage layout.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Averaging each stage actually uses after depth matching.
    pub fn stage_averaging(&self) -> Vec<StageAveraging> {
        self.stages.iter().map(|s| s.averaging).collect()
    }

    /// Per-stage averaging depth, settling times and steady-state effective averages.
    pub fn stage_depths(&self) -> Vec<StageDepth> {
        self.stages
            .iter()
            .enumerate()
            .map(|(b, s)| {
                let first_block_s = s.spec.first_block_s();
                let settle_s = s.averaging.settle_blocks() * s.spec.hop_s();
                StageDepth {
                    averaging: s.averaging,
                    first_block_s,
                    settle_s,
                    fill_s: first_block_s + settle_s,
                    steady_eff_avg: self.steady_eff_avg(b),
                    capped: s.capped,
                }
            })
            .collect()
    }

    /// Model effective averages of stage `stage` once its average is full (FIFO) or in
    /// steady state (exponential), for a single bin.
    pub fn steady_eff_avg(&self, stage: usize) -> f64 {
        let s = &self.stages[stage];
        match s.averaging {
            StageAveraging::Fifo { blocks } => s.est.model.neff_fifo(blocks, 1),
            StageAveraging::Exponential { alpha } => s.est.model.neff_exponential(alpha, None, 1),
        }
    }

    /// Current model effective averages of stage `stage` for a column of `bins` adjacent bins.
    pub fn eff_avg(&self, stage: usize, bins: usize) -> f64 {
        self.stages[stage].est.neff(bins)
    }

    /// Change the alignment delay. Restarts the ladder: the block grid and decimator phase
    /// are defined on the aligned stream. (A later improvement may avoid the resettle.)
    pub fn set_delay(&mut self, delay_samples: i64) {
        self.config.delay_samples = delay_samples;
        self.restart();
    }

    /// Change averaging. Clears the averages; framing continues.
    pub fn set_averaging(&mut self, averaging: Averaging) -> Result<(), ConfigError> {
        self.reconfigure_depth(averaging, self.config.depth)
    }

    /// Change the depth policy. Like [`Mtw::set_averaging`] it clears the averages (every
    /// stage, even one whose depth does not change, so all stages restart together);
    /// framing continues. An invalid policy leaves the engine untouched.
    pub fn set_depth_policy(&mut self, depth: DepthPolicy) -> Result<(), ConfigError> {
        self.reconfigure_depth(self.config.averaging, depth)
    }

    fn reconfigure_depth(
        &mut self,
        averaging: Averaging,
        depth: DepthPolicy,
    ) -> Result<(), ConfigError> {
        let models: Vec<OverlapModel> = self.stages.iter().map(|s| s.est.model.clone()).collect();
        let per_stage = stage_averaging(&self.layout, &models, averaging, depth)?;
        self.config.averaging = averaging;
        self.config.depth = depth;
        for (s, (a, capped)) in self.stages.iter_mut().zip(per_stage) {
            s.averaging = a;
            s.capped = capped;
            s.est.reset_averages(a);
        }
        Ok(())
    }

    /// Freeze: completed blocks are not accumulated; the frame keeps showing the held
    /// averages. The block grid keeps advancing.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// Whether frozen.
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Clear the averages; alignment, decimators and framing continue.
    pub fn reset_averages(&mut self) {
        for s in &mut self.stages {
            s.est.reset_averages(s.averaging);
        }
    }

    fn restart(&mut self) {
        self.aligner.reset(self.config.delay_samples);
        for s in &mut self.stages {
            if let Some(d) = &mut s.decimator {
                d.reset();
            }
            s.est.reset_stream(s.averaging);
        }
    }

    /// Push planar blocks of reference and measurement samples whose first sample has input
    /// index `start`. A `start` that does not continue the previous push restarts the
    /// ladder (a gap or overlap in the stream invalidates the block grid).
    pub fn push<S: Copy + Into<f64>>(
        &mut self,
        start: u64,
        reference: &[S],
        measurement: &[S],
        gate: SampleGate,
    ) -> Result<PushOutcome, PushError> {
        if reference.len() != measurement.len() {
            return Err(PushError::LengthMismatch {
                reference: reference.len(),
                measurement: measurement.len(),
            });
        }
        Ok(self.push_iter(
            start,
            reference.len(),
            reference.iter().map(|&v| v.into()),
            measurement.iter().map(|&v| v.into()),
            gate,
        ))
    }

    /// Push interleaved frames (`channels` samples per frame), taking the reference and
    /// measurement from the given channel positions.
    pub fn push_interleaved<S: Copy + Into<f64>>(
        &mut self,
        start: u64,
        frames: &[S],
        channels: usize,
        reference_channel: usize,
        measurement_channel: usize,
        gate: SampleGate,
    ) -> Result<PushOutcome, PushError> {
        if channels == 0
            || reference_channel >= channels
            || measurement_channel >= channels
            || !frames.len().is_multiple_of(channels)
        {
            return Err(PushError::BadInterleave {
                channels,
                reference: reference_channel,
                measurement: measurement_channel,
            });
        }
        let mut r = std::mem::take(&mut self.deint_r);
        let mut m = std::mem::take(&mut self.deint_m);
        r.clear();
        m.clear();
        for f in frames.chunks_exact(channels) {
            r.push(f[reference_channel].into());
            m.push(f[measurement_channel].into());
        }
        let out = self.push_iter(start, r.len(), r.iter().copied(), m.iter().copied(), gate);
        self.deint_r = r;
        self.deint_m = m;
        Ok(out)
    }

    fn push_iter(
        &mut self,
        start: u64,
        len: usize,
        reference: impl Iterator<Item = f64>,
        measurement: impl Iterator<Item = f64>,
        gate: SampleGate,
    ) -> PushOutcome {
        let mut out = PushOutcome::default();
        if len == 0 {
            return out;
        }
        let start = start as i64;
        if self.aligner.expected().is_some_and(|e| e != start) {
            self.restart();
            out.restarted = true;
        }
        self.pair_x.clear();
        self.pair_y.clear();
        self.aligner.push(
            start,
            reference,
            measurement,
            &mut self.pair_x,
            &mut self.pair_y,
        );
        if gate == SampleGate::Reject
            && let Some((a, b)) = self.aligner.pairs_touched(start, start + len as i64)
        {
            for s in &mut self.stages {
                // Output j of the decimator depends on pairs [jM, jM + L).
                let m = s.spec.factor as u64;
                let l = s.spec.filter_len() as u64;
                let lo = (a + 1).saturating_sub(l).div_ceil(m);
                let hi = b.div_ceil(m);
                s.est.reject(lo, hi);
            }
        }
        let frozen = self.frozen;
        for s in &mut self.stages {
            let (x, y): (&[f64], &[f64]) = match &mut s.decimator {
                Some(d) => {
                    self.dec_x.clear();
                    self.dec_y.clear();
                    d.push(&self.pair_x, &self.pair_y, &mut self.dec_x, &mut self.dec_y);
                    (&self.dec_x, &self.dec_y)
                }
                None => (&self.pair_x, &self.pair_y),
            };
            s.est.feed(x, y, frozen, |fate, _| match fate {
                BlockFate::Accumulated => out.blocks_accumulated += 1,
                BlockFate::Rejected => out.blocks_rejected += 1,
                BlockFate::Frozen => out.blocks_frozen += 1,
            });
        }
        out
    }

    /// One past the newest measurement input index stage `s`'s newest block depends on.
    fn stage_block_end(&self, s: &StageRuntime) -> Option<u64> {
        let start = s.est.last_block_start?;
        let origin = self.aligner.origin()?;
        let m = s.spec.factor as u64;
        let newest_pair = (start + s.spec.nfft as u64 - 1) * m + s.spec.filter_len() as u64 - 1;
        Some((origin + newest_pair as i64 + 1) as u64)
    }

    /// Averaged spectra of one stage (density-scaled), or `None` before its first block.
    pub fn stage_spectra(&self, stage: usize) -> Option<StageSpectra> {
        let s = self.stages.get(stage)?;
        let (sums, weight) = s.est.sums();
        if weight <= 0.0 {
            return None;
        }
        let n = s.spec.nfft;
        let bins = s.est.bins();
        let base = 1.0 / (weight * s.spec.rate_hz * s.est.window_sum_sq());
        let fold = |k: usize| if k == 0 || 2 * k == n { 1.0 } else { 2.0 };
        Some(StageSpectra {
            freq_hz: (0..bins).map(|k| k as f64 * s.spec.bin_hz).collect(),
            gxx: (0..bins).map(|k| sums.xx[k] * base * fold(k)).collect(),
            gyy: (0..bins).map(|k| sums.yy[k] * base * fold(k)).collect(),
            gxy: (0..bins).map(|k| sums.xy[k] * (base * fold(k))).collect(),
            blocks: s.est.held(),
        })
    }

    /// Column plan (static for the engine's layout and grid).
    pub fn column_sources(&self) -> &[ColumnSource] {
        &self.plan
    }

    /// Build the current frame.
    pub fn frame(&self) -> MtwFrame {
        let grid = self.config.grid;
        let n = grid.len();
        let nan = f64::NAN;
        let mut f = MtwFrame {
            grid,
            freq_hz: grid.frequencies(),
            h1: vec![Complex64::new(nan, nan); n],
            magnitude_db: vec![nan; n],
            phase_deg: vec![nan; n],
            coherence: vec![nan; n],
            eff_avg: vec![nan; n],
            columns: Vec::with_capacity(n),
            last_block_end: self
                .stages
                .iter()
                .filter_map(|s| self.stage_block_end(s))
                .max(),
        };
        for (i, src) in self.plan.iter().enumerate() {
            let est = match *src {
                ColumnSource::None => {
                    let fc = f.freq_hz[i];
                    let validity = if fc > self.layout.stages[0].served_hi_hz {
                        Validity::OutOfBand
                    } else {
                        Validity::Thinned
                    };
                    Err(validity)
                }
                ColumnSource::Stage(b) => self.column_estimate(b),
                ColumnSource::Blend {
                    deep,
                    shallow,
                    shallow_weight: w,
                } => match (self.column_estimate(deep), self.column_estimate(shallow)) {
                    (Ok(d), Ok(s)) => Ok(ColumnEstimate {
                        h1: d.h1 * (1.0 - w) + s.h1 * w,
                        coherence: d.coherence * (1.0 - w) + s.coherence * w,
                        eff_avg: d.eff_avg.min(s.eff_avg),
                    }),
                    (Err(e), _) | (_, Err(e)) => Err(e),
                },
            };
            let validity = match est {
                Ok(e) => {
                    f.h1[i] = e.h1;
                    f.magnitude_db[i] = 20.0 * e.h1.norm().log10();
                    f.phase_deg[i] = e.h1.arg().to_degrees();
                    f.coherence[i] = e.coherence;
                    f.eff_avg[i] = e.eff_avg;
                    Validity::Valid
                }
                Err(v) => v,
            };
            f.columns.push(ColumnInfo {
                validity,
                source: *src,
            });
        }
        f
    }

    fn column_estimate(&self, b: StageBins) -> Result<ColumnEstimate, Validity> {
        let s = &self.stages[b.stage];
        if s.est.held() == 0 {
            return Err(Validity::Settling);
        }
        let (sums, _) = s.est.sums();
        let xx: f64 = sums.xx[b.lo..b.hi].iter().sum();
        let yy: f64 = sums.yy[b.lo..b.hi].iter().sum();
        let xy: Complex64 = sums.xy[b.lo..b.hi].iter().sum();
        if !(xx > 0.0 && xx.is_finite()) {
            return Err(Validity::NoReference);
        }
        if !(yy > 0.0 && yy.is_finite()) {
            return Err(Validity::NoMeasurement);
        }
        Ok(ColumnEstimate {
            h1: xy / xx,
            coherence: (xy.norm_sqr() / (xx * yy)).min(1.0),
            eff_avg: s.est.neff(b.count()),
        })
    }
}

struct ColumnEstimate {
    h1: Complex64,
    coherence: f64,
    eff_avg: f64,
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
