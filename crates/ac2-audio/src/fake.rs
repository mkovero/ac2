//! A simulated duplex device for hardware-free tests.
//!
//! The fake runs the same plumbing as the real backends (transport, frame-counter clock,
//! event latch, output renderer, generator history); only the "device" is simulated. One
//! simulated callback renders the output block, hands it to a simulated DAC, simulates the
//! analog world for one block, and captures it through a simulated ADC:
//!
//! ```text
//! renderer ──► output faults (drop / repeat frames) ──► DAC ring (per output channel)
//!                                                           │  clock drift (resampling)
//!                                                           ▼
//! ADC input i = base content + noise + Σ paths (delay, FIR, noise) from DAC channels
//! ```
//!
//! Time is simulated: header and tick timestamps are device time derived from the frame
//! count, so results are bit-for-bit deterministic for a given [`FakeConfig::seed`]. In
//! [`FakeDrive::Manual`] a test advances the device with [`FakeDriver::step`]; nothing
//! depends on wall-clock time.
//!
//! # Index relations
//!
//! The fake is a single-callback device: the output index of a block equals its capture
//! index. Without faults, DAC frame `d` is output frame `d`, and a path with delay `D`
//! makes capture index `n` hear output index `n − D`. Dropping `k` output frames shifts the
//! loopback offset by −k, repeating `k` frames by +k. A drift of `ppm` makes capture index
//! `n` hear DAC position `n · (1 + ppm·1e-6) − D` (positive = DAC clock fast), so the
//! offset changes by `−ppm·1e-6` samples per sample.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::backend::{
    Backend, BackendKind, ClockRelation, DeviceCaps, DeviceId, DeviceSelector, Direction,
    DirectionCaps, DuplexRequest, FrameRange, IndexExactness, Negotiated, RateRange, SampleFormat,
    StaticLatency,
};
use crate::block::{BlockProducer, BlockStamp};
use crate::clock::FrameCounterClock;
use crate::error::{AudioError, Operation, Unsupported};
use crate::events::{BackendEvents, EventLatch};
use crate::output::{OutputRenderer, OutputStamp};
use crate::rng::Rng;
use crate::stream::{DuplexStream, Plumbing, StreamParts};

/// Device id the fake lists.
pub const FAKE_DEVICE_ID: &str = "fake";

/// How the simulated callback is driven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeDrive {
    /// A test calls [`FakeDriver::step`]. [`Backend::open`] parks the driver for
    /// [`FakeBackend::take_driver`]; [`FakeBackend::open_manual`] returns it directly.
    Manual,
    /// A background thread steps the device.
    Thread(Pace),
}

/// Speed of a [`FakeDrive::Thread`] device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// One block per block duration of wall-clock time.
    Realtime,
    /// As fast as possible (stresses the transport; the consumer will overflow).
    Unpaced,
}

/// Content of the device inputs before loopback paths and noise are added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputContent {
    /// Zeros.
    Silence,
    /// [`signature`] values, unique per (device channel, sample index), for routing tests.
    /// Not audio-like.
    Signature,
}

/// Deterministic per-channel content of [`InputContent::Signature`].
pub fn signature(device_channel: u16, sample: u64) -> f32 {
    (f32::from(device_channel) + 1.0) * 1000.0 + (sample % 1000) as f32
}

/// An analog path from a device output to a device input: delay, then a short FIR, plus
/// independent Gaussian noise. A cable loopback is `fir = [1.0]`, noise 0; an acoustic
/// measurement path is a room/speaker impulse response plus ambient noise.
#[derive(Clone, Debug, PartialEq)]
pub struct FakePath {
    /// Device output channel.
    pub output: u16,
    /// Device input channel.
    pub input: u16,
    /// Whole-sample delay, output→input.
    pub delay_frames: u32,
    /// Impulse response applied after the delay.
    pub fir: Vec<f32>,
    /// RMS of noise added to the input by this path.
    pub noise_rms: f32,
}

impl FakePath {
    /// A clean cable: delay only.
    pub fn loopback(output: u16, input: u16, delay_frames: u32) -> Self {
        Self {
            output,
            input,
            delay_frames,
            fir: vec![1.0],
            noise_rms: 0.0,
        }
    }

    /// An acoustic path: delay, impulse response `fir`, additive noise.
    pub fn acoustic(
        output: u16,
        input: u16,
        delay_frames: u32,
        fir: Vec<f32>,
        noise_rms: f32,
    ) -> Self {
        Self {
            output,
            input,
            delay_frames,
            fir,
            noise_rms,
        }
    }
}

/// A scripted fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeFault {
    /// Output frames `at_output_sample .. +frames` never reach the DAC (a silent render
    /// glitch). No flag is raised: only the loopback can see it.
    DropOutputFrames {
        /// First output index dropped.
        at_output_sample: u64,
        /// Frames dropped.
        frames: u32,
    },
    /// Output frames `at_output_sample .. +frames` are played twice. No flag is raised.
    RepeatOutputFrames {
        /// First output index repeated.
        at_output_sample: u64,
        /// Frames repeated.
        frames: u32,
    },
    /// The host reports an xrun before block `at_block`, but the frame counter stays
    /// contiguous (a late JACK cycle).
    XrunFlag {
        /// Callback number (0-based).
        at_block: u64,
    },
    /// The device loses `lost_frames` frames before block `at_block`: the counter jumps, the
    /// DAC plays silence and the captured frames are discarded; an xrun is reported.
    Xrun {
        /// Callback number (0-based).
        at_block: u64,
        /// Frames lost.
        lost_frames: u32,
    },
    /// The host reports a configuration change before block `at_block`.
    ConfigChange {
        /// Callback number (0-based).
        at_block: u64,
    },
}

/// Fake device settings.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeConfig {
    /// Rate, Hz. The device runs only at this rate.
    pub sample_rate: u32,
    /// Frames per callback. The device runs only at this size.
    pub block_frames: u32,
    /// Device input channels.
    pub inputs: u16,
    /// Device output channels.
    pub outputs: u16,
    /// Who steps the device.
    pub drive: FakeDrive,
    /// Seed for all noise.
    pub seed: u64,
    /// Base input content.
    pub input_content: InputContent,
    /// Gaussian noise RMS on every input.
    pub input_noise_rms: f32,
    /// Analog paths from outputs to inputs.
    pub paths: Vec<FakePath>,
    /// DAC clock error relative to the ADC, ppm; positive = DAC fast.
    pub drift_ppm: f64,
    /// Scripted faults.
    pub faults: Vec<FakeFault>,
    /// Raw frame counter of the first block (to exercise the 32-bit wrap).
    pub counter_origin: u32,
    /// A [`FakeDrive::Thread`] device stops producing after this many blocks.
    pub stop_after_blocks: Option<u64>,
    /// How far ahead DAC history is kept for drift, in seconds of run time.
    pub drift_horizon_seconds: f64,
}

impl Default for FakeConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            block_frames: 256,
            inputs: 4,
            outputs: 2,
            drive: FakeDrive::Manual,
            seed: 1,
            input_content: InputContent::Silence,
            input_noise_rms: 0.0,
            paths: Vec::new(),
            drift_ppm: 0.0,
            faults: Vec::new(),
            counter_origin: 0,
            stop_after_blocks: None,
            drift_horizon_seconds: 1800.0,
        }
    }
}

/// A [`FakeConfig`] that cannot be simulated.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum FakeConfigError {
    /// Rate, block size or input count is zero.
    #[error("{0} must be non-zero")]
    Zero(&'static str),
    /// A path names a channel the device does not have.
    #[error("path {path}: {direction:?} channel {channel} does not exist")]
    PathChannel {
        /// Index into `paths`.
        path: usize,
        /// Which end.
        direction: Direction,
        /// The channel.
        channel: u16,
    },
    /// A path has an empty impulse response.
    #[error("path {0}: empty FIR")]
    EmptyFir(usize),
    /// A noise level, drift or horizon is not finite, or a noise level is negative.
    #[error("{0} must be finite and non-negative (drift: finite, |ppm| < 10^5)")]
    BadNumber(&'static str),
    /// An [`FakeFault::Xrun`] before block 0.
    #[error("an xrun with lost frames cannot precede the first block")]
    XrunAtFirstBlock,
    /// Two output faults overlap, or one has zero frames.
    #[error("output faults overlap or are empty")]
    OutputFaults,
}

impl FakeConfig {
    fn validate(&self) -> Result<(), FakeConfigError> {
        if self.sample_rate == 0 {
            return Err(FakeConfigError::Zero("sample_rate"));
        }
        if self.block_frames == 0 {
            return Err(FakeConfigError::Zero("block_frames"));
        }
        if self.inputs == 0 {
            return Err(FakeConfigError::Zero("inputs"));
        }
        let ok = |x: f64| x.is_finite() && x >= 0.0;
        if !ok(f64::from(self.input_noise_rms)) {
            return Err(FakeConfigError::BadNumber("input_noise_rms"));
        }
        if !(self.drift_ppm.is_finite() && self.drift_ppm.abs() < 1e5) {
            return Err(FakeConfigError::BadNumber("drift_ppm"));
        }
        if !ok(self.drift_horizon_seconds) {
            return Err(FakeConfigError::BadNumber("drift_horizon_seconds"));
        }
        for (i, p) in self.paths.iter().enumerate() {
            if p.output >= self.outputs {
                return Err(FakeConfigError::PathChannel {
                    path: i,
                    direction: Direction::Output,
                    channel: p.output,
                });
            }
            if p.input >= self.inputs {
                return Err(FakeConfigError::PathChannel {
                    path: i,
                    direction: Direction::Input,
                    channel: p.input,
                });
            }
            if p.fir.is_empty() {
                return Err(FakeConfigError::EmptyFir(i));
            }
            if !ok(f64::from(p.noise_rms)) || p.fir.iter().any(|c| !c.is_finite()) {
                return Err(FakeConfigError::BadNumber("path noise_rms / fir"));
            }
        }
        if self
            .faults
            .iter()
            .any(|f| matches!(f, FakeFault::Xrun { at_block: 0, .. }))
        {
            // Frames lost before the first block would be invisible: the first block is
            // index 0 by definition.
            return Err(FakeConfigError::XrunAtFirstBlock);
        }
        let spans = output_faults(&self.faults);
        if spans.iter().any(|s| s.frames == 0)
            || spans
                .windows(2)
                .any(|w| w[0].at + u64::from(w[0].frames) > w[1].at)
        {
            return Err(FakeConfigError::OutputFaults);
        }
        Ok(())
    }

    fn ns(&self, frames: u64) -> u64 {
        (u128::from(frames) * 1_000_000_000 / u128::from(self.sample_rate)) as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OutputFault {
    at: u64,
    frames: u32,
    repeat: bool,
}

fn output_faults(faults: &[FakeFault]) -> Vec<OutputFault> {
    let mut v: Vec<OutputFault> = faults
        .iter()
        .filter_map(|f| match *f {
            FakeFault::DropOutputFrames {
                at_output_sample,
                frames,
            } => Some(OutputFault {
                at: at_output_sample,
                frames,
                repeat: false,
            }),
            FakeFault::RepeatOutputFrames {
                at_output_sample,
                frames,
            } => Some(OutputFault {
                at: at_output_sample,
                frames,
                repeat: true,
            }),
            _ => None,
        })
        .collect();
    v.sort_by_key(|f| f.at);
    v
}

/// The simulated device. Cloning shares the parked manual driver slot.
#[derive(Debug, Clone)]
pub struct FakeBackend {
    config: FakeConfig,
    parked: Arc<Mutex<Option<FakeDriver>>>,
}

impl FakeBackend {
    /// A fake device with `config`.
    pub fn new(config: FakeConfig) -> Result<Self, FakeConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            parked: Arc::new(Mutex::new(None)),
        })
    }

    /// The device settings.
    pub fn config(&self) -> &FakeConfig {
        &self.config
    }

    /// The driver of the most recent [`Backend::open`] in [`FakeDrive::Manual`] mode.
    pub fn take_driver(&self) -> Option<FakeDriver> {
        self.parked.lock().ok().and_then(|mut slot| slot.take())
    }

    /// Opens a stream and returns its driver, regardless of [`FakeConfig::drive`].
    pub fn open_manual(
        &self,
        mut request: DuplexRequest,
    ) -> Result<(DuplexStream, FakeDriver), AudioError> {
        request.validate()?;
        let c = &self.config;
        for (selector, direction) in [
            (&request.input_device, Direction::Input),
            (&request.output_device, Direction::Output),
        ] {
            if let DeviceSelector::Id(id) = selector
                && id.0 != FAKE_DEVICE_ID
            {
                return Err(AudioError::DeviceNotFound {
                    direction,
                    selector: selector.to_string(),
                });
            }
        }
        if let Some(r) = request.sample_rate.filter(|&r| r != c.sample_rate) {
            return Err(Unsupported::SampleRate {
                requested: r,
                offered: format!("{} Hz only", c.sample_rate),
            }
            .into());
        }
        if let Some(b) = request.buffer_frames.filter(|&b| b != c.block_frames) {
            return Err(Unsupported::BufferFrames {
                requested: b,
                fixed: Some(c.block_frames),
            }
            .into());
        }
        if let Some(&ch) = request.input_map.iter().find(|&&ch| ch >= c.inputs) {
            return Err(Unsupported::InputChannel {
                channel: ch,
                available: c.inputs,
            }
            .into());
        }
        if request.output_channels > c.outputs {
            return Err(Unsupported::OutputChannels {
                requested: request.output_channels,
                available: c.outputs,
            }
            .into());
        }

        let plumbing = Plumbing::new(&mut request, c.sample_rate, c.block_frames as usize);
        let negotiated = Negotiated {
            backend: BackendKind::Fake,
            input_device: DeviceId(FAKE_DEVICE_ID.into()),
            output_device: DeviceId(FAKE_DEVICE_ID.into()),
            sample_rate: c.sample_rate,
            input_channels: request.input_map.len() as u16,
            device_input_channels: c.inputs,
            output_channels: request.output_channels,
            device_output_channels: request.output_channels,
            buffer_frames: Some(c.block_frames),
            input_format: SampleFormat::F32,
            output_format: Some(SampleFormat::F32),
            clock: ClockRelation::SingleCallback,
            index: IndexExactness::Exact,
            latency: StaticLatency::Unknown,
        };
        let sim = Sim::new(
            c.clone(),
            &request,
            plumbing.producer,
            plumbing.renderer,
            Arc::clone(&plumbing.events),
        );
        let stream = DuplexStream::new(StreamParts {
            negotiated,
            capture: plumbing.consumer,
            output: plumbing.output,
            events: plumbing.events,
            // Simulated: nothing to drain in wall-clock time.
            drain: Duration::ZERO,
            guard: Box::new(()),
        });
        Ok((stream, FakeDriver { sim: Box::new(sim) }))
    }
}

impl Backend for FakeBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Fake
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let c = &self.config;
        let dir = |channels: u16| DirectionCaps {
            max_channels: channels,
            rates: vec![RateRange {
                min: c.sample_rate,
                max: c.sample_rate,
            }],
            buffer_frames: Some(FrameRange {
                min: c.block_frames,
                max: c.block_frames,
            }),
            formats: vec![SampleFormat::F32],
            default_rate: Some(c.sample_rate),
        };
        Ok(vec![DeviceCaps {
            backend: BackendKind::Fake,
            host: "fake".into(),
            id: DeviceId(FAKE_DEVICE_ID.into()),
            name: "simulated duplex device".into(),
            input: Some(dir(c.inputs)),
            output: (c.outputs > 0).then(|| dir(c.outputs)),
            duplex_clock: ClockRelation::SingleCallback,
            index: IndexExactness::Exact,
            latency: StaticLatency::Unknown,
            notes: Vec::new(),
        }])
    }

    fn open(&self, request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        let (mut stream, driver) = self.open_manual(request)?;
        match self.config.drive {
            FakeDrive::Manual => {
                let mut slot = self.parked.lock().map_err(|_| AudioError::Backend {
                    backend: BackendKind::Fake,
                    operation: Operation::Open,
                    detail: "driver slot poisoned".into(),
                })?;
                *slot = Some(driver);
            }
            FakeDrive::Thread(pace) => {
                let guard =
                    spawn_driver(driver, pace, &self.config).map_err(|e| AudioError::Backend {
                        backend: BackendKind::Fake,
                        operation: Operation::Start,
                        detail: e.to_string(),
                    })?;
                let drain =
                    Duration::from_nanos(self.config.ns(2 * u64::from(self.config.block_frames)));
                stream.set_guard(Box::new(guard), drain);
            }
        }
        Ok(stream)
    }
}

struct ThreadGuard {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ThreadGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            // A panicked driver thread has nothing left to clean up.
            let _ = t.join();
        }
    }
}

fn spawn_driver(
    mut driver: FakeDriver,
    pace: Pace,
    c: &FakeConfig,
) -> std::io::Result<ThreadGuard> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let limit = c.stop_after_blocks;
    let block = Duration::from_nanos(c.ns(u64::from(c.block_frames)));
    let thread = std::thread::Builder::new()
        .name("ac2-fake-audio".into())
        .spawn(move || {
            let t0 = Instant::now();
            let mut due = Duration::ZERO;
            while !stop_t.load(Ordering::Acquire) {
                if limit.is_some_and(|l| driver.blocks() >= l) {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                driver.step();
                if pace == Pace::Realtime {
                    due += block;
                    if let Some(wait) = due.checked_sub(t0.elapsed()) {
                        std::thread::sleep(wait);
                    }
                }
            }
        })?;
    Ok(ThreadGuard {
        stop,
        thread: Some(thread),
    })
}

/// Counters of the simulated device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FakeStats {
    /// Callbacks run.
    pub blocks: u64,
    /// Analog frames simulated (including frames lost to xruns).
    pub device_frames: u64,
    /// Frames the DAC received.
    pub dac_frames: u64,
    /// Output frames dropped by faults.
    pub output_frames_dropped: u64,
    /// Output frames played twice by faults.
    pub output_frames_repeated: u64,
    /// Reads of DAC samples that were not (yet / any more) available and read as zero. Non
    /// zero means a path delay is too short for the scripted drops or drift.
    pub dac_underreads: u64,
}

/// Steps a fake device; the stand-in for the audio callback.
#[derive(Debug)]
pub struct FakeDriver {
    sim: Box<Sim>,
}

impl FakeDriver {
    /// Runs one callback.
    pub fn step(&mut self) {
        self.sim.step();
    }

    /// Runs `blocks` callbacks.
    pub fn run_blocks(&mut self, blocks: u64) {
        for _ in 0..blocks {
            self.sim.step();
        }
    }

    /// Runs enough callbacks to cover `seconds` of device time.
    pub fn run_seconds(&mut self, seconds: f64) {
        let frames = (seconds * f64::from(self.sim.cfg.sample_rate))
            .ceil()
            .max(0.0) as u64;
        self.run_blocks(frames.div_ceil(u64::from(self.sim.cfg.block_frames)));
    }

    /// Callbacks run so far.
    pub fn blocks(&self) -> u64 {
        self.sim.stats.blocks
    }

    /// Device counters.
    pub fn stats(&self) -> FakeStats {
        self.sim.stats
    }
}

/// Half-width of the drift interpolator, in samples.
const SINC_HALF: usize = 16;
/// Fractional-delay phases of the drift interpolator. 1024 phases quantise the read
/// position to 1/2048 sample, far below anything a timing monitor resolves.
const SINC_PHASES: usize = 1024;

/// Blackman-windowed sinc table, `SINC_PHASES + 1` rows of `2 * SINC_HALF` taps; row `p`
/// interpolates at fraction `p / SINC_PHASES` between taps `SINC_HALF - 1` and `SINC_HALF`.
fn sinc_table() -> Box<[f32]> {
    let taps = 2 * SINC_HALF;
    let mut t = vec![0.0f32; (SINC_PHASES + 1) * taps];
    for p in 0..=SINC_PHASES {
        let frac = p as f64 / SINC_PHASES as f64;
        let row = &mut t[p * taps..(p + 1) * taps];
        let mut sum = 0.0;
        for (k, v) in row.iter_mut().enumerate() {
            let x = k as f64 - (SINC_HALF as f64 - 1.0) - frac;
            let sinc = if x == 0.0 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            let u = (x + SINC_HALF as f64) / (2.0 * SINC_HALF as f64);
            let w = 0.42 - 0.5 * (std::f64::consts::TAU * u).cos()
                + 0.08 * (2.0 * std::f64::consts::TAU * u).cos();
            *v = (sinc * w) as f32;
            sum += sinc * w;
        }
        // Unity DC gain in every phase, so drift does not modulate the level.
        for v in row.iter_mut() {
            *v = (f64::from(*v) / sum) as f32;
        }
    }
    t.into_boxed_slice()
}

struct PathRt {
    output: usize,
    input: usize,
    delay: f64,
    fir: Box<[f32]>,
    noise_rms: f64,
    rng: Rng,
}

struct Sim {
    cfg: FakeConfig,
    producer: BlockProducer,
    renderer: OutputRenderer,
    clock: FrameCounterClock,
    latch: EventLatch,
    events: Arc<BackendEvents>,
    input_map: Box<[usize]>,
    stream_outputs: usize,
    device_inputs: usize,
    device_outputs: usize,
    raw_counter: u32,
    block_faults: Box<[FakeFault]>,
    output_faults: Box<[OutputFault]>,
    output_fault_cursor: usize,
    /// DAC history, `device_outputs` interleaved channels.
    dac: Box<[f32]>,
    dac_mask: u64,
    out_buf: Box<[f32]>,
    in_buf: Box<[f32]>,
    paths: Box<[PathRt]>,
    input_rngs: Box<[Rng]>,
    drift_ratio: Option<f64>,
    sinc: Box<[f32]>,
    stats: FakeStats,
}

impl std::fmt::Debug for Sim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sim")
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl Sim {
    fn new(
        cfg: FakeConfig,
        req: &DuplexRequest,
        producer: BlockProducer,
        renderer: OutputRenderer,
        events: Arc<BackendEvents>,
    ) -> Self {
        let block = cfg.block_frames as usize;
        let device_inputs = usize::from(cfg.inputs);
        let device_outputs = usize::from(cfg.outputs).max(1);
        let output_faults = output_faults(&cfg.faults);
        let repeats: u64 = output_faults
            .iter()
            .filter(|f| f.repeat)
            .map(|f| u64::from(f.frames))
            .sum();
        let max_delay = cfg.paths.iter().map(|p| p.delay_frames).max().unwrap_or(0);
        let max_fir = cfg.paths.iter().map(|p| p.fir.len()).max().unwrap_or(1);
        let drift_margin =
            (cfg.drift_ppm.abs() * 1e-6 * cfg.drift_horizon_seconds * f64::from(cfg.sample_rate))
                .ceil() as u64;
        let dac_frames = (u64::from(max_delay)
            + max_fir as u64
            + 2 * SINC_HALF as u64
            + 4 * block as u64
            + repeats
            + drift_margin)
            .next_power_of_two();
        let mut seeds = Rng::new(cfg.seed);
        let paths = cfg
            .paths
            .iter()
            .map(|p| PathRt {
                output: usize::from(p.output),
                input: usize::from(p.input),
                delay: f64::from(p.delay_frames),
                fir: p.fir.clone().into_boxed_slice(),
                noise_rms: f64::from(p.noise_rms),
                rng: Rng::new(seeds.next_u64()),
            })
            .collect();
        let input_rngs = (0..device_inputs)
            .map(|_| Rng::new(seeds.next_u64()))
            .collect();
        let block_faults = cfg
            .faults
            .iter()
            .copied()
            .filter(|f| {
                !matches!(
                    f,
                    FakeFault::DropOutputFrames { .. } | FakeFault::RepeatOutputFrames { .. }
                )
            })
            .collect();
        let drift_ratio = (cfg.drift_ppm != 0.0).then_some(1.0 + cfg.drift_ppm * 1e-6);
        let latch = EventLatch::new(Arc::clone(&events));
        Self {
            producer,
            renderer,
            clock: FrameCounterClock::new(),
            latch,
            events,
            input_map: req.input_map.iter().map(|&c| usize::from(c)).collect(),
            stream_outputs: usize::from(req.output_channels),
            device_inputs,
            device_outputs,
            raw_counter: cfg.counter_origin,
            block_faults,
            output_faults: output_faults.into_boxed_slice(),
            output_fault_cursor: 0,
            dac: vec![0.0; dac_frames as usize * device_outputs].into_boxed_slice(),
            dac_mask: dac_frames - 1,
            out_buf: vec![0.0; block * usize::from(req.output_channels).max(1)].into_boxed_slice(),
            in_buf: vec![0.0; block * device_inputs].into_boxed_slice(),
            paths,
            input_rngs,
            drift_ratio,
            sinc: if cfg.drift_ppm != 0.0 {
                sinc_table()
            } else {
                Box::new([])
            },
            stats: FakeStats::default(),
            cfg,
        }
    }

    /// One callback. Allocation-free.
    fn step(&mut self) {
        let block_no = self.stats.blocks;
        let frames = self.cfg.block_frames;
        for i in 0..self.block_faults.len() {
            match self.block_faults[i] {
                FakeFault::XrunFlag { at_block } if at_block == block_no => self.events.xrun(),
                FakeFault::ConfigChange { at_block } if at_block == block_no => {
                    self.events.config_change();
                }
                FakeFault::Xrun {
                    at_block,
                    lost_frames,
                } if at_block == block_no => {
                    self.events.xrun();
                    self.lose_frames(lost_frames);
                }
                _ => {}
            }
        }

        let (start, mut flags) = self.clock.advance(self.raw_counter, frames);
        debug_assert_eq!(
            start, self.stats.device_frames,
            "output index is device time"
        );
        flags |= self.latch.take();
        let capture_ns = self.cfg.ns(start);
        let callback_ns = self.cfg.ns(start + u64::from(frames));
        let playback_ns = callback_ns + self.cfg.ns(u64::from(frames));

        let out_ch = self.stream_outputs;
        let out_buf = &mut self.out_buf;
        self.renderer.render(
            OutputStamp {
                start_sample: start,
                frames,
                flags,
                callback_ns,
                playback_ns: Some(playback_ns),
            },
            out_ch,
            |f, c, v| out_buf[f * out_ch + c] = v,
        );
        for f in 0..frames as usize {
            self.dac_push(start + f as u64, Some(f));
        }
        self.capture(start, frames as usize, true);

        let (map, inputs, buf) = (&self.input_map, self.device_inputs, &self.in_buf);
        self.producer.push_with(
            BlockStamp {
                start_sample: start,
                frames,
                flags,
                callback_ns,
                capture_ns: Some(capture_ns),
            },
            |f, c| buf[f * inputs + map[c]],
        );
        self.raw_counter = self.raw_counter.wrapping_add(frames);
        self.stats.blocks += 1;
    }

    /// Frames that pass while no callback runs: the DAC plays silence, the ADC's samples
    /// are lost, the counter moves on.
    fn lose_frames(&mut self, lost: u32) {
        let start = self.stats.device_frames;
        for i in 0..u64::from(lost) {
            self.dac_push(start + i, None);
            self.capture(start + i, 1, false);
        }
        self.raw_counter = self.raw_counter.wrapping_add(lost);
    }

    /// Hands output frame `index` to the DAC: frame `frame` of `out_buf`, or silence.
    fn dac_push(&mut self, index: u64, frame: Option<usize>) {
        while let Some(f) = self.output_faults.get(self.output_fault_cursor) {
            if f.at + u64::from(f.frames) <= index {
                self.output_fault_cursor += 1;
            } else {
                break;
            }
        }
        let fault = self.output_faults.get(self.output_fault_cursor).copied();
        let inside = fault.is_some_and(|f| index >= f.at);
        if inside && fault.is_some_and(|f| !f.repeat) {
            self.stats.output_frames_dropped += 1;
            return;
        }
        let w = self.stats.dac_frames;
        let base = ((w & self.dac_mask) as usize) * self.device_outputs;
        for c in 0..self.device_outputs {
            self.dac[base + c] = match frame {
                Some(f) if c < self.stream_outputs => self.out_buf[f * self.stream_outputs + c],
                _ => 0.0,
            };
        }
        self.stats.dac_frames += 1;
        if let Some(f) = fault
            && f.repeat
            && index == f.at + u64::from(f.frames) - 1
        {
            // Replay the last `frames` DAC frames once more.
            let k = u64::from(f.frames);
            for _ in 0..k {
                let w = self.stats.dac_frames;
                let dst = ((w & self.dac_mask) as usize) * self.device_outputs;
                let src = (((w - k) & self.dac_mask) as usize) * self.device_outputs;
                for c in 0..self.device_outputs {
                    self.dac[dst + c] = self.dac[src + c];
                }
                self.stats.dac_frames += 1;
            }
            self.stats.output_frames_repeated += k;
        }
    }

    /// One DAC sample at integer position `pos` (zero before the stream started).
    #[inline]
    fn dac_at(&mut self, channel: usize, pos: i64) -> f32 {
        if pos < 0 {
            return 0.0;
        }
        let pos = pos as u64;
        let written = self.stats.dac_frames;
        if pos >= written || written - pos > self.dac_mask + 1 {
            self.stats.dac_underreads += 1;
            return 0.0;
        }
        self.dac[((pos & self.dac_mask) as usize) * self.device_outputs + channel]
    }

    /// DAC signal at fractional position `pos` (band-limited interpolation).
    #[inline]
    fn dac_interp(&mut self, channel: usize, pos: f64) -> f32 {
        let base = pos.floor();
        let mut phase = ((pos - base) * SINC_PHASES as f64).round() as usize;
        let mut i0 = base as i64;
        if phase == SINC_PHASES {
            phase = 0;
            i0 += 1;
        }
        let taps = 2 * SINC_HALF;
        let mut acc = 0.0f32;
        for k in 0..taps {
            let coef = self.sinc[phase * taps + k];
            acc += coef * self.dac_at(channel, i0 - (SINC_HALF as i64 - 1) + k as i64);
        }
        acc
    }

    /// Simulates `frames` ADC frames starting at device index `start`; writes them into
    /// `in_buf` when `keep`.
    fn capture(&mut self, start: u64, frames: usize, keep: bool) {
        let inputs = self.device_inputs;
        for f in 0..frames {
            let n = start + f as u64;
            for i in 0..inputs {
                let base = match self.cfg.input_content {
                    InputContent::Silence => 0.0,
                    InputContent::Signature => signature(i as u16, n),
                };
                let noise = if self.cfg.input_noise_rms > 0.0 {
                    (f64::from(self.cfg.input_noise_rms) * self.input_rngs[i].gaussian()) as f32
                } else {
                    0.0
                };
                if keep {
                    self.in_buf[f * inputs + i] = base + noise;
                }
            }
            for p in 0..self.paths.len() {
                let v = self.path_sample(p, n);
                if keep {
                    let input = self.paths[p].input;
                    self.in_buf[f * inputs + input] += v;
                }
            }
        }
        self.stats.device_frames = self.stats.device_frames.max(start + frames as u64);
    }

    fn path_sample(&mut self, p: usize, n: u64) -> f32 {
        let (output, delay, fir_len, noise_rms) = {
            let path = &self.paths[p];
            (path.output, path.delay, path.fir.len(), path.noise_rms)
        };
        let mut acc = 0.0f32;
        match self.drift_ratio {
            None => {
                let pos = n as i64 - delay as i64;
                for t in 0..fir_len {
                    let coef = self.paths[p].fir[t];
                    acc += coef * self.dac_at(output, pos - t as i64);
                }
            }
            Some(ratio) => {
                let pos = n as f64 * ratio - delay;
                for t in 0..fir_len {
                    let coef = self.paths[p].fir[t];
                    acc += coef * self.dac_interp(output, pos - t as f64);
                }
            }
        }
        if noise_rms > 0.0 {
            acc += (noise_rms * self.paths[p].rng.gaussian()) as f32;
        }
        acc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_validation() {
        let bad = |c: FakeConfig| FakeBackend::new(c).err();
        assert_eq!(
            bad(FakeConfig {
                block_frames: 0,
                ..FakeConfig::default()
            }),
            Some(FakeConfigError::Zero("block_frames"))
        );
        assert!(matches!(
            bad(FakeConfig {
                paths: vec![FakePath::loopback(2, 0, 0)],
                ..FakeConfig::default()
            }),
            Some(FakeConfigError::PathChannel {
                direction: Direction::Output,
                ..
            })
        ));
        assert_eq!(
            bad(FakeConfig {
                faults: vec![
                    FakeFault::DropOutputFrames {
                        at_output_sample: 100,
                        frames: 10
                    },
                    FakeFault::RepeatOutputFrames {
                        at_output_sample: 105,
                        frames: 10
                    },
                ],
                ..FakeConfig::default()
            }),
            Some(FakeConfigError::OutputFaults)
        );
        assert!(
            bad(FakeConfig {
                drift_ppm: f64::NAN,
                ..FakeConfig::default()
            })
            .is_some()
        );
    }

    #[test]
    fn sinc_rows_have_unit_dc_gain_and_integer_phase_is_a_unit_impulse() {
        let t = sinc_table();
        let taps = 2 * SINC_HALF;
        for p in [0, 1, SINC_PHASES / 2, SINC_PHASES] {
            let s: f32 = t[p * taps..(p + 1) * taps].iter().sum();
            assert!((s - 1.0).abs() < 1e-5);
        }
        let row0 = &t[..taps];
        assert!((row0[SINC_HALF - 1] - 1.0).abs() < 1e-6);
        assert!(
            row0.iter()
                .enumerate()
                .all(|(k, v)| k == SINC_HALF - 1 || v.abs() < 1e-6)
        );
    }
}
