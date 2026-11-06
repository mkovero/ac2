//! Accounting of capture blocks and output ticks: counts, continuity, callback timing and
//! the device clock measured against the host clock. Pure: fed headers, never a stream.

use std::collections::BTreeMap;

use ac2_audio::{BlockFlags, BlockHeader, OutputTick};
use ac2_core::clock_rate::{self as core, RateFit};
use serde::Serialize;

/// Spread of an interval or lag, µs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Spread {
    /// Values seen.
    pub count: u64,
    /// Mean, µs.
    pub mean_us: f64,
    /// Smallest, µs.
    pub min_us: f64,
    /// Largest, µs.
    pub max_us: f64,
    /// Standard deviation, µs.
    pub stddev_us: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct Accum {
    n: u64,
    sum: f64,
    sum_sq: f64,
    min: f64,
    max: f64,
}

impl Accum {
    fn add(&mut self, v: f64) {
        if self.n == 0 {
            self.min = v;
            self.max = v;
        }
        self.n += 1;
        self.sum += v;
        self.sum_sq += v * v;
        self.min = self.min.min(v);
        self.max = self.max.max(v);
    }

    fn finish(&self) -> Option<Spread> {
        (self.n > 0).then(|| {
            let n = self.n as f64;
            let mean = self.sum / n;
            Spread {
                count: self.n,
                mean_us: mean,
                min_us: self.min,
                max_us: self.max,
                stddev_us: (self.sum_sq / n - mean * mean).max(0.0).sqrt(),
            }
        })
    }
}

/// A device clock measured against the host clock.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ClockRate {
    /// Measured rate, Hz.
    pub hz: f64,
    /// Deviation from the negotiated rate, ppm (positive: the device runs fast).
    pub ppm: f64,
    /// Seconds of audio the measurement spans.
    pub span_s: f64,
}

impl ClockRate {
    fn from_fit(fit: RateFit, nominal: f64) -> Option<Self> {
        core::ClockRate::from_fit(fit, nominal).map(|r| Self {
            hz: r.hz,
            ppm: r.ppm,
            span_s: r.span_s,
        })
    }
}

/// What the capture side delivered.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CaptureStats {
    /// Blocks received.
    pub blocks: u64,
    /// Frames received.
    pub frames: u64,
    /// Blocks per callback size (frames → blocks).
    pub block_sizes: BTreeMap<u32, u64>,
    /// Blocks flagged [`BlockFlags::XRUN`].
    pub xrun_blocks: u64,
    /// Blocks flagged [`BlockFlags::DISCONTINUITY`].
    pub discontinuity_blocks: u64,
    /// Blocks flagged [`BlockFlags::OVERFLOW`] (this test fell behind the device).
    pub overflow_blocks: u64,
    /// Blocks flagged [`BlockFlags::CONFIG_CHANGE`].
    pub config_change_blocks: u64,
    /// Blocks whose gap size is estimated from host timestamps.
    pub estimated_gap_blocks: u64,
    /// Blocks that did not start where the previous one ended.
    pub index_gaps: u64,
    /// Frames skipped by those gaps.
    pub gap_frames: u64,
    /// Blocks that started before the previous one ended (never valid).
    pub index_regressions: u64,
    /// Interval between successive contiguous callbacks.
    pub callback_interval: Option<Spread>,
    /// Interval between successive contiguous capture timestamps.
    pub capture_interval: Option<Spread>,
    /// How long before its callback a block was captured (the host's input latency).
    pub input_lag: Option<Spread>,
    /// The capture clock against the host clock.
    pub rate: Option<ClockRate>,
    /// Sample peak per block channel, dBFS (`None`: digital silence).
    pub peak_dbfs: Vec<Option<f64>>,
}

/// What the output side ran.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OutputCallbacks {
    /// Output callbacks seen.
    pub callbacks: u64,
    /// Frames rendered in them.
    pub frames: u64,
    /// Callbacks whose output index broke continuity.
    pub discontinuities: u64,
    /// Interval between successive contiguous callbacks.
    pub callback_interval: Option<Spread>,
    /// How far ahead of its callback the host expects a block to play.
    pub playback_lead: Option<Spread>,
    /// The output clock against the host clock.
    pub rate: Option<ClockRate>,
}

/// Running accounting of one stream.
#[derive(Debug)]
pub struct StreamStats {
    rate: f64,
    capture: CaptureStats,
    last: Option<BlockHeader>,
    callback: Accum,
    capture_iv: Accum,
    input_lag: Accum,
    in_fit: RateFit,
    peaks: Vec<f32>,
    out: OutputCallbacks,
    last_tick: Option<OutputTick>,
    out_iv: Accum,
    lead: Accum,
    out_fit: RateFit,
}

fn us(a: u64, b: u64) -> f64 {
    (a as f64 - b as f64) / 1e3
}

impl StreamStats {
    /// Accounting for a stream at `sample_rate` with `channels` capture channels.
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            rate: f64::from(sample_rate),
            capture: CaptureStats::default(),
            last: None,
            callback: Accum::default(),
            capture_iv: Accum::default(),
            input_lag: Accum::default(),
            in_fit: RateFit::default(),
            peaks: vec![0.0; usize::from(channels)],
            out: OutputCallbacks::default(),
            last_tick: None,
            out_iv: Accum::default(),
            lead: Accum::default(),
            out_fit: RateFit::default(),
        }
    }

    /// One capture block and its interleaved samples.
    pub fn add_block(&mut self, h: &BlockHeader, samples: &[f32]) {
        let c = &mut self.capture;
        c.blocks += 1;
        c.frames += u64::from(h.frames);
        *c.block_sizes.entry(h.frames).or_insert(0) += 1;
        for (flag, n) in [
            (BlockFlags::XRUN, &mut c.xrun_blocks),
            (BlockFlags::DISCONTINUITY, &mut c.discontinuity_blocks),
            (BlockFlags::OVERFLOW, &mut c.overflow_blocks),
            (BlockFlags::CONFIG_CHANGE, &mut c.config_change_blocks),
            (BlockFlags::GAP_ESTIMATED, &mut c.estimated_gap_blocks),
        ] {
            if h.flags.contains(flag) {
                *n += 1;
            }
        }
        if let Some(cap) = h.capture_ns {
            self.input_lag.add(us(h.callback_ns, cap));
        }
        let breaks = h.flags.breaks_continuity();
        if let Some(prev) = self.last {
            match h.start_sample.cmp(&prev.end_sample()) {
                std::cmp::Ordering::Equal => {}
                std::cmp::Ordering::Greater => {
                    c.index_gaps += 1;
                    c.gap_frames += h.start_sample - prev.end_sample();
                }
                std::cmp::Ordering::Less => c.index_regressions += 1,
            }
            // Intervals across a break are the break's length, not scheduling noise.
            if !breaks {
                self.callback.add(us(h.callback_ns, prev.callback_ns));
                if let (Some(a), Some(b)) = (prev.capture_ns, h.capture_ns) {
                    self.capture_iv.add(us(b, a));
                }
            }
        }
        // The capture timestamp is the device's own when the host gives one; the callback
        // time adds scheduling noise on top of it.
        let ts = h.capture_ns.unwrap_or(h.callback_ns);
        self.in_fit.add(h.start_sample, ts, breaks);
        let ch = usize::from(h.channels.max(1));
        for (i, v) in samples.iter().enumerate() {
            if let Some(p) = self.peaks.get_mut(i % ch) {
                *p = p.max(v.abs());
            }
        }
        self.last = Some(*h);
    }

    /// One output callback record.
    pub fn add_tick(&mut self, t: &OutputTick) {
        self.out.callbacks += 1;
        self.out.frames += u64::from(t.frames);
        let breaks = t.flags.breaks_continuity();
        if breaks {
            self.out.discontinuities += 1;
        }
        if let Some(prev) = self.last_tick
            && !breaks
        {
            self.out_iv.add(us(t.callback_ns, prev.callback_ns));
        }
        if let Some(pb) = t.playback_ns {
            self.lead.add(us(pb, t.callback_ns));
        }
        self.out_fit.add(
            t.start_sample,
            t.playback_ns.unwrap_or(t.callback_ns),
            breaks,
        );
        self.last_tick = Some(*t);
    }

    /// Frames received so far.
    pub fn frames(&self) -> u64 {
        self.capture.frames
    }

    /// The totals.
    pub fn finish(self) -> (CaptureStats, OutputCallbacks) {
        let mut c = self.capture;
        c.callback_interval = self.callback.finish();
        c.capture_interval = self.capture_iv.finish();
        c.input_lag = self.input_lag.finish();
        c.rate = ClockRate::from_fit(self.in_fit, self.rate);
        c.peak_dbfs = self
            .peaks
            .iter()
            .map(|&p| (p > 0.0).then(|| 20.0 * f64::from(p).log10()))
            .collect();
        let mut o = self.out;
        o.callback_interval = self.out_iv.finish();
        o.playback_lead = self.lead.finish();
        o.rate = ClockRate::from_fit(self.out_fit, self.rate);
        (c, o)
    }
}
