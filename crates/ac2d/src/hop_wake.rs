//! When the fan-out wakes while a live spectrum frames on whole device periods.
//!
//! A spectrum frames every hop (several device periods), and its frame should go out
//! within about a period of the block that ends the hop. Handing audio on every device
//! period achieves that, but wakes the fan-out and every job once a period although only
//! one hand-off in a hop completes a frame. The device delivers blocks on its own clock, so
//! the instant the block ending a hop arrives can be predicted from the blocks seen so far,
//! and the fan-out sleeps until then.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::fanout::JobFeed;

/// Predicts when the device delivers a sample, and so when to wake for it.
#[derive(Debug)]
pub(crate) struct HopWake {
    rate: f64,
    period: Duration,
    /// Woken this long after the predicted arrival: jitter in the device's delivery and in
    /// the wake itself, small against a period so the frame still goes within one.
    slack: Duration,
    /// Each new target starts this much earlier than the estimate, so a too-late estimate
    /// (blocks only ever seen some time after they arrived) creeps back until a wake finds
    /// the block not yet in and resets it from below. Thousands of ppm of a period per hop:
    /// far above any device clock's deviation, so the estimate never trails the device.
    creep: Duration,
    /// A sample and an instant by which it had arrived: the earliest such bound seen,
    /// carried forward at the nominal rate.
    anchor: Option<(Instant, u64)>,
    /// The end sample the last wake was planned for.
    target: u64,
    /// When the target's block was first found late (predicted, not in).
    late_since: Option<Instant>,
}

impl HopWake {
    pub(crate) fn new(rate: f64, period: Duration) -> Self {
        Self {
            rate,
            period,
            slack: period / 4,
            creep: period / 64,
            anchor: None,
            target: 0,
            late_since: None,
        }
    }

    /// Audio up to `end` was in at `at`.
    pub(crate) fn seen(&mut self, at: Instant, end: u64) {
        if let Some(a) = self.anchor
            && end >= a.1
            && self.arrival(a, end).is_some_and(|t| t <= at)
        {
            return;
        }
        self.anchor = Some((at, end));
    }

    /// When to wake next, `due` being the end sample of the block that completes the next
    /// frame (past the audio handed on so far); `None` when no job has said, so audio goes
    /// every period.
    pub(crate) fn next(&mut self, now: Instant, due: Option<u64>) -> Instant {
        let (Some(due), Some(a)) = (due, self.anchor) else {
            return now + self.period;
        };
        let mut a = a;
        if due != self.target {
            self.target = due;
            self.late_since = None;
            a.0 = a.0.checked_sub(self.creep).unwrap_or(a.0);
        }
        let Some(expected) = self.arrival(a, due) else {
            return now + self.period;
        };
        if now < expected + self.slack {
            self.anchor = Some(a);
            return expected + self.slack;
        }
        // Not in although predicted: it arrives after now, a tighter bound than the
        // estimate. Look again shortly, then every period if the stream has stalled.
        self.anchor = Some((now, due));
        let since = *self.late_since.get_or_insert(expected);
        if now < since + self.period {
            now + self.slack
        } else {
            now + self.period
        }
    }

    /// When `end` arrives if `a` did as stated.
    fn arrival(&self, a: (Instant, u64), end: u64) -> Option<Instant> {
        let after = Duration::try_from_secs_f64(end.saturating_sub(a.1) as f64 / self.rate);
        a.0.checked_add(after.ok()?)
    }
}

/// The next hand-off `handoff` after the one planned for `next`, which ran at `now`.
pub(crate) fn schedule(next: Instant, now: Instant, handoff: Duration) -> Instant {
    let at = next + handoff;
    if at <= now {
        // Overslept (a loaded machine): start a new schedule rather than catching up with a
        // run of back-to-back hand-offs.
        now + handoff
    } else {
        at
    }
}

/// One past the last sample of the block that completes the next result of any job framing
/// on device periods, audio up to `end` having been handed on; `None` while one has not
/// said. A job still busy with earlier audio said so for a result already handed on: its
/// results come every hop, so the next lies a whole number of hops on.
pub(crate) fn next_frame_end(jobs: &BTreeMap<u64, JobFeed>, end: u64) -> Option<u64> {
    let mut first: Option<u64> = None;
    for f in jobs.values() {
        let Some(hop) = f.per_period.filter(|&h| h > 0) else {
            continue;
        };
        let due = match f.queue.due.load(Ordering::Acquire) {
            0 => return None,
            d if d > end => d,
            d => d + ((end - d) / hop + 1) * hop,
        };
        first = Some(first.map_or(due, |x| x.min(due)));
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device delivering `period`-sample blocks at `rate × (1 + ppm)` with up to `jitter`
    /// of lateness per block, read by a fan-out that wakes as [`HopWake`] says for frames
    /// every `hop` samples. Returns wakes per second of audio and each frame's delay from
    /// the arrival of the block ending it to the wake that took it.
    fn simulate(ppm: f64, jitter: Duration, seconds: f64) -> (f64, Vec<Duration>) {
        let (rate, period, hop) = (96_000.0, 256u64, 1536u64);
        let p = Duration::from_secs_f64(period as f64 / rate);
        let t0 = Instant::now();
        // Arrival of the block ending at sample `(k + 1) × period`; a pseudo-random lateness.
        let arrive = |k: u64| {
            let lateness = ((k * 2_654_435_761) % 1000) as f64 / 1000.0;
            t0 + Duration::from_secs_f64(
                (k + 1) as f64 * period as f64 / (rate * (1.0 + ppm * 1e-6)),
            ) + jitter.mul_f64(lateness)
        };
        let blocks = (seconds * rate / period as f64) as u64;
        let mut w = HopWake::new(rate, p);
        let (mut now, mut got, mut wakes) = (t0, 0u64, 0u64);
        let mut delays = Vec::new();
        while got < blocks {
            let mut end = got * period;
            while got < blocks && arrive(got) <= now {
                got += 1;
                end = got * period;
                if end.is_multiple_of(hop) {
                    delays.push(now - arrive(got - 1));
                }
            }
            if end > 0 {
                w.seen(now, end);
            }
            let due = (end / hop + 1) * hop;
            // Other jobs and the meters keep their old cadence.
            let next = w
                .next(now, Some(due))
                .min(now + Duration::from_secs_f64(1.0 / 60.0));
            now = next.max(now + Duration::from_micros(1));
            wakes += 1;
        }
        (wakes as f64 / seconds, delays)
    }

    #[test]
    fn wakes_about_once_a_hop_and_takes_each_frame_within_a_period() {
        let p = Duration::from_secs_f64(256.0 / 96_000.0);
        for (ppm, jitter) in [
            (0.0, Duration::ZERO),
            (300.0, p / 10),
            (-300.0, p / 10),
            (0.0, p / 3),
        ] {
            let (per_s, delays) = simulate(ppm, jitter, 30.0);
            let hops_per_s = 96_000.0 / 1536.0;
            assert!(
                per_s < hops_per_s * 1.15,
                "{per_s:.1} wakes/s for {hops_per_s} hops/s ({ppm} ppm, {jitter:?})"
            );
            // The first frames come before the device's phase is known.
            let worst = delays.iter().skip(4).max().copied().unwrap_or_default();
            assert!(worst <= p, "{worst:?} late ({ppm} ppm, {jitter:?})");
        }
    }

    #[test]
    fn a_stalled_stream_is_polled_every_period() {
        let p = Duration::from_millis(2);
        let mut w = HopWake::new(96_000.0, p);
        let t0 = Instant::now();
        w.seen(t0, 1536);
        let mut now = t0;
        let mut steps = Vec::new();
        for _ in 0..12 {
            let next = w.next(now, Some(3072));
            steps.push(next - now);
            now = next;
        }
        // Woken for the predicted arrival, then shortly after it, then once a period.
        assert!(steps[0] > p * 8, "{steps:?}");
        assert!(steps[1..4].iter().all(|s| *s < p), "{steps:?}");
        assert!(steps.iter().skip(8).all(|s| *s == p), "{steps:?}");
    }

    #[test]
    fn the_next_frame_end_is_the_earliest_grid_point_past_the_audio_handed_on() {
        use crate::fanout::Queue;
        use std::sync::Arc;
        let feed = |hop: Option<u64>, due: u64| {
            let queue = Arc::new(Queue::default());
            queue.due.store(due, Ordering::Relaxed);
            JobFeed {
                tx: std::sync::mpsc::channel().0,
                queue,
                thread: std::thread::current(),
                per_period: hop,
            }
        };
        let mut jobs = BTreeMap::new();
        jobs.insert(1, feed(None, 0));
        assert_eq!(
            next_frame_end(&jobs, 1000),
            None,
            "no job frames on periods"
        );
        jobs.insert(2, feed(Some(768), 2304));
        assert_eq!(next_frame_end(&jobs, 1792), Some(2304));
        // A job behind: its next grid point after the audio handed on.
        assert_eq!(next_frame_end(&jobs, 2304), Some(3072));
        assert_eq!(next_frame_end(&jobs, 4000), Some(4608));
        jobs.insert(3, feed(Some(1536), 2560));
        assert_eq!(next_frame_end(&jobs, 2400), Some(2560));
        jobs.insert(4, feed(Some(1536), 0));
        assert_eq!(next_frame_end(&jobs, 2400), None, "a job yet to say");
    }
}
