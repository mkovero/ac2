//! Bursty delivery: a host that hands over audio in large lumps instead of a steady stream
//! of short callbacks.
//!
//! The daemon publishes the newest frame of every topic and a client marks a frame STALE
//! when nothing newer arrived for a second. A host that delivers 1.4 s of audio at once and
//! then nothing for 1.4 s (PipeWire's ALSA plugin left to choose its own period did exactly
//! that) makes every meter and measurement read stale between lumps, although no sample is
//! lost. The signature is unambiguous at the daemon: a pause in block arrivals longer than
//! [`BURST_GAP`], then more than [`BURST_GAP`] of audio arriving within [`BURST_WINDOW`],
//! far faster than real time. A stalled device that resumes delivers at real time and does
//! not match.

use std::time::{Duration, Instant};

/// Arrival pause, and audio arriving at once after it, that make a burst. A steady stream
/// of even 4096-frame callbacks at 44.1 kHz arrives every 93 ms.
pub(crate) const BURST_GAP: Duration = Duration::from_millis(200);
/// Blocks arriving this soon after the first one past a pause belong to the same lump.
const BURST_WINDOW: Duration = Duration::from_millis(100);
/// At most one warning per stream this often.
pub(crate) const WARN_EVERY: Duration = Duration::from_secs(60);

/// One lump: the pause before it and the audio it carried.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Burst {
    pub(crate) gap: Duration,
    pub(crate) audio: Duration,
}

struct Lump {
    start: Instant,
    gap: Duration,
    frames: u64,
}

/// `JACK device "jack", 256-frame buffer`: what a burst warning names.
pub(crate) fn label(n: &ac2_audio::Negotiated) -> String {
    let buffer = n.buffer_frames.map_or_else(
        || "host-chosen buffer".to_owned(),
        |b| format!("{b}-frame buffer"),
    );
    format!("{:?} device {:?}, {buffer}", n.backend, n.input_device.0)
}

/// Watches block arrivals of one stream.
pub(crate) struct BurstDetector {
    rate: f64,
    last: Option<Instant>,
    lump: Option<Lump>,
    warned: Option<Instant>,
    /// What the warning names: backend, device and buffer size.
    label: String,
}

impl BurstDetector {
    pub(crate) fn new(rate: u32, label: String) -> Self {
        Self {
            rate: f64::from(rate.max(1)),
            last: None,
            lump: None,
            warned: None,
            label,
        }
    }

    /// A block of `frames` arrived at `now`; the burst it completes, if any.
    pub(crate) fn block(&mut self, now: Instant, frames: u32) -> Option<Burst> {
        let gap = self.last.map(|l| now.saturating_duration_since(l));
        self.last = Some(now);
        match (gap, &mut self.lump) {
            (Some(g), _) if g > BURST_GAP => {
                self.lump = Some(Lump {
                    start: now,
                    gap: g,
                    frames: u64::from(frames),
                });
            }
            (_, Some(l)) if now.saturating_duration_since(l.start) <= BURST_WINDOW => {
                l.frames += u64::from(frames);
            }
            _ => self.lump = None,
        }
        let l = self.lump.as_ref()?;
        let audio = Duration::from_secs_f64(l.frames as f64 / self.rate);
        if audio <= BURST_GAP {
            return None;
        }
        let b = Burst { gap: l.gap, audio };
        self.lump = None;
        Some(b)
    }

    /// [`Self::block`], logging a warning at most once per [`WARN_EVERY`]. Returns whether
    /// it warned.
    pub(crate) fn observe(&mut self, now: Instant, frames: u32) -> bool {
        let Some(b) = self.block(now, frames) else {
            return false;
        };
        if self
            .warned
            .is_some_and(|w| now.saturating_duration_since(w) < WARN_EVERY)
        {
            return false;
        }
        self.warned = Some(now);
        tracing::warn!(
            "audio arrives in bursts on {}: nothing for {} ms, then {} ms at once; meters and \
             measurements go stale between bursts. Use a smaller buffer (about 20 ms: 1024 \
             frames at 48 kHz, or the server's period on JACK)",
            self.label,
            b.gap.as_millis(),
            b.audio.as_millis()
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: u32 = 48_000;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Steady 1024-frame callbacks (21 ms) never look like a burst, however long.
    #[test]
    fn a_steady_stream_is_not_a_burst() {
        let mut d = BurstDetector::new(FS, "test".into());
        let t0 = Instant::now();
        for i in 0..2000u32 {
            let t = t0 + Duration::from_secs_f64(f64::from(i) * 1024.0 / f64::from(FS));
            assert_eq!(d.block(t, 1024), None, "block {i}");
        }
    }

    /// A device that stalls and then resumes at real time is not a burst either.
    #[test]
    fn a_stall_that_resumes_at_real_time_is_not_a_burst() {
        let mut d = BurstDetector::new(FS, "test".into());
        let t0 = Instant::now();
        assert_eq!(d.block(t0, 256), None);
        let resume = t0 + ms(800);
        for i in 0..100u32 {
            let t = resume + Duration::from_secs_f64(f64::from(i) * 256.0 / f64::from(FS));
            assert_eq!(d.block(t, 256), None, "block {i}");
        }
    }

    /// 65,536 frames (1.37 s) at once every 1.37 s, in 1024-frame blocks popped together.
    #[test]
    fn lumps_of_audio_after_pauses_are_bursts() {
        let mut d = BurstDetector::new(FS, "test".into());
        let t0 = Instant::now();
        let mut found = Vec::new();
        for lump in 0..4u64 {
            let at = t0 + ms(1365 * lump);
            for i in 0..64u64 {
                if let Some(b) = d.block(at + Duration::from_micros(20 * i), 1024) {
                    found.push((lump, b));
                }
            }
        }
        // The first lump has no pause before it.
        assert_eq!(found.len(), 3, "{found:?}");
        for (_, b) in found {
            assert!(b.gap >= ms(1300) && b.gap <= ms(1400), "{b:?}");
            assert!(b.audio > BURST_GAP, "{b:?}");
        }
    }

    #[test]
    fn warns_once_per_minute() {
        let mut d = BurstDetector::new(FS, "test".into());
        let t0 = Instant::now();
        let lump = |d: &mut BurstDetector, at: Instant| {
            (0..32u64).any(|i| d.observe(at + Duration::from_micros(10 * i), 1024))
        };
        assert!(!lump(&mut d, t0));
        assert!(lump(&mut d, t0 + ms(700)));
        assert!(!lump(&mut d, t0 + ms(1400)));
        assert!(!lump(&mut d, t0 + ms(30_000)));
        assert!(lump(&mut d, t0 + ms(61_000)));
    }
}
