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
    pub fn push_sample(&mut self, x: f64) {
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

/// Sound level meter on one input: time-weighted level with Lmax/Lmin, Leq and Lpeak, and
/// the per-second A/C/Z energies of the Leq log ([`crate::leq`]).
///
/// Every frequency weighting (A, C, Z) runs with every time weighting (F, S, I), and the
/// peak with C and Z, all the time; [`SplMeter::select`] only chooses which are reported. A
/// change of weighting therefore reads a settled detector at once (a Slow detector started
/// at the switch would take 5 s to settle), and Lmax, Lmin, Leq and Lpeak of the newly
/// chosen weighting cover the same interval as before the change.
///
/// [Freezing](SplMeter::set_frozen) holds the displayed values only: the correction and
/// weighting filters and the per-second integration go on, so the log has no hole and the
/// detectors resume from a filter that never stopped.
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
    frozen: bool,
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
            frozen: false,
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

    /// Holds (`true`) or releases the detectors, Lmax/Lmin, Leq and Lpeak; the filters and
    /// the per-second integration keep running.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// Whether the displayed values are held.
    pub fn frozen(&self) -> bool {
        self.frozen
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
    pub fn process(&mut self, block: &[f64], mut emit: impl FnMut(Second)) {
        let Some(mut fir) = self.correction.take() else {
            self.run::<false>(block, block, &mut emit);
            return;
        };
        let mut corrected = std::mem::take(&mut self.corrected);
        for chunk in block.chunks(corrected.len().max(1)) {
            let c = &mut corrected[..chunk.len()];
            fir.process(chunk, c);
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
            let energy = if self.frozen {
                self.weigh::<CORR>(&r[..n], &c[..n])
            } else {
                if let Some(s) = self.settle_left.iter().copied().filter(|&s| s > 0).min() {
                    n = n.min(usize::try_from(s).unwrap_or(usize::MAX));
                }
                let e = self.detect::<CORR>(&r[..n], &c[..n]);
                for (l, &ew) in self.leq.iter_mut().zip(&e) {
                    l.push_energy(ew, n as u64);
                }
                for s in &mut self.settle_left {
                    *s = s.saturating_sub(n as u64);
                }
                e
            };
            self.seconds.add(energy, n as u64, emit);
            i += n;
        }
    }

    /// Filters a run without detecting (frozen); returns its Σy² per weighting.
    fn weigh<const CORR: bool>(&mut self, raw: &[f64], xc: &[f64]) -> [f64; 3] {
        let mut e = [0.0; 3];
        for (&x, &c) in raw.iter().zip(xc) {
            let ya = self.weight_a.process_sample(c);
            let yc = self.weight_c.process_sample(c);
            if CORR {
                self.peak_c.process_sample(x);
            }
            e[0] += ya * ya;
            e[1] += yc * yc;
            e[2] += c * c;
        }
        e
    }

    /// Filters and detects a run within one Lmin settling state; returns its Σy² per
    /// weighting.
    fn detect<const CORR: bool>(&mut self, raw: &[f64], xc: &[f64]) -> [f64; 3] {
        let settled = self.settle_left.map(|n| n == 0);
        let mut e = [0.0; 3];
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
        }
        self.peak[0].peak = pk[0];
        self.peak[1].peak = pk[1];
        e
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
            m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
            m.reset_interval();
            m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
            let steady = m.levels().leq;
            for (tb_ms, dref, minus, plus) in [
                (20.0, -3.6, 1.5, 1.5),
                (5.0, -8.8, 2.0, 2.0),
                (2.0, -12.6, 4.0, 2.0),
            ] {
                let x = burst(fs, amp, 0.01, tb_ms * 1e-3, 0.5);
                let mut m = meter(fs, Weighting::A, TimeWeighting::Impulse, PeakWeighting::Z);
                m.process(&x, |_| {});
                let d = m.levels().lmax - steady - dref;
                assert!(d >= -minus && d <= plus, "{fs} Tb={tb_ms}: {d:.3} dB");
                assert!(
                    d.abs() < 0.15,
                    "{fs} Tb={tb_ms}: {d:.3} dB off the analytic"
                );
            }
            let x = burst(fs, amp, 0.01, 0.005, 1.0);
            let mut m = meter(fs, Weighting::A, TimeWeighting::Impulse, PeakWeighting::Z);
            m.process(&x, |_| {});
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
        switched.process(a, |_| {});
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
                    s.process(b, |_| {});
                    let mut fresh = meter(fs, w, tw, pw);
                    fresh.process(&x, |_| {});
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
            m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
            m.reset_interval();
            m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
            let l_steady = m.levels().leq;
            let mut worst: f64 = 0.0;
            for (tw, rows) in [
                (TimeWeighting::Fast, &f_rows[..]),
                (TimeWeighting::Slow, &s_rows[..]),
            ] {
                for &(tb_ms, dref, minus, plus) in rows {
                    let x = burst(fs, amp, 0.01, tb_ms * 1e-3, 5.0 * tw.rise_s());
                    let mut m = meter(fs, Weighting::A, tw, PeakWeighting::Z);
                    m.process(&x, |_| {});
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
            m.process(&sine(4000.0, 0.5, fs, (fs * 8.0) as usize), |_| {});
            let mut silence = vec![0.0; (fs * 0.2) as usize];
            m.process(&silence, |_| {});
            let l0 = m.levels().level;
            silence.resize((fs * 0.5) as usize, 0.0);
            m.process(&silence, |_| {});
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
                m.process(&sine(1000.0, amp, fs, (fs * 8.0) as usize), |_| {});
                m.reset_interval();
                m.process(&sine(1000.0, amp, fs, (fs * 2.0) as usize), |_| {});
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
        m.process(&sine(1000.0, a1, fs, 48_000), |_| {});
        m.process(&sine(1000.0, a2, fs, 48_000), |_| {});
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
                m.process(&x, |_| {});
                let lcpeak = m.levels().lpeak;
                // LC of the steady sine, measured.
                let mut s = meter(fs, Weighting::C, TimeWeighting::Fast, PeakWeighting::C);
                s.process(&sine(f, amp, fs, (fs * 2.0) as usize), |_| {});
                s.reset_interval();
                s.process(&sine(f, amp, fs, (fs * 2.0) as usize), |_| {});
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
        m.process(&sine(1000.0, amp, fs, (fs * 8.0) as usize), |_| {});
        m.reset_interval();
        m.process(&sine(1000.0, amp, fs, fs as usize), |_| {});
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
                    m.process(x1, |_| {});
                    m.reset_interval();
                    m.process(x2, |_| {});
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
            m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
            m.reset_interval();
            m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
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
                m.process(&x, |_| {});
                let mut r = meter(fs, Weighting::A, tw, PeakWeighting::Z);
                r.process(&x, |_| {});
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
        m.process(&sine(1000.0, 0.5, fs, n), |_| {});
        m.process(&sine(1000.0, 0.05, fs, n), |_| {});
        m.process(&sine(1000.0, 0.5, fs, n), |_| {});
        let lv = m.levels();
        assert!((lv.lmax - power_dbfs(0.125)).abs() < 0.01, "{}", lv.lmax);
        assert!((lv.lmin - power_dbfs(0.00125)).abs() < 0.05, "{}", lv.lmin);
        // Lmin ignores the detector's rise from silence after a full reset.
        m.reset();
        m.process(&sine(1000.0, 0.5, fs, n), |_| {});
        assert!((m.levels().lmin - power_dbfs(0.125)).abs() < 0.05);
    }

    /// Noise whose level steps between −10 and −50 dBFS every 0.7 s, so the detectors rise
    /// and fall and the sums mix very different magnitudes.
    fn stepped_noise(fs: f64, seconds: f64) -> Vec<f64> {
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let step = (0.7 * fs) as usize;
        (0..(seconds * fs) as usize)
            .map(|i| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                let u = (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
                let amp = if (i / step).is_multiple_of(2) {
                    0.6
                } else {
                    0.006
                };
                amp * u
            })
            .collect()
    }

    /// Neumaier sum: the reference totals below.
    #[derive(Default)]
    struct Exact {
        s: f64,
        c: f64,
    }

    impl Exact {
        fn add(&mut self, v: f64) {
            let t = self.s + v;
            if self.s.abs() >= v.abs() {
                self.c += (self.s - t) + v;
            } else {
                self.c += (v - t) + self.s;
            }
            self.s = t;
        }

        fn get(&self) -> f64 {
            self.s + self.c
        }
    }

    /// The meter's filters, detectors, Leq and per-second energies built one by one, every
    /// sum compensated per sample: what the shared chain must reproduce. `taps` (when
    /// given) is convolved in f64 over the whole signal and delayed by the partition, as
    /// the meter's correction.
    struct Reference {
        levels: [[Levels; 3]; 3],
        leq: [f64; 3],
        seconds: Vec<[f64; 3]>,
    }

    fn reference(x: &[f64], fs: f64, taps: Option<&[f64]>) -> Reference {
        let xc: Vec<f64> = match taps {
            None => x.to_vec(),
            Some(h) => {
                let lat = crate::mic_curve::fir_partition(fs);
                let full = convolve_f64(x, h);
                (0..x.len())
                    .map(|n| if n < lat { 0.0 } else { full[n - lat] })
                    .collect()
            }
        };
        let mut a = WeightingFilter::new(Weighting::A, fs).expect("A");
        let mut c = WeightingFilter::new(Weighting::C, fs).expect("C");
        let mut pc = WeightingFilter::new(Weighting::C, fs).expect("C");
        let mut det = [0; 3].map(|_| TIME_WEIGHTINGS.map(|t| TimeWeightedDetector::new(t, fs)));
        let lat = taps.map_or(0, |_| crate::mic_curve::fir_partition(fs) as u64);
        let settle = TIME_WEIGHTINGS.map(|t| settle_samples(t, fs) + lat);
        let mut max = [[0.0f64; 3]; 3];
        let mut min = [[f64::INFINITY; 3]; 3];
        let mut leq: [Exact; 3] = Default::default();
        let mut sec: [Exact; 3] = Default::default();
        let mut seconds = Vec::new();
        let mut pk_c = 0.0f64;
        let per = fs as usize;
        for (n, (&xr, &xv)) in x.iter().zip(&xc).enumerate() {
            let y = [a.process_sample(xv), c.process_sample(xv), xv];
            pk_c = pk_c.max(pc.process_sample(xr).abs());
            for w in 0..3 {
                leq[w].add(y[w] * y[w]);
                sec[w].add(y[w] * y[w]);
                for t in 0..3 {
                    let ms = det[w][t].push(y[w]);
                    max[w][t] = max[w][t].max(ms);
                    if n as u64 >= settle[t] {
                        min[w][t] = min[w][t].min(ms);
                    }
                }
            }
            if (n + 1) % per == 0 {
                seconds.push([0, 1, 2].map(|w| sec[w].get() / fs));
                sec = Default::default();
            }
        }
        let levels = [0, 1, 2].map(|w| {
            [0, 1, 2].map(|t| Levels {
                scale: LevelScale::Dbfs,
                level: det[w][t].level_dbfs(),
                lmax: power_dbfs(max[w][t]),
                lmin: power_dbfs(min[w][t]),
                leq: power_dbfs(leq[w].get() / x.len() as f64),
                lpeak: power_dbfs(pk_c * pk_c),
                duration_s: x.len() as f64 / fs,
            })
        });
        Reference {
            levels,
            leq: [0, 1, 2].map(|w| leq[w].get()),
            seconds,
        }
    }

    /// Linear convolution in f64 through one large FFT.
    fn convolve_f64(x: &[f64], h: &[f64]) -> Vec<f64> {
        let n = (x.len() + h.len()).next_power_of_two();
        let mut planner = realfft::RealFftPlanner::<f64>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let spec = |v: &[f64]| {
            let mut t = vec![0.0; n];
            t[..v.len()].copy_from_slice(v);
            let mut s = fwd.make_output_vec();
            fwd.process(&mut t, &mut s).expect("fft");
            s
        };
        let (sx, sh) = (spec(x), spec(h));
        let mut p: Vec<_> = sx.iter().zip(&sh).map(|(a, b)| a * b).collect();
        let last = p.len() - 1;
        p[0].im = 0.0;
        p[last].im = 0.0;
        let mut y = vec![0.0; n];
        inv.process(&mut p, &mut y).expect("ifft");
        y.iter().map(|v| v / n as f64).collect()
    }

    fn run_meter(x: &[f64], fs: f64, taps: Option<&[f64]>) -> (SplMeter, Vec<Second>) {
        let mut m = meter(fs, Weighting::A, TimeWeighting::Fast, PeakWeighting::C);
        m.set_correction(taps);
        let mut out = Vec::new();
        // Odd block sizes: chunk, partition and second boundaries all fall mid-block.
        let mut i = 0;
        for size in [256usize, 1000, 37, 4096, 511].iter().cycle() {
            if i >= x.len() {
                break;
            }
            let e = (i + size).min(x.len());
            m.process(&x[i..e], |s| out.push(s));
            i = e;
        }
        (m, out)
    }

    /// Without a correction the shared chain is the separate filters sample for sample:
    /// detectors, Lmax, Lmin and Lpeak are bit-identical to a reference built from
    /// independent filters, and Leq and the per-second energies (chunked sums) agree to
    /// 10⁻¹² relative.
    #[test]
    fn one_chain_matches_separate_paths() {
        for fs in [48_000.0, 96_000.0] {
            let x = stepped_noise(fs, 6.0);
            let r = reference(&x, fs, None);
            let (m, secs) = run_meter(&x, fs, None);
            for (wi, w) in [Weighting::A, Weighting::C, Weighting::Z]
                .into_iter()
                .enumerate()
            {
                for (ti, t) in TIME_WEIGHTINGS.into_iter().enumerate() {
                    let got = m.levels_of(w, t, PeakWeighting::C);
                    let want = r.levels[wi][ti];
                    assert_eq!(got.level.to_bits(), want.level.to_bits(), "{w:?} {t:?}");
                    assert_eq!(got.lmax.to_bits(), want.lmax.to_bits(), "{w:?} {t:?}");
                    assert_eq!(got.lmin.to_bits(), want.lmin.to_bits(), "{w:?} {t:?}");
                    assert_eq!(got.lpeak.to_bits(), want.lpeak.to_bits(), "{w:?} {t:?}");
                }
                let e = m.leq[wi].energy();
                assert!((e / r.leq[wi] - 1.0).abs() < 1e-12, "{fs} {w:?} Leq");
            }
            assert_eq!(secs.len(), r.seconds.len());
            for (s, want) in secs.iter().zip(&r.seconds) {
                assert_eq!(s.measured, 1.0);
                for (w, (g, v)) in s.energy.iter().zip(want).enumerate() {
                    assert!((g / v - 1.0).abs() < 1e-12, "{fs} w{w}");
                }
            }
        }
    }

    /// With a mic-curve correction the f32 convolution is the only difference from an f64
    /// reference: every level, the Leq and each second's A/C/Z level within 0.001 dB.
    #[test]
    fn corrected_chain_matches_f64_reference() {
        let fs = 48_000.0;
        let h = mic_correction(fs);
        let x = stepped_noise(fs, 4.0);
        let r = reference(&x, fs, Some(&h));
        let (m, secs) = run_meter(&x, fs, Some(&h));
        let mut worst = 0.0f64;
        for (wi, w) in [Weighting::A, Weighting::C, Weighting::Z]
            .into_iter()
            .enumerate()
        {
            for (ti, t) in TIME_WEIGHTINGS.into_iter().enumerate() {
                let got = m.levels_of(w, t, PeakWeighting::C);
                let want = r.levels[wi][ti];
                for (g, v) in [
                    (got.level, want.level),
                    (got.lmax, want.lmax),
                    (got.lmin, want.lmin),
                    (got.leq, want.leq),
                ] {
                    worst = worst.max((g - v).abs());
                }
                assert_eq!(got.lpeak.to_bits(), want.lpeak.to_bits(), "{w:?} {t:?}");
            }
        }
        assert_eq!(secs.len(), r.seconds.len());
        for (s, want) in secs.iter().zip(&r.seconds) {
            for (g, v) in s.energy.iter().zip(want) {
                worst = worst.max((power_dbfs(*g) - power_dbfs(*v)).abs());
            }
        }
        eprintln!("corrected chain vs f64 reference: max |Δ| = {worst:.2e} dB");
        assert!(worst < 1e-3, "{worst}");
    }

    /// Frozen, the displayed values hold while the per-second energies come out as from a
    /// meter that never froze; released, the levels follow the input again.
    #[test]
    fn freeze_holds_the_display_not_the_log() {
        let fs = 48_000.0;
        let x = stepped_noise(fs, 5.0);
        let (_, want) = run_meter(&x, fs, None);
        let mut m = meter(fs, Weighting::A, TimeWeighting::Fast, PeakWeighting::C);
        let mut got = Vec::new();
        let half = x.len() / 2;
        m.process(&x[..half / 2], |s| got.push(s));
        m.set_frozen(true);
        assert!(m.frozen());
        let held = m.levels();
        m.process(&x[half / 2..half], |s| got.push(s));
        let lv = m.levels();
        assert_eq!(
            [lv.level, lv.lmax, lv.leq, lv.lpeak, lv.duration_s].map(f64::to_bits),
            [held.level, held.lmax, held.leq, held.lpeak, held.duration_s].map(f64::to_bits)
        );
        m.set_frozen(false);
        m.process(&x[half..], |s| got.push(s));
        assert_eq!(got.len(), want.len());
        // Frozen runs are cut differently into plain chunk sums: equal to rounding.
        for (g, w) in got.iter().zip(&want) {
            for (a, b) in g.energy.iter().zip(&w.energy) {
                assert!((a / b - 1.0).abs() < 1e-13, "{a} vs {b}");
            }
        }
        assert!(m.levels().duration_s > held.duration_s);
    }

    /// The per-second A and C levels of steady sines at the IEC 61672-1 Table 3 frequencies
    /// (rounded to whole hertz, so a second holds whole cycles) follow the analytic
    /// weighting of Annex E (checked against `tools/refgen` in [`crate::weighting`]) within
    /// the filters' design bound, with and without a (flat) correction in the path.
    #[test]
    fn per_second_weighting_follows_iec_table() {
        let g = GoldenSet::load("weighting_iec61672").expect("golden");
        let f = g.f64("exact_hz").expect("f");
        for w in [Weighting::A, Weighting::C] {
            for fs in [48_000.0, 96_000.0] {
                let bound = if fs < 90_000.0 { 0.13 } else { 0.002 };
                for flat in [false, true] {
                    let mut worst = 0.0f64;
                    for fi in f.iter().map(|f| f.round()) {
                        if !(20.0..=16_000.0).contains(&fi) {
                            continue;
                        }
                        let mut m = meter(fs, w, TimeWeighting::Fast, PeakWeighting::C);
                        if flat {
                            m.set_correction(Some(&[1.0]));
                        }
                        let mut out = Vec::new();
                        m.process(&sine(fi, 0.5, fs, 3 * fs as usize), |s| out.push(s));
                        // The last second: the filters have settled and the partition
                        // delay has passed.
                        let l = out[2].level_dbfs(w) - power_dbfs(0.125);
                        let err = l - w.analytic_db(fi);
                        worst = worst.max(err.abs());
                        assert!(
                            err.abs() <= bound + 1e-3,
                            "{w:?} {fs} flat {flat} {fi} Hz: {err:.4} dB"
                        );
                    }
                    eprintln!(
                        "{w:?} {fs} correction {flat}: max |per-second − Annex E| {worst:.4} dB"
                    );
                }
            }
        }
    }

    /// The compensated total over plain chunk sums stays exact over 48 h at 48 kHz:
    /// chunk energies built from samples k/2²⁰ (squares exact in f64) are checked against
    /// the exact integer total.
    #[test]
    fn chunked_leq_exact_over_48_hours() {
        let fs = 48_000.0;
        let chunks = (48.0 * 3600.0 * fs / CHUNK as f64) as u64;
        let mut leq = Leq::new(fs);
        let mut exact: u128 = 0;
        let mut s = 0x2545_f491_4f6c_dd1du64;
        let unit = 2f64.powi(-40);
        for i in 0..chunks {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            // Loud for the first hours, then 60 dB quieter: the small sums must survive
            // the large total.
            let k = if i < chunks / 8 { s >> 44 } else { s >> 54 };
            // 256 samples of k/2²⁰: Σx² = 256·k²·2⁻⁴⁰, exact in f64.
            let k2 = u128::from(k) * u128::from(k) * CHUNK as u128;
            exact += k2;
            leq.push_energy(k2 as f64 * unit, CHUNK as u64);
        }
        let want = exact as f64 * unit;
        let rel = (leq.energy() / want - 1.0).abs();
        eprintln!("48 h compensated total: relative error {rel:.1e}");
        assert!(rel < 1e-15, "{rel}");
        assert_eq!(leq.samples(), chunks * CHUNK as u64);
    }
}
