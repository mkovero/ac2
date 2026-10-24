//! Programme loudness (docs/design/programme-meter.md §6.1): ITU-R BS.1770-5 K-weighted
//! loudness with the EBU Tech 3341 time scales (momentary 400 ms, short-term 3 s, gated
//! integrated) and the Tech 3342 loudness range, for a source of one or two channels with
//! weight 1 each. Identical in definition to `tools/refgen/sets/loudness.py`, which tests it.
//!
//! The time base is a 10 ms sub-block counted in samples, independent of the audio block
//! size: K-weighted energy per sub-block in a ring of the last 300. Momentary and short-term
//! are re-summed from the ring at every sub-block (a running add/subtract sum leaves a
//! residue that can go negative on silence); evaluating every 10 ms rather than every
//! 100 ms is what lets a 400 ms tone read within 0.1 LU at any phase against the grid.
//! Integrated loudness and the range keep fixed 0.01 LU histograms, so memory does
//! not grow with the length of a show. True peak is not here: it is the meters'
//! [`crate::ppm::TruePeak`], reset on the audio-block boundary with the meters' max.

mod histogram;
mod kweighting;

pub use histogram::{BIN_LU, BINS, HISTOGRAM_FLOOR};
pub use kweighting::{Coefficients, KWeighting, TABLE_1, TABLE_2, TABLE_RATE};

use kweighting::ChannelFilter;

/// Sub-block length, seconds.
pub const SUB_BLOCK_S: f64 = 0.01;
/// Sub-blocks per sub-block ring and per short-term window (3 s).
pub const SHORT_TERM_SUB_BLOCKS: usize = 300;
/// Sub-blocks per momentary window and per gating block (400 ms).
pub const MOMENTARY_SUB_BLOCKS: usize = 40;
/// Sub-blocks between gating blocks and between range values (100 ms: 75 % overlap).
pub const HOP_SUB_BLOCKS: usize = 10;
/// Channels a source may have (a pair, or one input).
pub const MAX_CHANNELS: usize = 2;
/// Lowest and highest rate loudness runs at, Hz.
pub const MIN_RATE: f64 = 44100.0;
pub const MAX_RATE: f64 = 192000.0;

/// Mean squares below this (−150.7 LUFS) read −∞: below any programme, and above the
/// rounding residue of the K-filter on digital silence.
const SILENCE_MEAN_SQUARE: f64 = 1e-15;

/// BS.1770's offset: cancels the K-weighting gain at 997 Hz, so a 0 dBFS sine on one
/// channel reads −3.01 LUFS.
const OFFSET_LU: f64 = -0.691;

/// Loudness of a channel-weighted mean square, LUFS (−∞ at zero).
pub fn lufs(mean_square: f64) -> f64 {
    OFFSET_LU + 10.0 * mean_square.log10()
}

fn display_lufs(mean_square: f64) -> f64 {
    if mean_square < SILENCE_MEAN_SQUARE {
        f64::NEG_INFINITY
    } else {
        lufs(mean_square)
    }
}

/// Whether loudness runs at `sample_rate`: 44.1 to 192 kHz and a whole multiple of 100 Hz,
/// so a 10 ms sub-block is a whole number of samples and the K-weighting follows the
/// 48 kHz curve to 19.2 kHz.
pub fn supports_rate(sample_rate: f64) -> bool {
    (MIN_RATE..=MAX_RATE).contains(&sample_rate)
        && sample_rate.fract() == 0.0
        && (sample_rate as u64).is_multiple_of(100)
}

/// Why a meter cannot be built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Unsupported {
    /// Outside 44.1–192 kHz or not a multiple of 100 Hz.
    Rate(f64),
    /// Not one or two channels.
    Channels(usize),
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unsupported::Rate(fs) => write!(
                f,
                "loudness needs a rate of 44.1 to 192 kHz, a multiple of 100 Hz (got {fs} Hz)"
            ),
            Unsupported::Channels(n) => write!(f, "loudness needs one or two channels, got {n}"),
        }
    }
}

impl std::error::Error for Unsupported {}

/// A source's loudness at the last completed sub-block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessReading {
    /// LUFS over the last 400 ms; −∞ on silence or until a window lies wholly after the
    /// start or the last discontinuity.
    pub momentary: f64,
    /// LUFS over the last 3 s; −∞ likewise.
    pub short_term: f64,
    /// Gated integrated loudness since the reset, LUFS; `None` until a block passes.
    pub integrated: Option<f64>,
    /// Loudness range since the reset, LU; `None` until a value passes.
    pub range: Option<f64>,
    /// Highest momentary over windows wholly after the reset; −∞ until the first.
    pub max_momentary: f64,
    /// Highest short-term over windows wholly after the reset; −∞ until the first.
    pub max_short_term: f64,
    /// Samples metered since the reset (a discontinuity's gap excluded).
    pub duration_samples: u64,
    /// Gating blocks completed since the reset (every one, gated or not).
    pub gating_blocks: u64,
}

/// One source's loudness meter. Allocates at construction only; `process` is real-time
/// safe.
#[derive(Debug, Clone)]
pub struct LoudnessMeter {
    sample_rate: f64,
    k: KWeighting,
    channels: usize,
    filters: [ChannelFilter; MAX_CHANNELS],
    sub_block: usize,
    /// Samples into the current sub-block and their weighted energy.
    pos: usize,
    acc: f64,
    ring: [f64; SHORT_TERM_SUB_BLOCKS],
    /// Next ring slot.
    head: usize,
    /// Sub-blocks in the ring since the start or the last discontinuity (saturating).
    filled: usize,
    /// Sub-blocks since the later of the reset and the last discontinuity: the gating and
    /// range grid, and the extent windows must lie within to count towards the maxima.
    grid: u64,
    momentary: f64,
    short_term: f64,
    max_momentary: f64,
    max_short_term: f64,
    integrated_gate: histogram::IntegratedGate,
    range_gate: histogram::RangeGate,
    integrated: Option<f64>,
    range: Option<f64>,
    duration: u64,
    gating_blocks: u64,
    reset_pending: bool,
}

impl LoudnessMeter {
    /// A meter of `channels` (1 or 2, weight 1 each) at `sample_rate`. Its sub-block grid
    /// starts at the first sample processed, with zeroed filters: a new source follows the
    /// restart rule of a discontinuity.
    pub fn new(sample_rate: f64, channels: usize) -> Result<Self, Unsupported> {
        if !supports_rate(sample_rate) {
            return Err(Unsupported::Rate(sample_rate));
        }
        if !(1..=MAX_CHANNELS).contains(&channels) {
            return Err(Unsupported::Channels(channels));
        }
        Ok(Self {
            sample_rate,
            k: KWeighting::new(sample_rate),
            channels,
            filters: [ChannelFilter::default(); MAX_CHANNELS],
            sub_block: (sample_rate * SUB_BLOCK_S).round() as usize,
            pos: 0,
            acc: 0.0,
            ring: [0.0; SHORT_TERM_SUB_BLOCKS],
            head: 0,
            filled: 0,
            grid: 0,
            momentary: f64::NEG_INFINITY,
            short_term: f64::NEG_INFINITY,
            max_momentary: f64::NEG_INFINITY,
            max_short_term: f64::NEG_INFINITY,
            integrated_gate: histogram::IntegratedGate::new(),
            range_gate: histogram::RangeGate::new(),
            integrated: None,
            range: None,
            duration: 0,
            gating_blocks: 0,
            reset_pending: false,
        })
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Samples per 10 ms sub-block.
    pub fn sub_block_samples(&self) -> usize {
        self.sub_block
    }

    /// Meters a block: one slice per channel, read up to the shortest.
    pub fn process(&mut self, block: &[&[f32]]) {
        debug_assert_eq!(block.len(), self.channels, "one slice per channel");
        let n = block.iter().map(|c| c.len()).min().unwrap_or(0);
        if self.reset_pending && self.pos == 0 {
            self.apply_reset();
        }
        let mut t = 0;
        while t < n {
            let take = (n - t).min(self.sub_block - self.pos);
            for (filter, channel) in self.filters.iter_mut().zip(block) {
                for &x in &channel[t..t + take] {
                    let y = filter.process(&self.k, f64::from(x));
                    self.acc += y * y;
                }
            }
            self.pos += take;
            self.duration += take as u64;
            t += take;
            if self.pos == self.sub_block {
                self.complete_sub_block();
            }
        }
    }

    /// Requests a reset of integrated, range, the maxima and the duration. It takes effect
    /// at the first sub-block boundary at or after the first sample of the next
    /// [`process`](Self::process) call; the sub-block grid itself never re-phases.
    /// Momentary and short-term are signal state and carry on.
    pub fn reset(&mut self) {
        self.reset_pending = true;
    }

    /// Whether a requested reset has not yet reached its sub-block boundary.
    pub fn reset_pending(&self) -> bool {
        self.reset_pending
    }

    /// The capture lost samples before the next block: the K-filter state and the ring are
    /// cleared, the partial sub-block is dropped, and the sub-block, gating and range grids
    /// restart at the next sample processed. Integrated, range, the maxima and the duration
    /// keep what they had.
    pub fn discontinuity(&mut self) {
        for f in &mut self.filters {
            f.clear();
        }
        self.pos = 0;
        self.acc = 0.0;
        self.ring = [0.0; SHORT_TERM_SUB_BLOCKS];
        self.head = 0;
        self.filled = 0;
        self.grid = 0;
        self.momentary = f64::NEG_INFINITY;
        self.short_term = f64::NEG_INFINITY;
    }

    pub fn reading(&self) -> LoudnessReading {
        LoudnessReading {
            momentary: self.momentary,
            short_term: self.short_term,
            integrated: self.integrated,
            range: self.range,
            max_momentary: self.max_momentary,
            max_short_term: self.max_short_term,
            duration_samples: self.duration,
            gating_blocks: self.gating_blocks,
        }
    }

    fn apply_reset(&mut self) {
        self.reset_pending = false;
        self.integrated_gate.clear();
        self.range_gate.clear();
        self.integrated = None;
        self.range = None;
        self.max_momentary = f64::NEG_INFINITY;
        self.max_short_term = f64::NEG_INFINITY;
        self.duration = 0;
        self.gating_blocks = 0;
        self.grid = 0;
    }

    /// Mean square of the newest `n` sub-blocks.
    fn window(&self, n: usize) -> f64 {
        let len = SHORT_TERM_SUB_BLOCKS;
        let sum: f64 = (1..=n)
            .map(|i| self.ring[(self.head + len - i) % len])
            .sum();
        sum / (n * self.sub_block) as f64
    }

    fn complete_sub_block(&mut self) {
        self.ring[self.head] = self.acc;
        self.head = (self.head + 1) % SHORT_TERM_SUB_BLOCKS;
        self.acc = 0.0;
        self.pos = 0;
        self.filled = (self.filled + 1).min(SHORT_TERM_SUB_BLOCKS);
        self.grid += 1;
        for f in &mut self.filters[..self.channels] {
            f.flush_denormals();
        }

        let hop = HOP_SUB_BLOCKS as u64;
        if self.filled >= MOMENTARY_SUB_BLOCKS {
            let z = self.window(MOMENTARY_SUB_BLOCKS);
            self.momentary = display_lufs(z);
            let after = self.grid.checked_sub(MOMENTARY_SUB_BLOCKS as u64);
            if let Some(after) = after {
                self.max_momentary = self.max_momentary.max(self.momentary);
                if after % hop == 0 {
                    self.gating_blocks += 1;
                    self.integrated_gate.add(z);
                    self.integrated = self.integrated_gate.value();
                }
            }
        }
        if self.filled >= SHORT_TERM_SUB_BLOCKS {
            let z = self.window(SHORT_TERM_SUB_BLOCKS);
            self.short_term = display_lufs(z);
            let after = self.grid.checked_sub(SHORT_TERM_SUB_BLOCKS as u64);
            if let Some(after) = after {
                self.max_short_term = self.max_short_term.max(self.short_term);
                if after % hop == 0 {
                    self.range_gate.add(z);
                    self.range = self.range_gate.value();
                }
            }
        }
        if self.reset_pending {
            self.apply_reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A digital-silence tail is flushed to exact zero instead of decaying through
    /// subnormals, at the rate where the RLB poles are closest to the unit circle.
    #[test]
    fn silence_tail_flushes_the_filter_state() {
        let mut m = LoudnessMeter::new(192000.0, 2).expect("rate");
        let mut x = vec![0.0f32; 1920];
        x[0] = 1.0;
        m.process(&[&x, &x]);
        x[0] = 0.0;
        for _ in 0..1000 {
            m.process(&[&x, &x]);
        }
        let zero = m.filters.iter().all(|f| f.is_zero());
        assert!(zero, "filter state flushed");
        assert_eq!(m.reading().momentary, f64::NEG_INFINITY);
    }
}
