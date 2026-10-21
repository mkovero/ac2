//! A generated target curve's columns (`trace.target`), from its parameters: the daemon
//! stores them as a target trace; the client's fake daemon makes the same.

use ac2_core::target::{iso226, lf_shelf_db, tilt_db};
use ac2_proto::GridDef;
use ac2_proto::model::TargetCurve;

use crate::columns::{Columns, resample};

/// Points per octave of a generated curve: the default measurement grid, so a target and
/// a capture are compared column for column.
pub const PPO: u32 = 48;
/// The span a flat or tilted curve covers, Hz: the audio band.
pub const SPAN_HZ: (f64, f64) = (20.0, 20_000.0);

/// The grid and magnitude columns of `curve`, or why its parameters are refused
/// ([`TargetCurve::check`]).
pub fn generate(curve: &TargetCurve) -> Result<(GridDef, Columns), String> {
    curve.check()?;
    let (lo, hi) = match curve {
        TargetCurve::LoudnessShift { .. } => (iso226::FREQS[0], iso226::FREQS[28]),
        _ => SPAN_HZ,
    };
    let grid = log_grid(lo, hi);
    let freqs = grid.centres();
    let mag: Vec<f64> = match *curve {
        TargetCurve::Flat => vec![0.0; freqs.len()],
        TargetCurve::Tilt {
            slope_db_per_oct,
            pivot,
            lf_shelf,
        } => {
            let shelf = match lf_shelf {
                Some(s) => lf_shelf_db(&freqs, s.gain.0, s.corner.0)
                    .ok_or_else(|| "the shelf cannot be made".to_string())?,
                None => vec![0.0; freqs.len()],
            };
            freqs
                .iter()
                .zip(&shelf)
                .map(|(f, s)| tilt_db(*f, slope_db_per_oct, pivot.0) + s)
                .collect()
        }
        TargetCurve::LoudnessShift { from, to } => {
            // The standard gives the contours at its 29 frequencies only: between them the
            // curve is a straight line on a log axis, as an imported table of the same
            // points would be.
            let shift = iso226::loudness_shift(from.0, to.0);
            let at = Columns {
                mag_db: shift.iter().map(|v| *v as f32).collect(),
                phase_deg: None,
                coherence: None,
            };
            return Ok((grid, resample(&iso226::FREQS, &at, &freqs)));
        }
    };
    Ok((
        grid,
        Columns {
            mag_db: mag.iter().map(|v| *v as f32).collect(),
            phase_deg: None,
            coherence: None,
        },
    ))
}

/// The columns of the [`PPO`] log grid inside `lo … hi`.
fn log_grid(lo: f64, hi: f64) -> GridDef {
    let p = f64::from(PPO);
    // The tolerance keeps an edge that falls on a column (20 Hz is not one; 1 kHz is).
    GridDef::Log {
        ppo: PPO,
        k_min: (p * (lo / 1000.0).log2() - 1e-9).ceil() as i32,
        k_max: (p * (hi / 1000.0).log2() + 1e-9).floor() as i32,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ac2_proto::model::LfShelf;
    use ac2_proto::units::{Db, Hz, Phon};

    fn at(grid: &GridDef, c: &Columns, f: f64) -> f32 {
        let freqs = grid.centres();
        let i = freqs
            .iter()
            .position(|x| (x - f).abs() < 1e-6)
            .expect("on the grid");
        c.mag_db[i]
    }

    #[test]
    fn flat_and_tilt_span_the_audio_band_on_the_measurement_grid() {
        let (g, c) = generate(&TargetCurve::Flat).unwrap();
        let f = g.centres();
        assert!(f[0] >= 20.0 && f[0] < 20.0 * 2f64.powf(1.0 / 48.0));
        assert!(*f.last().unwrap() <= 20_000.0);
        assert!(c.mag_db.iter().all(|v| *v == 0.0));
        assert!(c.phase_deg.is_none() && c.coherence.is_none());

        let (g, c) = generate(&TargetCurve::house_tilt()).unwrap();
        assert_eq!(at(&g, &c, 1000.0), 0.0);
        assert!((at(&g, &c, 4000.0) + 2.0).abs() < 1e-5);
        assert!((at(&g, &c, 250.0) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn a_shelf_adds_to_the_tilt() {
        let curve = TargetCurve::Tilt {
            slope_db_per_oct: -1.0,
            pivot: Hz(1000.0),
            lf_shelf: Some(LfShelf {
                gain: Db(6.0),
                corner: Hz(125.0),
            }),
        };
        let (g, c) = generate(&curve).unwrap();
        // 125 Hz: three octaves below the pivot, plus half the shelf.
        assert!((at(&g, &c, 125.0) - 6.0).abs() < 1e-4);
        assert!(at(&g, &c, 8000.0) + 3.0 < 0.01);
    }

    #[test]
    fn a_loudness_shift_covers_the_contours_frequencies_and_passes_through_their_values() {
        let (g, c) = generate(&TargetCurve::loudness_shift()).unwrap();
        let f = g.centres();
        assert!(f[0] >= 20.0 && *f.last().unwrap() <= 12_500.0);
        let s = iso226::loudness_shift(85.0, 100.0);
        // 1 kHz, 125 Hz and 8 kHz are grid columns: the shift itself there.
        for (hz, i) in [(1000.0, 17), (125.0, 8), (8000.0, 26)] {
            assert!((f64::from(at(&g, &c, hz)) - s[i]).abs() < 1e-5, "{hz} Hz");
        }
    }

    #[test]
    fn refused_parameters_say_why() {
        let e = generate(&TargetCurve::LoudnessShift {
            from: Phon(90.0),
            to: Phon(90.0),
        })
        .unwrap_err();
        assert!(e.contains("same"), "{e}");
    }
}
