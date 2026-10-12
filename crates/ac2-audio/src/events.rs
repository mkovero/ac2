//! Asynchronous backend notifications (xruns, configuration changes, errors) and how they
//! become in-band block flags.
//!
//! Hosts report these on threads other than the audio callback (JACK's notification thread,
//! the HAL's property listeners). They only bump atomics here. Each callback owns an
//! [`EventLatch`] that compares the counters with the values it saw last and turns any
//! change into flags on its next block, without locks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

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
    /// The first [`ResetReason`] reported, as [`ResetReason::to_u8`]; 0 for none.
    reset: AtomicU8,
}

/// What the driver did that ended the stream as a driver reset. Every one of them means
/// the clock the stream was indexed on may have moved under it (another rate, another
/// source, a slip), so the output is cut and the index is not trusted past it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetReason {
    /// The driver asked to be reset (ASIO `kAsioResetRequest`): its settings changed,
    /// typically in its control panel, or it changed clock source.
    ResetRequest,
    /// The driver asked the host to resync (ASIO `kAsioResyncRequest`): its clock slipped
    /// or it lost and regained lock.
    ResyncRequest,
    /// The driver announced another buffer size (ASIO `kAsioBufferSizeChange`).
    BufferSizeChange,
    /// The driver reported a sample-rate change (ASIO `sampleRateDidChange`).
    RateChange,
    /// The driver reported a sample rate of 0: it has no clock (the external clock it
    /// follows went away).
    ClockLost,
}

impl ResetReason {
    fn to_u8(self) -> u8 {
        match self {
            Self::ResetRequest => 1,
            Self::ResyncRequest => 2,
            Self::BufferSizeChange => 3,
            Self::RateChange => 4,
            Self::ClockLost => 5,
        }
    }

    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::ResetRequest),
            2 => Some(Self::ResyncRequest),
            3 => Some(Self::BufferSizeChange),
            4 => Some(Self::RateChange),
            5 => Some(Self::ClockLost),
            _ => None,
        }
    }
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
    /// The stream ended because the driver reset or its clock changed (rate, buffer size or
    /// clock, typically in its control panel), and what it did first. A reopen should take
    /// the driver's new settings rather than force the old ones back.
    pub reset: Option<ResetReason>,
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

    /// Records that the driver reset or its clock changed, which ends the stream. The
    /// first reason is kept: what came after it is the same outage. Lock-free, callable
    /// from a driver callback.
    pub fn reset(&self, reason: ResetReason) {
        let _ = self
            .reset
            .compare_exchange(0, reason.to_u8(), Ordering::AcqRel, Ordering::Acquire);
        self.end();
    }

    /// Current values.
    pub fn snapshot(&self) -> EventSnapshot {
        EventSnapshot {
            xruns: self.xruns.load(Ordering::Relaxed),
            config_changes: self.config_changes.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            ended: self.ended.load(Ordering::Acquire),
            reset: ResetReason::from_u8(self.reset.load(Ordering::Acquire)),
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
