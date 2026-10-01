//! Delay finder and tracking for a transfer job (design Q1), on the raw, unaligned
//! reference / measurement pair: the finder's result is the absolute delay, never relative
//! to the delay the MTW currently applies.

use std::collections::VecDeque;

use ac2_core::delay::{
    Agreement, Band, Block as DBlock, DelayStream, FinderConfig, FinderResult, Tracker, find_auto,
};

/// Seconds of raw audio kept for `delay.find`: the longest default observation (sub, 4 s)
/// plus the ±1 s search span on both sides, with margin.
const HISTORY_S: f64 = 7.0;
/// Longest observation `delay.find` uses (the sub band's default).
const MAX_OBSERVATION_S: f64 = 4.0;

pub(crate) struct Finder {
    fs: f64,
    cap: usize,
    reference: VecDeque<f32>,
    measurement: VecDeque<f32>,
    start: u64,
    tracking: Option<(DelayStream, Tracker)>,
}

impl Finder {
    pub(crate) fn new(fs: f64) -> Self {
        Self {
            fs,
            cap: (HISTORY_S * fs) as usize,
            reference: VecDeque::new(),
            measurement: VecDeque::new(),
            start: 0,
            tracking: None,
        }
    }

    /// A stream gap: nothing before it can be spliced onto what follows.
    pub(crate) fn restart(&mut self) {
        self.reference.clear();
        self.measurement.clear();
        if let Some((s, t)) = &mut self.tracking {
            s.reset();
            t.reset();
        }
    }

    /// Appends a block; returns a new tracked delay when tracking moves it.
    pub(crate) fn push(&mut self, start: u64, r: &[f32], m: &[f32]) -> Option<i64> {
        if self.reference.is_empty() || start != self.start + self.reference.len() as u64 {
            self.reference.clear();
            self.measurement.clear();
            self.start = start;
        }
        self.reference.extend(r);
        self.measurement.extend(m);
        let excess = self.reference.len().saturating_sub(self.cap);
        if excess > 0 {
            self.reference.drain(..excess);
            self.measurement.drain(..excess);
            self.start += excess as u64;
        }
        let (stream, tracker) = self.tracking.as_mut()?;
        stream.push_ref(DBlock { start, samples: r });
        stream.push_meas(DBlock { start, samples: m });
        let mut moved = None;
        loop {
            match stream.poll() {
                Ok(Some(res)) => {
                    if let Some(d) = tracker.observe(&res) {
                        moved = Some(d);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("delay tracking: {e}");
                    break;
                }
            }
        }
        moved
    }

    /// Turns tracking on in `band` (from the held delay) or off.
    pub(crate) fn track(&mut self, enabled: bool, band: Band, held: i64) {
        self.tracking = if enabled {
            match DelayStream::new(FinderConfig::new(self.fs, band)) {
                Ok(s) => {
                    let mut t = Tracker::new(Agreement::for_band(band, self.fs));
                    t.set_held(Some(held));
                    Some((s, t))
                }
                Err(e) => {
                    tracing::warn!("delay tracking not started: {e}");
                    None
                }
            }
        } else {
            None
        };
    }

    /// The operator set the delay.
    pub(crate) fn set_held(&mut self, held: i64) {
        if let Some((_, t)) = &mut self.tracking {
            t.set_held(Some(held));
        }
    }

    /// Runs the finder (auto band) on the newest observation that has reference coverage for
    /// every lag of the default ±1 s search.
    pub(crate) fn find(&self) -> Result<FinderResult, String> {
        let cfg = FinderConfig::new(self.fs, Band::FullRange);
        let lead = cfg.search.max.max(0) as usize;
        let tail = cfg.search.min.min(0).unsigned_abs() as usize;
        let total = self.measurement.len();
        let room = total.saturating_sub(lead + tail);
        if room < cfg.min_observation_len() {
            return Err(format!(
                "not enough audio yet: {:.2} s captured, {:.2} s needed",
                total as f64 / self.fs,
                (lead + tail + cfg.min_observation_len()) as f64 / self.fs
            ));
        }
        let lm = room.min((MAX_OBSERVATION_S * self.fs) as usize);
        let m0 = total - tail - lm;
        let r0 = m0 - lead;
        let r: Vec<f32> = self.reference.range(r0..).copied().collect();
        let m: Vec<f32> = self.measurement.range(m0..m0 + lm).copied().collect();
        find_auto(
            DBlock {
                start: self.start + r0 as u64,
                samples: &r,
            },
            DBlock {
                start: self.start + m0 as u64,
                samples: &m,
            },
            &cfg,
        )
        .map_err(|e| e.to_string())
    }
}
