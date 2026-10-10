//! JACK backend through the `jack` crate.
//!
//! One process callback services capture and playback on the server's clock, and the
//! server's frame counter gives exact indices. Output and capture of one cycle therefore
//! share one index, so the generator→loopback offset is fixed for the life of the client.
//! cpal's JACK host is not used: it creates one client per stream and loses that property.
//!
//! Rate and buffer size belong to the server; a request can only confirm them.
//!
//! On Linux this is the only real backend: it reaches JACK2 and PipeWire alike (PipeWire
//! through its own libjack, pipewire-jack). When no server answers, the reason names the
//! remedy, telling a PipeWire desktop without pipewire-jack apart from a machine with no
//! audio server at all.

use std::collections::BTreeMap;
use std::ffi::{CStr, c_char, c_int};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once, Weak};

use jack::{
    AudioIn, AudioOut, Client, ClientOptions, ClientStatus, Control, Frames, LatencyType,
    PortFlags, ProcessScope,
};

use crate::backend::{
    Backend, BackendKind, ClockRelation, Delivery, DeviceCaps, DeviceId, DeviceSelector, Direction,
    DirectionCaps, DuplexRequest, FrameRange, IndexExactness, Negotiated, Presence, RateRange,
    SampleFormat, StaticLatency,
};
use crate::block::{BlockProducer, BlockStamp};
use crate::clock::FrameCounterClock;
use crate::error::{AudioError, Operation, Unavailability, Unsupported};
use crate::events::{BackendEvents, EventLatch};
use crate::output::{OutputRenderer, OutputStamp};
use crate::stream::{DuplexStream, OutputPatch, PatchLink, PatchState, Plumbing, StreamParts};

/// Ports per direction. The callback gathers port buffers into fixed arrays of this size,
/// so it never allocates.
pub const MAX_PORTS: usize = 64;
/// The only device id JACK lists: the server.
pub const JACK_DEVICE_ID: &str = "jack";
const AUDIO_TYPE: &str = "32 bit float mono audio";

/// JACK client settings.
///
/// Output ports are never connected at open: which outputs reach the physical playback
/// ports is the operator's choice, made through the stream's
/// [`output_patch`](DuplexStream::output_patch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JackConfig {
    /// Client name shown in the graph.
    pub client_name: String,
    /// Connect our input ports to the physical capture ports named by the input map.
    pub connect_inputs: bool,
}

impl Default for JackConfig {
    fn default() -> Self {
        Self {
            client_name: "ac2".into(),
            connect_inputs: true,
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

fn unavailable(reason: Unavailability) -> AudioError {
    AudioError::Unavailable {
        backend: BackendKind::Jack,
        reason,
    }
}

/// PipeWire's native socket, where PipeWire puts it: `$PIPEWIRE_RUNTIME_DIR`, else
/// `$XDG_RUNTIME_DIR`, named `$PIPEWIRE_REMOTE` or `pipewire-0`.
/// The JACK2 server's socket: `jack_<server>_<uid>_0` in `$JACK_TMPDIR` (else `/dev/shm`),
/// the server named by `$JACK_DEFAULT_SERVER` (else `default`). It exists while the server
/// runs, and a restarted server makes a new one.
fn jack_server_socket() -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let dir = std::env::var_os("JACK_TMPDIR").unwrap_or_else(|| "/dev/shm".into());
    let server = std::env::var("JACK_DEFAULT_SERVER").unwrap_or_else(|_| "default".into());
    // This process's user, as the server names its socket.
    let uid = std::fs::metadata("/proc/self").ok()?.uid();
    Some(Path::new(&dir).join(format!("jack_{server}_{uid}_0")))
}

/// A socket file's identity: a server started anew makes a new file (another inode or
/// change time), so a server restarted between two looks still reads as back.
fn socket_generation(p: &Path) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(p).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (m.ino(), m.ctime(), m.ctime_nsec()).hash(&mut h);
    Some(h.finish())
}

pub fn pipewire_socket() -> Option<PathBuf> {
    let dir =
        std::env::var_os("PIPEWIRE_RUNTIME_DIR").or_else(|| std::env::var_os("XDG_RUNTIME_DIR"))?;
    let name = std::env::var_os("PIPEWIRE_REMOTE").unwrap_or_else(|| "pipewire-0".into());
    Some(Path::new(&dir).join(name))
}

/// Why no JACK client could be opened. `library_loaded`: some libjack was found;
/// `pipewire`: PipeWire's socket exists.
fn no_server(library_loaded: bool, pipewire: bool) -> Unavailability {
    match (library_loaded, pipewire) {
        (false, _) => Unavailability::NoJackLibrary,
        // A libjack that cannot reach a server while PipeWire runs is JACK2's: PipeWire's
        // own libjack would have connected.
        (true, true) => Unavailability::PipeWireWithoutJack,
        (true, false) => Unavailability::NoJackServer,
    }
}

/// libjack prints to stderr on its own, and probing for a server that is not there prints
/// half a dozen errors ("Cannot connect to server socket", "jack server is not running or
/// cannot be started", ...) that say nothing [`Unavailability`] does not. Those, and its
/// complaints about memory locking and real-time scheduling the server copes without, go
/// to debug; anything else stays a warning.
fn quiet_libjack() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        jack::set_logger(jack::LoggerType::Custom {
            info: libjack_info,
            error: libjack_error,
        });
    });
}

/// libjack messages that are expected while probing or that libjack recovers from.
fn libjack_noise(msg: &str) -> bool {
    const NOISE: [&str; 12] = [
        "Cannot connect to server",
        "server is not running",
        "connect(2) call to",
        "attempt to connect to server failed",
        "jack_client_open",
        "JackShmReadWritePtr",
        "Cannot lock down",
        "real-time scheduling",
        "AcquireSelfRealTime",
        "SetInitCallback",
        "Cannot open shm",
        "jack_client_new",
    ];
    NOISE.iter().any(|n| msg.contains(n))
}

fn libjack_message(msg: *const c_char) -> String {
    if msg.is_null() {
        return String::new();
    }
    // SAFETY: libjack passes a NUL-terminated string valid for the duration of the call.
    #[allow(unsafe_code)]
    let s = unsafe { CStr::from_ptr(msg) };
    s.to_string_lossy().into_owned()
}

// The libc crate binds neither on Linux; glibc and musl agree on the values.
const PTHREAD_CANCEL_DISABLE: c_int = 1;
#[allow(unsafe_code)]
unsafe extern "C" {
    fn pthread_setcancelstate(state: c_int, oldstate: *mut c_int) -> c_int;
}

/// Runs a libjack callback body. JACK2 tears a client down by `pthread_cancel`ing its
/// threads, and a log write is a cancellation point: there glibc would start a forced
/// unwind, which `catch_unwind` swallows, and glibc aborts the process when a forced unwind
/// is not rethrown. With cancellation disabled for the body, a pending cancellation waits
/// for libjack's next cancellation point, outside any Rust frame. `catch_unwind` still
/// keeps a Rust panic from unwinding into C.
fn libjack_callback(body: impl FnOnce()) {
    let mut old: c_int = 0;
    // SAFETY: changes only the calling thread's cancel state; not a cancellation point.
    #[allow(unsafe_code)]
    unsafe {
        pthread_setcancelstate(PTHREAD_CANCEL_DISABLE, &mut old)
    };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    let mut ignored: c_int = 0;
    // SAFETY: as above; restoring an enabled state does not act on a pending cancellation.
    #[allow(unsafe_code)]
    unsafe {
        pthread_setcancelstate(old, &mut ignored)
    };
}

#[allow(unsafe_code)]
unsafe extern "C" fn libjack_info(msg: *const c_char) {
    libjack_callback(|| log::debug!(target: "libjack", "{}", libjack_message(msg)));
}

#[allow(unsafe_code)]
unsafe extern "C" fn libjack_error(msg: *const c_char) {
    libjack_callback(|| {
        let m = libjack_message(msg);
        if libjack_noise(&m) {
            log::debug!(target: "libjack", "{m}");
        } else {
            log::warn!(target: "libjack", "{m}");
        }
    });
}

/// Calls the libjack error callback the way libjack's own threads do, so a test can cancel
/// a thread inside it without a JACK server.
#[doc(hidden)]
pub fn libjack_error_for_test(msg: &CStr) {
    // SAFETY: `msg` is NUL-terminated and outlives the call.
    #[allow(unsafe_code)]
    unsafe {
        libjack_error(msg.as_ptr())
    };
}

fn backend_err(operation: Operation, e: impl std::fmt::Display) -> AudioError {
    AudioError::Backend {
        backend: BackendKind::Jack,
        operation,
        detail: e.to_string(),
    }
}

fn connect(name: &str) -> Result<Client, AudioError> {
    quiet_libjack();
    let pipewire = || pipewire_socket().is_some_and(|p| p.exists());
    Client::new(name, ClientOptions::NO_START_SERVER)
        .map(|(c, _)| c)
        .map_err(|e| match e {
            jack::Error::LibraryError(_) => unavailable(no_server(false, pipewire())),
            // The failure an operator meets in practice; the status bits say nothing more
            // useful to them than which server is missing.
            jack::Error::ClientError(s) if s.contains(ClientStatus::SERVER_FAILED) => {
                unavailable(no_server(true, pipewire()))
            }
            e => unavailable(Unavailability::Host(e.to_string())),
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
            // The server is the only device JACK lists.
            system_default: true,
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

    /// The server's socket, without connecting a client: JACK2's own, else PipeWire's (whose
    /// libjack serves JACK clients). The ports are not looked at: a server that is back
    /// is worth one real attempt, which names a missing port itself.
    fn probe(&self, _device: &DeviceSelector) -> Presence {
        [jack_server_socket(), pipewire_socket()]
            .into_iter()
            .flatten()
            .find_map(|p| socket_generation(&p))
            .map_or(Presence::Absent, |generation| Presence::Present {
                generation,
            })
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
        let active = Arc::new(
            client
                .activate_async(notifications, process)
                .map_err(|e| backend_err(Operation::Start, e))?,
        );

        let c = active.as_client();
        if self.config.connect_inputs {
            for (ours, &dev) in in_names.iter().zip(&request.input_map) {
                if let Some(src) = capture.get(usize::from(dev)) {
                    c.connect_ports_by_name(src, ours)
                        .map_err(|e| backend_err(Operation::Connect, e))?;
                }
            }
        }
        let patch = JackPatch {
            client: Arc::downgrade(&active),
            ours: out_names,
            playback,
            made: Mutex::new(BTreeMap::new()),
        };

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
            delivery: Delivery::Device,
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
            patch: Some(Arc::new(patch)),
        }))
    }
}

type Active = jack::AsyncClient<Notifications, Process>;

/// Connections from our output ports to the physical playback ports. Holds the client
/// weakly: the stream's guard alone keeps it alive, so a patch kept by the control side
/// never delays closing the stream.
struct JackPatch {
    client: Weak<Active>,
    /// Our output ports, full names, by output channel.
    ours: Vec<String>,
    /// Physical playback ports in the order the device listing names them.
    playback: Vec<String>,
    /// Connections this patch made, by output channel.
    made: Mutex<BTreeMap<u16, (String, String)>>,
}

impl OutputPatch for JackPatch {
    fn connect_only(&self, outputs: &[u16]) -> Result<PatchState, AudioError> {
        let active = self
            .client
            .upgrade()
            .ok_or_else(|| backend_err(Operation::Connect, "the stream is closed"))?;
        let c = active.as_client();
        let mut made = self
            .made
            .lock()
            .map_err(|_| backend_err(Operation::Connect, "patch state poisoned"))?;
        let mut state = PatchState::default();
        // Our own connections of outputs no longer chosen go first.
        let dropped: Vec<u16> = made
            .keys()
            .copied()
            .filter(|k| !outputs.contains(k))
            .collect();
        for k in dropped {
            if let Some((from, to)) = made.remove(&k) {
                // Already gone (the operator removed it) is fine.
                let _ = c.disconnect_ports_by_name(&from, &to);
            }
        }
        for &k in outputs {
            let (Some(from), Some(to)) = (
                self.ours.get(usize::from(k)),
                self.playback.get(usize::from(k)),
            ) else {
                state.unmatched.push(k);
                continue;
            };
            let connected = c
                .port_by_name(from)
                .is_some_and(|p| p.is_connected_to(to).unwrap_or(false));
            if !connected {
                c.connect_ports_by_name(from, to)
                    .map_err(|e| backend_err(Operation::Connect, format!("{from} → {to}: {e}")))?;
                made.insert(k, (from.clone(), to.clone()));
            }
            state.links.push(PatchLink {
                output: k,
                from: from.clone(),
                to: to.clone(),
            });
        }
        Ok(state)
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
    use super::*;

    #[test]
    fn no_server_names_the_remedy_for_this_machine() {
        assert_eq!(no_server(true, true), Unavailability::PipeWireWithoutJack);
        assert_eq!(no_server(true, false), Unavailability::NoJackServer);
        assert_eq!(no_server(false, true), Unavailability::NoJackLibrary);
        assert_eq!(no_server(false, false), Unavailability::NoJackLibrary);
        let pw = Unavailability::PipeWireWithoutJack.to_string();
        for want in [
            "pipewire-jack",
            "sudo apt install pipewire-jack",
            "pacman",
            "pw-jack ac2d",
        ] {
            assert!(pw.contains(want), "{pw}");
        }
        let none = Unavailability::NoJackServer.to_string();
        assert!(
            none.contains("jackd -d alsa") && none.contains("PipeWire"),
            "{none}"
        );
    }

    #[test]
    fn probing_noise_is_told_from_real_errors() {
        for n in [
            "Cannot connect to server socket err = No such file or directory",
            "Cannot connect to server request channel",
            "jack server is not running or cannot be started",
            "JackShmReadWritePtr::~JackShmReadWritePtr - Init not done for -1, skipping unlock",
            "Cannot lock down 107341340 byte memory area (Cannot allocate memory)",
            "Cannot use real-time scheduling (RR/5) (1: Operation not permitted)",
            "JackClient::AcquireSelfRealTime error",
            "JackMessageBuffer::SetInitCallback : callback could not be executed",
        ] {
            assert!(libjack_noise(n), "{n}");
        }
        assert!(!libjack_noise(
            "Cannot connect ports owned by inactive clients"
        ));
    }

    #[test]
    fn numbered_aliases_are_not_names() {
        for g in ["in1", "out_2", "capture_3", "playback4", "12"] {
            assert!(generic_port_name(g), "{g}");
        }
        for n in ["Mic1_in", "capture_FL", "Front Left"] {
            assert!(!generic_port_name(n), "{n}");
        }
    }
}
