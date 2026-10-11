use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::backend::{BackendKind, Presence};
use crate::error::{AudioError, Unavailability};

/// How the simulated device fails in an outage ([`FakeBackend::stall`](super::FakeBackend::stall),
/// [`FakeBackend::vanish`](super::FakeBackend::vanish)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutageKind {
    /// The device stops delivering without an error, as behind a hung audio server: running
    /// streams get no callbacks and no notification, and opening a stream blocks until the
    /// outage ends.
    Stall,
    /// The device goes away with an error, as when its server shuts down: running streams
    /// are ended by the host, and opening one fails as unavailable until the outage ends.
    Vanish,
}

/// The simulated device's availability, shared by every clone of a [`FakeBackend`] and
/// every stream opened on it. A stream that was running when an outage began never
/// delivers again, even after it ends: the device was reset under it, and only a stream
/// opened afterwards hears it (which is what a recovering daemon must do).
#[derive(Debug)]
pub(super) struct Outage {
    base: Instant,
    /// The latest outage's kind: 0 none yet, 1 stall, 2 vanish.
    pub(super) kind: AtomicU8,
    /// When the latest outage ends, ns after `base`; `u64::MAX` until restored.
    until_ns: AtomicU64,
    /// Outages begun so far.
    pub(super) begun: AtomicU64,
    /// Opens blocked by a stall wait here for its end.
    lock: Mutex<()>,
    ended: Condvar,
}

impl Outage {
    pub(super) fn new() -> Self {
        Self {
            base: Instant::now(),
            kind: AtomicU8::new(0),
            until_ns: AtomicU64::new(0),
            begun: AtomicU64::new(0),
            lock: Mutex::new(()),
            ended: Condvar::new(),
        }
    }

    fn now_ns(&self) -> u64 {
        u64::try_from(self.base.elapsed().as_nanos()).unwrap_or(u64::MAX - 1)
    }

    pub(super) fn begin(&self, kind: OutageKind, lasts: Option<Duration>) {
        let _g = self.lock.lock();
        let until = lasts.map_or(u64::MAX, |d| {
            self.now_ns()
                .saturating_add(u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
                .min(u64::MAX - 1)
        });
        self.kind.store(
            match kind {
                OutageKind::Stall => 1,
                OutageKind::Vanish => 2,
            },
            Ordering::Release,
        );
        self.until_ns.store(until, Ordering::Release);
        self.begun.fetch_add(1, Ordering::AcqRel);
    }

    pub(super) fn end(&self) {
        let _g = self.lock.lock();
        self.until_ns.store(0, Ordering::Release);
        self.ended.notify_all();
    }

    /// The outage in force now, if any.
    pub(super) fn active(&self) -> Option<OutageKind> {
        if self.now_ns() >= self.until_ns.load(Ordering::Acquire) {
            return None;
        }
        match self.kind.load(Ordering::Acquire) {
            1 => Some(OutageKind::Stall),
            2 => Some(OutageKind::Vanish),
            _ => None,
        }
    }

    /// The device as a server socket would show it: gone while vanished; there while
    /// stalled (a hung server keeps its socket) and after, a new generation once an outage
    /// has ended (the server came back).
    pub(super) fn presence(&self) -> Presence {
        let begun = self.begun.load(Ordering::Acquire);
        match self.active() {
            Some(OutageKind::Vanish) => Presence::Absent,
            Some(OutageKind::Stall) => Presence::Present {
                generation: (2 * begun).saturating_sub(1),
            },
            None => Presence::Present {
                generation: 2 * begun,
            },
        }
    }

    /// Waits out a stall (an open behind a hung server), then refuses while vanished.
    pub(super) fn admit(&self) -> Result<(), AudioError> {
        let mut g = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            match self.active() {
                None => return Ok(()),
                Some(OutageKind::Vanish) => {
                    return Err(AudioError::Unavailable {
                        backend: BackendKind::Fake,
                        reason: Unavailability::Host("the simulated device is gone".into()),
                    });
                }
                Some(OutageKind::Stall) => {
                    // Woken by `end`; a timed outage ends by itself, so look again soon.
                    g = self
                        .ended
                        .wait_timeout(g, Duration::from_millis(20))
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0;
                }
            }
        }
    }
}
