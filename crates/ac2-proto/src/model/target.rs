//! Target curves the daemon generates (`trace.target`) instead of reading them from a file.

use serde::{Deserialize, Serialize};

use crate::units::{Db, Hz, Phon};

/// A generated target curve: its shape and the parameters it was made from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetCurve {
    /// 0 dB at every frequency.
    Flat,
    /// A straight slope on a log-frequency axis, 0 dB at the pivot, optionally with a
    /// low-frequency shelf added.
    Tilt {
        /// Slope, dB per octave (negative: falling toward the top).
        slope_db_per_oct: f64,
        /// Where the tilt is 0 dB.
        pivot: Hz,
        /// A low shelf on top of the tilt.
        lf_shelf: Option<LfShelf>,
    },
    /// How the perceived tonal balance shifts when the playback level changes from
    /// `from` to `to` (equal-loudness contours, ISO 226:2003), 0 dB at 1 kHz. A view of
    /// perception, not a response to equalise a system to: the ear applies it itself.
    LoudnessShift {
        /// The level the programme was balanced at.
        from: Phon,
        /// The level it is played at.
        to: Phon,
    },
}

/// A second-order low shelf (the cookbook shelf at slope S = 1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LfShelf {
    /// Gain below the corner (positive: more low end).
    pub gain: Db,
    /// Where the shelf has half its gain in dB.
    pub corner: Hz,
}

impl TargetCurve {
    /// Steepest tilt, dB per octave either way.
    pub const MAX_SLOPE_DB_PER_OCT: f64 = 6.0;
    /// Pivot range, Hz.
    pub const PIVOT_HZ: (f64, f64) = (20.0, 20_000.0);
    /// Largest shelf gain either way, dB.
    pub const MAX_SHELF_DB: f64 = 20.0;
    /// Shelf corner range, Hz: a low-frequency shelf.
    pub const SHELF_CORNER_HZ: (f64, f64) = (20.0, 1000.0);
    /// Loudness level range, phon: 20 phon is the lowest contour the measurements under
    /// the standard support; above 100 phon there are none.
    pub const PHON: (f64, f64) = (20.0, 100.0);

    /// The default house tilt: −1 dB/oct about 1 kHz, no shelf.
    pub fn house_tilt() -> Self {
        Self::Tilt {
            slope_db_per_oct: -1.0,
            pivot: Hz(1000.0),
            lf_shelf: None,
        }
    }

    /// The default loudness shift: balanced at 85 phon, played at 100.
    pub fn loudness_shift() -> Self {
        Self::LoudnessShift {
            from: Phon(85.0),
            to: Phon(100.0),
        }
    }

    /// Whether the daemon can make this curve; the reason when not.
    pub fn check(&self) -> Result<(), String> {
        let within = |v: f64, (lo, hi): (f64, f64)| v.is_finite() && (lo..=hi).contains(&v);
        match *self {
            Self::Flat => Ok(()),
            Self::Tilt {
                slope_db_per_oct,
                pivot,
                lf_shelf,
            } => {
                let m = Self::MAX_SLOPE_DB_PER_OCT;
                if !within(slope_db_per_oct, (-m, m)) {
                    return Err(format!("slope: −{m} … {m} dB/oct"));
                }
                let (lo, hi) = Self::PIVOT_HZ;
                if !within(pivot.0, Self::PIVOT_HZ) {
                    return Err(format!("pivot: {lo} Hz … {} kHz", hi / 1000.0));
                }
                if let Some(s) = lf_shelf {
                    let g = Self::MAX_SHELF_DB;
                    if !within(s.gain.0, (-g, g)) {
                        return Err(format!("shelf gain: −{g} … {g} dB"));
                    }
                    let (lo, hi) = Self::SHELF_CORNER_HZ;
                    if !within(s.corner.0, Self::SHELF_CORNER_HZ) {
                        return Err(format!("shelf corner: {lo} … {hi} Hz"));
                    }
                }
                Ok(())
            }
            Self::LoudnessShift { from, to } => {
                let (lo, hi) = Self::PHON;
                if !within(from.0, Self::PHON) || !within(to.0, Self::PHON) {
                    return Err(format!("loudness levels: {lo} … {hi} phon"));
                }
                if from == to {
                    return Err("the two loudness levels are the same: no shift".into());
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_valid_and_out_of_range_parameters_are_refused() {
        for c in [
            TargetCurve::Flat,
            TargetCurve::house_tilt(),
            TargetCurve::loudness_shift(),
        ] {
            assert_eq!(c.check(), Ok(()));
        }
        let tilt = |slope, pivot, lf_shelf| TargetCurve::Tilt {
            slope_db_per_oct: slope,
            pivot: Hz(pivot),
            lf_shelf,
        };
        assert!(tilt(-7.0, 1000.0, None).check().is_err());
        assert!(tilt(f64::NAN, 1000.0, None).check().is_err());
        assert!(tilt(-1.0, 10.0, None).check().is_err());
        let shelf = |gain, corner| {
            Some(LfShelf {
                gain: Db(gain),
                corner: Hz(corner),
            })
        };
        assert_eq!(tilt(-1.0, 1000.0, shelf(6.0, 100.0)).check(), Ok(()));
        assert!(tilt(-1.0, 1000.0, shelf(25.0, 100.0)).check().is_err());
        assert!(tilt(-1.0, 1000.0, shelf(6.0, 5000.0)).check().is_err());
        let loud = |a, b| TargetCurve::LoudnessShift {
            from: Phon(a),
            to: Phon(b),
        };
        assert!(loud(85.0, 85.0).check().is_err());
        assert!(loud(10.0, 85.0).check().is_err());
        assert!(loud(85.0, 110.0).check().is_err());
    }
}
