//! Input meters and channel labels of the session and measurement dialogs: how full a bar
//! is, what it reads and which state colours it, and what an input or output is called.
//!
//! Scale: −60 … 0 dBFS, linear in dB. Below −60 dBFS a bar is empty and reads `—` (an
//! unpatched input reads the same as a silent one: nothing to see). RMS is the bar, the
//! sample peak a tick on it, so a noise stimulus (crest ≈ 12 dB) shows both its body and
//! its headroom.

use crate::format::{self, NO_VALUE};

/// Lowest level shown, dBFS.
pub const FLOOR_DBFS: f64 = -60.0;
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
    /// RMS readout: `−12.3`, or `—` below the floor.
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
        let text = if rms.is_finite() && rms >= FLOOR_DBFS {
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
        assert_eq!(quiet.text, NO_VALUE);
        assert_eq!(quiet.rms_fill, 0.0);
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
}
