//! Microphone consent (TCC), through AVFoundation.
//!
//! Without a grant, macOS does not fail a capture IOProc or feed it zeros: its
//! `AudioDeviceStart` blocks on a consent prompt, for good when nobody can answer it. A
//! caller that captures checks [`microphone`] first and, when undecided, asks with
//! [`request_microphone`] under a deadline.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

/// What TCC says about this process and the microphone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authorization {
    /// Allowed.
    Authorized,
    /// The user said no.
    Denied,
    /// Policy forbids it (MDM, parental controls).
    Restricted,
    /// Nobody has decided yet: a capture would wait for a prompt.
    NotDetermined,
}

/// How a [`request_microphone`] went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    /// The answer, or [`Authorization::NotDetermined`] when none came in time.
    pub authorization: Authorization,
    /// The request call itself returned (it is made on a thread of its own, so a call that
    /// never returns cannot hold the caller).
    pub call_returned: bool,
    /// The completion handler ran within the deadline.
    pub answered: bool,
}

fn map(s: AVAuthorizationStatus) -> Authorization {
    match s {
        AVAuthorizationStatus::Authorized => Authorization::Authorized,
        AVAuthorizationStatus::Denied => Authorization::Denied,
        AVAuthorizationStatus::Restricted => Authorization::Restricted,
        _ => Authorization::NotDetermined,
    }
}

/// `AVCaptureDevice.authorizationStatus(for: .audio)`. Never prompts.
pub fn microphone() -> Authorization {
    // SAFETY: an immutable framework constant, read after the framework is loaded.
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return Authorization::NotDetermined;
    };
    // SAFETY: a class method taking `AVMediaTypeAudio`, which it accepts.
    map(unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) })
}

/// Asks for the microphone when nobody has decided yet (the system shows its prompt when
/// it can), and waits up to `timeout` for the answer. Returns at once when already decided.
pub fn request_microphone(timeout: Duration) -> Request {
    let now = microphone();
    if now != Authorization::NotDetermined {
        return Request {
            authorization: now,
            call_returned: true,
            answered: true,
        };
    }
    let (tx, rx) = mpsc::sync_channel::<bool>(1);
    let returned = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&returned);
    let spawned = std::thread::Builder::new()
        .name("cahal-consent".into())
        .spawn(move || {
            // SAFETY: as in `microphone`.
            let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
                return;
            };
            let block = RcBlock::new(move |granted: Bool| {
                let _ = tx.try_send(granted.as_bool());
            });
            // SAFETY: a class method taking `AVMediaTypeAudio` and a block AVFoundation
            // copies and calls once on a queue of its own.
            unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &block) };
            r.store(true, Ordering::Release);
        });
    if spawned.is_err() {
        return Request {
            authorization: Authorization::NotDetermined,
            call_returned: false,
            answered: false,
        };
    }
    let answer = rx.recv_timeout(timeout).ok();
    Request {
        authorization: match answer {
            Some(true) => Authorization::Authorized,
            Some(false) => match microphone() {
                Authorization::Restricted => Authorization::Restricted,
                _ => Authorization::Denied,
            },
            None => Authorization::NotDetermined,
        },
        call_returned: returned.load(Ordering::Acquire),
        answered: answer.is_some(),
    }
}
