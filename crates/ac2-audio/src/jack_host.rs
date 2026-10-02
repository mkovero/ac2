//! JACK backend through the `jack` crate.
//!
//! One process callback services capture and playback on the server's clock, and the
//! server's frame counter gives exact indices. Output and capture of one cycle therefore
//! share one index, so the generator→loopback offset is fixed for the life of the client.
//! cpal's JACK host is not used: it creates one client per stream and loses that property.
//!
//! Rate and buffer size belong to the server; a request can only confirm them.

use std::sync::Arc;

use jack::{
    AudioIn, AudioOut, Client, ClientOptions, ClientStatus, Control, Frames, LatencyType,
    PortFlags, ProcessScope,
};

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
use crate::stream::{DuplexStream, Plumbing, StreamParts};

/// Ports per direction. The callback gathers port buffers into fixed arrays of this size,
/// so it never allocates.
pub const MAX_PORTS: usize = 64;
/// The only device id JACK lists: the server.
pub const JACK_DEVICE_ID: &str = "jack";
const AUDIO_TYPE: &str = "32 bit float mono audio";

/// JACK client settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JackConfig {
    /// Client name shown in the graph.
    pub client_name: String,
    /// Connect our input ports to the physical capture ports named by the input map.
    pub connect_inputs: bool,
    /// Connect our output ports to the physical playback ports in order. Off by default:
    /// automated runs must never reach real speakers.
    pub connect_outputs: bool,
}

impl Default for JackConfig {
    fn default() -> Self {
        Self {
            client_name: "ac2".into(),
            connect_inputs: true,
            connect_outputs: false,
        }
    }
}

/// The JACK backend.
#[derive(Debug, Clone, Default)]
pub struct JackBackend {
    config: JackConfig,
}

impl JackBackend {
    /// A backend with `config`.
    pub fn new(config: JackConfig) -> Self {
        Self { config }
    }
}

fn unavailable(e: impl std::fmt::Display) -> AudioError {
    AudioError::Unavailable {
        backend: BackendKind::Jack,
        reason: e.to_string(),
    }
}

fn backend_err(operation: Operation, e: impl std::fmt::Display) -> AudioError {
    AudioError::Backend {
        backend: BackendKind::Jack,
        operation,
        detail: e.to_string(),
    }
}

fn connect(name: &str) -> Result<Client, AudioError> {
    Client::new(name, ClientOptions::NO_START_SERVER)
        .map(|(c, _)| c)
        .map_err(|e| match e {
            // The one failure an operator meets in practice; the status bits say nothing
            // more useful to them.
            jack::Error::ClientError(s) if s.contains(ClientStatus::SERVER_FAILED) => {
                unavailable("JACK server not running")
            }
            e => unavailable(e),
        })
}

/// What an operator calls a physical port: its short name (`capture_1`, or `capture_FL`
/// where the server names channels), unless an alias names the jack itself (`Mic1_in` from
/// a driver that knows its front panel). Aliases that only number the port again
/// (`alsa_pcm:hw:USB:in1`, `dummy_pcm:dummy:out1`) add nothing and are skipped.
fn port_label(client: &Client, full: &str) -> String {
    let tail = |s: &str| s.rsplit(':').next().unwrap_or(s).to_owned();
    let Some(port) = client.port_by_name(full) else {
        return tail(full);
    };
    let descriptive = port
        .aliases()
        .unwrap_or_default()
        .iter()
        .map(|a| tail(a))
        .find(|a| !generic_port_name(a));
    descriptive.unwrap_or_else(|| port.short_name().unwrap_or_else(|_| tail(full)))
}

/// `in1`, `out_2`, `capture_3`, `playback4`: a direction and a number, nothing more.
fn generic_port_name(name: &str) -> bool {
    let stem = name
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches(['_', '-', ' '])
        .to_ascii_lowercase();
    stem.is_empty()
        || ["in", "out", "input", "output", "capture", "playback"].contains(&stem.as_str())
}

/// Physical ports for capture (`true`) or playback (`false`). A physical capture source is
/// an *output* port from the graph's point of view.
fn physical(client: &Client, capture: bool) -> Vec<String> {
    let dir = if capture {
        PortFlags::IS_OUTPUT
    } else {
        PortFlags::IS_INPUT
    };
    client.ports(None, Some(AUDIO_TYPE), PortFlags::IS_PHYSICAL | dir)
}

/// Widest latency range over `ports`.
fn latency(client: &Client, ports: &[String], mode: LatencyType) -> FrameRange {
    let (min, max) = ports
        .iter()
        .filter_map(|p| client.port_by_name(p))
        .map(|p| p.get_latency_range(mode))
        .fold((u32::MAX, 0), |(a, b), (lo, hi)| (a.min(lo), b.max(hi)));
    FrameRange {
        min: if min == u32::MAX { 0 } else { min },
        max,
    }
}

fn static_latency(client: &Client, capture: &[String], playback: &[String]) -> StaticLatency {
    StaticLatency::PortRanges {
        capture: latency(client, capture, LatencyType::Capture),
        playback: latency(client, playback, LatencyType::Playback),
    }
}

fn check_selector(selector: &DeviceSelector, direction: Direction) -> Result<(), AudioError> {
    match selector {
        DeviceSelector::Id(id) if id.0 != JACK_DEVICE_ID => Err(AudioError::DeviceNotFound {
            direction,
            selector: selector.to_string(),
        }),
        _ => Ok(()),
    }
}

impl Backend for JackBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Jack
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let client = connect(&format!("{}-enum", self.config.client_name))?;
        let rate = client.sample_rate();
        let frames = client.buffer_size();
        let capture = physical(&client, true);
        let playback = physical(&client, false);
        let labels = |ports: &[String]| -> Vec<String> {
            ports
                .iter()
                .take(MAX_PORTS)
                .map(|p| port_label(&client, p))
                .collect()
        };
        let (capture_names, playback_names) = (labels(&capture), labels(&playback));
        let dir = |n: usize, names: &Vec<String>| DirectionCaps {
            max_channels: n.min(MAX_PORTS) as u16,
            rates: vec![RateRange {
                min: rate,
                max: rate,
            }],
            buffer_frames: Some(FrameRange {
                min: frames,
                max: frames,
            }),
            formats: vec![SampleFormat::F32],
            default_rate: Some(rate),
            default_buffer: Some(frames),
            channel_names: Some(names.clone()),
        };
        Ok(vec![DeviceCaps {
            backend: BackendKind::Jack,
            host: "jack".into(),
            id: DeviceId(JACK_DEVICE_ID.into()),
            name: format!(
                "JACK server ({} capture, {} playback ports)",
                capture.len(),
                playback.len()
            ),
            input: Some(dir(capture.len(), &capture_names)),
            output: Some(dir(playback.len(), &playback_names)),
            duplex_clock: ClockRelation::SingleCallback,
            index: IndexExactness::Exact,
            latency: static_latency(&client, &capture, &playback),
            notes: vec!["rate and buffer size are the server's".into()],
        }])
    }

    fn open(&self, mut request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        request.validate()?;
        check_selector(&request.input_device, Direction::Input)?;
        check_selector(&request.output_device, Direction::Output)?;
        for n in [
            request.input_map.len(),
            usize::from(request.output_channels),
        ] {
            if n > MAX_PORTS {
                return Err(Unsupported::TooManyChannels {
                    requested: n,
                    max: MAX_PORTS,
                }
                .into());
            }
        }
        let client = connect(&self.config.client_name)?;
        let rate = client.sample_rate();
        if let Some(r) = request.sample_rate.filter(|&r| r != rate) {
            return Err(Unsupported::SampleRate {
                requested: r,
                offered: format!("{rate} Hz (server)"),
            }
            .into());
        }
        let frames = client.buffer_size();
        if let Some(b) = request.buffer_frames.filter(|&b| b != frames) {
            return Err(Unsupported::BufferFrames {
                requested: b,
                fixed: Some(frames),
            }
            .into());
        }
        let capture = physical(&client, true);
        let playback = physical(&client, false);
        if self.config.connect_inputs
            && let Some(&ch) = request
                .input_map
                .iter()
                .find(|&&c| usize::from(c) >= capture.len())
        {
            return Err(Unsupported::InputChannel {
                channel: ch,
                available: capture.len() as u16,
            }
            .into());
        }

        let mut in_ports = Vec::with_capacity(request.input_map.len());
        for i in 0..request.input_map.len() {
            in_ports.push(
                client
                    .register_port(&format!("in_{}", i + 1), AudioIn::default())
                    .map_err(|e| backend_err(Operation::Open, e))?,
            );
        }
        let mut out_ports = Vec::with_capacity(usize::from(request.output_channels));
        for i in 0..request.output_channels {
            out_ports.push(
                client
                    .register_port(&format!("out_{}", i + 1), AudioOut::default())
                    .map_err(|e| backend_err(Operation::Open, e))?,
            );
        }
        let in_names: Vec<String> = in_ports.iter().filter_map(|p| p.name().ok()).collect();
        let out_names: Vec<String> = out_ports.iter().filter_map(|p| p.name().ok()).collect();

        let used_capture: Vec<String> = request
            .input_map
            .iter()
            .filter_map(|&c| capture.get(usize::from(c)).cloned())
            .collect();
        let used_playback: Vec<String> = playback
            .iter()
            .take(usize::from(request.output_channels))
            .cloned()
            .collect();
        let cap_lat = latency(&client, &used_capture, LatencyType::Capture);
        let play_lat = latency(&client, &used_playback, LatencyType::Playback);
        let frames_to_ns = |f: u32| u64::from(f) * 1_000_000_000 / u64::from(rate);

        let plumbing = Plumbing::new(&mut request, rate, frames as usize);
        let events = Arc::clone(&plumbing.events);
        let process = Process {
            in_ports,
            out_ports,
            producer: plumbing.producer,
            renderer: plumbing.renderer,
            clock: FrameCounterClock::new(),
            latch: EventLatch::new(Arc::clone(&events)),
            events: Arc::clone(&events),
            capture_latency_ns: frames_to_ns(cap_lat.max),
            playback_latency_ns: frames_to_ns(play_lat.max),
            buffer_frames: frames,
        };
        let notifications = Notifications {
            events: Arc::clone(&events),
            sample_rate: rate,
        };
        let active = client
            .activate_async(notifications, process)
            .map_err(|e| backend_err(Operation::Start, e))?;

        let c = active.as_client();
        if self.config.connect_inputs {
            for (ours, &dev) in in_names.iter().zip(&request.input_map) {
                if let Some(src) = capture.get(usize::from(dev)) {
                    c.connect_ports_by_name(src, ours)
                        .map_err(|e| backend_err(Operation::Connect, e))?;
                }
            }
        }
        if self.config.connect_outputs {
            for (ours, dst) in out_names.iter().zip(&playback) {
                c.connect_ports_by_name(ours, dst)
                    .map_err(|e| backend_err(Operation::Connect, e))?;
            }
        }

        let negotiated = Negotiated {
            backend: BackendKind::Jack,
            input_device: DeviceId(JACK_DEVICE_ID.into()),
            output_device: DeviceId(JACK_DEVICE_ID.into()),
            sample_rate: rate,
            input_channels: request.input_map.len() as u16,
            device_input_channels: capture.len() as u16,
            output_channels: request.output_channels,
            device_output_channels: request.output_channels,
            buffer_frames: Some(frames),
            input_format: SampleFormat::F32,
            output_format: Some(SampleFormat::F32),
            clock: ClockRelation::SingleCallback,
            index: IndexExactness::Exact,
            latency: StaticLatency::PortRanges {
                capture: cap_lat,
                playback: play_lat,
            },
        };
        // Output written in cycle n leaves at the next cycle boundary, then passes the
        // playback latency.
        let drain =
            std::time::Duration::from_nanos(frames_to_ns(2 * frames + play_lat.max) + 10_000_000);
        Ok(DuplexStream::new(StreamParts {
            negotiated,
            capture: plumbing.consumer,
            output: plumbing.output,
            events,
            drain,
            guard: Box::new(active),
        }))
    }
}

struct Notifications {
    events: Arc<BackendEvents>,
    sample_rate: Frames,
}

impl jack::NotificationHandler for Notifications {
    fn xrun(&mut self, _: &Client) -> Control {
        self.events.xrun();
        Control::Continue
    }

    fn sample_rate(&mut self, _: &Client, rate: Frames) -> Control {
        // The server also announces the unchanged rate at activation.
        if rate != self.sample_rate {
            self.sample_rate = rate;
            self.events.config_change();
        }
        Control::Continue
    }

    // The trait declares this `unsafe` because it runs from a signal-like context where
    // the client must not be used; only an atomic is touched here.
    #[allow(unsafe_code)]
    unsafe fn shutdown(&mut self, _: ClientStatus, _: &str) {
        self.events.end();
    }
}

struct Process {
    in_ports: Vec<jack::Port<AudioIn>>,
    out_ports: Vec<jack::Port<AudioOut>>,
    producer: BlockProducer,
    renderer: OutputRenderer,
    clock: FrameCounterClock,
    latch: EventLatch,
    events: Arc<BackendEvents>,
    capture_latency_ns: u64,
    playback_latency_ns: u64,
    buffer_frames: Frames,
}

impl jack::ProcessHandler for Process {
    fn process(&mut self, client: &Client, ps: &ProcessScope) -> Control {
        // jack_get_time: a vDSO clock read, no syscall. Used as the callback time.
        let wake_ns = client.time() * 1000;
        let frames = ps.n_frames();
        let (start, mut flags) = self.clock.advance(ps.last_frame_time(), frames);
        flags |= self.latch.take();
        // The engine's filtered cycle times live in shared memory (no syscall). Cycle start
        // is when the period boundary hit the driver, the best ADC time available.
        let (cycle_ns, next_ns) = ps
            .cycle_times()
            .map(|t| (t.current_usecs * 1000, t.next_usecs * 1000))
            .unwrap_or((wake_ns, wake_ns));

        let mut ins: [&[f32]; MAX_PORTS] = [&[]; MAX_PORTS];
        for (slot, p) in ins.iter_mut().zip(&self.in_ports) {
            *slot = p.as_slice(ps);
        }
        self.producer.push_with(
            BlockStamp {
                start_sample: start,
                frames,
                flags,
                callback_ns: wake_ns,
                capture_ns: Some(cycle_ns.saturating_sub(self.capture_latency_ns)),
            },
            |f, c| ins[c].get(f).copied().unwrap_or(0.0),
        );

        let mut outs: [&mut [f32]; MAX_PORTS] = std::array::from_fn(|_| <&mut [f32]>::default());
        let channels = self.out_ports.len();
        for (slot, p) in outs.iter_mut().zip(self.out_ports.iter_mut()) {
            *slot = p.as_mut_slice(ps);
        }
        self.renderer.render(
            OutputStamp {
                start_sample: start,
                frames,
                flags,
                callback_ns: wake_ns,
                playback_ns: Some(next_ns + self.playback_latency_ns),
            },
            channels,
            |f, c, v| {
                if let Some(s) = outs[c].get_mut(f) {
                    *s = v;
                }
            },
        );
        Control::Continue
    }

    fn buffer_size(&mut self, _: &Client, frames: Frames) -> Control {
        // The server also announces the unchanged size at activation.
        if frames != self.buffer_frames {
            self.buffer_frames = frames;
            self.events.config_change();
        }
        Control::Continue
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn numbered_aliases_are_not_names() {
        for g in ["in1", "out_2", "capture_3", "playback4", "12"] {
            assert!(super::generic_port_name(g), "{g}");
        }
        for n in ["Mic1_in", "capture_FL", "Front Left"] {
            assert!(!super::generic_port_name(n), "{n}");
        }
    }
}
