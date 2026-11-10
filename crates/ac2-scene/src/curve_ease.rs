//! A live spectrum curve may ease from one frame to the next. A frame is drawn the pass it
//! arrives: the curve never trails its newest frame by a glide interval, which on a live
//! signal reads as the picture lagging the sound. Only content slower than the display
//! eases, and only over one display frame ([`EASE_S`]): a 65 536-point spectrum at 96 kHz
//! comes every 85 ms, and drawn as received its step lands within one frame; split over
//! two it reads as motion rather than a cut, at a cost of half a display frame on average.
//! Frames at the display's own rate are steps the eye cannot resolve: drawn as received.
//! Only the line moves; cursor readouts, peak hold and captures read the newest frame
//! itself.

use std::time::Duration;

/// How long a slow frame's step is eased over, s: one display frame at 60 frames a second.
pub const EASE_S: f64 = 1.0 / 60.0;
/// Frames that come at least this long after the one before are slower than the display
/// and ease ([`eases`]); faster ones are drawn as received.
pub const SLOW_FRAME_S: f64 = 1.0 / 30.0;

/// Whether a frame that came `interval` after the one before eases in ([`EASE_S`]).
pub fn eases(interval: Duration) -> bool {
    interval.as_secs_f64() >= SLOW_FRAME_S
}

/// How far a glide of `duration_s` has gone `since_s` after it started, 0–1; `None` once it
/// has arrived.
pub fn progress(since_s: f64, duration_s: f64) -> Option<f64> {
    (since_s < duration_s).then(|| (since_s / duration_s).max(0.0))
}

/// The levels drawn `k` (0–1, [`progress`]) of the way from `from` to `to`, dB per column.
/// Linear in time, so successive glides join into steady motion. A column without a level
/// on either side is drawn as received: a gap opening or closing is a discontinuity and
/// shows as one. Curves of different lengths (another grid) are not one curve moving: `to`.
pub fn eased(from: &[f32], to: &[f32], k: f64) -> Vec<f32> {
    if from.len() != to.len() {
        return to.to_vec();
    }
    let k = k.clamp(0.0, 1.0);
    from.iter()
        .zip(to)
        .map(|(&a, &b)| {
            if a.is_finite() && b.is_finite() {
                (f64::from(a) + (f64::from(b) - f64::from(a)) * k) as f32
            } else {
                b
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_frames_slower_than_the_display_ease() {
        assert!(!eases(Duration::from_micros(16_667)));
        assert!(!eases(Duration::from_millis(21)));
        assert!(eases(Duration::from_millis(34)));
        assert!(eases(Duration::from_millis(85)));
    }

    #[test]
    fn progress_runs_from_zero_until_arrival() {
        assert_eq!(progress(0.0, 0.1), Some(0.0));
        assert!(progress(0.05, 0.1).is_some_and(|k| (k - 0.5).abs() < 1e-12));
        assert_eq!(progress(0.1, 0.1), None);
        assert_eq!(progress(0.3, 0.1), None);
    }

    #[test]
    fn columns_move_linearly_from_the_old_level_to_the_new() {
        let from = [-60.0, -40.0];
        let to = [-50.0, -60.0];
        assert_eq!(eased(&from, &to, 0.0), vec![-60.0, -40.0]);
        assert_eq!(eased(&from, &to, 0.5), vec![-55.0, -50.0]);
        assert_eq!(eased(&from, &to, 1.0), vec![-50.0, -60.0]);
    }

    #[test]
    fn a_gap_opening_or_closing_is_drawn_as_received() {
        let e = eased(&[f32::NAN, -40.0], &[-50.0, f32::NAN], 0.5);
        assert_eq!(e[0], -50.0);
        assert!(e[1].is_nan());
    }

    #[test]
    fn another_grid_is_drawn_as_received() {
        assert_eq!(eased(&[-60.0], &[-50.0, -40.0], 0.5), vec![-50.0, -40.0]);
    }
}
