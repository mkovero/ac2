//! Input meters and channel labels of the session and measurement dialogs: how full a bar
//! is, what it reads and which state colours it, and what an input or output is called.
//!
//! Scale: −60 … 0 dBFS, linear in dB. Below −60 dBFS a bar is empty, but the readout keeps
//! the number down to −120 dBFS: a measurement mic's room noise (−70 … −90 dBFS) is the
//! floor every later measurement sits on, so it is worth reading before a sweep. Only digital
//! silence (or less than −120 dBFS) reads `—`. RMS is the bar, the
//! sample peak a tick on it, so a noise stimulus (crest ≈ 12 dB) shows both its body and
//! its headroom.

use crate::format::{self, NO_VALUE};

/// Lowest level shown, dBFS.
pub const FLOOR_DBFS: f64 = -60.0;
/// Lowest level the readout prints, dBFS; below it (or digital silence) it reads `—`.
pub const READOUT_FLOOR_DBFS: f64 = -120.0;
/// RMS below this reads as no signal.
pub const SIGNAL_DBFS: f64 = -50.0;
/// Peak above this is close to clipping.
pub const HOT_PEAK_DBFS: f64 = -6.0;

/// What a meter shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterState {
    /// Nothing measured yet (no frame for this input).
    NoData,
    /// Below [`SIGNAL_DBFS`].
    Silent,
    /// Signal with headroom.
    Signal,
    /// Peak within 6 dB of full scale.
    Hot,
    /// Clipped (now, or within the hold time).
    Clip,
}

/// One input's meter.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterReading {
    /// Bar length, 0 … 1 (RMS).
    pub rms_fill: f32,
    /// Peak tick position, 0 … 1.
    pub peak_fill: f32,
    /// RMS readout: `−12.3`, or `—` for digital silence (below [`READOUT_FLOOR_DBFS`]).
    pub text: String,
    pub state: MeterState,
}

fn fill(dbfs: f64) -> f32 {
    if dbfs.is_finite() {
        ((dbfs - FLOOR_DBFS) / -FLOOR_DBFS).clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

impl MeterReading {
    /// No frame for this input.
    pub fn none() -> Self {
        Self {
            rms_fill: 0.0,
            peak_fill: 0.0,
            text: NO_VALUE.into(),
            state: MeterState::NoData,
        }
    }

    /// From a frame's peak and RMS (dBFS; −∞ for digital silence) and clip state
    /// (`clipped` = in this interval or held).
    pub fn new(peak_dbfs: f32, rms_dbfs: f32, clipped: bool) -> Self {
        let (peak, rms) = (f64::from(peak_dbfs), f64::from(rms_dbfs));
        let state = if clipped {
            MeterState::Clip
        } else if peak.is_finite() && peak >= HOT_PEAK_DBFS {
            MeterState::Hot
        } else if rms.is_finite() && rms >= SIGNAL_DBFS {
            MeterState::Signal
        } else {
            MeterState::Silent
        };
        let text = if rms.is_finite() && rms >= READOUT_FLOOR_DBFS {
            format::fixed(rms, 1)
        } else {
            NO_VALUE.into()
        };
        Self {
            rms_fill: fill(rms),
            peak_fill: fill(peak),
            text,
            state,
        }
    }

    /// The readout with its unit: `−12.3 dBFS`, `—`.
    pub fn with_unit(&self) -> String {
        if self.text == NO_VALUE {
            self.text.clone()
        } else {
            format!("{} dBFS", self.text)
        }
    }
}

/// What an input is called: the mic name the operator gave it, else the name the backend
/// gives the channel, else `Input N` (one-based).
pub fn input_name(channel: u16, mic: Option<&str>, device_name: Option<&str>) -> String {
    fn named(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    named(mic).or(named(device_name)).map_or_else(
        || format!("Input {}", u32::from(channel) + 1),
        str::to_owned,
    )
}

/// What an output is called: the backend's name, else `Output N`.
pub fn output_name(channel: u16, device_name: Option<&str>) -> String {
    device_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || format!("Output {}", u32::from(channel) + 1),
            str::to_owned,
        )
}

/// A channel choice in a list: `2 · Room mic`.
pub fn channel_choice(channel: u16, name: &str) -> String {
    format!("{} · {name}", u32::from(channel) + 1)
}

/// What an input is wired as in the setup: the loopback return of the stimulus, or a
/// measurement mic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputRole {
    Reference,
    Mic,
}

impl InputRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Mic => "mic",
        }
    }
}

/// What the operation in focus (the running sweep, else the selected measurement) uses an
/// input as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputUse {
    Reference,
    Measurement,
}

impl InputUse {
    /// The short mark next to the meter.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Reference => "REF",
            Self::Measurement => "MEAS",
        }
    }
}

/// A session input's row in the always-on meters: its label, what the operation in focus
/// uses it as, and its reading.
#[derive(Clone, Debug, PartialEq)]
pub struct InputRow {
    pub channel: u16,
    pub label: String,
    pub used: Option<InputUse>,
    pub reading: MeterReading,
}

/// A session input's label: its name ([`input_name`]), its role, and the one-based input
/// number when the name does not already say it, with the mic curve in use
/// ([`crate::cal::curve_short`]) after a mic's name: `MM1 34804 · 90° · mic (in 1)`,
/// `loopback · reference (in 2)`, `Input 3 · mic`, `capture_4 (in 4)`.
pub fn input_label(
    channel: u16,
    name: &str,
    curve: Option<&str>,
    role: Option<InputRole>,
) -> String {
    let n = u32::from(channel) + 1;
    let generic = name == format!("Input {n}");
    let mut s = name.to_owned();
    if let Some(c) = curve {
        s.push_str(" · ");
        s.push_str(c);
    }
    if let Some(r) = role {
        s.push_str(" · ");
        s.push_str(r.name());
    }
    if !generic {
        s.push_str(&format!(" (in {n})"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_states_and_text() {
        let m = MeterReading::new(-6.0, -18.0, false);
        assert_eq!(m.state, MeterState::Hot);
        assert!((m.rms_fill - 0.7).abs() < 1e-6);
        assert!((m.peak_fill - 0.9).abs() < 1e-6);
        assert_eq!(m.text, "\u{2212}18.0");
        assert_eq!(m.with_unit(), "\u{2212}18.0 dBFS");
        assert_eq!(
            MeterReading::new(-20.0, -32.0, false).state,
            MeterState::Signal
        );
        let quiet = MeterReading::new(-58.0, -70.0, false);
        assert_eq!(quiet.state, MeterState::Silent);
        assert_eq!(quiet.text, "\u{2212}70.0");
        assert_eq!(quiet.rms_fill, 0.0);
        assert_eq!(MeterReading::new(-120.0, -130.0, false).text, NO_VALUE);
        let silent = MeterReading::new(f32::NEG_INFINITY, f32::NEG_INFINITY, false);
        assert_eq!(silent.state, MeterState::Silent);
        assert_eq!(silent.with_unit(), NO_VALUE);
        assert_eq!(
            MeterReading::new(-40.0, -50.0, true).state,
            MeterState::Clip
        );
        assert_eq!(MeterReading::new(3.0, 0.5, false).rms_fill, 1.0);
        assert_eq!(MeterReading::none().state, MeterState::NoData);
    }

    #[test]
    fn names_fall_back_in_order() {
        assert_eq!(input_name(2, None, None), "Input 3");
        assert_eq!(input_name(2, None, Some("capture_3")), "capture_3");
        assert_eq!(input_name(2, Some("M30 FOH"), Some("capture_3")), "M30 FOH");
        assert_eq!(input_name(0, Some("  "), Some("")), "Input 1");
        assert_eq!(output_name(1, None), "Output 2");
        assert_eq!(output_name(1, Some("playback_2")), "playback_2");
        assert_eq!(channel_choice(1, "Room mic"), "2 · Room mic");
    }

    #[test]
    fn input_labels_name_the_role_and_the_input() {
        assert_eq!(
            input_label(0, "MM1 34804", None, Some(InputRole::Mic)),
            "MM1 34804 · mic (in 1)"
        );
        assert_eq!(
            input_label(0, "MM1 34804", Some("90°"), Some(InputRole::Mic)),
            "MM1 34804 · 90° · mic (in 1)"
        );
        assert_eq!(
            input_label(1, "loopback", None, Some(InputRole::Reference)),
            "loopback · reference (in 2)"
        );
        assert_eq!(
            input_label(2, "Input 3", None, Some(InputRole::Mic)),
            "Input 3 · mic"
        );
        assert_eq!(input_label(3, "capture_4", None, None), "capture_4 (in 4)");
        assert_eq!(input_label(3, "Input 4", None, None), "Input 4");
        assert_eq!(InputUse::Reference.tag(), "REF");
        assert_eq!(InputUse::Measurement.tag(), "MEAS");
    }
}
