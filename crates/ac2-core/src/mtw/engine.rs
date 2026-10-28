//! The engine: stage setup, pushing samples, delay and averaging changes, column readout.

use num_complex::Complex64;

use super::estimator::{BlockFate, StageEstimator};
use super::{
    Averaging, ColumnInfo, ColumnSource, ConfigError, DelayChange, DepthPolicy,
    KEEP_MIN_WINDOW_CORRELATION, Layout, Mtw, MtwConfig, MtwFrame, NeffMemo, OverlapModel,
    PairDecimator, PushError, PushOutcome, SampleGate, StageAveraging, StageBins, StageDepth,
    StageRuntime, StageSpectra, Validity, align, column_plan, keep_lag, split_delay,
    stage_averaging,
};

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
        let stages: Vec<StageRuntime> = layout
            .stages
            .iter()
            .zip(per_stage)
            .zip(models)
            .map(|((spec, (averaging, capped)), model)| StageRuntime {
                keep_lag: (keep_lag(
                    &crate::window::Window::Hann.coefficients(spec.nfft),
                    KEEP_MIN_WINDOW_CORRELATION,
                ) * spec.factor) as f64,
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
        if !config.delay_samples.is_finite() {
            return Err(ConfigError::InvalidDelay(config.delay_samples));
        }
        // A splice to a larger delay reaches back at most as far as the deepest stage can
        // keep its averages; any further and every stage starts over anyway.
        let history = stages
            .iter()
            .map(|s| s.keep_lag as usize)
            .max()
            .unwrap_or(0);
        let mut m = Self {
            aligner: align::Aligner::new(split_delay(config.delay_samples).0, history),
            config,
            layout,
            stages,
            plan,
            pair_x: Vec::new(),
            pair_y: Vec::new(),
            dec_x: Vec::new(),
            dec_y: Vec::new(),
            deint_r: Vec::new(),
            deint_m: Vec::new(),
            neff_memo: Vec::new(),
        };
        m.restart();
        Ok(m)
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

    /// How full the averages are, 0 to 1: over the stages, the least of the blocks
    /// accumulated since the stage's average last started over ÷ the blocks it needs to be
    /// full ([`StageAveraging::full_blocks`]).
    ///
    /// Blocks, not elapsed time: a block that protection rejected adds nothing, so an
    /// average that has not been fed never reads full. Whether the stage started over at a
    /// restart (stream gap, delay change) or at an averaging reset only moves when its first
    /// block arrives (`first_block_s` after a restart, one hop after a reset); the block
    /// count to full is the same, so one ratio serves both. A `FastLf`-capped stage needs
    /// only its capped depth. The least full stage is not necessarily the deepest: a delay
    /// change can start over only some stages.
    pub fn fill(&self) -> f64 {
        self.stages
            .iter()
            .map(|s| (s.est.held() as f64 / s.averaging.full_blocks()).min(1.0))
            .fold(1.0, f64::min)
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

    /// Largest window misalignment, in full-rate samples, at which a delay change keeps
    /// stage `stage`'s averages ([`KEEP_MIN_WINDOW_CORRELATION`]).
    pub fn keep_lag(&self, stage: usize) -> f64 {
        self.stages[stage].keep_lag
    }

    /// Change the alignment delay (full-rate samples, fractions allowed). A delay that is
    /// not finite is ignored.
    ///
    /// The stream is not restarted: from the next pair on the reference is taken at the new
    /// whole-sample delay, blocks straddling that splice never enter an average, and new
    /// blocks get the new fraction's rotation. A stage keeps its averages, rotated by
    /// `e^{j2πfΔ/fs}` so that they read as measured at the new delay, while the window of
    /// every held block lies within [`Mtw::keep_lag`] of the new alignment; otherwise it
    /// starts over (framing continues). When no stage can keep held blocks, or the reference
    /// history does not reach back far enough, the whole ladder restarts.
    pub fn set_delay(&mut self, delay_samples: f64) -> DelayChange {
        let all = (1u32 << self.stages.len()) - 1;
        let old = self.config.delay_samples;
        if !delay_samples.is_finite() || delay_samples == old {
            return DelayChange {
                restarted: false,
                kept: all,
            };
        }
        self.config.delay_samples = delay_samples;
        let restarted = DelayChange {
            restarted: true,
            kept: 0,
        };
        let (whole_old, _) = split_delay(old);
        let (whole, frac) = split_delay(delay_samples);
        let mut kept = 0u32;
        let mut keeps_blocks = false;
        for (i, s) in self.stages.iter().enumerate() {
            let held = s.est.held() > 0;
            if !held || s.est.max_misalignment(delay_samples) <= s.keep_lag {
                kept |= 1 << i;
                keeps_blocks |= held;
            }
        }
        if !keeps_blocks {
            self.restart();
            return restarted;
        }
        if whole != whole_old {
            let Some(p0) = self.aligner.splice(whole) else {
                self.restart();
                return restarted;
            };
            for (i, s) in self.stages.iter_mut().enumerate() {
                // Stage sample j depends on pairs [j·M, j·M + L): those with
                // j·M < p0 < j·M + L mix both alignments. At full rate there are none, and
                // the empty cut at p0 still drops every block with samples on both sides.
                let m = s.spec.factor as u64;
                let l = s.spec.filter_len() as u64;
                let lo = (p0 + 1).saturating_sub(l).div_ceil(m);
                let hi = p0.div_ceil(m).max(lo);
                s.est.cut(lo, hi);
                if kept & (1 << i) != 0 && !s.est.begin_segment(whole) {
                    kept &= !(1 << i);
                }
            }
        }
        let delta = delay_samples - old;
        for (i, s) in self.stages.iter_mut().enumerate() {
            // A delay of d full-rate samples is d / M stage samples: e^{j2πk·d/(M·N)} at bin k.
            let per_bin = 1.0 / (s.spec.factor * s.spec.nfft) as f64;
            if kept & (1 << i) != 0 {
                s.est.rotate_averages(delta * per_bin);
            } else {
                s.est.reset_averages(s.averaging);
                s.est.begin_segment(whole);
            }
            s.est.set_block_rotation(frac * per_bin);
        }
        self.neff_memo.clear();
        DelayChange {
            restarted: false,
            kept,
        }
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
        self.neff_memo.clear();
        Ok(())
    }

    /// Clear the averages; alignment, decimators and framing continue.
    pub fn reset_averages(&mut self) {
        for s in &mut self.stages {
            s.est.reset_averages(s.averaging);
        }
        self.neff_memo.clear();
    }

    /// Restart the ladder at the current delay: alignment, decimators, framing and averages
    /// start over, as after a gap in the stream.
    pub fn restart(&mut self) {
        let (whole, frac) = split_delay(self.config.delay_samples);
        self.aligner.reset(whole);
        for s in &mut self.stages {
            if let Some(d) = &mut s.decimator {
                d.reset();
            }
            s.est.reset_stream(s.averaging, whole);
            s.est
                .set_block_rotation(frac / (s.spec.factor * s.spec.nfft) as f64);
        }
        self.neff_memo.clear();
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
            s.est.feed(x, y, |fate, _| match fate {
                BlockFate::Accumulated => out.blocks_accumulated += 1,
                BlockFate::Rejected => out.blocks_rejected += 1,
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

    /// H1 = Gxy / Gxx per bin of stage `stage` into `out` (0 where the reference has no
    /// energy); `false` before its first block. The density scaling of
    /// [`Mtw::stage_spectra`] is common to both spectra and cancels in the ratio, so the raw
    /// sums give the same H1 without building the spectra.
    pub fn stage_h1_into(&self, stage: usize, out: &mut Vec<Complex64>) -> bool {
        out.clear();
        let Some(s) = self.stages.get(stage) else {
            return false;
        };
        let (sums, weight) = s.est.sums();
        if weight <= 0.0 {
            return false;
        }
        out.extend(sums.xy.iter().zip(&sums.xx).map(|(xy, xx)| {
            if *xx > 0.0 {
                *xy / *xx
            } else {
                Complex64::new(0.0, 0.0)
            }
        }));
        true
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

    /// Build the current frame.
    pub fn frame(&mut self) -> MtwFrame {
        let mut f = MtwFrame {
            grid: self.config.grid,
            freq_hz: Vec::new(),
            h1: Vec::new(),
            magnitude_db: Vec::new(),
            phase_deg: Vec::new(),
            coherence: Vec::new(),
            eff_avg: Vec::new(),
            columns: Vec::new(),
            last_block_end: None,
        };
        self.frame_into(&mut f);
        f
    }

    /// Build the current frame into `f`, reusing its buffers.
    pub fn frame_into(&mut self, f: &mut MtwFrame) {
        let grid = self.config.grid;
        let n = grid.len();
        let nan = f64::NAN;
        if f.grid != grid || f.freq_hz.len() != n {
            f.freq_hz = grid.frequencies();
        }
        f.grid = grid;
        let reset = |v: &mut Vec<f64>| {
            v.clear();
            v.resize(n, nan);
        };
        f.h1.clear();
        f.h1.resize(n, Complex64::new(nan, nan));
        reset(&mut f.magnitude_db);
        reset(&mut f.phase_deg);
        reset(&mut f.coherence);
        reset(&mut f.eff_avg);
        f.columns.clear();
        f.last_block_end = self
            .stages
            .iter()
            .filter_map(|s| self.stage_block_end(s))
            .max();
        self.neff_memo
            .resize_with(self.stages.len(), NeffMemo::default);
        for (memo, s) in self.neff_memo.iter_mut().zip(&self.stages) {
            let held = s.est.held();
            if memo.held != held {
                memo.held = held;
                memo.neff = None;
            }
        }
        for i in 0..n {
            let src = self.plan[i];
            let est = match src {
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
                source: src,
            });
        }
    }

    fn column_estimate(&mut self, b: StageBins) -> Result<ColumnEstimate, Validity> {
        let s = &self.stages[b.stage];
        if s.est.held() == 0 {
            return Err(Validity::Settling);
        }
        let (sums, _) = s.est.sums();
        let (mut xx, mut yy, mut xy) = (0.0, 0.0, Complex64::new(0.0, 0.0));
        // Σₖ Gxxₖ·γ²ₖ. The sums' common density scale cancels against Σ Gxx.
        let mut xx_coh = 0.0;
        for k in b.lo..b.hi {
            let (bxx, byy, bxy) = (sums.xx[k], sums.yy[k], sums.xy[k]);
            xx += bxx;
            yy += byy;
            xy += bxy;
            // A bin without reference or measurement energy holds no coherent power
            // (|Gxy|² ≤ Gxx·Gyy); rounding can push γ²ₖ a hair above 1.
            if bxx > 0.0 && byy > 0.0 {
                xx_coh += bxx * (bxy.norm_sqr() / (bxx * byy)).min(1.0);
            }
        }
        if !(xx > 0.0 && xx.is_finite()) {
            return Err(Validity::NoReference);
        }
        if !(yy > 0.0 && yy.is_finite()) {
            return Err(Validity::NoMeasurement);
        }
        let memo = &mut self.neff_memo[b.stage];
        let eff_avg = *memo.neff.get_or_insert_with(|| s.est.neff(1));
        Ok(ColumnEstimate {
            h1: xy / xx,
            coherence: (xx_coh / xx).min(1.0),
            eff_avg,
        })
    }
}

struct ColumnEstimate {
    h1: Complex64,
    coherence: f64,
    eff_avg: f64,
}
