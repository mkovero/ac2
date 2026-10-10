//! A live spectrum curve glides from one frame to the next. Frames of a long window come
//! several times a second at most (a 65 536-point spectrum at 96 kHz every 85 ms); drawn as
//! received, the curve jumps at each and reads as a stutter. Moving each column from the
//! drawn level to the new frame's level over about the time until the next frame keeps the
//! curve in continuous motion without smearing it over time: it reaches every frame as the
//! next one arrives. Only the line moves; cursor readouts, peak hold and captures read the
//! newest frame itself.

/// Shortest glide, s: frames faster than this (a display frame or two apart) are hardly
/// steps at all.
pub const MIN_S: f64 = 0.03;
/// Longest glide, s: a frame later than this is not part of one motion (a hiccup, a
/// once-a-second refresh of a settled result); the curve moves to it in this time.
pub const MAX_S: f64 = 0.2;

/// How long the curve takes to reach a frame that came `interval_s` after the one before.
pub fn duration_s(interval_s: f64) -> f64 {
    if interval_s.is_finite() {
        interval_s.clamp(MIN_S, MAX_S)
    } else {
        MAX_S
    }
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
    fn the_glide_lasts_about_one_frame_interval() {
        assert_eq!(duration_s(0.0853), 0.0853);
        assert_eq!(duration_s(0.005), MIN_S);
        assert_eq!(duration_s(1.0), MAX_S);
        assert_eq!(duration_s(f64::NAN), MAX_S);
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
