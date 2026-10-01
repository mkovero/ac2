//! Assigning absolute sample indices to callback blocks.
//!
//! Two sources exist:
//!
//! - [`FrameCounterClock`]: the backend hands every cycle an exact wrapping frame counter
//!   (JACK `last_frame_time`, the fake device). Gaps are exact.
//! - [`TimestampClock`]: the backend hands only buffers and host timestamps (cpal). The
//!   index is a running count of delivered frames; a gap can only be estimated from a
//!   timestamp jump, so it is flagged `DISCONTINUITY | GAP_ESTIMATED`. Consumers reset on
//!   the flag rather than trusting the size.

use crate::block::BlockFlags;

/// Index from an exact 32-bit wrapping frame counter.
///
/// The counter is extended to 64 bits across its wrap (about 24.8 h at 48 kHz). The
/// returned index starts at 0 for the first block.
#[derive(Debug, Clone, Default)]
pub struct FrameCounterClock {
    state: Option<CounterState>,
}

#[derive(Debug, Clone, Copy)]
struct CounterState {
    expected_raw: u32,
    next_index: u64,
}

impl FrameCounterClock {
    /// A clock that has not seen a block yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Index and flags for a cycle starting at counter value `raw` with `frames` frames.
    #[inline]
    pub fn advance(&mut self, raw: u32, frames: u32) -> (u64, BlockFlags) {
        let Some(s) = self.state.as_mut() else {
            self.state = Some(CounterState {
                expected_raw: raw.wrapping_add(frames),
                next_index: u64::from(frames),
            });
            return (0, BlockFlags::FIRST);
        };
        let ahead = raw.wrapping_sub(s.expected_raw);
        let (start, flags) = if ahead == 0 {
            (s.next_index, BlockFlags::NONE)
        } else if ahead < 1 << 31 {
            // Forward jump: frames went by without a cycle for us. The size is exact.
            (s.next_index + u64::from(ahead), BlockFlags::DISCONTINUITY)
        } else {
            // The counter went backwards (server restart or a counter reset). The index
            // must stay monotonic, so continue where we were and flag the break.
            (s.next_index, BlockFlags::DISCONTINUITY)
        };
        s.expected_raw = raw.wrapping_add(frames);
        s.next_index = start + u64::from(frames);
        (start, flags)
    }
}

/// Index from a running frame count, with gaps estimated from host timestamps.
#[derive(Debug, Clone)]
pub struct TimestampClock {
    ns_per_frame: f64,
    next_index: u64,
    last: Option<(u64, u32)>,
    tolerance_blocks: f64,
}

impl TimestampClock {
    /// A gap is only declared when a timestamp lands more than this fraction of the previous
    /// block's duration later than predicted. Callback timestamps carry scheduling jitter;
    /// device-derived capture timestamps are much tighter, but some hosts synthesise them
    /// from the callback time, so the threshold has to tolerate that jitter.
    pub const DEFAULT_TOLERANCE_BLOCKS: f64 = 0.5;

    /// A clock at `sample_rate` that has not seen a block yet.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            ns_per_frame: 1e9 / f64::from(sample_rate.max(1)),
            next_index: 0,
            last: None,
            tolerance_blocks: Self::DEFAULT_TOLERANCE_BLOCKS,
        }
    }

    /// Index and flags for a block of `frames` frames whose first frame has timestamp
    /// `ts_ns` (capture or playback time if the host gives one, else callback time).
    #[inline]
    pub fn advance(&mut self, frames: u32, ts_ns: u64) -> (u64, BlockFlags) {
        let mut flags = BlockFlags::NONE;
        match self.last {
            None => flags |= BlockFlags::FIRST,
            Some((prev_ns, prev_frames)) => {
                let expected = f64::from(prev_frames) * self.ns_per_frame;
                let actual = ts_ns.saturating_sub(prev_ns) as f64;
                let excess = actual - expected;
                if excess > expected * self.tolerance_blocks {
                    let missing = (excess / self.ns_per_frame).round() as u64;
                    self.next_index += missing;
                    flags |= BlockFlags::DISCONTINUITY | BlockFlags::GAP_ESTIMATED;
                }
            }
        }
        let start = self.next_index;
        self.next_index += u64::from(frames);
        self.last = Some((ts_ns, frames));
        (start, flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_counter_is_contiguous_across_u32_wrap() {
        let mut c = FrameCounterClock::new();
        let mut raw = u32::MAX - 300;
        assert_eq!(c.advance(raw, 256), (0, BlockFlags::FIRST));
        for i in 1..5u64 {
            raw = raw.wrapping_add(256);
            assert_eq!(c.advance(raw, 256), (i * 256, BlockFlags::NONE));
        }
    }

    #[test]
    fn frame_counter_reports_exact_gap_including_across_wrap() {
        let mut c = FrameCounterClock::new();
        let origin = u32::MAX - 600;
        c.advance(origin, 256);
        let (s, f) = c.advance(origin.wrapping_add(256 * 3), 256);
        assert_eq!((s, f), (768, BlockFlags::DISCONTINUITY));
        assert_eq!(
            c.advance(origin.wrapping_add(256 * 4), 256),
            (1024, BlockFlags::NONE)
        );
    }

    #[test]
    fn frame_counter_regression_keeps_index_monotonic() {
        let mut c = FrameCounterClock::new();
        c.advance(10_000, 256);
        c.advance(10_256, 256);
        let (s, f) = c.advance(5, 256);
        assert_eq!((s, f), (512, BlockFlags::DISCONTINUITY));
        assert_eq!(c.advance(261, 256), (768, BlockFlags::NONE));
    }

    #[test]
    fn timestamp_clock_tolerates_jitter_and_estimates_gaps() {
        let mut c = TimestampClock::new(48_000);
        let block_ns = 256.0 / 48_000.0 * 1e9;
        let mut t = 1e9f64;
        assert_eq!(c.advance(256, t as u64), (0, BlockFlags::FIRST));
        t += block_ns * 1.3;
        assert_eq!(c.advance(256, t as u64), (256, BlockFlags::NONE));
        t += block_ns * 0.7;
        assert_eq!(c.advance(256, t as u64), (512, BlockFlags::NONE));
        t += block_ns * 3.0;
        let (s, f) = c.advance(256, t as u64);
        assert_eq!(s, 768 + 512);
        assert_eq!(f, BlockFlags::DISCONTINUITY | BlockFlags::GAP_ESTIMATED);
    }
}
