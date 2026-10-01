//! Asynchronous backend notifications (xruns, configuration changes, errors) and how they
//! become in-band block flags.
//!
//! Hosts report these on threads other than the audio callback (JACK's notification thread,
//! cpal's error callback). They only bump atomics here. Each callback owns an
//! [`EventLatch`] that compares the counters with the values it saw last and turns any
//! change into flags on its next block, without locks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::block::BlockFlags;

/// Counters bumped by backend notification paths. Relaxed: they are counts, and the flag
/// they produce only needs to arrive eventually.
#[derive(Debug, Default)]
pub struct BackendEvents {
    xruns: AtomicU64,
    config_changes: AtomicU64,
    errors: AtomicU64,
    ended: AtomicBool,
}

/// Plain copy of [`BackendEvents`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSnapshot {
    /// Xruns reported by the host.
    pub xruns: u64,
    /// Rate, buffer size or device changes reported by the host.
    pub config_changes: u64,
    /// Other errors reported by the host.
    pub errors: u64,
    /// The host stopped the stream (server shut down, device removed).
    pub ended: bool,
}

impl BackendEvents {
    /// Records an xrun.
    pub fn xrun(&self) {
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a configuration change.
    pub fn config_change(&self) {
        self.config_changes.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a non-fatal error.
    pub fn error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Records that the host ended the stream.
    pub fn end(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        self.ended.store(true, Ordering::Release);
    }

    /// Current values.
    pub fn snapshot(&self) -> EventSnapshot {
        EventSnapshot {
            xruns: self.xruns.load(Ordering::Relaxed),
            config_changes: self.config_changes.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            ended: self.ended.load(Ordering::Acquire),
        }
    }
}

/// Turns counter changes into block flags for one callback.
#[derive(Debug)]
pub struct EventLatch {
    events: Arc<BackendEvents>,
    seen_xruns: u64,
    seen_config: u64,
}

impl EventLatch {
    /// A latch that reports changes after the current counter values.
    pub fn new(events: Arc<BackendEvents>) -> Self {
        let now = events.snapshot();
        Self {
            events,
            seen_xruns: now.xruns,
            seen_config: now.config_changes,
        }
    }

    /// Flags for everything reported since the previous call.
    #[inline]
    pub fn take(&mut self) -> BlockFlags {
        let mut flags = BlockFlags::NONE;
        let x = self.events.xruns.load(Ordering::Relaxed);
        if x != self.seen_xruns {
            self.seen_xruns = x;
            flags |= BlockFlags::XRUN;
        }
        let c = self.events.config_changes.load(Ordering::Relaxed);
        if c != self.seen_config {
            self.seen_config = c;
            flags |= BlockFlags::CONFIG_CHANGE;
        }
        flags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latch_reports_each_change_once_per_latch() {
        let ev = Arc::new(BackendEvents::default());
        ev.xrun();
        let mut a = EventLatch::new(Arc::clone(&ev));
        let mut b = EventLatch::new(Arc::clone(&ev));
        assert_eq!(
            a.take(),
            BlockFlags::NONE,
            "earlier events are not replayed"
        );
        ev.xrun();
        ev.xrun();
        ev.config_change();
        let both = BlockFlags::XRUN | BlockFlags::CONFIG_CHANGE;
        assert_eq!(a.take(), both);
        assert_eq!(a.take(), BlockFlags::NONE);
        assert_eq!(b.take(), both, "each callback has its own latch");
        ev.end();
        let s = ev.snapshot();
        assert_eq!(
            (s.xruns, s.config_changes, s.errors, s.ended),
            (3, 1, 1, true)
        );
    }
}
