//! Test client for the daemon: a DEALER for ctrl, DEALERs for data, and helpers to run the
//! fake audio device by hand.
#![allow(dead_code, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakePath, Pace};
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::flow::{DATA_WINDOW_DEFAULT, FLOW_TOPIC, frame_charge, outside_window};
use ac2_proto::frame::{Frame, FrameData};
use ac2_proto::model::{
    DeviceSelector, LoopbackRoute, MeasConfig, MeasKind, Resolution, SessionConfig, SplConfig,
    TfAveraging, TransferConfig,
};
use ac2_proto::units::{MeasId, RequestId, Rev};
use ac2_proto::{
    CodedFrame, Command, DataMessage, DataRequest, DataUp, DecodeError, Event, FlowNotice,
    ProtoError, Reply, ReplyBody, Request, Subscription, Topic, WireFrame, decode_data_message,
    decode_reply, decode_wire, encode_data_request, encode_request,
};
use ac2_zmq::{Context, CurveClient, Socket, SocketType};
use ac2d::{DaemonConfig, Handle, Listen};

pub const FS: u32 = 48_000;
pub const BLOCK: u32 = 256;
/// Loopback cable delay of the fake rig (output 0 → input 0).
pub const LOOP_DELAY: u32 = 37;
/// Extra acoustic delay of the measurement path (output 0 → input 1).
pub const ACOUSTIC_DELAY: u32 = 120;
/// Gain of the acoustic path.
pub const ACOUSTIC_GAIN: f32 = 0.5;

static SEQ: AtomicU64 = AtomicU64::new(0);

pub fn unique(name: &str) -> String {
    format!(
        "{name}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// Daemon logs in test output (`RUST_LOG`, default warn).
pub fn init_log() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();
}

/// The fake rig: output 0 feeds a loopback cable into input 0 and an acoustic path (delay,
/// gain, a little noise) into input 1.
pub fn rig(drive: FakeDrive) -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive,
        seed: 7,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY + ACOUSTIC_DELAY, vec![ACOUSTIC_GAIN], 1e-5),
        ],
        ..FakeConfig::default()
    })
    .unwrap()
}

pub fn manual_rig() -> FakeBackend {
    rig(FakeDrive::Manual)
}

pub fn realtime_rig() -> FakeBackend {
    rig(FakeDrive::Thread(Pace::Realtime))
}

pub fn config(backend: FakeBackend, listen: Listen) -> DaemonConfig {
    DaemonConfig::new(Arc::new(backend), listen, -10.0)
}

/// [`config`] with a stimulus lease that outlives any stall of a loaded machine, for tests
/// that play a tone through a whole real-time run: a refresh missed by a starved test
/// process would silence it and read as a wrong level. The expiry itself is tested against
/// short deadlines (`stimulus`, `remote_stimulus`, `sweep`).
pub fn steady_config(backend: FakeBackend, listen: Listen) -> DaemonConfig {
    let mut cfg = config(backend, listen);
    cfg.lease_expiry = Duration::from_secs(60);
    cfg
}

pub fn inproc(name: &str) -> Listen {
    Listen::Inproc { name: unique(name) }
}

/// Local endpoints for a test: ipc in a temp dir on Unix, loopback TCP elsewhere.
pub fn local(dir: &std::path::Path) -> Listen {
    #[cfg(unix)]
    {
        Listen::Local {
            ctrl: format!("ipc://{}", dir.join("ctrl").display()),
            data: format!("ipc://{}", dir.join("data").display()),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        local_tcp()
    }
}

pub fn local_tcp() -> Listen {
    Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    }
}

pub fn session(loopback: bool) -> SessionConfig {
    SessionConfig {
        backend: None,
        input_device: DeviceSelector::Default,
        output_device: DeviceSelector::Default,
        input_channels: vec![0, 1],
        spare_inputs: 0,
        output_channels: 2,
        sample_rate_hz: Some(FS),
        buffer_frames: Some(BLOCK),
        loopback: loopback.then_some(LoopbackRoute {
            output: 0,
            input: 0,
        }),
        remote_inputs: Vec::new(),
        link_offset: None,
    }
}

pub fn transfer(name: &str) -> MeasConfig {
    MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name: name.into(),
        kind: MeasKind::Transfer {
            config: TransferConfig {
                reference_input: 0,
                measurement_input: 1,
                averaging: TfAveraging::Fifo { blocks: 4 },
                resolution: Resolution::FortyEighth,
                smoothing: None,
                depth: ac2_proto::model::DepthPolicy::EqualConfidence,
            },
        },
    }
}

pub fn spl(name: &str, input: u16) -> MeasConfig {
    MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name: name.into(),
        kind: MeasKind::Spl {
            config: SplConfig {
                cue: None,
                bands: None,
                input,
                weighting: ac2_proto::model::Weighting::Z,
                time_weighting: ac2_proto::model::TimeWeighting::Fast,
                peak_weighting: ac2_proto::model::PeakWeighting::Z,
                leq: ac2_proto::model::LeqConfig::default_windows(),
                position: None,
            },
        },
    }
}

/// A ctrl client.
pub struct Client {
    pub sock: Socket,
    next: u64,
}

const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Sends `frames` on a DEALER, waiting at most [`CALL_TIMEOUT`] for it to take them. A
/// DEALER whose connection was refused may have no pipe left to queue on (a client that
/// reads the server's ERROR before the connection drops does not reconnect), and a
/// blocking send there never returns.
fn send_within(sock: &Socket, frames: &[Vec<u8>], what: &str) {
    let deadline = Instant::now() + CALL_TIMEOUT;
    loop {
        match sock.try_send(frames) {
            Ok(()) => return,
            Err(ac2_zmq::Error::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => panic!("{what} not sent within {CALL_TIMEOUT:?}: {e}"),
        }
    }
}

impl Client {
    pub fn connect(ctx: &Context, endpoint: &str) -> Self {
        Self::connect_with(ctx, endpoint, None)
    }

    pub fn connect_with(ctx: &Context, endpoint: &str, curve: Option<&CurveClient>) -> Self {
        let sock = ctx.socket(SocketType::Dealer).unwrap();
        if let Some(c) = curve {
            sock.set_curve_client(c).unwrap();
        }
        sock.connect(endpoint).unwrap();
        Self { sock, next: 1 }
    }

    pub fn next_id(&mut self) -> RequestId {
        self.next += 1;
        RequestId(self.next)
    }

    pub fn send(&self, req: &Request) {
        send_within(&self.sock, &[encode_request(req).unwrap()], req.cmd.name());
    }

    /// Sends `req` if the connection takes it now, for a client that may already be
    /// refused; whether it went.
    pub fn offer(&self, req: &Request) -> bool {
        self.sock.try_send(&[encode_request(req).unwrap()]).is_ok()
    }

    pub fn recv(&self, timeout: Duration) -> Option<Reply> {
        let m = self.sock.recv_timeout(timeout).unwrap()?;
        Some(decode_reply(&m.frames()[0]).unwrap())
    }

    pub fn recv_raw(&self, timeout: Duration) -> Option<Vec<u8>> {
        let m = self.sock.recv_timeout(timeout).unwrap()?;
        Some(m.into_frames().remove(0))
    }

    pub fn call_req(&mut self, req: Request) -> Reply {
        self.send(&req);
        let deadline = Instant::now() + CALL_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let r = self
                .recv(left)
                .unwrap_or_else(|| panic!("no reply to {}", req.cmd.name()));
            if r.id == req.id {
                return r;
            }
        }
    }

    pub fn call(&mut self, cmd: Command) -> Result<ReplyBody, ProtoError> {
        let id = self.next_id();
        self.call_req(Request::new(id, cmd)).result
    }

    /// A call that must succeed.
    pub fn ok(&mut self, cmd: Command) -> ReplyBody {
        let name = cmd.name();
        self.call(cmd)
            .unwrap_or_else(|e| panic!("{name} failed: {e:?}"))
    }

    /// The state the daemon holds now.
    #[track_caller]
    pub fn state(&mut self) -> ac2_proto::model::State {
        self.snapshot(Command::StateSnapshot).state
    }

    /// The stimulus lease, taken without force.
    #[track_caller]
    pub fn acquire(&mut self) -> ac2_proto::units::LeaseToken {
        self.lease(Command::GenAcquire { force: false }).lease_token
    }

    /// The measurement's current `config_rev`: the `base` of an edit of it.
    pub fn config_rev(&mut self, meas: MeasId) -> Rev {
        let state = self.state();
        let m = state.measurements.iter().find(|m| m.id == meas);
        m.unwrap_or_else(|| panic!("no measurement {meas}"))
            .config_rev
    }

    /// `meas.update` on the measurement's current configuration.
    pub fn update(&mut self, meas: MeasId, config: MeasConfig) -> Result<ReplyBody, ProtoError> {
        let base = self.config_rev(meas);
        self.call(Command::MeasUpdate { meas, base, config })
    }

    /// A `meas.update` that must succeed.
    pub fn update_ok(&mut self, meas: MeasId, config: MeasConfig) -> ReplyBody {
        self.update(meas, config)
            .unwrap_or_else(|e| panic!("meas.update failed: {e:?}"))
    }
}

/// Reads of a reply of a known kind: `name` on a [`ReplyBody`], and `Client::name(cmd)` for a
/// call that must succeed with it. Any other reply fails the test, naming what came back.
macro_rules! reply_kinds {
    ($($name:ident: $pat:pat => $out:expr, $ty:ty;)*) => {
        pub trait Expect {
            $(fn $name(self) -> $ty;)*
        }

        impl Expect for ReplyBody {
            $(#[track_caller]
            fn $name(self) -> $ty {
                match self {
                    $pat => $out,
                    other => panic!("expected {}, got {other:?}", stringify!($name)),
                }
            })*
        }

        impl Client {
            $(#[track_caller]
            pub fn $name(&mut self, cmd: Command) -> $ty {
                self.ok(cmd).$name()
            })*
        }
    };
}

reply_kinds! {
    welcome: ReplyBody::Welcome(v) => v, ac2_proto::Welcome;
    session: ReplyBody::Session(v) => v, ac2_proto::model::Session;
    lease: ReplyBody::Lease(v) => v, ac2_proto::model::Lease;
    generator: ReplyBody::Generator(v) => v, ac2_proto::model::Generator;
    measurement: ReplyBody::Measurement(v) => v, ac2_proto::model::Measurement;
    delay_finding: ReplyBody::DelayFinding(v) => v, ac2_proto::model::DelayFinding;
    trace: ReplyBody::Trace(v) => v, ac2_proto::model::TraceMeta;
    sweep: ReplyBody::Sweep(v) => v, ac2_proto::model::SweepRun;
    traces: ReplyBody::Traces(v) => v, Vec<ac2_proto::model::TraceMeta>;
    trace_data: ReplyBody::TraceData(v) => *v, ac2_proto::model::TraceData;
    export: ReplyBody::Export { file_name, content } => (file_name, content), (String, ac2_proto::units::Blob);
    export_text: ReplyBody::Export { content, .. } => String::from_utf8(content.0).unwrap(), String;
    calibration: ReplyBody::Calibration(v) => v, ac2_proto::model::CalEntry;
    mic: ReplyBody::Mic(v) => v, ac2_proto::model::Mic;
    outputs: ReplyBody::Outputs(v) => v, Vec<ac2_proto::model::OutputSetup>;
    server: ReplyBody::Server(v) => v, ac2_proto::model::ServerInfo;
    spl_log_page: ReplyBody::SplLogPage(v) => v, ac2_proto::model::SplLogPage;
    snapshot: ReplyBody::Snapshot(v) => *v, ac2_proto::StateSnapshot;
    grid: ReplyBody::Grid(v) => v, ac2_proto::GridDef;
    session_file: ReplyBody::SessionFile(v) => v, ac2_proto::model::SessionFile;
    transfer: ReplyBody::Transfer(v) => v, ac2_proto::model::Transfer;
    chunk: ReplyBody::Chunk(v) => v, ac2_proto::units::Blob;
    salvage: ReplyBody::Salvage(v) => v, ac2_proto::model::Salvage;
    sessions: ReplyBody::Sessions(v) => v, Vec<ac2_proto::model::SessionFile>;
    recording: ReplyBody::Recording(v) => v, ac2_proto::model::RecordingRun;
    plans: ReplyBody::Plans(v) => v, Vec<ac2_proto::model::PlanSpec>;
    walk: ReplyBody::Walk(v) => v, ac2_proto::model::WalkState;
    sim: ReplyBody::Sim(v) => v, ac2_proto::model::SimState;
    recordings: ReplyBody::Recordings(v) => v, Vec<ac2_proto::model::RecordingFile>;
    spl_band_log: ReplyBody::SplBandLog(v) => *v, ac2_proto::model::SplBandLog;
    events: ReplyBody::Events(v) => v, Vec<Event>;
    ack: ReplyBody::Ack { rev } => rev, Rev;
    backends: ReplyBody::Backends(v) => v, Vec<ac2_proto::model::BackendInfo>;
    preview: ReplyBody::Preview(v) => v, ac2_proto::model::Preview;
    loopback_detection: ReplyBody::LoopbackDetection(v) => v, ac2_proto::model::LoopbackDetection;
    inputs: ReplyBody::Inputs(v) => v, ac2_proto::model::InputSetups;
    source_state: ReplyBody::SourceState(v) => *v, ac2_proto::model::SourceState;
    line_calibration: ReplyBody::LineCalibration(v) => v, ac2_proto::model::LineCalEntry;
    spl_history: ReplyBody::SplHistory(v) => *v, ac2_proto::model::SplHistory;
}

/// A file of the `ac2-traces` test fixtures.
pub fn fixture(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ac2-traces/tests/fixtures");
    std::fs::read(dir.join(name)).unwrap()
}

/// Imports the fixture `fixture_name` as a trace, as if from the file `file_name`, under `owner`.
pub fn import_fixture(
    c: &mut Client,
    fixture_name: &str,
    file_name: &str,
    owner: ac2_proto::model::TraceOwner,
) -> ac2_proto::model::TraceMeta {
    c.trace(Command::TraceImport {
        file_name: file_name.into(),
        format: ac2_proto::model::ImportFormat::Auto,
        role: ac2_proto::model::ImportRole::Trace,
        ir: None,
        content: ac2_proto::units::Blob(fixture(fixture_name)),
        owner,
    })
}

/// An async client named `name` on `h`, its mirror synced.
pub async fn mirror(h: &Handle, name: &str) -> ac2_client::Client {
    let ep = ac2_client::Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = ac2_client::Client::connect(ac2_client::ClientConfig::new(ep, name))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
    c
}

/// How long a poll goes on and how often it looks.
#[derive(Clone, Copy)]
pub struct Poll {
    pub limit: Duration,
    pub every: Duration,
}

impl Poll {
    pub const fn new(limit: Duration, every_ms: u64) -> Self {
        Self {
            limit,
            every: Duration::from_millis(every_ms),
        }
    }
}

/// Polls the newest frame of `stream` of `meas` the async client holds until `pick` returns a
/// value; `pick` sees the frame as held, so it decides whether a stale one counts.
pub async fn frame_until<T>(
    c: &ac2_client::Client,
    (meas, stream): (MeasId, ac2_proto::Stream),
    poll: Poll,
    what: &str,
    mut pick: impl FnMut(&ac2_client::TopicFrame) -> Option<T>,
) -> T {
    let topic = Topic::Data { meas, stream };
    let deadline = Instant::now() + poll.limit;
    loop {
        if let Some(v) = c.latest().unwrap().get(&topic).and_then(&mut pick) {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(poll.every).await;
    }
}

/// Polls the async client's mirrored state, once it has one, until `pick` returns a value.
pub async fn mirror_until<T>(
    c: &ac2_client::Client,
    poll: Poll,
    what: &str,
    mut pick: impl FnMut(&Arc<ac2_proto::model::State>) -> Option<T>,
) -> T {
    let deadline = Instant::now() + poll.limit;
    loop {
        if let Some(v) = c.view().state.as_ref().and_then(&mut pick) {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(poll.every).await;
    }
}

/// `meas.update` through the async client on the measurement's current configuration (read
/// from a snapshot, not the mirror, which may not have the latest commit yet).
pub async fn meas_update(
    c: &ac2_client::Client,
    meas: MeasId,
    config: MeasConfig,
) -> Result<ReplyBody, ac2_client::ClientError> {
    let s = c.snapshot().await?;
    let m = s.state.measurements.iter().find(|m| m.id == meas);
    let base = m
        .unwrap_or_else(|| panic!("no measurement {meas}"))
        .config_rev;
    c.call(Command::MeasUpdate { meas, base, config }).await
}

/// A data peer: a DEALER on the data socket that subscribes, returns credit for what it
/// reads (unless held), and applies each topic's deltas on the frame it holds, as a client
/// does.
pub struct Sub {
    pub sock: Socket,
    held: std::cell::RefCell<std::collections::HashMap<Topic, CodedFrame>>,
    subs: std::cell::RefCell<Vec<Subscription>>,
    /// Credit is returned after each message read.
    pub auto_credit: std::cell::Cell<bool>,
    /// Read and not yet credited: frame bytes (charged) and messages outside the window.
    owed: std::cell::Cell<(u32, u32)>,
    window: std::cell::Cell<u32>,
}

/// Every subscription, for a peer that asks for everything (the empty prefix).
fn everything() -> Vec<Subscription> {
    let mut v = vec![Subscription::AllData, Subscription::InputMeters];
    v.extend([Topic::PreviewGonio, Topic::Timing, Topic::Evt, Topic::Ka].map(Subscription::Topic));
    v
}

impl Sub {
    pub fn connect(ctx: &Context, endpoint: &str, prefixes: &[&[u8]]) -> Self {
        Self::connect_with(ctx, endpoint, prefixes, None, None)
    }

    pub fn connect_with(
        ctx: &Context,
        endpoint: &str,
        prefixes: &[&[u8]],
        curve: Option<&CurveClient>,
        rcvhwm: Option<u32>,
    ) -> Self {
        let s = Self::open(ctx, endpoint, curve, rcvhwm);
        s.ask(prefixes, |m| s.up(m));
        s
    }

    /// A peer the daemon is expected to refuse: its window and subscriptions go if the
    /// connection takes them (queued while it is set up), and are dropped once it is gone.
    pub fn knock(
        ctx: &Context,
        endpoint: &str,
        prefixes: &[&[u8]],
        curve: Option<&CurveClient>,
    ) -> Self {
        let s = Self::open(ctx, endpoint, curve, None);
        s.ask(prefixes, |m| {
            s.offer(m);
        });
        s
    }

    fn open(
        ctx: &Context,
        endpoint: &str,
        curve: Option<&CurveClient>,
        rcvhwm: Option<u32>,
    ) -> Self {
        let sock = ctx.socket(SocketType::Dealer).unwrap();
        if let Some(c) = curve {
            sock.set_curve_client(c).unwrap();
        }
        if let Some(h) = rcvhwm {
            sock.set_recv_hwm(h).unwrap();
        }
        sock.connect(endpoint).unwrap();
        Self {
            sock,
            held: Default::default(),
            subs: Default::default(),
            auto_credit: std::cell::Cell::new(true),
            owed: Default::default(),
            window: std::cell::Cell::new(DATA_WINDOW_DEFAULT),
        }
    }

    /// The default window and the subscriptions `prefixes` name, each sent by `send`.
    fn ask(&self, prefixes: &[&[u8]], send: impl Fn(DataUp)) {
        send(DataUp::Window {
            bytes: DATA_WINDOW_DEFAULT,
        });
        for p in prefixes {
            let subs = if p.is_empty() {
                everything()
            } else {
                vec![Subscription::parse_prefix(p).unwrap()]
            };
            for sub in subs {
                self.subs.borrow_mut().push(sub);
                send(DataUp::Subscribe { sub });
            }
        }
    }

    /// Sends one request on the data DEALER.
    pub fn up(&self, msg: DataUp) {
        let b = encode_data_request(&DataRequest::new(msg)).unwrap();
        send_within(&self.sock, &[b], "data request");
    }

    /// Sends one request if the connection takes it now; whether it went.
    pub fn offer(&self, msg: DataUp) -> bool {
        let b = encode_data_request(&DataRequest::new(msg)).unwrap();
        self.sock.try_send(&[b]).is_ok()
    }

    pub fn subscribe(&self, sub: Subscription) {
        self.subs.borrow_mut().push(sub);
        self.up(DataUp::Subscribe { sub });
    }

    pub fn unsubscribe(&self, sub: Subscription) {
        self.subs.borrow_mut().retain(|s| *s != sub);
        self.up(DataUp::Unsubscribe { sub });
    }

    pub fn set_window(&self, bytes: u32) {
        self.window.set(bytes);
        self.up(DataUp::Window { bytes });
    }

    pub fn keyframe(&self, topic: Topic) {
        self.up(DataUp::Keyframe { topic });
    }

    /// Returns the credit for everything read so far.
    pub fn credit(&self) {
        let (bytes, events) = self.owed.replace((0, 0));
        if bytes > 0 || events > 0 {
            self.up(DataUp::Credit { bytes, events });
        }
    }

    /// Returns credit for part of what was read.
    pub fn credit_some(&self, bytes: u32, events: u32) {
        let (b, e) = self.owed.get();
        let (bytes, events) = (bytes.min(b), events.min(e));
        self.owed.set((b - bytes, e - events));
        if bytes > 0 || events > 0 {
            self.up(DataUp::Credit { bytes, events });
        }
    }

    /// Bytes and outside-window messages read and not yet credited.
    pub fn owed(&self) -> (u32, u32) {
        self.owed.get()
    }

    /// The next message as received, its parts; counted for credit.
    pub fn raw(&self, timeout: Duration) -> Option<Vec<Vec<u8>>> {
        let m = self.sock.recv_timeout(timeout).unwrap()?;
        let f = m.into_frames();
        let (mut bytes, mut events) = self.owed.get();
        if f.first().is_some_and(|t| outside_window(t)) {
            events += 1;
        } else {
            bytes += frame_charge(f.iter().map(Vec::len).sum());
        }
        self.owed.set((bytes, events));
        if self.auto_credit.get() {
            self.credit();
        }
        Some(f)
    }

    /// The next event, notice or frame within `timeout`. A delta with no frame held, or not
    /// on the one held, is skipped: the topic's next keyframe brings it back. A `rejoin`
    /// notice is answered by stating the window and subscriptions again.
    pub fn next(&self, timeout: Duration) -> Option<DataMessage> {
        let deadline = Instant::now() + timeout;
        loop {
            // A zero timeout still takes what is queued.
            let left = deadline.saturating_duration_since(Instant::now());
            let m = self.raw(left)?;
            let parts: Vec<&[u8]> = m.iter().map(Vec::as_slice).collect();
            let first = parts.first().copied().unwrap_or_default();
            if first == FLOW_TOPIC || Topic::parse(first) == Ok(Topic::Evt) {
                let msg = decode_data_message(&parts).unwrap();
                if msg == DataMessage::Flow(FlowNotice::Rejoin) {
                    self.up(DataUp::Window {
                        bytes: self.window.get(),
                    });
                    for sub in self.subs.borrow().iter() {
                        self.up(DataUp::Subscribe { sub: *sub });
                    }
                }
                return Some(msg);
            }
            let w = decode_wire(&parts).unwrap();
            let topic = w.topic();
            let mut held = self.held.borrow_mut();
            let applied = match w {
                WireFrame::Key(k) => k,
                d @ WireFrame::Delta(_) => match d.apply(held.get(&topic)) {
                    Ok(c) => c,
                    Err(DecodeError::NoBase | DecodeError::WrongBase { .. }) => continue,
                    Err(e) => panic!("{topic}: {e}"),
                },
            };
            let f = applied.to_frame().unwrap();
            held.insert(topic, applied);
            return Some(DataMessage::Frame(f));
        }
    }

    /// First frame matching `pred` within `timeout`.
    pub fn frame(&self, timeout: Duration, mut pred: impl FnMut(&Frame) -> bool) -> Option<Frame> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            match self.next(left)? {
                DataMessage::Frame(f) if pred(&f) => return Some(f),
                _ => {}
            }
        }
    }

    /// First event matching `pred` within `timeout`.
    pub fn event(&self, timeout: Duration, mut pred: impl FnMut(&Event) -> bool) -> Option<Event> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            match self.next(left)? {
                DataMessage::Event(e) if pred(&e) => return Some(e),
                _ => {}
            }
        }
    }

    pub fn ka(&self, timeout: Duration) -> Option<Frame> {
        self.frame(timeout, |f| matches!(f.data, FrameData::Ka(_)))
    }
}

/// Connects a ctrl client and a data subscriber to `h`.
pub fn connect(h: &Handle, prefixes: &[&[u8]]) -> (Client, Sub) {
    let c = Client::connect(h.context(), h.ctrl_endpoint());
    let s = Sub::connect(h.context(), h.data_endpoint(), prefixes);
    (c, s)
}

/// Takes the manual driver the daemon's last stream open parked.
pub fn driver(b: &FakeBackend) -> FakeDriver {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(d) = b.take_driver() {
            return d;
        }
        assert!(Instant::now() < deadline, "no driver parked");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Runs `seconds` of device time in 50 ms steps, each once the daemon has taken the last.
pub fn run(d: &mut FakeDriver, seconds: f64) {
    let blocks = (seconds * f64::from(FS) / f64::from(BLOCK)).ceil() as u64;
    let chunk = (0.05 * f64::from(FS) / f64::from(BLOCK)).ceil() as u64;
    let mut done = 0;
    while done < blocks {
        let n = chunk.min(blocks - done);
        d.run_blocks(n);
        done += n;
        taken(d);
    }
}

/// Waits until the daemon has read every captured block. Stepping on regardless would
/// overflow the capture ring whenever the machine is busy, and the tests would see lost
/// audio they never scripted.
pub fn taken(d: &FakeDriver) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while d.capture_queued() > 0 {
        assert!(
            Instant::now() < deadline,
            "the daemon stopped reading audio"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// One past the last sample the driver has produced.
pub fn end_sample(d: &FakeDriver) -> u64 {
    d.blocks() * u64::from(BLOCK)
}

/// Waits until the daemon's keepalive reports audio up to `end` taken in (any epoch).
pub fn handed_on(ka: &Sub, end: u64) {
    ka.frame(Duration::from_secs(30), |f| {
        matches!(f.data, ac2_proto::frame::FrameData::Ka(_)) && f.stamp.audio_sample.0 + 1 >= end
    })
    .unwrap_or_else(|| panic!("audio up to {end} never arrived"));
}

pub fn wall_now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}
