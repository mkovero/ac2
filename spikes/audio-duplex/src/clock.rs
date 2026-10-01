//! Assigning absolute sample indices to callback blocks.
//!
//! Two sources exist. JACK hands every cycle an exact 32-bit frame counter, so gaps are
//! exact. cpal hands only buffers plus host timestamps, so the index is a running sum of
//! delivered frames and a gap can only be *estimated* from a timestamp jump; such blocks are
//! flagged `DISCONTINUITY | GAP_ESTIMATED` so consumers reset rather than trusting the size.

use crate::block::BlockFlags;

/// Sample index from a backend's exact wrapping frame counter (JACK `last_frame_time`).
#[derive(Debug, Clone, Default)]
pub struct FrameCounterClock {
    state: Option<FrameCounterState>,
}

#[derive(Debug, Clone, Copy)]
struct FrameCounterState {
    base: u64,
    last_raw: u32,
    last_ext: u64,
    expected_next_raw: u32,
}

impl FrameCounterClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `(start_sample, flags)` for a cycle that starts at `raw` and has `frames`.
    #[inline]
    pub fn advance(&mut self, raw: u32, frames: u32) -> (u64, BlockFlags) {
        match self.state.as_mut() {
            None => {
                self.state = Some(FrameCounterState {
                    base: 0,
                    last_raw: raw,
                    last_ext: 0,
                    expected_next_raw: raw.wrapping_add(frames),
                });
                (0, BlockFlags::FIRST)
            }
            Some(s) => {
                // Wrapping difference handles the 2^32 rollover (~24.8 h at 48 kHz).
                let ext = s.last_ext + u64::from(raw.wrapping_sub(s.last_raw));
                let flags = if raw == s.expected_next_raw {
                    BlockFlags::NONE
                } else {
                    BlockFlags::DISCONTINUITY
                };
                s.last_raw = raw;
                s.last_ext = ext;
                s.expected_next_raw = raw.wrapping_add(frames);
                (ext - s.base, flags)
            }
        }
    }
}

/// Sample index from a running frame count, with gap estimation from host timestamps.
#[derive(Debug, Clone)]
pub struct TimestampClock {
    sample_rate: f64,
    next_start: u64,
    last: Option<(u64, u32)>,
    /// A timestamp jump larger than expected by more than this fraction of the previous
    /// block's duration counts as a gap. Callback timestamps jitter by scheduling noise;
    /// capture timestamps are device-derived and much tighter, but some hosts synthesise
    /// them from the callback time.
    tolerance_blocks: f64,
}

impl TimestampClock {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: f64::from(sample_rate),
            next_start: 0,
            last: None,
            tolerance_blocks: 0.5,
        }
    }

    /// `ts_ns` is the capture timestamp if the host provides one (else callback time).
    #[inline]
    pub fn advance(&mut self, frames: u32, ts_ns: u64) -> (u64, BlockFlags) {
        let mut flags = BlockFlags::NONE;
        match self.last {
            None => flags |= BlockFlags::FIRST,
            Some((prev_ns, prev_frames)) => {
                let expected = f64::from(prev_frames) / self.sample_rate * 1e9;
                let actual = ts_ns.saturating_sub(prev_ns) as f64;
                let excess = actual - expected;
                if excess > expected * self.tolerance_blocks {
                    let missing = (excess * self.sample_rate / 1e9).round() as u64;
                    self.next_start += missing;
                    flags |= BlockFlags::DISCONTINUITY | BlockFlags::GAP_ESTIMATED;
                }
            }
        }
        let start = self.next_start;
        self.next_start += u64::from(frames);
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
        let (s0, f0) = c.advance(raw, 256);
        assert_eq!((s0, f0), (0, BlockFlags::FIRST));
        for i in 1..5u64 {
            raw = raw.wrapping_add(256);
            let (s, f) = c.advance(raw, 256);
            assert_eq!(s, i * 256);
            assert!(f.is_empty());
        }
    }

    #[test]
    fn frame_counter_reports_exact_gap() {
        let mut c = FrameCounterClock::new();
        c.advance(1000, 256);
        let (s, f) = c.advance(1000 + 256 * 3, 256);
        assert_eq!(s, 768);
        assert!(f.contains(BlockFlags::DISCONTINUITY));
        assert!(!f.contains(BlockFlags::GAP_ESTIMATED));
        let (s, f) = c.advance(1000 + 256 * 4, 256);
        assert_eq!(s, 1024);
        assert!(f.is_empty());
    }

    #[test]
    fn timestamp_clock_tolerates_jitter_and_estimates_gaps() {
        let mut c = TimestampClock::new(48_000);
        let block_ns = 256.0 / 48_000.0 * 1e9;
        let mut t = 1_000_000_000.0f64;
        let (s, f) = c.advance(256, t as u64);
        assert_eq!((s, f), (0, BlockFlags::FIRST));
        // 30 % jitter: no gap.
        t += block_ns * 1.3;
        let (s, f) = c.advance(256, t as u64);
        assert_eq!(s, 256);
        assert!(f.is_empty());
        t += block_ns * 0.7;
        c.advance(256, t as u64);
        // Two blocks missing.
        t += block_ns * 3.0;
        let (s, f) = c.advance(256, t as u64);
        assert_eq!(s, 768 + 512);
        assert!(f.contains(BlockFlags::DISCONTINUITY | BlockFlags::GAP_ESTIMATED));
    }
}
