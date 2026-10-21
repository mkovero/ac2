//! s-domain prototypes, normalised to the section's f₀ (ω = 1 at f₀).

use super::{Alignment, BesselNorm, CORNER_DB, Pass, Sos};
use num_complex::Complex64;
use std::f64::consts::PI;

/// Symmetric (cookbook) peaking EQ `(s² + s·A/Q + 1) / (s² + s/(A·Q) + 1)`, `A = 10^{g/40}`:
/// a cut is the exact inverse of the equal boost.
pub(super) fn peaking_symmetric(gain_db: f64, q: f64) -> Sos {
    let a = 10f64.powf(gain_db / 40.0);
    Sos {
        b: [1.0, a / q, 1.0],
        a: [1.0, 1.0 / (a * q), 1.0],
    }
}

/// Band-pass-sum peaking `1 + (K−1)·BP(s)` for a boost (poles at damping 1/Q), its inverse
/// for a cut (zeros at damping 1/Q), `K = 10^{|g|/20}`.
pub(super) fn peaking_band_pass(gain_db: f64, q: f64) -> Sos {
    let k = 10f64.powf(gain_db.abs() / 20.0);
    let fixed = [1.0, 1.0 / q, 1.0];
    let moved = [1.0, k / q, 1.0];
    if gain_db >= 0.0 {
        Sos { b: moved, a: fixed }
    } else {
        Sos { b: fixed, a: moved }
    }
}

/// Cookbook second-order shelf; |H| = A (half the dB gain) at ω = 1.
pub(super) fn shelf_second(pass: Pass, gain_db: f64, q: f64) -> Sos {
    let a = 10f64.powf(gain_db / 40.0);
    let m = a.sqrt() / q;
    match pass {
        Pass::Low => Sos {
            b: [a, a * m, a * a],
            a: [a, m, 1.0],
        },
        Pass::High => Sos {
            b: [a * a, a * m, a],
            a: [1.0, m, a],
        },
    }
}

/// First-order shelf: pole and zero a factor K apart, their geometric mean at ω = 1, so
/// |H| is half the dB gain there.
pub(super) fn shelf_first(pass: Pass, gain_db: f64) -> Sos {
    let k = 10f64.powf(gain_db / 20.0);
    let r = k.sqrt();
    match pass {
        Pass::Low => Sos {
            b: [0.0, 1.0, r],
            a: [0.0, 1.0, 1.0 / r],
        },
        Pass::High => Sos {
            b: [0.0, k, r],
            a: [0.0, 1.0, r],
        },
    }
}

fn db_at(sos: &Sos, w: f64) -> f64 {
    20.0 * sos.at(Complex64::new(0.0, w)).norm().log10()
}

/// ω of the point [`CORNER_DB`] from flat on the flat side of a shelf designed with its
/// midpoint at ω = 1. Searched from the flat end inwards, so a shelf with overshoot takes
/// the crossing nearest the flat side; bisected on log ω.
pub(super) fn corner_ratio(pass: Pass, gain_db: f64, sos: &Sos) -> f64 {
    let target = CORNER_DB * gain_db.signum();
    let past = |w: f64| (db_at(sos, w) - target) * gain_db.signum() >= 0.0;
    // Flat side: high frequencies for a low shelf, low for a high shelf.
    let step = 2f64.powf(1.0 / 8.0);
    let (mut flat, dir) = match pass {
        Pass::Low => (1e6, 1.0 / step),
        Pass::High => (1e-6, step),
    };
    let mut inner = flat * dir;
    while !past(inner) {
        flat = inner;
        inner *= dir;
    }
    let (mut lo, mut hi) = (flat.ln(), inner.ln());
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if past(mid.exp()) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    (0.5 * (lo + hi)).exp()
}

fn butterworth(pass: Pass, order: u8) -> Vec<Sos> {
    let n = order as usize;
    let mut out = Vec::with_capacity(n.div_ceil(2));
    for k in 0..n / 2 {
        let theta = (2 * k + 1) as f64 * PI / (2 * n) as f64;
        let q = 1.0 / (2.0 * theta.sin());
        out.push(match pass {
            Pass::Low => Sos {
                b: [0.0, 0.0, 1.0],
                a: [1.0, 1.0 / q, 1.0],
            },
            Pass::High => Sos {
                b: [1.0, 0.0, 0.0],
                a: [1.0, 1.0 / q, 1.0],
            },
        });
    }
    if n % 2 == 1 {
        out.push(match pass {
            Pass::Low => Sos {
                b: [0.0, 0.0, 1.0],
                a: [0.0, 1.0, 1.0],
            },
            Pass::High => Sos {
                b: [0.0, 1.0, 0.0],
                a: [0.0, 1.0, 1.0],
            },
        });
    }
    out
}

/// Sections of a high- or low-pass prototype.
pub(super) fn pass(pass: Pass, alignment: Alignment, order: u8) -> Vec<Sos> {
    match alignment {
        Alignment::Butterworth => butterworth(pass, order),
        Alignment::LinkwitzRiley => {
            let half = butterworth(pass, order / 2);
            half.iter().chain(half.iter()).copied().collect()
        }
        Alignment::Bessel(norm) => bessel_poles(order, norm)
            .into_iter()
            .filter(|p| p.im >= 0.0)
            .map(|p| bessel_section(pass, p))
            .collect(),
    }
}

fn bessel_section(pass: Pass, p: Complex64) -> Sos {
    if p.im > 0.0 {
        let w2 = p.norm_sqr();
        let d = -2.0 * p.re; // ω/Q
        match pass {
            Pass::Low => Sos {
                b: [0.0, 0.0, w2],
                a: [1.0, d, w2],
            },
            // s → 1/s, scaled so the leading coefficient is 1.
            Pass::High => Sos {
                b: [1.0, 0.0, 0.0],
                a: [1.0, d / w2, 1.0 / w2],
            },
        }
    } else {
        let sigma = -p.re;
        match pass {
            Pass::Low => Sos {
                b: [0.0, 0.0, sigma],
                a: [0.0, 1.0, sigma],
            },
            Pass::High => Sos {
                b: [0.0, sigma, 0.0],
                a: [0.0, sigma, 1.0],
            },
        }
    }
}

/// Coefficients `a_k` (of `s^k`) of the reverse Bessel polynomial θₙ:
/// `a_k = (2n−k)! / (2^{n−k} k! (n−k)!)`.
fn reverse_bessel(n: usize) -> Vec<f64> {
    let fact = |m: usize| (1..=m).fold(1.0f64, |p, i| p * i as f64);
    (0..=n)
        .map(|k| fact(2 * n - k) / (2f64.powi((n - k) as i32) * fact(k) * fact(n - k)))
        .collect()
}

fn poly_at(c: &[f64], z: Complex64) -> Complex64 {
    c.iter()
        .rev()
        .fold(Complex64::new(0.0, 0.0), |acc, &ck| acc * z + ck)
}

/// Poles of the order-`n` Bessel low-pass prototype under a normalisation, conjugate pairs
/// exact and real poles real. The delay-normalised poles are the roots of θₙ (unit group
/// delay at DC); the phase normalisation divides them by `a₀^{1/n}` (the asymptotic phase
/// then matches the Butterworth's); −3 dB divides them by the delay-normalised −3 dB
/// frequency.
pub fn bessel_poles(n: u8, norm: BesselNorm) -> Vec<Complex64> {
    let n = n as usize;
    let c = reverse_bessel(n);
    // Durand–Kerner on the monic θₙ (a_n = 1), started on a circle of the roots' mean
    // magnitude; Bessel roots are simple, so it converges quadratically.
    let r = c[0].powf(1.0 / n as f64);
    let seed = Complex64::new(0.4, 0.9);
    let mut z: Vec<Complex64> = (0..n).map(|k| seed.powu(k as u32) * r).collect();
    for _ in 0..500 {
        let mut delta = 0.0f64;
        for i in 0..n {
            let mut den = Complex64::new(1.0, 0.0);
            for j in 0..n {
                if i != j {
                    den *= z[i] - z[j];
                }
            }
            let step = poly_at(&c, z[i]) / den;
            z[i] -= step;
            delta = delta.max(step.norm());
        }
        if delta < 1e-15 * r {
            break;
        }
    }
    // Polish with Newton steps and make conjugate symmetry exact.
    let dc: Vec<f64> = (1..=n).map(|k| c[k] * k as f64).collect();
    for p in z.iter_mut() {
        for _ in 0..3 {
            *p -= poly_at(&c, *p) / poly_at(&dc, *p);
        }
        if p.im.abs() < 1e-9 * r {
            p.im = 0.0;
        }
    }
    let mut poles: Vec<Complex64> = z.iter().filter(|p| p.im > 0.0).copied().collect();
    poles.extend(z.iter().filter(|p| p.im == 0.0).copied());
    let pairs: Vec<Complex64> = poles
        .iter()
        .filter(|p| p.im > 0.0)
        .map(|p| p.conj())
        .collect();
    poles.extend(pairs);
    poles.sort_by(|a, b| a.im.total_cmp(&b.im).then(a.re.total_cmp(&b.re)));
    let scale = match norm {
        BesselNorm::Delay => 1.0,
        BesselNorm::Phase => c[0].powf(1.0 / n as f64),
        BesselNorm::Minus3db => {
            let mag2 = |w: f64| {
                poles
                    .iter()
                    .map(|p| p.norm_sqr() / (Complex64::new(0.0, w) - p).norm_sqr())
                    .product::<f64>()
            };
            let (mut lo, mut hi) = (1e-3f64.ln(), 1e3f64.ln());
            for _ in 0..200 {
                let mid = 0.5 * (lo + hi);
                if mag2(mid.exp()) > 0.5 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            (0.5 * (lo + hi)).exp()
        }
    };
    poles.iter().map(|p| p / scale).collect()
}
