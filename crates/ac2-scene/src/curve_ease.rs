//! A live spectrum curve glides from one frame to the next. Frames of a long window come
//! several times a second at most (a 65 536-point spectrum at 96 kHz every 85 ms); drawn as
//! received, the curve jumps at each and reads as a stutter. Moving each column from the
//! drawn level to the new frame's level over about the time until the next frame keeps the
//! curve in continuous motion without smearing it over time: it reaches every frame as the
//! next one arrives. Only the line moves; cursor readouts, peak hold and captures read the
//! newest frame itself.
//!
//! Frames reach the display later than they were captured by a delay that varies: the
//! daemon hands audio on in batches whose period beats against a long FFT's hop, the link
//! drains on its own clock. A glide as long as the time since the previous frame's arrival
//! then ends too early before a late frame (the curve stops, a dropped frame to the eye) and
//! too late before an early one (cut short, a jump). Paced by the frames' capture times
//! instead ([`Pacing`]), every glide ends when the latest a frame has recently been would
//! have it arrive, so the next frame is there before the curve stops; the cost is that much
//! display latency.

use std::collections::VecDeque;
use std::time::Instant;

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

/// Frames over which the latest arrival is remembered: a hop beating against the hand-off
/// period comes back to its latest phase every few frames, and a few seconds of history
/// keep a clock drifting between hosts out of it.
pub const PACING_FRAMES: usize = 32;

/// The capture times and arrivals of a live curve's recent frames, which pace its glides.
#[derive(Clone, Debug, Default)]
pub struct Pacing {
    /// Arrival and capture wall clock (ns) of the first frame: lags are kept relative to
    /// it, small enough for an `f64`.
    anchor: Option<(Instant, u64)>,
    /// Capture wall clock of the newest frame, ns.
    last_ns: u64,
    /// Arrival minus capture time of recent frames, s, relative to the anchor's.
    lags: VecDeque<f64>,
}

impl Pacing {
    /// Takes a frame captured at `wall_ns` (daemon wall clock) that reached the display at
    /// `arrived`; the glide to it, s: the capture interval since the previous frame plus how
    /// much earlier this frame came than the latest recent one. `None` for the first frame,
    /// or one not captured after the previous (the history starts over from it).
    pub fn glide_s(&mut self, arrived: Instant, wall_ns: u64) -> Option<f64> {
        let Some((at0, wall0)) = self.anchor.filter(|_| wall_ns > self.last_ns) else {
            self.anchor = Some((arrived, wall_ns));
            self.last_ns = wall_ns;
            self.lags.clear();
            self.lags.push_back(0.0);
            return None;
        };
        let interval_s = (wall_ns - self.last_ns) as f64 / 1e9;
        let lag =
            arrived.saturating_duration_since(at0).as_secs_f64() - (wall_ns - wall0) as f64 / 1e9;
        self.last_ns = wall_ns;
        if self.lags.len() == PACING_FRAMES {
            self.lags.pop_front();
        }
        self.lags.push_back(lag);
        let latest = self.lags.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Some(duration_s(interval_s + (latest - lag)))
    }

    /// Starts over: the next frame is a first.
    pub fn clear(&mut self) {
        *self = Self::default();
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

    /// 85 ms frames handed on at 60 Hz: seven arrive 83 ms apart, then one 100 ms after.
    /// Glides paced by arrival intervals stop before the late frame; paced by capture time,
    /// each glide is still running when the next frame comes, and they end evenly spaced.
    #[test]
    fn glides_paced_by_capture_time_never_wait_for_a_late_frame() {
        let t0 = Instant::now();
        let hop_ns = 85_333_333u64;
        let handoff_ns = 16_666_667u64;
        let mut p = Pacing::default();
        let mut prev: Option<(Instant, f64)> = None;
        let mut ends = Vec::new();
        for k in 0..64u64 {
            let wall = 1_700_000_000_000_000_000 + k * hop_ns;
            // Arrives at the next hand-off after its capture.
            let since = k * hop_ns;
            let arrive_ns = since.div_ceil(handoff_ns) * handoff_ns + 3_000_000;
            let arrived = t0 + std::time::Duration::from_nanos(arrive_ns);
            let glide = p.glide_s(arrived, wall);
            if let Some((start, d)) = prev {
                let ran = arrived.duration_since(start).as_secs_f64();
                // Once the history holds the latest phase of the beat (the 18th frame), a
                // glide is still running when the next frame comes.
                if k > 17 {
                    assert!(
                        ran <= d + 1e-6,
                        "frame {k}: glide of {d} s stopped {ran} s in"
                    );
                }
            }
            match glide {
                Some(d) => {
                    prev = Some((arrived, d));
                    ends.push(arrived + std::time::Duration::from_secs_f64(d));
                }
                None => assert_eq!(k, 0, "only the first frame has no glide"),
            }
        }
        // Once the latest arrival is known, glides end a hop apart.
        let later: Vec<f64> = ends[17..]
            .windows(2)
            .map(|w| w[1].duration_since(w[0]).as_secs_f64())
            .collect();
        for dt in later {
            assert!((dt - 0.085_333).abs() < 1e-5, "glides end {dt} s apart");
        }
    }

    #[test]
    fn a_capture_clock_going_back_starts_the_pacing_over() {
        let t0 = Instant::now();
        let mut p = Pacing::default();
        assert_eq!(p.glide_s(t0, 1_000_000_000), None);
        let d = p.glide_s(t0 + std::time::Duration::from_millis(80), 1_080_000_000);
        assert!(d.is_some_and(|d| (d - 0.08).abs() < 1e-9), "{d:?}");
        assert_eq!(
            p.glide_s(t0 + std::time::Duration::from_millis(160), 900_000_000),
            None
        );
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
