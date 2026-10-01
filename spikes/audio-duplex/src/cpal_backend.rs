//! cpal backend: default host per OS (CoreAudio, WASAPI, ALSA).
//!
//! cpal has no duplex stream: input and output are two streams with their own callbacks
//! (and, except CoreAudio's HAL IO thread, their own threads). The capture side owns the
//! sample index; the output side only renders silence (or a capped tone) and records its
//! own timing. cpal does not convert formats, so both callbacks convert from/to the
//! device's native format here.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Data, ErrorKind, FromSample, I24, InputCallbackInfo, OutputCallbackInfo,
    SampleFormat, StreamConfig, SupportedBufferSize, SupportedStreamConfigRange,
};

use crate::backend::{
    AudioBackend, AudioError, BackendEvents, BackendKind, BufferRange, ClockRelation, DeviceCaps,
    DirectionCaps, DuplexRequest, DuplexStream, EventLatch, Negotiated, RateRange, StaticLatency,
};
use crate::block::{BlockHeader, BlockProducer, transport};
use crate::clock::TimestampClock;
use crate::output::{OutputControl, OutputRenderer};

/// Formats this spike converts; preference order when a device offers several.
const FORMAT_PREFERENCE: [SampleFormat; 4] = [
    SampleFormat::F32,
    SampleFormat::I32,
    SampleFormat::I24,
    SampleFormat::I16,
];

#[derive(Debug, Default)]
pub struct CpalBackend {
    /// Host name as cpal spells it (`alsa`, `coreaudio`, `wasapi`); `None` = default host.
    pub host: Option<String>,
}

impl CpalBackend {
    fn host(&self) -> Result<cpal::Host, AudioError> {
        match &self.host {
            None => Ok(cpal::default_host()),
            Some(name) => {
                let id = cpal::available_hosts()
                    .into_iter()
                    .find(|h| h.name().eq_ignore_ascii_case(name))
                    .ok_or_else(|| AudioError::NoDevice(format!("cpal host {name}")))?;
                cpal::host_from_id(id).map_err(|e| AudioError::Backend(e.to_string()))
            }
        }
    }
}

fn device_id(d: &cpal::Device) -> String {
    d.id().map(|i| i.id().to_string()).unwrap_or_default()
}

fn device_name(d: &cpal::Device) -> String {
    d.description()
        .map(|desc| desc.name().to_string())
        .unwrap_or_else(|_| device_id(d))
}

fn direction_caps(
    ranges: Result<impl Iterator<Item = SupportedStreamConfigRange>, cpal::Error>,
    default: Result<cpal::SupportedStreamConfig, cpal::Error>,
    notes: &mut Vec<String>,
    what: &str,
) -> Option<DirectionCaps> {
    let ranges: Vec<_> = match ranges {
        Ok(r) => r.collect(),
        Err(e) => {
            notes.push(format!("{what} configs: {e}"));
            return None;
        }
    };
    if ranges.is_empty() {
        return None;
    }
    let mut rates: Vec<RateRange> = ranges
        .iter()
        .map(|r| RateRange {
            min: r.min_sample_rate(),
            max: r.max_sample_rate(),
        })
        .collect();
    rates.sort_by_key(|r| (r.min, r.max));
    rates.dedup();
    let mut formats: Vec<String> = ranges
        .iter()
        .map(|r| r.sample_format().to_string())
        .collect();
    formats.sort();
    formats.dedup();
    let buffer_frames = ranges.iter().find_map(|r| match r.buffer_size() {
        SupportedBufferSize::Range { min, max } => Some(BufferRange {
            min: *min,
            max: *max,
        }),
        SupportedBufferSize::Unknown => None,
    });
    let default = default.ok();
    Some(DirectionCaps {
        max_channels: ranges.iter().map(|r| r.channels()).max().unwrap_or(0),
        rates,
        buffer_frames,
        sample_formats: formats,
        default_rate: default.as_ref().map(|d| d.sample_rate()),
        default_channels: default.as_ref().map(|d| d.channels()),
    })
}

/// Pick a config with at least `min_channels` at `rate`, preferring formats we convert
/// cheaply and the fewest surplus channels.
fn pick_config(
    ranges: Vec<SupportedStreamConfigRange>,
    rate: u32,
    min_channels: u16,
) -> Option<(SupportedStreamConfigRange, SampleFormat)> {
    let mut best: Option<(usize, u16, SupportedStreamConfigRange)> = None;
    for r in ranges {
        let Some(pref) = FORMAT_PREFERENCE
            .iter()
            .position(|f| *f == r.sample_format())
        else {
            continue;
        };
        if !r.contains_rate(rate) || r.channels() < min_channels {
            continue;
        }
        let key = (pref, r.channels());
        if best.as_ref().is_none_or(|(p, c, _)| key < (*p, *c)) {
            best = Some((key.0, key.1, r));
        }
    }
    best.map(|(_, _, r)| {
        let f = r.sample_format();
        (r, f)
    })
}

fn duplex_relation(d: &cpal::Device) -> ClockRelation {
    if d.supports_input() && d.supports_output() {
        // CoreAudio and ALSA use one device object/PCM name for both directions; WASAPI
        // endpoints are single-direction, so they never reach this arm.
        ClockRelation::SameDeviceSeparateCallbacks
    } else {
        ClockRelation::Unknown
    }
}

impl AudioBackend for CpalBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Cpal
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let host = self.host()?;
        let host_id = host.id();
        let devices = host
            .devices()
            .map_err(|e| AudioError::Backend(e.to_string()))?;
        let mut out = Vec::new();
        for d in devices {
            let mut notes = Vec::new();
            let input = if d.supports_input() {
                direction_caps(
                    d.supported_input_configs(),
                    d.default_input_config(),
                    &mut notes,
                    "input",
                )
            } else {
                None
            };
            let output = if d.supports_output() {
                direction_caps(
                    d.supported_output_configs(),
                    d.default_output_config(),
                    &mut notes,
                    "output",
                )
            } else {
                None
            };
            out.push(DeviceCaps {
                backend: BackendKind::Cpal,
                host: host_id.to_string(),
                id: device_id(&d),
                name: device_name(&d),
                input,
                output,
                duplex_clock: duplex_relation(&d),
                latency: StaticLatency::PerCallbackTimestamps,
                notes,
            });
        }
        Ok(out)
    }

    fn open_duplex(&self, req: &DuplexRequest) -> Result<DuplexStream, AudioError> {
        let host = self.host()?;
        let find = |want: &Option<String>, input: bool| -> Result<cpal::Device, AudioError> {
            match want {
                None => if input {
                    host.default_input_device()
                } else {
                    host.default_output_device()
                }
                .ok_or_else(|| AudioError::NoDevice("no default device".into())),
                Some(w) => host
                    .devices()
                    .map_err(|e| AudioError::Backend(e.to_string()))?
                    .find(|d| {
                        (device_id(d) == *w || device_name(d) == *w)
                            && if input {
                                d.supports_input()
                            } else {
                                d.supports_output()
                            }
                    })
                    .ok_or_else(|| AudioError::NoDevice(w.clone())),
            }
        };
        let in_dev = find(&req.input_device, true)?;
        let out_dev = match (&req.output_device, in_dev.supports_output()) {
            // Same device for both directions keeps one clock where the host allows it.
            (None, true) => in_dev.clone(),
            _ => find(&req.output_device, false)?,
        };

        let need_in = req.input_map.iter().copied().max().map_or(1, |m| m + 1);
        let rate = match req.sample_rate {
            Some(r) => r,
            None => in_dev
                .default_input_config()
                .map_err(|e| AudioError::Backend(e.to_string()))?
                .sample_rate(),
        };
        let in_ranges: Vec<_> = in_dev
            .supported_input_configs()
            .map_err(|e| AudioError::Backend(e.to_string()))?
            .collect();
        let out_ranges: Vec<_> = out_dev
            .supported_output_configs()
            .map_err(|e| AudioError::Backend(e.to_string()))?
            .collect();
        let (in_range, in_fmt) = pick_config(in_ranges, rate, need_in).ok_or_else(|| {
            AudioError::Unsupported(format!("no input config with {need_in} ch at {rate} Hz"))
        })?;
        let (out_range, out_fmt) = pick_config(out_ranges, rate, req.output_channels.max(1))
            .ok_or_else(|| {
                AudioError::Unsupported(format!(
                    "no output config with {} ch at {rate} Hz",
                    req.output_channels
                ))
            })?;
        let buffer_size = req
            .buffer_frames
            .map_or(BufferSize::Default, BufferSize::Fixed);
        let in_cfg = StreamConfig {
            channels: in_range.channels(),
            sample_rate: rate,
            buffer_size,
        };
        let out_cfg = StreamConfig {
            channels: out_range.channels(),
            sample_rate: rate,
            buffer_size,
        };

        let channels = req.input_map.len() as u16;
        let min_block = req.buffer_frames.unwrap_or(64).max(16) as usize;
        let cap_frames = (req.ring_seconds * f64::from(rate)) as usize;
        let (producer, consumer) = transport(channels, cap_frames, min_block);
        let events = Arc::new(BackendEvents::default());
        let control = Arc::new(OutputControl::default());
        let (tick_p, tick_c) = rtrb::RingBuffer::new(4096);

        let mut input = InputState {
            producer,
            clock: TimestampClock::new(rate),
            latch: EventLatch::new(Arc::clone(&events)),
            map: req.input_map.iter().map(|&c| usize::from(c)).collect(),
            device_channels: usize::from(in_cfg.channels),
        };
        let ev_in = Arc::clone(&events);
        let in_stream = in_dev
            .build_input_stream_raw(
                in_cfg,
                in_fmt,
                move |data: &Data, info: &InputCallbackInfo| input.on_data(data, info),
                move |e| on_error(&ev_in, &e),
                None,
            )
            .map_err(|e| AudioError::Backend(format!("input stream: {e}")))?;

        let mut renderer = OutputRenderer::new(req.output, rate, Arc::clone(&control), tick_p);
        let out_ch = usize::from(out_cfg.channels);
        let ev_out = Arc::clone(&events);
        let out_stream = out_dev
            .build_output_stream_raw(
                out_cfg,
                out_fmt,
                move |data: &mut Data, info: &OutputCallbackInfo| {
                    render_output(&mut renderer, out_ch, data, info)
                },
                move |e| on_error(&ev_out, &e),
                None,
            )
            .map_err(|e| AudioError::Backend(format!("output stream: {e}")))?;

        out_stream
            .play()
            .map_err(|e| AudioError::Backend(e.to_string()))?;
        in_stream
            .play()
            .map_err(|e| AudioError::Backend(e.to_string()))?;

        let same = device_id(&in_dev) == device_id(&out_dev);
        let negotiated = Negotiated {
            backend: BackendKind::Cpal,
            input_device: device_id(&in_dev),
            output_device: device_id(&out_dev),
            sample_rate: rate,
            input_channels: channels,
            device_input_channels: in_cfg.channels,
            output_channels: out_cfg.channels,
            buffer_frames: in_stream.buffer_size().ok(),
            input_format: in_fmt.to_string(),
            output_format: out_fmt.to_string(),
            clock: if same {
                ClockRelation::SameDeviceSeparateCallbacks
            } else {
                ClockRelation::Unknown
            },
            latency: StaticLatency::PerCallbackTimestamps,
        };
        Ok(DuplexStream::new(
            negotiated,
            consumer,
            tick_c,
            control,
            events,
            req.output,
            Box::new((in_stream, out_stream)),
        ))
    }
}

/// Runs on a non-RT notification path on most hosts; counters only.
fn on_error(events: &BackendEvents, e: &cpal::Error) {
    let counter = match e.kind() {
        ErrorKind::Xrun => &events.xruns,
        ErrorKind::DeviceChanged | ErrorKind::StreamInvalidated => &events.config_changes,
        _ => &events.errors,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

struct InputState {
    producer: BlockProducer,
    clock: TimestampClock,
    latch: EventLatch,
    map: Vec<usize>,
    device_channels: usize,
}

impl InputState {
    #[inline]
    fn on_data(&mut self, data: &Data, info: &InputCallbackInfo) {
        match data.sample_format() {
            SampleFormat::F32 => self.push::<f32>(data, info),
            SampleFormat::I32 => self.push::<i32>(data, info),
            SampleFormat::I24 => self.push::<I24>(data, info),
            SampleFormat::I16 => self.push::<i16>(data, info),
            // pick_config only selects the formats above.
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
        flags |= self.latch.take_flags();
        let header = BlockHeader {
            start_sample: start,
            frames,
            channels: 0,
            flags,
            callback_ns,
            capture_ns: Some(capture_ns),
        };
        let (map, dc) = (&self.map, self.device_channels);
        self.producer.push_with(header, |f, c| {
            <f32 as FromSample<T>>::from_sample_(src[f * dc + map[c]])
        });
    }
}

#[inline]
fn render_output(
    renderer: &mut OutputRenderer,
    channels: usize,
    data: &mut Data,
    info: &OutputCallbackInfo,
) {
    let ts = info.timestamp();
    let cb = ts.callback.as_nanos() as u64;
    let pb = Some(ts.playback.as_nanos() as u64);
    match data.sample_format() {
        SampleFormat::F32 => write_out::<f32>(renderer, channels, data, cb, pb),
        SampleFormat::I32 => write_out::<i32>(renderer, channels, data, cb, pb),
        SampleFormat::I24 => write_out::<I24>(renderer, channels, data, cb, pb),
        SampleFormat::I16 => write_out::<i16>(renderer, channels, data, cb, pb),
        _ => {}
    }
}

#[inline]
fn write_out<T>(
    renderer: &mut OutputRenderer,
    channels: usize,
    data: &mut Data,
    cb: u64,
    pb: Option<u64>,
) where
    T: cpal::SizedSample + FromSample<f32>,
{
    let Some(out) = data.as_slice_mut::<T>() else {
        return;
    };
    let frames = out.len() / channels.max(1);
    renderer.render(frames, channels, cb, pb, |f, c, v| {
        out[f * channels + c] = T::from_sample_(v);
    });
    // Any trailing partial frame stays at cpal's pre-filled equilibrium.
}
