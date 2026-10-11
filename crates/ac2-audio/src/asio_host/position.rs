//! The shared sample index from the driver's sample position.
//!
//! One `bufferSwitch` serves capture and playback, so both directions of a switch take the
//! same index: the driver's sample position minus the position of the first indexed switch.
//! The position is the driver's count of frames since `start()`; it advances by exactly
//! one buffer per switch unless the driver dropped periods, and then the jump is the exact
//! number of frames lost, which is what `DISCONTINUITY` must report. Counting switches would
//! miss periods the driver drops internally, so the count is only the fallback when the
//! position cannot be trusted.

use crate::block::BlockFlags;
use crate::clock::TimestampClock;

/// Index and flags per switch from the driver's sample position.
#[derive(Debug)]
pub(crate) struct PositionClock {
    state: State,
    rate: u32,
}

#[derive(Debug)]
enum State {
    /// No switch indexed yet.
    Fresh,
    /// The position is trusted. `expected` is the position the next switch should carry.
    Exact { expected: i64, last: i64, next: u64 },
    /// The position was missing or stuck: a running count, gaps estimated from the
    /// driver's system time when it gives one.
    Estimated {
        next: u64,
        timestamps: Option<(TimestampClock, u64)>,
    },
}

impl PositionClock {
    /// A clock for a stream at `rate` that has seen no switch.
    pub(crate) fn new(rate: u32) -> Self {
        Self {
            state: State::Fresh,
            rate,
        }
    }

    /// True once the position could not be trusted and the index is a running count.
    pub(crate) fn estimated(&self) -> bool {
        matches!(self.state, State::Estimated { .. })
    }

    /// Index and flags for a switch of `frames` frames at driver position `pos` (`None`
    /// when the driver gave none or an invalid one) and driver system time `system_ns`.
    pub(crate) fn advance(
        &mut self,
        pos: Option<i64>,
        frames: u32,
        system_ns: Option<u64>,
    ) -> (u64, BlockFlags) {
        let step = i64::from(frames);
        match (&mut self.state, pos) {
            (State::Fresh, Some(p)) => {
                self.state = State::Exact {
                    expected: p + step,
                    last: p,
                    next: u64::from(frames),
                };
                (0, BlockFlags::FIRST)
            }
            (State::Fresh, None) => {
                self.state = State::Estimated {
                    next: 0,
                    timestamps: None,
                };
                let (start, _) = self.estimate(frames, system_ns);
                (start, BlockFlags::FIRST | BlockFlags::GAP_ESTIMATED)
            }
            (
                State::Exact {
                    expected,
                    last,
                    next,
                },
                Some(p),
            ) if p != *last => {
                let ahead = p - *expected;
                let (start, flags) = if ahead == 0 {
                    (*next, BlockFlags::NONE)
                } else if ahead > 0 {
                    // Frames went by without a switch: the jump is the exact loss.
                    (*next + ahead as u64, BlockFlags::DISCONTINUITY)
                } else {
                    // Backwards, or forward by less than a buffer: the driver's count
                    // restarted. The index stays monotonic and the break is flagged.
                    (*next, BlockFlags::DISCONTINUITY)
                };
                *expected = p + step;
                *last = p;
                *next = start + u64::from(frames);
                (start, flags)
            }
            (State::Exact { next, .. }, _) => {
                // No position, or one that did not move between two different switches:
                // from here on the count is the only index.
                let next = *next;
                self.state = State::Estimated {
                    next,
                    timestamps: None,
                };
                let (start, flags) = self.estimate(frames, system_ns);
                (start, flags | BlockFlags::GAP_ESTIMATED)
            }
            (State::Estimated { .. }, _) => self.estimate(frames, system_ns),
        }
    }

    fn estimate(&mut self, frames: u32, system_ns: Option<u64>) -> (u64, BlockFlags) {
        let rate = self.rate;
        let State::Estimated { next, timestamps } = &mut self.state else {
            unreachable!("only called in the estimated state");
        };
        let (start, flags) = match (system_ns, timestamps.as_mut()) {
            (Some(ns), Some((clock, offset))) => {
                let (i, f) = clock.advance(frames, ns);
                (*offset + i, f)
            }
            (Some(ns), None) => {
                let mut clock = TimestampClock::new(rate);
                let (i, _) = clock.advance(frames, ns);
                let offset = *next;
                *timestamps = Some((clock, offset));
                (offset + i, BlockFlags::NONE)
            }
            (None, _) => {
                *timestamps = None;
                (*next, BlockFlags::NONE)
            }
        };
        *next = start + u64::from(frames);
        (start, flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_of_one_buffer_index_from_the_first_position() {
        let mut c = PositionClock::new(48_000);
        assert_eq!(c.advance(Some(256), 256, None), (0, BlockFlags::FIRST));
        assert_eq!(c.advance(Some(512), 256, None), (256, BlockFlags::NONE));
        assert_eq!(c.advance(Some(768), 256, None), (512, BlockFlags::NONE));
        assert!(!c.estimated());
    }

    #[test]
    fn a_jump_is_an_exact_discontinuity() {
        let mut c = PositionClock::new(48_000);
        c.advance(Some(1_000), 256, None);
        // Three buffers dropped inside the driver.
        let (s, f) = c.advance(Some(1_000 + 4 * 256), 256, None);
        assert_eq!((s, f), (4 * 256, BlockFlags::DISCONTINUITY));
        assert_eq!(
            c.advance(Some(1_000 + 5 * 256), 256, None),
            (5 * 256, BlockFlags::NONE)
        );
        // A partial jump (17 frames) is exact too.
        let (s, f) = c.advance(Some(1_000 + 6 * 256 + 17), 256, None);
        assert_eq!((s, f), (6 * 256 + 17, BlockFlags::DISCONTINUITY));
    }

    #[test]
    fn a_position_that_restarts_keeps_the_index_monotonic() {
        let mut c = PositionClock::new(48_000);
        c.advance(Some(10_000), 256, None);
        c.advance(Some(10_256), 256, None);
        assert_eq!(
            c.advance(Some(0), 256, None),
            (512, BlockFlags::DISCONTINUITY)
        );
        assert_eq!(c.advance(Some(256), 256, None), (768, BlockFlags::NONE));
    }

    #[test]
    fn a_stuck_or_missing_position_falls_back_to_the_count() {
        let mut c = PositionClock::new(48_000);
        c.advance(Some(0), 256, None);
        c.advance(Some(256), 256, None);
        // Stuck: the same position on the next switch.
        assert_eq!(
            c.advance(Some(256), 256, None),
            (512, BlockFlags::GAP_ESTIMATED)
        );
        assert!(c.estimated());
        // From here the count runs on, whatever the driver says.
        assert_eq!(c.advance(Some(99_999), 256, None), (768, BlockFlags::NONE));
        assert_eq!(c.advance(None, 256, None), (1024, BlockFlags::NONE));
    }

    #[test]
    fn without_any_position_the_system_time_estimates_gaps() {
        let mut c = PositionClock::new(48_000);
        let period = 256.0 / 48_000.0 * 1e9;
        let mut t = 5e9f64;
        assert_eq!(
            c.advance(None, 256, Some(t as u64)),
            (0, BlockFlags::FIRST | BlockFlags::GAP_ESTIMATED)
        );
        t += period;
        assert_eq!(
            c.advance(None, 256, Some(t as u64)),
            (256, BlockFlags::NONE)
        );
        t += 3.0 * period;
        let (s, f) = c.advance(None, 256, Some(t as u64));
        assert_eq!(s, 256 * 4);
        assert_eq!(f, BlockFlags::DISCONTINUITY | BlockFlags::GAP_ESTIMATED);
    }
}
