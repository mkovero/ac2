//! Loopback detection: which input carries a known burst back from an output.
//!
//! A loopback cable returns the stimulus with only the converters' latency and a fixed gain,
//! so its capture is the burst itself, delayed: the normalised cross-correlation at the
//! right lag is close to 1. An acoustic path smears the burst over the room's impulse
//! response and adds noise, so its correlation is lower, and it always arrives later than
//! the cable (sound needs time to travel). Inputs are ranked by correlation; among inputs
//! that correlate equally well (a clean simulated acoustic path, two cables) the earliest
//! arrival ranks first.

use realfft::RealFftPlanner;
use realfft::num_complex::Complex;

/// Normalised correlation at or above which the best-ranked input is called a loopback.
pub const LOOPBACK_MIN_CORRELATION: f64 = 0.8;

/// Correlations within this of the best one count as equal; the earlier arrival then ranks
/// first.
const TIE: f64 = 0.02;

/// One input's match against the burst.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopbackCandidate {
    /// Index of the capture in the slice passed to [`rank_inputs`].
    pub input: usize,
    /// Lag of the best match, samples (capture index − burst index).
    pub lag: usize,
    /// Normalised cross-correlation at that lag, −1 … 1 (negative: polarity inverted).
    pub correlation: f64,
    /// Gain of the capture relative to the burst at that lag, dB; `None` for silence.
    pub gain_db: Option<f64>,
}

/// Matches every capture against `burst` over lags `0 ..= max_lag` and ranks them, best
/// first. Capture sample `k` must be taken at the index burst sample `k` was emitted at
/// (lag 0 = no delay); a capture shorter than `burst.len() + max_lag` is searched over the
/// lags it covers.
pub fn rank_inputs(burst: &[f64], captures: &[&[f64]], max_lag: usize) -> Vec<LoopbackCandidate> {
    let mut out: Vec<LoopbackCandidate> = captures
        .iter()
        .enumerate()
        .map(|(input, y)| best_match(burst, y, max_lag, input))
        .collect();
    let best = out
        .iter()
        .map(|c| c.correlation.abs())
        .fold(0.0f64, f64::max);
    out.sort_by(|a, b| {
        let (ra, rb) = (a.correlation.abs(), b.correlation.abs());
        let (ta, tb) = (ra >= best - TIE, rb >= best - TIE);
        match (ta, tb) {
            (true, true) => a.lag.cmp(&b.lag).then(rb.total_cmp(&ra)),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => rb.total_cmp(&ra),
        }
        .then(a.input.cmp(&b.input))
    });
    out
}

fn best_match(x: &[f64], y: &[f64], max_lag: usize, input: usize) -> LoopbackCandidate {
    let none = LoopbackCandidate {
        input,
        lag: 0,
        correlation: 0.0,
        gain_db: None,
    };
    if x.is_empty() || y.len() < x.len() {
        return none;
    }
    let max_lag = max_lag.min(y.len() - x.len());
    let ex: f64 = x.iter().map(|v| v * v).sum();
    if ex <= 0.0 {
        return none;
    }
    // r(l) = Σ x[n]·y[n+l] through one FFT product. With both zero-padded to at least
    // len(x) + max_lag, no lag searched wraps around.
    let n = (x.len() + max_lag).max(y.len()).next_power_of_two();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let spec = |v: &[f64]| -> Vec<Complex<f64>> {
        let mut buf = vec![0.0; n];
        buf[..v.len()].copy_from_slice(v);
        let mut out = fwd.make_output_vec();
        // Lengths match the plan, so the transform cannot fail.
        let _ = fwd.process(&mut buf, &mut out);
        out
    };
    let sx = spec(x);
    let sy = spec(&y[..y.len().min(n)]);
    let mut prod: Vec<Complex<f64>> = sx.iter().zip(&sy).map(|(a, b)| a.conj() * b).collect();
    // The inverse wants real DC and Nyquist bins; they are, up to rounding.
    if let Some(f) = prod.first_mut() {
        f.im = 0.0;
    }
    if let Some(l) = prod.last_mut() {
        l.im = 0.0;
    }
    let mut r = inv.make_output_vec();
    let _ = inv.process(&mut prod, &mut r);
    let scale = 1.0 / n as f64;
    let (lag, peak) = r
        .iter()
        .take(max_lag + 1)
        .map(|v| v * scale)
        .enumerate()
        .fold((0usize, 0.0f64), |(bl, bv), (l, v)| {
            if v.abs() > bv.abs() { (l, v) } else { (bl, bv) }
        });
    let ey: f64 = y[lag..lag + x.len()].iter().map(|v| v * v).sum();
    if ey <= 0.0 || peak == 0.0 {
        return LoopbackCandidate { lag, ..none };
    }
    LoopbackCandidate {
        input,
        lag,
        correlation: (peak / (ex * ey).sqrt()).clamp(-1.0, 1.0),
        gain_db: Some(20.0 * (peak.abs() / ex).log10()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic white noise, −1 … 1.
    fn noise(n: usize, seed: u64) -> Vec<f64> {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
            })
            .collect()
    }

    fn delayed(x: &[f64], d: usize, gain: f64, len: usize) -> Vec<f64> {
        let mut y = vec![0.0; len];
        for (i, v) in x.iter().enumerate() {
            if i + d < len {
                y[i + d] = gain * v;
            }
        }
        y
    }

    #[test]
    fn cable_beats_an_equally_clean_later_path_and_noise() {
        let x = noise(4800, 1);
        let len = 4800 + 2400;
        let cable = delayed(&x, 32, 1.0, len);
        // A clean "acoustic" path: −6 dB, 5 ms later; correlates just as well.
        let mut room = delayed(&x, 272, 0.5, len);
        for (v, n) in room.iter_mut().zip(noise(len, 9)) {
            *v += 1e-4 * n;
        }
        let other = noise(len, 3);
        let silent = vec![0.0; len];
        let r = rank_inputs(&x, &[&silent, &room, &other, &cable], 2400);
        assert_eq!(r[0].input, 3);
        assert_eq!(r[0].lag, 32);
        assert!(r[0].correlation > 0.999, "{r:?}");
        assert!(r[0].gain_db.is_some_and(|g| g.abs() < 1e-6));
        assert_eq!(r[1].input, 1);
        assert_eq!(r[1].lag, 272);
        assert!(r[1].gain_db.is_some_and(|g| (g + 6.02).abs() < 0.05));
        assert!(r[2].correlation.abs() < 0.1, "{r:?}");
        assert_eq!(r[3].input, 0);
        assert_eq!(r[3].correlation, 0.0);
        assert_eq!(r[3].gain_db, None);
    }

    #[test]
    fn inverted_cable_and_short_capture() {
        let x = noise(1000, 5);
        let y = delayed(&x, 10, -0.25, 1100);
        let r = rank_inputs(&x, &[&y], 5000);
        assert_eq!(r[0].lag, 10);
        assert!(r[0].correlation < -0.999);
        assert!(rank_inputs(&x, &[&y[..500]], 10)[0].correlation == 0.0);
    }
}
