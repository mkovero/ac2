//! JACK backend via the `jack` crate: one process callback services capture and playback
//! on one clock, and the server's frame counter gives exact sample indices.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use jack::{
    AudioIn, AudioOut, Client, ClientOptions, Control, Frames, LatencyType, PortFlags, ProcessScope,
};

use crate::backend::{
    AudioBackend, AudioError, BackendEvents, BackendKind, BufferRange, ClockRelation,
    ConnectPolicy, DeviceCaps, DirectionCaps, DuplexRequest, DuplexStream, EventLatch, Negotiated,
    RateRange, StaticLatency,
};
use crate::block::{BlockHeader, BlockProducer, transport};
use crate::clock::FrameCounterClock;
use crate::output::{OutputControl, OutputRenderer};

/// Upper bound on ports per direction; lets the callback gather port buffers on the stack.
pub const MAX_PORTS: usize = 64;
const AUDIO_TYPE: &str = "32 bit float mono audio";

#[derive(Debug)]
pub struct JackBackend {
    pub client_name: String,
}

impl Default for JackBackend {
    fn default() -> Self {
        Self {
            client_name: "ac2-spike".into(),
        }
    }
}

fn connect(name: &str) -> Result<Client, AudioError> {
    Client::new(name, ClientOptions::NO_START_SERVER)
        .map(|(c, _)| c)
        .map_err(|e| AudioError::NoDevice(format!("jack server: {e}")))
}

fn physical(client: &Client, capture: bool) -> Vec<String> {
    // A physical capture source is an *output* port from the graph's point of view.
    let dir = if capture {
        PortFlags::IS_OUTPUT
    } else {
        PortFlags::IS_INPUT
    };
    client.ports(None, Some(AUDIO_TYPE), PortFlags::IS_PHYSICAL | dir)
}

/// Worst-case latency range over the given ports.
fn latency(client: &Client, ports: &[String], mode: LatencyType) -> (u32, u32) {
    ports
        .iter()
        .filter_map(|p| client.port_by_name(p))
        .map(|p| p.get_latency_range(mode))
        .fold((0, 0), |(a, b), (lo, hi)| (a.max(lo), b.max(hi)))
}

fn static_latency(client: &Client, caps: &[String], plays: &[String]) -> StaticLatency {
    StaticLatency::PortRanges {
        capture: latency(client, caps, LatencyType::Capture),
        playback: latency(client, plays, LatencyType::Playback),
    }
}

impl AudioBackend for JackBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Jack
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let client = connect(&format!("{}-enum", self.client_name))?;
        let rate = client.sample_rate();
        let bs = client.buffer_size();
        let caps = physical(&client, true);
        let plays = physical(&client, false);
        let dir = |n: usize| DirectionCaps {
            max_channels: n as u16,
            rates: vec![RateRange {
                min: rate,
                max: rate,
            }],
            buffer_frames: Some(BufferRange { min: bs, max: bs }),
            sample_formats: vec!["f32".into()],
            default_rate: Some(rate),
            default_channels: Some(n as u16),
        };
        Ok(vec![DeviceCaps {
            backend: BackendKind::Jack,
            host: "jack".into(),
            id: "jack".into(),
            name: format!(
                "JACK server ({} capture, {} playback)",
                caps.len(),
                plays.len()
            ),
            input: Some(dir(caps.len())),
            output: Some(dir(plays.len())),
            duplex_clock: ClockRelation::SingleCallback,
            latency: static_latency(&client, &caps, &plays),
            notes: vec![
                "rate and buffer size are the server's; a client cannot choose them".into(),
            ],
        }])
    }

    fn open_duplex(&self, req: &DuplexRequest) -> Result<DuplexStream, AudioError> {
        if req.input_map.len() > MAX_PORTS || usize::from(req.output_channels) > MAX_PORTS {
            return Err(AudioError::Unsupported(format!(
                "more than {MAX_PORTS} ports"
            )));
        }
        let client = connect(&self.client_name)?;
        let rate = client.sample_rate();
        if req.sample_rate.is_some_and(|r| r != rate) {
            return Err(AudioError::Unsupported(format!("server runs at {rate} Hz")));
        }
        let bs = client.buffer_size();
        if req.buffer_frames.is_some_and(|b| b != bs) {
            return Err(AudioError::Unsupported(format!(
                "server buffer is {bs} frames"
            )));
        }
        let caps = physical(&client, true);
        let plays = physical(&client, false);

        let mut in_ports = Vec::with_capacity(req.input_map.len());
        for i in 0..req.input_map.len() {
            in_ports.push(
                client
                    .register_port(&format!("in_{}", i + 1), AudioIn::default())
                    .map_err(|e| AudioError::Backend(e.to_string()))?,
            );
        }
        let mut out_ports = Vec::with_capacity(usize::from(req.output_channels));
        for i in 0..req.output_channels {
            out_ports.push(
                client
                    .register_port(&format!("out_{}", i + 1), AudioOut::default())
                    .map_err(|e| AudioError::Backend(e.to_string()))?,
            );
        }
        let in_names: Vec<String> = in_ports.iter().filter_map(|p| p.name().ok()).collect();
        let out_names: Vec<String> = out_ports.iter().filter_map(|p| p.name().ok()).collect();

        let used_caps: Vec<String> = req
            .input_map
            .iter()
            .filter_map(|&c| caps.get(usize::from(c)).cloned())
            .collect();
        let lat = static_latency(&client, &used_caps, &plays);
        let (capture_latency_frames, playback_latency_frames) = match lat {
            StaticLatency::PortRanges { capture, playback } => (capture.1, playback.1),
            _ => (0, 0),
        };
        let frames_to_ns = |f: u32| u64::from(f) * 1_000_000_000 / u64::from(rate);

        let channels = req.input_map.len() as u16;
        let cap_frames = (req.ring_seconds * f64::from(rate)) as usize;
        let (producer, consumer) = transport(channels, cap_frames, bs as usize);
        let events = Arc::new(BackendEvents::default());
        let control = Arc::new(OutputControl::default());
        let (tick_p, tick_c) = rtrb::RingBuffer::new(4096);

        let process = Process {
            in_ports,
            out_ports,
            producer,
            clock: FrameCounterClock::new(),
            latch: EventLatch::new(Arc::clone(&events)),
            renderer: OutputRenderer::new(req.output, rate, Arc::clone(&control), tick_p),
            events: Arc::clone(&events),
            capture_latency_ns: frames_to_ns(capture_latency_frames),
            playback_latency_ns: frames_to_ns(playback_latency_frames),
        };
        let notifications = Notifications {
            events: Arc::clone(&events),
        };
        let active = client
            .activate_async(notifications, process)
            .map_err(|e| AudioError::Backend(e.to_string()))?;

        let c = active.as_client();
        let mut notes = Vec::new();
        if req.connect != ConnectPolicy::None {
            for (ours, &dev) in in_names.iter().zip(&req.input_map) {
                match caps.get(usize::from(dev)) {
                    Some(src) => {
                        if let Err(e) = c.connect_ports_by_name(src, ours) {
                            notes.push(format!("connect {src} -> {ours}: {e}"));
                        }
                    }
                    None => notes.push(format!("no physical capture port {dev}")),
                }
            }
        }
        if req.connect == ConnectPolicy::Both {
            for (ours, dst) in out_names.iter().zip(&plays) {
                if let Err(e) = c.connect_ports_by_name(ours, dst) {
                    notes.push(format!("connect {ours} -> {dst}: {e}"));
                }
            }
        }
        for n in notes {
            eprintln!("jack: {n}");
        }

        let negotiated = Negotiated {
            backend: BackendKind::Jack,
            input_device: "jack".into(),
            output_device: "jack".into(),
            sample_rate: rate,
            input_channels: channels,
            device_input_channels: caps.len() as u16,
            output_channels: req.output_channels,
            buffer_frames: Some(bs),
            input_format: "f32".into(),
            output_format: "f32".into(),
            clock: ClockRelation::SingleCallback,
            latency: lat,
        };
        Ok(DuplexStream::new(
            negotiated,
            consumer,
            tick_c,
            control,
            events,
            req.output,
            Box::new(active),
        ))
    }
}

struct Notifications {
    events: Arc<BackendEvents>,
}

impl jack::NotificationHandler for Notifications {
    fn xrun(&mut self, _: &Client) -> Control {
        self.events.xruns.fetch_add(1, Ordering::Relaxed);
        Control::Continue
    }

    fn sample_rate(&mut self, _: &Client, _srate: Frames) -> Control {
        self.events.config_changes.fetch_add(1, Ordering::Relaxed);
        Control::Continue
    }
}

struct Process {
    in_ports: Vec<jack::Port<AudioIn>>,
    out_ports: Vec<jack::Port<AudioOut>>,
    producer: BlockProducer,
    clock: FrameCounterClock,
    latch: EventLatch,
    renderer: OutputRenderer,
    events: Arc<BackendEvents>,
    capture_latency_ns: u64,
    playback_latency_ns: u64,
}

impl jack::ProcessHandler for Process {
    fn process(&mut self, client: &Client, ps: &ProcessScope) -> Control {
        // Actual wake time (clock_gettime via vDSO) — measures scheduling jitter.
        let wake_ns = client.time() * 1000;
        let n = ps.n_frames();
        let (start, mut flags) = self.clock.advance(ps.last_frame_time(), n);
        flags |= self.latch.take_flags();
        // jack_get_cycle_times reads the engine's DLL-filtered cycle timing from shared
        // memory; no syscall. Cycle start is when the period boundary hit the driver.
        let (cycle_ns, next_ns) = ps
            .cycle_times()
            .map(|t| (t.current_usecs * 1000, t.next_usecs * 1000))
            .unwrap_or((wake_ns, wake_ns));

        let mut ins: [&[f32]; MAX_PORTS] = [&[]; MAX_PORTS];
        for (slot, p) in ins.iter_mut().zip(&self.in_ports) {
            *slot = p.as_slice(ps);
        }
        let header = BlockHeader {
            start_sample: start,
            frames: n,
            channels: 0,
            flags,
            callback_ns: wake_ns,
            capture_ns: Some(cycle_ns.saturating_sub(self.capture_latency_ns)),
        };
        self.producer.push_with(header, |f, c| ins[c][f]);

        let mut outs: [&mut [f32]; MAX_PORTS] = std::array::from_fn(|_| <&mut [f32]>::default());
        let m = self.out_ports.len();
        for (slot, p) in outs.iter_mut().zip(self.out_ports.iter_mut()) {
            *slot = p.as_mut_slice(ps);
        }
        // Output written now leaves the client at the next cycle boundary.
        let playback_ns = next_ns + self.playback_latency_ns;
        self.renderer
            .render(n as usize, m, wake_ns, Some(playback_ns), |f, c, v| {
                outs[c][f] = v
            });
        Control::Continue
    }

    fn buffer_size(&mut self, _: &Client, _size: Frames) -> Control {
        self.events.config_changes.fetch_add(1, Ordering::Relaxed);
        Control::Continue
    }
}
