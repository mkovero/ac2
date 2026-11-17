//! Analysis windows. All windows are periodic (DFT-even): the length-N window is the first N
//! points of a symmetric window of length N + 1, which is what makes overlapped segments sum
//! correctly and keeps one convention across every analysis (design Q4).

use std::f64::consts::{PI, TAU};

/// Window shapes offered by ac2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Window {
    /// General-purpose default.
    Hann,
    /// 4-term Blackman-Harris (−92 dB sidelobes) for high dynamic range.
    BlackmanHarris4,
    /// HFT95 flat-top (Heinzel et al. 2002): reads tone levels with negligible scalloping.
    FlatTop,
    /// No window. For periodic stimuli whose period equals the FFT length.
    Rectangular,
}

impl Window {
    /// Cosine-sum coefficients a_k for w[j] = Σ (−1)^k a_k cos(2π k j / N).
    fn cosine_terms(self) -> &'static [f64] {
        match self {
            Window::Hann => &[0.5, 0.5],
            Window::BlackmanHarris4 => &[0.35875, 0.48829, 0.14128, 0.01168],
            Window::FlatTop => &[1.0, 1.938_337_9, 1.304_520_2, 0.402_827_0, 0.035_066_5],
            Window::Rectangular => &[1.0],
        }
    }

    /// Window samples of length `n`.
    pub fn coefficients(self, n: usize) -> Vec<f64> {
        let terms = self.cosine_terms();
        (0..n)
            .map(|j| {
                let phase = TAU * j as f64 / n as f64;
                terms
                    .iter()
                    .enumerate()
                    .map(|(k, a)| {
                        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                        sign * a * (k as f64 * phase).cos()
                    })
                    .sum()
            })
            .collect()
    }

    /// Half-width of the main lobe in bins: a tone `d` bins from a bin centre has its first
    /// spectral nulls `lobe_half_width()` bins either side of it. A sum of K cosine terms
    /// (orders 0..K−1) is K pairs of Dirichlet kernels at offsets 0, ±1, … ±(K−1) bins, each
    /// with nulls on every other integer offset, so the first offset where all of them are
    /// in a null is K.
    pub fn lobe_half_width(self) -> usize {
        self.cosine_terms().len()
    }

    /// |W(ν)|², the squared magnitude of the length-`n` window's transform `ν` bins from its
    /// centre (ν need not be an integer): what a unit complex exponential `ν` bins away from
    /// a bin leaves in that bin. Summed over all `n` integer steps of ν it is `n·Σw²`
    /// (Parseval).
    ///
    /// W(ν) = Σ_m (−1)^m (a_m/2)·[D(ν−m) + D(ν+m)] with the Dirichlet kernel
    /// D(x) = e^{−iπx(n−1)/n}·sin(πx)/sin(πx/n). The offsets x = ν∓m differ by integers, so
    /// sin(πx) = (−1)^m·sin(πν) and the phase is a common factor times (−1)^m·e^{∓iπm/n};
    /// what remains inside the sum is e^{∓iπm/n}/sin(π(ν∓m)/n) with alternating sign.
    pub fn transform_power(self, nu: f64, n: usize) -> f64 {
        let terms = self.cosine_terms();
        let nf = n as f64;
        let nearest = nu.round();
        // On an integer offset every kernel is either at its peak (n) or in a null.
        if (nu - nearest).abs() < 1e-12 {
            let m = (nearest.rem_euclid(nf)).min(nf - nearest.rem_euclid(nf)) as usize;
            return match terms.get(m) {
                Some(a) if m == 0 => (a * nf).powi(2),
                Some(a) => (a * nf / 2.0).powi(2),
                None => 0.0,
            };
        }
        let (mut re, mut im) = (0.0, 0.0);
        for (m, a) in terms.iter().enumerate() {
            let sign = if m % 2 == 0 { 1.0 } else { -1.0 };
            let mf = m as f64;
            let below = 1.0 / (PI * (nu - mf) / nf).sin();
            let above = 1.0 / (PI * (nu + mf) / nf).sin();
            let (sin_m, cos_m) = (PI * mf / nf).sin_cos();
            re += sign * a / 2.0 * cos_m * (below + above);
            im += sign * a / 2.0 * sin_m * (above - below);
        }
        (PI * nu).sin().powi(2) * (re * re + im * im)
    }

    /// Worst-case amplitude loss for a tone halfway between bins, in dB (positive number).
    /// Shown to the operator; never applied as a correction.
    pub fn max_scalloping_loss_db(self) -> f64 {
        match self {
            Window::Hann => 1.42,
            Window::BlackmanHarris4 => 0.83,
            Window::FlatTop => 0.0044,
            Window::Rectangular => 3.92,
        }
    }
}

/// Sums that set the scale of amplitude and power spectra.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowGains {
    /// S₁ = Σw. Amplitude spectra divide by it so a bin-centred tone reads its amplitude.
    pub sum: f64,
    /// S₂ = Σw². Power spectral densities divide by fs·S₂.
    pub sum_sq: f64,
    /// Window length.
    pub len: usize,
}

impl WindowGains {
    /// Gains of the given window samples.
    pub fn of(w: &[f64]) -> Self {
        Self {
            sum: w.iter().sum(),
            sum_sq: w.iter().map(|v| v * v).sum(),
            len: w.len(),
        }
    }

    /// Equivalent noise bandwidth in bins, N·S₂ / S₁².
    pub fn enbw_bins(&self) -> f64 {
        self.len as f64 * self.sum_sq / (self.sum * self.sum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_testkit::golden::GoldenSet;

    #[test]
    fn hann_matches_golden() {
        let g = GoldenSet::load("spectrum_hann_tone_noise").expect("golden set");
        let expected = g.f64("window").expect("window array");
        let w = Window::Hann.coefficients(expected.len());
        for (i, (a, b)) in w.iter().zip(&expected).enumerate() {
            assert!((a - b).abs() < 1e-15, "sample {i}: {a} vs {b}");
        }
        let gains = WindowGains::of(&w);
        assert!((gains.sum - g.scalar("window_sum").expect("sum")).abs() < 1e-9);
        assert!((gains.sum_sq - g.scalar("window_sum_sq").expect("sum_sq")).abs() < 1e-9);
        assert!((gains.enbw_bins() - g.scalar("enbw_bins").expect("enbw")).abs() < 1e-12);
    }

    /// Scalloping loss measured from the window's own spectrum at half a bin must match the
    /// constant reported to the operator.
    #[test]
    fn scalloping_constants_match_window_shape() {
        let n = 4096;
        for win in [
            Window::Hann,
            Window::BlackmanHarris4,
            Window::FlatTop,
            Window::Rectangular,
        ] {
            let w = win.coefficients(n);
            let dc: f64 = w.iter().sum();
            let half_bin: f64 = {
                let (re, im) = w.iter().enumerate().fold((0.0, 0.0), |(re, im), (j, v)| {
                    let ph = std::f64::consts::PI * j as f64 / n as f64;
                    (re + v * ph.cos(), im - v * ph.sin())
                });
                (re * re + im * im).sqrt()
            };
            let loss_db = -20.0 * (half_bin / dc).log10();
            let stated = win.max_scalloping_loss_db();
            assert!(
                (loss_db - stated).abs() < 0.01,
                "{win:?}: measured {loss_db:.4} dB, stated {stated} dB"
            );
        }
    }

    /// The closed-form transform against the defining sum, on and between bins, and its
    /// Parseval total over the n bins.
    #[test]
    fn transform_power_matches_the_direct_sum() {
        let n = 64;
        for win in [
            Window::Hann,
            Window::BlackmanHarris4,
            Window::FlatTop,
            Window::Rectangular,
        ] {
            let w = win.coefficients(n);
            let direct = |nu: f64| {
                let (re, im) = w.iter().enumerate().fold((0.0, 0.0), |(re, im), (j, v)| {
                    let ph = TAU * nu * j as f64 / n as f64;
                    (re + v * ph.cos(), im - v * ph.sin())
                });
                re * re + im * im
            };
            let peak = direct(0.0);
            for nu in [0.0, 0.25, -0.5, 1.0, 2.0, 3.3, -4.0, 5.0, 7.5, -20.25, 31.9] {
                let (got, want) = (win.transform_power(nu, n), direct(nu));
                assert!(
                    (got - want).abs() < 1e-12 * peak,
                    "{win:?} at {nu}: {got} vs {want}"
                );
            }
            let s2 = WindowGains::of(&w).sum_sq;
            for d in [0.0, 0.3, 0.5] {
                let total: f64 = (0..n).map(|k| win.transform_power(k as f64 - d, n)).sum();
                assert!(
                    (total / (n as f64 * s2) - 1.0).abs() < 1e-12,
                    "{win:?} offset {d}"
                );
            }
            // The lobe edge is a null for a tone on a bin.
            let k = win.lobe_half_width() as f64;
            assert!(direct(k) < 1e-20 * peak, "{win:?} null at {k}");
            assert!(direct(k - 0.5) > 1e-12 * peak, "{win:?} inside the lobe");
        }
    }

    #[test]
    fn periodic_not_symmetric() {
        let w = Window::Hann.coefficients(8);
        assert_eq!(w[0], 0.0);
        assert!(w[7] > 0.0, "periodic window must not end at zero");
    }
}
