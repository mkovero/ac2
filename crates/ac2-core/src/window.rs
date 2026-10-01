//! Analysis windows. All windows are periodic (DFT-even): the length-N window is the first N
//! points of a symmetric window of length N + 1, which is what makes overlapped segments sum
//! correctly and keeps one convention across every analysis (design Q4).

use std::f64::consts::TAU;

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

    /// Coherent gain S₁ / N.
    pub fn coherent_gain(&self) -> f64 {
        self.sum / self.len as f64
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

    #[test]
    fn periodic_not_symmetric() {
        let w = Window::Hann.coefficients(8);
        assert_eq!(w[0], 0.0);
        assert!(w[7] > 0.0, "periodic window must not end at zero");
    }
}
