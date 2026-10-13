//! How much of each block's period the stream's own callback spends: the figure a DSP load
//! meter shows.
//!
//! The callback reads the monotonic clock on entry and on exit (a vDSO read on Linux,
//! `QueryPerformanceCounter` on Windows, `mach_absolute_time` on macOS: none of them is a
//! system call) and adds the time it took, and the period its block covers, to two
//! counters. A reader outside the callback turns differences of those counters into a mean
//! load over any window it likes, and takes the largest share a single callback used since
//! its previous look. Only this process's callback is timed: the rest of the audio graph
//! (other JACK clients, the driver, the server) is not part of the figure.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::events::BackendEvents;

/// Counters the callbacks add to. Relaxed: a reader that sees one counter a block ahead of
/// the other is off by one block in a window of hundreds.
#[derive(Debug, Default)]
pub(crate) struct LoadCounters {
    busy_ns: AtomicU64,
    period_ns: AtomicU64,
    /// One more than the largest busy ÷ period of a single callback since the reader last
    /// took it, in millionths; 0 when no callback ran since.
    peak_ppm: AtomicU64,
}

/// The callback's side: one per callback, like [`EventLatch`](crate::events::EventLatch).
#[derive(Debug)]
pub(crate) struct LoadTimer {
    events: Arc<BackendEvents>,
    rate: u64,
    /// Whether this callback's blocks count as the stream's period. A stream run by two
    /// callbacks over the same time (Core Audio's separate input and output devices) counts
    /// the period once, or the load would read as half of what both take together.
    counts_period: bool,
}

impl LoadTimer {
    /// The timer of the callback whose blocks pace the stream at `rate`.
    pub(crate) fn new(events: Arc<BackendEvents>, rate: u32) -> Self {
        Self {
            events,
            rate: u64::from(rate),
            counts_period: true,
        }
    }

    /// The timer of a second callback over the same time as the one [`LoadTimer::new`] times:
    /// its time counts, its period does not.
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
    pub(crate) fn beside(events: Arc<BackendEvents>, rate: u32) -> Self {
        Self {
            counts_period: false,
            ..Self::new(events, rate)
        }
    }

    /// The callback starts.
    #[inline]
    pub(crate) fn start(&self) -> Instant {
        Instant::now()
    }

    /// The callback that started at `started` ends, having handled `frames` frames.
    #[inline]
    pub(crate) fn finish(&self, started: Instant, frames: u32) {
        if self.rate == 0 || frames == 0 {
            return;
        }
        let busy = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let period = (u64::from(frames) * 1_000_000_000 / self.rate).max(1);
        let c = &self.events.load;
        c.busy_ns.fetch_add(busy, Ordering::Relaxed);
        if self.counts_period {
            c.period_ns.fetch_add(period, Ordering::Relaxed);
        }
        let ppm = busy.saturating_mul(1_000_000) / period;
        c.peak_ppm
            .fetch_max(ppm.saturating_add(1), Ordering::Relaxed);
    }
}

/// Running totals of a stream's callback time and the period its blocks covered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoadTotals {
    /// Time spent inside the callback, ns.
    pub busy_ns: u64,
    /// Audio time the callback's blocks covered, ns.
    pub period_ns: u64,
}

impl LoadTotals {
    /// Mean load between `earlier` and `self`, as a share of the period (1.0 = the callback
    /// took the whole period); `None` when no block was timed in between.
    pub fn since(&self, earlier: &LoadTotals) -> Option<f64> {
        let period = self.period_ns.checked_sub(earlier.period_ns)?;
        let busy = self.busy_ns.checked_sub(earlier.busy_ns)?;
        (period > 0).then(|| busy as f64 / period as f64)
    }
}

/// A reader of a stream's callback load and xrun count, for a health meter. Cheap to
/// clone; it outlives the stream harmlessly (its numbers just stop moving).
#[derive(Clone, Debug)]
pub struct StreamHealth {
    events: Arc<BackendEvents>,
}

impl StreamHealth {
    pub(crate) fn new(events: Arc<BackendEvents>) -> Self {
        Self { events }
    }

    /// Whether `other` reads the same stream.
    pub fn same_stream(&self, other: &StreamHealth) -> bool {
        Arc::ptr_eq(&self.events, &other.events)
    }

    /// Callback time and covered period so far.
    pub fn load_totals(&self) -> LoadTotals {
        let c = &self.events.load;
        LoadTotals {
            busy_ns: c.busy_ns.load(Ordering::Relaxed),
            period_ns: c.period_ns.load(Ordering::Relaxed),
        }
    }

    /// The largest share of its period a single callback took since the previous call
    /// (1.0 = the whole period); `None` when no callback ran since. One reader per stream:
    /// a second one would take the peaks the first should see.
    pub fn take_peak(&self) -> Option<f64> {
        match self.events.load.peak_ppm.swap(0, Ordering::Relaxed) {
            0 => None,
            p => Some((p - 1) as f64 * 1e-6),
        }
    }

    /// Xruns the host reported so far.
    pub fn xruns(&self) -> u64 {
        self.events.snapshot().xruns
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn load_is_callback_time_over_covered_period_and_peaks_are_taken_once() {
        let ev = Arc::new(BackendEvents::default());
        let h = StreamHealth::new(Arc::clone(&ev));
        assert_eq!(h.take_peak(), None, "no callback yet");
        let t = LoadTimer::new(Arc::clone(&ev), 48_000);
        let before = h.load_totals();
        // 4800 frames at 48 kHz cover 100 ms; the callback takes at least 10 ms of it.
        let start = t.start();
        std::thread::sleep(Duration::from_millis(10));
        t.finish(start, 4800);
        let after = h.load_totals();
        assert_eq!(after.period_ns - before.period_ns, 100_000_000);
        let load = after.since(&before).expect("a block was timed");
        assert!((0.1..1.0).contains(&load), "{load}");
        let peak = h.take_peak().expect("a callback ran");
        assert!((peak - load).abs() < 1e-6, "one callback: peak = mean");
        assert_eq!(h.take_peak(), None, "taken");
        assert_eq!(
            after.since(&after),
            None,
            "nothing timed in an empty window"
        );
    }

    #[test]
    fn a_second_callback_over_the_same_time_adds_its_time_but_not_the_period() {
        let ev = Arc::new(BackendEvents::default());
        let h = StreamHealth::new(Arc::clone(&ev));
        let a = LoadTimer::new(Arc::clone(&ev), 48_000);
        let b = LoadTimer::beside(Arc::clone(&ev), 48_000);
        let s = a.start();
        a.finish(s, 480);
        let s = b.start();
        b.finish(s, 480);
        assert_eq!(h.load_totals().period_ns, 10_000_000);
    }
}
