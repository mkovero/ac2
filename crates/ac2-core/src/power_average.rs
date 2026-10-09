//! Averaging of successive band-power intervals (the fractional-octave RTA's frames), on
//! power and weighted by each interval's duration.
//!
//! A level meter's average is the mean of power over time: averaging dB would weight the
//! quiet moments of a fluctuating signal as much as the loud ones and read low (two equal
//! halves at 0 dB and −20 dB average to −2.96 dB in power, −10 dB in dB). The RTA's
//! intervals are not all the same length (one per result, as the results are paced), so each
//! interval counts by its duration: a FIFO over intervals that together span the record
//! reads exactly the record's power mean, whatever the pacing.

use std::collections::VecDeque;

use crate::spectrum::Averaging;

/// An invalid averaging (FIFO of no frames, a time constant not positive and finite).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InvalidAveraging;

impl std::fmt::Display for InvalidAveraging {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid averaging parameters")
    }
}

impl std::error::Error for InvalidAveraging {}

/// Averages per-band powers of successive intervals.
#[derive(Debug, Clone)]
pub struct PowerAverager {
    averaging: Averaging,
    /// FIFO: the stored intervals, oldest first, as (duration, power per band).
    fifo: VecDeque<(f64, Vec<f64>)>,
    /// FIFO: Σ duration·power per band, and Σ duration, over `fifo`.
    sum: Vec<f64>,
    sum_s: f64,
    /// FIFO: pushes since the sums were last rebuilt from the stored intervals.
    since_resum: usize,
    /// Exponential: the state; `None` before the first interval.
    state: Option<Vec<f64>>,
    /// Exponential: time averaged so far.
    elapsed_s: f64,
    out: Vec<f64>,
}

impl PowerAverager {
    /// An averager of `bands` band powers.
    pub fn new(averaging: Averaging, bands: usize) -> Result<Self, InvalidAveraging> {
        match averaging {
            Averaging::Off => {}
            Averaging::Fifo { frames } if frames >= 1 => {}
            Averaging::Exponential { time_constant_s }
                if time_constant_s.is_finite() && time_constant_s > 0.0 => {}
            Averaging::Fifo { .. } | Averaging::Exponential { .. } => {
                return Err(InvalidAveraging);
            }
        }
        Ok(Self {
            averaging,
            fifo: VecDeque::new(),
            sum: vec![0.0; bands],
            sum_s: 0.0,
            since_resum: 0,
            state: None,
            elapsed_s: 0.0,
            out: vec![0.0; bands],
        })
    }

    /// Forgets every interval.
    pub fn reset(&mut self) {
        self.fifo.clear();
        self.sum.fill(0.0);
        self.sum_s = 0.0;
        self.since_resum = 0;
        self.state = None;
        self.elapsed_s = 0.0;
    }

    /// Adds an interval of `duration_s` (> 0) whose mean power per band is `powers`, and
    /// returns the average.
    ///
    /// - Off: the interval itself.
    /// - FIFO of N: the duration-weighted mean of the last N intervals.
    /// - Exponential of τ: each interval of length dt moves the average by
    ///   α = 1 − e^(−dt/τ) toward it; until τ worth has been averaged, α is at least
    ///   dt / (time so far), so the start is the plain power mean of what arrived rather
    ///   than the first interval decaying away.
    ///
    /// # Panics
    /// If `powers.len()` differs from the band count given to [`PowerAverager::new`].
    pub fn push(&mut self, powers: &[f64], duration_s: f64) -> &[f64] {
        assert_eq!(powers.len(), self.out.len(), "band count");
        let dt = if duration_s.is_finite() && duration_s > 0.0 {
            duration_s
        } else {
            0.0
        };
        match self.averaging {
            Averaging::Off => self.out.copy_from_slice(powers),
            Averaging::Fifo { frames } => {
                let mut slot = if self.fifo.len() == frames {
                    let (d, old) = self.fifo.pop_front().expect("frames ≥ 1");
                    for (s, o) in self.sum.iter_mut().zip(&old) {
                        *s -= d * o;
                    }
                    self.sum_s -= d;
                    old
                } else {
                    Vec::with_capacity(powers.len())
                };
                slot.clear();
                slot.extend_from_slice(powers);
                for (s, p) in self.sum.iter_mut().zip(powers) {
                    *s += dt * p;
                }
                self.sum_s += dt;
                self.fifo.push_back((dt, slot));
                self.since_resum += 1;
                if self.since_resum >= frames {
                    // Re-sum from the stored intervals once per FIFO length so the
                    // subtract/add rounding cannot drift without bound.
                    self.since_resum = 0;
                    self.sum.fill(0.0);
                    self.sum_s = 0.0;
                    for (d, p) in &self.fifo {
                        for (s, v) in self.sum.iter_mut().zip(p) {
                            *s += d * v;
                        }
                        self.sum_s += d;
                    }
                }
                if self.sum_s > 0.0 {
                    for (o, s) in self.out.iter_mut().zip(&self.sum) {
                        *o = s / self.sum_s;
                    }
                } else {
                    self.out.copy_from_slice(powers);
                }
            }
            Averaging::Exponential { time_constant_s } => {
                self.elapsed_s += dt;
                match &mut self.state {
                    None => self.state = Some(powers.to_vec()),
                    Some(s) => {
                        let start = if self.elapsed_s > 0.0 {
                            dt / self.elapsed_s
                        } else {
                            0.0
                        };
                        let a = (1.0 - (-dt / time_constant_s).exp()).max(start);
                        for (x, p) in s.iter_mut().zip(powers) {
                            *x += a * (p - *x);
                        }
                    }
                }
                if let Some(s) = &self.state {
                    self.out.copy_from_slice(s);
                }
            }
        }
        &self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rta::OctaveFilterBank;

    fn db(p: f64) -> f64 {
        10.0 * p.log10()
    }

    struct Noise(u64);
    impl Noise {
        fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f64 {
            let (u1, u2) = (self.uniform(), self.uniform());
            (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
        }
    }

    #[test]
    fn fifo_is_the_duration_weighted_power_mean_not_the_db_mean() {
        let mut a = PowerAverager::new(Averaging::Fifo { frames: 4 }, 1).expect("valid");
        // Equal halves at 0 dB and −20 dB: power mean −2.96 dB, dB mean −10 dB.
        for p in [1.0, 0.01, 1.0, 0.01] {
            a.push(&[p], 0.016);
        }
        let got = a.push(&[1.0], 0.016)[0];
        // The FIFO now holds 0.01, 1, 0.01, 1.
        assert!((got - 0.505).abs() < 1e-12, "{got}");
        assert!((db(got) + 2.967).abs() < 1e-3);
        // Unequal intervals count by their length: 3 s at 1, 1 s at 0.01.
        let mut a = PowerAverager::new(Averaging::Fifo { frames: 10 }, 1).expect("valid");
        a.push(&[1.0], 3.0);
        let got = a.push(&[0.01], 1.0)[0];
        assert!((got - (3.0 + 0.01) / 4.0).abs() < 1e-12, "{got}");
    }

    #[test]
    fn exponential_starts_as_the_power_mean_then_forgets() {
        let tau = 1.0;
        let mut a = PowerAverager::new(
            Averaging::Exponential {
                time_constant_s: tau,
            },
            1,
        )
        .expect("valid");
        // Well inside τ the average is the power mean so far, not the first interval.
        a.push(&[1.0], 0.01);
        let got = a.push(&[0.01], 0.01)[0];
        assert!((got - 0.505).abs() < 1e-12, "{got}");
        // Long after a step, it reads the new level.
        for _ in 0..2000 {
            a.push(&[0.01], 0.01);
        }
        let got = a.push(&[0.01], 0.01)[0];
        assert!((got / 0.01 - 1.0).abs() < 1e-6, "{got}");
        // Invalid parameters are refused.
        assert!(PowerAverager::new(Averaging::Fifo { frames: 0 }, 1).is_err());
        assert!(
            PowerAverager::new(
                Averaging::Exponential {
                    time_constant_s: 0.0
                },
                1
            )
            .is_err()
        );
        assert!(
            PowerAverager::new(
                Averaging::Exponential {
                    time_constant_s: f64::NAN
                },
                1
            )
            .is_err()
        );
    }

    /// Noise whose level jumps between 0 dB and −20 dB, read through the filterbank in
    /// unequally paced intervals: a FIFO spanning the record reads the bank's power over
    /// the whole record in every band (the power mean), several dB above the mean of the
    /// intervals' dB.
    #[test]
    fn fifo_over_a_fluctuating_noise_reads_its_whole_record_power() {
        let fs = 48_000.0;
        let mut rng = Noise(0x0AC2_A7E5);
        let x: Vec<f64> = (0..(fs as usize * 4))
            .map(|i| {
                // 0.25 s at full level, 0.25 s 20 dB down.
                let g = if (i / 12_000) % 2 == 0 { 0.3 } else { 0.03 };
                g * rng.gauss()
            })
            .collect();
        let mut whole = OctaveFilterBank::new(crate::rta::BandFraction::Third, fs, 100.0, 10_000.0)
            .expect("bank");
        whole.process(&x);
        let mut want = vec![0.0; whole.len()];
        whole.band_powers(&mut want);

        let mut bank = OctaveFilterBank::new(crate::rta::BandFraction::Third, fs, 100.0, 10_000.0)
            .expect("bank");
        let mut avg =
            PowerAverager::new(Averaging::Fifo { frames: 10_000 }, bank.len()).expect("valid");
        let mut p = vec![0.0; bank.len()];
        let mut db_sum = vec![0.0; bank.len()];
        let mut n = 0.0;
        let mut at = 0;
        let mut got = Vec::new();
        // Paced like results: intervals of 400 … 1100 samples.
        for k in 0.. {
            if at >= x.len() {
                break;
            }
            let len = (400 + (k * 337) % 700).min(x.len() - at);
            bank.process(&x[at..at + len]);
            at += len;
            let dur = bank.samples() as f64 / fs;
            bank.band_powers(&mut p);
            bank.reset_powers();
            got = avg.push(&p, dur).to_vec();
            for (s, v) in db_sum.iter_mut().zip(&p) {
                *s += db(*v);
            }
            n += 1.0;
        }
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            let err = db(*g) - db(*w);
            // The decimated stages count their own samples, a few per interval apart.
            assert!(err.abs() < 0.05, "band {i}: {err:.3} dB");
            // The intervals' dB mean reads several dB low.
            let db_mean = db_sum[i] / n;
            assert!(db(*w) - db_mean > 3.0, "band {i}: dB mean {db_mean:.2}");
        }
    }
}
