//! Typed output levels: the global sample-peak limit and generator gain.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A level that was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum LevelError {
    /// NaN or infinite.
    #[error("level is not a finite number")]
    NotFinite,
    /// Above full scale. Full scale is the hard ceiling of a converter; a limit above it
    /// would mean "no limit".
    #[error("{db} dB is above full scale (0 dB)")]
    AboveFullScale {
        /// The requested value in dB.
        db: f64,
    },
}

fn checked_db(db: f64) -> Result<f64, LevelError> {
    if !db.is_finite() {
        Err(LevelError::NotFinite)
    } else if db > 0.0 {
        Err(LevelError::AboveFullScale { db })
    } else {
        Ok(db)
    }
}

/// Global maximum for every sample the output path writes, as a sample-peak level in dB
/// re full scale (±1.0).
///
/// The output path can only see sample values, so this is a peak limit. Operator-facing
/// levels are RMS (decision 4a, 0 dBFS = RMS of a full-scale sine); converting an RMS
/// maximum to a peak limit needs the signal's crest factor and is the daemon's job. The
/// generator should never reach this limit: hitting it means a level computation upstream
/// was wrong, so every limited sample is counted and flagged, never silent.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaxLevel {
    db: f64,
}

impl MaxLevel {
    /// Limit at `db` dB sample peak re full scale; must be finite and ≤ 0.
    pub fn from_peak_db(db: f64) -> Result<Self, LevelError> {
        checked_db(db).map(|db| Self { db })
    }

    /// The limit in dB.
    pub fn peak_db(self) -> f64 {
        self.db
    }

    /// The limit as a linear sample magnitude.
    pub fn linear(self) -> f32 {
        10f64.powf(self.db / 20.0) as f32
    }

    /// Whether a signal of linear sample peak `peak` stays within the limit. Compared in f64:
    /// a signal planned to sit exactly at the limit (a level at its ceiling) must pass, and the
    /// f32 of [`Self::linear`] can round below the f64 peak by up to 6e-8 relative.
    pub fn admits_peak(self, peak: f64) -> bool {
        peak <= 10f64.powf(self.db / 20.0) * (1.0 + 1e-9)
    }
}

/// Generator gain, ≤ 0 dB. Applied before the [`MaxLevel`] limit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gain {
    linear: f32,
}

impl Gain {
    /// Unity gain.
    pub const UNITY: Self = Self { linear: 1.0 };
    /// Muted.
    pub const MUTE: Self = Self { linear: 0.0 };

    /// Gain of `db` dB; must be finite and ≤ 0.
    pub fn from_db(db: f64) -> Result<Self, LevelError> {
        checked_db(db).map(|db| Self {
            linear: 10f64.powf(db / 20.0) as f32,
        })
    }

    /// Linear factor in 0..=1.
    pub fn linear(self) -> f32 {
        self.linear
    }

    pub(crate) fn from_bits(bits: u32) -> Self {
        Self {
            linear: f32::from_bits(bits),
        }
    }

    pub(crate) fn to_bits(self) -> u32 {
        self.linear.to_bits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peak_exactly_at_the_limit_is_admitted() {
        // f32 rounds 10^(-50/20) down: the peak of a signal planned at the limit must still pass
        for db in [-60.0, -50.0, -40.0, -30.0, -20.0, -6.0, 0.0] {
            let m = MaxLevel::from_peak_db(db).expect("level");
            let peak = 10f64.powf(db / 20.0);
            assert!(m.admits_peak(peak), "{db} dB");
            assert!(!m.admits_peak(peak * (1.0 + 1e-6)), "{db} dB");
        }
        let m = MaxLevel::from_peak_db(-50.0).expect("level");
        assert!(10f64.powf(-50.0 / 20.0) > f64::from(m.linear()));
        // pink noise at its ceiling: rms(ceiling) × the clamped crest of 6
        let rms = 10f64.powf(-50.0 / 20.0) / 6.0;
        assert!(m.admits_peak(rms * 6.0));
    }

    #[test]
    fn levels_reject_non_finite_and_above_full_scale() {
        assert_eq!(MaxLevel::from_peak_db(f64::NAN), Err(LevelError::NotFinite));
        assert_eq!(
            MaxLevel::from_peak_db(0.5),
            Err(LevelError::AboveFullScale { db: 0.5 })
        );
        assert!(Gain::from_db(f64::INFINITY).is_err());
        assert!(Gain::from_db(1.0).is_err());
        let m = MaxLevel::from_peak_db(-20.0).expect("valid");
        assert!((m.linear() - 0.1).abs() < 1e-7);
        assert_eq!(Gain::from_db(0.0).map(Gain::linear), Ok(1.0));
        assert_eq!(Gain::from_bits(Gain::UNITY.to_bits()), Gain::UNITY);
    }
}
