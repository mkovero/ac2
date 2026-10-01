//! Consumer-side run statistics: block counts, flags, index continuity, callback jitter.

use serde::Serialize;

use crate::block::{BlockFlags, BlockHeader};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Jitter {
    pub count: u64,
    pub mean_us: f64,
    pub min_us: f64,
    pub max_us: f64,
    pub stddev_us: f64,
    /// Largest |interval − nominal block duration|.
    pub max_abs_dev_us: f64,
}

#[derive(Debug, Default)]
struct Accum {
    n: u64,
    sum: f64,
    sum_sq: f64,
    min: f64,
    max: f64,
    max_abs_dev: f64,
}

impl Accum {
    fn add(&mut self, v_us: f64, nominal_us: f64) {
        if self.n == 0 {
            self.min = v_us;
            self.max = v_us;
        }
        self.n += 1;
        self.sum += v_us;
        self.sum_sq += v_us * v_us;
        self.min = self.min.min(v_us);
        self.max = self.max.max(v_us);
        self.max_abs_dev = self.max_abs_dev.max((v_us - nominal_us).abs());
    }

    fn finish(&self) -> Jitter {
        if self.n == 0 {
            return Jitter::default();
        }
        let n = self.n as f64;
        let mean = self.sum / n;
        Jitter {
            count: self.n,
            mean_us: mean,
            min_us: self.min,
            max_us: self.max,
            stddev_us: (self.sum_sq / n - mean * mean).max(0.0).sqrt(),
            max_abs_dev_us: self.max_abs_dev,
        }
    }
}

#[derive(Debug)]
pub struct RunStats {
    sample_rate: f64,
    pub blocks: u64,
    pub frames: u64,
    pub frame_sizes: std::collections::BTreeMap<u32, u64>,
    pub flagged_first: u64,
    pub flagged_xrun: u64,
    pub flagged_discontinuity: u64,
    pub flagged_overflow: u64,
    pub flagged_config_change: u64,
    pub flagged_gap_estimated: u64,
    /// Blocks whose start_sample did not equal the previous end_sample.
    pub index_gaps: u64,
    /// Sum of forward index jumps, frames.
    pub gap_frames: u64,
    /// start_sample went backwards (must never happen).
    pub index_regressions: u64,
    last: Option<BlockHeader>,
    callback: Accum,
    capture: Accum,
    /// capture_ns lag behind callback_ns, the backend's stated input latency (µs).
    input_lag: Accum,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunReport {
    pub blocks: u64,
    pub frames: u64,
    pub frame_sizes: std::collections::BTreeMap<u32, u64>,
    pub flagged_first: u64,
    pub flagged_xrun: u64,
    pub flagged_discontinuity: u64,
    pub flagged_overflow: u64,
    pub flagged_config_change: u64,
    pub flagged_gap_estimated: u64,
    pub index_gaps: u64,
    pub gap_frames: u64,
    pub index_regressions: u64,
    pub callback_interval: Jitter,
    pub capture_interval: Jitter,
    pub input_lag_us: Jitter,
}

impl RunStats {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: f64::from(sample_rate),
            blocks: 0,
            frames: 0,
            frame_sizes: Default::default(),
            flagged_first: 0,
            flagged_xrun: 0,
            flagged_discontinuity: 0,
            flagged_overflow: 0,
            flagged_config_change: 0,
            flagged_gap_estimated: 0,
            index_gaps: 0,
            gap_frames: 0,
            index_regressions: 0,
            last: None,
            callback: Accum::default(),
            capture: Accum::default(),
            input_lag: Accum::default(),
        }
    }

    pub fn add(&mut self, h: &BlockHeader) {
        self.blocks += 1;
        self.frames += u64::from(h.frames);
        *self.frame_sizes.entry(h.frames).or_insert(0) += 1;
        let f = h.flags;
        let count = |flag: BlockFlags, c: &mut u64| {
            if f.contains(flag) {
                *c += 1;
            }
        };
        count(BlockFlags::FIRST, &mut self.flagged_first);
        count(BlockFlags::XRUN, &mut self.flagged_xrun);
        count(BlockFlags::DISCONTINUITY, &mut self.flagged_discontinuity);
        count(BlockFlags::OVERFLOW, &mut self.flagged_overflow);
        count(BlockFlags::CONFIG_CHANGE, &mut self.flagged_config_change);
        count(BlockFlags::GAP_ESTIMATED, &mut self.flagged_gap_estimated);
        if let Some(cap) = h.capture_ns {
            self.input_lag
                .add(h.callback_ns as f64 / 1e3 - cap as f64 / 1e3, 0.0);
        }
        if let Some(prev) = self.last {
            let nominal_us = f64::from(prev.frames) / self.sample_rate * 1e6;
            match h.start_sample.cmp(&prev.end_sample()) {
                std::cmp::Ordering::Equal => {}
                std::cmp::Ordering::Greater => {
                    self.index_gaps += 1;
                    self.gap_frames += h.start_sample - prev.end_sample();
                }
                std::cmp::Ordering::Less => self.index_regressions += 1,
            }
            // Jitter only across contiguous blocks: a gap interval is not scheduling noise.
            if !h.flags.intersects(BlockFlags::BREAKS_CONTINUITY) {
                self.callback.add(
                    (h.callback_ns as f64 - prev.callback_ns as f64) / 1e3,
                    nominal_us,
                );
                if let (Some(a), Some(b)) = (prev.capture_ns, h.capture_ns) {
                    self.capture.add((b as f64 - a as f64) / 1e3, nominal_us);
                }
            }
        }
        self.last = Some(*h);
    }

    pub fn report(&self) -> RunReport {
        RunReport {
            blocks: self.blocks,
            frames: self.frames,
            frame_sizes: self.frame_sizes.clone(),
            flagged_first: self.flagged_first,
            flagged_xrun: self.flagged_xrun,
            flagged_discontinuity: self.flagged_discontinuity,
            flagged_overflow: self.flagged_overflow,
            flagged_config_change: self.flagged_config_change,
            flagged_gap_estimated: self.flagged_gap_estimated,
            index_gaps: self.index_gaps,
            gap_frames: self.gap_frames,
            index_regressions: self.index_regressions,
            callback_interval: self.callback.finish(),
            capture_interval: self.capture.finish(),
            input_lag_us: self.input_lag.finish(),
        }
    }
}
