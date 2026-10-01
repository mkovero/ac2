//! Kaiser-windowed lowpass design and the pair decimator.
//!
//! The decimator evaluates the FIR only at the kept output instants (the polyphase form of a
//! decimating filter: every input sample enters the delay line, only one output in `M` is
//! computed). Both channels of a pair share one coefficient set and one phase counter, so it
//! is impossible for the reference and the measurement to be decimated at different phases;
//! H1 = Gxy/Gxx is transparent to the filter only when both legs see identical chains.

use std::f64::consts::PI;

/// Kaiser lowpass design parameters and taps.
#[derive(Debug, Clone, PartialEq)]
pub struct FirDesign {
    /// Input (full) sample rate.
    pub input_rate_hz: f64,
    /// Passband edge: the highest frequency the stage serves.
    pub passband_hz: f64,
    /// Stopband edge: everything above is attenuated by at least `attenuation_db`.
    pub stopband_hz: f64,
    /// Ideal-lowpass cutoff, midway between the edges.
    pub cutoff_hz: f64,
    /// Design stopband attenuation in dB.
    pub attenuation_db: f64,
    /// Kaiser β.
    pub beta: f64,
    /// Symmetric (linear-phase) taps, odd length, unity DC gain.
    pub taps: Vec<f64>,
}

impl FirDesign {
    /// Kaiser window-method lowpass (Kaiser's length and β formulas, as in
    /// `scipy.signal.kaiserord` + `firwin`). The length is rounded up to odd so the group
    /// delay is an integer number of input samples.
    ///
    /// # Panics
    /// Unless `0 < passband < stopband <= input_rate / 2`.
    pub fn kaiser_lowpass(
        input_rate_hz: f64,
        passband_hz: f64,
        stopband_hz: f64,
        attenuation_db: f64,
    ) -> Self {
        let nyq = input_rate_hz / 2.0;
        assert!(
            passband_hz > 0.0 && stopband_hz > passband_hz && stopband_hz <= nyq,
            "invalid FIR edges {passband_hz} / {stopband_hz} at {input_rate_hz} Hz"
        );
        let width = (stopband_hz - passband_hz) / nyq;
        let numtaps = ((attenuation_db - 7.95) / (2.285 * PI * width) + 1.0).ceil() as usize;
        let numtaps = numtaps.max(3) | 1;
        let beta = kaiser_beta(attenuation_db);
        let cutoff_hz = 0.5 * (passband_hz + stopband_hz);
        let c = cutoff_hz / nyq;
        let alpha = 0.5 * (numtaps - 1) as f64;
        let i0b = bessel_i0(beta);
        let mut taps: Vec<f64> = (0..numtaps)
            .map(|n| {
                let m = n as f64 - alpha;
                let r = m / alpha;
                let win = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0b;
                c * sinc(c * m) * win
            })
            .collect();
        let dc: f64 = taps.iter().sum();
        for t in &mut taps {
            *t /= dc;
        }
        Self {
            input_rate_hz,
            passband_hz,
            stopband_hz,
            cutoff_hz,
            attenuation_db,
            beta,
            taps,
        }
    }

    /// Number of taps.
    pub fn len(&self) -> usize {
        self.taps.len()
    }

    /// Never true for a designed filter.
    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
    }

    /// Complex frequency response at `f_hz` (input-rate frequency).
    pub fn response(&self, f_hz: f64) -> num_complex::Complex64 {
        let w = 2.0 * PI * f_hz / self.input_rate_hz;
        self.taps
            .iter()
            .enumerate()
            .map(|(n, h)| num_complex::Complex64::from_polar(*h, -w * n as f64))
            .sum()
    }
}

/// Kaiser β for a stopband attenuation in dB (Kaiser 1974).
fn kaiser_beta(a: f64) -> f64 {
    if a > 50.0 {
        0.1102 * (a - 8.7)
    } else if a > 21.0 {
        0.5842 * (a - 21.0).powf(0.4) + 0.07886 * (a - 21.0)
    } else {
        0.0
    }
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

/// Modified Bessel function of the first kind, order 0, by its power series. The series has
/// only positive terms, so summing until the term is negligible is accurate for the β range
/// used here (β < 20).
fn bessel_i0(x: f64) -> f64 {
    let q = 0.25 * x * x;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut k = 1.0;
    while term > 1e-18 * sum {
        term *= q / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

/// Two-channel decimator with one shared phase counter.
///
/// Output `j` is the filter evaluated over inputs `j·M .. j·M + L` (L taps): the first output
/// appears only once the delay line is full, so the filter's start-up transient is never
/// emitted.
#[derive(Debug, Clone)]
pub struct PairDecimator {
    taps: Vec<f64>,
    factor: usize,
    /// Delay lines stored twice over so the newest `L` samples are always one contiguous slice.
    hist_x: Vec<f64>,
    hist_y: Vec<f64>,
    pos: usize,
    /// Inputs consumed since reset.
    consumed: u64,
    /// Input count at which the next output is due (shared by both channels).
    next_out: u64,
}

impl PairDecimator {
    /// Decimator by `factor` with the given (symmetric) taps.
    ///
    /// # Panics
    /// If `factor` is zero or `taps` is empty.
    pub fn new(taps: Vec<f64>, factor: usize) -> Self {
        assert!(factor > 0 && !taps.is_empty());
        let l = taps.len();
        Self {
            taps,
            factor,
            hist_x: vec![0.0; 2 * l],
            hist_y: vec![0.0; 2 * l],
            pos: 0,
            consumed: 0,
            next_out: l as u64,
        }
    }

    /// Filter length.
    pub fn len(&self) -> usize {
        self.taps.len()
    }

    /// Never true.
    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
    }

    /// Decimation factor.
    pub fn factor(&self) -> usize {
        self.factor
    }

    /// Clear the delay lines and phase.
    pub fn reset(&mut self) {
        self.hist_x.fill(0.0);
        self.hist_y.fill(0.0);
        self.pos = 0;
        self.consumed = 0;
        self.next_out = self.taps.len() as u64;
    }

    /// Push an equal-length pair of input slices; decimated outputs are appended to
    /// `out_x` / `out_y` (always the same number to each).
    ///
    /// # Panics
    /// If `x` and `y` differ in length: a pair can only advance together.
    pub fn push(&mut self, x: &[f64], y: &[f64], out_x: &mut Vec<f64>, out_y: &mut Vec<f64>) {
        assert_eq!(
            x.len(),
            y.len(),
            "pair decimator legs must advance together"
        );
        let l = self.taps.len();
        for (&a, &b) in x.iter().zip(y) {
            self.hist_x[self.pos] = a;
            self.hist_x[self.pos + l] = a;
            self.hist_y[self.pos] = b;
            self.hist_y[self.pos + l] = b;
            self.pos += 1;
            if self.pos == l {
                self.pos = 0;
            }
            self.consumed += 1;
            if self.consumed == self.next_out {
                self.next_out += self.factor as u64;
                // Oldest..newest is hist[pos..pos+L]; the taps are symmetric, so the dot
                // product with them in either order is the convolution.
                let wx = &self.hist_x[self.pos..self.pos + l];
                let wy = &self.hist_y[self.pos..self.pos + l];
                let mut sx = 0.0;
                let mut sy = 0.0;
                for ((h, a), b) in self.taps.iter().zip(wx).zip(wy) {
                    sx += h * a;
                    sy += h * b;
                }
                out_x.push(sx);
                out_y.push(sy);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bessel_i0_known_values() {
        // Abramowitz & Stegun table 9.8: I0(1) = 1.266065878, I0(5) = 27.23987182.
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008_4).abs() < 1e-14);
        assert!((bessel_i0(5.0) - 27.239_871_823_604_44).abs() < 1e-11);
    }

    #[test]
    fn taps_symmetric_unity_dc() {
        let d = FirDesign::kaiser_lowpass(48_000.0, 1_000.0, 10_000.0, 90.0);
        assert_eq!(d.len() % 2, 1);
        let s: f64 = d.taps.iter().sum();
        assert!((s - 1.0).abs() < 1e-14);
        for i in 0..d.len() {
            assert!((d.taps[i] - d.taps[d.len() - 1 - i]).abs() < 1e-18);
        }
    }

    #[test]
    fn phase_is_shared_and_chunking_invariant() {
        let d = FirDesign::kaiser_lowpass(48_000.0, 1_000.0, 10_000.0, 90.0);
        let x: Vec<f64> = (0..5000)
            .map(|i| ((i * 7919) % 101) as f64 - 50.0)
            .collect();
        let y: Vec<f64> = x.iter().map(|v| -2.0 * v).collect();
        let mut a = PairDecimator::new(d.taps.clone(), 4);
        let (mut ax, mut ay) = (Vec::new(), Vec::new());
        a.push(&x, &y, &mut ax, &mut ay);
        let mut b = PairDecimator::new(d.taps.clone(), 4);
        let (mut bx, mut by) = (Vec::new(), Vec::new());
        let mut i = 0;
        for chunk in [1usize, 3, 17, 64, 999, 1].iter().cycle() {
            if i >= x.len() {
                break;
            }
            let e = (i + chunk).min(x.len());
            b.push(&x[i..e], &y[i..e], &mut bx, &mut by);
            i = e;
        }
        assert_eq!(ax, bx);
        assert_eq!(ay, by);
        assert_eq!(ax.len(), (5000 - d.len()) / 4 + 1);
        for (p, q) in ax.iter().zip(&ay) {
            assert_eq!(*q, -2.0 * p);
        }
        // Output 0 is the filter over inputs 0..L.
        let direct: f64 = d.taps.iter().zip(&x).map(|(h, v)| h * v).sum();
        assert!((ax[0] - direct).abs() < 1e-9);
    }

    #[test]
    #[should_panic(expected = "advance together")]
    fn unequal_legs_rejected() {
        let mut d = PairDecimator::new(vec![1.0], 2);
        d.push(&[1.0, 2.0], &[1.0], &mut Vec::new(), &mut Vec::new());
    }
}
