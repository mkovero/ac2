//! The audio backends a front end (the `ac2d` binary, an app embedding the daemon) can name.
//!
//! Each platform has one real backend: JACK on Linux (JACK2, or PipeWire through
//! pipewire-jack; there is no ALSA backend), cpal on macOS and Windows (Core Audio, WASAPI).
//! The simulated rig is never a fallback: a fake device must never stand in for a missing
//! real one, so it runs only when named.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use ac2_audio::fake::{FakeDrive, FakePath, Pace};
use ac2_audio::{Backend, FakeBackend, FakeConfig};

/// A backend by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    /// The OS audio host through cpal (Core Audio / WASAPI); macOS and Windows.
    Cpal,
    /// JACK (JACK2, or PipeWire through pipewire-jack); Linux.
    Jack,
    /// A simulated rig ([`FAKE_RIG`]); never real audio.
    Fake,
}

impl BackendChoice {
    /// This platform's real backend, and the default: JACK on Linux, cpal elsewhere.
    pub const fn platform() -> Self {
        if cfg!(target_os = "linux") {
            Self::Jack
        } else {
            Self::Cpal
        }
    }

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
            other => Err(format!(
                "unknown backend {other} ({} or fake)",
                Self::platform()
            )),
        }
    }
}

/// What the fake backend simulates, in real time: output 1 → input 1 through a loopback
/// cable (32 samples) and output 1 → input 2 through an "acoustic" path (5 ms, −6 dB, a
/// little noise), so a transfer measurement with reference 1 and measurement 2 shows a
/// real delay and magnitude once a stimulus plays. The acoustic path's driver distorts
/// ([`FAKE_RIG_DISTORTION`]): a sweep measurement shows known H2 and H3.
pub const FAKE_RIG: &str = "fake rig: out 1 → in 1 loopback (32 samples), out 1 → in 2 \
                            acoustic (5 ms, −6 dB; H2 −40 dB, H3 −50 dB at −20 dBFS)";

/// The fake rig's acoustic path is `y = x + c2·x² + c3·x³` before the room: at a sweep of
/// −20 dBFS (peak 0.1) the second harmonic is `c2·0.1/2` = −40 dB and the third
/// `c3·0.01/4` ≈ −50 dB re the fundamental; each harmonic falls with the level (H2 10 dB, H3
/// 20 dB per 10 dB less).
pub const FAKE_RIG_DISTORTION: [f64; 2] = [0.2, 1.265];

fn fake_config() -> FakeConfig {
    const LOOP: u32 = 32;
    let names = |n: &[&str]| Some(n.iter().map(|s| (*s).to_owned()).collect());
    FakeConfig {
        drive: FakeDrive::Thread(Pace::Realtime),
        paths: vec![
            FakePath::loopback(0, 0, LOOP),
            FakePath::acoustic(0, 1, LOOP + 240, vec![0.5], 1e-4)
                .distorting(FAKE_RIG_DISTORTION.to_vec()),
        ],
        input_names: names(&["Loop return", "Room mic", "Line 3", "Line 4"]),
        output_names: names(&["Out 1 (speaker + loop)", "Out 2"]),
        ..FakeConfig::default()
    }
}

/// What a backend is, for the operator choosing one.
pub(crate) fn describe(kind: ac2_audio::BackendKind) -> String {
    match kind {
        ac2_audio::BackendKind::Jack => "JACK audio server (JACK2, or PipeWire's JACK): every \
                                         port on one clock, rate and buffer set by the server"
            .into(),
        ac2_audio::BackendKind::Cpal => {
            let host = if cfg!(target_os = "macos") {
                "Core Audio"
            } else {
                "WASAPI"
            };
            format!("System audio ({host}): the interfaces the operating system lists")
        }
        ac2_audio::BackendKind::Fake => {
            "Simulated rig (no audio): out 1 returns on in 1 (loop) and in 2 (room)".into()
        }
        ac2_audio::BackendKind::Replay => "A recording played back (capture only)".into(),
    }
}

/// Every backend a daemon started on `choice` offers: that one alone. A platform has one
/// real backend, and the simulated rig is offered only when named.
pub fn backends(choice: BackendChoice) -> Result<Vec<Arc<dyn Backend>>, String> {
    Ok(vec![backend(choice)?])
}

/// Builds the backend `choice` names; a backend this platform does not have is refused
/// with what to use instead.
pub fn backend(choice: BackendChoice) -> Result<Arc<dyn Backend>, String> {
    match choice {
        BackendChoice::Fake => Ok(Arc::new(
            FakeBackend::new(fake_config()).map_err(|e| e.to_string())?,
        )),
        #[cfg(target_os = "linux")]
        BackendChoice::Jack => Ok(Arc::new(ac2_audio::JackBackend::new(
            ac2_audio::JackConfig::default(),
        ))),
        #[cfg(target_os = "linux")]
        BackendChoice::Cpal => Err("Linux audio goes through JACK (JACK2, or PipeWire through \
                                    pipewire-jack); there is no ALSA backend: use --backend jack"
            .into()),
        #[cfg(not(target_os = "linux"))]
        BackendChoice::Cpal => Ok(Arc::new(ac2_audio::CpalBackend::new())),
        #[cfg(not(target_os = "linux"))]
        BackendChoice::Jack => {
            Err("JACK is supported on Linux only; use --backend cpal (the system audio)".into())
        }
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

    #[test]
    fn one_real_backend_per_platform_and_the_simulated_rig_alone() {
        let kinds = |c| {
            backends(c)
                .expect("backends")
                .iter()
                .map(|b| b.kind())
                .collect::<Vec<_>>()
        };
        assert_eq!(kinds(BackendChoice::Fake), [ac2_audio::BackendKind::Fake]);
        let real = kinds(BackendChoice::platform());
        let other = if cfg!(target_os = "linux") {
            assert_eq!(real, [ac2_audio::BackendKind::Jack]);
            BackendChoice::Cpal
        } else {
            assert_eq!(real, [ac2_audio::BackendKind::Cpal]);
            BackendChoice::Jack
        };
        match backend(other) {
            Err(e) => assert!(e.contains("--backend"), "{e}"),
            Ok(_) => panic!("the other platform's backend is refused"),
        }
    }
}
