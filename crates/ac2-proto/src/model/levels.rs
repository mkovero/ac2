//! Programme-meter settings of the rig (docs/design/programme-meter.md §3, §4, §7): which
//! ballistic the bars read, the correlator's window, the over threshold and the correlation
//! gate. The daemon computes these quantities, so the choice is the rig's (`levels.set`),
//! not an app preference: two apps on one rig read the same bar.

use serde::{Deserialize, Serialize};

use crate::units::{Dbfs, Dbtp};

/// The meter's dynamics. A typed choice: the standards' types, their constants in
/// `ac2_core::ppm`. Scales (Nordic, DIN, BBC, EBU) are faces on these detectors and are not
/// shipped: a bar reads dBFS or dBu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ballistic {
    /// Sample peak through 4× oversampling (BS.1770 Annex 2): what the converter or the
    /// link will actually see. The default.
    #[default]
    TruePeak,
    /// IEC 60268-10 Type I (the Nordic and DIN scales): quasi-peak, 5 ms integration,
    /// 20 dB return in 1.7 s.
    Type1,
    /// IEC 60268-10 Type II (the BBC and EBU scales): quasi-peak, 10 ms integration,
    /// 24 dB return in 2.8 s.
    Type2,
    /// IEC 60268-17 VU: rectified average, 300 ms to 99 % of a steady tone. Offered on a
    /// calibrated `Line` input only ([`crate::cal::offers_vu`]): 0 VU is +4 dBu, which
    /// needs the volt.
    Vu,
}

impl Ballistic {
    /// Every ballistic, in the order a chooser lists them.
    pub const ALL: [Self; 4] = [Self::TruePeak, Self::Type1, Self::Type2, Self::Vu];

    /// The word every client uses for it: `true peak`, `Type I`, `Type II`, `VU`.
    pub fn word(self) -> &'static str {
        match self {
            Self::TruePeak => "true peak",
            Self::Type1 => "Type I",
            Self::Type2 => "Type II",
            Self::Vu => "VU",
        }
    }

    /// The unit its reading is in: dBTP for the true peak, dBFS otherwise (a client converts
    /// to dBu from the input's line calibration).
    pub fn unit(self) -> &'static str {
        match self {
            Self::TruePeak => "dBTP",
            Self::Type1 | Self::Type2 | Self::Vu => "dBFS",
        }
    }
}

/// The correlator's exponential window. Shown in seconds, never as *Fast* / *Slow*: those
/// words mean 125 ms and 1 s on the SPL meter, and one word must not read as two quantities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrelationWindow {
    /// 0.3 s.
    Short,
    /// 1 s. The default.
    #[default]
    Long,
}

impl CorrelationWindow {
    /// Both windows, short first.
    pub const ALL: [Self; 2] = [Self::Short, Self::Long];

    /// The window, seconds.
    pub fn seconds(self) -> f32 {
        match self {
            Self::Short => 0.3,
            Self::Long => 1.0,
        }
    }

    /// As shown: `0.3 s` / `1 s`.
    pub fn text(self) -> &'static str {
        match self {
            Self::Short => "0.3 s",
            Self::Long => "1 s",
        }
    }
}

/// Over thresholds a rig may set, dBTP: below −20 every note is an over, above +6 none can be
/// (the true-peak interpolator rarely exceeds a sample peak by more).
pub const OVER_THRESHOLD_RANGE: (f64, f64) = (-20.0, 6.0);
/// Correlation gates a rig may set, dBFS (RMS of a side).
pub const CORRELATION_GATE_RANGE: (f64, f64) = (-120.0, 0.0);

/// The rig's programme-meter settings, kept in its rig settings file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelsSettings {
    /// What the `ppm` bar reads; the daemon runs every ballistic and reports this one.
    pub ballistic: Ballistic,
    /// The correlator's window.
    pub correlation_window: CorrelationWindow,
    /// An over is a frame's `true_peak` above this (R128 practice: −1 dBTP), and a new
    /// event only after it fell 1 dB below it.
    pub over_threshold: Dbtp,
    /// A side below this reads *quiet* and the pair carries no correlation: a dead channel
    /// must not read 0 and look like wide stereo.
    pub correlation_gate: Dbfs,
}

impl Default for LevelsSettings {
    fn default() -> Self {
        Self {
            ballistic: Ballistic::TruePeak,
            correlation_window: CorrelationWindow::Long,
            over_threshold: Dbtp(0.0),
            correlation_gate: Dbfs(-60.0),
        }
    }
}

impl LevelsSettings {
    /// A threshold and a gate within their ranges and finite; the message names the one
    /// that is not.
    pub fn check(&self) -> Result<(), String> {
        let t = self.over_threshold.0;
        if !t.is_finite() || t < OVER_THRESHOLD_RANGE.0 || t > OVER_THRESHOLD_RANGE.1 {
            return Err(format!(
                "over threshold {t} dBTP is outside {} … +{} dBTP",
                OVER_THRESHOLD_RANGE.0, OVER_THRESHOLD_RANGE.1
            ));
        }
        let g = self.correlation_gate.0;
        if !g.is_finite() || g < CORRELATION_GATE_RANGE.0 || g > CORRELATION_GATE_RANGE.1 {
            return Err(format!(
                "correlation gate {g} dBFS is outside {} … {} dBFS",
                CORRELATION_GATE_RANGE.0, CORRELATION_GATE_RANGE.1
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_and_windows_are_what_the_design_says() {
        assert_eq!(Ballistic::TruePeak.word(), "true peak");
        assert_eq!(Ballistic::Type1.word(), "Type I");
        assert_eq!(Ballistic::Type2.word(), "Type II");
        assert_eq!(Ballistic::Vu.word(), "VU");
        assert_eq!(CorrelationWindow::Short.seconds(), 0.3);
        assert_eq!(CorrelationWindow::Long.seconds(), 1.0);
        assert_eq!(CorrelationWindow::Short.text(), "0.3 s");
        assert_eq!(CorrelationWindow::Long.text(), "1 s");
        let d = LevelsSettings::default();
        assert_eq!(d.ballistic, Ballistic::TruePeak);
        assert_eq!(d.correlation_window, CorrelationWindow::Long);
        assert_eq!(d.over_threshold, Dbtp(0.0));
        assert_eq!(d.correlation_gate, Dbfs(-60.0));
        assert!(d.check().is_ok());
    }

    #[test]
    fn absurd_values_are_refused_by_name() {
        let t = LevelsSettings {
            over_threshold: Dbtp(7.0),
            ..Default::default()
        };
        assert!(
            t.check()
                .expect_err("out of range")
                .starts_with("over threshold")
        );
        let nan = LevelsSettings {
            over_threshold: Dbtp(f64::NAN),
            ..Default::default()
        };
        assert!(nan.check().is_err());
        let g = LevelsSettings {
            correlation_gate: Dbfs(1.0),
            ..Default::default()
        };
        assert!(
            g.check()
                .expect_err("out of range")
                .starts_with("correlation gate")
        );
        let b = rmp_serde::to_vec_named(&Ballistic::TruePeak).expect("encode");
        assert_eq!(
            rmp_serde::from_slice::<String>(&b).expect("decode"),
            "true_peak"
        );
    }
}
