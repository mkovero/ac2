//! `ac2 selftest net`: block-sized messages of zeros between two hosts at an audio block
//! rate, for the timing study's network phase (`docs/design/timing-study.md` §5 phase C,
//! Q6). No audio is opened at all.
//!
//! The transport is the data wire's (`docs/design/flow-control.md`): CURVE, a ROUTER on the
//! listening side and a DEALER on the connecting side, and the receiver returning byte
//! credit the way a client does ([`ac2_proto::flow`]: the same [`DataRequest`] messages, a
//! 64 KiB window, each message charged [`frame_charge`], credit returned once per read burst
//! and mid-burst whenever half the window is owed). A stalled reader therefore holds the
//! sender back exactly as it would hold the daemon back. Which side listens and which side
//! sends are independent: firewalls decide who can bind.
//!
//! Messages are `[tag][body…]` (the ROUTER adds the peer's routing id in front):
//!
//! - `hello` + JSON [`Hello`]: the connecting side first, the listener answers;
//! - `flow` + a msgpack [`DataRequest`]: the receiver's window, then its credit;
//! - `blk` + a 40-byte header + `frames × channels` f32 zeros. The header, little-endian:
//!   `0 u64 seq  8 u64 scheduled_ns  16 u64 sent_ns  24 u64 stalls  32 u64 late_sends`,
//!   both instants `CLOCK_REALTIME`, the counts the sender's so far;
//! - `end` + JSON [`SendEnd`]: the sender's closing counts.
//!
//! The receiver's directory holds `arrivals.bin` (a 16-byte header, then one 32-byte record
//! per block in arrival order, all little-endian) and `meta.json`:
//!
//! ```text
//! header  0  magic "AC2NETA1"   8  u32 format version (1)   12  u32 record bytes (32)
//! record  0  u64 seq   8  u64 scheduled_ns   16  u64 sent_ns   24  u64 arrived_ns
//! ```
//!
//! `scheduled_ns` and `sent_ns` are read on the sender's REALTIME clock, `arrived_ns` on the
//! receiver's, when the message is read off the socket. The sender's directory holds its own
//! `meta.json`; the listener's also `server.key`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ac2_proto::flow::{
    DATA_WINDOW_DEFAULT, DATA_WINDOW_MAX, DataRequest, DataUp, EVENT_BOUND, FRAME_CHARGE_MIN,
    decode_data_request, encode_data_request, frame_charge,
};
use ac2_zmq::{
    AuthorizedKeys, Context, CurveClient, KeyPair, PublicKey, SecretKey, SecureContext, Socket,
    SocketType,
};
use serde::{Deserialize, Serialize};

use super::stamps::Provenance;

#[path = "net_stats.rs"]
mod stats;
pub(crate) use stats::Receiver;
pub use stats::{NetSummary, Quantiles};

/// `arrivals.bin` magic.
pub const MAGIC: &[u8; 8] = b"AC2NETA1";
/// `arrivals.bin` format version.
pub const FORMAT_VERSION: u32 = 1;
/// Bytes per `arrivals.bin` record.
pub const RECORD_BYTES: usize = 32;
/// Bytes before the first record.
pub const HEADER_BYTES: usize = 16;
/// Bytes of a block message's header part.
pub const BLOCK_HEADER_BYTES: usize = 40;
/// Window the receiver states: the data wire's default, so stalls behave as they would.
pub const WINDOW: u32 = DATA_WINDOW_DEFAULT;

const TAG_HELLO: &[u8] = b"hello";
const TAG_FLOW: &[u8] = b"flow";
const TAG_BLOCK: &[u8] = b"blk";
const TAG_END: &[u8] = b"end";

const ZAP_DOMAIN: &str = "ac2-selftest-net";
/// The connecting side's CURVE keypair, the same on every host. The listener authorises
/// this one key, which lets any copy of the tool in without a second key exchange; the
/// server key still pins the listener, and CURVE's per-connection session keys still
/// encrypt, so the stream costs what the data wire's does.
const CLIENT_PUBLIC: &str = "KdXi/Qv36!r)TAIh%*R%9n741Un$3[a(c{Bm@6gY";
const CLIENT_SECRET: &str = "CIe@7kSI8{r(oNrF@v{gSPAB%EQEYAls*ph{kpWo";

/// As the daemon's data ROUTER: twice what credit and the event bound let be outstanding,
/// because libzmq reports what a connection took only every half queue.
const SNDHWM: u32 = 2 * (DATA_WINDOW_MAX / FRAME_CHARGE_MIN + 1 + EVENT_BOUND);
/// The first block goes this long after the window arrives, so the schedule does not start
/// already behind.
const LEAD: Duration = Duration::from_millis(20);
/// How long the connecting side waits for the listener's hello: a wrong `--server-key` or
/// a firewall looks like silence.
const CONNECT_WAIT: Duration = Duration::from_secs(60);
/// After its own run length the receiver waits this long for the sender's `end`.
const END_GRACE: Duration = Duration::from_secs(10);
/// Queued messages (the `end`) still go out this long after the socket closes.
const LINGER: Duration = Duration::from_secs(2);
/// Poll slice while nothing is due: bounds how late a Ctrl-C is seen.
const SLICE: Duration = Duration::from_millis(100);
/// Gaps listed in `meta.json`; the count covers all of them.
const GAPS_LISTED: usize = 100;

/// Which way the blocks go from this host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Paces blocks onto the wire.
    Send,
    /// Reads them, returns credit, and logs each arrival.
    Receive,
}

/// Whether this host bound or connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Bound a ROUTER.
    Listen,
    /// Connected a DEALER.
    Connect,
}

/// The stream, as the sender states it; the receiver adopts the sender's.
#[derive(Clone, Debug, PartialEq)]
pub struct NetSpec {
    /// Which way blocks go.
    pub role: Role,
    /// Block rate is `rate / frames` per second.
    pub rate: u32,
    /// Frames per block.
    pub frames: u32,
    /// Channels per frame (f32 each).
    pub channels: u32,
    /// Run length: the sender's schedule; the receiver's cap (plus a grace for `end`).
    pub duration: Duration,
    /// Credit window the receiver states, bytes.
    pub window: u32,
    /// Receiver only, for tests: return no credit from `.0` after the start for `.1`.
    pub credit_pause: Option<(Duration, Duration)>,
}

impl NetSpec {
    /// Bytes of a block's payload.
    pub fn payload_bytes(&self) -> u64 {
        u64::from(self.frames) * u64::from(self.channels) * 4
    }

    /// Bytes of a block message as credit counts them (tag, header, payload).
    pub fn message_bytes(&self) -> u64 {
        (TAG_BLOCK.len() + BLOCK_HEADER_BYTES) as u64 + self.payload_bytes()
    }
}

/// What each side tells the other first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The speaker's role.
    pub role: Role,
    /// The speaker's host.
    pub host: String,
    /// Stream rate, Hz.
    pub rate: u32,
    /// Frames per block.
    pub frames: u32,
    /// Channels.
    pub channels: u32,
}

/// The sender's closing counts, in its `meta.json` and in `end`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SendEnd {
    /// Ctrl-C ended it early.
    pub interrupted: bool,
    /// Seconds from the first block's schedule to the last send.
    pub seconds: f64,
    /// Blocks sent (seq 0 … blocks − 1); none is ever skipped.
    pub blocks: u64,
    /// Times a due block found the window exhausted.
    pub stalls: u64,
    /// Time spent waiting for credit, ms.
    pub stall_ms: f64,
    /// Blocks whose send waited for credit.
    pub credit_waits: u64,
    /// Blocks sent more than one block period after their scheduled instant.
    pub late_sends: u64,
    /// Latest send against its schedule, µs.
    pub max_late_us: f64,
    /// Sends libzmq refused for a full queue (credit should make this zero).
    pub queue_refusals: u64,
    /// The receiver's connection vanished.
    pub peer_gone: bool,
}

/// What the sender said in-band, from the last block header read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct InBand {
    /// Stalls so far.
    pub stalls: u64,
    /// Late sends so far.
    pub late_sends: u64,
}

/// Why the receiver stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ended {
    /// The sender's `end` arrived.
    Sender,
    /// Its own run length and the grace passed without `end`.
    Deadline,
    /// Ctrl-C.
    Interrupted,
}

/// The receiver's closing counts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReceiveEnd {
    /// Why it stopped.
    pub ended: Ended,
    /// Seconds from the handshake to the stop.
    pub seconds: f64,
    /// Blocks read.
    pub received: u64,
    /// Blocks not read: of those the sender's `end` counts, else up to the highest seq read.
    pub missing: u64,
    /// The first gaps in the sequence, `[first missing, last missing]`.
    pub gaps: Vec<[u64; 2]>,
    /// Blocks read after a higher seq.
    pub reordered: u64,
    /// The sender's counts from the last block header.
    pub in_band: InBand,
    /// The sender's `end`, when it arrived.
    pub sender: Option<SendEnd>,
    /// Timing of what arrived.
    pub summary: Option<NetSummary>,
}

/// `meta.json` and the run's summary, either side.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NetMeta {
    /// `arrivals.bin` format version.
    pub format_version: u32,
    /// Host and build.
    #[serde(flatten)]
    pub provenance: Provenance,
    /// This host's role.
    pub role: Role,
    /// Whether this host listened or connected.
    pub side: Side,
    /// Bound endpoint, or the one connected to.
    pub addr: String,
    /// The other host, as it named itself.
    pub peer_host: String,
    /// Stream rate, Hz (the sender's).
    pub sample_rate: u32,
    /// Frames per block.
    pub frames: u32,
    /// Channels.
    pub channels: u32,
    /// Bytes of zeros per block.
    pub payload_bytes: u64,
    /// Bytes per block message as credit charges it.
    pub message_bytes: u64,
    /// Credit window, bytes.
    pub window_bytes: u32,
    /// One block period, µs.
    pub block_period_us: f64,
    /// Handshake done, ns since the Unix epoch (`CLOCK_REALTIME`).
    pub start_realtime_ns: u64,
    /// The same, UTC.
    pub start_utc: String,
    /// Sender's counts, filled at the end.
    pub sent: Option<SendEnd>,
    /// Receiver's counts, filled at the end.
    pub received: Option<ReceiveEnd>,
}

/// Why a run could not start or finish.
#[derive(Debug)]
pub enum NetError {
    /// The directory already holds a run.
    Exists(PathBuf),
    /// A socket operation failed.
    Zmq(&'static str, ac2_zmq::Error),
    /// The two sides could not agree on a run.
    Handshake(String),
    /// A file could not be written.
    Io(io::Error),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exists(p) => write!(f, "{} already holds a run", p.display()),
            Self::Zmq(what, e) => write!(f, "{what}: {e}"),
            Self::Handshake(m) => f.write_str(m),
            Self::Io(e) => write!(f, "cannot write the log: {e}"),
        }
    }
}

impl std::error::Error for NetError {}

impl From<io::Error> for NetError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

fn zerr(what: &'static str) -> impl FnOnce(ac2_zmq::Error) -> NetError {
    move |e| NetError::Zmq(what, e)
}

/// One end of the link, bound or connected, before the handshake.
#[derive(Debug)]
pub struct Endpoint {
    // Dropped before the contexts below.
    sock: Socket,
    side: Side,
    addr: String,
    server_key: PublicKey,
    _secure: Option<SecureContext>,
    _plain: Option<Context>,
}

impl Endpoint {
    /// This side's endpoint: the bound one (port resolved) or the one connected to.
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// The listener's public key, which the connecting side passes as `--server-key`.
    pub fn server_key(&self) -> &PublicKey {
        &self.server_key
    }

    /// Bound or connected.
    pub fn side(&self) -> Side {
        self.side
    }
}

fn client_keys() -> Result<KeyPair, NetError> {
    Ok(KeyPair {
        public: PublicKey::from_z85(CLIENT_PUBLIC).map_err(zerr("client key"))?,
        secret: SecretKey::from_z85(CLIENT_SECRET).map_err(zerr("client key"))?,
    })
}

fn common_options(s: &Socket) -> Result<(), NetError> {
    s.set_linger(Some(LINGER)).map_err(zerr("linger"))?;
    s.set_send_hwm(SNDHWM).map_err(zerr("SNDHWM"))?;
    s.set_recv_hwm(SNDHWM).map_err(zerr("RCVHWM"))
}

/// Binds a CURVE ROUTER on `addr` (`host:port`; port 0 picks one) with a fresh server key.
pub fn listen(addr: &str) -> Result<Endpoint, NetError> {
    let keys = KeyPair::generate().map_err(zerr("server key"))?;
    let mut authorized = AuthorizedKeys::new();
    authorized
        .insert("selftest-net", client_keys()?.public)
        .map_err(|e| NetError::Handshake(e.to_string()))?;
    let secure = SecureContext::new(ZAP_DOMAIN, authorized, |_| {}).map_err(zerr("ZAP"))?;
    let sock = secure
        .curve_server_socket(SocketType::Router, &keys)
        .map_err(zerr("ROUTER"))?;
    // A send to a vanished receiver then fails instead of vanishing itself.
    sock.set_router_mandatory(true)
        .map_err(zerr("ROUTER_MANDATORY"))?;
    common_options(&sock)?;
    sock.bind(&format!("tcp://{addr}")).map_err(zerr("bind"))?;
    let addr = sock.last_endpoint().map_err(zerr("endpoint"))?;
    Ok(Endpoint {
        sock,
        side: Side::Listen,
        addr,
        server_key: keys.public,
        _secure: Some(secure),
        _plain: None,
    })
}

/// Connects a CURVE DEALER to `addr` (`host:port`), pinning `server_key`.
pub fn connect(addr: &str, server_key: PublicKey) -> Result<Endpoint, NetError> {
    let ctx = Context::new().map_err(zerr("context"))?;
    let sock = ctx.socket(SocketType::Dealer).map_err(zerr("DEALER"))?;
    common_options(&sock)?;
    sock.set_curve_client(&CurveClient {
        keys: client_keys()?,
        server_key,
    })
    .map_err(zerr("CURVE"))?;
    let addr = format!("tcp://{addr}");
    sock.connect(&addr).map_err(zerr("connect"))?;
    Ok(Endpoint {
        sock,
        side: Side::Connect,
        addr,
        server_key,
        _secure: None,
        _plain: Some(ctx),
    })
}

/// A message's routing id (ROUTER side) and its other parts.
type Received = (Option<Vec<u8>>, Vec<Vec<u8>>);

/// The socket with the peer's routing id once known (ROUTER side).
struct Wire<'a> {
    sock: &'a Socket,
    router: bool,
    peer: Option<Vec<u8>>,
}

impl Wire<'_> {
    /// Sends without blocking; `WouldBlock` leaves nothing queued.
    fn send(&self, parts: &[&[u8]]) -> ac2_zmq::Result<()> {
        if self.router {
            let id = self.peer.as_deref().unwrap_or_default();
            let mut all: Vec<&[u8]> = Vec::with_capacity(parts.len() + 1);
            all.push(id);
            all.extend_from_slice(parts);
            self.sock.try_send(&all)
        } else {
            self.sock.try_send(parts)
        }
    }

    /// Sends, retrying a full queue for up to a second (handshake and `end` only).
    fn send_soon(&self, parts: &[&[u8]], what: &'static str) -> Result<(), NetError> {
        let until = Instant::now() + Duration::from_secs(1);
        loop {
            match self.send(parts) {
                Err(e) if e.is_would_block() && Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                r => return r.map_err(zerr(what)),
            }
        }
    }

    /// The next message's parts without the routing id; another peer's are dropped (the
    /// listener serves the first that says hello). The id is returned for the handshake.
    fn recv(&self) -> Result<Option<Received>, NetError> {
        loop {
            let Some(m) = self.sock.try_recv().map_err(zerr("receive"))? else {
                return Ok(None);
            };
            let mut frames = m.into_frames();
            if !self.router {
                return Ok(Some((None, frames)));
            }
            if frames.is_empty() {
                continue;
            }
            let id = frames.remove(0);
            match &self.peer {
                Some(p) if *p != id => continue,
                _ => return Ok(Some((Some(id), frames))),
            }
        }
    }
}

pub(crate) fn realtime_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

fn create(dir: &Path, name: &str) -> Result<File, NetError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(name))
        .map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => NetError::Exists(dir.to_path_buf()),
            _ => NetError::Io(e),
        })
}

/// Writes the listener's public key to `<dir>/server.key` (Z85 and a newline).
pub fn write_server_key(dir: &Path, key: &PublicKey) -> Result<PathBuf, NetError> {
    fs::create_dir_all(dir)?;
    let mut f = create(dir, "server.key")?;
    writeln!(f, "{}", key.to_z85())?;
    Ok(dir.join("server.key"))
}

fn write_meta(dir: &Path, meta: &NetMeta) -> Result<(), NetError> {
    let json = serde_json::to_vec_pretty(meta).map_err(io::Error::other)?;
    fs::write(dir.join("meta.json"), json)?;
    Ok(())
}

/// Offset of block `k` from the start, ns: computed from `k` each time, so rounding never
/// accumulates into drift.
pub(crate) fn offset_ns(k: u64, frames: u32, rate: u32) -> u64 {
    let ns = u128::from(k) * u128::from(frames) * 1_000_000_000 / u128::from(rate.max(1));
    u64::try_from(ns).unwrap_or(u64::MAX)
}

/// Exchanges hellos and, on the receiver, states the window. Returns the peer's hello.
fn handshake(
    wire: &mut Wire<'_>,
    me: &Hello,
    side: Side,
    stop: &AtomicBool,
) -> Result<Hello, NetError> {
    let mine = serde_json::to_vec(me).map_err(io::Error::other)?;
    if side == Side::Connect {
        wire.send_soon(&[TAG_HELLO, &mine], "hello")?;
    }
    let began = Instant::now();
    let peer = loop {
        if stop.load(Ordering::Acquire) {
            return Err(NetError::Handshake(
                "interrupted before the peer said hello".into(),
            ));
        }
        if side == Side::Connect && began.elapsed() > CONNECT_WAIT {
            return Err(NetError::Handshake(format!(
                "no answer from the listener in {} s (check --server-key, the address and \
                 the firewall)",
                CONNECT_WAIT.as_secs()
            )));
        }
        wire.sock.wait_readable(SLICE).map_err(zerr("poll"))?;
        let Some((id, frames)) = wire.recv()? else {
            continue;
        };
        if frames.first().map(Vec::as_slice) != Some(TAG_HELLO) || frames.len() != 2 {
            continue;
        }
        let hello: Hello = serde_json::from_slice(&frames[1])
            .map_err(|e| NetError::Handshake(format!("peer's hello: {e}")))?;
        if wire.router {
            wire.peer = id;
        }
        break hello;
    };
    if side == Side::Listen {
        wire.send_soon(&[TAG_HELLO, &mine], "hello")?;
    }
    if peer.role == me.role {
        return Err(NetError::Handshake(format!(
            "both hosts have --role {}; one sends, one receives",
            match me.role {
                Role::Send => "send",
                Role::Receive => "receive",
            }
        )));
    }
    Ok(peer)
}

fn encode_up(msg: DataUp) -> Result<Vec<u8>, NetError> {
    encode_data_request(&DataRequest::new(msg))
        .map_err(|e| NetError::Handshake(format!("flow message: {e}")))
}

/// Runs one side to the end: handshake, then send or receive for `spec.duration`.
pub fn run(
    ep: Endpoint,
    spec: &NetSpec,
    dir: &Path,
    provenance: Provenance,
    stop: &AtomicBool,
) -> Result<NetMeta, NetError> {
    fs::create_dir_all(dir)?;
    if dir.join("meta.json").exists() {
        return Err(NetError::Exists(dir.to_path_buf()));
    }
    let arrivals = match spec.role {
        Role::Receive => Some(BufWriter::new(create(dir, "arrivals.bin")?)),
        Role::Send => None,
    };
    let mut wire = Wire {
        sock: &ep.sock,
        router: ep.side == Side::Listen,
        peer: None,
    };
    let me = Hello {
        role: spec.role,
        host: provenance.host.clone(),
        rate: spec.rate,
        frames: spec.frames,
        channels: spec.channels,
    };
    let peer = handshake(&mut wire, &me, ep.side, stop)?;
    // The sender's stream is the one on the wire.
    let stream = match spec.role {
        Role::Send => spec.clone(),
        Role::Receive => NetSpec {
            rate: peer.rate,
            frames: peer.frames,
            channels: peer.channels,
            ..spec.clone()
        },
    };
    if stream.rate == 0 || stream.frames == 0 || stream.channels == 0 {
        return Err(NetError::Handshake("the sender's stream is empty".into()));
    }
    let start_realtime_ns = realtime_ns();
    let mut meta = NetMeta {
        format_version: FORMAT_VERSION,
        provenance,
        role: spec.role,
        side: ep.side,
        addr: ep.addr.clone(),
        peer_host: peer.host,
        sample_rate: stream.rate,
        frames: stream.frames,
        channels: stream.channels,
        payload_bytes: stream.payload_bytes(),
        message_bytes: stream.message_bytes(),
        window_bytes: spec.window,
        block_period_us: offset_ns(1_000_000, stream.frames, stream.rate) as f64 / 1e9,
        start_realtime_ns,
        start_utc: chrono::DateTime::<chrono::Utc>::from(SystemTime::now())
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        sent: None,
        received: None,
    };
    write_meta(dir, &meta)?;
    match (spec.role, arrivals) {
        (Role::Send, _) => meta.sent = Some(send(&wire, &stream, stop)?),
        (Role::Receive, Some(mut out)) => {
            let window = encode_up(DataUp::Window { bytes: spec.window })?;
            wire.send_soon(&[TAG_FLOW, &window], "window")?;
            let mut rx = Receiver::new(&stream, GAPS_LISTED);
            out.write_all(MAGIC)?;
            out.write_all(&FORMAT_VERSION.to_le_bytes())?;
            out.write_all(&(RECORD_BYTES as u32).to_le_bytes())?;
            meta.received = Some(receive(&wire, &stream, &mut rx, &mut out, stop)?);
            out.flush()?;
        }
        (Role::Receive, None) => unreachable!("the receiver's file is opened above"),
    }
    write_meta(dir, &meta)?;
    Ok(meta)
}

/// Waits until `due` or a message, whichever is first: `poll` counts whole milliseconds, so
/// the last stretch is slept instead.
fn wait_until(sock: &Socket, due: Instant) -> Result<(), NetError> {
    let left = due.saturating_duration_since(Instant::now());
    if left > Duration::from_millis(2) {
        let poll = (left - Duration::from_millis(1)).min(SLICE);
        sock.wait_readable(poll).map_err(zerr("poll"))?;
    } else if !left.is_zero() {
        std::thread::sleep(left);
    }
    Ok(())
}

/// Reads credit (and the window, until the first arrives). `None` until a window was stated.
fn take_credit(wire: &Wire<'_>, credit: &mut Option<u64>) -> Result<(), NetError> {
    while let Some((_, frames)) = wire.recv()? {
        if frames.len() != 2 || frames[0] != TAG_FLOW {
            continue;
        }
        let Ok(req) = decode_data_request(&frames[1]) else {
            continue;
        };
        match req.msg {
            DataUp::Window { bytes } if credit.is_none() => *credit = Some(u64::from(bytes)),
            DataUp::Credit { bytes, .. } => {
                *credit = Some(credit.unwrap_or(0).saturating_add(u64::from(bytes)));
            }
            _ => {}
        }
    }
    Ok(())
}

fn send(wire: &Wire<'_>, spec: &NetSpec, stop: &AtomicBool) -> Result<SendEnd, NetError> {
    let mut credit: Option<u64> = None;
    while credit.is_none() {
        if stop.load(Ordering::Acquire) {
            return Ok(SendEnd {
                interrupted: true,
                ..SendEnd::default()
            });
        }
        wire.sock.wait_readable(SLICE).map_err(zerr("poll"))?;
        take_credit(wire, &mut credit)?;
    }
    let payload = vec![0u8; usize::try_from(spec.payload_bytes()).unwrap_or(usize::MAX)];
    let charge = u64::from(frame_charge(
        usize::try_from(spec.message_bytes()).unwrap_or(usize::MAX),
    ));
    let period_ns = offset_ns(1, spec.frames, spec.rate);
    let start = Instant::now() + LEAD;
    let start_rt = realtime_ns() + LEAD.as_nanos() as u64;
    let end = start + spec.duration;
    let mut e = SendEnd::default();
    let mut stall_began: Option<Instant> = None;
    let mut last_stall_end: Option<Instant> = None;
    let mut header = [0u8; BLOCK_HEADER_BYTES];
    let mut k: u64 = 0;
    let mut last_send = start;
    loop {
        if stop.load(Ordering::Acquire) {
            e.interrupted = true;
            break;
        }
        let off = offset_ns(k, spec.frames, spec.rate);
        let due = start + Duration::from_nanos(off);
        if due >= end {
            break;
        }
        take_credit(wire, &mut credit)?;
        let now = Instant::now();
        if now < due {
            wait_until(wire.sock, due)?;
            continue;
        }
        if credit.unwrap_or(0) < charge {
            if stall_began.is_none() {
                stall_began = Some(now);
                e.stalls += 1;
            }
            wire.sock
                .wait_readable(Duration::from_millis(10))
                .map_err(zerr("poll"))?;
            continue;
        }
        if let Some(b) = stall_began.take() {
            e.stall_ms += now.duration_since(b).as_secs_f64() * 1e3;
            last_stall_end = Some(now);
        }
        let sent_ns = realtime_ns();
        let late = Instant::now().saturating_duration_since(due);
        let late_ns = u64::try_from(late.as_nanos()).unwrap_or(u64::MAX);
        let is_late = late_ns > period_ns;
        header[0..8].copy_from_slice(&k.to_le_bytes());
        header[8..16].copy_from_slice(&(start_rt + off).to_le_bytes());
        header[16..24].copy_from_slice(&sent_ns.to_le_bytes());
        header[24..32].copy_from_slice(&e.stalls.to_le_bytes());
        header[32..40].copy_from_slice(&(e.late_sends + u64::from(is_late)).to_le_bytes());
        match wire.send(&[TAG_BLOCK, &header, &payload]) {
            Ok(()) => {
                credit = credit.map(|c| c - charge);
                if last_stall_end.is_some_and(|t| due < t) {
                    e.credit_waits += 1;
                }
                if is_late {
                    e.late_sends += 1;
                }
                e.max_late_us = e.max_late_us.max(late_ns as f64 / 1e3);
                k += 1;
                last_send = Instant::now();
            }
            Err(err) if err.is_would_block() => {
                // Credit should rule this out; the block waits as for credit.
                e.queue_refusals += 1;
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(ac2_zmq::Error::HostUnreachable) => {
                e.peer_gone = true;
                break;
            }
            Err(err) => return Err(NetError::Zmq("send", err)),
        }
    }
    if let Some(b) = stall_began {
        e.stall_ms += b.elapsed().as_secs_f64() * 1e3;
    }
    e.blocks = k;
    e.seconds = last_send.saturating_duration_since(start).as_secs_f64();
    if !e.peer_gone {
        let body = serde_json::to_vec(&e).map_err(io::Error::other)?;
        // A receiver already gone is no error at the end.
        let _ = wire.send_soon(&[TAG_END, &body], "end");
    }
    Ok(e)
}

fn receive(
    wire: &Wire<'_>,
    spec: &NetSpec,
    rx: &mut Receiver,
    out: &mut impl Write,
    stop: &AtomicBool,
) -> Result<ReceiveEnd, NetError> {
    let start = Instant::now();
    let cap = start + spec.duration + END_GRACE;
    let half = u64::from(spec.window / 2).max(1);
    let mut owed: u64 = 0;
    let mut sender: Option<SendEnd> = None;
    let paused = |now: Instant| {
        spec.credit_pause.is_some_and(|(from, len)| {
            let t = now.duration_since(start);
            t >= from && t < from + len
        })
    };
    let give = |owed: &mut u64| -> Result<(), NetError> {
        if *owed == 0 {
            return Ok(());
        }
        let bytes = u32::try_from(*owed).unwrap_or(u32::MAX);
        let up = encode_up(DataUp::Credit { bytes, events: 0 })?;
        match wire.send(&[TAG_FLOW, &up]) {
            Ok(()) => {
                *owed -= u64::from(bytes);
                Ok(())
            }
            // Credit stays owed and goes with the next burst.
            Err(e) if e.is_would_block() || matches!(e, ac2_zmq::Error::HostUnreachable) => Ok(()),
            Err(e) => Err(NetError::Zmq("credit", e)),
        }
    };
    let ended = loop {
        if stop.load(Ordering::Acquire) {
            break Ended::Interrupted;
        }
        if sender.is_some() {
            break Ended::Sender;
        }
        if Instant::now() >= cap {
            break Ended::Deadline;
        }
        wire.sock.wait_readable(SLICE).map_err(zerr("poll"))?;
        while let Some((_, frames)) = wire.recv()? {
            let arrived_ns = realtime_ns();
            match frames.first().map(Vec::as_slice) {
                Some(TAG_BLOCK) if frames.len() == 3 => {
                    let h = &frames[1];
                    if h.len() != BLOCK_HEADER_BYTES {
                        continue;
                    }
                    let n: usize = frames.iter().map(Vec::len).sum();
                    owed += u64::from(frame_charge(n));
                    let rec = stats::Arrival::from_header(h, arrived_ns);
                    out.write_all(&rec.to_bytes())?;
                    rx.push(&rec);
                    if owed >= half && !paused(Instant::now()) {
                        give(&mut owed)?;
                    }
                }
                Some(TAG_END) if frames.len() == 2 => {
                    sender = serde_json::from_slice(&frames[1]).ok();
                }
                _ => {}
            }
        }
        if !paused(Instant::now()) {
            give(&mut owed)?;
        }
    };
    Ok(rx.finish(ended, start.elapsed().as_secs_f64(), sender))
}

#[cfg(test)]
#[path = "net_tests.rs"]
mod tests;
