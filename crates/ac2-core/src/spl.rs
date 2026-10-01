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
//! raw ──┬── [mic-curve correction, §5.7] ── A/C/Z ── time weighting ── L, Lmax, Lmin
//!       │                                        └── Leq (f64 energy)
//!       └── C/Z (uncorrected) ── |x| max ── Lpeak
//! ```
//!
//! The mic-curve correction filter is not part of this module yet; see
//! [`SplMeter::process`] for where it belongs.

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

/// SPL meter configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplMeterConfig {
    /// Sample rate in Hz.
    pub fs: f64,
    /// Frequency weighting for L, Lmax, Lmin and Leq.
    pub weighting: Weighting,
    /// Time weighting for L, Lmax and Lmin.
    pub time_weighting: TimeWeighting,
    /// Frequency weighting for Lpeak.
    pub peak_weighting: PeakWeighting,
}

/// Sound level meter on one input: time-weighted level with Lmax/Lmin, Leq and Lpeak.
#[derive(Debug, Clone)]
pub struct SplMeter {
    cfg: SplMeterConfig,
    weight: WeightingFilter,
    peak_weight: WeightingFilter,
    detector: TimeWeightedDetector,
    leq: Leq,
    peak: PeakDetector,
    max_ms: f64,
    min_ms: f64,
    settle_left: u64,
    settle_samples: u64,
}

impl SplMeter {
    /// Builds the meter; fails if the rate is too low for A/C weighting.
    pub fn new(cfg: SplMeterConfig) -> Result<Self, WeightingError> {
        let peak_w = match cfg.peak_weighting {
            PeakWeighting::C => Weighting::C,
            PeakWeighting::Z => Weighting::Z,
        };
        // Lmin is meaningless while the detector rises from its silent initial state; five
        // rise time constants bring it within 0.03 dB of a steady input.
        let settle_samples = (5.0 * cfg.time_weighting.rise_s() * cfg.fs).ceil() as u64;
        Ok(Self {
            weight: WeightingFilter::new(cfg.weighting, cfg.fs)?,
            peak_weight: WeightingFilter::new(peak_w, cfg.fs)?,
            detector: TimeWeightedDetector::new(cfg.time_weighting, cfg.fs),
            leq: Leq::new(cfg.fs),
            peak: PeakDetector::default(),
            max_ms: 0.0,
            min_ms: f64::INFINITY,
            settle_left: settle_samples,
            settle_samples,
            cfg,
        })
    }

    /// Configuration.
    pub fn config(&self) -> &SplMeterConfig {
        &self.cfg
    }

    /// Processes raw input samples (FS). Does not allocate.
    ///
    /// Mic-curve hook (PLAN.md §5.7, phase 5): a per-input magnitude correction filter,
    /// normalised at the calibrator frequency, goes on `x` immediately before
    /// `self.weight` — so it feeds the time-weighted, Lmax/Lmin and Leq paths — and never
    /// before `self.peak_weight`, whose LCpeak stays on the uncorrected samples unless the
    /// correction filter's latency and pre-ringing are characterised (§5.3).
    pub fn process(&mut self, block: &[f64]) {
        for &x in block {
            let y = self.weight.process_sample(x);
            let ms = self.detector.push(y);
            self.leq.push_sample(y);
            self.peak.push(self.peak_weight.process_sample(x));
            self.max_ms = self.max_ms.max(ms);
            if self.settle_left > 0 {
                self.settle_left -= 1;
            } else {
                self.min_ms = self.min_ms.min(ms);
            }
        }
    }

    /// Current levels in dBFS.
    pub fn levels(&self) -> Levels {
        Levels {
            scale: LevelScale::Dbfs,
            level: self.detector.level_dbfs(),
            lmax: power_dbfs(self.max_ms),
            lmin: if self.min_ms.is_finite() {
                power_dbfs(self.min_ms)
            } else {
                f64::NAN
            },
            leq: self.leq.level_dbfs(),
            lpeak: self.peak.level_dbfs(),
            duration_s: self.leq.duration_s(),
        }
    }

    /// The Leq accumulator of the current interval.
    pub fn leq(&self) -> &Leq {
        &self.leq
    }

    /// Starts a new interval for Leq, Lpeak, Lmax and Lmin. Filters and detector keep their
    /// state, so the running level is continuous.
    pub fn reset_interval(&mut self) {
        self.leq.reset();
        self.peak.reset();
        self.max_ms = self.detector.mean_square();
        self.min_ms = f64::INFINITY;
    }

    /// Back to the initial state: silent filters and detector, empty interval.
    pub fn reset(&mut self) {
        self.weight.reset();
        self.peak_weight.reset();
        self.detector.reset();
        self.reset_interval();
        self.max_ms = 0.0;
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
