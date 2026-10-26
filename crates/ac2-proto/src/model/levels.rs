//! Programme-meter settings of the rig (docs/design/programme-meter.md §3, §4, §7): which
//! ballistic the bars read, the correlator's window, the over threshold and the correlation
//! gate. The daemon computes these quantities, so the choice is the rig's (`levels.set`),
//! not an app preference: two apps on one rig read the same bar.

use serde::{Deserialize, Serialize};

use crate::units::{Dbfs, Dbtp, Lufs};

/// What a bar reads: one of the peak detectors (the standards' types, their constants in
/// `ac2_core::ppm`) or a loudness window of the bar's source. Scales (Nordic, DIN, BBC, EBU)
/// are faces on the peak detectors and are not shipped: a peak bar reads dBFS or dBu.
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
    /// BS.1770 momentary loudness of the bar's source (400 ms), LUFS. Line and digital
    /// sources only; an input without a source reads its true peak.
    Momentary,
    /// BS.1770 short-term loudness of the bar's source (3 s), LUFS.
    ShortTerm,
}

impl Ballistic {
    /// The detectors: what each input runs, whatever the bars read. Per-input arrays are
    /// sized and indexed by this list.
    pub const PEAK: [Self; 4] = [Self::TruePeak, Self::Type1, Self::Type2, Self::Vu];

    /// What a chooser steps: the detectors, then the two loudness windows.
    pub const ALL: [Self; 6] = [
        Self::TruePeak,
        Self::Type1,
        Self::Type2,
        Self::Vu,
        Self::Momentary,
        Self::ShortTerm,
    ];

    /// The word every client uses for it: `true peak`, `Type I`, `Type II`, `VU`,
    /// `Mom loudness`, `Short loudness` (never M / S, which are the goniometer's axes).
    pub fn word(self) -> &'static str {
        match self {
            Self::TruePeak => "true peak",
            Self::Type1 => "Type I",
            Self::Type2 => "Type II",
            Self::Vu => "VU",
            Self::Momentary => "Mom loudness",
            Self::ShortTerm => "Short loudness",
        }
    }

    /// Whether the bars read a loudness window of their source rather than a detector.
    pub fn is_loudness(self) -> bool {
        matches!(self, Self::Momentary | Self::ShortTerm)
    }

    /// The unit its bar reading is in: dBTP for the true peak, dBFS for the other detectors
    /// (a client converts to dBu from the input's line calibration), LUFS for a loudness
    /// window. Under `momentary` / `short_term` a frame's per-input `ppm` / `max` are each
    /// input's true peak in dBTP and the loudness is in the frame's `loudness`, so this unit
    /// names the ballistic only, never an input's reading.
    pub fn unit(self) -> &'static str {
        match self {
            Self::TruePeak => "dBTP",
            Self::Type1 | Self::Type2 | Self::Vu => "dBFS",
            Self::Momentary | Self::ShortTerm => "LUFS",
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
/// Loudness targets a rig may set, LUFS: spans R128's −23, ATSC A/85's −24, AES TD1008's
/// −16 … −20 and the −16 … −14 of streaming services, with room either side.
pub const LOUDNESS_TARGET_RANGE: (f64, f64) = (-36.0, -5.0);

/// The rig's programme-meter settings, kept in its rig settings file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelsSettings {
    /// What the bars read; the daemon runs every detector and reports this one in `ppm`.
    /// Under `momentary` / `short_term` each input's `ppm` / `max` are its true peak in dBTP
    /// and the bars read the source loudness in the frame's `loudness`.
    pub ballistic: Ballistic,
    /// The correlator's window.
    pub correlation_window: CorrelationWindow,
    /// An over is a frame's `true_peak` above this (R128 practice: −1 dBTP), and a new
    /// event only after it fell 1 dB below it.
    pub over_threshold: Dbtp,
    /// A side below this reads *quiet* and the pair carries no correlation: a dead channel
    /// must not read 0 and look like wide stereo.
    pub correlation_gate: Dbfs,
    /// The programme level a loudness bar is drawn around (its span is target −18 … +9 LU)
    /// and lit above. The rig's, because it colours the bar: two apps on one rig must agree.
    pub loudness_target: Lufs,
}

impl Default for LevelsSettings {
    fn default() -> Self {
        Self {
            ballistic: Ballistic::TruePeak,
            correlation_window: CorrelationWindow::Long,
            over_threshold: Dbtp(0.0),
            correlation_gate: Dbfs(-60.0),
            loudness_target: Lufs(-23.0),
        }
    }
}

impl LevelsSettings {
    /// A threshold, a gate and a target within their ranges and finite; the message names
    /// the one that is not.
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
        let l = self.loudness_target.0;
        if !l.is_finite() || l < LOUDNESS_TARGET_RANGE.0 || l > LOUDNESS_TARGET_RANGE.1 {
            return Err(format!(
                "loudness target {l} LUFS is outside {} … {} LUFS",
                LOUDNESS_TARGET_RANGE.0, LOUDNESS_TARGET_RANGE.1
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
        assert_eq!(Ballistic::Momentary.word(), "Mom loudness");
        assert_eq!(Ballistic::ShortTerm.word(), "Short loudness");
        assert_eq!(Ballistic::Momentary.unit(), "LUFS");
        assert_eq!(Ballistic::ShortTerm.unit(), "LUFS");
        assert!(Ballistic::ALL[..4] == Ballistic::PEAK);
        assert!(Ballistic::PEAK.iter().all(|b| !b.is_loudness()));
        assert!(Ballistic::Momentary.is_loudness() && Ballistic::ShortTerm.is_loudness());
        assert_eq!(CorrelationWindow::Short.seconds(), 0.3);
        assert_eq!(CorrelationWindow::Long.seconds(), 1.0);
        assert_eq!(CorrelationWindow::Short.text(), "0.3 s");
        assert_eq!(CorrelationWindow::Long.text(), "1 s");
        let d = LevelsSettings::default();
        assert_eq!(d.ballistic, Ballistic::TruePeak);
        assert_eq!(d.correlation_window, CorrelationWindow::Long);
        assert_eq!(d.over_threshold, Dbtp(0.0));
        assert_eq!(d.correlation_gate, Dbfs(-60.0));
        assert_eq!(d.loudness_target, Lufs(-23.0));
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
        for (l, ok) in [(-40.0, false), (-36.0, true), (-5.0, true), (-4.0, false)] {
            let s = LevelsSettings {
                loudness_target: Lufs(l),
                ..Default::default()
            };
            assert_eq!(s.check().is_ok(), ok, "{l}");
        }
        let low = LevelsSettings {
            loudness_target: Lufs(-40.0),
            ..Default::default()
        };
        assert_eq!(
            low.check().expect_err("out of range"),
            "loudness target -40 LUFS is outside -36 … -5 LUFS"
        );
        let nan = LevelsSettings {
            loudness_target: Lufs(f64::NAN),
            ..Default::default()
        };
        assert!(nan.check().is_err());
        for (b, name) in [
            (Ballistic::TruePeak, "true_peak"),
            (Ballistic::Momentary, "momentary"),
            (Ballistic::ShortTerm, "short_term"),
        ] {
            let v = rmp_serde::to_vec_named(&b).expect("encode");
            assert_eq!(rmp_serde::from_slice::<String>(&v).expect("decode"), name);
        }
    }
}
