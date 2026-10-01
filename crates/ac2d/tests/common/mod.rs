//! Test client for the daemon: a DEALER for ctrl, SUBs for data, and helpers to run the
//! fake audio device by hand.
#![allow(dead_code, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakePath, Pace};
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::frame::{Frame, FrameData};
use ac2_proto::model::{
    DeviceSelector, LogGridSpec, LoopbackRoute, MeasConfig, MeasKind, SessionConfig, SplConfig,
    TfAveraging, TransferConfig,
};
use ac2_proto::units::RequestId;
use ac2_proto::{
    Command, DataMessage, Event, ProtoError, Reply, ReplyBody, Request, decode_data_message,
    decode_reply, encode_request,
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
        input_device: DeviceSelector::Default,
        output_device: DeviceSelector::Default,
        input_channels: vec![0, 1],
        output_channels: 2,
        sample_rate_hz: Some(FS),
        buffer_frames: Some(BLOCK),
        loopback: loopback.then_some(LoopbackRoute {
            output: 0,
            input: 0,
        }),
    }
}

pub fn transfer(name: &str) -> MeasConfig {
    MeasConfig {
        name: name.into(),
        kind: MeasKind::Transfer {
            config: TransferConfig {
                reference_input: 0,
                measurement_input: 1,
                averaging: TfAveraging::Fifo { blocks: 4 },
                grid: LogGridSpec {
                    ppo: 48,
                    k_min: -240,
                    k_max: 239,
                },
                smoothing: None,
            },
        },
    }
}

pub fn spl(name: &str, input: u16) -> MeasConfig {
    MeasConfig {
        name: name.into(),
        kind: MeasKind::Spl {
            config: SplConfig {
                input,
                weighting: ac2_proto::model::Weighting::Z,
                time_weighting: ac2_proto::model::TimeWeighting::Fast,
                peak_weighting: ac2_proto::model::PeakWeighting::Z,
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
        self.sock.send(&[encode_request(req).unwrap()]).unwrap();
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
}

/// A data subscriber.
pub struct Sub {
    pub sock: Socket,
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
        let sock = ctx.socket(SocketType::Sub).unwrap();
        if let Some(c) = curve {
            sock.set_curve_client(c).unwrap();
        }
        if let Some(h) = rcvhwm {
            sock.set_recv_hwm(h).unwrap();
        }
        for p in prefixes {
            sock.subscribe(p).unwrap();
        }
        sock.connect(endpoint).unwrap();
        Self { sock }
    }

    pub fn next(&self, timeout: Duration) -> Option<DataMessage> {
        let m = self.sock.recv_timeout(timeout).unwrap()?;
        let parts: Vec<&[u8]> = m.frames().iter().map(Vec::as_slice).collect();
        Some(decode_data_message(&parts).unwrap())
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

/// Runs `seconds` of device time in 50 ms steps (the fan-out keeps up between steps).
pub fn run(d: &mut FakeDriver, seconds: f64) {
    let blocks = (seconds * f64::from(FS) / f64::from(BLOCK)).ceil() as u64;
    let chunk = (0.05 * f64::from(FS) / f64::from(BLOCK)).ceil() as u64;
    let mut done = 0;
    while done < blocks {
        let n = chunk.min(blocks - done);
        d.run_blocks(n);
        done += n;
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// One past the last sample the driver has produced.
pub fn end_sample(d: &FakeDriver) -> u64 {
    d.blocks() * u64::from(BLOCK)
}

pub fn wall_now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}
