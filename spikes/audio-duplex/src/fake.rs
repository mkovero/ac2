//! Hardware-free backend: a thread plays the role of a single duplex callback, with a
//! deterministic loopback (output channel 0 → input channel 0 after a fixed delay) and
//! scriptable faults. It drives exactly the same block/clock/output code as real backends.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::backend::{
    AudioBackend, AudioError, BackendEvents, BackendKind, BufferRange, ClockRelation, DeviceCaps,
    DirectionCaps, DuplexRequest, DuplexStream, EventLatch, Negotiated, RateRange, StaticLatency,
};
use crate::block::{BlockHeader, transport};
use crate::clock::FrameCounterClock;
use crate::output::{OutputControl, OutputRenderer};

#[derive(Clone, Debug)]
pub struct FakeConfig {
    pub sample_rate: u32,
    pub period: u32,
    pub device_inputs: u16,
    pub device_outputs: u16,
    /// Loopback delay from output channel 0 to input channel 0, frames.
    pub loopback_delay: usize,
    /// Pace callbacks in real time (for `--run`); tests run as fast as possible.
    pub realtime: bool,
    /// Stop producing after this many callbacks (tests), `None` = until dropped.
    pub max_callbacks: Option<u64>,
    /// At callback index `.0`, the device "loses" `.1` frames and reports an xrun.
    pub xrun_at: Option<(u64, u32)>,
}

impl Default for FakeConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            period: 256,
            device_inputs: 4,
            device_outputs: 2,
            loopback_delay: 1000,
            realtime: true,
            max_callbacks: None,
            xrun_at: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct FakeBackend {
    pub config: FakeConfig,
}

struct FakeGuard {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for FakeGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Deterministic content for non-loopback channels so tests can check routing.
pub fn fake_channel_value(device_channel: u16, sample: u64) -> f32 {
    (device_channel as f32 + 1.0) * 1000.0 + (sample % 1000) as f32
}

impl AudioBackend for FakeBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Fake
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let c = &self.config;
        let dir = |ch: u16| DirectionCaps {
            max_channels: ch,
            rates: vec![RateRange {
                min: c.sample_rate,
                max: c.sample_rate,
            }],
            buffer_frames: Some(BufferRange {
                min: c.period,
                max: c.period,
            }),
            sample_formats: vec!["f32".into()],
            default_rate: Some(c.sample_rate),
            default_channels: Some(ch),
        };
        Ok(vec![DeviceCaps {
            backend: BackendKind::Fake,
            host: "fake".into(),
            id: "fake".into(),
            name: "fake loopback".into(),
            input: Some(dir(c.device_inputs)),
            output: Some(dir(c.device_outputs)),
            duplex_clock: ClockRelation::SingleCallback,
            latency: StaticLatency::PortRanges {
                capture: (0, 0),
                playback: (0, 0),
            },
            notes: vec![],
        }])
    }

    fn open_duplex(&self, req: &DuplexRequest) -> Result<DuplexStream, AudioError> {
        let c = self.config.clone();
        if let Some(&bad) = req.input_map.iter().find(|&&ch| ch >= c.device_inputs) {
            return Err(AudioError::Unsupported(format!(
                "input channel {bad} >= device inputs {}",
                c.device_inputs
            )));
        }
        if req.output_channels > c.device_outputs {
            return Err(AudioError::Unsupported("too many output channels".into()));
        }
        if req.sample_rate.is_some_and(|r| r != c.sample_rate) {
            return Err(AudioError::Unsupported("fake runs at one rate".into()));
        }
        let channels = req.input_map.len() as u16;
        let cap_frames = (req.ring_seconds * f64::from(c.sample_rate)) as usize;
        let (mut producer, consumer) = transport(channels, cap_frames, c.period as usize);
        let (tick_p, tick_c) = rtrb::RingBuffer::new(1024);
        let control = Arc::new(OutputControl::default());
        let events = Arc::new(BackendEvents::default());
        let mut renderer =
            OutputRenderer::new(req.output, c.sample_rate, Arc::clone(&control), tick_p);
        let mut latch = EventLatch::new(Arc::clone(&events));
        let stop = Arc::new(AtomicBool::new(false));
        let input_map = req.input_map.clone();
        let out_ch = usize::from(req.output_channels.max(1));
        let ev = Arc::clone(&events);
        let stop_t = Arc::clone(&stop);

        let thread = std::thread::Builder::new()
            .name("fake-audio".into())
            .spawn(move || {
                let period = c.period as usize;
                // All buffers allocated before the "callback" loop.
                let mut out_buf = vec![0.0f32; period * out_ch];
                let ring_len = c.loopback_delay + period;
                let mut delay_line = vec![0.0f32; ring_len.max(1)];
                let mut delay_pos = 0usize;
                let mut loop_buf = vec![0.0f32; period];
                let mut clock = FrameCounterClock::new();
                let mut device_frame: u32 = 7_000; // arbitrary non-zero origin
                let t0 = Instant::now();
                let mut n: u64 = 0;
                while !stop_t.load(Ordering::Acquire) && c.max_callbacks.is_none_or(|m| n < m) {
                    if let Some((at, lost)) = c.xrun_at
                        && at == n
                    {
                        device_frame = device_frame.wrapping_add(lost);
                        ev.xruns.fetch_add(1, Ordering::Relaxed);
                    }
                    let cb_ns = t0.elapsed().as_nanos() as u64;
                    renderer.render(period, out_ch, cb_ns, Some(cb_ns), |f, ch, v| {
                        out_buf[f * out_ch + ch] = v;
                    });
                    for (f, slot) in loop_buf.iter_mut().enumerate() {
                        delay_line[delay_pos] = out_buf[f * out_ch];
                        let read = (delay_pos + ring_len - c.loopback_delay) % ring_len;
                        *slot = delay_line[read];
                        delay_pos = (delay_pos + 1) % ring_len;
                    }
                    let (start, mut flags) = clock.advance(device_frame, c.period);
                    flags |= latch.take_flags();
                    let header = BlockHeader {
                        start_sample: start,
                        frames: c.period,
                        channels: 0,
                        flags,
                        callback_ns: cb_ns,
                        capture_ns: Some(cb_ns),
                    };
                    producer.push_with(header, |f, i| {
                        let dev = input_map[i];
                        if dev == 0 {
                            loop_buf[f]
                        } else {
                            fake_channel_value(dev, start + f as u64)
                        }
                    });
                    device_frame = device_frame.wrapping_add(c.period);
                    n += 1;
                    if c.realtime {
                        let due = Duration::from_secs_f64(
                            (n * u64::from(c.period)) as f64 / f64::from(c.sample_rate),
                        );
                        if let Some(wait) = due.checked_sub(t0.elapsed()) {
                            std::thread::sleep(wait);
                        }
                    }
                }
            })
            .map_err(|e| AudioError::Backend(e.to_string()))?;

        let negotiated = Negotiated {
            backend: BackendKind::Fake,
            input_device: "fake".into(),
            output_device: "fake".into(),
            sample_rate: self.config.sample_rate,
            input_channels: channels,
            device_input_channels: self.config.device_inputs,
            output_channels: req.output_channels,
            buffer_frames: Some(self.config.period),
            input_format: "f32".into(),
            output_format: "f32".into(),
            clock: ClockRelation::SingleCallback,
            latency: StaticLatency::None,
        };
        Ok(DuplexStream::new(
            negotiated,
            consumer,
            tick_c,
            control,
            events,
            req.output,
            Box::new(FakeGuard {
                stop,
                thread: Some(thread),
            }),
        ))
    }
}
