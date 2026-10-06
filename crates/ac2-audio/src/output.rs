//! The output path shared by every backend: renders silence or the generator, applies
//! fades, gain and the global level limit, records the generator history and emits one
//! [`OutputTick`] per callback.
//!
//! Safety contract (CLAUDE.md "Audio safety"): with [`OutputSource::Silence`] nothing but
//! zeros is ever written. With a generator, every change of audibility is a linear ramp
//! over [`FADE_SECONDS`] (decision 6b: never a hard cut), and every written sample is
//! bounded by the request's [`MaxLevel`]; limited samples are counted and flagged.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::block::BlockFlags;
use crate::generator::{GeneratorPort, RouteSet, SignalSource};
use crate::history::{HistoryReader, HistoryWriter, history};
use crate::level::{Gain, MaxLevel};

/// Fade length for start, stop and source swaps (decision 6b).
pub const FADE_SECONDS: f64 = 0.020;

/// What the output callback emits.
#[derive(Debug, Default)]
pub enum OutputSource {
    /// Zeros on every channel. Never anything else.
    #[default]
    Silence,
    /// A lock-free controlled generator; see [`generator`](crate::generator()).
    Generator(GeneratorPort),
}

/// Audibility of the output, as of the most recent rendered block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputState {
    /// Only zeros are written.
    Silent,
    /// Ramping up.
    FadingIn,
    /// At the set gain.
    Active,
    /// Ramping down (stop, shutdown or source swap).
    FadingOut,
}

impl OutputState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::FadingIn,
            2 => Self::Active,
            3 => Self::FadingOut,
            _ => Self::Silent,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            Self::Silent => 0,
            Self::FadingIn => 1,
            Self::Active => 2,
            Self::FadingOut => 3,
        }
    }
}

/// State shared between control threads and the output callback.
#[derive(Debug)]
pub(crate) struct OutputShared {
    /// The generator should be audible.
    pub(crate) run: AtomicBool,
    /// The stream is stopping: fade out and stay silent for good.
    pub(crate) shutdown: AtomicBool,
    pub(crate) gain_bits: AtomicU32,
    /// Sample-peak limit set while the stream runs (linear, `f32` bits): the callback
    /// enforces the lower of it and the limit the stream was opened with, so it can tighten
    /// the open limit but never loosen it.
    pub(crate) limit_bits: AtomicU32,
    state: AtomicU8,
    /// Anything other than zeros was ever written.
    emitted: AtomicBool,
    limited_samples: AtomicU64,
    frames_rendered: AtomicU64,
    ticks_dropped: AtomicU64,
}

impl Default for OutputShared {
    fn default() -> Self {
        Self {
            run: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            gain_bits: AtomicU32::new(Gain::UNITY.to_bits()),
            limit_bits: AtomicU32::new(1.0f32.to_bits()),
            state: AtomicU8::new(OutputState::Silent.to_u8()),
            emitted: AtomicBool::new(false),
            limited_samples: AtomicU64::new(0),
            frames_rendered: AtomicU64::new(0),
            ticks_dropped: AtomicU64::new(0),
        }
    }
}

impl OutputShared {
    pub(crate) fn state(&self) -> OutputState {
        OutputState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub(crate) fn begin_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    pub(crate) fn has_emitted(&self) -> bool {
        self.emitted.load(Ordering::Acquire)
    }

    pub(crate) fn stats(&self) -> OutputStats {
        OutputStats {
            state: self.state(),
            frames_rendered: self.frames_rendered.load(Ordering::Relaxed),
            limited_samples: self.limited_samples.load(Ordering::Relaxed),
            ticks_dropped: self.ticks_dropped.load(Ordering::Relaxed),
        }
    }
}

/// Output-side counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputStats {
    /// Audibility as of the latest block.
    pub state: OutputState,
    /// Frames rendered since open.
    pub frames_rendered: u64,
    /// Samples that hit the [`MaxLevel`] limit or were not finite. Non-zero means a level
    /// computation upstream is wrong.
    pub limited_samples: u64,
    /// [`OutputTick`] records lost because nobody drained them.
    pub ticks_dropped: u64,
}

/// One output callback's timing.
///
/// Lets a consumer relate output sample indices to host time. The host's playback time is
/// an estimate that misses converter and external latency; the loopback correlation is the
/// truth (decision 3a). On single-callback backends (JACK, fake) `start_sample` is the same
/// index as the capture block of the same cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputTick {
    /// Output index of the first frame.
    pub start_sample: u64,
    /// Frames rendered.
    pub frames: u32,
    /// Continuity bits of the output index (gaps, xruns, config changes).
    pub flags: BlockFlags,
    /// When the callback ran (backend clock, ns).
    pub callback_ns: u64,
    /// Host estimate of when the first frame reaches the DAC, if any.
    pub playback_ns: Option<u64>,
    /// Samples in this block that were limited (see [`OutputStats::limited_samples`]).
    pub limited_samples: u32,
    /// Audibility at the end of this block.
    pub state: OutputState,
}

/// Index and timing of one output block, supplied by the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutputStamp {
    pub(crate) start_sample: u64,
    pub(crate) frames: u32,
    pub(crate) flags: BlockFlags,
    pub(crate) callback_ns: u64,
    pub(crate) playback_ns: Option<u64>,
}

/// Settings for [`OutputRenderer::new`].
#[derive(Debug)]
pub(crate) struct RendererConfig {
    pub(crate) source: OutputSource,
    /// Channels the stream exposes; the generator routes must be below this.
    pub(crate) output_channels: u16,
    pub(crate) sample_rate: u32,
    pub(crate) max_level: MaxLevel,
    /// Output channel to record and how many frames to keep.
    pub(crate) history: Option<(u16, usize)>,
}

/// What the stream keeps from the renderer.
#[derive(Debug)]
pub(crate) struct RendererParts {
    pub(crate) shared: Arc<OutputShared>,
    pub(crate) ticks: rtrb::Consumer<OutputTick>,
    pub(crate) history: Option<HistoryReader>,
}

/// Tick records kept for the consumer: several seconds even at small blocks.
const TICK_CAPACITY: usize = 4096;
/// Mono render chunk; callbacks larger than this are rendered in pieces.
const CHUNK: usize = 1024;

/// Callback-side renderer. Never allocates, locks or blocks after construction.
pub(crate) struct OutputRenderer {
    shared: Arc<OutputShared>,
    sources: Option<rtrb::Consumer<Box<dyn SignalSource>>>,
    retired: Option<rtrb::Producer<Box<dyn SignalSource>>>,
    source: Option<Box<dyn SignalSource>>,
    route_changes: Option<rtrb::Consumer<RouteSet>>,
    routes: RouteSet,
    output_channels: usize,
    history_channel: usize,
    history: Option<HistoryWriter>,
    max: f32,
    /// Fade envelope position in samples, 0 (silent) ..= `fade_frames` (full). An integer
    /// position makes the end points exact: a fade-out reaches true zero after exactly
    /// `fade_frames` samples, with no float accumulation residue.
    env_pos: u32,
    fade_frames: u32,
    gain: f32,
    gain_step: f32,
    scratch: Box<[f32]>,
    zeros: Box<[f32]>,
    ticks: rtrb::Producer<OutputTick>,
}

impl std::fmt::Debug for OutputRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputRenderer")
            .field("routes", &self.routes)
            .field("env_pos", &self.env_pos)
            .finish_non_exhaustive()
    }
}

impl OutputRenderer {
    /// Builds the renderer. Routes and history channel must have been validated against
    /// `output_channels` by the caller.
    pub(crate) fn new(cfg: RendererConfig) -> (Self, RendererParts) {
        let (shared, sources, retired, route_changes, routes) = match cfg.source {
            OutputSource::Silence => (
                Arc::new(OutputShared::default()),
                None,
                None,
                None,
                RouteSet::default(),
            ),
            OutputSource::Generator(port) => (
                port.shared,
                Some(port.sources),
                Some(port.retired),
                Some(port.route_changes),
                port.set,
            ),
        };
        let (history_writer, history_reader, history_channel) = match cfg.history {
            Some((ch, frames)) => {
                let (w, r) = history(frames);
                (Some(w), Some(r), usize::from(ch))
            }
            None => (None, None, 0),
        };
        let (tp, tc) = rtrb::RingBuffer::new(TICK_CAPACITY);
        let fade_frames = (FADE_SECONDS * f64::from(cfg.sample_rate)).round().max(1.0) as u32;
        let renderer = Self {
            shared: Arc::clone(&shared),
            sources,
            retired,
            source: None,
            route_changes,
            routes,
            output_channels: usize::from(cfg.output_channels),
            history_channel,
            history: history_writer,
            max: cfg.max_level.linear(),
            env_pos: 0,
            fade_frames,
            gain: 0.0,
            gain_step: 1.0 / fade_frames as f32,
            scratch: vec![0.0; CHUNK].into_boxed_slice(),
            zeros: vec![0.0; CHUNK].into_boxed_slice(),
            ticks: tp,
        };
        (
            renderer,
            RendererParts {
                shared,
                ticks: tc,
                history: history_reader,
            },
        )
    }

    fn is_routed(&self, channel: usize) -> bool {
        channel < self.output_channels && self.routes.contains(channel)
    }

    /// Applies a queued routing change once nothing is audible. Returns true while a change
    /// is waiting for the fade-out: a channel never starts or stops carrying the signal
    /// mid-waveform.
    fn service_route_queue(&mut self) -> bool {
        let Some(queue) = self.route_changes.as_mut() else {
            return false;
        };
        loop {
            let Ok(next) = queue.peek().copied() else {
                return false;
            };
            if next != self.routes && self.env_pos > 0 {
                return true;
            }
            self.routes = next;
            let _ = queue.pop();
        }
    }

    /// Takes a queued source when it can be swapped without a click. Returns true while a
    /// swap is still waiting for the fade-out.
    fn service_source_queue(&mut self) -> bool {
        let Some(queue) = self.sources.as_mut() else {
            return false;
        };
        if queue.is_empty() {
            return false;
        }
        if self.source.is_some() && self.env_pos > 0 {
            return true;
        }
        let Some(retired) = self.retired.as_mut() else {
            return false;
        };
        if self.source.is_some() && retired.is_full() {
            // The control side is not collecting; keep the current source rather than
            // dropping (deallocating) the old one here.
            return true;
        }
        if let Ok(next) = queue.pop()
            && let Some(old) = self.source.replace(next)
        {
            // Room was checked above; on failure the box would be dropped here, which is
            // still correct, only not allocation-free.
            let _ = retired.push(old);
        }
        false
    }

    /// Renders one block of `stamp.frames` frames into a buffer with `channels` channels
    /// through `write(frame, channel, value)`. Channels at or above the stream's output
    /// channel count (devices may have more) receive zeros.
    #[inline]
    pub(crate) fn render(
        &mut self,
        stamp: OutputStamp,
        channels: usize,
        mut write: impl FnMut(usize, usize, f32),
    ) {
        let swap_waiting = self.service_source_queue() | self.service_route_queue();
        let sh = &*self.shared;
        let audible = sh.run.load(Ordering::Acquire)
            && !sh.shutdown.load(Ordering::Acquire)
            && self.source.is_some()
            && !swap_waiting;
        let env_target = if audible { self.fade_frames } else { 0 };
        let gain_target = Gain::from_bits(sh.gain_bits.load(Ordering::Acquire))
            .linear()
            .clamp(0.0, 1.0);
        if self.env_pos == 0 {
            // Nothing audible: jump to the new gain, the fade-in provides the ramp.
            self.gain = gain_target;
        }
        let max = self
            .max
            .min(f32::from_bits(sh.limit_bits.load(Ordering::Acquire)));
        let inv_fade = 1.0 / self.fade_frames as f32;
        let history_routed = self.is_routed(self.history_channel);

        let frames = stamp.frames as usize;
        let mut limited: u32 = 0;
        let mut done = 0usize;
        while done < frames {
            let n = (frames - done).min(CHUNK);
            let buf = &mut self.scratch[..n];
            if self.env_pos == 0 && env_target == 0 {
                buf.fill(0.0);
            } else {
                match self.source.as_mut() {
                    Some(src) => src.fill(buf),
                    None => buf.fill(0.0),
                }
                for v in buf.iter_mut() {
                    if self.env_pos < env_target {
                        self.env_pos += 1;
                    } else if self.env_pos > env_target {
                        self.env_pos -= 1;
                    }
                    self.gain = step_toward(self.gain, gain_target, self.gain_step);
                    let x = *v * (self.env_pos as f32 * inv_fade) * self.gain;
                    *v = if !x.is_finite() {
                        limited = limited.saturating_add(1);
                        0.0
                    } else if x.abs() > max {
                        limited = limited.saturating_add(1);
                        x.clamp(-max, max)
                    } else {
                        x
                    };
                }
            }
            let buf = &self.scratch[..n];
            if let Some(h) = self.history.as_mut() {
                let start = stamp.start_sample + done as u64;
                h.write(
                    start,
                    if history_routed {
                        buf
                    } else {
                        &self.zeros[..n]
                    },
                );
            }
            for (f, &v) in buf.iter().enumerate() {
                for c in 0..channels {
                    let routed = c < self.output_channels && self.routes.contains(c);
                    write(done + f, c, if routed { v } else { 0.0 });
                }
            }
            done += n;
        }

        let state = match (self.env_pos, env_target) {
            (0, 0) => OutputState::Silent,
            (e, t) if e == t => OutputState::Active,
            (_, t) if t > 0 => OutputState::FadingIn,
            _ => OutputState::FadingOut,
        };
        if self.env_pos > 0 {
            sh.emitted.store(true, Ordering::Release);
        }
        sh.state.store(state.to_u8(), Ordering::Release);
        sh.frames_rendered
            .fetch_add(stamp.frames.into(), Ordering::Relaxed);
        if limited > 0 {
            sh.limited_samples
                .fetch_add(limited.into(), Ordering::Relaxed);
        }
        let tick = OutputTick {
            start_sample: stamp.start_sample,
            frames: stamp.frames,
            flags: stamp.flags,
            callback_ns: stamp.callback_ns,
            playback_ns: stamp.playback_ns,
            limited_samples: limited,
            state,
        };
        if self.ticks.push(tick).is_err() {
            sh.ticks_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// One linear ramp step; snaps to the target when within a step so float residue never
/// leaves the ramp hanging just short of it.
#[inline]
fn step_toward(x: f32, target: f32, step: f32) -> f32 {
    if (target - x).abs() <= step {
        target
    } else if x < target {
        x + step
    } else {
        x - step
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::{Sine, generator};

    const RATE: u32 = 48_000;
    const FADE: usize = 960;

    fn stamp(start: u64, frames: u32) -> OutputStamp {
        OutputStamp {
            start_sample: start,
            frames,
            flags: BlockFlags::NONE,
            callback_ns: 0,
            playback_ns: None,
        }
    }

    fn renderer(source: OutputSource, max_db: f64) -> (OutputRenderer, RendererParts) {
        OutputRenderer::new(RendererConfig {
            source,
            output_channels: 3,
            sample_rate: RATE,
            max_level: MaxLevel::from_peak_db(max_db).expect("level"),
            history: Some((1, 8192)),
        })
    }

    /// Renders `frames` frames of a 3-channel buffer starting at `start`.
    fn run(r: &mut OutputRenderer, start: u64, frames: usize) -> Vec<[f32; 3]> {
        let mut out = vec![[f32::NAN; 3]; frames];
        r.render(stamp(start, frames as u32), 3, |f, c, v| out[f][c] = v);
        out
    }

    fn peak(xs: impl Iterator<Item = f32>) -> f32 {
        xs.fold(0.0, |m, v| m.max(v.abs()))
    }

    #[test]
    fn silence_writes_only_zeros_and_ticks() {
        let (mut r, mut parts) = renderer(OutputSource::Silence, -20.0);
        let out = run(&mut r, 0, 3000);
        assert!(out.iter().flatten().all(|&v| v == 0.0));
        let t = parts.ticks.pop().expect("tick");
        assert_eq!(
            (t.start_sample, t.frames, t.state),
            (0, 3000, OutputState::Silent)
        );
        assert!(!parts.shared.has_emitted());
        let mut h = [1.0; 3000];
        parts
            .history
            .as_ref()
            .expect("history")
            .read(0, &mut h)
            .expect("read");
        assert!(h.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn generator_fades_in_routes_records_and_fades_out_on_stop() {
        let (mut handle, port) = generator([1, 2]).expect("routes");
        handle
            .set_source(Box::new(Sine::new(1000.0, RATE, 0.05)))
            .expect("queue");
        let (mut r, parts) = renderer(OutputSource::Generator(port), -20.0);
        // Not started: silent even with a source.
        assert!(run(&mut r, 0, 256).iter().flatten().all(|&v| v == 0.0));
        handle.start();
        let out = run(&mut r, 256, 4800);
        assert_eq!(handle.state(), OutputState::Active);
        assert!(out.iter().all(|f| f[0] == 0.0), "channel 0 is not routed");
        assert!(
            out.iter().all(|f| f[1] == f[2]),
            "routed channels carry one signal"
        );
        // Linear ramp: the first millisecond stays far below the target level.
        assert!(peak(out[..48].iter().map(|f| f[1])) < 0.05 * 0.06);
        let settled = peak(out[FADE..].iter().map(|f| f[1]));
        assert!((settled - 0.05).abs() < 0.05 * 0.01, "{settled}");
        // History holds exactly what channel 1 got, at output indices.
        let mut h = vec![0.0; 4800];
        parts
            .history
            .as_ref()
            .expect("history")
            .read(256, &mut h)
            .expect("read");
        assert!(h.iter().zip(&out).all(|(a, f)| *a == f[1]));

        handle.stop();
        let tail = run(&mut r, 5056, FADE + 100);
        assert_eq!(handle.state(), OutputState::Silent);
        assert!(
            peak(tail[..48].iter().map(|f| f[1])) > 0.05 * 0.8,
            "no hard cut"
        );
        assert!(tail[FADE..].iter().flatten().all(|&v| v == 0.0));
        assert_eq!(parts.shared.stats().limited_samples, 0);
    }

    #[test]
    fn shutdown_fades_out_within_fade_time_and_is_final() {
        let (mut handle, port) = generator([0]).expect("routes");
        handle
            .set_source(Box::new(Sine::new(440.0, RATE, 0.05)))
            .expect("queue");
        handle.start();
        let (mut r, parts) = renderer(OutputSource::Generator(port), -6.0);
        run(&mut r, 0, 2000);
        parts.shared.begin_shutdown();
        let tail = run(&mut r, 2000, FADE);
        assert_eq!(parts.shared.state(), OutputState::Silent);
        assert_eq!(tail[FADE - 1][0], 0.0);
        handle.start();
        assert!(run(&mut r, 2960, 2000).iter().flatten().all(|&v| v == 0.0));
    }

    #[test]
    fn max_level_limits_and_counts() {
        let (mut handle, port) = generator([0]).expect("routes");
        handle
            .set_source(Box::new(Sine::new(1000.0, RATE, 1.0)))
            .expect("queue");
        handle.start();
        let (mut r, mut parts) = renderer(OutputSource::Generator(port), -20.0);
        let out = run(&mut r, 0, 4800);
        let cap = MaxLevel::from_peak_db(-20.0).expect("level").linear();
        assert!(out.iter().all(|f| f[0].abs() <= cap));
        let limited = parts.shared.stats().limited_samples;
        assert!(limited > 1000, "{limited}");
        assert_eq!(
            parts.ticks.pop().map(|t| u64::from(t.limited_samples)),
            Ok(limited)
        );
    }

    #[test]
    fn a_limit_set_while_running_tightens_but_never_loosens() {
        let (mut handle, port) = generator([0]).expect("routes");
        handle
            .set_source(Box::new(Sine::new(1000.0, RATE, 1.0)))
            .expect("queue");
        handle.start();
        let (mut r, _parts) = renderer(OutputSource::Generator(port), -20.0);
        let open = MaxLevel::from_peak_db(-20.0).expect("level").linear();
        run(&mut r, 0, 4800);
        handle.set_max_level(MaxLevel::from_peak_db(-30.0).expect("level"));
        let tight = MaxLevel::from_peak_db(-30.0).expect("level").linear();
        let out = run(&mut r, 4800, 4800);
        assert!(out.iter().all(|f| f[0].abs() <= tight));
        assert!(peak(out.iter().map(|f| f[0])) > tight * 0.99);
        // Above the limit the stream was opened with: that one still holds.
        handle.set_max_level(MaxLevel::from_peak_db(0.0).expect("level"));
        let out = run(&mut r, 9600, 4800);
        assert!(out.iter().all(|f| f[0].abs() <= open));
        assert!(peak(out.iter().map(|f| f[0])) > open * 0.99);
    }

    #[test]
    fn non_finite_source_output_becomes_zero_and_is_counted() {
        struct Bad;
        impl SignalSource for Bad {
            fn fill(&mut self, out: &mut [f32]) {
                out.fill(f32::NAN);
            }
        }
        let (mut handle, port) = generator([0]).expect("routes");
        handle.set_source(Box::new(Bad)).expect("queue");
        handle.start();
        let (mut r, parts) = renderer(OutputSource::Generator(port), -20.0);
        let out = run(&mut r, 0, 100);
        assert!(out.iter().flatten().all(|&v| v == 0.0));
        assert_eq!(parts.shared.stats().limited_samples, 100);
    }

    #[test]
    fn gain_ramps_and_source_swap_fades_through_silence() {
        let (mut handle, port) = generator([0]).expect("routes");
        handle
            .set_source(Box::new(Sine::new(1000.0, RATE, 0.05)))
            .expect("queue");
        handle.start();
        let (mut r, _parts) = renderer(OutputSource::Generator(port), -6.0);
        run(&mut r, 0, 2000);
        handle.set_gain(Gain::from_db(-6.0).expect("gain"));
        let out = run(&mut r, 2000, 2000);
        let late = peak(out[FADE..].iter().map(|f| f[0]));
        assert!((late - 0.05 * 0.501).abs() < 0.001, "{late}");

        handle
            .set_source(Box::new(Sine::new(250.0, RATE, 0.05)))
            .expect("queue");
        // Swap: fade out the old source...
        let a = run(&mut r, 4000, FADE);
        assert_eq!(a[FADE - 1][0], 0.0);
        // ...then the new one fades in at the next block.
        let b = run(&mut r, 4960, 2000);
        assert!(peak(b[..48].iter().map(|f| f[0])) < 0.01);
        assert!(peak(b[FADE..].iter().map(|f| f[0])) > 0.02);
        assert_eq!(handle.collect_retired(), 1);
    }

    #[test]
    fn routing_changes_without_reopening_and_fades_through_silence() {
        let (mut handle, port) = generator([0]).expect("routes");
        handle
            .set_source(Box::new(Sine::new(1000.0, RATE, 0.05)))
            .expect("queue");
        let (mut r, parts) = renderer(OutputSource::Generator(port), -6.0);
        // Silent: a routing change applies at once.
        handle.set_routes(&[2]).expect("routes");
        assert!(run(&mut r, 0, 256).iter().flatten().all(|&v| v == 0.0));
        handle.start();
        let out = run(&mut r, 256, 4800);
        assert!(out.iter().all(|f| f[0] == 0.0 && f[1] == 0.0));
        assert!(peak(out[FADE..].iter().map(|f| f[2])) > 0.04, "plays on 2");

        // Audible: fade out on the old channel, then fade in on the new one.
        handle.set_routes(&[0, 1]).expect("routes");
        let a = run(&mut r, 5056, FADE);
        assert!(a.iter().all(|f| f[0] == 0.0 && f[1] == 0.0));
        assert!(peak(a[..48].iter().map(|f| f[2])) > 0.04, "no hard cut");
        assert_eq!(a[FADE - 1][2], 0.0);
        let b = run(&mut r, 5056 + FADE as u64, 4800);
        assert!(b.iter().all(|f| f[2] == 0.0), "2 no longer routed");
        assert!(peak(b[..48].iter().map(|f| f[0])) < 0.01, "fades in");
        assert!(peak(b[FADE..].iter().map(|f| f[0])) > 0.04);
        assert!(b.iter().all(|f| f[0] == f[1]));
        // The history follows its channel's routing (channel 1 now carries the signal).
        let mut h = vec![0.0; 4800];
        parts
            .history
            .as_ref()
            .expect("history")
            .read(5056 + FADE as u64, &mut h)
            .expect("read");
        assert!(h.iter().zip(&b).all(|(a, f)| *a == f[1]));
        assert_eq!(parts.shared.stats().limited_samples, 0);
    }

    #[test]
    fn extra_device_channels_get_zeros_and_large_blocks_render_in_chunks() {
        let (mut handle, port) = generator([0]).expect("routes");
        handle
            .set_source(Box::new(Sine::new(1000.0, RATE, 0.05)))
            .expect("queue");
        handle.start();
        let (mut r, _parts) = renderer(OutputSource::Generator(port), -6.0);
        let frames = CHUNK * 3 + 17;
        let mut out = vec![[f32::NAN; 5]; frames];
        r.render(stamp(0, frames as u32), 5, |f, c, v| out[f][c] = v);
        assert!(out.iter().all(|f| f[1..].iter().all(|&v| v == 0.0)));
        assert!(out.iter().all(|f| f[0].is_finite()));
    }
}
