//! Core Audio backend (macOS), on the `cahal` binding: the HAL directly, one IOProc per
//! device.
//!
//! One IOProc call serves capture and playback of a device, both stamped on the device's
//! one sample-time line, so both directions of a cycle share one exact index taken from the
//! sample time, as on JACK and ASIO. Input and output on two devices run two IOProcs, each
//! indexed on its own clock. The adapter is plain Rust over [`source::HalSource`] and runs
//! in every OS's tests against scripted devices; only `cahal` touches the HAL.

#![forbid(unsafe_code)]

mod caps;
mod engine;
mod open;
mod source;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use cahal::Scope;

use crate::backend::{
    Backend, BackendKind, DeviceCaps, DeviceSelector, Direction, DuplexRequest, Presence,
};
use crate::error::{AudioError, Operation};
use crate::stream::DuplexStream;
use source::HalSource;

/// Core Audio through the HAL.
#[derive(Clone)]
pub struct CoreAudioBackend {
    source: Arc<dyn HalSource>,
    consent_wait: Duration,
}

impl std::fmt::Debug for CoreAudioBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreAudioBackend")
            .field("consent_wait", &self.consent_wait)
            .finish_non_exhaustive()
    }
}

impl CoreAudioBackend {
    /// How long an open waits for the operator to answer the system's microphone prompt.
    pub const CONSENT_WAIT: Duration = Duration::from_secs(60);

    /// The machine's HAL.
    #[cfg(target_os = "macos")]
    pub fn new() -> Self {
        Self::with_source(Arc::new(source::CahalSource))
    }

    pub(crate) fn with_source(source: Arc<dyn HalSource>) -> Self {
        Self {
            source,
            consent_wait: Self::CONSENT_WAIT,
        }
    }

    /// The same backend with another bound on the microphone request an open makes when
    /// nobody has decided yet.
    pub fn with_consent_wait(mut self, wait: Duration) -> Self {
        self.consent_wait = wait;
        self
    }
}

#[cfg(target_os = "macos")]
impl Default for CoreAudioBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for CoreAudioBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::CoreAudio
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let src = &*self.source;
        let err = |e: cahal::Error| open::backend_err(Operation::Enumerate, e);
        let default_in = src.default_uid(Scope::Input).map_err(err)?;
        let default_out = src.default_uid(Scope::Output).map_err(err)?;
        let pid = std::process::id() as i32;
        let mut out = Vec::new();
        for uid in src.uids().map_err(err)? {
            // A device that left between the list and the look is skipped.
            let Ok(d) = src.describe(&uid) else {
                continue;
            };
            if d.input.streams.is_empty() && d.output.streams.is_empty() {
                continue;
            }
            out.push(caps::device_caps(
                &d,
                default_in.as_deref() == Some(uid.as_str()),
                default_out.as_deref() == Some(uid.as_str()),
                pid,
            ));
        }
        Ok(out)
    }

    fn open(&self, request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        open::open_stream(Arc::clone(&self.source), request, self.consent_wait)
    }

    /// Present with the device's object id as the generation: a replugged device usually
    /// comes back under a new one.
    fn probe(&self, device: &DeviceSelector) -> Presence {
        let Ok(uid) = open::resolve(&*self.source, device, Direction::Input) else {
            return Presence::Absent;
        };
        match self.source.object_id(&uid) {
            Ok(Some(id)) => Presence::Present {
                generation: u64::from(id),
            },
            Ok(None) => Presence::Absent,
            Err(_) => Presence::Unknown,
        }
    }

    /// Core Audio devices have no driver panel of their own: their rate, format and clock
    /// source are set in Audio MIDI Setup, which this brings up. A change made there
    /// reaches a running stream through the HAL's listeners (a rate change resets it).
    fn control_panel(&self, device: &DeviceSelector) -> Result<(), AudioError> {
        let uid = open::resolve(&*self.source, device, Direction::Input)?;
        match self.source.object_id(&uid) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(AudioError::DeviceNotFound {
                    direction: Direction::Input,
                    selector: device.to_string(),
                });
            }
            Err(e) => return Err(open::backend_err(Operation::Open, e)),
        }
        self.source
            .open_audio_midi_setup()
            .map_err(|e| open::backend_err(Operation::Open, e))
    }
}
