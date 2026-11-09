//! The receiver's accounting for `selftest net`: sequence gaps, and how the arrivals spread.
//!
//! Each side reads its own REALTIME, so `arrived − sent` holds the hosts' clock offset as
//! well as the network's delay. Over a run the offset moves only as fast as the clocks'
//! discipline slews it, so subtracting the run's smallest `arrived − sent` leaves the
//! delay's variation above its floor: the part a remote stream's buffer must absorb.

use serde::Serialize;

use super::{Ended, InBand, NetSpec, RECORD_BYTES, ReceiveEnd, SendEnd, offset_ns};

/// One block as read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Arrival {
    pub seq: u64,
    pub scheduled_ns: u64,
    pub sent_ns: u64,
    pub arrived_ns: u64,
    pub stalls: u64,
    pub late_sends: u64,
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}

impl Arrival {
    /// From a block header (`BLOCK_HEADER_BYTES` long) and the instant it was read.
    pub fn from_header(h: &[u8], arrived_ns: u64) -> Self {
        Self {
            seq: u64_at(h, 0),
            scheduled_ns: u64_at(h, 8),
            sent_ns: u64_at(h, 16),
            stalls: u64_at(h, 24),
            late_sends: u64_at(h, 32),
            arrived_ns,
        }
    }

    /// The `arrivals.bin` record.
    pub fn to_bytes(self) -> [u8; RECORD_BYTES] {
        let mut r = [0u8; RECORD_BYTES];
        r[0..8].copy_from_slice(&self.seq.to_le_bytes());
        r[8..16].copy_from_slice(&self.scheduled_ns.to_le_bytes());
        r[16..24].copy_from_slice(&self.sent_ns.to_le_bytes());
        r[24..32].copy_from_slice(&self.arrived_ns.to_le_bytes());
        r
    }
}

/// p50 / p99 / p99.9 / max of a set of durations, µs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Quantiles {
    /// Median.
    pub p50_us: f64,
    /// 99th percentile.
    pub p99_us: f64,
    /// 99.9th percentile.
    pub p999_us: f64,
    /// Largest.
    pub max_us: f64,
}

/// Nearest-rank quantiles of `v` (ns), sorting it.
pub(crate) fn quantiles(v: &mut [i64]) -> Quantiles {
    if v.is_empty() {
        return Quantiles::default();
    }
    v.sort_unstable();
    let n = v.len();
    let at = |p: f64| {
        let rank = (p * n as f64).ceil() as usize;
        v[rank.clamp(1, n) - 1] as f64 / 1e3
    };
    Quantiles {
        p50_us: at(0.5),
        p99_us: at(0.99),
        p999_us: at(0.999),
        max_us: v[n - 1] as f64 / 1e3,
    }
}

/// How the blocks arrived.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct NetSummary {
    /// Blocks counted.
    pub blocks: u64,
    /// Smallest `arrived − sent`, µs: the network's floor plus the hosts' clock offset.
    pub one_way_min_us: f64,
    /// `arrived − sent` above its smallest.
    pub one_way: Quantiles,
    /// `arrived − scheduled` above its smallest: adds the sender's own lateness.
    pub vs_schedule: Quantiles,
    /// How far each interval between arrivals lies from the blocks' scheduled spacing.
    pub interval_jitter: Quantiles,
    /// One block period, µs.
    pub block_period_us: f64,
    /// A remote stream's buffer delay: one-way p99.9 above the floor, plus one block (a
    /// block exists only once its last frame was captured).
    pub link_offset_ms: f64,
}

/// The receiver's running counts.
#[derive(Debug)]
pub(crate) struct Receiver {
    frames: u32,
    rate: u32,
    gaps_listed: usize,
    one_way: Vec<i64>,
    vs_schedule: Vec<i64>,
    intervals: Vec<i64>,
    last: Option<(u64, u64)>,
    received: u64,
    next: u64,
    gaps: Vec<[u64; 2]>,
    reordered: u64,
    in_band: InBand,
}

fn diff(a: u64, b: u64) -> i64 {
    (i128::from(a) - i128::from(b)).clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

impl Receiver {
    /// For the sender's stream; lists at most `gaps_listed` gaps.
    pub fn new(spec: &NetSpec, gaps_listed: usize) -> Self {
        Self {
            frames: spec.frames,
            rate: spec.rate,
            gaps_listed,
            one_way: Vec::new(),
            vs_schedule: Vec::new(),
            intervals: Vec::new(),
            last: None,
            received: 0,
            next: 0,
            gaps: Vec::new(),
            reordered: 0,
            in_band: InBand::default(),
        }
    }

    pub(crate) fn push(&mut self, a: &Arrival) {
        self.received += 1;
        self.one_way.push(diff(a.arrived_ns, a.sent_ns));
        self.vs_schedule.push(diff(a.arrived_ns, a.scheduled_ns));
        match self.last {
            Some((seq, at)) if a.seq > seq => {
                let spacing = offset_ns(a.seq, self.frames, self.rate)
                    - offset_ns(seq, self.frames, self.rate);
                let interval = diff(a.arrived_ns, at);
                self.intervals
                    .push(interval.saturating_sub(spacing as i64).saturating_abs());
                self.last = Some((a.seq, a.arrived_ns));
            }
            Some(_) => {}
            None => self.last = Some((a.seq, a.arrived_ns)),
        }
        if a.seq < self.next {
            self.reordered += 1;
        } else {
            if a.seq > self.next && self.gaps.len() < self.gaps_listed {
                self.gaps.push([self.next, a.seq - 1]);
            }
            self.next = a.seq + 1;
        }
        self.in_band = InBand {
            stalls: a.stalls,
            late_sends: a.late_sends,
        };
    }

    /// The closing counts and the timing summary.
    pub fn finish(&mut self, ended: Ended, seconds: f64, sender: Option<SendEnd>) -> ReceiveEnd {
        let total = sender
            .as_ref()
            .map_or(self.next, |s| s.blocks.max(self.next));
        ReceiveEnd {
            ended,
            seconds,
            received: self.received,
            missing: total.saturating_sub(self.received),
            gaps: self.gaps.clone(),
            reordered: self.reordered,
            in_band: self.in_band,
            sender,
            summary: self.summary(),
        }
    }

    fn summary(&mut self) -> Option<NetSummary> {
        let min = *self.one_way.iter().min()?;
        let sched_min = self.vs_schedule.iter().min().copied().unwrap_or(0);
        let mut above: Vec<i64> = self.one_way.iter().map(|d| d - min).collect();
        let mut sched: Vec<i64> = self.vs_schedule.iter().map(|d| d - sched_min).collect();
        let one_way = quantiles(&mut above);
        let period_ns = offset_ns(1_000_000, self.frames, self.rate) as f64 / 1e6;
        Some(NetSummary {
            blocks: self.received,
            one_way_min_us: min as f64 / 1e3,
            one_way,
            vs_schedule: quantiles(&mut sched),
            interval_jitter: quantiles(&mut self.intervals),
            block_period_us: period_ns / 1e3,
            link_offset_ms: (one_way.p999_us + period_ns / 1e3) / 1e3,
        })
    }
}
