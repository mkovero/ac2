//! Where a live spectrograph's time axis stands at the moment a picture is drawn.
//!
//! Frames reach the app on a steady clock, but the audio each carries advances unevenly:
//! the daemon hands on whatever whole blocks its analysis has finished, so at 96 kHz with
//! 1024-sample blocks one frame moves the audio on 10.7 ms and the next 42.7 ms. Rows
//! placed by the frames alone jump by those steps, which the eye reads as judder, and the
//! jumps are the audio's, not the display's.
//!
//! The time axis instead runs on the display's own clock, `now − offset`, in the session's
//! audio time (`audio_sample ÷ rate`), so it moves the same distance every display frame.
//! The offset is the earliest the audio has recently reached the app (the smallest of
//! arrival − audio time over the last [`WINDOW`] frames), so when a frame arrives the axis
//! already stands at or past the audio it carries: the newest frame shows the pass it
//! arrives, at the top (the 3D view's front), never held back above the edge. Between
//! frames the axis runs on past the newest audio by up to the frames' spread and a hop
//! ([`ScrollClock::reach_s`]), the newest column standing in for that time, as the slots
//! between two frames do once the next one is in. Further, the stream has stalled: the
//! axis waits there.
//!
//! The offset rises towards its target at most [`SLEW`] seconds per second, a speed change
//! of a few percent the eye does not see, so the axis never steps back when the earliest
//! frame leaves the window. A frame earlier than any before moves the axis on to it at
//! once (rare once the window holds the hand-off's earliest phase). A jump beyond
//! [`RESET_S`] (a stall, a new session, the audio clock going back) is a new start.

use std::collections::VecDeque;
use std::time::Instant;

/// Frames over which the latest arrival is remembered: about two seconds at the 60 frames
/// a second the daemon publishes, enough for the hand-off's beat against the analysis
/// blocks to come back to its latest phase several times.
pub const WINDOW: usize = 120;
/// Fastest change of the offset, seconds per second: the axis runs between 0.95 and 1.05
/// times real time while it settles.
pub const SLEW: f64 = 0.05;
/// A frame further than this from where the axis stands starts it over.
pub const RESET_S: f64 = 0.25;

/// The display-clock time axis of one live spectrograph.
#[derive(Clone, Debug, Default)]
pub struct ScrollClock {
    /// When the first frame arrived and the audio time it carried, s: lags are kept
    /// relative to them, small enough for an `f64`.
    anchor: Option<(Instant, f64)>,
    /// Audio time of the newest frame, s.
    newest_s: f64,
    /// Arrival minus audio time of recent frames, s, relative to the anchor's, and the
    /// audio step each frame made.
    lags: VecDeque<(f64, f64)>,
    /// How far past the newest frame the axis runs: the largest recent step and the
    /// spread of the lags.
    reach_s: f64,
    /// The offset at `at`, and the one it moves to.
    offset: f64,
    at: Option<Instant>,
    target: f64,
}

/// `b − a` in seconds, negative when `b` is earlier.
fn secs(a: Instant, b: Instant) -> f64 {
    match b.checked_duration_since(a) {
        Some(d) => d.as_secs_f64(),
        None => -a.duration_since(b).as_secs_f64(),
    }
}

impl ScrollClock {
    /// Takes a frame carrying audio up to `audio_s` (session audio time, s) that arrived
    /// at `arrived`. A frame no newer than the newest is ignored; one older, or one far
    /// from where the axis stands, starts the clock over from it.
    pub fn observe(&mut self, arrived: Instant, audio_s: f64) {
        if !audio_s.is_finite() {
            return;
        }
        let Some((t0, a0)) = self.anchor.filter(|_| audio_s >= self.newest_s) else {
            self.start(arrived, audio_s);
            return;
        };
        if audio_s == self.newest_s {
            return;
        }
        let lag = secs(t0, arrived) - (audio_s - a0);
        let offset = self.offset_at(arrived);
        if (lag - offset).abs() > RESET_S {
            self.start(arrived, audio_s);
            return;
        }
        let step = audio_s - self.newest_s;
        self.newest_s = audio_s;
        if self.lags.len() == WINDOW {
            self.lags.pop_front();
        }
        self.lags.push_back((lag, step));
        // Never behind the frame just in: an early one moves the axis on to it.
        self.offset = offset.min(lag);
        self.at = Some(arrived);
        let (lo, hi, step) = self.lags.iter().fold(
            (f64::INFINITY, f64::NEG_INFINITY, 0.0f64),
            |(lo, hi, st), &(l, s)| (lo.min(l), hi.max(l), st.max(s)),
        );
        self.target = lo;
        self.reach_s = step + (hi - lo);
    }

    fn start(&mut self, arrived: Instant, audio_s: f64) {
        *self = Self {
            anchor: Some((arrived, audio_s)),
            newest_s: audio_s,
            lags: VecDeque::from([(0.0, 0.0)]),
            reach_s: 0.0,
            offset: 0.0,
            at: Some(arrived),
            target: 0.0,
        };
    }

    /// The offset at `now`, on its way from the last frame's towards the target.
    fn offset_at(&self, now: Instant) -> f64 {
        let dt = self.at.map_or(0.0, |at| secs(at, now).max(0.0));
        let step = SLEW * dt;
        self.offset + (self.target - self.offset).clamp(-step, step)
    }

    /// The audio time, s, the axis stands at `now`; `None` before the first frame. It runs
    /// past the newest frame while the next is due, at most [`Self::reach_s`].
    pub fn position_s(&self, now: Instant) -> Option<f64> {
        let (t0, a0) = self.anchor?;
        Some((a0 + secs(t0, now) - self.offset_at(now)).min(self.newest_s + self.reach_s))
    }

    /// How far past the newest frame the axis runs before it waits for the next, s: the
    /// largest audio step of the recent frames and the spread of their arrivals, the most
    /// a frame on time can still be due.
    pub fn reach_s(&self) -> f64 {
        self.reach_s
    }

    /// The axis still moves at `now`: it has not reached where it waits for a stalled
    /// stream.
    pub fn moving(&self, now: Instant) -> bool {
        self.anchor.is_some()
            && self
                .position_s(now)
                .is_some_and(|p| p < self.newest_s + self.reach_s)
    }

    /// Audio time of the newest frame, s.
    pub fn newest_s(&self) -> Option<f64> {
        self.anchor.map(|_| self.newest_s)
    }

    /// Starts over: the next frame is a first.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const FRAME_S: f64 = 1.0 / 60.0;
    const BLOCK_S: f64 = 1024.0 / 96_000.0;

    /// Frames every 1/60 s carrying whole 1024-sample blocks at 96 kHz, the audio finished
    /// some 0–20 ms before each hand-off: steps of 10.7, 21.3, 32 and 42.7 ms.
    fn uneven(i: u64) -> (f64, f64) {
        let t = i as f64 * FRAME_S;
        // A fixed scramble of the analysis lag, 0–20 ms, repeating with the blocks' own
        // beat against the frames (25 blocks in 16 frames).
        let u = ((i * 7) % 16) as f64 / 15.0 * 0.020;
        let audio = ((t - 0.005 - u) / BLOCK_S).floor() * BLOCK_S;
        (t, audio)
    }

    #[test]
    fn the_axis_moves_evenly_under_uneven_audio_steps() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut c = ScrollClock::default();
        let mut steps = std::collections::BTreeSet::<i64>::new();
        let mut prev_audio: Option<f64> = None;
        let mut positions = Vec::new();
        for i in 0..600u64 {
            let (t, audio) = uneven(i);
            if let Some(p) = prev_audio {
                steps.insert(((audio - p) / BLOCK_S).round() as i64);
            }
            prev_audio = Some(audio);
            c.observe(at(t), audio);
            // Drawn 4 ms after the frame, once per display frame.
            if i > 240 {
                let p = c.position_s(at(t + 0.004)).expect("started");
                // The frame just in is shown: the axis stands at or past it.
                assert!(p >= audio, "frame {i}: {p} behind {audio}");
                // And past it by no more than the spread of the arrivals.
                assert!(
                    p <= audio + 0.004 + 0.032,
                    "frame {i}: {p} ahead of {audio}"
                );
                positions.push(p);
            }
        }
        assert!(steps.len() >= 3, "the fixture steps unevenly: {steps:?}");
        for w in positions.windows(2) {
            let d = w[1] - w[0];
            assert!(
                (d - FRAME_S).abs() < 1e-9,
                "advance {d} s per display frame, not {FRAME_S}"
            );
        }
    }

    #[test]
    fn the_axis_waits_a_hop_past_the_newest_frame() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut c = ScrollClock::default();
        // Frames 85 ms apart, arriving 85 ms apart.
        for i in 0..10 {
            c.observe(at(i as f64 * 0.085), 1.0 + i as f64 * 0.085);
        }
        let last = 9.0 * 0.085;
        assert!((c.reach_s() - 0.085).abs() < 1e-9);
        // The axis runs on until the next frame is due ...
        assert!(c.moving(at(last + 0.08)));
        assert!(
            (c.position_s(at(last + 0.08)).expect("started") - (1.0 + last + 0.08)).abs() < 1e-9
        );
        // ... and waits there when it does not come.
        assert!(!c.moving(at(last + 0.2)));
        assert!(
            (c.position_s(at(last + 0.2)).expect("started") - (1.0 + last + 0.085)).abs() < 1e-9
        );
    }

    #[test]
    fn a_stall_or_the_clock_going_back_starts_over() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut c = ScrollClock::default();
        assert_eq!(c.position_s(t0), None);
        c.observe(at(0.0), 10.0);
        c.observe(at(0.02), 10.02);
        assert!((c.position_s(at(0.02)).expect("started") - 10.02).abs() < 1e-9);
        // A second's stall: the axis starts from the frame after it.
        c.observe(at(1.5), 10.6);
        assert!((c.position_s(at(1.5)).expect("started") - 10.6).abs() < 1e-9);
        // A new session's audio clock starts near zero.
        c.observe(at(1.6), 0.1);
        assert_eq!(c.newest_s(), Some(0.1));
        assert!((c.position_s(at(1.6)).expect("started") - 0.1).abs() < 1e-9);
    }

    #[test]
    fn an_early_frame_moves_the_axis_on_and_a_late_one_never_back() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut c = ScrollClock::default();
        c.observe(at(0.0), 0.0);
        // A frame 30 ms late: the axis keeps its pace, ahead of that frame.
        c.observe(at(0.05), 0.02);
        let p = c.position_s(at(0.05)).expect("started");
        assert!((p - 0.05).abs() < 1e-9, "{p}");
        // A frame 10 ms early: the axis moves on to it at once.
        c.observe(at(0.06), 0.07);
        assert!((c.position_s(at(0.06)).expect("started") - 0.07).abs() < 1e-9);
        // The earliest leaves the window: the axis slows by SLEW, it does not step back.
        for i in 1..=WINDOW as u64 {
            let t = 0.06 + i as f64 * FRAME_S;
            c.observe(at(t), t + 0.01 - 0.005);
        }
        let t = 0.06 + WINDOW as f64 * FRAME_S;
        let a = c.position_s(at(t)).expect("started");
        let b = c.position_s(at(t + 0.01)).expect("started");
        assert!(
            (b - a - 0.01 * (1.0 - SLEW)).abs() < 1e-9,
            "{} per 10 ms",
            b - a
        );
    }
}
