//! The audio backends a front end (the `ac2d` binary, an app embedding the daemon) can name.
//! There is no default and no fallback: a fake device must never stand in for a missing
//! real one, so the caller always chooses.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use ac2_audio::fake::{FakeDrive, FakePath, Pace};
use ac2_audio::{Backend, CpalBackend, FakeBackend, FakeConfig};

/// A backend by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    /// The platform audio API (ALSA / CoreAudio / WASAPI).
    Cpal,
    /// JACK (Linux, feature `jack`).
    Jack,
    /// A simulated rig ([`FAKE_RIG`]); never real audio.
    Fake,
}

impl BackendChoice {
    /// The name `--backend` takes.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cpal => "cpal",
            Self::Jack => "jack",
            Self::Fake => "fake",
        }
    }
}

impl fmt::Display for BackendChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for BackendChoice {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "cpal" => Ok(Self::Cpal),
            "jack" => Ok(Self::Jack),
            "fake" => Ok(Self::Fake),
            other => Err(format!("unknown backend {other} (jack, cpal or fake)")),
        }
    }
}

/// What the fake backend simulates, in real time: output 1 → input 1 through a loopback
/// cable (32 samples) and output 1 → input 2 through an "acoustic" path (5 ms, −6 dB, a
/// little noise), so a transfer measurement with reference 1 and measurement 2 shows a
/// real delay and magnitude once a stimulus plays.
pub const FAKE_RIG: &str =
    "fake rig: out 1 → in 1 loopback (32 samples), out 1 → in 2 acoustic (5 ms, −6 dB)";

fn fake_config() -> FakeConfig {
    const LOOP: u32 = 32;
    FakeConfig {
        drive: FakeDrive::Thread(Pace::Realtime),
        paths: vec![
            FakePath::loopback(0, 0, LOOP),
            FakePath::acoustic(0, 1, LOOP + 240, vec![0.5], 1e-4),
        ],
        ..FakeConfig::default()
    }
}

/// Builds the backend `choice` names.
pub fn backend(choice: BackendChoice) -> Result<Arc<dyn Backend>, String> {
    match choice {
        BackendChoice::Cpal => Ok(Arc::new(CpalBackend::new())),
        BackendChoice::Fake => Ok(Arc::new(
            FakeBackend::new(fake_config()).map_err(|e| e.to_string())?,
        )),
        #[cfg(all(feature = "jack", target_os = "linux"))]
        BackendChoice::Jack => Ok(Arc::new(ac2_audio::JackBackend::new(
            ac2_audio::JackConfig::default(),
        ))),
        #[cfg(not(all(feature = "jack", target_os = "linux")))]
        BackendChoice::Jack => Err("this build has no JACK backend (feature `jack`, Linux)".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_fake_builds() {
        for c in [
            BackendChoice::Cpal,
            BackendChoice::Jack,
            BackendChoice::Fake,
        ] {
            assert_eq!(c.name().parse::<BackendChoice>(), Ok(c));
        }
        assert!("alsa".parse::<BackendChoice>().is_err());
        assert!(backend(BackendChoice::Fake).is_ok());
    }
}
