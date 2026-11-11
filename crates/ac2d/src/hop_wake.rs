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
    /// Woken this long after the predicted arrival: the device's delivery jitter and the
    /// wake's own lateness. Everything here is added to every frame's latency, so it is a
    /// fraction of a period, and at most what a scheduler's jitter needs.
    slack: Duration,
    /// How often a block predicted but not yet in is looked for: half the slack, so the
    /// estimate it resets to lies within half the slack before the arrival, and the next
    /// hop's wake still comes after its block.
    poll: Duration,
    /// A sample and an instant by which it had arrived: the earliest such bound seen,
    /// carried forward at the nominal rate.
    anchor: Option<(Instant, u64)>,
    /// The end sample the last wake was planned for.
    target: u64,
    /// When the target's block was first found late (predicted, not in).
    late_since: Option<Instant>,
    /// The previous target's block was found late too.
    late_before: bool,
    /// A block has been found late once: the estimate has met the device's phase from
    /// below, and creeps back slowly from then on.
    settled: bool,
    /// The anchor is when its block arrived, not a bound: only the device's jitter and
    /// the wake's lateness need slack, and half of it does.
    exact: bool,
}

/// Each new target starts this fraction of the time since the previous one earlier than
/// the estimate, so a too-late estimate (blocks seen only some time after they arrived)
/// creeps back until a wake finds the block not yet in and resets it from below. A
/// Half a thousandth: above any device clock's deviation (a few tens of ppm), so the
/// estimate never trails the device. Until the first such reset it creeps by a poll per
/// target, as the first estimate may lie up to a period late.
const CREEP: f64 = 5e-4;

/// The most slack: a scheduler wakes a thread within a few tens of microseconds, a
/// device's callback jitters by about as much.
const MAX_SLACK: Duration = Duration::from_micros(400);

impl HopWake {
    pub(crate) fn new(rate: f64, period: Duration) -> Self {
        let slack = (period / 8).min(MAX_SLACK);
        Self {
            rate,
            period,
            slack,
            poll: slack / 2,
            anchor: None,
            target: 0,
            late_since: None,
            late_before: false,
            settled: false,
            exact: false,
        }
    }

    /// The end sample the last wake was planned for (0 before one was).
    pub(crate) fn target(&self) -> u64 {
        self.target
    }

    /// Audio up to `end` was in at `at`: when it arrived if `exact`, else some time
    /// before.
    pub(crate) fn seen(&mut self, at: Instant, end: u64, exact: bool) {
        self.exact = exact;
        if !exact
            && let Some(a) = self.anchor
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
            let gap = due.saturating_sub(self.target.max(a.1)) as f64 / self.rate;
            let creep = if self.settled {
                Duration::try_from_secs_f64(gap * CREEP)
                    .unwrap_or_default()
                    .min(self.poll)
            } else {
                self.poll
            };
            self.target = due;
            self.late_before = self.late_since.take().is_some();
            a.0 = a.0.checked_sub(creep).unwrap_or(a.0);
        }
        let Some(expected) = self.arrival(a, due) else {
            return now + self.period;
        };
        let slack = if self.exact { self.poll } else { self.slack };
        if now < expected + slack {
            self.anchor = Some(a);
            return expected + slack;
        }
        // Not in although predicted: it arrives after now, a tighter bound than the
        // estimate when two blocks running are late (one late block is the device's or the
        // scheduler's hiccup, not its phase). Look again shortly, then every period if the
        // stream has stalled.
        self.settled = true;
        self.anchor = Some(if self.late_before { (now, due) } else { a });
        let since = *self.late_since.get_or_insert(expected);
        if now < since + self.period {
            now + self.poll
        } else {
            now + self.period
        }
    }

    /// When `end` arrives (or arrived) if `a` did as stated.
    fn arrival(&self, a: (Instant, u64), end: u64) -> Option<Instant> {
        let span = |n: u64| Duration::try_from_secs_f64(n as f64 / self.rate).ok();
        if end >= a.1 {
            a.0.checked_add(span(end - a.1)?)
        } else {
            a.0.checked_sub(span(a.1 - end)?)
        }
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

/// When a block popped at `popped` was in, and whether that is when it arrived: its
/// callback's stamp `callback_ns` put on this host's clock by `stamp_now`, the stream's
/// clock read just after `popped`. Otherwise `popped` itself, a bound: where the stream's
/// clock cannot be read, or reads the block as from the future, from long ago, or from
/// this very instant (a clock that moves only with the audio, such as a simulation's).
pub(crate) fn in_by(popped: Instant, stamp_now: Option<u64>, callback_ns: u64) -> (Instant, bool) {
    stamp_now
        .and_then(|n| n.checked_sub(callback_ns))
        .filter(|&d| d > 0 && d < STAMP_TRUST_NS)
        .and_then(|d| popped.checked_sub(Duration::from_nanos(d)))
        .map_or((popped, false), |t| (t, true))
}

/// A block older than this when popped is a stall, not a measure of when it arrived.
const STAMP_TRUST_NS: u64 = 1_000_000_000;

/// Width of a [`WakeLog`] bucket.
const BUCKET: Duration = Duration::from_micros(50);
/// Buckets of a [`WakeLog`] histogram: 0 … 10 ms, the last one everything longer.
const BUCKETS: usize = 200;
/// How often [`WakeLog`] says what it saw.
const LOG_EVERY: Duration = Duration::from_secs(60);

/// How long after its block arrived each spectrum frame's block was taken, and how long
/// the fan-out took to hand it to the jobs, said at debug level once a minute
/// (`RUST_LOG=…,ac2d::hop_wake=debug`): what a frame's `capture_wall_ns` (stamped when the
/// block is popped) cannot show.
#[derive(Debug)]
pub(crate) struct WakeLog {
    since: Instant,
    wakes: u32,
    /// Device callback → popped here.
    pickup: Histogram,
    /// Woken → the block handed to the jobs.
    handed: Histogram,
}

#[derive(Debug)]
struct Histogram {
    counts: [u32; BUCKETS],
    max: Duration,
}

impl Histogram {
    fn new() -> Self {
        Self {
            counts: [0; BUCKETS],
            max: Duration::ZERO,
        }
    }

    fn add(&mut self, d: Duration) {
        let i = (d.as_nanos() / BUCKET.as_nanos()).min(BUCKETS as u128 - 1) as usize;
        self.counts[i] += 1;
        self.max = self.max.max(d);
    }

    fn total(&self) -> u32 {
        self.counts.iter().sum()
    }

    /// The upper edge of the bucket holding quantile `q`, ms.
    fn quantile_ms(&self, q: f64) -> f64 {
        let rank = (f64::from(self.total()) * q).ceil().max(1.0) as u32;
        let mut seen = 0;
        for (i, &c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= rank {
                return BUCKET.as_secs_f64() * 1e3 * (i + 1) as f64;
            }
        }
        self.max.as_secs_f64() * 1e3
    }

    fn summary(&self) -> String {
        format!(
            "p50 {:.2} / p95 {:.2} / max {:.2} ms over {}",
            self.quantile_ms(0.5),
            self.quantile_ms(0.95),
            self.max.as_secs_f64() * 1e3,
            self.total()
        )
    }
}

impl WakeLog {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            since: now,
            wakes: 0,
            pickup: Histogram::new(),
            handed: Histogram::new(),
        }
    }

    /// A frame's block was popped `ns` after its callback ran (stream clock).
    pub(crate) fn pickup(&mut self, ns: u64) {
        self.pickup.add(Duration::from_nanos(ns));
    }

    /// A frame's block reached the jobs `d` after the fan-out woke.
    pub(crate) fn handed(&mut self, d: Duration) {
        self.handed.add(d);
    }

    /// One wake of the fan-out ended at `now`; says what it saw once a minute.
    pub(crate) fn wake(&mut self, now: Instant) {
        self.wakes += 1;
        let span = now.saturating_duration_since(self.since);
        if span < LOG_EVERY {
            return;
        }
        tracing::debug!(
            "fan-out over {:.0} s: {:.1} wakes/s; frame block popped after its callback {}; \
             handed to the jobs after the wake {}",
            span.as_secs_f64(),
            f64::from(self.wakes) / span.as_secs_f64(),
            self.pickup.summary(),
            self.handed.summary(),
        );
        *self = Self::new(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How a simulated device and fan-out keep time.
    #[derive(Clone, Copy, Debug)]
    struct Rig {
        /// The device's clock against the host's.
        ppm: f64,
        /// Every block arrives up to this late, uniformly.
        jitter: Duration,
        /// One block in 64 arrives up to this much later still (a scheduling hiccup).
        hiccup: Duration,
        /// Every wake comes up to this late (timer slack, scheduling).
        wake_late: Duration,
        /// The fan-out reads each block's callback time on the host's clock ([`in_by`]
        /// with a stream clock); otherwise only that it was in when popped.
        stamped: bool,
    }

    /// Pseudo-random in 0 … 1 from `k`.
    fn unit(k: u64, salt: u64) -> f64 {
        ((k.wrapping_add(salt)).wrapping_mul(2_654_435_761) % 1000) as f64 / 1000.0
    }

    /// A device delivering 256-sample blocks at 96 kHz on `rig`'s clock, read by a fan-out
    /// that wakes as [`HopWake`] says for frames every 1536 samples. Returns wakes per
    /// second of audio and each frame's delay from the arrival of the block ending it to
    /// the wake that took it.
    fn simulate(rig: Rig, seconds: f64) -> (f64, Vec<Duration>) {
        let (rate, period, hop) = (96_000.0, 256u64, 1536u64);
        let p = Duration::from_secs_f64(period as f64 / rate);
        let t0 = Instant::now();
        // Arrival of the block ending at sample `(k + 1) × period`.
        let arrive = |k: u64| {
            let late = rig.jitter.mul_f64(unit(k, 0))
                + if k % 64 == 17 {
                    rig.hiccup.mul_f64(unit(k, 7))
                } else {
                    Duration::ZERO
                };
            t0 + Duration::from_secs_f64(
                (k + 1) as f64 * period as f64 / (rate * (1.0 + rig.ppm * 1e-6)),
            ) + late
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
                let at = if rig.stamped { arrive(got - 1) } else { now };
                w.seen(at, end, rig.stamped);
            }
            let due = (end / hop + 1) * hop;
            // Other jobs and the meters keep their old cadence.
            let next = w
                .next(now, Some(due))
                .min(now + Duration::from_secs_f64(1.0 / 60.0));
            now = next.max(now + Duration::from_micros(1)) + rig.wake_late.mul_f64(unit(wakes, 3));
            wakes += 1;
        }
        (wakes as f64 / seconds, delays)
    }

    fn percentile(d: &[Duration], q: f64) -> Duration {
        let mut v = d.to_vec();
        v.sort();
        v[((v.len() - 1) as f64 * q) as usize]
    }

    #[test]
    fn wakes_about_once_a_hop_and_takes_each_frame_soon_after_it_arrives() {
        let p = Duration::from_secs_f64(256.0 / 96_000.0);
        let us = Duration::from_micros;
        // JACK on a real-time thread, read by an ordinary one.
        let jack = Rig {
            ppm: 0.0,
            jitter: us(40),
            hiccup: us(800),
            wake_late: us(60),
            stamped: true,
        };
        let bound = Rig {
            stamped: false,
            ..jack
        };
        // (rig, wakes per hop, p50, p95 from the block's arrival to the wake taking it)
        for (rig, wakes, p50_max, p95_max) in [
            (jack, 1.1, us(250), us(300)),
            (Rig { ppm: 300.0, ..jack }, 1.1, us(250), us(300)),
            (
                Rig {
                    ppm: -300.0,
                    ..jack
                },
                1.1,
                us(250),
                us(300),
            ),
            (bound, 1.15, us(250), us(700)),
            (
                Rig {
                    ppm: 300.0,
                    ..bound
                },
                1.15,
                us(250),
                us(700),
            ),
            // A third of a period of jitter on every block: polled more, never a period late.
            (
                Rig {
                    jitter: p / 3,
                    ..bound
                },
                1.3,
                us(450),
                us(1000),
            ),
        ] {
            let (per_s, delays) = simulate(rig, 30.0);
            let hops_per_s = 96_000.0 / 1536.0;
            assert!(
                per_s < hops_per_s * wakes,
                "{per_s:.1} wakes/s for {hops_per_s} hops/s ({rig:?})"
            );
            // The first frames come before the device's phase is known.
            let d = &delays[4..];
            let (p50, p95) = (percentile(d, 0.5), percentile(d, 0.95));
            assert!(
                p50 <= p50_max && p95 <= p95_max,
                "{p50:?} {p95:?} ({rig:?})"
            );
            let worst = percentile(d, 1.0);
            assert!(worst <= p, "{worst:?} late ({rig:?})");
        }
    }

    #[test]
    fn an_earlier_block_popped_later_keeps_the_estimate_of_a_later_one() {
        // A wake that found the frame's block not in pins the estimate on that block; a poll
        // then taking only the block before it is no news about the frame's.
        let p = Duration::from_secs_f64(256.0 / 96_000.0);
        let mut w = HopWake::new(96_000.0, p);
        let t0 = Instant::now();
        w.seen(t0, 3072, false);
        w.seen(t0 + p / 4, 2816, false);
        assert_eq!(w.anchor, Some((t0, 3072)));
    }

    #[test]
    fn a_stalled_stream_is_polled_every_period() {
        let p = Duration::from_millis(2);
        let mut w = HopWake::new(96_000.0, p);
        let t0 = Instant::now();
        w.seen(t0, 1536, false);
        let mut now = t0;
        let mut steps = Vec::new();
        for _ in 0..40 {
            let next = w.next(now, Some(3072));
            steps.push(next - now);
            now = next;
        }
        // Woken for the predicted arrival, then shortly after it, then once a period.
        assert!(steps[0] > p * 8, "{steps:?}");
        assert!(steps[1..4].iter().all(|s| *s < p), "{steps:?}");
        assert!(steps.iter().skip(30).all(|s| *s == p), "{steps:?}");
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
