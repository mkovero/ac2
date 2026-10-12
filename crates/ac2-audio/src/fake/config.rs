use thiserror::Error;

use crate::backend::Direction;

use super::{FAKE_DEVICE_ID, FAKE_INPUT_ID, FAKE_OUTPUT_ID};

/// How the fake lists its inputs and outputs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FakeEndpoints {
    /// One device with both directions, like a JACK server or a Core Audio interface.
    #[default]
    Duplex,
    /// An input-only device ([`FAKE_INPUT_ID`]) and an output-only one
    /// ([`FAKE_OUTPUT_ID`]) under one name, like one interface listed once per
    /// direction: a session must name a different output device than its input device to
    /// play anything.
    Split,
}

impl FakeEndpoints {
    /// The id of the device carrying `direction`.
    pub(super) fn id(self, direction: Direction) -> &'static str {
        match (self, direction) {
            (Self::Duplex, _) => FAKE_DEVICE_ID,
            (Self::Split, Direction::Input) => FAKE_INPUT_ID,
            (Self::Split, Direction::Output) => FAKE_OUTPUT_ID,
        }
    }
}

/// How the simulated callback is driven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeDrive {
    /// A test calls [`FakeDriver::step`](super::FakeDriver::step). [`Backend::open`](crate::backend::Backend::open) parks the driver for
    /// [`FakeBackend::take_driver`](super::FakeBackend::take_driver); [`FakeBackend::open_manual`](super::FakeBackend::open_manual) returns it directly.
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

/// An analog path from a device output to a device input: delay, an optional memoryless
/// nonlinearity, then a short FIR, plus independent Gaussian noise. A cable loopback is
/// `fir = [1.0]`, noise 0; an acoustic measurement path is a room/speaker impulse response
/// plus ambient noise, and a distorting driver in front of it is the nonlinearity (a
/// Hammerstein model, whose harmonic levels are known analytically).
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
    /// Coefficients `[c2, c3, …]` of `y = x + c2·x² + c3·x³ + …`, applied to each output
    /// sample before the FIR; empty = linear. A sine of peak `A` then has a second harmonic of
    /// `c2·A²/2` and a third of `c3·A³/4` (with `¾·c3·A³` added to the fundamental).
    pub nonlinearity: Vec<f64>,
    /// A diffuse room tail added after the FIR; `None` = none.
    pub reverb: Option<FakeReverb>,
}

/// A diffuse room tail: a Schroeder reverberator (eight parallel feedback combs, then four
/// series all-passes). Every comb's loop gain is set so its energy falls 60 dB in `t60_s`,
/// at every frequency, and the all-passes change no level: the tail decays with exactly
/// that reverberation time, which a room measurement can be checked against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FakeReverb {
    /// Reverberation time, seconds.
    pub t60_s: f32,
    /// Gain of the tail re the path's direct sound.
    pub level: f32,
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
            nonlinearity: Vec::new(),
            reverb: None,
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
            nonlinearity: Vec::new(),
            reverb: None,
        }
    }

    /// This path with the memoryless nonlinearity `[c2, c3, …]` (see
    /// [`FakePath::nonlinearity`]).
    pub fn distorting(mut self, coefficients: Vec<f64>) -> Self {
        self.nonlinearity = coefficients;
        self
    }

    /// This path with a diffuse room tail ([`FakeReverb`]).
    pub fn reverberant(mut self, reverb: FakeReverb) -> Self {
        self.reverb = Some(reverb);
        self
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
    /// Names of the input channels as listed (one per input); `None`: unnamed, like a device
    /// whose driver names no channels.
    pub input_names: Option<Vec<String>>,
    /// Names of the output channels as listed (one per output).
    pub output_names: Option<Vec<String>>,
    /// One device or one per direction.
    pub endpoints: FakeEndpoints,
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
            input_names: None,
            output_names: None,
            endpoints: FakeEndpoints::Duplex,
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
    /// Channel names given, but not one per channel.
    #[error("channel names must name every channel of their direction")]
    ChannelNames,
}

impl FakeConfig {
    pub(super) fn validate(&self) -> Result<(), FakeConfigError> {
        if self.sample_rate == 0 {
            return Err(FakeConfigError::Zero("sample_rate"));
        }
        if self.block_frames == 0 {
            return Err(FakeConfigError::Zero("block_frames"));
        }
        if self.inputs == 0 {
            return Err(FakeConfigError::Zero("inputs"));
        }
        if self
            .input_names
            .as_ref()
            .is_some_and(|n| n.len() != usize::from(self.inputs))
            || self
                .output_names
                .as_ref()
                .is_some_and(|n| n.len() != usize::from(self.outputs))
        {
            return Err(FakeConfigError::ChannelNames);
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
            if !ok(f64::from(p.noise_rms))
                || p.reverb
                    .is_some_and(|r| !(r.t60_s.is_finite() && r.t60_s > 0.0 && r.level.is_finite()))
                || p.fir.iter().any(|c| !c.is_finite())
                || p.nonlinearity.iter().any(|c| !c.is_finite())
            {
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

    pub(super) fn ns(&self, frames: u64) -> u64 {
        (u128::from(frames) * 1_000_000_000 / u128::from(self.sample_rate)) as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OutputFault {
    pub(super) at: u64,
    pub(super) frames: u32,
    pub(super) repeat: bool,
}

pub(super) fn output_faults(faults: &[FakeFault]) -> Vec<OutputFault> {
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
