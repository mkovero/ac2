//! cpal backend on the OS's default host (Core Audio, WASAPI). Not built on Linux, where
//! JACK is the only real backend.
//!
//! cpal has no duplex stream: capture and playback are two streams with their own callbacks
//! and, except on CoreAudio's IO thread, their own threads. Each side therefore keeps its own
//! index: the capture index and the output index are separate counters, both running frame
//! counts with timestamp-estimated gaps. Their relation is learnt by the loopback timing
//! monitor, never assumed.
//!
//! cpal converts nothing, so the native device format is opened and converted here
//! (f32, i32, i24, i16).

use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Data, ErrorKind, FromSample, I24, InputCallbackInfo, OutputCallbackInfo,
    StreamConfig, SupportedBufferSize, SupportedStreamConfigRange,
};

use crate::backend::{
    Backend, BackendKind, ClockRelation, Delivery, DeviceCaps, DeviceId, DeviceSelector, Direction,
    DirectionCaps, DuplexRequest, FrameRange, IndexExactness, Negotiated, RateRange, SampleFormat,
    StaticLatency, short_buffer_frames,
};
use crate::block::{BlockProducer, BlockStamp};
use crate::clock::TimestampClock;
use crate::error::{AudioError, Operation, Unsupported};
use crate::events::{BackendEvents, EventLatch};
use crate::output::{OutputRenderer, OutputStamp};
use crate::stream::{DuplexStream, Plumbing, StreamParts};

/// Formats converted here, in preference order: float needs no scaling, wider integers keep
/// the converter's resolution.
const FORMATS: [(cpal::SampleFormat, SampleFormat); 4] = [
    (cpal::SampleFormat::F32, SampleFormat::F32),
    (cpal::SampleFormat::I32, SampleFormat::I32),
    (cpal::SampleFormat::I24, SampleFormat::I24),
    (cpal::SampleFormat::I16, SampleFormat::I16),
];

fn our_format(f: cpal::SampleFormat) -> Option<SampleFormat> {
    FORMATS.iter().find(|(c, _)| *c == f).map(|(_, o)| *o)
}

/// cpal on the default host.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpalBackend;

impl CpalBackend {
    /// The backend for the OS's default cpal host.
    pub fn new() -> Self {
        Self
    }
}

fn backend_err(operation: Operation, e: impl std::fmt::Display) -> AudioError {
    AudioError::Backend {
        backend: BackendKind::Cpal,
        operation,
        detail: e.to_string(),
    }
}

fn device_id(d: &cpal::Device) -> DeviceId {
    DeviceId(d.id().map(|i| i.to_string()).unwrap_or_default())
}

fn device_name(d: &cpal::Device) -> String {
    d.description()
        .map(|desc| desc.name().to_string())
        .unwrap_or_else(|_| device_id(d).0)
}

fn direction_caps(
    ranges: Result<Vec<SupportedStreamConfigRange>, cpal::Error>,
    default_rate: Option<u32>,
    notes: &mut Vec<String>,
    direction: Direction,
) -> Option<DirectionCaps> {
    let ranges = match ranges {
        Ok(r) => r,
        Err(e) => {
            notes.push(format!("{direction:?} configs: {e}"));
            return None;
        }
    };
    let usable: Vec<_> = ranges
        .iter()
        .filter(|r| our_format(r.sample_format()).is_some())
        .collect();
    if usable.len() < ranges.len() {
        notes.push(format!(
            "{direction:?}: {} configs in formats not converted here",
            ranges.len() - usable.len()
        ));
    }
    if usable.is_empty() {
        return None;
    }
    let mut rates: Vec<RateRange> = usable
        .iter()
        .map(|r| RateRange {
            min: r.min_sample_rate(),
            max: r.max_sample_rate(),
        })
        .collect();
    rates.sort_by_key(|r| (r.min, r.max));
    rates.dedup();
    let mut formats: Vec<SampleFormat> = Vec::new();
    for (c, o) in FORMATS {
        if usable.iter().any(|r| r.sample_format() == c) {
            formats.push(o);
        }
    }
    let buffer_frames = usable.iter().find_map(|r| frame_range(r.buffer_size()));
    Some(DirectionCaps {
        max_channels: usable.iter().map(|r| r.channels()).max().unwrap_or(0),
        rates,
        buffer_frames,
        formats,
        default_rate,
        // cpal states a buffer range, never the size a default stream would use, and has
        // no channel names.
        default_buffer: None,
        channel_names: None,
    })
}

/// A config to open: channels, cpal's format, ours, and the buffer sizes it supports.
type Picked = (u16, cpal::SampleFormat, SampleFormat, Option<FrameRange>);

fn frame_range(b: &SupportedBufferSize) -> Option<FrameRange> {
    match b {
        SupportedBufferSize::Range { min, max } => Some(FrameRange {
            min: *min,
            max: *max,
        }),
        SupportedBufferSize::Unknown => None,
    }
}

/// The cheapest-to-convert config with at least `min_channels` at `rate`, fewest surplus
/// channels first.
fn pick_config(
    ranges: Vec<SupportedStreamConfigRange>,
    rate: u32,
    min_channels: u16,
) -> Option<Picked> {
    ranges
        .into_iter()
        .filter(|r| r.channels() >= min_channels && r.contains_rate(rate))
        .filter_map(|r| {
            let pref = FORMATS.iter().position(|(c, _)| *c == r.sample_format())?;
            Some((pref, r))
        })
        .min_by_key(|(pref, r)| (*pref, r.channels()))
        .and_then(|(_, r)| {
            our_format(r.sample_format()).map(|o| {
                (
                    r.channels(),
                    r.sample_format(),
                    o,
                    frame_range(r.buffer_size()),
                )
            })
        })
}

/// Both directions' supported ranges, intersected; `None` where neither states one.
fn common_range(a: Option<FrameRange>, b: Option<FrameRange>) -> Option<FrameRange> {
    match (a, b) {
        (Some(a), Some(b)) if a.min.max(b.min) <= a.max.min(b.max) => Some(FrameRange {
            min: a.min.max(b.min),
            max: a.max.min(b.max),
        }),
        (Some(r), None) | (None, Some(r)) => Some(r),
        // Disjoint ranges: the input side decides; the output may still accept it.
        (Some(a), Some(_)) => Some(a),
        (None, None) => None,
    }
}

fn duplex_relation(d: &cpal::Device) -> ClockRelation {
    if d.supports_input() && d.supports_output() {
        // CoreAudio and ALSA use one device object (one PCM name) for both directions.
        // WASAPI endpoints are single-direction and never get here.
        ClockRelation::SameDeviceSeparateCallbacks
    } else {
        ClockRelation::Unknown
    }
}

fn find_device(
    host: &cpal::Host,
    selector: &DeviceSelector,
    direction: Direction,
) -> Result<cpal::Device, AudioError> {
    let not_found = || AudioError::DeviceNotFound {
        direction,
        selector: selector.to_string(),
    };
    match selector {
        DeviceSelector::Default => match direction {
            Direction::Input => host.default_input_device(),
            Direction::Output => host.default_output_device(),
        }
        .ok_or_else(not_found),
        DeviceSelector::Id(want) => host
            .devices()
            .map_err(|e| backend_err(Operation::Enumerate, e))?
            .find(|d| {
                device_id(d) == *want
                    && match direction {
                        Direction::Input => d.supports_input(),
                        Direction::Output => d.supports_output(),
                    }
            })
            .ok_or_else(not_found),
    }
}

/// Error callback: runs on a notification path, counters only.
fn on_error(events: &BackendEvents, e: &cpal::Error) {
    match e.kind() {
        ErrorKind::Xrun => events.xrun(),
        ErrorKind::DeviceChanged => events.config_change(),
        ErrorKind::StreamInvalidated | ErrorKind::DeviceNotAvailable => events.end(),
        _ => events.error(),
    }
}

impl Backend for CpalBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Cpal
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let host = cpal::default_host();
        let host_name = host.id().to_string();
        let devices = host
            .devices()
            .map_err(|e| backend_err(Operation::Enumerate, e))?;
        let mut out = Vec::new();
        for d in devices {
            let mut notes = Vec::new();
            let input = if d.supports_input() {
                direction_caps(
                    d.supported_input_configs().map(Iterator::collect),
                    d.default_input_config().ok().map(|c| c.sample_rate()),
                    &mut notes,
                    Direction::Input,
                )
            } else {
                None
            };
            let output = if d.supports_output() {
                direction_caps(
                    d.supported_output_configs().map(Iterator::collect),
                    d.default_output_config().ok().map(|c| c.sample_rate()),
                    &mut notes,
                    Direction::Output,
                )
            } else {
                None
            };
            out.push(DeviceCaps {
                backend: BackendKind::Cpal,
                host: host_name.clone(),
                id: device_id(&d),
                name: device_name(&d),
                input,
                output,
                duplex_clock: duplex_relation(&d),
                index: IndexExactness::Estimated,
                latency: StaticLatency::PerCallbackTimestamps,
                notes,
            });
        }
        Ok(out)
    }

    fn open(&self, mut request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        request.validate()?;
        let host = cpal::default_host();
        let in_dev = find_device(&host, &request.input_device, Direction::Input)?;
        let out_dev = if request.output_channels == 0 {
            None
        } else if request.output_device == DeviceSelector::Default && in_dev.supports_output() {
            // One device for both directions keeps one clock where the host allows it.
            Some(in_dev.clone())
        } else {
            Some(find_device(
                &host,
                &request.output_device,
                Direction::Output,
            )?)
        };

        let rate = match request.sample_rate {
            Some(r) => r,
            None => in_dev
                .default_input_config()
                .map_err(|e| backend_err(Operation::Open, e))?
                .sample_rate(),
        };
        let need_in = request.input_map.iter().copied().max().map_or(1, |m| m + 1);
        let in_ranges: Vec<_> = in_dev
            .supported_input_configs()
            .map_err(|e| backend_err(Operation::Open, e))?
            .collect();
        let device_inputs = in_ranges.iter().map(|r| r.channels()).max().unwrap_or(0);
        if need_in > device_inputs {
            return Err(Unsupported::InputChannel {
                channel: need_in - 1,
                available: device_inputs,
            }
            .into());
        }
        if !in_ranges.iter().any(|r| r.contains_rate(rate)) {
            return Err(Unsupported::SampleRate {
                requested: rate,
                offered: in_ranges
                    .iter()
                    .map(|r| format!("{}-{} Hz", r.min_sample_rate(), r.max_sample_rate()))
                    .collect::<Vec<_>>()
                    .join(", "),
            }
            .into());
        }
        let (in_channels, in_cpal_fmt, in_fmt, in_buffers) = pick_config(in_ranges, rate, need_in)
            .ok_or(Unsupported::SampleFormat {
                direction: Direction::Input,
            })?;

        let out_cfg = match &out_dev {
            None => None,
            Some(d) => {
                let ranges: Vec<_> = d
                    .supported_output_configs()
                    .map_err(|e| backend_err(Operation::Open, e))?
                    .collect();
                let available = ranges.iter().map(|r| r.channels()).max().unwrap_or(0);
                if request.output_channels > available {
                    return Err(Unsupported::OutputChannels {
                        requested: request.output_channels,
                        available,
                    }
                    .into());
                }
                Some(pick_config(ranges, rate, request.output_channels).ok_or(
                    Unsupported::SampleFormat {
                        direction: Direction::Output,
                    },
                )?)
            }
        };

        // No buffer asked for: a short fixed one, never the host's default (see
        // `SHORT_BUFFER_AT_48K`).
        let frames = request.buffer_frames.unwrap_or_else(|| {
            short_buffer_frames(rate, common_range(in_buffers, out_cfg.and_then(|c| c.3)))
        });
        let buffer_size = BufferSize::Fixed(frames);

        // Variable-size hosts (WASAPI) can deliver small packets; size headers for them.
        let min_block = request.buffer_frames.unwrap_or(64).clamp(16, 64) as usize;
        let plumbing = Plumbing::new(&mut request, rate, min_block);
        let events = Arc::clone(&plumbing.events);

        let mut input = InputSide {
            producer: plumbing.producer,
            clock: TimestampClock::new(rate),
            latch: EventLatch::new(Arc::clone(&events)),
            map: request.input_map.iter().map(|&c| usize::from(c)).collect(),
            device_channels: usize::from(in_channels),
        };
        let ev = Arc::clone(&events);
        let in_stream = in_dev
            .build_input_stream_raw(
                StreamConfig {
                    channels: in_channels,
                    sample_rate: rate,
                    buffer_size,
                },
                in_cpal_fmt,
                move |data: &Data, info: &InputCallbackInfo| input.on_data(data, info),
                move |e| on_error(&ev, &e),
                None,
            )
            .map_err(|e| backend_err(Operation::Open, e))?;

        let out_stream = match (&out_dev, out_cfg) {
            (Some(d), Some((channels, cpal_fmt, _, _))) => {
                let mut output = OutputSide {
                    renderer: plumbing.renderer,
                    clock: TimestampClock::new(rate),
                    latch: EventLatch::new(Arc::clone(&events)),
                    device_channels: usize::from(channels),
                };
                let ev = Arc::clone(&events);
                let s = d
                    .build_output_stream_raw(
                        StreamConfig {
                            channels,
                            sample_rate: rate,
                            buffer_size,
                        },
                        cpal_fmt,
                        move |data: &mut Data, info: &OutputCallbackInfo| {
                            output.on_data(data, info);
                        },
                        move |e| on_error(&ev, &e),
                        None,
                    )
                    .map_err(|e| backend_err(Operation::Open, e))?;
                Some(s)
            }
            _ => {
                // Capture only: the renderer never runs, so nothing is ever emitted.
                drop(plumbing.renderer);
                None
            }
        };

        if let Some(s) = &out_stream {
            s.play().map_err(|e| backend_err(Operation::Start, e))?;
        }
        in_stream
            .play()
            .map_err(|e| backend_err(Operation::Start, e))?;

        // What the host runs, else what was asked for (it was accepted as a fixed size).
        let buffer_frames = in_stream.buffer_size().ok().or(Some(frames));
        let same_device = out_dev
            .as_ref()
            .is_some_and(|d| device_id(d) == device_id(&in_dev));
        let negotiated = Negotiated {
            backend: BackendKind::Cpal,
            input_device: device_id(&in_dev),
            output_device: out_dev.as_ref().map_or(DeviceId(String::new()), device_id),
            sample_rate: rate,
            input_channels: request.input_map.len() as u16,
            device_input_channels: in_channels,
            output_channels: request.output_channels,
            device_output_channels: out_cfg.map_or(0, |(c, _, _, _)| c),
            buffer_frames,
            input_format: in_fmt,
            output_format: out_cfg.map(|(_, _, f, _)| f),
            clock: if same_device {
                duplex_relation(&in_dev)
            } else {
                ClockRelation::Unknown
            },
            index: IndexExactness::Estimated,
            latency: StaticLatency::PerCallbackTimestamps,
            delivery: Delivery::Device,
        };
        // Device buffers hold about two periods; allow three plus scheduling slack.
        let period = buffer_frames.unwrap_or(frames);
        let drain = Duration::from_secs_f64(3.0 * f64::from(period) / f64::from(rate))
            + Duration::from_millis(20);
        Ok(DuplexStream::new(StreamParts {
            negotiated,
            capture: plumbing.consumer,
            output: plumbing.output,
            events,
            drain,
            guard: Box::new((in_stream, out_stream)),
            patch: None,
        }))
    }
}

struct InputSide {
    producer: BlockProducer,
    clock: TimestampClock,
    latch: EventLatch,
    map: Box<[usize]>,
    device_channels: usize,
}

impl InputSide {
    #[inline]
    fn on_data(&mut self, data: &Data, info: &InputCallbackInfo) {
        match data.sample_format() {
            cpal::SampleFormat::F32 => self.push::<f32>(data, info),
            cpal::SampleFormat::I32 => self.push::<i32>(data, info),
            cpal::SampleFormat::I24 => self.push::<I24>(data, info),
            cpal::SampleFormat::I16 => self.push::<i16>(data, info),
            // Only the formats above are ever opened.
            _ => {}
        }
    }

    #[inline]
    fn push<T>(&mut self, data: &Data, info: &InputCallbackInfo)
    where
        T: cpal::SizedSample,
        f32: FromSample<T>,
    {
        let Some(src) = data.as_slice::<T>() else {
            return;
        };
        let ts = info.timestamp();
        let callback_ns = ts.callback.as_nanos() as u64;
        let capture_ns = ts.capture.as_nanos() as u64;
        let frames = (src.len() / self.device_channels.max(1)) as u32;
        let (start, mut flags) = self.clock.advance(frames, capture_ns);
        flags |= self.latch.take();
        let (map, dc) = (&self.map, self.device_channels);
        self.producer.push_with(
            BlockStamp {
                start_sample: start,
                frames,
                flags,
                callback_ns,
                capture_ns: Some(capture_ns),
            },
            |f, c| <f32 as FromSample<T>>::from_sample_(src[f * dc + map[c]]),
        );
    }
}

struct OutputSide {
    renderer: OutputRenderer,
    clock: TimestampClock,
    latch: EventLatch,
    device_channels: usize,
}

impl OutputSide {
    #[inline]
    fn on_data(&mut self, data: &mut Data, info: &OutputCallbackInfo) {
        let ts = info.timestamp();
        let callback_ns = ts.callback.as_nanos() as u64;
        let playback_ns = ts.playback.as_nanos() as u64;
        match data.sample_format() {
            cpal::SampleFormat::F32 => self.write::<f32>(data, callback_ns, playback_ns),
            cpal::SampleFormat::I32 => self.write::<i32>(data, callback_ns, playback_ns),
            cpal::SampleFormat::I24 => self.write::<I24>(data, callback_ns, playback_ns),
            cpal::SampleFormat::I16 => self.write::<i16>(data, callback_ns, playback_ns),
            _ => {}
        }
    }

    #[inline]
    fn write<T>(&mut self, data: &mut Data, callback_ns: u64, playback_ns: u64)
    where
        T: cpal::SizedSample + FromSample<f32>,
    {
        let Some(out) = data.as_slice_mut::<T>() else {
            return;
        };
        let ch = self.device_channels.max(1);
        let frames = (out.len() / ch) as u32;
        let (start, mut flags) = self.clock.advance(frames, playback_ns);
        flags |= self.latch.take();
        self.renderer.render(
            OutputStamp {
                start_sample: start,
                frames,
                flags,
                callback_ns,
                playback_ns: Some(playback_ns),
            },
            ch,
            |f, c, v| out[f * ch + c] = T::from_sample_(v),
        );
        // A trailing partial frame (never seen in practice) keeps cpal's equilibrium fill.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only: lists devices, opens nothing. Skips where the host has no devices (CI).
    #[test]
    fn enumeration_lists_devices_or_skips() {
        let devices = match CpalBackend::new().enumerate() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("skipping: cpal enumeration failed: {e}");
                return;
            }
        };
        if devices.is_empty() {
            eprintln!("skipping: no cpal devices");
            return;
        }
        for d in &devices {
            assert_eq!(d.backend, BackendKind::Cpal);
            assert_eq!(d.index, IndexExactness::Estimated);
            for caps in [&d.input, &d.output].into_iter().flatten() {
                assert!(caps.rates.iter().all(|r| r.min <= r.max));
                assert!(!caps.formats.is_empty());
            }
        }
    }
}
