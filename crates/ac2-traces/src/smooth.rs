//! Display smoothing of stored transfer and spectrum traces.
//!
//! The store keeps every trace unsmoothed, so its smoothing can change at any time and
//! averaging / A−B combine the measured columns. The smoothing a trace shows is applied
//! here, when its data is served, with the same kernel the live job uses
//! ([`ac2_core::smoothing`]): a capture re-smoothed at the measurement's setting reads the
//! same as the live curve it was taken from.

use ac2_core::grid::LogGrid;
use ac2_core::smoothing::{self as core, LinearSmoother, Smoother, TfColumns};
use ac2_proto::GridDef;
use ac2_proto::model::{Smoothing, SmoothingFraction, SmoothingMode, TraceKind};
use num_complex::Complex64;

use crate::columns::Columns;

/// The DSP crate's fraction and mode for a protocol [`Smoothing`].
pub fn core_smoothing(s: Smoothing) -> (core::SmoothingFraction, core::SmoothingMode) {
    use core::{SmoothingFraction as F, SmoothingMode as M};
    let f = match s.fraction {
        SmoothingFraction::Third => F::Third,
        SmoothingFraction::Sixth => F::Sixth,
        SmoothingFraction::Twelfth => F::Twelfth,
        SmoothingFraction::TwentyFourth => F::TwentyFourth,
        SmoothingFraction::FortyEighth => F::FortyEighth,
    };
    let m = match s.mode {
        SmoothingMode::Magnitude => M::Magnitude,
        SmoothingMode::MagnitudePhase => M::MagnitudePhase,
    };
    (f, m)
}

/// Whether a trace of `kind` can be smoothed: transfer functions and narrowband spectra.
/// Targets are specified curves, and RTA bands already are fractional-octave band powers.
/// A smoothed spectrum is labelled as such wherever it is shown: it no longer reads as the
/// tone level of a bin.
pub fn smoothable(kind: TraceKind) -> bool {
    matches!(
        kind,
        TraceKind::Transfer | TraceKind::Sweep | TraceKind::Spectrum { .. }
    )
}

/// `c` (on `grid`) smoothed by `s`. Log grids (transfer traces) are smoothed as transfer
/// functions, linear grids (spectra) as power on their bins; other grids come back
/// unchanged. NaN columns are gaps: smoothing never crosses them and they stay NaN.
/// Coherence is never smoothed (it is the trust indicator).
pub fn smooth(grid: &GridDef, c: &Columns, s: Smoothing) -> Columns {
    let (ppo, k_min, k_max) = match *grid {
        GridDef::Log { ppo, k_min, k_max } => (ppo, k_min, k_max),
        GridDef::Linear { n, .. } => return smooth_linear(n as usize / 2 + 1, c, s),
        _ => return c.clone(),
    };
    let g = LogGrid { ppo, k_min, k_max };
    if g.len() != c.len() || !c.consistent() {
        return c.clone();
    }
    let (fraction, mode) = core_smoothing(s);
    let phase = c.phase_deg.as_deref();
    let h: Vec<Complex64> = c
        .mag_db
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let p = phase.map_or(0.0, |p| f64::from(p[i]).to_radians());
            Complex64::from_polar(10f64.powf(f64::from(*m) / 20.0), p)
        })
        .collect();
    let valid: Vec<bool> = h
        .iter()
        .map(|z| z.re.is_finite() && z.im.is_finite())
        .collect();
    let coherence = vec![0.0; c.len()];
    let out = Smoother::new(g, fraction).smooth(
        TfColumns {
            h: &h,
            coherence: &coherence,
            valid: &valid,
        },
        mode,
    );
    let pick = |f: &dyn Fn(Complex64) -> f64| -> Vec<f32> {
        out.h
            .iter()
            .zip(&out.valid)
            .map(|(z, ok)| if *ok { f(*z) as f32 } else { f32::NAN })
            .collect()
    };
    Columns {
        mag_db: pick(&|z| 20.0 * z.norm().log10()),
        phase_deg: c
            .phase_deg
            .as_ref()
            .map(|_| pick(&|z| z.arg().to_degrees())),
        coherence: c.coherence.clone(),
    }
}

/// A spectrum's levels (dB) power-smoothed on its `bins` linear bins. A spectrum has no
/// phase, so the mode does not matter.
fn smooth_linear(bins: usize, c: &Columns, s: Smoothing) -> Columns {
    if c.len() != bins || !c.consistent() {
        return c.clone();
    }
    let (fraction, _) = core_smoothing(s);
    let power: Vec<f64> = c
        .mag_db
        .iter()
        .map(|l| 10f64.powf(f64::from(*l) / 10.0))
        .collect();
    let valid: Vec<bool> = c.mag_db.iter().map(|l| l.is_finite()).collect();
    let out = LinearSmoother::new(bins, fraction).smooth(&power, &valid);
    Columns {
        mag_db: out
            .iter()
            .zip(&valid)
            .map(|(p, ok)| {
                if *ok {
                    (10.0 * p.log10()) as f32
                } else {
                    f32::NAN
                }
            })
            .collect(),
        phase_deg: c.phase_deg.clone(),
        coherence: c.coherence.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn grid() -> GridDef {
        GridDef::Log {
            ppo: 48,
            k_min: -96,
            k_max: 95,
        }
    }

    fn sixth() -> Smoothing {
        Smoothing {
            fraction: SmoothingFraction::Sixth,
            mode: SmoothingMode::Magnitude,
        }
    }

    #[test]
    fn a_flat_curve_stays_flat_and_gaps_stay_gaps() {
        let mut mag = vec![-6.0f32; 192];
        mag[100] = f32::NAN;
        let c = Columns {
            mag_db: mag,
            phase_deg: Some(vec![30.0; 192]),
            coherence: Some(vec![0.5; 192]),
        };
        let s = smooth(&grid(), &c, sixth());
        assert!(s.mag_db[100].is_nan());
        assert!(s.phase_deg.as_ref().unwrap()[100].is_nan());
        for (i, v) in s.mag_db.iter().enumerate().filter(|(i, _)| *i != 100) {
            assert!((v + 6.0).abs() < 1e-4, "{i}: {v}");
        }
        // Power mode leaves phase as measured; coherence is never smoothed.
        assert!((s.phase_deg.unwrap()[3] - 30.0).abs() < 1e-4);
        assert_eq!(s.coherence, c.coherence);
    }

    #[test]
    fn a_narrow_peak_is_spread() {
        let mut mag = vec![0.0f32; 192];
        mag[96] = 20.0;
        let c = Columns {
            mag_db: mag,
            phase_deg: None,
            coherence: None,
        };
        let s = smooth(&grid(), &c, sixth());
        // Power 100 spread over a kernel of ≈ 6 columns: ≈ 12.4 dB.
        assert!((s.mag_db[96] - 12.4).abs() < 0.5, "{}", s.mag_db[96]);
        assert!(s.mag_db[98] > 0.1);
        assert!(s.phase_deg.is_none());
    }

    /// A spectrum is smoothed as power with the core linear-bin kernel; gaps stay gaps.
    #[test]
    fn a_spectrum_is_power_smoothed_on_its_bins() {
        let g = GridDef::Linear {
            fs: ac2_proto::units::Hz(48_000.0),
            n: 4096,
        };
        let mut mag: Vec<f32> = (0..2049).map(|k| -60.0 + 20.0 * ((k % 3) as f32)).collect();
        mag[1000] = f32::NAN;
        let c = Columns {
            mag_db: mag.clone(),
            phase_deg: None,
            coherence: None,
        };
        let s = smooth(&g, &c, sixth());
        assert!(s.mag_db[1000].is_nan());
        let power: Vec<f64> = mag
            .iter()
            .map(|l| 10f64.powf(f64::from(*l) / 10.0))
            .collect();
        let valid: Vec<bool> = mag.iter().map(|l| l.is_finite()).collect();
        let want = LinearSmoother::new(2049, core::SmoothingFraction::Sixth).smooth(&power, &valid);
        for k in (1..2049).filter(|k| *k != 1000) {
            let w = (10.0 * want[k].log10()) as f32;
            assert!(
                (s.mag_db[k] - w).abs() < 1e-4,
                "bin {k}: {} vs {w}",
                s.mag_db[k]
            );
        }
        // High up the comb of -60 / -40 / -20 dB averages to its mean power.
        let mean = 10.0 * ((1e-6 + 1e-4 + 1e-2) / 3.0f64).log10();
        assert!(
            (f64::from(s.mag_db[1800]) - mean).abs() < 0.05,
            "{}",
            s.mag_db[1800]
        );
        // Mode does not matter without phase.
        let m = Smoothing {
            mode: SmoothingMode::MagnitudePhase,
            ..sixth()
        };
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&smooth(&g, &c, m).mag_db), bits(&s.mag_db));
    }

    #[test]
    fn other_grids_pass_through() {
        let g = GridDef::Linear {
            fs: ac2_proto::units::Hz(48_000.0),
            n: 8,
        };
        let c = Columns {
            mag_db: vec![1.0, 9.0, 1.0, 9.0, 1.0, 3.0],
            phase_deg: None,
            coherence: None,
        };
        // Column count other than the grid's bins: passed through.
        assert_eq!(smooth(&g, &c, sixth()), c);
        assert!(!smoothable(TraceKind::Rta {
            scale: ac2_proto::model::LevelScale::Dbfs
        }));
        assert!(smoothable(TraceKind::Spectrum {
            scale: ac2_proto::model::LevelScale::Dbfs
        }));
    }
}
