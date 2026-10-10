//! Leq and band Leq bars ease between frames. The Leq streams arrive once a second, so a
//! bar drawn as received jumps once a second; easing the bar's length over a fraction of
//! that second reads as one level moving. Only the length eases: the number, the state
//! and its colours, the limit and headroom marks are the frame's from the moment it
//! arrives, and the bar has reached the frame's level long before the next one is due.

/// How long a bar takes to reach a new frame's level, s: a third of the once-a-second
/// frame period, so the bar rests at every reading for most of the second.
pub const EASE_S: f64 = 0.3;

/// Whether bars that started easing `since_s` ago still move.
pub fn easing(since_s: f64) -> bool {
    (0.0..EASE_S).contains(&since_s)
}

/// The bar levels `since_s` after a frame with levels `to` replaced one with `from`, dB.
/// A bar with no level on either side is drawn as received: a level appearing or
/// vanishing (a reset, a window filling from nothing) is a discontinuity and shows as one.
/// Different sets of bars (another window layout) are not one level moving: as received.
pub fn eased(from: &[f64], to: &[f64], since_s: f64) -> Vec<f64> {
    if from.len() != to.len() || !easing(since_s) {
        return to.to_vec();
    }
    // Ease-out cubic: it leaves fast, so the bar answers the frame at once, and settles
    // without overshooting into a level that was never measured.
    let k = 1.0 - (1.0 - since_s / EASE_S).powi(3);
    from.iter()
        .zip(to)
        .map(|(&a, &b)| {
            if a.is_finite() && b.is_finite() {
                a + (b - a) * k
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
    fn a_bar_moves_from_the_old_level_to_the_new_without_overshoot() {
        let from = [60.0, 70.0];
        let to = [66.0, 64.0];
        assert_eq!(eased(&from, &to, 0.0), vec![60.0, 70.0]);
        let mut last = from.to_vec();
        for i in 1..=30 {
            let now = eased(&from, &to, f64::from(i) * 0.01);
            assert!(now[0] >= last[0] && now[0] <= 66.0, "{now:?}");
            assert!(now[1] <= last[1] && now[1] >= 64.0, "{now:?}");
            last = now;
        }
        assert_eq!(eased(&from, &to, EASE_S), vec![66.0, 64.0]);
        assert_eq!(eased(&from, &to, 5.0), vec![66.0, 64.0]);
    }

    #[test]
    fn a_level_appearing_or_vanishing_is_drawn_as_received() {
        let e = eased(&[f64::NAN, 70.0], &[66.0, f64::NAN], 0.1);
        assert_eq!(e[0], 66.0);
        assert!(e[1].is_nan());
    }

    #[test]
    fn another_set_of_bars_is_drawn_as_received() {
        assert_eq!(eased(&[60.0], &[66.0, 64.0], 0.1), vec![66.0, 64.0]);
    }

    #[test]
    fn half_way_in_time_the_bar_is_most_of_the_way() {
        let e = eased(&[60.0], &[68.0], EASE_S / 2.0)[0];
        assert!((e - 67.0).abs() < 1e-9, "{e}");
    }
}
