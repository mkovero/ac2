//! The sample index of an IOProc, from the HAL's sample times.
//!
//! One IOProc call serves the input and the output of one device, and the HAL stamps both
//! sides on the device's one sample-time line (`mSampleTime`). The index of a cycle is its
//! input sample time minus the first cycle's, so input and output of a cycle share it and a
//! jump in the sample time is the exact number of frames the device lost.

use crate::types::Gap;

/// Turns a cycle's sample time into an index that only moves forward.
#[derive(Clone, Debug, Default)]
pub struct Indexer {
    state: State,
    estimated: bool,
}

#[derive(Clone, Copy, Debug, Default)]
enum State {
    /// No cycle seen.
    #[default]
    Fresh,
    /// Indexed from the sample time: `index = sample_time − origin`. The origin keeps the
    /// first cycle's fraction, so a sample time that is not a whole number (a rate scalar
    /// off 1) rounds the same way every cycle and no frame is lost or counted twice.
    Exact { origin: f64, next: u64 },
    /// The sample time was not valid: the index counts frames.
    Counting { next: u64 },
}

impl Indexer {
    /// An indexer that has seen no cycle.
    pub fn new() -> Self {
        Self::default()
    }

    /// True once a sample time was invalid: gaps since then are not known exactly.
    pub fn estimated(&self) -> bool {
        self.estimated
    }

    /// Index and gap for a cycle of `frames` frames whose sample time is `sample_time`
    /// (`None` when the HAL did not flag it valid).
    pub fn advance(&mut self, sample_time: Option<f64>, frames: u32) -> (u64, Gap) {
        let step = u64::from(frames);
        let sample_time = sample_time.filter(|t| t.is_finite());
        let (index, gap) = match (self.state, sample_time) {
            (State::Fresh, Some(t)) => {
                self.state = State::Exact {
                    origin: t,
                    next: step,
                };
                return (0, Gap::First);
            }
            (State::Fresh, None) => {
                self.estimated = true;
                self.state = State::Counting { next: step };
                return (0, Gap::First);
            }
            (State::Exact { origin, next }, Some(t)) => {
                let ahead = (t - origin) - next as f64;
                if ahead.abs() < 0.5 {
                    (next, Gap::None)
                } else if ahead > 0.0 {
                    let n = ahead.round() as u64;
                    (next + n, Gap::Forward(n))
                } else {
                    // The device restarted its IO: count on from here on the new line.
                    self.state = State::Exact {
                        origin: t - next as f64,
                        next,
                    };
                    (next, Gap::Backward)
                }
            }
            (State::Exact { next, .. }, None) => {
                self.estimated = true;
                self.state = State::Counting { next };
                (next, Gap::Forward(0))
            }
            (State::Counting { next }, None) => (next, Gap::None),
            (State::Counting { next }, Some(t)) => {
                // Valid again, but how many frames passed meanwhile is not known.
                self.state = State::Exact {
                    origin: t - next as f64,
                    next,
                };
                (next, Gap::Forward(0))
            }
        };
        match &mut self.state {
            State::Exact { next, .. } | State::Counting { next } => *next = index + step,
            State::Fresh => {}
        }
        (index, gap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(times: &[Option<f64>], frames: u32) -> Vec<(u64, Gap)> {
        let mut ix = Indexer::new();
        times.iter().map(|&t| ix.advance(t, frames)).collect()
    }

    #[test]
    fn steady_cycles_count_frames_from_zero() {
        let t: Vec<_> = (0..5)
            .map(|i| Some(1_000_000.0 + 512.0 * f64::from(i)))
            .collect();
        let r = run(&t, 512);
        assert_eq!(r[0], (0, Gap::First));
        for (i, &(ix, gap)) in r.iter().enumerate().skip(1) {
            assert_eq!((ix, gap), (512 * i as u64, Gap::None));
        }
    }

    #[test]
    fn a_jump_is_the_exact_number_of_frames_lost() {
        let r = run(
            &[Some(0.0), Some(512.0), Some(1024.0 + 1536.0), Some(3072.0)],
            512,
        );
        assert_eq!(r[2], (2560, Gap::Forward(1536)));
        assert_eq!(r[3], (3072, Gap::None));
    }

    #[test]
    fn backwards_keeps_the_index_monotonic() {
        let r = run(&[Some(5000.0), Some(5512.0), Some(100.0), Some(612.0)], 512);
        assert_eq!(r[2], (1024, Gap::Backward));
        assert_eq!(r[3], (1536, Gap::None));
    }

    #[test]
    fn an_invalid_time_is_flagged_once_and_then_counted() {
        let mut ix = Indexer::new();
        assert_eq!(ix.advance(Some(0.0), 256), (0, Gap::First));
        assert!(!ix.estimated());
        assert_eq!(ix.advance(None, 256), (256, Gap::Forward(0)));
        assert!(ix.estimated());
        assert_eq!(ix.advance(None, 256), (512, Gap::None));
        // Valid again somewhere else: flagged, then exact on the new line.
        assert_eq!(ix.advance(Some(90_000.0), 256), (768, Gap::Forward(0)));
        assert_eq!(ix.advance(Some(90_256.0), 256), (1024, Gap::None));
        assert!(ix.estimated());
    }

    #[test]
    fn fractional_times_neither_lose_nor_repeat_frames() {
        // A constant fraction (the origin's) and jitter below half a frame both read as
        // continuous.
        let jitter = [0.0, 0.3, -0.2, 0.45, -0.45, 0.1];
        let t: Vec<_> = (0..6)
            .map(|i| Some(17.37 + 480.0 * f64::from(i as u32) + jitter[i]))
            .collect();
        for (i, (ix, gap)) in run(&t, 480).into_iter().enumerate().skip(1) {
            assert_eq!((ix, gap), (480 * i as u64, Gap::None), "cycle {i}");
        }
        // A fractional jump rounds to whole frames.
        let r = run(&[Some(0.4), Some(480.4), Some(960.4 + 99.7)], 480);
        assert_eq!(r[2], (1060, Gap::Forward(100)));
    }

    #[test]
    fn a_first_invalid_time_counts_from_zero() {
        let r = run(&[None, None, Some(7.0)], 64);
        assert_eq!(
            r,
            vec![(0, Gap::First), (64, Gap::None), (128, Gap::Forward(0))]
        );
    }
}
