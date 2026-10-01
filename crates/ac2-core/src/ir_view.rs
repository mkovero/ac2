//! Live impulse-response views: linear, log and ETC (PLAN.md §3.3).
//!
//! The input is a one-sided transfer function on uniform bins (`n/2 + 1` bins of an
//! `n`-point FFT), measured with the operator's inserted delay already removed. Its inverse
//! FFT is therefore an impulse response whose time zero **is** the inserted delay: an
//! arrival exactly at the inserted delay sits at t = 0, a later one at positive time, an
//! earlier one (or acausal pre-ringing of a band-limited estimate) at negative time.
//!
//! The IFFT is circular, so the `n` samples are rotated (fftshift) to span
//! `t ∈ [−n/2, n/2) / fs`: half the window before the inserted delay, half after. An arrival
//! more than `n/2` samples away from the inserted delay wraps to the other side; that is a
//! property of the FFT length, not of this view.
//!
//! - **Linear**: the samples.
//! - **Log**: `20·log10|h|`, absolute (not peak-normalised: levels compare across traces).
//! - **ETC**: `20·log10` of the Hilbert envelope `|h + j·𝓗{h}|`, which removes each
//!   arrival's oscillation so reflections read at their level. The Hilbert transform is
//!   taken in the frequency domain (`−j·sign(f)`), treating the IR as circular — which it is.
//!
//! Display decimation keeps peaks: [`bucket_max`] returns the largest value of each bucket
//! and where it was. A stride pick can step over a one-sample arrival and draw it absent.

use num_complex::Complex64;
use realfft::RealFftPlanner;

/// Lowest dB value either log view reports; an exact zero has no logarithm.
pub const FLOOR_DB: f64 = -200.0;

/// One-sided uniform-bin transfer function.
#[derive(Debug, Clone, Copy)]
pub struct UniformTf<'a> {
    /// Bins `0..=n/2` of an `n`-point FFT (`n = 2·(len − 1)`). Non-finite bins count as 0.
    pub h: &'a [Complex64],
    /// Sample rate, Hz.
    pub sample_rate: f64,
    /// Inserted delay removed from `h`, seconds; time zero of the view.
    pub inserted_delay_s: f64,
}

/// Impulse response with time zero at the inserted delay.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpulseResponse {
    /// Samples; index `i` is time `(i − n/2) / fs` relative to the inserted delay.
    pub samples: Vec<f64>,
    /// Sample rate, Hz.
    pub sample_rate: f64,
    /// Inserted delay this response is relative to, seconds.
    pub inserted_delay_s: f64,
}

/// Why an IR could not be formed.
#[derive(Debug, Clone, PartialEq)]
pub enum IrError {
    /// Fewer than 2 bins.
    TooShort,
    /// Sample rate not positive and finite.
    BadSampleRate,
}

impl std::fmt::Display for IrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IrError::TooShort => write!(f, "transfer function needs at least 2 bins"),
            IrError::BadSampleRate => write!(f, "sample rate must be positive and finite"),
        }
    }
}

impl std::error::Error for IrError {}

impl ImpulseResponse {
    /// Inverse FFT of `tf`, rotated so time zero sits at index `n/2`.
    ///
    /// The imaginary parts of the DC and Nyquist bins are dropped: a real response has none,
    /// and an estimate's residue there carries no time-domain meaning.
    pub fn from_tf(tf: UniformTf<'_>) -> Result<Self, IrError> {
        if tf.h.len() < 2 {
            return Err(IrError::TooShort);
        }
        if !(tf.sample_rate.is_finite() && tf.sample_rate > 0.0) {
            return Err(IrError::BadSampleRate);
        }
        let n = 2 * (tf.h.len() - 1);
        let mut spec: Vec<Complex64> =
            tf.h.iter()
                .map(|z| {
                    if z.re.is_finite() && z.im.is_finite() {
                        *z
                    } else {
                        Complex64::new(0.0, 0.0)
                    }
                })
                .collect();
        let last = spec.len() - 1;
        spec[0].im = 0.0;
        spec[last].im = 0.0;
        let mut circ = vec![0.0; n];
        let ifft = RealFftPlanner::<f64>::new().plan_fft_inverse(n);
        ifft.process(&mut spec, &mut circ)
            .map_err(|_| IrError::TooShort)?;
        let scale = 1.0 / n as f64;
        let half = n / 2;
        let samples = (0..n).map(|i| circ[(i + half) % n] * scale).collect();
        Ok(Self {
            samples,
            sample_rate: tf.sample_rate,
            inserted_delay_s: tf.inserted_delay_s,
        })
    }

    /// Number of samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// True if there are no samples.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Time of sample `i` relative to the inserted delay, seconds.
    pub fn time_s(&self, i: usize) -> f64 {
        (i as f64 - (self.len() / 2) as f64) / self.sample_rate
    }

    /// Absolute time of sample `i` on the session's time base (inserted delay + relative).
    pub fn absolute_time_s(&self, i: usize) -> f64 {
        self.inserted_delay_s + self.time_s(i)
    }

    /// Linear view.
    pub fn linear(&self) -> &[f64] {
        &self.samples
    }

    /// Log view: `20·log10|h|`, floored at [`FLOOR_DB`].
    pub fn log_db(&self) -> Vec<f64> {
        self.samples.iter().map(|v| amp_db(v.abs())).collect()
    }

    /// ETC view: Hilbert envelope in dB, floored at [`FLOOR_DB`].
    pub fn etc_db(&self) -> Vec<f64> {
        self.envelope().into_iter().map(amp_db).collect()
    }

    /// Hilbert envelope `|h + j·𝓗{h}|`.
    pub fn envelope(&self) -> Vec<f64> {
        let n = self.len();
        let mut planner = RealFftPlanner::<f64>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let mut x = self.samples.clone();
        let mut spec = fwd.make_output_vec();
        if fwd.process(&mut x, &mut spec).is_err() {
            return vec![0.0; n];
        }
        // 𝓗 multiplies positive frequencies by −j; DC and Nyquist have no quadrature part.
        let last = spec.len() - 1;
        for (k, z) in spec.iter_mut().enumerate() {
            *z = if k == 0 || k == last {
                Complex64::new(0.0, 0.0)
            } else {
                Complex64::new(z.im, -z.re)
            };
        }
        let mut quad = vec![0.0; n];
        if inv.process(&mut spec, &mut quad).is_err() {
            return vec![0.0; n];
        }
        let scale = 1.0 / n as f64;
        self.samples
            .iter()
            .zip(&quad)
            .map(|(r, q)| r.hypot(q * scale))
            .collect()
    }
}

fn amp_db(a: f64) -> f64 {
    let db = 20.0 * a.log10();
    if db.is_nan() {
        FLOOR_DB
    } else {
        db.max(FLOOR_DB)
    }
}

/// A decimated display point: the extreme value of a bucket and the source index it came
/// from, so the point is drawn at the peak's own time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeakPoint {
    /// Source index.
    pub index: usize,
    /// Value at `index`.
    pub value: f64,
}

/// Bucket boundaries: bucket `b` covers `[b·len/n, (b+1)·len/n)`.
fn buckets(len: usize, n: usize) -> impl Iterator<Item = (usize, usize)> {
    let n = n.min(len);
    (0..n).map(move |b| (b * len / n, (b + 1) * len / n))
}

/// Largest value per bucket, for `n` display points (all points if `n ≥ len`). NaN values
/// are skipped; an all-NaN bucket yields its first index with NaN.
pub fn bucket_max(values: &[f64], n: usize) -> Vec<PeakPoint> {
    buckets(values.len(), n)
        .map(|(lo, hi)| extreme(values, lo, hi, |a, b| a > b))
        .collect()
}

/// Smallest and largest value per bucket (in index order within the bucket), for signed
/// curves such as the linear IR where both polarities of an arrival must survive.
pub fn bucket_min_max(values: &[f64], n: usize) -> Vec<(PeakPoint, PeakPoint)> {
    buckets(values.len(), n)
        .map(|(lo, hi)| {
            let mn = extreme(values, lo, hi, |a, b| a < b);
            let mx = extreme(values, lo, hi, |a, b| a > b);
            if mn.index <= mx.index {
                (mn, mx)
            } else {
                (mx, mn)
            }
        })
        .collect()
}

fn extreme(values: &[f64], lo: usize, hi: usize, better: impl Fn(f64, f64) -> bool) -> PeakPoint {
    let mut best = PeakPoint {
        index: lo,
        value: f64::NAN,
    };
    for (i, &v) in values.iter().enumerate().take(hi).skip(lo) {
        if !v.is_nan() && (best.value.is_nan() || better(v, best.value)) {
            best = PeakPoint { index: i, value: v };
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_testkit::golden::GoldenSet;
    use std::f64::consts::TAU;

    fn delay_tf(n: usize, d: f64) -> Vec<Complex64> {
        (0..=n / 2)
            .map(|k| Complex64::from_polar(1.0, -TAU * k as f64 * d / n as f64))
            .collect()
    }

    #[test]
    fn pure_delay_peaks_at_right_time() {
        let fs = 48000.0;
        let n = 1024;
        for d in [0i32, 10, -5, 300, -300] {
            let h = delay_tf(n, f64::from(d));
            let ir = ImpulseResponse::from_tf(UniformTf {
                h: &h,
                sample_rate: fs,
                inserted_delay_s: 0.01,
            })
            .expect("ir");
            let peak = bucket_max(ir.linear(), ir.len()).into_iter().fold(
                PeakPoint {
                    index: 0,
                    value: f64::MIN,
                },
                |a, p| {
                    if p.value > a.value { p } else { a }
                },
            );
            assert!(
                (peak.value - 1.0).abs() < 1e-12,
                "delay {d}: peak {}",
                peak.value
            );
            let t = ir.time_s(peak.index);
            assert!((t - f64::from(d) / fs).abs() < 1e-12, "delay {d}: at {t} s");
            assert!((ir.absolute_time_s(peak.index) - (0.01 + f64::from(d) / fs)).abs() < 1e-12);
            // Everything else is zero for an integer delay.
            let rest: f64 = ir
                .linear()
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != peak.index)
                .map(|(_, v)| v.abs())
                .fold(0.0, f64::max);
            assert!(rest < 1e-12);
        }
    }

    /// h(t) = e^{−t/T}·sin(2π f₀ t): the ETC falls at 20·log10(e)/T dB per second.
    #[test]
    fn etc_of_decaying_exponential_has_right_slope() {
        let fs = 48000.0;
        let n = 8192;
        let t_decay = 0.01;
        let f0 = 3000.0;
        let x: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 / fs;
                if i < n / 2 {
                    (-t / t_decay).exp() * (TAU * f0 * t).sin()
                } else {
                    0.0
                }
            })
            .collect();
        let mut planner = RealFftPlanner::<f64>::new();
        let fwd = planner.plan_fft_forward(n);
        let mut buf = x.clone();
        let mut h = fwd.make_output_vec();
        fwd.process(&mut buf, &mut h).expect("fft");
        let ir = ImpulseResponse::from_tf(UniformTf {
            h: &h,
            sample_rate: fs,
            inserted_delay_s: 0.0,
        })
        .expect("ir");
        let etc = ir.etc_db();
        // Fit over 5..40 ms after the onset (well clear of the onset and the floor).
        let pts: Vec<(f64, f64)> = (0..ir.len())
            .map(|i| (ir.time_s(i), etc[i]))
            .filter(|(t, _)| (0.005..0.04).contains(t))
            .collect();
        let m = pts.len() as f64;
        let (st, se) = pts.iter().fold((0.0, 0.0), |(a, b), (t, e)| (a + t, b + e));
        let (mt, me) = (st / m, se / m);
        let (num, den) = pts.iter().fold((0.0, 0.0), |(a, b), (t, e)| {
            (a + (t - mt) * (e - me), b + (t - mt) * (t - mt))
        });
        let slope = num / den;
        let expected = -20.0 * std::f64::consts::LOG10_E / t_decay;
        assert!(
            (slope / expected - 1.0).abs() < 1e-3,
            "slope {slope} dB/s, expected {expected}"
        );
        // ETC carries no oscillation at 2·f₀ (which would swing tens of dB); what remains is
        // leakage of the onset step through the circular Hilbert transform.
        let dev = pts
            .iter()
            .map(|(t, e)| (e - (me + slope * (t - mt))).abs())
            .fold(0.0, f64::max);
        assert!(dev < 0.3, "ripple {dev} dB");
    }

    #[test]
    fn matches_refgen_etc_golden() {
        let gs = GoldenSet::load("display_ir_etc").expect("golden set");
        let h = gs.c128("h").expect("h");
        let ir = ImpulseResponse::from_tf(UniformTf {
            h: &h,
            sample_rate: gs.scalar("fs_hz").expect("fs"),
            inserted_delay_s: 0.0,
        })
        .expect("ir");
        gs.assert_f64("ir", ir.linear());
        gs.assert_f64("etc_db", &ir.etc_db());
    }

    #[test]
    fn decimation_preserves_max() {
        let mut v: Vec<f64> = (0..10_007).map(|i| -((i % 97) as f64)).collect();
        v[5003] = 42.0;
        v[17] = -500.0;
        v[9000] = f64::NAN;
        let d = bucket_max(&v, 300);
        assert_eq!(d.len(), 300);
        let top = d.iter().copied().fold(
            PeakPoint {
                index: 0,
                value: f64::MIN,
            },
            |a, p| {
                if p.value > a.value { p } else { a }
            },
        );
        assert_eq!(
            top,
            PeakPoint {
                index: 5003,
                value: 42.0
            }
        );
        // Every bucket's value is the true max of its range.
        let len = v.len();
        for (b, p) in d.iter().enumerate() {
            let (lo, hi) = (b * len / 300, (b + 1) * len / 300);
            let m = v[lo..hi]
                .iter()
                .copied()
                .filter(|x| !x.is_nan())
                .fold(f64::MIN, f64::max);
            assert_eq!(p.value, m);
            assert!((lo..hi).contains(&p.index));
        }
        let mm = bucket_min_max(&v, 300);
        assert!(
            mm.iter()
                .any(|(a, b)| a.value == -500.0 || b.value == -500.0)
        );
        assert!(mm.iter().all(|(a, b)| a.index <= b.index));
        // Fewer points than requested: identity.
        let short = [1.0, 3.0, 2.0];
        let d = bucket_max(&short, 10);
        assert_eq!(d.iter().map(|p| p.value).collect::<Vec<_>>(), short);
    }
}
