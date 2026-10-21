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
//!       │                                   ├── C ──┼── Leq (f64 energy)
//!       │                                   └── Z ──┴── per-second energy (the Leq log)
//!       └── C, Z (uncorrected) ── |x| max ── Lpeak
//! ```
//!
//! Every weighting combination runs at once; the meter reports the chosen one, so the
//! choice changes on a running meter without a restart or a settling detector. The
//! per-second energies of [`crate::leq`] come from the same weighted samples: one
//! correction and one A/C filter pair per input, and the log measures exactly the signal
//! the meter shows.
//!
//! The mic-curve correction is a minimum-phase FIR ([`crate::mic_curve`]) normalised to
//! 0 dB at the calibrator frequency. Lpeak stays on the uncorrected samples (§5.3): the
//! corrected path lags by one convolution partition and carries the curve's HF boost,
//! which would change a crest-factor reading without a standard tolerance to judge it by.

use crate::leq::{Second, SecondIntegrator};
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
        self.push_square(x * x)
    }

    /// Feeds one squared sample `x²`; returns the new time-weighted mean square.
    #[inline]
    pub fn push_square(&mut self, sq: f64) -> f64 {
        self.avg += self.alpha_avg * (sq - self.avg);
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

/// Samples squared and summed plainly before the sum joins a compensated total. A plain sum
/// of n non-negative terms is within (n − 1)·2⁻⁵³ of exact, so 256 terms stay within
/// 3·10⁻¹⁴ relative; the compensated sum over the chunks adds only O(2⁻⁵³ + k·2⁻¹⁰⁶) for k
/// chunks, so a 48 h interval at 96 kHz (k ≈ 6.5·10⁷) is still within 10⁻¹³ (4·10⁻¹³ dB).
const CHUNK: usize = 256;

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
    #[cfg(test)]
    pub(crate) fn push_sample(&mut self, x: f64) {
        self.add(x * x);
        self.samples += 1;
    }

    /// Adds a block of samples.
    pub fn push(&mut self, block: &[f64]) {
        for chunk in block.chunks(CHUNK) {
            let s: f64 = chunk.iter().map(|x| x * x).sum();
            self.add(s);
        }
        self.samples += block.len() as u64;
    }

    /// Adds `n` samples (at most [`CHUNK`]) whose Σx² is `energy`, summed plainly.
    #[inline]
    fn push_energy(&mut self, energy: f64, n: u64) {
        self.add(energy);
        self.samples += n;
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
    #[cfg(test)]
    pub(crate) fn exposure_level_dbfs(&self) -> f64 {
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
    #[cfg(test)]
    pub(crate) fn from_calibrator(calibrator_db_spl: f64, measured_dbfs: f64) -> Self {
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

/// Sound level meter on one input: time-weighted level with Lmax/Lmin, Leq and Lpeak, and
/// the per-second A/C/Z energies of the Leq log ([`crate::leq`]).
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
    /// C on the raw samples for the peak while a correction is in the path; without one
    /// the peak reads `weight_c`, which then filters the same samples.
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
    seconds: SecondIntegrator,
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
            seconds: SecondIntegrator::new(fs),
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

    /// Runs the time-weighted, Lmax/Lmin, Leq and per-second paths through `taps` (a
    /// mic-curve correction from [`crate::mic_curve::Correction::design_fir`]) before
    /// frequency weighting; `None` removes it. Filter state starts silent; the interval
    /// continues.
    pub fn set_correction(&mut self, taps: Option<&[f64]>) {
        let had = self.correction.is_some();
        let part = crate::mic_curve::fir_partition(self.cfg.fs);
        self.correction = taps.map(|h| PartitionedFir::new(h, part));
        // The raw-sample C filter moves between the peak and the weighted path with its
        // state, so the peak path never restarts.
        match (had, self.correction.is_some()) {
            (false, true) => self.peak_c = self.weight_c.clone(),
            (true, false) => self.weight_c = self.peak_c.clone(),
            _ => {}
        }
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

    /// The per-second integration: where the current second stands.
    pub fn seconds(&self) -> &SecondIntegrator {
        &self.seconds
    }

    /// `samples` were lost (a capture discontinuity): the second grid moves on without
    /// energy or measured time ([`SecondIntegrator::skip`]); the filters continue as if
    /// the input were contiguous.
    pub fn skip(&mut self, samples: u64, emit: impl FnMut(Second)) {
        self.seconds.skip(samples, emit);
    }

    /// Processes raw input samples (FS); every completed second of A/C/Z energy goes to
    /// `emit`. Does not allocate.
    ///
    /// The mic-curve correction (PLAN.md §5.7), when set, goes on `x` immediately before
    /// the frequency weightings — so it feeds the time-weighted, Lmax/Lmin, Leq and
    /// per-second paths — and never before the peak path: LCpeak stays on the uncorrected
    /// samples (§5.3).
    pub fn process(&mut self, block: &[f64], emit: impl FnMut(Second)) {
        self.process_tapped(block, emit, |_| {});
    }

    /// [`Self::process`], handing `tap` the mic-curve-corrected, unweighted samples in
    /// order (the block itself without a correction): the signal the A/C/Z seconds are
    /// integrated from, for analyses that must measure exactly what the log measures (the
    /// 1/3-octave band Leq). Does not allocate.
    pub fn process_tapped(
        &mut self,
        block: &[f64],
        mut emit: impl FnMut(Second),
        mut tap: impl FnMut(&[f64]),
    ) {
        let Some(mut fir) = self.correction.take() else {
            tap(block);
            self.run::<false>(block, block, &mut emit);
            return;
        };
        let mut corrected = std::mem::take(&mut self.corrected);
        for chunk in block.chunks(corrected.len().max(1)) {
            let c = &mut corrected[..chunk.len()];
            fir.process(chunk, c);
            tap(c);
            self.run::<true>(chunk, c, &mut emit);
        }
        self.corrected = corrected;
        self.correction = Some(fir);
    }

    /// `raw` and its corrected samples `xc` (the same slice without a correction), in runs
    /// of at most [`CHUNK`] within one second and one Lmin settling state.
    fn run<const CORR: bool>(&mut self, raw: &[f64], xc: &[f64], emit: &mut impl FnMut(Second)) {
        let mut i = 0;
        while i < raw.len() {
            let mut n = (raw.len() - i)
                .min(CHUNK)
                .min(usize::try_from(self.seconds.room()).unwrap_or(usize::MAX));
            let (r, c) = (&raw[i..], &xc[i..]);
            if let Some(s) = self.settle_left.iter().copied().filter(|&s| s > 0).min() {
                n = n.min(usize::try_from(s).unwrap_or(usize::MAX));
            }
            let (energy, max) = self.detect::<CORR>(&r[..n], &c[..n]);
            for (l, &ew) in self.leq.iter_mut().zip(&energy) {
                l.push_energy(ew, n as u64);
            }
            for s in &mut self.settle_left {
                *s = s.saturating_sub(n as u64);
            }
            self.seconds.add(energy, max, n as u64, emit);
            i += n;
        }
    }

    /// Filters and detects a run within one Lmin settling state; returns its Σy² per
    /// weighting and its highest C peak² and LAF mean square.
    fn detect<const CORR: bool>(&mut self, raw: &[f64], xc: &[f64]) -> ([f64; 3], [f64; 2]) {
        let settled = self.settle_left.map(|n| n == 0);
        let mut e = [0.0; 3];
        let mut max = [0.0f64; 2];
        let mut pk = [self.peak[0].peak, self.peak[1].peak];
        for (&x, &c) in raw.iter().zip(xc) {
            let ya = self.weight_a.process_sample(c);
            let yc = self.weight_c.process_sample(c);
            let pc = if CORR {
                self.peak_c.process_sample(x)
            } else {
                yc
            };
            pk[0] = pk[0].max(pc.abs());
            pk[1] = pk[1].max(x.abs());
            max[0] = max[0].max(pc * pc);
            let sq = [ya * ya, yc * yc, c * c];
            for (w, &s) in sq.iter().enumerate() {
                e[w] += s;
                let (dets, maxs, mins) = (
                    &mut self.detectors[w],
                    &mut self.max_ms[w],
                    &mut self.min_ms[w],
                );
                for (((d, max), min), &ok) in dets.iter_mut().zip(maxs).zip(mins).zip(&settled) {
                    let ms = d.push_square(s);
                    *max = max.max(ms);
                    if ok {
                        *min = min.min(ms);
                    }
                }
            }
            // The log's LAFmax is the displayed LAF detector's: both see every sample.
            let laf = &self.detectors[w_index(Weighting::A)][t_index(TimeWeighting::Fast)];
            max[1] = max[1].max(laf.mean_square());
        }
        self.peak[0].peak = pk[0];
        self.peak[1].peak = pk[1];
        (e, max)
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
mod tests;
