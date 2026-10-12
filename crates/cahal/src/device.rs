//! One HAL device: its properties, listeners, and the IOProc.

use std::time::{Duration, Instant};

use objc2_core_audio as ca;
use objc2_core_audio_types::{
    AudioStreamBasicDescription, AudioValueRange, kAudioFormatFlagIsBigEndian,
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved, kAudioFormatLinearPCM,
};

use crate::ioproc::{self, IoProc};
use crate::layout::Cycle;
use crate::listener::Listener;
use crate::property::{self, GLOBAL, MAIN, addr, global, scope};
use crate::types::{
    ClockSource, DeviceInfo, DeviceProperty, Error, FrameRange, IoConfig, Latency, RateRange,
    Scope, StreamFormat, StreamInfo, StreamLayout, Transport,
};

/// An `AudioObjectID` that is a device. `Copy`: the HAL owns the object.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Device(u32);

fn wildcard(selector: u32) -> ca::AudioObjectPropertyAddress {
    addr(
        selector,
        ca::kAudioObjectPropertyScopeWildcard,
        ca::kAudioObjectPropertyElementWildcard,
    )
}

fn selectors(p: DeviceProperty) -> &'static [u32] {
    match p {
        DeviceProperty::Alive => &[ca::kAudioDevicePropertyDeviceIsAlive],
        DeviceProperty::NominalRate => &[ca::kAudioDevicePropertyNominalSampleRate],
        DeviceProperty::BufferFrameSize => &[ca::kAudioDevicePropertyBufferFrameSize],
        DeviceProperty::ProcessorOverload => &[ca::kAudioDeviceProcessorOverload],
        DeviceProperty::Latency => &[
            ca::kAudioDevicePropertyLatency,
            ca::kAudioDevicePropertySafetyOffset,
        ],
        DeviceProperty::StreamConfiguration => &[ca::kAudioDevicePropertyStreamConfiguration],
        DeviceProperty::ClockSource => &[ca::kAudioDevicePropertyClockSource],
        DeviceProperty::IoStoppedAbnormally => &[ca::kAudioDevicePropertyIOStoppedAbnormally],
    }
}

fn stream_format(f: &AudioStreamBasicDescription) -> StreamFormat {
    let float = f.mFormatID == kAudioFormatLinearPCM
        && f.mFormatFlags & kAudioFormatFlagIsFloat != 0
        && f.mFormatFlags & kAudioFormatFlagIsBigEndian == 0
        && f.mBitsPerChannel == 32;
    if float {
        StreamFormat::Float32
    } else {
        StreamFormat::Other {
            format_id: f.mFormatID,
            flags: f.mFormatFlags,
            bits: f.mBitsPerChannel,
        }
    }
}

impl Device {
    pub(crate) fn from_id(id: u32) -> Self {
        Self(id)
    }

    /// The `AudioObjectID`. A replugged device may come back under another one; its UID
    /// stays.
    pub fn object_id(self) -> u32 {
        self.0
    }

    /// UID, name, transport and the streams of both directions.
    pub fn info(self) -> Result<DeviceInfo, Error> {
        Ok(DeviceInfo {
            uid: self.uid()?,
            name: property::get_string(self.0, global(ca::kAudioObjectPropertyName))
                .unwrap_or_default(),
            transport: self.transport()?,
            input: self.layout(Scope::Input)?,
            output: self.layout(Scope::Output)?,
        })
    }

    /// `kAudioDevicePropertyDeviceUID`.
    pub fn uid(self) -> Result<String, Error> {
        property::get_string(self.0, global(ca::kAudioDevicePropertyDeviceUID))
    }

    /// `kAudioDevicePropertyTransportType`.
    pub fn transport(self) -> Result<Transport, Error> {
        property::get::<u32>(self.0, global(ca::kAudioDevicePropertyTransportType))
            .map(Transport::from_raw)
    }

    /// `kAudioDevicePropertyDeviceIsAlive`.
    pub fn is_alive(self) -> Result<bool, Error> {
        property::get::<u32>(self.0, global(ca::kAudioDevicePropertyDeviceIsAlive)).map(|v| v != 0)
    }

    /// The streams of `dir`, device order, each with its virtual format.
    pub fn layout(self, dir: Scope) -> Result<StreamLayout, Error> {
        let streams = property::get_vec::<u32>(
            self.0,
            addr(ca::kAudioDevicePropertyStreams, scope(dir), MAIN),
        )?;
        let mut out = Vec::with_capacity(streams.len());
        for s in streams {
            let f = property::get::<AudioStreamBasicDescription>(
                s,
                global(ca::kAudioStreamPropertyVirtualFormat),
            )?;
            out.push(StreamInfo {
                channels: f.mChannelsPerFrame,
                interleaved: f.mFormatFlags & kAudioFormatFlagIsNonInterleaved == 0,
                format: stream_format(&f),
            });
        }
        Ok(StreamLayout { streams: out })
    }

    /// Channels per IOProc buffer of `dir` (`kAudioDevicePropertyStreamConfiguration`).
    pub fn buffer_channels(self, dir: Scope) -> Result<Vec<u32>, Error> {
        property::get_buffer_channels(
            self.0,
            addr(
                ca::kAudioDevicePropertyStreamConfiguration,
                scope(dir),
                MAIN,
            ),
        )
    }

    /// Each channel's name (`kAudioObjectPropertyElementName`), `None` where the device
    /// names none.
    pub fn channel_names(self, dir: Scope) -> Result<Vec<Option<String>>, Error> {
        let n = self.layout(dir)?.channels();
        Ok((1..=n)
            .map(|el| {
                property::get_string(
                    self.0,
                    addr(ca::kAudioObjectPropertyElementName, scope(dir), el),
                )
                .ok()
                .filter(|s| !s.trim().is_empty())
            })
            .collect())
    }

    /// `kAudioDevicePropertyNominalSampleRate`.
    pub fn nominal_rate(self) -> Result<f64, Error> {
        property::get::<f64>(self.0, global(ca::kAudioDevicePropertyNominalSampleRate))
    }

    /// Sets the nominal rate and waits (up to 2 s) until the device reports it: the HAL
    /// applies the change asynchronously.
    pub fn set_nominal_rate(self, hz: f64) -> Result<(), Error> {
        property::set(
            self.0,
            global(ca::kAudioDevicePropertyNominalSampleRate),
            &hz,
        )?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if (self.nominal_rate()? - hz).abs() < 0.5 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::Unsupported("the device did not take the rate"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// `kAudioDevicePropertyAvailableNominalSampleRates`.
    pub fn available_rates(self) -> Result<Vec<RateRange>, Error> {
        Ok(property::get_vec::<AudioValueRange>(
            self.0,
            global(ca::kAudioDevicePropertyAvailableNominalSampleRates),
        )?
        .into_iter()
        .map(|r| RateRange {
            min: r.mMinimum,
            max: r.mMaximum,
        })
        .collect())
    }

    /// `kAudioDevicePropertyBufferFrameSize`: frames per IOProc cycle.
    pub fn buffer_frames(self) -> Result<u32, Error> {
        property::get::<u32>(self.0, global(ca::kAudioDevicePropertyBufferFrameSize))
    }

    /// Asks for `frames` per cycle. The HAL runs a device at the smallest size any client
    /// asks for, so read it back after starting.
    pub fn set_buffer_frames(self, frames: u32) -> Result<(), Error> {
        property::set(
            self.0,
            global(ca::kAudioDevicePropertyBufferFrameSize),
            &frames,
        )
    }

    /// `kAudioDevicePropertyBufferFrameSizeRange`.
    pub fn buffer_frame_range(self) -> Result<FrameRange, Error> {
        let r = property::get::<AudioValueRange>(
            self.0,
            global(ca::kAudioDevicePropertyBufferFrameSizeRange),
        )?;
        Ok(FrameRange {
            min: r.mMinimum.max(1.0) as u32,
            max: r.mMaximum.max(1.0) as u32,
        })
    }

    /// `kAudioDevicePropertyUsesVariableBufferFrameSizes`: the largest cycle, when cycles
    /// vary in size.
    pub fn variable_buffer_frames(self) -> Option<u32> {
        let a = global(ca::kAudioDevicePropertyUsesVariableBufferFrameSizes);
        if !property::has(self.0, a) {
            return None;
        }
        property::get::<u32>(self.0, a).ok().filter(|&v| v != 0)
    }

    /// The device's, the safety offset's and each stream's latency for `dir`, in frames.
    pub fn latency(self, dir: Scope) -> Result<Latency, Error> {
        let sc = scope(dir);
        let device = property::get::<u32>(self.0, addr(ca::kAudioDevicePropertyLatency, sc, MAIN))
            .unwrap_or(0);
        let safety_offset =
            property::get::<u32>(self.0, addr(ca::kAudioDevicePropertySafetyOffset, sc, MAIN))
                .unwrap_or(0);
        let streams =
            property::get_vec::<u32>(self.0, addr(ca::kAudioDevicePropertyStreams, sc, MAIN))?
                .into_iter()
                .map(|s| {
                    property::get::<u32>(s, global(ca::kAudioStreamPropertyLatency)).unwrap_or(0)
                })
                .collect();
        Ok(Latency {
            device,
            safety_offset,
            streams,
        })
    }

    /// `kAudioDevicePropertyClockDomain`: devices with the same non-zero domain run on one
    /// clock.
    pub fn clock_domain(self) -> Result<u32, Error> {
        property::get::<u32>(self.0, global(ca::kAudioDevicePropertyClockDomain))
    }

    /// The clock sources the device offers, the current one marked; empty when it lists
    /// none.
    pub fn clock_sources(self) -> Result<Vec<ClockSource>, Error> {
        for sc in [GLOBAL, scope(Scope::Output), scope(Scope::Input)] {
            let a = addr(ca::kAudioDevicePropertyClockSources, sc, MAIN);
            if !property::has(self.0, a) {
                continue;
            }
            let ids = property::get_vec::<u32>(self.0, a)?;
            let current =
                property::get::<u32>(self.0, addr(ca::kAudioDevicePropertyClockSource, sc, MAIN))
                    .ok();
            return Ok(ids
                .into_iter()
                .map(|id| ClockSource {
                    id,
                    name: property::clock_source_name(self.0, sc, id).unwrap_or_default(),
                    current: current == Some(id),
                })
                .collect());
        }
        Ok(Vec::new())
    }

    /// The pid holding the device in hog (exclusive) mode, if any process does.
    pub fn hog_pid(self) -> Option<i32> {
        let a = global(ca::kAudioDevicePropertyHogMode);
        if !property::has(self.0, a) {
            return None;
        }
        property::get::<i32>(self.0, a).ok().filter(|&p| p != -1)
    }

    /// Calls `f` on the HAL's notification thread whenever one of `props` changes. `f`
    /// must not block.
    pub fn listen(
        self,
        props: &[DeviceProperty],
        f: impl Fn(DeviceProperty) + Send + Sync + 'static,
    ) -> Result<Listener, Error> {
        let props: Vec<DeviceProperty> = props.to_vec();
        let addrs = props
            .iter()
            .flat_map(|&p| selectors(p).iter().map(|&s| wildcard(s)))
            .collect();
        Listener::register(
            self.0,
            addrs,
            Box::new(move |sel| {
                if let Some(&p) = props.iter().find(|&&p| selectors(p).contains(&sel)) {
                    f(p);
                }
            }),
        )
    }

    /// Starts an IOProc on the device that calls `f` once per cycle on the HAL's I/O thread
    /// with the cycle's buffers and its [`CycleTime`](crate::CycleTime). Output buffers are
    /// zeroed before `f` runs. Every stream of a direction in use must be 32-bit float.
    pub fn start_io<F>(self, config: IoConfig, f: F) -> Result<IoProc, Error>
    where
        F: FnMut(Cycle<'_, '_>) + Send + 'static,
    {
        ioproc::start(self, config, f)
    }
}
