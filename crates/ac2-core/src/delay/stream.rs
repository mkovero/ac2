//! Back-to-back observation windows cut from sample-indexed ref and meas blocks.

use std::collections::VecDeque;

use super::{
    Block, ConfigError, FinderConfig, FinderResult, FinderScratch, Local, Outcome, SearchRange,
    find_in,
};

/// While tracking, the full search still runs at least this often (seconds of audio): a
/// local search cannot see an arrival that appears far from the followed ones.
const FULL_INTERVAL_S: f64 = 2.0;

/// Buffers the ref and meas streams and runs the finder once per observation window.
///
/// Windows are back to back, so consecutive results share no samples (what [`super::Tracker`]
/// requires). A window waits until the ref covers every lag of the search range,
/// `[b − Dmax, b + Lm − Dmin)`. A block that does not continue its stream is a
/// discontinuity: that stream's buffer restarts at the new block and any window it breaks
/// is dropped.
///
/// [`DelayStream::poll_tracking`] searches only near the arrivals of the last accepted
/// result while they hold the tracked delay, and falls back to the full search whenever
/// the local result could differ from it (see there).
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
    /// First and strongest arrival (integer lags) of the last accepted result, and the
    /// period its full search established; `None` until a full search accepts.
    follow: Option<Follow>,
    /// Windows since the last full search.
    since_full: usize,
    /// Windows between forced full searches.
    full_every: usize,
    full_searches: u64,
}

#[derive(Debug, Clone, Copy)]
struct Follow {
    first: i64,
    strongest: i64,
    period: Option<u64>,
}

impl DelayStream {
    /// Plans FFTs and grids for `cfg` up front.
    pub fn new(cfg: FinderConfig) -> Result<Self, ConfigError> {
        let scratch = FinderScratch::for_config(&cfg)?;
        let lm = cfg.observation_len();
        if lm == 0 {
            return Err(ConfigError::Observation(0.0));
        }
        let full_every = ((FULL_INTERVAL_S * cfg.fs / lm as f64).round() as usize).max(1);
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
            follow: None,
            since_full: 0,
            full_every,
            full_searches: 0,
        })
    }

    /// Full searches run so far (each poll runs one unless a local search was kept).
    pub fn full_searches(&self) -> u64 {
        self.full_searches
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
        self.follow = None;
    }

    /// Buffer bound: a window plus every lag it may need, plus one window of lead.
    fn cap(&self) -> usize {
        2 * self.lm
            + (self.cfg.search.min.unsigned_abs() + self.cfg.search.max.unsigned_abs()) as usize
    }

    pub fn push_ref(&mut self, block: Block<'_>) {
        let cap = self.cap();
        append(&mut self.ref_buf, &mut self.ref_start, block, cap);
    }

    pub fn push_meas(&mut self, block: Block<'_>) {
        let cap = self.cap();
        append(&mut self.meas_buf, &mut self.meas_start, block, cap);
    }

    /// Run the finder on the next complete window, if one is ready.
    pub fn poll(&mut self) -> Result<Option<FinderResult>, ConfigError> {
        self.next_window(None)
    }

    /// [`DelayStream::poll`] for a tracker holding `held`. While the last accepted result's
    /// first arrival is `held`, the window is searched only near its first and strongest
    /// arrivals, and that local result is returned only when it is accepted with the same
    /// first arrival and both arrivals clear of the local edges. Any other local outcome
    /// is replaced by the full search on the same window, so every refusal, ambiguity and
    /// delay change comes from the full search; the full search also runs every
    /// [`FULL_INTERVAL_S`] and whenever the local span would not be much smaller.
    pub fn poll_tracking(
        &mut self,
        held: Option<i64>,
    ) -> Result<Option<FinderResult>, ConfigError> {
        self.next_window(held)
    }

    /// Lags a local search around `f` covers, on the full search's acquisition tile grid
    /// so its tiles are the full search's; `None` when that saves little.
    fn local_range(&self, f: &Follow) -> Option<SearchRange> {
        let full = self.cfg.search;
        let n1 = self.cfg.band.class().segment(self.cfg.fs) as i64;
        let step = n1 / 4;
        // one acquisition segment either side holds the refinement window (±N₁/4), the
        // deblending reach and enough noise-only lags for the floor median
        let lo = f.first.min(f.strongest) - n1;
        let hi = f.first.max(f.strongest) + n1;
        let min = full.min + (lo - full.min).max(0) / step * step;
        let max = hi.min(full.max);
        let r = SearchRange { min, max };
        (!r.is_empty() && 2 * r.len() <= full.len()).then_some(r)
    }

    /// A local result may stand for the full search's (see [`DelayStream::poll_tracking`]).
    fn keep_local(&self, res: &FinderResult, r: SearchRange, held: i64) -> bool {
        let Outcome::Accepted { first, strongest } = &res.outcome else {
            return false;
        };
        let full = self.cfg.search;
        let margin = (self.cfg.band.class().segment(self.cfg.fs) / 2) as i64;
        let clear = |d: i64| {
            (d - r.min >= margin || r.min == full.min) && (r.max - d >= margin || r.max == full.max)
        };
        first.delay == held && clear(first.delay) && clear(strongest.delay)
    }

    fn next_window(&mut self, held: Option<i64>) -> Result<Option<FinderResult>, ConfigError> {
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
        let rb = Block {
            start: from.max(0) as u64,
            samples: &self.ref_out,
        };
        let mb = Block {
            start: self.meas_start,
            samples: &self.meas_out,
        };
        let local = match (held, self.follow) {
            (Some(h), Some(f)) if f.first == h && self.since_full + 1 < self.full_every => {
                self.local_range(&f).map(|r| (h, f, r))
            }
            _ => None,
        };
        let mut kept = None;
        if let Some((h, f, r)) = local {
            let l = Local {
                search: r,
                period: f.period,
            };
            let res = find_in(&mut self.scratch, rb, mb, &self.cfg, Some(l))?;
            if self.keep_local(&res, r, h) {
                kept = Some(res);
            }
        }
        let res = match kept {
            Some(res) => {
                self.since_full += 1;
                if let Outcome::Accepted { first, strongest } = &res.outcome {
                    self.follow = self.follow.map(|f| Follow {
                        first: first.delay,
                        strongest: strongest.delay,
                        ..f
                    });
                }
                res
            }
            None => {
                let res = find_in(&mut self.scratch, rb, mb, &self.cfg, None)?;
                self.full_searches += 1;
                self.since_full = 0;
                self.follow = match &res.outcome {
                    Outcome::Accepted { first, strongest } => Some(Follow {
                        first: first.delay,
                        strongest: strongest.delay,
                        period: res.confidence.period,
                    }),
                    _ => None,
                };
                res
            }
        };
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

/// Appends `block` to `buf`, whose first sample is at `*start`, keeping the newest `cap`
/// samples. A block that does not continue the buffer restarts it. Room is made before
/// appending, so the deque never grows past `cap` (past its capacity it would double it).
fn append(buf: &mut VecDeque<f32>, start: &mut u64, block: Block<'_>, cap: usize) {
    if buf.is_empty() || block.start != *start + buf.len() as u64 {
        buf.clear();
        *start = block.start;
    }
    let skip = block.samples.len().saturating_sub(cap);
    let keep = &block.samples[skip..];
    let drop = (buf.len() + keep.len()).saturating_sub(cap);
    buf.drain(..drop);
    *start += drop as u64;
    if buf.is_empty() {
        *start = block.start + skip as u64;
    }
    buf.reserve_exact(cap - buf.len());
    buf.extend(keep);
}
