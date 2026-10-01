//! Output side of the duplex stream: silence by default, an optional capped test tone, and
//! per-callback timing records for later generator↔loopback alignment.
//!
//! Safety contract (CLAUDE.md "Audio safety"): nothing emits unless an [`EmitLevel`] was
//! constructed, which refuses anything above [`EmitLevel::MAX_DBFS`]. Any emitting path fades
//! in and fades out over [`FADE_SECONDS`]; [`OutputControl::request_stop`] starts the
//! fade-out and [`OutputControl::is_silent`] reports when it is safe to tear down.

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::Serialize;

/// 6b: 20 ms fade, never a hard cut.
pub const FADE_SECONDS: f64 = 0.020;

/// Requested test-tone level, dBFS where 0 dBFS is the RMS of a full-scale sine (4a).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct EmitLevel(f64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmitLevelError {
    NotANumber(String),
    AboveCap { requested_milli_db: i64 },
}

impl std::fmt::Display for EmitLevelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotANumber(s) => write!(f, "not a dBFS level: {s:?}"),
            Self::AboveCap { requested_milli_db } => write!(
                f,
                "{:.1} dBFS exceeds the spike's cap of {} dBFS",
                *requested_milli_db as f64 / 1000.0,
                EmitLevel::MAX_DBFS
            ),
        }
    }
}

impl std::error::Error for EmitLevelError {}

impl EmitLevel {
    pub const MAX_DBFS: f64 = -20.0;

    pub fn new(dbfs: f64) -> Result<Self, EmitLevelError> {
        if !dbfs.is_finite() {
            return Err(EmitLevelError::NotANumber(dbfs.to_string()));
        }
        if dbfs > Self::MAX_DBFS {
            return Err(EmitLevelError::AboveCap {
                requested_milli_db: (dbfs * 1000.0).round() as i64,
            });
        }
        Ok(Self(dbfs))
    }

    /// Accepts `-30`, `-30dbfs`, `-30dBFS`.
    pub fn parse(s: &str) -> Result<Self, EmitLevelError> {
        let t = s.trim();
        let num = t
            .strip_suffix("dbfs")
            .or_else(|| t.strip_suffix("dBFS"))
            .or_else(|| t.strip_suffix("dBfs"))
            .unwrap_or(t);
        let v: f64 = num
            .trim()
            .parse()
            .map_err(|_| EmitLevelError::NotANumber(s.to_string()))?;
        Self::new(v)
    }

    pub fn dbfs(self) -> f64 {
        self.0
    }

    /// Peak amplitude of a sine at this level: 0 dBFS RMS-of-full-scale-sine means a
    /// full-scale sine (peak 1.0) is 0 dBFS, so peak = 10^(L/20).
    pub fn sine_peak(self) -> f64 {
        10f64.powf(self.0 / 20.0)
    }
}

/// What the output callback produces.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum OutputMode {
    Silence,
    /// Sine on the first output channel only.
    Tone {
        level: EmitLevel,
        freq_hz: f64,
    },
}

/// Shared between the control thread and the output callback.
#[derive(Debug, Default)]
pub struct OutputControl {
    stop: AtomicBool,
    silent: AtomicBool,
    pub callbacks: AtomicU64,
    pub frames: AtomicU64,
}

impl OutputControl {
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// True once the renderer has finished its fade-out (or never emitted).
    pub fn is_silent(&self) -> bool {
        self.silent.load(Ordering::Acquire)
    }
}

/// One output callback's timing; lets a consumer map generator sample index to the host's
/// predicted DAC time (plausibility only — the loopback is the reference, 3a).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct OutputTick {
    pub start_sample: u64,
    pub frames: u32,
    pub callback_ns: u64,
    pub playback_ns: Option<u64>,
}

/// RT-side renderer owned by the output callback.
#[derive(Debug)]
pub struct OutputRenderer {
    mode: OutputMode,
    sample_rate: f64,
    phase: f64,
    gain: f64,
    gain_step: f64,
    next_sample: u64,
    control: Arc<OutputControl>,
    ticks: rtrb::Producer<OutputTick>,
}

impl OutputRenderer {
    pub fn new(
        mode: OutputMode,
        sample_rate: u32,
        control: Arc<OutputControl>,
        ticks: rtrb::Producer<OutputTick>,
    ) -> Self {
        let sr = f64::from(sample_rate);
        if matches!(mode, OutputMode::Silence) {
            control.silent.store(true, Ordering::Release);
        }
        Self {
            mode,
            sample_rate: sr,
            phase: 0.0,
            gain: 0.0,
            gain_step: 1.0 / (FADE_SECONDS * sr).max(1.0),
            next_sample: 0,
            control,
            ticks,
        }
    }

    /// Next mono sample of the emitted signal (0.0 for silence).
    #[inline]
    fn next(&mut self, stopping: bool) -> f32 {
        match self.mode {
            OutputMode::Silence => 0.0,
            OutputMode::Tone { level, freq_hz } => {
                if stopping {
                    self.gain = (self.gain - self.gain_step).max(0.0);
                } else {
                    self.gain = (self.gain + self.gain_step).min(1.0);
                }
                let s = level.sine_peak() * self.gain * self.phase.sin();
                self.phase += TAU * freq_hz / self.sample_rate;
                if self.phase >= TAU {
                    self.phase -= TAU;
                }
                // Belt and braces: never exceed the cap even if the fade math were wrong.
                let cap = EmitLevel(EmitLevel::MAX_DBFS).sine_peak();
                s.clamp(-cap, cap) as f32
            }
        }
    }

    /// Render one interleaved callback buffer of `frames` frames by calling
    /// `write(frame, channel, value)`. Only channel 0 carries the tone.
    #[inline]
    pub fn render(
        &mut self,
        frames: usize,
        channels: usize,
        callback_ns: u64,
        playback_ns: Option<u64>,
        mut write: impl FnMut(usize, usize, f32),
    ) {
        let stopping = self.control.stop.load(Ordering::Acquire);
        for f in 0..frames {
            let s = self.next(stopping);
            if channels > 0 {
                write(f, 0, s);
            }
            for c in 1..channels {
                write(f, c, 0.0);
            }
        }
        if stopping && self.gain == 0.0 {
            self.control.silent.store(true, Ordering::Release);
        }
        // Timing records are best effort: a full ring just loses records, never blocks.
        let _ = self.ticks.push(OutputTick {
            start_sample: self.next_sample,
            frames: frames as u32,
            callback_ns,
            playback_ns,
        });
        self.next_sample += frames as u64;
        self.control.callbacks.fetch_add(1, Ordering::Relaxed);
        self.control
            .frames
            .fetch_add(frames as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer(
        mode: OutputMode,
    ) -> (
        OutputRenderer,
        Arc<OutputControl>,
        rtrb::Consumer<OutputTick>,
    ) {
        let ctl = Arc::new(OutputControl::default());
        let (p, c) = rtrb::RingBuffer::new(64);
        (
            OutputRenderer::new(mode, 48_000, Arc::clone(&ctl), p),
            ctl,
            c,
        )
    }

    #[test]
    fn level_cap_is_enforced() {
        assert!(EmitLevel::new(-19.9).is_err());
        assert!(EmitLevel::new(0.0).is_err());
        assert!(EmitLevel::new(f64::NAN).is_err());
        assert!(EmitLevel::parse("-10dbfs").is_err());
        assert_eq!(EmitLevel::parse("-30dBFS").map(EmitLevel::dbfs), Ok(-30.0));
        assert_eq!(EmitLevel::parse(" -20 ").map(EmitLevel::dbfs), Ok(-20.0));
        assert!(EmitLevel::parse("loud").is_err());
    }

    #[test]
    fn silence_mode_writes_zeros_and_is_immediately_silent() {
        let (mut r, ctl, mut ticks) = renderer(OutputMode::Silence);
        assert!(ctl.is_silent());
        let mut buf = vec![1.0f32; 256 * 4];
        r.render(256, 4, 10, Some(20), |f, c, v| buf[f * 4 + c] = v);
        assert!(buf.iter().all(|&s| s == 0.0));
        let t = ticks.pop().expect("tick");
        assert_eq!(
            (t.start_sample, t.frames, t.playback_ns),
            (0, 256, Some(20))
        );
    }

    #[test]
    fn tone_fades_in_stays_under_cap_and_fades_out() {
        let level = EmitLevel::new(-20.0).expect("level");
        let (mut r, ctl, _t) = renderer(OutputMode::Tone {
            level,
            freq_hz: 1000.0,
        });
        assert!(!ctl.is_silent());
        let fade = (FADE_SECONDS * 48_000.0) as usize;
        let mut out = vec![0.0f32; 48_000];
        r.render(48_000, 1, 0, None, |f, _, v| out[f] = v);
        let peak = level.sine_peak() as f32;
        assert!(out.iter().all(|s| s.abs() <= peak + 1e-6));
        // First 1 ms is well below full level (fade-in).
        assert!(out[..48].iter().all(|s| s.abs() < peak * 0.06));
        // After the fade the tone reaches its level.
        let late_peak = out[fade..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((late_peak - peak).abs() < peak * 0.01);

        ctl.request_stop();
        let mut tail = vec![0.0f32; fade + 64];
        r.render(tail.len(), 1, 0, None, |f, _, v| tail[f] = v);
        assert!(ctl.is_silent());
        assert!(tail[fade + 1..].iter().all(|&s| s == 0.0));
        // No hard cut: the first samples after stop are still near full level envelope.
        let early = tail[..48].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(early > peak * 0.8);
    }
}
