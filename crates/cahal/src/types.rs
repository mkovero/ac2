//! Plain data the HAL reports, on every OS so code that maps it can be tested anywhere.

use std::fmt;

/// A direction of a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Capture.
    Input,
    /// Playback.
    Output,
}

/// A four-character code, as the HAL's selectors and status codes are often spelled
/// (`'nsrt'`, `'!dat'`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FourCc(pub u32);

impl fmt::Display for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.0.to_be_bytes();
        if b.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
            write!(
                f,
                "'{}'",
                b.iter().map(|&c| char::from(c)).collect::<String>()
            )
        } else {
            write!(f, "{}", self.0 as i32)
        }
    }
}

impl fmt::Debug for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// What can go wrong talking to the HAL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A HAL call returned a non-zero `OSStatus`.
    Status {
        /// The function that failed.
        call: &'static str,
        /// The property selector, for property calls (0 otherwise).
        selector: FourCc,
        /// The status, often itself a four-character code (`'!obj'`, `'who?'`).
        status: i32,
    },
    /// The object or value asked for does not exist.
    NotFound,
    /// The device or the HAL cannot do this; the text says what.
    Unsupported(&'static str),
    /// A string from the HAL was not valid UTF-8.
    Utf8,
    /// The IOProc was started but not called within the deadline.
    StartTimedOut,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status {
                call,
                selector,
                status,
            } => {
                if selector.0 == 0 {
                    write!(f, "{call} failed: {}", FourCc(*status as u32))
                } else {
                    write!(f, "{call} {selector} failed: {}", FourCc(*status as u32))
                }
            }
            Self::NotFound => f.write_str("not found"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::Utf8 => f.write_str("a HAL string is not UTF-8"),
            Self::StartTimedOut => f.write_str("the device did not start calling back"),
        }
    }
}

impl std::error::Error for Error {}

/// `kAudioDeviceTransportType…`: how a device is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transport {
    /// Built into the machine.
    BuiltIn,
    /// An aggregate of other devices.
    Aggregate,
    /// A software device (BlackHole, Loopback).
    Virtual,
    /// USB.
    Usb,
    /// Thunderbolt.
    Thunderbolt,
    /// PCI / PCIe.
    Pci,
    /// FireWire.
    FireWire,
    /// Bluetooth (classic or LE).
    Bluetooth,
    /// AVB.
    Avb,
    /// HDMI or DisplayPort.
    Display,
    /// AirPlay.
    AirPlay,
    /// Anything else, as the HAL's code.
    Other(FourCc),
}

impl Transport {
    /// From the raw `kAudioDevicePropertyTransportType` value.
    pub fn from_raw(code: u32) -> Self {
        match &code.to_be_bytes() {
            b"bltn" => Self::BuiltIn,
            b"grup" => Self::Aggregate,
            b"virt" => Self::Virtual,
            b"usb " => Self::Usb,
            b"thun" => Self::Thunderbolt,
            b"pci " => Self::Pci,
            b"1394" => Self::FireWire,
            b"blue" | b"blea" => Self::Bluetooth,
            b"eavb" => Self::Avb,
            b"hdmi" | b"dprt" => Self::Display,
            b"airp" => Self::AirPlay,
            _ => Self::Other(FourCc(code)),
        }
    }
}

/// A device as the HAL describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    /// `kAudioDevicePropertyDeviceUID`: stable across reboots and replugs.
    pub uid: String,
    /// `kAudioObjectPropertyName`.
    pub name: String,
    /// `kAudioDevicePropertyTransportType`.
    pub transport: Transport,
    /// Capture streams, in device order.
    pub input: StreamLayout,
    /// Playback streams, in device order.
    pub output: StreamLayout,
}

/// The streams of one direction, in device order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamLayout {
    /// One entry per stream.
    pub streams: Vec<StreamInfo>,
}

impl StreamLayout {
    /// Channels across all streams.
    pub fn channels(&self) -> u32 {
        self.streams.iter().map(|s| s.channels).sum()
    }

    /// True when every stream's virtual format is 32-bit native float.
    pub fn all_float32(&self) -> bool {
        self.streams
            .iter()
            .all(|s| s.format == StreamFormat::Float32)
    }

    /// The streams that carry any of the device channels `channels` (counted across all
    /// streams in device order); a channel past the last is in none.
    pub fn streams_of(&self, channels: impl IntoIterator<Item = usize>) -> StreamSet {
        let mut set = StreamSet::NONE;
        for ch in channels {
            let mut first = 0usize;
            for (i, s) in self.streams.iter().enumerate() {
                let end = first + s.channels as usize;
                if ch < end {
                    set = set.with(i);
                    break;
                }
                first = end;
            }
        }
        set
    }
}

/// A set of a direction's streams by index in device order, for
/// `kAudioDevicePropertyIOProcStreamUsage`. Streams past [`StreamSet::MAX`] are always in
/// it: the HAL then converts them, which costs time but changes no sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamSet(u64);

impl StreamSet {
    /// Streams a set names one by one.
    pub const MAX: usize = 64;
    /// Every stream.
    pub const ALL: Self = Self(u64::MAX);
    /// No stream.
    pub const NONE: Self = Self(0);

    /// The set with stream `i` added.
    #[must_use]
    pub const fn with(self, i: usize) -> Self {
        if i < Self::MAX {
            Self(self.0 | 1 << i)
        } else {
            self
        }
    }

    /// Stream `i` is in the set.
    pub const fn contains(self, i: usize) -> bool {
        i >= Self::MAX || self.0 & (1 << i) != 0
    }

    /// No stream is in the set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Every one of `n` streams is in the set.
    pub fn covers(self, n: usize) -> bool {
        (0..n.min(Self::MAX)).all(|i| self.contains(i))
    }
}

/// One stream of a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    /// Channels in the stream.
    pub channels: u32,
    /// True when one buffer carries all channels interleaved; false when the IOProc gets
    /// one buffer per channel.
    pub interleaved: bool,
    /// The virtual format the IOProc sees.
    pub format: StreamFormat,
}

/// A stream's virtual format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamFormat {
    /// Linear PCM, 32-bit float, packed, native endian: the only format the IOProc reads
    /// and writes through [`crate::Channel`].
    Float32,
    /// Anything else, as the HAL states it.
    Other {
        /// `mFormatID`.
        format_id: u32,
        /// `mFormatFlags`.
        flags: u32,
        /// `mBitsPerChannel`.
        bits: u32,
    },
}

/// `kAudioDevicePropertyLatency`, `…SafetyOffset` and the stream latencies of one
/// direction, in frames.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Latency {
    /// The device's latency.
    pub device: u32,
    /// The safety offset.
    pub safety_offset: u32,
    /// `kAudioStreamPropertyLatency` of each stream, device order.
    pub streams: Vec<u32>,
}

/// An inclusive range of rates, Hz.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateRange {
    /// Lowest.
    pub min: f64,
    /// Highest.
    pub max: f64,
}

/// An inclusive range of frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRange {
    /// Smallest.
    pub min: u32,
    /// Largest.
    pub max: u32,
}

/// A clock source a device offers, as its driver names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockSource {
    /// The source's id.
    pub id: u32,
    /// Its name ("Internal", "Word Clock").
    pub name: String,
    /// The device runs on it now.
    pub current: bool,
}

/// What a running IOProc serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoConfig {
    /// Read the device's inputs.
    pub input: bool,
    /// Write the device's outputs.
    pub output: bool,
    /// The input streams the HAL serves this IOProc (`kAudioDevicePropertyIOProcStreamUsage`);
    /// the others arrive without a buffer and read as silence, and the HAL skips their
    /// conversion. Output streams are always on: what is written to them can change
    /// without a restart.
    pub input_streams: StreamSet,
}

impl IoConfig {
    /// Every stream of the chosen directions.
    pub const fn new(input: bool, output: bool) -> Self {
        Self {
            input,
            output,
            input_streams: StreamSet::ALL,
        }
    }
}

/// How a cycle's sample time relates to the previous cycle's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gap {
    /// The first cycle.
    First,
    /// Continuous.
    None,
    /// Frames lost: the index jumped forward by exactly this many. `Forward(0)` marks a
    /// cycle whose sample time was not valid: from there on the index counts frames.
    Forward(u64),
    /// The sample time went backwards (the device restarted its IO); the index kept
    /// counting forward.
    Backward,
}

/// The timing of one IOProc cycle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CycleTime {
    /// Index of the first frame, shared by the input and output of this cycle.
    pub index: u64,
    /// Frames in the cycle.
    pub frames: u32,
    /// Against the previous cycle.
    pub gap: Gap,
    /// True once a sample time was invalid and the index became a frame count.
    pub estimated: bool,
    /// `kAudioDeviceProcessorOverload` notifications since the previous cycle.
    pub overloads: u32,
    /// `inInputTime.mSampleTime` as the HAL gave it; `None` when not flagged valid.
    pub input_sample_time: Option<f64>,
    /// `inOutputTime.mSampleTime` as the HAL gave it; `None` when not flagged valid.
    pub output_sample_time: Option<f64>,
    /// `inNow.mHostTime` in ns on the host clock.
    pub now_ns: u64,
    /// When the first input frame was sampled, ns on the host clock.
    pub input_ns: Option<u64>,
    /// When the first output frame will play, ns on the host clock.
    pub output_ns: Option<u64>,
    /// The HAL's rate scalar (actual / nominal rate) for this cycle.
    pub rate_scalar: f64,
}

/// Counters of one IOProc.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IoStats {
    /// Cycles the closure was called for.
    pub cycles: u64,
    /// Overload notifications.
    pub overloads: u64,
    /// Cycles that followed a gap.
    pub gaps: u64,
    /// Frames lost in forward gaps.
    pub frames_lost: u64,
    /// Cycles whose buffers did not match the layout the IOProc started with (skipped,
    /// outputs silent).
    pub mismatched: u64,
    /// The closure panicked; it is not called again and the outputs stay silent.
    pub panicked: bool,
}

/// A system-wide property to listen to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemProperty {
    /// The device list changed.
    Devices,
    /// The default input device changed.
    DefaultInput,
    /// The default output device changed.
    DefaultOutput,
    /// coreaudiod restarted: every device object is gone.
    ServiceRestarted,
}

/// A device property to listen to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceProperty {
    /// `kAudioDevicePropertyDeviceIsAlive`.
    Alive,
    /// `kAudioDevicePropertyNominalSampleRate`.
    NominalRate,
    /// `kAudioDevicePropertyBufferFrameSize`.
    BufferFrameSize,
    /// `kAudioDeviceProcessorOverload`.
    ProcessorOverload,
    /// `kAudioDevicePropertyLatency` or the safety offset.
    Latency,
    /// `kAudioDevicePropertyStreamConfiguration`: the channels per buffer changed.
    StreamConfiguration,
    /// `kAudioDevicePropertyClockSource`.
    ClockSource,
    /// `kAudioDevicePropertyIOStoppedAbnormally`.
    IoStoppedAbnormally,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_cc_reads_as_text_when_it_is() {
        assert_eq!(FourCc(u32::from_be_bytes(*b"!dat")).to_string(), "'!dat'");
        assert_eq!(FourCc((-50i32) as u32).to_string(), "-50");
    }

    #[test]
    fn transport_codes() {
        assert_eq!(
            Transport::from_raw(u32::from_be_bytes(*b"virt")),
            Transport::Virtual
        );
        assert_eq!(
            Transport::from_raw(u32::from_be_bytes(*b"usb ")),
            Transport::Usb
        );
        assert!(matches!(Transport::from_raw(7), Transport::Other(_)));
    }

    #[test]
    fn layout_channels_and_format() {
        let f = |channels, format| StreamInfo {
            channels,
            interleaved: true,
            format,
        };
        let l = StreamLayout {
            streams: vec![f(8, StreamFormat::Float32), f(8, StreamFormat::Float32)],
        };
        assert_eq!(l.channels(), 16);
        assert!(l.all_float32());
        let other = StreamFormat::Other {
            format_id: 1,
            flags: 0,
            bits: 24,
        };
        let l = StreamLayout {
            streams: vec![f(2, other)],
        };
        assert!(!l.all_float32());
    }

    #[test]
    fn streams_of_channels() {
        let f = |channels, interleaved| StreamInfo {
            channels,
            interleaved,
            format: StreamFormat::Float32,
        };
        // Analog 1–8 interleaved, ADAT 9–16 non-interleaved, a mono stream last.
        let l = StreamLayout {
            streams: vec![f(8, true), f(8, false), f(1, true)],
        };
        let s = l.streams_of([0, 7]);
        assert!(s.contains(0) && !s.contains(1) && !s.contains(2));
        let s = l.streams_of([8, 16]);
        assert!(!s.contains(0) && s.contains(1) && s.contains(2));
        assert!(
            s.contains(StreamSet::MAX),
            "streams past the set's reach stay on"
        );
        assert!(l.streams_of([17]).is_empty());
        assert!(l.streams_of([0, 15, 16]).covers(3));
        assert!(!l.streams_of([0, 16]).covers(3));
        assert!(StreamSet::ALL.covers(200));
        assert_eq!(IoConfig::new(true, false).input_streams, StreamSet::ALL);
    }
}
