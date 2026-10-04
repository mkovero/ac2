//! SPL time weighting (Fast/Slow/Impulse), Leq, peak (PLAN.md §5.3, §3.6).
//!
//! All levels are on the dBFS scale of decision 4a: a mean square `p` reads 10·lg(2p), so a
//! full-scale sine reads 0 dBFS, and a peak `a` reads 10·lg(2a²) (+3.01 dB above the RMS level
//! of a sine of that peak, as IEC 61672-1 peak sound level relates to sound level). Sound
//! pressure level is the dBFS value plus a [`Sensitivity`] offset; the voltage scale is never
//! in this path.
//!
//! Signal chain of [`SplMeter`]:
//!
//! ```text
//! raw ──┬── [mic-curve correction, §5.7] ──┬── A ──┬── F, S, I ── L, Lmax, Lmin
//!       │                                   ├── C ──┤
//!       │                                   └── Z ──┴── Leq (f64 energy)
//!       └── C, Z (uncorrected) ── |x| max ── Lpeak
//! ```
//!
//! Every weighting combination runs at once; the meter reports the chosen one, so the
//! choice changes on a running meter without a restart or a settling detector.
//!
//! The mic-curve correction is a minimum-phase FIR ([`crate::mic_curve`]) normalised to
//! 0 dB at the calibrator frequency. Lpeak stays on the uncorrected samples (§5.3): the
//! corrected path lags by one convolution partition and carries the curve's HF boost,
//! which would change a crest-factor reading without a standard tolerance to judge it by.

use crate::mic_curve::PartitionedFir;
use crate::spectrum::power_dbfs;
use crate::weighting::{Weighting, WeightingError, WeightingFilter};

/// Exponential time weighting of the squared signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeWeighting {
    /// F: τ = 125 ms (IEC 61672-1 5.8.1).
    Fast,
    /// S: τ = 1 s (IEC 61672-1 5.8.1).
    Slow,
    /// I: 35 ms exponential averaging followed by a peak detector that follows a rising
    /// average at once and otherwise decays exponentially with 1.5 s (the IEC 60651 impulse
    /// characteristic; not part of IEC 61672-1). Applying the asymmetry to the averaged mean
    /// square rather than to x² keeps a steady sine reading its RMS.
    Impulse,
}

impl TimeWeighting {
    /// Averaging time constant, which sets the rise (s).
    pub fn rise_s(self) -> f64 {
        match self {
            TimeWeighting::Fast => 0.125,
            TimeWeighting::Slow => 1.0,
            TimeWeighting::Impulse => 0.035,
        }
    }

    /// Time constant of the fall after the signal stops (s).
    pub fn fall_s(self) -> f64 {
        match self {
            TimeWeighting::Fast => 0.125,
            TimeWeighting::Slow => 1.0,
            TimeWeighting::Impulse => 1.5,
        }
    }
}

/// One-pole smoothing coefficient for time constant `tau` at rate `fs`: the discrete
/// recursion ms += α(x² − ms) has exactly the continuous decay e^{−t/τ} at the sample
/// instants.
fn one_pole_alpha(tau: f64, fs: f64) -> f64 {
    1.0 - (-1.0 / (fs * tau)).exp()
}

/// Exponential time-weighting detector on the squared signal (mean square in FS²).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeWeightedDetector {
    weighting: TimeWeighting,
    alpha_avg: f64,
    alpha_fall: f64,
    avg: f64,
    ms: f64,
}

impl TimeWeightedDetector {
    /// Detector at rate `fs`, starting from silence.
    pub fn new(weighting: TimeWeighting, fs: f64) -> Self {
        Self {
            weighting,
            alpha_avg: one_pole_alpha(weighting.rise_s(), fs),
            alpha_fall: one_pole_alpha(weighting.fall_s(), fs),
            avg: 0.0,
            ms: 0.0,
        }
    }

    /// Time weighting.
    pub fn weighting(&self) -> TimeWeighting {
        self.weighting
    }

    /// Feeds one sample; returns the new time-weighted mean square.
    #[inline]
    pub fn push(&mut self, x: f64) -> f64 {
        self.avg += self.alpha_avg * (x * x - self.avg);
        self.ms = match self.weighting {
            TimeWeighting::Fast | TimeWeighting::Slow => self.avg,
            TimeWeighting::Impulse if self.avg >= self.ms => self.avg,
            TimeWeighting::Impulse => (self.ms * (1.0 - self.alpha_fall)).max(self.avg),
        };
        self.ms
    }

    /// Current time-weighted mean square (FS²).
    pub fn mean_square(&self) -> f64 {
        self.ms
    }

    /// Current time-weighted level in dBFS.
    pub fn level_dbfs(&self) -> f64 {
        power_dbfs(self.ms)
    }

    /// Back to silence.
    pub fn reset(&mut self) {
        self.avg = 0.0;
        self.ms = 0.0;
    }
}

/// Equivalent continuous level over an arbitrary interval: f64 energy sum with Neumaier
/// compensation, so hour-long intervals at 192 kHz keep full precision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leq {
    fs: f64,
    sum: f64,
    comp: f64,
    samples: u64,
}

impl Leq {
    /// Empty interval at rate `fs`.
    pub fn new(fs: f64) -> Self {
        Self {
            fs,
            sum: 0.0,
            comp: 0.0,
            samples: 0,
        }
    }

    fn add(&mut self, v: f64) {
        let t = self.sum + v;
        if self.sum.abs() >= v.abs() {
            self.comp += (self.sum - t) + v;
        } else {
            self.comp += (v - t) + self.sum;
        }
        self.sum = t;
    }

    /// Adds one sample.
    #[inline]
    pub fn push_sample(&mut self, x: f64) {
        self.add(x * x);
        self.samples += 1;
    }

    /// Adds a block of samples.
    pub fn push(&mut self, block: &[f64]) {
        // Plain partial sums over short runs are exact enough; the compensated sum carries
        // the long-interval total.
        for chunk in block.chunks(256) {
            let s: f64 = chunk.iter().map(|x| x * x).sum();
            self.add(s);
        }
        self.samples += block.len() as u64;
    }

    /// Samples in the interval.
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Interval length in seconds.
    pub fn duration_s(&self) -> f64 {
        self.samples as f64 / self.fs
    }

    /// Energy Σx² (FS²·samples).
    pub fn energy(&self) -> f64 {
        self.sum + self.comp
    }

    /// Mean square over the interval (FS²); NaN for an empty interval.
    pub fn mean_square(&self) -> f64 {
        if self.samples == 0 {
            f64::NAN
        } else {
            self.energy() / self.samples as f64
        }
    }

    /// Leq in dBFS.
    pub fn level_dbfs(&self) -> f64 {
        power_dbfs(self.mean_square())
    }

    /// Sound exposure level in dBFS·s: Leq + 10·lg(T / 1 s) (IEC 61672-1 Formula 4).
    pub fn exposure_level_dbfs(&self) -> f64 {
        power_dbfs(self.energy() / self.fs)
    }

    /// Starts a new interval.
    pub fn reset(&mut self) {
        self.sum = 0.0;
        self.comp = 0.0;
        self.samples = 0;
    }
}

/// Largest absolute sample value.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PeakDetector {
    peak: f64,
}

impl PeakDetector {
    /// Feeds one sample.
    #[inline]
    pub fn push(&mut self, x: f64) {
        self.peak = self.peak.max(x.abs());
    }

    /// Largest |x| so far.
    pub fn peak(&self) -> f64 {
        self.peak
    }

    /// Peak level in dBFS: 10·lg(2·peak²).
    pub fn level_dbfs(&self) -> f64 {
        power_dbfs(self.peak * self.peak)
    }

    /// Starts a new interval.
    pub fn reset(&mut self) {
        self.peak = 0.0;
    }
}

/// Microphone sensitivity calibration: dB SPL = dBFS + `offset_db`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sensitivity {
    /// SPL of 0 dBFS.
    pub offset_db: f64,
}

impl Sensitivity {
    /// From a calibrator of known level (e.g. 94 or 114 dB SPL) and the level measured from
    /// it in dBFS on the same scale as every other level here.
    pub fn from_calibrator(calibrator_db_spl: f64, measured_dbfs: f64) -> Self {
        Self {
            offset_db: calibrator_db_spl - measured_dbfs,
        }
    }

    /// Converts a dBFS level to dB SPL.
    pub fn spl(&self, dbfs: f64) -> f64 {
        dbfs + self.offset_db
    }
}

/// Unit of the values in [`Levels`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LevelScale {
    /// dBFS (0 dBFS = full-scale sine).
    Dbfs,
    /// dB SPL re 20 µPa via a [`Sensitivity`].
    DbSpl,
}

/// A snapshot of an [`SplMeter`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    /// Unit of every value below.
    pub scale: LevelScale,
    /// Current time-weighted level.
    pub level: f64,
    /// Maximum time-weighted level in the interval.
    pub lmax: f64,
    /// Minimum time-weighted level in the interval (after the detector has settled).
    pub lmin: f64,
    /// Equivalent continuous level over the interval.
    pub leq: f64,
    /// Peak level (C or Z, uncorrected path) over the interval.
    pub lpeak: f64,
    /// Interval length in seconds.
    pub duration_s: f64,
}

impl Levels {
    /// The same levels in dB SPL.
    ///
    /// # Panics
    /// If the levels are already in dB SPL.
    pub fn calibrated(self, sensitivity: Sensitivity) -> Levels {
        assert_eq!(self.scale, LevelScale::Dbfs, "levels already calibrated");
        let s = |v| sensitivity.spl(v);
        Levels {
            scale: LevelScale::DbSpl,
            level: s(self.level),
            lmax: s(self.lmax),
            lmin: s(self.lmin),
            leq: s(self.leq),
            lpeak: s(self.lpeak),
            duration_s: self.duration_s,
        }
    }
}

/// Frequency weighting for the peak path (IEC 61672-1 allows C; Z is offered as well).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeakWeighting {
    /// C weighting (LCpeak).
    C,
    /// No weighting (LZpeak).
    Z,
}

/// SPL meter configuration: the rate and the weightings the meter reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplMeterConfig {
    /// Sample rate in Hz.
    pub fs: f64,
    /// Frequency weighting reported for L, Lmax, Lmin and Leq.
    pub weighting: Weighting,
    /// Time weighting reported for L, Lmax and Lmin.
    pub time_weighting: TimeWeighting,
    /// Frequency weighting reported for Lpeak.
    pub peak_weighting: PeakWeighting,
}

const TIME_WEIGHTINGS: [TimeWeighting; 3] = [
    TimeWeighting::Fast,
    TimeWeighting::Slow,
    TimeWeighting::Impulse,
];

fn w_index(w: Weighting) -> usize {
    match w {
        Weighting::A => 0,
        Weighting::C => 1,
        Weighting::Z => 2,
    }
}

fn t_index(t: TimeWeighting) -> usize {
    match t {
        TimeWeighting::Fast => 0,
        TimeWeighting::Slow => 1,
        TimeWeighting::Impulse => 2,
    }
}

fn p_index(p: PeakWeighting) -> usize {
    match p {
        PeakWeighting::C => 0,
        PeakWeighting::Z => 1,
    }
}

/// Lmin is meaningless while a detector rises from its silent initial state; five rise time
/// constants bring it within 0.03 dB of a steady input.
fn settle_samples(t: TimeWeighting, fs: f64) -> u64 {
    (5.0 * t.rise_s() * fs).ceil() as u64
}

/// Sound level meter on one input: time-weighted level with Lmax/Lmin, Leq and Lpeak.
///
/// Every frequency weighting (A, C, Z) runs with every time weighting (F, S, I), and the
/// peak with C and Z, all the time; [`SplMeter::select`] only chooses which are reported. A
/// change of weighting therefore reads a settled detector at once (a Slow detector started
/// at the switch would take 5 s to settle), and Lmax, Lmin, Leq and Lpeak of the newly
/// chosen weighting cover the same interval as before the change.
#[derive(Debug, Clone)]
pub struct SplMeter {
    cfg: SplMeterConfig,
    weight_a: WeightingFilter,
    weight_c: WeightingFilter,
    peak_c: WeightingFilter,
    /// `[frequency weighting][time weighting]`.
    detectors: [[TimeWeightedDetector; 3]; 3],
    leq: [Leq; 3],
    /// C, Z.
    peak: [PeakDetector; 2],
    max_ms: [[f64; 3]; 3],
    min_ms: [[f64; 3]; 3],
    /// Samples before each time weighting's Lmin counts.
    settle_left: [u64; 3],
    settle_samples: [u64; 3],
    correction: Option<PartitionedFir>,
    corrected: Vec<f64>,
}

impl SplMeter {
    /// Builds the meter; fails if the rate is too low for A/C weighting.
    pub fn new(cfg: SplMeterConfig) -> Result<Self, WeightingError> {
        let fs = cfg.fs;
        let settle = TIME_WEIGHTINGS.map(|t| settle_samples(t, fs));
        let row = || TIME_WEIGHTINGS.map(|t| TimeWeightedDetector::new(t, fs));
        Ok(Self {
            weight_a: WeightingFilter::new(Weighting::A, fs)?,
            weight_c: WeightingFilter::new(Weighting::C, fs)?,
            peak_c: WeightingFilter::new(Weighting::C, fs)?,
            detectors: [row(), row(), row()],
            leq: [Leq::new(fs); 3],
            peak: [PeakDetector::default(); 2],
            max_ms: [[0.0; 3]; 3],
            min_ms: [[f64::INFINITY; 3]; 3],
            settle_left: settle,
            settle_samples: settle,
            correction: None,
            corrected: Vec::new(),
            cfg,
        })
    }

    /// Chooses the weightings reported from now on. Nothing restarts: every combination has
    /// been measuring all along, over the same interval.
    pub fn select(
        &mut self,
        weighting: Weighting,
        time_weighting: TimeWeighting,
        peak_weighting: PeakWeighting,
    ) {
        self.cfg.weighting = weighting;
        self.cfg.time_weighting = time_weighting;
        self.cfg.peak_weighting = peak_weighting;
    }

    /// Runs the time-weighted, Lmax/Lmin and Leq paths through `taps` (a mic-curve
    /// correction from [`crate::mic_curve::Correction::design_fir`]) before frequency
    /// weighting; `None` removes it. Filter state starts silent; the interval continues.
    pub fn set_correction(&mut self, taps: Option<&[f64]>) {
        let part = crate::mic_curve::fir_partition(self.cfg.fs);
        self.correction = taps.map(|h| PartitionedFir::new(h, part));
        let lat = self.correction.as_ref().map_or(0, PartitionedFir::latency);
        self.corrected = vec![0.0; lat];
        // The corrected path starts one partition late: Lmin waits for it too.
        for (i, t) in TIME_WEIGHTINGS.into_iter().enumerate() {
            self.settle_samples[i] = settle_samples(t, self.cfg.fs) + lat as u64;
            self.settle_left[i] = self.settle_left[i].max(self.settle_samples[i]);
        }
    }

    /// Whether a mic-curve correction filter is in the path.
    pub fn has_correction(&self) -> bool {
        self.correction.is_some()
    }

    /// Configuration: the rate and the weightings reported.
    pub fn config(&self) -> &SplMeterConfig {
        &self.cfg
    }

    /// Processes raw input samples (FS). Does not allocate.
    ///
    /// The mic-curve correction (PLAN.md §5.7), when set, goes on `x` immediately before
    /// the frequency weightings — so it feeds the time-weighted, Lmax/Lmin and Leq paths —
    /// and never before the peak path: LCpeak stays on the uncorrected samples (§5.3).
    pub fn process(&mut self, block: &[f64]) {
        let Some(mut fir) = self.correction.take() else {
            for &x in block {
                self.step(x, x);
            }
            return;
        };
        let mut corrected = std::mem::take(&mut self.corrected);
        for chunk in block.chunks(corrected.len().max(1)) {
            let c = &mut corrected[..chunk.len()];
            fir.process(chunk, c);
            for (&x, &xc) in chunk.iter().zip(c.iter()) {
                self.step(x, xc);
            }
        }
        self.corrected = corrected;
        self.correction = Some(fir);
    }

    /// One sample: `x` raw (peak path), `xc` mic-corrected (everything else).
    #[inline]
    fn step(&mut self, x: f64, xc: f64) {
        let ys = [
            self.weight_a.process_sample(xc),
            self.weight_c.process_sample(xc),
            xc,
        ];
        let settled = self.settle_left.map(|n| n == 0);
        for (w, &y) in ys.iter().enumerate() {
            self.leq[w].push_sample(y);
            let (dets, maxs, mins) = (
                &mut self.detectors[w],
                &mut self.max_ms[w],
                &mut self.min_ms[w],
            );
            for (((d, max), min), &ok) in dets.iter_mut().zip(maxs).zip(mins).zip(&settled) {
                let ms = d.push(y);
                *max = max.max(ms);
                if ok {
                    *min = min.min(ms);
                }
            }
        }
        for n in &mut self.settle_left {
            *n = n.saturating_sub(1);
        }
        self.peak[0].push(self.peak_c.process_sample(x));
        self.peak[1].push(x);
    }

    /// Current levels in dBFS, in the weightings reported.
    pub fn levels(&self) -> Levels {
        self.levels_of(
            self.cfg.weighting,
            self.cfg.time_weighting,
            self.cfg.peak_weighting,
        )
    }

    /// Current levels in dBFS in any weightings.
    pub fn levels_of(&self, w: Weighting, t: TimeWeighting, p: PeakWeighting) -> Levels {
        let (wi, ti) = (w_index(w), t_index(t));
        let min = self.min_ms[wi][ti];
        Levels {
            scale: LevelScale::Dbfs,
            level: self.detectors[wi][ti].level_dbfs(),
            lmax: power_dbfs(self.max_ms[wi][ti]),
            lmin: if min.is_finite() {
                power_dbfs(min)
            } else {
                f64::NAN
            },
            leq: self.leq[wi].level_dbfs(),
            lpeak: self.peak[p_index(p)].level_dbfs(),
            duration_s: self.leq[wi].duration_s(),
        }
    }

    /// The Leq accumulator of the current interval, in the frequency weighting reported.
    pub fn leq(&self) -> &Leq {
        &self.leq[w_index(self.cfg.weighting)]
    }

    /// Starts a new interval for Leq, Lpeak, Lmax and Lmin. Filters and detectors keep their
    /// state, so the running level is continuous.
    pub fn reset_interval(&mut self) {
        for l in &mut self.leq {
            l.reset();
        }
        for p in &mut self.peak {
            p.reset();
        }
        for (w, row) in self.detectors.iter().enumerate() {
            for (t, d) in row.iter().enumerate() {
                self.max_ms[w][t] = d.mean_square();
            }
        }
        self.min_ms = [[f64::INFINITY; 3]; 3];
    }

    /// Back to the initial state: silent filters and detectors, empty interval.
    pub fn reset(&mut self) {
        self.weight_a.reset();
        self.weight_c.reset();
        self.peak_c.reset();
        if let Some(c) = &mut self.correction {
            c.reset();
        }
        for d in self.detectors.iter_mut().flatten() {
            d.reset();
        }
        self.reset_interval();
        self.max_ms = [[0.0; 3]; 3];
        self.settle_left = self.settle_samples;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_testkit::golden::GoldenSet;
    use std::f64::consts::TAU;

    const RATES: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];

    fn meter(fs: f64, w: Weighting, tw: TimeWeighting, pw: PeakWeighting) -> SplMeter {
        SplMeter::new(SplMeterConfig {
            fs,
            weighting: w,
            time_weighting: tw,
            peak_weighting: pw,
        })
        .expect("meter")
    }

    fn sine(f: f64, amp: f64, fs: f64, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| amp * (TAU * f * i as f64 / fs).sin())
            .collect()
    }

    /// A 4 kHz toneburst of `tb` seconds (whole samples) after `pre` seconds of silence,
    /// followed by `post` seconds of silence.
    fn burst(fs: f64, amp: f64, pre: f64, tb: f64, post: f64) -> Vec<f64> {
        let (npre, nb, npost) = (
            (pre * fs).round() as usize,
            (tb * fs).round() as usize,
            (post * fs).round() as usize,
        );
        let mut x = vec![0.0; npre + nb + npost];
        for k in 0..nb {
            x[npre + k] = amp * (TAU * 4000.0 * k as f64 / fs).sin();
        }
        x
    }

    #[test]
    fn detector_toneburst_matches_golden() {
        let g = GoldenSet::load("spl_toneburst").expect("golden");
        let fs = g.parameter("fs_hz").and_then(|v| v.as_f64()).expect("fs");
        let amp = 0.5;
        for (tw, tbs, key) in [
            (TimeWeighting::Fast, "tb_f_ms", "response_f_db"),
            (TimeWeighting::Slow, "tb_s_ms", "response_s_db"),
        ] {
            let tb = g.f64(tbs).expect("tb");
            let resp: Vec<f64> = tb
                .iter()
                .map(|&ms| {
                    let x = burst(fs, amp, 0.01, ms * 1e-3, 5.0 * tw.rise_s());
                    let mut d = TimeWeightedDetector::new(tw, fs);
                    let max = x.iter().fold(0.0f64, |m, &v| m.max(d.push(v)));
                    10.0 * (max / (amp * amp / 2.0)).log10()
                })
                .collect();
            g.assert_f64(key, &resp);
        }
    }

    /// F, S and I against the analytic detector of `tools/refgen` (`spl_time_weighting`):
    /// the level after a step of the mean square (10·lg(1 − e^(−t/τ)), τ = 125 ms, 1 s,
    /// 35 ms), its fall after the signal stops (−4.343·t/τ dB, τ = 125 ms, 1 s, 1.5 s) and the
    /// I-weighted maximum of 4 kHz tonebursts.
    #[test]
    fn time_weighting_matches_golden() {
        let g = GoldenSet::load("spl_time_weighting").expect("golden");
        let fs = g.parameter("fs_hz").and_then(|v| v.as_f64()).expect("fs");
        let tws = [
            ("f", TimeWeighting::Fast),
            ("s", TimeWeighting::Slow),
            ("i", TimeWeighting::Impulse),
        ];
        let step_ms = g.f64("step_ms").expect("step_ms");
        let decay_ms = g.f64("decay_ms").expect("decay_ms");
        // Feeds `x` until sample `upto` (exclusive) and returns the mean square there.
        let run = |d: &mut TimeWeightedDetector, n: &mut usize, upto: usize, x: f64| {
            let mut ms = d.mean_square();
            while *n < upto {
                ms = d.push(x);
                *n += 1;
            }
            ms
        };
        for (name, tw) in tws {
            // Step: x = 1 from sample 0, read at the last sample of the first t seconds.
            let mut d = TimeWeightedDetector::new(tw, fs);
            let mut n = 0usize;
            let levels: Vec<f64> = step_ms
                .iter()
                .map(|&t| 10.0 * run(&mut d, &mut n, (t * 1e-3 * fs).round() as usize, 1.0).log10())
                .collect();
            g.assert_f64(&format!("step_{name}_db"), &levels);
            // Fall: 10 s of x = 1, then silence.
            let mut d = TimeWeightedDetector::new(tw, fs);
            let mut n = 0usize;
            let at_stop = run(&mut d, &mut n, (10.0 * fs) as usize, 1.0);
            let mut n = 0usize;
            let levels: Vec<f64> = decay_ms
                .iter()
                .map(|&t| {
                    let ms = run(&mut d, &mut n, (t * 1e-3 * fs).round() as usize, 0.0);
                    10.0 * (ms / at_stop).log10()
                })
                .collect();
            g.assert_f64(&format!("decay_{name}_db"), &levels);
            // The analytic fall the golden values follow, as a sanity bound.
            for (&t, l) in decay_ms.iter().zip(&levels) {
                let want = -10.0 * std::f64::consts::E.log10() * t * 1e-3 / tw.fall_s();
                assert!(
                    (l - want).abs() < 0.01,
                    "{tw:?} fall at {t} ms: {l} vs {want}"
                );
            }
        }
        let tb = g.f64("tb_i_ms").expect("tb");
        let amp = 0.5;
        let resp: Vec<f64> = tb
            .iter()
            .map(|&ms| {
                let x = burst(fs, amp, 0.01, ms * 1e-3, 0.2);
                let mut d = TimeWeightedDetector::new(TimeWeighting::Impulse, fs);
                let max = x.iter().fold(0.0f64, |m, &v| m.max(d.push(v)));
                10.0 * (max / (amp * amp / 2.0)).log10()
            })
            .collect();
        g.assert_f64("response_i_db", &resp);
    }

    /// IEC 60651 Impulse single-burst responses of an A-weighted 4 kHz tone (−3.6, −8.8 and
    /// −12.6 dB at 20, 5 and 2 ms, type 1 tolerances ±1.5, ±2 and −4/+2 dB) at every rate, and
    /// the held value: 1 s after a 5 ms burst LAI has fallen 2.9 dB, LAF some 35 dB.
    #[test]
    fn impulse_bursts_per_rate() {
        let amp = 0.5;
        for fs in RATES {
            let mut m = meter(fs, Weighting::A, TimeWeighting::Impulse, PeakWeighting::Z);
            m.process(&sine(4000.0, amp, fs, fs as usize));
            m.reset_interval();
            m.process(&sine(4000.0, amp, fs, fs as usize));
            let steady = m.levels().leq;
            for (tb_ms, dref, minus, plus) in [
                (20.0, -3.6, 1.5, 1.5),
                (5.0, -8.8, 2.0, 2.0),
                (2.0, -12.6, 4.0, 2.0),
            ] {
                let x = burst(fs, amp, 0.01, tb_ms * 1e-3, 0.5);
                let mut m = meter(fs, Weighting::A, TimeWeighting::Impulse, PeakWeighting::Z);
                m.process(&x);
                let d = m.levels().lmax - steady - dref;
                assert!(d >= -minus && d <= plus, "{fs} Tb={tb_ms}: {d:.3} dB");
                assert!(
                    d.abs() < 0.15,
                    "{fs} Tb={tb_ms}: {d:.3} dB off the analytic"
                );
            }
            let x = burst(fs, amp, 0.01, 0.005, 1.0);
            let mut m = meter(fs, Weighting::A, TimeWeighting::Impulse, PeakWeighting::Z);
            m.process(&x);
            let l = m.levels();
            let fast = m.levels_of(Weighting::A, TimeWeighting::Fast, PeakWeighting::Z);
            assert!(
                (l.lmax - l.level - 2.9).abs() < 0.1,
                "{fs}: {}",
                l.lmax - l.level
            );
            assert!(
                fast.lmax - fast.level > 30.0,
                "{fs}: F {}",
                fast.lmax - fast.level
            );
        }
    }

    /// A meter switched to other weightings mid-run reads exactly what a meter built with
    /// them from the start reads: level, Lmax, Lmin, Leq and Lpeak over the same interval.
    #[test]
    fn select_reads_every_combination_of_the_same_interval() {
        let fs = 48_000.0;
        let mut x = sine(1000.0, 0.5, fs, (fs * 3.0) as usize);
        x.extend(sine(63.0, 0.05, fs, (fs * 4.0) as usize));
        x.extend(burst(fs, 0.3, 0.0, 0.2, 0.5));
        let (a, b) = x.split_at(x.len() / 2);
        let mut switched = meter(fs, Weighting::A, TimeWeighting::Fast, PeakWeighting::C);
        switched.process(a);
        let bits =
            |v: Levels| [v.level, v.lmax, v.lmin, v.leq, v.lpeak, v.duration_s].map(f64::to_bits);
        for w in [Weighting::A, Weighting::C, Weighting::Z] {
            for tw in [
                TimeWeighting::Fast,
                TimeWeighting::Slow,
                TimeWeighting::Impulse,
            ] {
                for pw in [PeakWeighting::C, PeakWeighting::Z] {
                    let mut s = switched.clone();
                    s.select(w, tw, pw);
                    s.process(b);
                    let mut fresh = meter(fs, w, tw, pw);
                    fresh.process(&x);
                    let (l, f) = (s.levels(), fresh.levels());
                    assert_eq!(bits(l), bits(f), "{w:?} {tw:?} {pw:?}");
                    assert_eq!(s.config().weighting, w);
                    assert_eq!(s.leq().level_dbfs().to_bits(), f.leq.to_bits());
                }
            }
        }
    }

    /// IEC 61672-1 Table 4: A-weighted 4 kHz toneburst responses for F and S (maximum
    /// time-weighted level) and sound exposure level, within class 1 limits, per rate.
    #[test]
    fn toneburst_table4_class1_per_rate() {
        // (Tb ms, δref F, class 1 −, +), (… S …), (… LE …)
        let f_rows = [
            (1000.0, 0.0, 0.5, 0.5),
            (500.0, -0.1, 0.5, 0.5),
            (200.0, -1.0, 0.5, 0.5),
            (100.0, -2.6, 1.0, 1.0),
            (50.0, -4.8, 1.0, 1.0),
            (20.0, -8.3, 1.0, 1.0),
            (10.0, -11.1, 1.0, 1.0),
            (5.0, -14.1, 1.0, 1.0),
            (2.0, -18.0, 1.5, 1.0),
            (1.0, -21.0, 2.0, 1.0),
            (0.5, -24.0, 2.5, 1.0),
            (0.25, -27.0, 3.0, 1.0),
        ];
        let s_rows = [
            (1000.0, -2.0, 0.5, 0.5),
            (500.0, -4.1, 0.5, 0.5),
            (200.0, -7.4, 0.5, 0.5),
            (100.0, -10.2, 1.0, 1.0),
            (50.0, -13.1, 1.0, 1.0),
            (20.0, -17.0, 1.5, 1.0),
            (10.0, -20.0, 2.0, 1.0),
            (5.0, -23.0, 2.5, 1.0),
            (2.0, -27.0, 3.0, 1.0),
        ];
        let amp = 0.5;
        for fs in RATES {
            // Steady A-weighted level of the 4 kHz tone (the reference for every δ).
            let mut m = meter(fs, Weighting::A, TimeWeighting::Fast, PeakWeighting::Z);
            m.process(&sine(4000.0, amp, fs, fs as usize));
            m.reset_interval();
            m.process(&sine(4000.0, amp, fs, fs as usize));
            let l_steady = m.levels().leq;
            let mut worst: f64 = 0.0;
            for (tw, rows) in [
                (TimeWeighting::Fast, &f_rows[..]),
                (TimeWeighting::Slow, &s_rows[..]),
            ] {
                for &(tb_ms, dref, minus, plus) in rows {
                    let x = burst(fs, amp, 0.01, tb_ms * 1e-3, 5.0 * tw.rise_s());
                    let mut m = meter(fs, Weighting::A, tw, PeakWeighting::Z);
                    m.process(&x);
                    let lv = m.levels();
                    let d = lv.lmax - l_steady - dref;
                    assert!(
                        d >= -minus && d <= plus,
                        "{fs} {tw:?} Tb={tb_ms} ms: deviation {d:.3} dB"
                    );
                    // Deviation from Formula 7 itself is far tighter than the class limits.
                    let eq7 = 10.0 * (1.0 - (-tb_ms * 1e-3 / tw.rise_s()).exp()).log10();
                    worst = worst.max((lv.lmax - l_steady - eq7).abs());
                    if tw == TimeWeighting::Fast {
                        // LAE − LA = 10 lg(Tb / 1 s), class 1 limits as for F.
                        let de = lv.leq + 10.0 * lv.duration_s.log10() - l_steady;
                        let eref = 10.0 * (tb_ms * 1e-3f64).log10();
                        let dev = de - eref;
                        assert!(
                            dev >= -minus && dev <= plus,
                            "{fs} LE Tb={tb_ms} ms: deviation {dev:.3} dB"
                        );
                        let le = m.leq().exposure_level_dbfs();
                        assert!((le - (lv.leq + 10.0 * lv.duration_s.log10())).abs() < 1e-9);
                    }
                }
            }
            eprintln!("{fs} Hz: max |toneburst response − Formula 7| = {worst:.3} dB");
            assert!(worst < 0.2, "{fs}: {worst}");
        }
    }

    /// Decay after a steady 4 kHz tone stops: 34.7 dB/s (F, +3.8/−3.7), 4.3 dB/s
    /// (S, +0.8/−0.7) per IEC 61672-1 5.8; Impulse falls with 1.5 s → 2.9 dB/s.
    #[test]
    fn decay_rates() {
        let fs = 48_000.0;
        for (tw, rate, minus, plus) in [
            (TimeWeighting::Fast, 34.7, 3.7, 3.8),
            (TimeWeighting::Slow, 4.3, 0.7, 0.8),
            (TimeWeighting::Impulse, 2.895, 0.01, 0.01),
        ] {
            let mut m = meter(fs, Weighting::A, tw, PeakWeighting::Z);
            m.process(&sine(4000.0, 0.5, fs, (fs * 8.0) as usize));
            let mut silence = vec![0.0; (fs * 0.2) as usize];
            m.process(&silence);
            let l0 = m.levels().level;
            silence.resize((fs * 0.5) as usize, 0.0);
            m.process(&silence);
            let l1 = m.levels().level;
            let measured = (l0 - l1) / 0.5;
            assert!(
                measured >= rate - minus && measured <= rate + plus,
                "{tw:?}: {measured} dB/s"
            );
        }
    }

    /// Steady 1 kHz sine: F, S, Impulse and Leq agree within ±0.1 dB (IEC 61672-1 5.8.3) and
    /// read the sine's dBFS level (A(1 kHz) = 0 dB).
    #[test]
    fn steady_state_accuracy() {
        for fs in RATES {
            let amp = 0.5;
            let expect = 20.0 * f64::log10(amp);
            for tw in [
                TimeWeighting::Fast,
                TimeWeighting::Slow,
                TimeWeighting::Impulse,
            ] {
                let mut m = meter(fs, Weighting::A, tw, PeakWeighting::C);
                m.process(&sine(1000.0, amp, fs, (fs * 8.0) as usize));
                m.reset_interval();
                m.process(&sine(1000.0, amp, fs, (fs * 2.0) as usize));
                let lv = m.levels();
                assert!(
                    (lv.level - expect).abs() < 0.01,
                    "{fs} {tw:?} L {}",
                    lv.level
                );
                assert!((lv.leq - expect).abs() < 0.001, "{fs} Leq {}", lv.leq);
                assert!((lv.level - lv.leq).abs() < 0.1);
                // Sine peak is 3.01 dB above its RMS level on this scale. Lpeak is a sample
                // peak, so at 1 kHz it may miss the crest by up to half a sample period
                // (cos(π·1000/44100) → −0.022 dB).
                assert!(
                    (lv.lpeak - lv.leq - 3.0103).abs() < 0.025,
                    "{}",
                    lv.lpeak - lv.leq
                );
                assert!(lv.lmax >= lv.level && lv.lmin <= lv.level);
                // Fast ripple at 2 kHz is ~α/(2 sin(π·2f/fs)) relative: well under 0.01 dB.
                assert!(lv.lmax - lv.lmin < 0.01 || tw == TimeWeighting::Impulse);
            }
        }
    }

    /// Impulse: 35 ms rise and 1.5 s fall from a step in mean square (the 35 ms average has
    /// long decayed when the 1.5 s fall is read).
    #[test]
    fn impulse_asymmetric_time_constants() {
        let fs = 48_000.0;
        let mut d = TimeWeightedDetector::new(TimeWeighting::Impulse, fs);
        let n_rise = (0.035 * fs) as usize;
        let mut ms = 0.0;
        for _ in 0..n_rise {
            ms = d.push(1.0);
        }
        assert!((ms - (1.0 - (-1.0f64).exp())).abs() < 1e-3, "{ms}");
        for _ in 0..(1.5 * fs) as usize {
            ms = d.push(0.0);
        }
        let expect = (1.0 - (-1.0f64).exp()) * (-1.0f64).exp();
        assert!((ms - expect).abs() < 1e-3, "{ms} vs {expect}");
    }

    /// Leq of a known signal: two equal-length segments at different levels, and exactness
    /// of the f64 accumulation over 2²⁵ samples (≈ 11.7 min at 48 kHz).
    #[test]
    fn leq_known_signals() {
        let fs = 48_000.0;
        let mut m = meter(fs, Weighting::Z, TimeWeighting::Fast, PeakWeighting::Z);
        let a1 = 0.5;
        let a2 = 0.05;
        m.process(&sine(1000.0, a1, fs, 48_000));
        m.process(&sine(1000.0, a2, fs, 48_000));
        let expect = power_dbfs((a1 * a1 / 2.0 + a2 * a2 / 2.0) / 2.0);
        assert!((m.levels().leq - expect).abs() < 1e-6, "{}", m.levels().leq);
        assert!((m.levels().duration_s - 2.0).abs() < 1e-12);

        let mut leq = Leq::new(fs);
        let block = vec![1e-3; 1 << 15];
        for _ in 0..1 << 10 {
            leq.push(&block);
        }
        assert_eq!(leq.samples(), 1 << 25);
        assert!(
            (leq.mean_square() / 1e-6 - 1.0).abs() < 1e-13,
            "{}",
            leq.mean_square()
        );
        let mut by_sample = Leq::new(fs);
        for &v in &block {
            by_sample.push_sample(v);
        }
        assert!((by_sample.mean_square() / 1e-6 - 1.0).abs() < 1e-13);
        assert!(Leq::new(fs).mean_square().is_nan());
    }

    /// IEC 61672-1 Table 5: LCpeak − LC for one-cycle and half-cycle signals, class 1, per
    /// rate. Frequencies are the exact ones (Annex D): 10^1.5, 10^2.7, 10^3.9 Hz.
    #[test]
    fn c_peak_table5_class1_per_rate() {
        let cases = [
            // (frequency, cycles: 1.0 full, 0.5 positive half, -0.5 negative half, ref, tol)
            (10f64.powf(1.5), 1.0f64, 2.5, 2.0),
            (10f64.powf(2.7), 1.0, 3.5, 1.0),
            (10f64.powf(3.9), 1.0, 3.4, 2.0),
            (10f64.powf(2.7), 0.5, 2.4, 1.0),
            (10f64.powf(2.7), -0.5, 2.4, 1.0),
        ];
        let amp = 0.25;
        for fs in RATES {
            for (f, cycles, dref, tol) in cases {
                let sign = if cycles < 0.0 { -1.0 } else { 1.0 };
                let n = (cycles.abs() / f * fs).round() as usize;
                let mut x = vec![0.0; (0.05 * fs) as usize];
                x.extend((0..n).map(|i| sign * amp * (TAU * f * i as f64 / fs).sin()));
                x.extend(std::iter::repeat_n(0.0, (2.0 * fs) as usize));
                let mut m = meter(fs, Weighting::C, TimeWeighting::Fast, PeakWeighting::C);
                m.process(&x);
                let lcpeak = m.levels().lpeak;
                // LC of the steady sine, measured.
                let mut s = meter(fs, Weighting::C, TimeWeighting::Fast, PeakWeighting::C);
                s.process(&sine(f, amp, fs, (fs * 2.0) as usize));
                s.reset_interval();
                s.process(&sine(f, amp, fs, (fs * 2.0) as usize));
                let lc = s.levels().leq;
                let dev = lcpeak - lc - dref;
                assert!(
                    dev.abs() <= tol,
                    "{fs} f={f:.1} cycles={cycles}: LCpeak-LC = {:.2} (ref {dref})",
                    lcpeak - lc
                );
                eprintln!(
                    "{fs} Hz f={f:.1} cycles={cycles:+}: LCpeak-LC = {:.2} dB (ref {dref})",
                    lcpeak - lc
                );
            }
        }
    }

    #[test]
    fn sensitivity_and_calibrated_levels() {
        let s = Sensitivity::from_calibrator(94.0, -26.0);
        assert_eq!(s.offset_db, 120.0);
        assert_eq!(s.spl(-26.0), 94.0);
        // Calibrate on a 1 kHz tone, then read the same tone in dB SPL.
        let fs = 48_000.0;
        let mut m = meter(fs, Weighting::A, TimeWeighting::Slow, PeakWeighting::C);
        let amp = 10f64.powf(-26.0 / 20.0);
        m.process(&sine(1000.0, amp, fs, (fs * 8.0) as usize));
        m.reset_interval();
        m.process(&sine(1000.0, amp, fs, fs as usize));
        let cal = Sensitivity::from_calibrator(94.0, m.levels().leq);
        let spl = m.levels().calibrated(cal);
        assert_eq!(spl.scale, LevelScale::DbSpl);
        assert!((spl.leq - 94.0).abs() < 1e-12);
        assert!((spl.level - 94.0).abs() < 0.01);
    }

    /// A smooth mic model: 2nd-order Butterworth roll-off at 30 Hz and a first-order
    /// shelf rising towards +6 dB above 8 kHz, sampled at 1/6-octave points.
    fn mic_model_db(f: f64) -> f64 {
        let hp = 10.0 * (f.powi(4) / (f.powi(4) + 30f64.powi(4))).log10();
        let shelf =
            10.0 * ((1.0 + (f / 4000.0).powi(2) * 4.0) / (1.0 + (f / 4000.0).powi(2))).log10();
        hp + shelf
    }

    fn mic_curve() -> crate::mic_curve::Correction {
        use crate::mic_curve::MicCurve;
        let pts: Vec<(f64, f64)> = (-36..=26)
            .map(|k| 1000.0 * 2f64.powf(f64::from(k) / 6.0))
            .map(|f| (f, mic_model_db(f)))
            .collect();
        MicCurve::from_points(&pts)
            .expect("curve")
            .normalised(1000.0)
    }

    fn mic_correction(fs: f64) -> Vec<f64> {
        mic_curve().design_fir(fs)
    }

    /// The calibrator frequency reads the same with and without the curve (nothing counts
    /// twice); elsewhere a tone reads its level minus the curve; Lpeak is untouched.
    #[test]
    fn mic_correction_levels() {
        for fs in RATES {
            let h = mic_correction(fs);
            let c = mic_curve();
            for f in [1000.0, 8000.0, 31.5, 63.0, 12_500.0] {
                let curve_db = c.db(f);
                let amp = 0.25;
                // One continuous tone: a phase jump between the intervals would ring the LF boost.
                let x = sine(f, amp, fs, (fs * 6.0) as usize);
                let (x1, x2) = x.split_at(x.len() / 2);
                let mut plain = meter(fs, Weighting::Z, TimeWeighting::Slow, PeakWeighting::Z);
                let mut corr = meter(fs, Weighting::Z, TimeWeighting::Slow, PeakWeighting::Z);
                corr.set_correction(Some(&h));
                assert!(corr.has_correction() && !plain.has_correction());
                for m in [&mut plain, &mut corr] {
                    m.process(x1);
                    m.reset_interval();
                    m.process(x2);
                }
                let (p, cl) = (plain.levels(), corr.levels());
                let err = cl.leq - (p.leq - curve_db);
                eprintln!("{fs} Hz, tone {f} Hz: corrected − expected = {err:+.4} dB");
                assert!(err.abs() < 0.02, "{fs} {f}: {err}");
                assert_eq!(cl.lpeak, p.lpeak, "Lpeak must stay on the raw path");
            }
        }
    }

    /// Toneburst responses relative to the steady level are unchanged by a correction in
    /// the path (minimum phase: no pre-ringing, the filter's energy sits at its start).
    #[test]
    fn mic_correction_keeps_toneburst_response() {
        let fs = 48_000.0;
        let h = mic_correction(fs);
        let amp = 0.5;
        let steady = |corrected: bool| {
            let mut m = meter(fs, Weighting::A, TimeWeighting::Fast, PeakWeighting::Z);
            if corrected {
                m.set_correction(Some(&h));
            }
            m.process(&sine(4000.0, amp, fs, fs as usize));
            m.reset_interval();
            m.process(&sine(4000.0, amp, fs, fs as usize));
            m.levels().leq
        };
        let (s_corr, s_plain) = (steady(true), steady(false));
        for tw in [
            TimeWeighting::Fast,
            TimeWeighting::Slow,
            TimeWeighting::Impulse,
        ] {
            for tb_ms in [1000.0, 200.0, 50.0, 10.0, 2.0] {
                let x = burst(fs, amp, 0.01, tb_ms * 1e-3, 5.0 * tw.fall_s());
                let mut m = meter(fs, Weighting::A, tw, PeakWeighting::Z);
                m.set_correction(Some(&h));
                m.process(&x);
                let mut r = meter(fs, Weighting::A, tw, PeakWeighting::Z);
                r.process(&x);
                let d = (m.levels().lmax - s_corr) - (r.levels().lmax - s_plain);
                // A 2 ms burst spreads ±500 Hz around 4 kHz, where the curve slopes, so it
                // is corrected slightly differently from the steady tone; the class 1
                // tolerance there is +1/−1.5 dB.
                let tol = if tb_ms < 5.0 { 0.1 } else { 0.02 };
                assert!(d.abs() < tol, "{tw:?} Tb={tb_ms}: {d}");
            }
        }
    }

    #[test]
    fn lmax_lmin_track_level_changes() {
        let fs = 48_000.0;
        let mut m = meter(fs, Weighting::Z, TimeWeighting::Fast, PeakWeighting::Z);
        let n = (2.0 * fs) as usize;
        m.process(&sine(1000.0, 0.5, fs, n));
        m.process(&sine(1000.0, 0.05, fs, n));
        m.process(&sine(1000.0, 0.5, fs, n));
        let lv = m.levels();
        assert!((lv.lmax - power_dbfs(0.125)).abs() < 0.01, "{}", lv.lmax);
        assert!((lv.lmin - power_dbfs(0.00125)).abs() < 0.05, "{}", lv.lmin);
        // Lmin ignores the detector's rise from silence after a full reset.
        m.reset();
        m.process(&sine(1000.0, 0.5, fs, n));
        assert!((m.levels().lmin - power_dbfs(0.125)).abs() < 0.05);
    }
}
