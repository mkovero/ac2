//! Back-to-back observation windows cut from sample-indexed ref and meas blocks.

use std::collections::VecDeque;

use super::{Block, ConfigError, FinderConfig, FinderResult, FinderScratch, find_with};

/// Buffers the ref and meas streams and runs the finder once per observation window.
///
/// Windows are back to back, so consecutive results share no samples (what [`super::Tracker`]
/// requires). A window waits until the ref covers every lag of the search range,
/// `[b − Dmax, b + Lm − Dmin)`. A block that does not continue its stream is a
/// discontinuity: that stream's buffer restarts at the new block and any window it breaks
/// is dropped.
#[derive(Debug)]
pub struct DelayStream {
    cfg: FinderConfig,
    lm: usize,
    scratch: FinderScratch,
    ref_buf: VecDeque<f32>,
    ref_start: u64,
    meas_buf: VecDeque<f32>,
    meas_start: u64,
    ref_out: Vec<f32>,
    meas_out: Vec<f32>,
}

impl DelayStream {
    /// Plans FFTs and grids for `cfg` up front.
    pub fn new(cfg: FinderConfig) -> Result<Self, ConfigError> {
        let scratch = FinderScratch::for_config(&cfg)?;
        let lm = cfg.observation_len();
        if lm == 0 {
            return Err(ConfigError::Observation(0.0));
        }
        Ok(Self {
            cfg,
            lm,
            scratch,
            ref_buf: VecDeque::new(),
            ref_start: 0,
            meas_buf: VecDeque::new(),
            meas_start: 0,
            ref_out: Vec::new(),
            meas_out: Vec::new(),
        })
    }

    pub fn config(&self) -> &FinderConfig {
        &self.cfg
    }

    /// Observation window length, samples.
    pub fn observation_len(&self) -> usize {
        self.lm
    }

    /// Drop everything buffered (config change, routing change).
    pub fn reset(&mut self) {
        self.ref_buf.clear();
        self.meas_buf.clear();
    }

    /// Buffer bound: a window plus every lag it may need, plus one window of lead.
    fn cap(&self) -> usize {
        2 * self.lm
            + (self.cfg.search.min.unsigned_abs() + self.cfg.search.max.unsigned_abs()) as usize
    }

    pub fn push_ref(&mut self, block: Block<'_>) {
        if self.ref_buf.is_empty() || block.start != self.ref_start + self.ref_buf.len() as u64 {
            self.ref_buf.clear();
            self.ref_start = block.start;
        }
        self.ref_buf.extend(block.samples);
        let cap = self.cap();
        if self.ref_buf.len() > cap {
            let drop = self.ref_buf.len() - cap;
            self.ref_buf.drain(..drop);
            self.ref_start += drop as u64;
        }
    }

    pub fn push_meas(&mut self, block: Block<'_>) {
        if self.meas_buf.is_empty() || block.start != self.meas_start + self.meas_buf.len() as u64 {
            self.meas_buf.clear();
            self.meas_start = block.start;
        }
        self.meas_buf.extend(block.samples);
        let cap = self.cap();
        if self.meas_buf.len() > cap {
            let drop = self.meas_buf.len() - cap;
            self.meas_buf.drain(..drop);
            self.meas_start += drop as u64;
        }
    }

    /// Run the finder on the next complete window, if one is ready.
    pub fn poll(&mut self) -> Result<Option<FinderResult>, ConfigError> {
        if self.ref_buf.is_empty() || self.meas_buf.is_empty() {
            return Ok(None);
        }
        let (d_min, d_max) = (self.cfg.search.min, self.cfg.search.max);
        let ref_start = self.ref_start as i128;
        let ref_end = ref_start + self.ref_buf.len() as i128;
        // the earliest window whose largest lag still has ref
        let earliest = ref_start + i128::from(d_max.max(0));
        let b = self.meas_start as i128;
        if b < earliest {
            let drop = ((earliest - b) as usize).min(self.meas_buf.len());
            self.meas_buf.drain(..drop);
            self.meas_start += drop as u64;
            if self.meas_buf.is_empty() {
                return Ok(None);
            }
        }
        if self.meas_buf.len() < self.lm {
            return Ok(None);
        }
        let b = self.meas_start as i128;
        let need_end = b + self.lm as i128 - i128::from(d_min.min(0));
        if ref_end < need_end {
            return Ok(None);
        }
        let from = (b - i128::from(d_max)).max(ref_start);
        let to = (b + self.lm as i128 - i128::from(d_min)).min(ref_end);
        self.ref_out.clear();
        if from < to {
            let (i0, i1) = ((from - ref_start) as usize, (to - ref_start) as usize);
            self.ref_out.extend(self.ref_buf.range(i0..i1));
        }
        self.meas_out.clear();
        self.meas_out.extend(self.meas_buf.range(..self.lm));
        let res = find_with(
            &mut self.scratch,
            Block {
                start: from.max(0) as u64,
                samples: &self.ref_out,
            },
            Block {
                start: self.meas_start,
                samples: &self.meas_out,
            },
            &self.cfg,
        )?;
        self.meas_buf.drain(..self.lm);
        self.meas_start += self.lm as u64;
        // ref before the next window's earliest lag is no longer needed
        let keep_from = self.meas_start as i128 - i128::from(d_max);
        if keep_from > ref_start {
            let drop = ((keep_from - ref_start) as usize).min(self.ref_buf.len());
            self.ref_buf.drain(..drop);
            self.ref_start += drop as u64;
        }
        Ok(Some(res))
    }
}
