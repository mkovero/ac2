//! When the next frame of a rate-limited stream may go.
//!
//! Frames can only go out when a thread is awake, and threads wake on capture hand-offs
//! whose timing jitters: a scheduler late by a few milliseconds, a timer coalesced by the
//! OS. Spacing frames by at least a period from the previous *actual* send loses rate to
//! every late wakeup (a frame due at 33.3 ms that finds the wakeups at 32 and 49 ms goes
//! at 49, and the period starts again from there), which halves the rate when wakeups are
//! about a period apart. A cadence keeps a grid of slots instead: each frame takes the
//! next slot, so a late frame is followed by an earlier one and the average rate holds.

use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub(crate) struct Cadence {
    period: Duration,
    /// How far ahead of its slot a frame may go: wakeups land on either side of a slot, and
    /// one just before it would otherwise push the frame to the wakeup after.
    early: Duration,
    /// The next slot; `None` until the first frame, which goes at once.
    next: Option<Instant>,
}

impl Cadence {
    /// Frames every `period` on average, each up to `early` before its slot.
    pub(crate) fn new(period: Duration, early: Duration) -> Self {
        Self {
            period,
            early: early.min(period / 2),
            next: None,
        }
    }

    /// When the next frame may go; `None`: at once.
    pub(crate) fn ready_at(&self) -> Option<Instant> {
        self.next.map(|n| n.checked_sub(self.early).unwrap_or(n))
    }

    pub(crate) fn is_ready(&self, now: Instant) -> bool {
        self.ready_at().is_none_or(|t| now >= t)
    }

    /// A frame went at `now`. The next slot is a period after this frame's slot, but the
    /// next frame never goes sooner than half a period after this one: a frame that went
    /// far behind its slot (the thread stalled) is not followed by a burst of catch-up
    /// frames, the grid restarts instead.
    pub(crate) fn take(&mut self, now: Instant) {
        let slot = self.next.unwrap_or(now) + self.period;
        self.next = Some(slot.max(now + self.period / 2 + self.early));
    }

    /// The next frame goes at once.
    pub(crate) fn reset(&mut self) {
        self.next = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames sent at `wakeups` (ms) when ready; their count.
    fn run(period_ms: f64, early_ms: f64, wakeups: impl IntoIterator<Item = f64>) -> usize {
        let t0 = Instant::now();
        let ms = |v: f64| Duration::from_secs_f64(v / 1e3);
        let mut c = Cadence::new(ms(period_ms), ms(early_ms));
        let mut n = 0;
        for w in wakeups {
            let now = t0 + ms(w);
            if c.is_ready(now) {
                c.take(now);
                n += 1;
            }
        }
        n
    }

    #[test]
    fn jittered_wakeups_near_the_period_keep_the_rate() {
        // Wakeups every 16.7 ms ± 3 ms for 10 s; frames at 30 Hz.
        let wakeups = (0..600).map(|i| {
            let jitter = if i % 2 == 0 { 3.0 } else { -3.0 };
            f64::from(i) * 50.0 / 3.0 + jitter
        });
        let n = run(1000.0 / 30.0, 25.0 / 3.0, wakeups);
        assert!((299..=301).contains(&n), "{n} frames in 10 s");
    }

    #[test]
    fn irregular_wakeups_average_out() {
        // Wakeups alternately 20 and 30 ms apart (mean 25 ms): every 33 ms slot has one
        // within reach, so frames keep 30 Hz.
        let mut t = 0.0;
        let wakeups: Vec<f64> = (0..400)
            .map(|i| {
                t += if i % 2 == 0 { 20.0 } else { 30.0 };
                t
            })
            .collect();
        let secs = t / 1e3;
        let n = run(1000.0 / 30.0, 25.0 / 3.0, wakeups) as f64;
        // Spacing only by the time since the previous frame gives 20 Hz here (every
        // second wakeup, 50 ms apart).
        assert!(n / secs > 29.0, "{:.1} Hz", n / secs);
    }

    #[test]
    fn a_stall_restarts_the_grid_without_a_burst() {
        let t0 = Instant::now();
        let ms = |v: f64| Duration::from_secs_f64(v / 1e3);
        let mut c = Cadence::new(ms(50.0), ms(12.5));
        c.take(t0);
        // A 500 ms stall: one frame, then the next no sooner than half a period later.
        let late = t0 + ms(550.0);
        assert!(c.is_ready(late));
        c.take(late);
        assert!(!c.is_ready(late + ms(1.0)));
        assert!(!c.is_ready(late + ms(24.0)));
        assert!(c.is_ready(late + ms(25.0)));
    }

    #[test]
    fn the_first_frame_goes_at_once_and_early_is_bounded() {
        let t0 = Instant::now();
        let ms = |v: f64| Duration::from_secs_f64(v / 1e3);
        let mut c = Cadence::new(ms(20.0), ms(100.0));
        assert!(c.is_ready(t0));
        c.take(t0);
        assert!(!c.is_ready(t0 + ms(9.0)));
        assert!(c.is_ready(t0 + ms(10.0)));
        c.reset();
        assert!(c.is_ready(t0));
    }
}
