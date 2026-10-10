//! When the next frame of a rate-limited stream may go.
//!
//! Frames can only go out when a thread is awake, and threads wake on capture hand-offs
//! whose timing jitters: a scheduler late by a few milliseconds, a timer coalesced by the
//! OS. Spacing frames by at least a period from the previous *actual* send loses rate to
//! every late wakeup (a frame due at 33.3 ms that finds the wakeups at 32 and 49 ms goes
//! at 49, and the period starts again from there), which halves the rate when wakeups are
//! about a period apart. A cadence keeps a grid of slots instead: each frame takes the
//! next slot, so a late frame is followed by an earlier one and the average rate holds.
//!
//! The grid suits results that come at least as often as the period. A result that comes
//! less often lands between slots and waits for the next one, up to a period: a spectrum
//! every 85 ms on a 33 ms grid goes out 67 or 100 ms apart, never 85, and the display
//! stutters. Such a result goes as soon as it is formed, provided the previous result went
//! a period or more before, and the grid restarts from it.

use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub(crate) struct Cadence {
    period: Duration,
    /// How far ahead of its slot a frame may go: wakeups land on either side of a slot, and
    /// one just before it would otherwise push the frame to the wakeup after.
    early: Duration,
    /// The next slot; `None` until the first frame, which goes at once.
    next: Option<Instant>,
    /// When the last frame carrying a new result went.
    result_at: Option<Instant>,
}

impl Cadence {
    /// Frames every `period` on average, each up to `early` before its slot.
    pub(crate) fn new(period: Duration, early: Duration) -> Self {
        Self {
            period,
            early: early.min(period / 2),
            next: None,
            result_at: None,
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

    /// Whether a frame carrying a result newer than the previous frame's may go at `now`:
    /// on the grid, or at once when the previous new result went a period or more before
    /// (the result is slower than the grid, so the period is honoured between results).
    pub(crate) fn is_ready_for_result(&self, now: Instant) -> bool {
        self.is_ready(now) || self.result_at.is_none_or(|t| now >= t + self.period)
    }

    /// A frame carrying a new result went at `now`. One that went ahead of its slot starts
    /// the grid anew from here, so the frames after it keep a period from it.
    pub(crate) fn take_result(&mut self, now: Instant) {
        if self.is_ready(now) {
            self.take(now);
        } else {
            self.next = Some(now + self.period);
        }
        self.result_at = Some(now);
    }

    /// The next frame goes at once.
    pub(crate) fn reset(&mut self) {
        self.next = None;
        self.result_at = None;
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

    /// Frames at `wakeups` (ms) of results formed at `results` (ms), each frame going on the
    /// first wakeup it is ready; when the frames carrying a new result went.
    fn result_frames(
        period_ms: f64,
        early_ms: f64,
        wakeups: impl IntoIterator<Item = f64>,
        results: &[f64],
    ) -> Vec<f64> {
        let t0 = Instant::now();
        let ms = |v: f64| Duration::from_secs_f64(v / 1e3);
        let mut c = Cadence::new(ms(period_ms), ms(early_ms));
        let (mut formed, mut sent) = (0usize, 0usize);
        let mut out = Vec::new();
        for w in wakeups {
            let now = t0 + ms(w);
            while formed < results.len() && results[formed] <= w {
                formed += 1;
            }
            let fresh = formed > sent;
            if fresh && c.is_ready_for_result(now) {
                c.take_result(now);
                sent = formed;
                out.push(w);
            } else if c.is_ready(now) {
                c.take(now);
            }
        }
        out
    }

    #[test]
    fn a_result_slower_than_the_grid_goes_on_the_wakeup_after_it() {
        // Wakeups every 16.7 ms, a 30 Hz grid, a result every 85.3 ms (8192 samples at
        // 96 kHz): each goes on the first wakeup after it is formed, so frames are 83 or
        // 100 ms apart and never lag their result by more than a wakeup. On the grid alone
        // they would lag by up to a period, 67 or 100 ms apart.
        let wakeup = 50.0 / 3.0;
        let hop = 8192.0 / 96.0;
        let results: Vec<f64> = (1..=100).map(|k| f64::from(k) * hop + 1.0).collect();
        let wakeups = (0..600).map(|i| f64::from(i) * wakeup);
        let frames = result_frames(1000.0 / 30.0, 25.0 / 3.0, wakeups, &results);
        assert_eq!(frames.len(), 100);
        for (f, r) in frames.iter().zip(&results) {
            assert!(
                f - r < wakeup + 1e-6,
                "result at {r:.1} ms went at {f:.1} ms"
            );
        }
    }

    #[test]
    fn results_faster_than_the_grid_keep_its_rate() {
        // A new result on every 16.7 ms wakeup still goes at 30 Hz, on the grid.
        let wakeups: Vec<f64> = (0..600).map(|i| f64::from(i) * 50.0 / 3.0).collect();
        let frames = result_frames(1000.0 / 30.0, 25.0 / 3.0, wakeups.clone(), &wakeups);
        assert!(
            (299..=301).contains(&frames.len()),
            "{} frames in 10 s",
            frames.len()
        );
        for w in frames.windows(2) {
            assert!(w[1] - w[0] > 30.0, "frames {:.1} ms apart", w[1] - w[0]);
        }
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
