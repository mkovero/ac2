//! Context, sockets, typed options and multipart messages.

use std::ffi::{CString, c_int};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::curve::CurveClient;
use crate::error::{Error, Result, zmq};
use crate::monitor::Monitor;
use crate::poll::{PollItem, poll};
use crate::raw::{self, RawContext, RawSocket};
use crate::zap::ZapGuard;

/// A libzmq context: owns the I/O threads and the inproc namespace. Cheap to clone; terminated
/// when the last clone and the last socket created from it are dropped.
///
/// A plain context cannot create CURVE *server* sockets; see [`crate::SecureContext`].
#[derive(Clone, Debug)]
pub struct Context(pub(crate) Arc<RawContext>);

impl Context {
    /// A new context with libzmq's default options (one I/O thread).
    pub fn new() -> Result<Self> {
        Ok(Self(Arc::new(zmq(RawContext::new())?)))
    }

    /// A new socket of `kind`, with linger 0 (see [`Socket::set_linger`]).
    pub fn socket(&self, kind: SocketType) -> Result<Socket> {
        Socket::new(self, kind.raw(), Some(kind))
    }

    /// A PAIR socket; only monitors use it.
    pub(crate) fn pair_socket(&self) -> Result<Socket> {
        Socket::new(self, raw::ZMQ_PAIR, None)
    }
}

/// The socket types ac2 uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SocketType {
    /// Daemon ctrl socket: async request/reply with routing ids.
    Router,
    /// Client ctrl socket.
    Dealer,
    /// Daemon data socket: publishes and reports subscriptions.
    XPub,
    /// Client data socket.
    Sub,
    /// Feeds an I/O thread from other threads.
    Push,
    /// The I/O thread's end of a [`SocketType::Push`] pipe.
    Pull,
    /// Synchronous replier (the ZAP handler).
    Rep,
}

impl SocketType {
    fn raw(self) -> c_int {
        match self {
            Self::Router => raw::ZMQ_ROUTER,
            Self::Dealer => raw::ZMQ_DEALER,
            Self::XPub => raw::ZMQ_XPUB,
            Self::Sub => raw::ZMQ_SUB,
            Self::Push => raw::ZMQ_PUSH,
            Self::Pull => raw::ZMQ_PULL,
            Self::Rep => raw::ZMQ_REP,
        }
    }
}

/// One received multipart message and the connection metadata libzmq attached to it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    frames: Vec<Vec<u8>>,
    user_id: Option<String>,
    peer_address: Option<String>,
}

impl Message {
    #[cfg(test)]
    pub(crate) fn from_frames(frames: Vec<Vec<u8>>) -> Self {
        Self {
            frames,
            ..Self::default()
        }
    }

    /// All frames, in order. On a ROUTER the first frame is the sender's routing id.
    pub fn frames(&self) -> &[Vec<u8>] {
        &self.frames
    }

    /// The frames, consuming the message.
    pub fn into_frames(self) -> Vec<Vec<u8>> {
        self.frames
    }

    /// The user id the ZAP handler assigned to the sending connection (the authorized key's
    /// name under CURVE). `None` on connections that did not pass ZAP (inproc, NULL
    /// mechanism without a ZAP domain). This, not anything inside the payload, is the
    /// authenticated identity of the peer.
    pub fn user_id(&self) -> Option<&str> {
        self.user_id.as_deref()
    }

    /// The peer's address as libzmq reports it (`Peer-Address`; tcp and ipc only). For logs.
    pub fn peer_address(&self) -> Option<&str> {
        self.peer_address.as_deref()
    }
}

/// Kernel-level dead-peer detection on a socket's TCP connections; see
/// [`Socket::set_tcp_liveness`].
///
/// A peer that vanishes without closing its connection (a laptop lid closed, Wi-Fi gone, a
/// power cut) leaves the connection open for as long as the kernel keeps retransmitting,
/// about a quarter of an hour, with its queue and subscriptions alive. Two kernel timers
/// close it sooner: keepalive probes an idle connection, and the retransmission limit ends
/// one whose sent data goes unacknowledged (a queue towards a dead peer is never idle, so
/// keepalive alone would not fire there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TcpLiveness {
    /// Keepalive probing starts after this long with nothing received.
    pub idle: Duration,
    /// Between keepalive probes.
    pub interval: Duration,
    /// Unanswered keepalive probes before the connection is closed (ignored on Windows).
    pub count: u32,
    /// Longest time sent data may stay unacknowledged before the connection is closed
    /// (`TCP_USER_TIMEOUT` on Linux, `TCP_MAXRT` on Windows; whole milliseconds).
    pub max_unacked: Duration,
}

/// One message part whose content libzmq holds without copying it: received parts stay in
/// the buffer libzmq read them into, parts made with [`Part::from_vec`] keep the `Vec`'s
/// buffer, and [`Part::share`] only adds a reference. Reads as bytes through `Deref`.
///
/// `Send` but not `Sync`.
#[derive(Debug)]
pub struct Part(raw::RawMsg);

impl Part {
    /// A part owning `data`, not copied (libzmq frees it once the last queued share has
    /// been sent).
    pub fn from_vec(data: Vec<u8>) -> Result<Self> {
        zmq(raw::RawMsg::from_vec(data)).map(Self)
    }

    /// A part holding a copy of `data` (for small parts such as tags and topics).
    pub fn copy_from(data: &[u8]) -> Result<Self> {
        zmq(raw::RawMsg::copy_from(data)).map(Self)
    }

    /// Another part with the same content, without copying large content.
    pub fn share(&self) -> Result<Self> {
        zmq(self.0.share()).map(Self)
    }
}

impl std::ops::Deref for Part {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.0.bytes()
    }
}

impl AsRef<[u8]> for Part {
    fn as_ref(&self) -> &[u8] {
        self.0.bytes()
    }
}

/// A libzmq socket.
///
/// `Send` but not `Sync`: a socket may move to another thread but is used by one thread at a
/// time. All options must be set before `bind`/`connect` unless libzmq documents otherwise.
#[derive(Debug)]
pub struct Socket {
    // Declared first: the socket closes before the ZAP guard below may stop the handler.
    raw: RawSocket,
    kind: Option<SocketType>,
    ctx: Context,
    /// CURVE server sockets keep their context's ZAP handler alive (see [`crate::zap`]).
    _zap: Option<Arc<ZapGuard>>,
}

static ENDPOINT_SEQ: AtomicU64 = AtomicU64::new(0);

/// A process-unique inproc endpoint name under `ac2-zmq/<purpose>`.
pub(crate) fn unique_inproc(purpose: &str) -> String {
    format!(
        "inproc://ac2-zmq/{purpose}/{}",
        ENDPOINT_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn c_endpoint(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error::InvalidArgument("endpoint contains NUL"))
}

fn millis(d: Duration, what: &'static str) -> Result<c_int> {
    c_int::try_from(d.as_millis()).map_err(|_| Error::InvalidArgument(what))
}

fn count(n: u32, what: &'static str) -> Result<c_int> {
    c_int::try_from(n).map_err(|_| Error::InvalidArgument(what))
}

impl Socket {
    fn new(ctx: &Context, raw_kind: c_int, kind: Option<SocketType>) -> Result<Self> {
        let s = Self {
            raw: zmq(RawSocket::new(&ctx.0, raw_kind))?,
            kind,
            ctx: ctx.clone(),
            _zap: None,
        };
        // libzmq's default linger is infinite: context teardown would block on undeliverable
        // messages to vanished peers.
        s.set_linger(Some(Duration::ZERO))?;
        Ok(s)
    }

    pub(crate) fn attach_zap(&mut self, guard: Arc<ZapGuard>) {
        self._zap = Some(guard);
    }

    pub(crate) fn raw(&self) -> &RawSocket {
        &self.raw
    }

    /// The type this socket was created with.
    pub fn kind(&self) -> Option<SocketType> {
        self.kind
    }

    fn require(&self, kind: SocketType, what: &'static str) -> Result<()> {
        if self.kind == Some(kind) {
            Ok(())
        } else {
            Err(Error::InvalidArgument(what))
        }
    }

    pub(crate) fn set_int(&self, opt: c_int, v: c_int) -> Result<()> {
        zmq(self.raw.set_int(opt, v))
    }

    pub(crate) fn set_bytes(&self, opt: c_int, v: &[u8]) -> Result<()> {
        zmq(self.raw.set_bytes(opt, v))
    }

    // --- options -------------------------------------------------------------------------

    /// How long unsent messages are kept after close. `None` = until delivered (blocks context
    /// teardown); new sockets start at zero.
    pub fn set_linger(&self, linger: Option<Duration>) -> Result<()> {
        let v = match linger {
            None => -1,
            Some(d) => millis(d, "linger exceeds i32 milliseconds")?,
        };
        self.set_int(raw::ZMQ_LINGER, v)
    }

    /// Outgoing queue limit per peer, in whole messages (multipart counts once). 0 = no limit.
    pub fn set_send_hwm(&self, messages: u32) -> Result<()> {
        self.set_int(raw::ZMQ_SNDHWM, count(messages, "send HWM exceeds i32")?)
    }

    /// Incoming queue limit per peer, in whole messages. 0 = no limit.
    pub fn set_recv_hwm(&self, messages: u32) -> Result<()> {
        self.set_int(raw::ZMQ_RCVHWM, count(messages, "recv HWM exceeds i32")?)
    }

    /// Kernel send buffer (`SO_SNDBUF`) in bytes; `None` = OS default. A small buffer bounds
    /// how much stale data sits in the kernel when a remote subscriber stalls.
    pub fn set_send_buffer(&self, bytes: Option<u32>) -> Result<()> {
        let v = bytes.map_or(Ok(-1), |b| count(b, "send buffer exceeds i32"))?;
        self.set_int(raw::ZMQ_SNDBUF, v)
    }

    /// Kernel receive buffer (`SO_RCVBUF`) in bytes; `None` = OS default.
    pub fn set_recv_buffer(&self, bytes: Option<u32>) -> Result<()> {
        let v = bytes.map_or(Ok(-1), |b| count(b, "recv buffer exceeds i32"))?;
        self.set_int(raw::ZMQ_RCVBUF, v)
    }

    /// Delay before a disconnected connecting socket retries.
    pub fn set_reconnect_interval(&self, interval: Duration) -> Result<()> {
        let v = millis(interval, "reconnect interval exceeds i32 milliseconds")?;
        self.set_int(raw::ZMQ_RECONNECT_IVL, v)
    }

    /// Upper bound of the reconnect back-off: after each failed attempt the delay doubles
    /// from [`Socket::set_reconnect_interval`] up to `max`. `None` = no back-off (every retry
    /// after the base interval), libzmq's default.
    pub fn set_reconnect_interval_max(&self, max: Option<Duration>) -> Result<()> {
        let v = max.map_or(Ok(0), |d| {
            millis(d, "reconnect interval max exceeds i32 milliseconds")
        })?;
        self.set_int(raw::ZMQ_RECONNECT_IVL_MAX, v)
    }

    /// The reconnect back-off bound; see [`Socket::set_reconnect_interval_max`].
    pub fn reconnect_interval_max(&self) -> Result<Option<Duration>> {
        let v = zmq(self.raw.get_int(raw::ZMQ_RECONNECT_IVL_MAX))?;
        Ok((v > 0).then(|| Duration::from_millis(v.unsigned_abs().into())))
    }

    /// Kernel dead-peer detection on this socket's TCP connections, set before
    /// `bind`/`connect` (listeners pass it to every accepted connection); `None` leaves the
    /// OS defaults. Not used on ipc or inproc, whose peers cannot vanish unnoticed.
    ///
    /// This, not libzmq's ZMTP heartbeats, is how ac2 finds dead peers: libzmq 4.3.5's
    /// heartbeat code trips assertions in its stream engine (on a connection whose reads are
    /// paused by a full receive queue, and when a heartbeat timer fires after the connection
    /// failed), which aborts the process.
    pub fn set_tcp_liveness(&self, l: Option<TcpLiveness>) -> Result<()> {
        let Some(l) = l else {
            self.set_int(raw::ZMQ_TCP_KEEPALIVE, -1)?;
            return self.set_int(raw::ZMQ_TCP_MAXRT, 0);
        };
        let secs = |d: Duration, what| {
            c_int::try_from(d.as_secs())
                .ok()
                .filter(|s| *s > 0)
                .ok_or(Error::InvalidArgument(what))
        };
        let idle = secs(l.idle, "keepalive idle must be 1 s to i32 seconds")?;
        let interval = secs(l.interval, "keepalive interval must be 1 s to i32 seconds")?;
        let maxrt = millis(l.max_unacked, "max unacked exceeds i32 milliseconds")?;
        if maxrt <= 0 {
            return Err(Error::InvalidArgument("max unacked must be positive"));
        }
        self.set_int(raw::ZMQ_TCP_KEEPALIVE, 1)?;
        self.set_int(raw::ZMQ_TCP_KEEPALIVE_IDLE, idle)?;
        self.set_int(raw::ZMQ_TCP_KEEPALIVE_INTVL, interval)?;
        self.set_int(
            raw::ZMQ_TCP_KEEPALIVE_CNT,
            count(l.count.max(1), "keepalive count exceeds i32")?,
        )?;
        self.set_int(raw::ZMQ_TCP_MAXRT, maxrt)
    }

    /// The dead-peer detection settings; `None` when left to the OS.
    pub fn tcp_liveness(&self) -> Result<Option<TcpLiveness>> {
        if zmq(self.raw.get_int(raw::ZMQ_TCP_KEEPALIVE))? != 1 {
            return Ok(None);
        }
        let get = |opt| zmq(self.raw.get_int(opt)).map(c_int::unsigned_abs);
        Ok(Some(TcpLiveness {
            idle: Duration::from_secs(get(raw::ZMQ_TCP_KEEPALIVE_IDLE)?.into()),
            interval: Duration::from_secs(get(raw::ZMQ_TCP_KEEPALIVE_INTVL)?.into()),
            count: get(raw::ZMQ_TCP_KEEPALIVE_CNT)?,
            max_unacked: Duration::from_millis(get(raw::ZMQ_TCP_MAXRT)?.into()),
        }))
    }

    /// The outgoing queue limit per peer; see [`Socket::set_send_hwm`].
    pub fn send_hwm(&self) -> Result<u32> {
        zmq(self.raw.get_int(raw::ZMQ_SNDHWM)).map(c_int::unsigned_abs)
    }

    /// The incoming queue limit per peer; see [`Socket::set_recv_hwm`].
    pub fn recv_hwm(&self) -> Result<u32> {
        zmq(self.raw.get_int(raw::ZMQ_RCVHWM)).map(c_int::unsigned_abs)
    }

    /// Maximum time for the ZMTP (and CURVE) handshake; `None` = no limit.
    pub fn set_handshake_interval(&self, interval: Option<Duration>) -> Result<()> {
        let v = interval.map_or(Ok(0), |d| {
            millis(d, "handshake interval exceeds i32 milliseconds")
        })?;
        self.set_int(raw::ZMQ_HANDSHAKE_IVL, v)
    }

    /// Routing id this socket presents to a ROUTER peer: 1–255 bytes, not starting with 0
    /// (reserved by libzmq for generated ids).
    pub fn set_routing_id(&self, id: &[u8]) -> Result<()> {
        if id.is_empty() || id.len() > 255 || id[0] == 0 {
            return Err(Error::InvalidArgument(
                "routing id must be 1-255 bytes, not starting with 0",
            ));
        }
        self.set_bytes(raw::ZMQ_ROUTING_ID, id)
    }

    /// ROUTER: sending to an unknown or disconnected routing id fails with
    /// [`Error::HostUnreachable`] instead of being dropped silently.
    pub fn set_router_mandatory(&self, on: bool) -> Result<()> {
        self.require(SocketType::Router, "router_mandatory needs a ROUTER")?;
        self.set_int(raw::ZMQ_ROUTER_MANDATORY, c_int::from(on))
    }

    /// XPUB: report every subscribe, including duplicates from additional subscribers (the
    /// last unsubscribe of a prefix is reported either way). See
    /// [`crate::SubscriptionTracker`].
    pub fn set_xpub_verbose(&self, on: bool) -> Result<()> {
        self.require(SocketType::XPub, "xpub_verbose needs an XPUB")?;
        self.set_int(raw::ZMQ_XPUB_VERBOSE, c_int::from(on))
    }

    /// SUB: receive messages whose first frame starts with `prefix` (empty = everything).
    pub fn subscribe(&self, prefix: &[u8]) -> Result<()> {
        self.require(SocketType::Sub, "subscribe needs a SUB")?;
        self.set_bytes(raw::ZMQ_SUBSCRIBE, prefix)
    }

    /// SUB: undo one [`Socket::subscribe`] of `prefix`.
    pub fn unsubscribe(&self, prefix: &[u8]) -> Result<()> {
        self.require(SocketType::Sub, "unsubscribe needs a SUB")?;
        self.set_bytes(raw::ZMQ_UNSUBSCRIBE, prefix)
    }

    /// Makes this a CURVE client that only completes the handshake with the server holding
    /// the private half of `config.server_key`. Set before `connect`.
    pub fn set_curve_client(&self, config: &CurveClient) -> Result<()> {
        self.set_bytes(raw::ZMQ_CURVE_SERVERKEY, config.server_key.as_bytes())?;
        self.set_bytes(raw::ZMQ_CURVE_PUBLICKEY, config.keys.public.as_bytes())?;
        self.set_bytes(raw::ZMQ_CURVE_SECRETKEY, config.keys.secret.as_bytes())
    }

    /// The endpoint this socket last bound or connected, with wildcards resolved (e.g. the
    /// port picked for `tcp://127.0.0.1:*`).
    pub fn last_endpoint(&self) -> Result<String> {
        let mut buf = [0u8; 1024];
        let len = zmq(self.raw.get_bytes(raw::ZMQ_LAST_ENDPOINT, &mut buf))?;
        let bytes = buf[..len].split(|b| *b == 0).next().unwrap_or_default();
        String::from_utf8(bytes.to_vec())
            .map_err(|_| Error::InvalidArgument("endpoint is not UTF-8"))
    }

    // --- connections -----------------------------------------------------------------------

    /// Binds to `endpoint` (`tcp://host:port`, `ipc://path`, `inproc://name`).
    pub fn bind(&self, endpoint: &str) -> Result<()> {
        zmq(self.raw.bind(&c_endpoint(endpoint)?))
    }

    /// Connects to `endpoint`; libzmq connects (and reconnects) in the background.
    pub fn connect(&self, endpoint: &str) -> Result<()> {
        zmq(self.raw.connect(&c_endpoint(endpoint)?))
    }

    /// Starts reporting this socket's connection events. Attach before `bind`/`connect` to
    /// see every event. A socket has at most one monitor; a new one replaces the previous.
    pub fn monitor(&self) -> Result<Monitor> {
        let ep = unique_inproc("monitor");
        zmq(self.raw.monitor(&c_endpoint(&ep)?, raw::ZMQ_EVENT_ALL))?;
        let pair = self.ctx.pair_socket()?;
        pair.connect(&ep)?;
        Ok(Monitor::new(pair))
    }

    // --- messages --------------------------------------------------------------------------

    /// Sends `frames` as one atomic multipart message, blocking while the peer queue is full.
    pub fn send<B: AsRef<[u8]>>(&self, frames: &[B]) -> Result<()> {
        self.send_with(frames, 0)
    }

    /// Sends without blocking; [`Error::WouldBlock`] when the message cannot be queued now
    /// (nothing of it is queued then).
    pub fn try_send<B: AsRef<[u8]>>(&self, frames: &[B]) -> Result<()> {
        self.send_with(frames, raw::ZMQ_DONTWAIT)
    }

    fn send_with<B: AsRef<[u8]>>(&self, frames: &[B], flags: c_int) -> Result<()> {
        let Some(last) = frames.len().checked_sub(1) else {
            return Err(Error::InvalidArgument("a message has at least one frame"));
        };
        for (i, f) in frames.iter().enumerate() {
            let more = if i < last { raw::ZMQ_SNDMORE } else { 0 };
            // libzmq admits a multipart message as a whole: only the first frame can hit the
            // high-water mark, so `WouldBlock` never leaves a partial message queued.
            zmq(self.raw.send_frame(f.as_ref(), flags | more))?;
        }
        Ok(())
    }

    /// Sends `parts` as one atomic multipart message without blocking and without copying
    /// their content: each is shared ([`Part::share`]), so the caller keeps them (to send
    /// again, or to retry after [`Error::WouldBlock`], when nothing was queued).
    pub fn try_send_parts(&self, parts: &[Part]) -> Result<()> {
        let Some(last) = parts.len().checked_sub(1) else {
            return Err(Error::InvalidArgument("a message has at least one frame"));
        };
        for (i, p) in parts.iter().enumerate() {
            let more = if i < last { raw::ZMQ_SNDMORE } else { 0 };
            let mut m = zmq(p.0.share())?;
            // As in `send_with`: only the first part can hit the high-water mark.
            zmq(self.raw.send_msg(&mut m, raw::ZMQ_DONTWAIT | more))?;
        }
        Ok(())
    }

    /// Receives one whole multipart message if one is queued, its parts left in libzmq's
    /// buffers (no copy, no connection metadata); `Ok(None)` otherwise.
    pub fn try_recv_parts(&self) -> Result<Option<Vec<Part>>> {
        let (first, mut more) = match zmq(self.raw.recv_msg(raw::ZMQ_DONTWAIT)) {
            Ok(m) => m,
            Err(Error::WouldBlock) => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut parts = vec![Part(first)];
        while more {
            // The remaining parts of a multipart message arrive atomically with the first.
            let (p, m) = zmq(self.raw.recv_msg(0))?;
            more = m;
            parts.push(Part(p));
        }
        Ok(Some(parts))
    }

    /// Receives one whole multipart message, blocking until one arrives.
    pub fn recv(&self) -> Result<Message> {
        self.recv_with(0)
    }

    /// Receives one message if one is queued; `Ok(None)` otherwise.
    pub fn try_recv(&self) -> Result<Option<Message>> {
        match self.recv_with(raw::ZMQ_DONTWAIT) {
            Ok(m) => Ok(Some(m)),
            Err(Error::WouldBlock) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Receives one message, waiting at most `timeout`; `Ok(None)` on timeout.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<Message>> {
        if self.wait_readable(timeout)? {
            self.try_recv()
        } else {
            Ok(None)
        }
    }

    /// Waits until a message is queued or `timeout` passes.
    pub fn wait_readable(&self, timeout: Duration) -> Result<bool> {
        let mut items = [PollItem::readable(self)];
        poll(&mut items, Some(timeout))?;
        Ok(items[0].is_readable())
    }

    /// Whether a message is queued now (`ZMQ_EVENTS`). Also clears the pending signal of
    /// [`Socket::notify_fd`], so it must be checked before every wait on that descriptor.
    pub fn has_message(&self) -> Result<bool> {
        let ev = zmq(self.raw.get_int(raw::ZMQ_EVENTS))?;
        Ok(ev & c_int::from(raw::ZMQ_POLLIN) != 0)
    }

    /// The descriptor libzmq signals when this socket's state may have changed (`ZMQ_FD`),
    /// for waiting in an event loop instead of in [`Socket::wait_readable`]. It is
    /// edge-triggered and says nothing by itself: a waiter checks
    /// [`Socket::has_message`] first, waits for the descriptor to become readable only when
    /// that is `false`, and checks again after every wake. Never read from or closed by the
    /// caller; valid as long as the socket.
    #[cfg(unix)]
    pub fn notify_fd(&self) -> Result<std::os::fd::RawFd> {
        zmq(self.raw.get_int(raw::ZMQ_FD))
    }

    fn recv_with(&self, flags: c_int) -> Result<Message> {
        let first = zmq(self.raw.recv_frame(flags, true))?;
        let mut more = first.more;
        let mut msg = Message {
            frames: vec![first.data],
            user_id: first.user_id,
            peer_address: first.peer_address,
        };
        while more {
            // The remaining frames of a multipart message arrive atomically with the first.
            let f = zmq(self.raw.recv_frame(0, false))?;
            more = f.more;
            msg.frames.push(f.data);
        }
        Ok(msg)
    }
}
