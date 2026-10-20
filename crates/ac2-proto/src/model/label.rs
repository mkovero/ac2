//! How the enums of the model read in text: the one wording the app, the CLI and the
//! daemon's own log share. Display only — the wire spelling is serde's.

use std::fmt;

use super::{
    AlarmSubject, AverageMethod, CueState, CueSubject, LevelScale, PeakQuantity, PeakWeighting,
    SpecLevel, TimeWeighting, TraceKind, Weighting,
};

impl Weighting {
    /// `A`, `C`, `Z` (IEC 61672-1).
    pub fn letter(self) -> &'static str {
        match self {
            Weighting::A => "A",
            Weighting::C => "C",
            Weighting::Z => "Z",
        }
    }
}

impl fmt::Display for Weighting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.letter())
    }
}

impl TimeWeighting {
    /// `F`, `S`, `I`.
    pub fn letter(self) -> &'static str {
        match self {
            TimeWeighting::Fast => "F",
            TimeWeighting::Slow => "S",
            TimeWeighting::Impulse => "I",
        }
    }
}

impl PeakWeighting {
    /// `C`, `Z`.
    pub fn letter(self) -> &'static str {
        match self {
            PeakWeighting::C => "C",
            PeakWeighting::Z => "Z",
        }
    }
}

impl LevelScale {
    /// `dB SPL`, `dBFS`: the unit of a level shown next to the quantity's name (`LAeq 30 min
    /// ≤ 99 dB SPL`), whose name already carries the weighting.
    pub fn unit(self) -> &'static str {
        match self {
            LevelScale::DbSpl => "dB SPL",
            LevelScale::Dbfs => "dBFS",
        }
    }

    /// `dB SPL (A)`, `dBFS (C)`: the unit of a level shown without the quantity's name.
    pub fn weighted_unit(self, w: Weighting) -> String {
        format!("{} ({w})", self.unit())
    }
}

impl PeakQuantity {
    /// `LCpeak`, `LAFmax`.
    pub fn name(self) -> &'static str {
        match self {
            PeakQuantity::LcPeak => "LCpeak",
            PeakQuantity::LafMax => "LAFmax",
        }
    }

    /// The frequency weighting the quantity is in.
    pub fn weighting(self) -> Weighting {
        match self {
            PeakQuantity::LcPeak => Weighting::C,
            PeakQuantity::LafMax => Weighting::A,
        }
    }
}

/// A window length in words: `30 s`, `5 min`, `90 min`, `3 h`, `1 min 30 s`. Minutes up to
/// two hours, as limits are written (LAeq,60min).
pub fn window_length(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "—".to_string();
    }
    let s = seconds.round() as u64;
    if s < 60 {
        format!("{s} s")
    } else if s > 7200 && s.is_multiple_of(3600) {
        format!("{} h", s / 3600)
    } else if s.is_multiple_of(60) {
        format!("{} min", s / 60)
    } else {
        format!("{} min {} s", s / 60, s % 60)
    }
}

/// `LAeq 30 min`.
pub fn window_name(duration_s: f64, weighting: Weighting) -> String {
    format!("L{weighting}eq {}", window_length(duration_s))
}

/// `63 Hz band LZeq 60 min`.
fn band_window_name(nominal_hz: f64, duration_s: f64, weighting: Weighting) -> String {
    format!(
        "{nominal_hz} Hz band {}",
        window_name(duration_s, weighting)
    )
}

impl fmt::Display for AlarmSubject {
    /// `LAeq 30 min`, `LCpeak`, `63 Hz band LZeq 60 min`, `predicted LAeq at the
    /// transfer's place`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            AlarmSubject::Window {
                duration,
                weighting,
            } => f.pad(&window_name(duration.0, weighting)),
            AlarmSubject::Peak { quantity } => f.pad(quantity.name()),
            AlarmSubject::Band {
                duration,
                weighting,
                nominal,
            } => f.pad(&band_window_name(nominal.0, duration.0, weighting)),
            AlarmSubject::Predicted => f.pad("predicted LAeq at the transfer's place"),
        }
    }
}

impl fmt::Display for CueSubject {
    /// `meter`, `LAeq 15 min`, `LCpeak`, `63 Hz band LZeq 60 min`, `predicted LAeq`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            CueSubject::Meter => f.pad("meter"),
            CueSubject::Window {
                duration,
                weighting,
            } => f.pad(&window_name(duration.0, weighting)),
            CueSubject::Peak { quantity } => f.pad(quantity.name()),
            CueSubject::Band {
                duration,
                weighting,
                nominal,
            } => f.pad(&band_window_name(nominal.0, duration.0, weighting)),
            CueSubject::Predicted => f.pad("predicted LAeq"),
        }
    }
}

impl fmt::Display for CueState {
    /// `ok`, `near`, `over`, `fault`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            CueState::Ok => "ok",
            CueState::Near => "near",
            CueState::Over => "over",
            CueState::Fault => "fault",
        })
    }
}

impl AverageMethod {
    /// `power`, `complex`, `coherence-weighted`.
    pub fn label(self) -> &'static str {
        match self {
            AverageMethod::Power => "power",
            AverageMethod::Complex => "complex",
            AverageMethod::CoherenceWeighted => "coherence-weighted",
        }
    }
}

impl TraceKind {
    /// `transfer`, `target`, `spectrum`, `spectrum density`, `RTA`, `sweep`.
    pub fn label(self) -> &'static str {
        match self {
            TraceKind::Transfer => "transfer",
            TraceKind::Target => "target",
            TraceKind::Spectrum {
                level: SpecLevel::Tone,
                ..
            } => "spectrum",
            TraceKind::Spectrum {
                level: SpecLevel::Density,
                ..
            } => "spectrum density",
            TraceKind::Rta { .. } => "RTA",
            TraceKind::Sweep => "sweep",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::{Hz, Seconds};

    #[test]
    fn window_lengths_and_subjects() {
        assert_eq!(window_length(90.0), "1 min 30 s");
        assert_eq!(window_length(30.0), "30 s");
        assert_eq!(window_length(4.0 * 3600.0), "4 h");
        assert_eq!(window_length(7200.0), "120 min");
        let w = AlarmSubject::Window {
            duration: Seconds(90.0),
            weighting: Weighting::A,
        };
        assert_eq!(w.to_string(), "LAeq 1 min 30 s");
        let b = CueSubject::Band {
            duration: Seconds(3600.0),
            weighting: Weighting::Z,
            nominal: Hz(1000.0),
        };
        assert_eq!(b.to_string(), "1000 Hz band LZeq 60 min");
        assert_eq!(LevelScale::DbSpl.weighted_unit(Weighting::A), "dB SPL (A)");
        assert_eq!(LevelScale::Dbfs.weighted_unit(Weighting::C), "dBFS (C)");
    }
}
