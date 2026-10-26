//! Frame pacing for repaints asked by data and animation.
//!
//! The surface does not wait for vblank ([`crate::gpu::PRESENT_MODE`]), so nothing but the
//! app keeps a stream of daemon frames or an easing animation from drawing as fast as the
//! renderer can. Frames asked this way start at least one interval apart, counted from the
//! last frame's start, so a frame's own drawing time is inside the interval rather than
//! added to it. Input is not paced: egui repaints at once for a key or pointer event.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use eframe::egui;

/// The least time between frame starts, and when the last frame started. Shared between
/// the UI thread (which marks frames) and the daemon link's thread (which asks for them).
#[derive(Debug)]
pub struct Pacer {
    interval: Duration,
    epoch: Instant,
    /// Nanoseconds from `epoch` to the last frame's start; `u64::MAX` before the first.
    last_start_ns: AtomicU64,
}

impl Pacer {
    pub fn new(interval: Duration) -> Arc<Self> {
        Arc::new(Self {
            interval,
            epoch: Instant::now(),
            last_start_ns: AtomicU64::new(u64::MAX),
        })
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// A frame starts at `now`.
    pub fn frame_started(&self, now: Instant) {
        let ns = now.saturating_duration_since(self.epoch).as_nanos();
        self.last_start_ns
            .store(u64::try_from(ns).unwrap_or(u64::MAX - 1), Ordering::Relaxed);
    }

    /// How long from `now` until the next paced frame may start.
    pub fn wait(&self, now: Instant) -> Duration {
        match self.last_start_ns.load(Ordering::Relaxed) {
            u64::MAX => Duration::ZERO,
            ns => (self.epoch + Duration::from_nanos(ns) + self.interval)
                .saturating_duration_since(now),
        }
    }

    /// Asks egui for the next paced frame.
    pub fn request(&self, ctx: &egui::Context) {
        let early = ctx.input(|i| i.predicted_dt);
        ctx.request_repaint_after(repaint_delay(self.wait(Instant::now()), early));
    }
}

/// The delay to hand `request_repaint_after` for a frame `wait` from now. egui takes its
/// predicted frame time (`predicted_dt`, 1/60 s under eframe) off every delay so the frame
/// is ready by then; uncorrected, a 1/60 s cap would become no cap at all and a 1/30 s one
/// would run at 60. The result is never zero: egui answers a zero delay with two passes.
pub fn repaint_delay(wait: Duration, predicted_dt: f32) -> Duration {
    let early = Duration::try_from_secs_f32(predicted_dt).unwrap_or(Duration::ZERO);
    (wait + early).max(Duration::from_nanos(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn the_first_frame_is_not_held() {
        let p = Pacer::new(16 * MS);
        assert_eq!(p.wait(Instant::now()), Duration::ZERO);
    }

    /// The interval counts from the frame's start: time spent drawing is not added.
    #[test]
    fn frames_start_one_interval_apart() {
        let p = Pacer::new(16 * MS);
        let t0 = Instant::now();
        p.frame_started(t0);
        assert_eq!(p.wait(t0), 16 * MS);
        assert_eq!(p.wait(t0 + 10 * MS), 6 * MS);
        assert_eq!(p.wait(t0 + 16 * MS), Duration::ZERO);
        // A frame that took longer than the interval: the next one at once.
        assert_eq!(p.wait(t0 + 40 * MS), Duration::ZERO);
    }

    /// egui's own subtraction of the predicted frame time is undone, so the frame lands
    /// `wait` from now, and a frame due now is still a single pass.
    #[test]
    fn egui_early_wake_is_compensated() {
        let dt = 1.0 / 60.0;
        let d = repaint_delay(16 * MS, dt);
        let early = Duration::from_secs_f32(dt);
        assert_eq!(d.saturating_sub(early), 16 * MS);
        assert_eq!(repaint_delay(Duration::ZERO, dt), early);
        assert_eq!(repaint_delay(Duration::ZERO, 0.0), Duration::from_nanos(1));
        assert_eq!(repaint_delay(5 * MS, f32::NAN), 5 * MS);
    }
}
