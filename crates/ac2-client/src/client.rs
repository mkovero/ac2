//! The client: connect, typed calls with retry, mirrored state, data subscriptions.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac2_proto::model::{ClientRole, State};
use ac2_proto::units::{ClientId, RequestId};
use ac2_proto::{
    Command, ErrorCode, ErrorDetail, GridDef, GridId, ProtoError, Reply, ReplyBody, Request,
    StateSnapshot, Subscription, Warning, Welcome, encode_request,
};
use ac2_zmq::{Context, CurveClient, Socket, SocketType, TcpLiveness};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::data::DataState;
use crate::endpoint::Endpoints;
use crate::error::ClientError;
use crate::io::{Io, SyncIn, wall_ns};
use crate::mirror::{Mirror, MirrorView, Need};

/// How a request is retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    /// Wait for a reply this long per send.
    pub timeout: Duration,
    /// Resends of the same request id after a timeout. The daemon deduplicates ids, so a
    /// resend never executes the command twice.
    pub retries: u32,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            timeout: Duration::from_millis(1500),
            retries: 2,
        }
    }
}

/// Connection settings.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Where the daemon listens.
    pub endpoints: Endpoints,
    /// CURVE (remote mode). `None` for local transports.
    pub curve: Option<CurveClient>,
    /// Client software name and version, sent in `hello`.
    pub name: String,
    /// Default retry policy of [`Client::call`].
    pub retry: Retry,
    /// Mirror the daemon state (snapshot + events). Keepalives (liveness, clock offset,
    /// incarnation) are always tracked.
    pub mirror: bool,
    /// The ZeroMQ context to create the sockets in; `None` = a context of the client's own.
    /// An `inproc://` daemon (embedded in this process) is reachable only through its own
    /// context, and then nothing crosses the kernel.
    pub context: Option<Context>,
    /// Bytes of every message received from the daemon (replies, events, keepalives, data
    /// frames), as decoded by ZeroMQ: CURVE and TCP overhead are not in it. Shared, so its
    /// owner can read the rate across reconnects; each connect adds to the same counter.
    pub received_bytes: Arc<AtomicU64>,
    /// No keepalive for this long (counted from the connect until the first one): the
    /// connection is dead as far as this client is concerned and [`Client::await_dead`]
    /// resolves, whatever the kernel thinks of the TCP connection. [`DEAD_AFTER`] by default.
    pub dead_after: Duration,
    /// Frame bytes the daemon may have in flight towards this client's data connection
    /// before it waits for credit, which each [`Client::latest`] returns for what it read
    /// ([`ac2_proto::flow::DATA_WINDOW_DEFAULT`] by default; the daemon clamps it). The
    /// standing delay of a consumer slower than the daemon is this divided by its rate.
    pub data_window: u32,
    /// Log every data frame each drain reads into [`crate::Latest::arrivals`] (diagnostics:
    /// arrival spacing, age, size per frame). Off by default: the log costs a clock read
    /// and an entry per frame.
    pub record_arrivals: bool,
}

/// Default [`ClientConfig::dead_after`]: the application-level watchdog above the kernel's.
///
/// The daemon sends keepalives at 4 Hz, so 10 s is forty missed in a row: a daemon too busy
/// to send one, or a Wi-Fi roam, is over long before that, while a kiosk is back within the
/// time an operator would wait before reaching for the power switch. It is also longer than
/// the kernel's own detection (5 s idle + 3 × 1 s probes, 15 s unacknowledged send), so a
/// TCP connection the kernel can judge is closed by the kernel first and this fires only
/// where the kernel cannot judge: a middlebox answering the probes, a platform whose probe
/// count is fixed, or a daemon that is reachable but silent.
pub const DEAD_AFTER: Duration = Duration::from_secs(10);

/// First retry after a lost or refused connection.
const RECONNECT_FIRST: Duration = Duration::from_millis(100);
/// Upper bound of the reconnect back-off: an unreachable daemon (a Pi switched off, out of
/// Wi-Fi range) costs one attempt per socket every 2 s instead of ten a second, while a
/// daemon that restarts is found again within that.
const RECONNECT_MAX: Duration = Duration::from_secs(2);
/// Dead-peer detection towards the daemon (TCP only): a daemon host that vanished without
/// closing the connection (power cut, network gone) leaves it open for as long as the
/// kernel retransmits, and a data connection that has nothing to send would not notice
/// even then. With
/// these an idle connection is probed after 5 s and closed after three unanswered probes,
/// one with unacknowledged data after 15 s, and the reconnect loop takes over, so a daemon
/// that comes back is reached again.
const DAEMON_LIVENESS: TcpLiveness = TcpLiveness {
    idle: Duration::from_secs(5),
    interval: Duration::from_secs(1),
    count: 3,
    max_unacked: Duration::from_secs(15),
};
/// Receive queue of the data DEALER, in messages: what the daemon's credit lets be in
/// flight (its own send queue is sized the same), so libzmq never stops reading the
/// connection while the window is open and nothing in flight waits in the kernel instead.
const DATA_RCVHWM: u32 = 2
    * (ac2_proto::flow::DATA_WINDOW_MAX / ac2_proto::flow::FRAME_CHARGE_MIN
        + 1
        + ac2_proto::flow::EVENT_BOUND);

/// Largest part a data socket accepts from the daemon, bytes.
const DATA_MAX_PART: usize = ac2_proto::MAX_DATA_PART_BYTES + ac2_proto::TRANSPORT_SLACK_BYTES;

/// A data socket's `ZMQ_FD` (the DEALER's or its monitor's), owned by libzmq: registered
/// with the runtime for readiness only, never read or closed here.
#[cfg(unix)]
#[derive(Debug)]
struct NotifyFd(std::os::fd::RawFd);

#[cfg(unix)]
impl std::os::fd::AsRawFd for NotifyFd {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0
    }
}

/// Reconnect back-off and dead-peer detection, shared by every client socket.
fn liveness(s: &Socket) -> Result<(), ClientError> {
    s.set_reconnect_interval(RECONNECT_FIRST)?;
    s.set_reconnect_interval_max(Some(RECONNECT_MAX))?;
    s.set_tcp_liveness(Some(DAEMON_LIVENESS))?;
    Ok(())
}

impl ClientConfig {
    /// Local daemon, default settings, mirroring on.
    pub fn local(name: impl Into<String>) -> Self {
        Self::new(Endpoints::local(), name)
    }

    /// Settings for `endpoints`.
    pub fn new(endpoints: Endpoints, name: impl Into<String>) -> Self {
        Self {
            endpoints,
            curve: None,
            name: name.into(),
            retry: Retry::default(),
            mirror: true,
            context: None,
            received_bytes: Arc::default(),
            dead_after: DEAD_AFTER,
            data_window: ac2_proto::flow::DATA_WINDOW_DEFAULT,
            record_arrivals: false,
        }
    }

    /// What follows "… is not responding" when the daemon does not answer: for another
    /// host, the firewall hint (empty for a local daemon). A daemon that refuses this
    /// client's key does answer, with [`ClientError::NotAuthorized`].
    pub fn not_responding_hints(&self) -> String {
        let mut s = String::new();
        if let Some(h) = self.endpoints.firewall_hint() {
            s.push_str("; ");
            s.push_str(&h);
        }
        s
    }
}

/// How often a call waiting for its reply looks whether the daemon refused the key.
const REFUSAL_POLL: Duration = Duration::from_millis(100);

/// The request half: ids, encoding, retry. Shared with the sync and lease tasks.
#[derive(Debug)]
pub(crate) struct CallCore {
    io: Io,
    next_id: AtomicU64,
    retry: Retry,
    /// This client's CURVE key, named when the daemon refuses it.
    key: Option<ac2_zmq::PublicKey>,
}

impl CallCore {
    fn next_id(&self) -> RequestId {
        RequestId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// The daemon refused this client's key: no reply will come until it is authorized.
    fn refusal(&self) -> Option<ClientError> {
        let key = self.key.as_ref()?;
        self.io.refused().then(|| ClientError::NotAuthorized {
            fingerprint: key.fingerprint(),
            key: key.to_z85(),
        })
    }

    pub(crate) async fn call(&self, cmd: Command, retry: Retry) -> Result<ReplyBody, ClientError> {
        self.call_warned(cmd, retry).await.map(|(body, _)| body)
    }

    /// [`Self::call`], with the reply's warnings.
    pub(crate) async fn call_warned(
        &self,
        cmd: Command,
        retry: Retry,
    ) -> Result<(ReplyBody, Vec<Warning>), ClientError> {
        let (op, id, bytes, mut rx) = self.start(cmd)?;
        // Removes the waiter however this future ends (reply, timeout or cancellation).
        struct Guard<'a>(&'a CallCore, u64);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.forget(self.1);
            }
        }
        let _guard = Guard(self, id);
        self.finish(op, &bytes, &mut rx, retry).await
    }

    /// [`Self::start`], the reply to be awaited apart ([`SentCall::reply`]).
    pub(crate) fn send(
        self: &Arc<Self>,
        cmd: Command,
        retry: Retry,
    ) -> Result<SentCall, ClientError> {
        let (op, id, bytes, rx) = self.start(cmd)?;
        Ok(SentCall {
            core: self.clone(),
            op,
            id,
            bytes,
            rx,
            retry,
        })
    }

    fn forget(&self, id: u64) {
        if let Ok(mut p) = self.io.pending.lock() {
            p.remove(&id);
        }
    }

    /// Encodes `cmd`, registers its waiter and sends it once, before anything is awaited:
    /// what is started in order reaches the socket in that order.
    fn start(&self, cmd: Command) -> Result<Started, ClientError> {
        let op = cmd.name();
        let req = Request::new(self.next_id(), cmd);
        let bytes = encode_request(&req).map_err(ClientError::Encode)?;
        if bytes.len() > ac2_proto::MAX_CTRL_BYTES {
            return Err(ClientError::TooLarge {
                op,
                bytes: bytes.len(),
            });
        }
        let (tx, rx) = oneshot::channel();
        let id = req.id.0;
        self.io
            .pending
            .lock()
            .map_err(|_| ClientError::Closed)?
            .insert(id, tx);
        if let Err(e) = self.io.send(&bytes) {
            self.forget(id);
            return Err(e);
        }
        Ok((op, id, bytes, rx))
    }

    /// The reply to a request [`Self::start`] sent, resent after each timeout the retry
    /// policy allows.
    async fn finish(
        &self,
        op: &'static str,
        bytes: &[u8],
        rx: &mut oneshot::Receiver<Result<Reply, ClientError>>,
        retry: Retry,
    ) -> Result<(ReplyBody, Vec<Warning>), ClientError> {
        let attempts = retry.retries.saturating_add(1);
        for attempt in 0..attempts {
            if attempt > 0 {
                self.io.send(bytes)?;
            }
            // In slices: a refused key is known within a handshake, long before the
            // attempt's timeout.
            let until = tokio::time::Instant::now() + retry.timeout;
            loop {
                let left = until.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    break;
                }
                match tokio::time::timeout(left.min(REFUSAL_POLL), &mut *rx).await {
                    Ok(Ok(reply)) => {
                        let reply = reply?;
                        return match reply.result {
                            Ok(body) => Ok((body, reply.warnings)),
                            Err(e) => Err(ClientError::Daemon(e)),
                        };
                    }
                    Ok(Err(_)) => return Err(ClientError::Closed),
                    Err(_) => {
                        if let Some(e) = self.refusal() {
                            return Err(e);
                        }
                    }
                }
            }
        }
        Err(self
            .refusal()
            .unwrap_or(ClientError::Timeout { op, attempts }))
    }

    /// Sends without waiting for the reply (used from `Drop`).
    pub(crate) fn send_nowait(&self, cmd: Command) {
        let req = Request::new(self.next_id(), cmd);
        if let Ok(bytes) = encode_request(&req) {
            let _ = self.io.send(&bytes);
        }
    }
}

type Started = (
    &'static str,
    u64,
    Vec<u8>,
    oneshot::Receiver<Result<Reply, ClientError>>,
);

/// A request sent whose reply is still to come ([`Client::send_warned`]).
#[derive(Debug)]
pub struct SentCall {
    core: Arc<CallCore>,
    op: &'static str,
    id: u64,
    bytes: Vec<u8>,
    rx: oneshot::Receiver<Result<Reply, ClientError>>,
    retry: Retry,
}

impl SentCall {
    /// The reply, with its warnings; resent after a timeout as [`Client::call`] does.
    pub async fn reply(mut self) -> Result<(ReplyBody, Vec<Warning>), ClientError> {
        let core = self.core.clone();
        core.finish(self.op, &self.bytes, &mut self.rx, self.retry)
            .await
    }
}

impl Drop for SentCall {
    fn drop(&mut self) {
        self.core.forget(self.id);
    }
}

/// Unwraps one reply body variant or fails with [`ClientError::UnexpectedReply`].
#[macro_export]
macro_rules! expect_body {
    ($op:expr, $reply:expr, $pat:pat => $out:expr) => {
        match $reply {
            $pat => Ok($out),
            other => Err($crate::ClientError::UnexpectedReply {
                op: $op,
                got: $crate::body_name(&other),
            }),
        }
    };
}

#[derive(Debug)]
struct Inner {
    core: Arc<CallCore>,
    welcome: Arc<Mutex<Welcome>>,
    view: watch::Receiver<Arc<MirrorView>>,
    /// Registration of the data DEALER's notification descriptor with the runtime, made on
    /// the first [`Client::data_changed`]. Declared before `data` so it is deregistered
    /// before the socket (and its descriptor) closes.
    #[cfg(unix)]
    data_fd: std::sync::OnceLock<tokio::io::unix::AsyncFd<NotifyFd>>,
    /// The same for the DEALER's monitor, whose new handshake a drain answers.
    #[cfg(unix)]
    mon_fd: std::sync::OnceLock<tokio::io::unix::AsyncFd<NotifyFd>>,
    data: Mutex<DataState>,
    grids: Mutex<HashMap<GridId, Arc<GridDef>>>,
    sync_task: JoinHandle<()>,
    name: String,
    dead_after: Duration,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.sync_task.abort();
        // Requests already queued (a lease release) leave before the thread stops.
        self.core.io.quit();
    }
}

/// A connected client. Cheap to clone; the connection closes with the last clone (and any
/// [`crate::StimulusLease`] holding one).
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    // Request ids only need to differ from the previous process's within the daemon's 30 s
    // dedup window; the clock is the fallback when the OS RNG is unavailable.
    if getrandom::fill(&mut b).is_err() {
        return wall_ns() as u64;
    }
    u64::from_le_bytes(b)
}

impl Client {
    /// Connects, says `hello` (refusing another protocol version) and starts the sync task.
    /// Must be called inside a tokio runtime.
    pub async fn connect(cfg: ClientConfig) -> Result<Self, ClientError> {
        // A socket directory not there yet is a daemon not running, as a missing socket
        // is; it is checked once a daemon answered in it.
        #[cfg(unix)]
        let mut absent = Vec::new();
        #[cfg(unix)]
        for ep in [&cfg.endpoints.ctrl, &cfg.endpoints.data] {
            if !ac2_paths::socket_dir::check_endpoint_present(ep).map_err(ClientError::SocketDir)? {
                absent.push(ep.clone());
            }
        }
        let ctx = match &cfg.context {
            Some(c) => c.clone(),
            None => Context::new()?,
        };
        let curve = cfg.curve.as_ref();
        let dealer = ctx.socket(SocketType::Dealer)?;
        // Every receiving socket is bounded: libzmq would otherwise buffer a part of any
        // size before the protocol's own bound is checked.
        dealer.set_max_message_size(Some(
            ac2_proto::MAX_CTRL_BYTES + ac2_proto::TRANSPORT_SLACK_BYTES,
        ))?;
        // A lease release queued on shutdown still leaves.
        dealer.set_linger(Some(Duration::from_millis(500)))?;
        liveness(&dealer)?;
        if let Some(c) = curve {
            dealer.set_curve_client(c)?;
        }
        // Before the connect: the daemon's answer to the first handshake is the one wanted.
        let dealer_mon = dealer.monitor()?;
        dealer.connect(&cfg.endpoints.ctrl)?;

        // Q5 step 1: evt and ka are subscribed before anything else is asked. Events get a
        // data connection of their own, read by the I/O thread, so that frames waiting to be
        // drained never delay an event.
        let sync = ctx.socket(SocketType::Dealer)?;
        sync.set_max_message_size(Some(DATA_MAX_PART))?;
        liveness(&sync)?;
        if let Some(c) = curve {
            sync.set_curve_client(c)?;
        }
        let sync_mon = sync.monitor()?;
        sync.connect(&cfg.endpoints.data)?;

        let data = ctx.socket(SocketType::Dealer)?;
        data.set_max_message_size(Some(DATA_MAX_PART))?;
        liveness(&data)?;
        data.set_recv_hwm(DATA_RCVHWM)?;
        if let Some(c) = curve {
            data.set_curve_client(c)?;
        }
        let data_mon = data.monitor()?;
        data.connect(&cfg.endpoints.data)?;
        let data_state = DataState::new(
            data,
            data_mon,
            cfg.data_window,
            Arc::clone(&cfg.received_bytes),
            cfg.record_arrivals,
        )?;

        let (sync_tx, sync_rx) = mpsc::unbounded_channel();
        let received = Arc::clone(&cfg.received_bytes);
        let io = Io::start(
            &ctx,
            (dealer, dealer_mon),
            crate::io::SyncSocket::new(sync, sync_mon, &Subscription::sync_set())?,
            sync_tx,
            received,
        )?;
        let core = Arc::new(CallCore {
            io,
            // Random start: under CURVE two processes share one client id (the key name), and
            // the daemon dedups by (client id, request id).
            next_id: AtomicU64::new(random_u64() >> 1),
            retry: cfg.retry,
            key: curve.map(|c| c.keys.public),
        });
        let welcome = hello(&core, &cfg.name).await?;
        #[cfg(unix)]
        for ep in &absent {
            ac2_paths::socket_dir::check_endpoint(ep).map_err(ClientError::SocketDir)?;
        }
        let mirror = Mirror::new(cfg.mirror);
        let (view_tx, view_rx) = watch::channel(Arc::new(view_of(&mirror, &welcome)));
        let welcome = Arc::new(Mutex::new(welcome));
        let sync_task = tokio::spawn(run_sync(
            core.clone(),
            sync_rx,
            view_tx,
            mirror,
            cfg.name.clone(),
            welcome.clone(),
        ));
        let inner = Inner {
            core,
            welcome,
            view: view_rx,
            #[cfg(unix)]
            data_fd: std::sync::OnceLock::new(),
            #[cfg(unix)]
            mon_fd: std::sync::OnceLock::new(),
            data: Mutex::new(data_state),
            grids: Mutex::new(HashMap::new()),
            sync_task,
            name: cfg.name,
            dead_after: cfg.dead_after,
        };
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    pub(crate) fn core(&self) -> &Arc<CallCore> {
        &self.inner.core
    }

    /// The daemon's `welcome` (refreshed after a daemon restart).
    pub fn welcome(&self) -> Welcome {
        self.inner
            .welcome
            .lock()
            .map(|w| w.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    /// The identity the daemon bound to this connection.
    pub fn client_id(&self) -> ClientId {
        self.welcome().client_id
    }

    /// What this connection may do, as the latest `welcome` said: a demotion or promotion
    /// drops the data connection, and the client asks again when it rejoins.
    pub fn role(&self) -> ClientRole {
        self.welcome().role
    }

    /// The name sent in `hello`.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Sends `cmd` with the default retry policy.
    pub async fn call(&self, cmd: Command) -> Result<ReplyBody, ClientError> {
        view_only_gate(self.role(), &cmd)?;
        let r = self.inner.core.retry;
        self.inner.core.call(cmd, r).await
    }

    /// Sends `cmd` with the default retry policy; the reply body with the warnings the
    /// daemon attached (what the command did that the operator should hear about, e.g. a
    /// `stream.close` leaving the rig meters blind).
    pub async fn call_warned(
        &self,
        cmd: Command,
    ) -> Result<(ReplyBody, Vec<Warning>), ClientError> {
        view_only_gate(self.role(), &cmd)?;
        let r = self.inner.core.retry;
        self.inner.core.call_warned(cmd, r).await
    }

    /// Sends `cmd` now, with the default retry policy; the reply is awaited apart. Calls
    /// sent this way reach the daemon in the order they were sent, which it executes them
    /// in, however their replies are awaited.
    pub fn send_warned(&self, cmd: Command) -> Result<SentCall, ClientError> {
        view_only_gate(self.role(), &cmd)?;
        let core = &self.inner.core;
        core.send(cmd, core.retry)
    }

    /// Sends `cmd` with an explicit retry policy.
    pub async fn call_with(&self, cmd: Command, retry: Retry) -> Result<ReplyBody, ClientError> {
        view_only_gate(self.role(), &cmd)?;
        self.inner.core.call(cmd, retry).await
    }

    /// `state.snapshot`.
    pub async fn snapshot(&self) -> Result<StateSnapshot, ClientError> {
        let op = "state.snapshot";
        let r = self.call(Command::StateSnapshot).await?;
        expect_body!(op, r, ReplyBody::Snapshot(s) => *s)
    }

    /// Current mirror view.
    pub fn view(&self) -> Arc<MirrorView> {
        self.inner.view.borrow().clone()
    }

    /// A receiver that wakes on every mirror change (state, keepalive, incarnation).
    pub fn watch(&self) -> watch::Receiver<Arc<MirrorView>> {
        self.inner.view.clone()
    }

    /// Resolves once no keepalive has arrived for [`ClientConfig::dead_after`] (counted from
    /// the connect until the first one), with the length of the silence. The transport may
    /// still look connected: a peer that vanished without closing leaves the TCP connection
    /// half-open until the kernel's probes give up, and a subscription never sends, so a
    /// caller that wants to be sure drops this client and connects anew.
    pub async fn await_dead(&self) -> Duration {
        let dead_after = self.inner.dead_after;
        let mut rx = self.watch();
        loop {
            let silent = rx.borrow_and_update().silent_for(Instant::now());
            if silent >= dead_after {
                return silent;
            }
            tokio::select! {
                changed = rx.changed() => {
                    if changed.is_err() {
                        // The sync task is gone: nothing will ever arrive.
                        return silent;
                    }
                }
                () = tokio::time::sleep(dead_after - silent) => {}
            }
        }
    }

    /// Waits until the mirror is synced, at most `timeout`.
    pub async fn wait_synced(&self, timeout: Duration) -> Result<Arc<State>, ClientError> {
        let mut rx = self.watch();
        let fut = async {
            loop {
                let v = rx.borrow_and_update().clone();
                if let (true, Some(s)) = (v.synced(), v.state.clone()) {
                    return Ok(s);
                }
                if rx.changed().await.is_err() {
                    return Err(ClientError::Closed);
                }
            }
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| ClientError::Timeout {
                op: "state sync",
                attempts: 1,
            })?
    }

    /// The rig said it turned its network off and was not heard from since (network only).
    pub fn network_turned_off(&self) -> bool {
        self.inner.core.io.network_off()
    }

    /// Ctrl and data messages dropped as malformed by the I/O thread so far.
    pub fn malformed_ctrl(&self) -> u64 {
        self.inner.core.io.malformed.load(Ordering::Relaxed)
    }

    // ----- data ------------------------------------------------------------------------

    /// Subscribes the data connection to `sub` (reference counted).
    pub fn subscribe(&self, sub: Subscription) -> Result<(), ClientError> {
        self.lock_data()?.subscribe(sub)
    }

    /// Undoes one [`Client::subscribe`] of `sub`; its kept frames are dropped when the last
    /// subscription of it goes.
    pub fn unsubscribe(&self, sub: Subscription) -> Result<(), ClientError> {
        self.lock_data()?.unsubscribe(sub)
    }

    /// The newest frame per topic, with age and STALE computed now. Every message received
    /// since the previous call is applied, and the values of the newest per topic built.
    /// Frames of an older stream epoch or another incarnation are discarded. Returns the
    /// credit for what it read: a consumer that calls this rarely is sent less.
    pub fn latest(&self) -> Result<crate::data::Latest, ClientError> {
        let mut out = crate::data::Latest::default();
        self.latest_into(&mut out)?;
        Ok(out)
    }

    /// [`Client::latest`] into `out`, updated in place: a caller that keeps one `Latest`
    /// across frames reuses its map, and its topic names are shared, never rebuilt.
    pub fn latest_into(&self, out: &mut crate::data::Latest) -> Result<(), ClientError> {
        let view = self.view();
        self.lock_data()?
            .drain(&view, Instant::now(), wall_ns(), out)
    }

    /// Waits until a data frame is queued that [`Client::latest`] has not read yet (at once
    /// if one already is), so a consumer sleeps instead of polling:
    ///
    /// ```text
    /// loop {
    ///     client.data_changed().await;
    ///     client.latest_into(&mut latest)?;
    ///     // render; a consumer that wants fewer wake-ups than frames waits for its next
    ///     // frame time here before draining again.
    /// }
    /// ```
    ///
    /// Nothing wakes in between: on Unix the runtime watches the data socket's descriptor,
    /// no thread of the client is involved. Cancellation-safe. Ages and STALE still need a
    /// refresh of their own while no frame arrives. Returns at once on a socket error, which
    /// `latest` then reports. Also returns on a new connection to the daemon, which only a
    /// drain introduces (it states the subscriptions again), and on the keepalive the
    /// daemon sends a connection nothing else went to for a second.
    pub async fn data_changed(&self) {
        #[cfg(unix)]
        loop {
            // The descriptors only signal changes; the queues themselves are checked first
            // and after every wake.
            let (fd, mon) = match self.data_has_message() {
                Ok(true) | Err(_) => return,
                Ok(false) => match (self.data_notify_fd(false), self.data_notify_fd(true)) {
                    (Ok(fd), Ok(mon)) => (fd, mon),
                    _ => return,
                },
            };
            let woke = tokio::select! {
                r = fd.readable() => r.map(|mut g| g.clear_ready()),
                r = mon.readable() => r.map(|mut g| g.clear_ready()),
            };
            if woke.is_err() {
                return;
            }
        }
        #[cfg(not(unix))]
        loop {
            // No descriptor to hand the runtime (a Winsock handle): look at the frame rate
            // of the fastest display, the most a consumer can use.
            match self.data_has_message() {
                Ok(true) | Err(_) => return,
                Ok(false) => tokio::time::sleep(Duration::from_millis(8)).await,
            }
        }
    }

    /// Blocking form of [`Client::data_changed`] for threads outside a runtime; `false`
    /// when `timeout` passed first. Holds the data lock while it waits.
    pub fn wait_data(&self, timeout: Duration) -> bool {
        self.lock_data()
            .and_then(|d| {
                let mut items = [
                    ac2_zmq::PollItem::readable(d.socket()),
                    ac2_zmq::PollItem::readable(d.monitor_socket()),
                ];
                Ok(ac2_zmq::poll(&mut items, Some(timeout))? > 0)
            })
            .unwrap_or(true)
    }

    fn data_has_message(&self) -> Result<bool, ClientError> {
        let d = self.lock_data()?;
        Ok(d.socket().has_message()? || d.monitor_socket().has_message()?)
    }

    /// The data DEALER's descriptor, or its monitor's (`monitor`), registered once.
    #[cfg(unix)]
    fn data_notify_fd(
        &self,
        monitor: bool,
    ) -> Result<&tokio::io::unix::AsyncFd<NotifyFd>, ClientError> {
        let cell = if monitor {
            &self.inner.mon_fd
        } else {
            &self.inner.data_fd
        };
        if let Some(fd) = cell.get() {
            return Ok(fd);
        }
        let raw = {
            let d = self.lock_data()?;
            if monitor {
                d.monitor_socket().notify_fd()?
            } else {
                d.socket().notify_fd()?
            }
        };
        let fd =
            tokio::io::unix::AsyncFd::with_interest(NotifyFd(raw), tokio::io::Interest::READABLE)
                .map_err(|e| ClientError::Invalid(format!("data socket descriptor: {e}")))?;
        // A concurrent first call may have registered it already; keep that one.
        let _ = cell.set(fd);
        cell.get().ok_or(ClientError::Closed)
    }

    /// [`Client::latest`] plus the grid of every frame (fetched once per id).
    pub async fn latest_with_grids(
        &self,
    ) -> Result<(crate::data::Latest, BTreeMap<GridId, Arc<GridDef>>), ClientError> {
        let latest = self.latest()?;
        let mut grids = BTreeMap::new();
        for id in latest.grid_ids() {
            grids.insert(id, self.grid(id).await?);
        }
        Ok((latest, grids))
    }

    fn lock_data(&self) -> Result<std::sync::MutexGuard<'_, DataState>, ClientError> {
        self.inner.data.lock().map_err(|_| ClientError::Closed)
    }

    /// The grid `id`, from the cache or `grid.get`. Grids are immutable and their id is a
    /// hash of their definition, so a cached grid never goes stale.
    pub async fn grid(&self, id: GridId) -> Result<Arc<GridDef>, ClientError> {
        if let Some(g) = self
            .inner
            .grids
            .lock()
            .ok()
            .and_then(|g| g.get(&id).cloned())
        {
            return Ok(g);
        }
        let op = "grid.get";
        let r = self.call(Command::GridGet { grid_id: id }).await?;
        let def = expect_body!(op, r, ReplyBody::Grid(g) => g)?;
        if def.id() != id {
            return Err(ClientError::GridMismatch {
                asked: id.0,
                got: def.id().0,
            });
        }
        let def = Arc::new(def);
        if let Ok(mut g) = self.inner.grids.lock() {
            g.insert(id, def.clone());
        }
        Ok(def)
    }

    /// Whether `id` is cached.
    pub fn grid_cached(&self, id: GridId) -> bool {
        self.inner.grids.lock().is_ok_and(|g| g.contains_key(&id))
    }
}

/// A viewer's command that the daemon would refuse is refused here, unsent: the daemon
/// decides, and this only spares the round trip and gives the same `view_only` answer.
fn view_only_gate(role: ClientRole, cmd: &Command) -> Result<(), ClientError> {
    if role == ClientRole::Viewer && !cmd.allowed(&role) {
        return Err(ClientError::Daemon(ProtoError {
            code: ErrorCode::Refused,
            msg: format!(
                "{}: this client is authorized as a viewer on this rig; it may only look \
                 (not sent)",
                cmd.name()
            ),
            detail: Some(ErrorDetail::ViewOnly),
        }));
    }
    Ok(())
}

/// The mirror view with the identity of the `welcome` that belongs to its incarnation.
fn view_of(mirror: &Mirror, w: &Welcome) -> MirrorView {
    let mut v = mirror.view();
    if v.incarnation.is_none_or(|i| i == w.daemon_incarnation) {
        v.client_id = Some(w.client_id.clone());
        v.role = Some(w.role);
    }
    v
}

async fn hello(core: &CallCore, name: &str) -> Result<Welcome, ClientError> {
    let op = "hello";
    let r = core
        .call(
            Command::Hello {
                client: name.to_owned(),
            },
            core.retry,
        )
        .await?;
    expect_body!(op, r, ReplyBody::Welcome(w) => w)
}

async fn run_sync(
    core: Arc<CallCore>,
    mut rx: mpsc::UnboundedReceiver<SyncIn>,
    view_tx: watch::Sender<Arc<MirrorView>>,
    mut mirror: Mirror,
    name: String,
    welcome: Arc<Mutex<Welcome>>,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        // Ticks only while the mirror has something to retry or check; otherwise the task
        // sleeps until the next keepalive or event.
        let ticking = mirror.needs_tick();
        let now = Instant::now();
        let (mut need, received) = tokio::select! {
            msg = rx.recv() => match msg {
                None => return,
                Some(SyncIn::Event(e)) => (mirror.on_event(*e, now), true),
                Some(SyncIn::Ka { stamp, meta, at, local_wall_ns }) => {
                    (mirror.on_ka(&stamp, *meta, at, local_wall_ns), true)
                }
                Some(SyncIn::Resync) => (mirror.on_resync_required(now), true),
                Some(SyncIn::Rejoined) => {
                    // Only the welcome: the state itself is unchanged by a role.
                    if let Ok(w) = hello(&core, &name).await
                        && let Ok(mut slot) = welcome.lock()
                    {
                        *slot = w;
                    }
                    (None, true)
                }
            },
            _ = tick.tick(), if ticking => (mirror.on_tick(Instant::now()), false),
        };
        if !received && need.is_none() {
            continue;
        }
        while let Some(n) = need.take() {
            need = match n {
                Need::Rehello => match hello(&core, &name).await {
                    Ok(w) => {
                        if let Ok(mut slot) = welcome.lock() {
                            *slot = w;
                        }
                        Some(Need::Snapshot)
                    }
                    Err(_) => {
                        mirror.on_request_failed(Instant::now());
                        None
                    }
                },
                Need::Snapshot => match core.call(Command::StateSnapshot, core.retry).await {
                    Ok(ReplyBody::Snapshot(s)) => mirror.on_snapshot(*s, Instant::now()),
                    _ => {
                        mirror.on_request_failed(Instant::now());
                        None
                    }
                },
                Need::Since(rev) => {
                    match core.call(Command::StateSince { rev }, core.retry).await {
                        Ok(ReplyBody::Events(evs)) => mirror.on_since(evs, Instant::now()),
                        Err(e) if e.code() == Some(ErrorCode::ResyncRequired) => {
                            mirror.on_resync_required(Instant::now())
                        }
                        _ => {
                            mirror.on_request_failed(Instant::now());
                            None
                        }
                    }
                }
            };
        }
        // Every message changes something visible (liveness, clock, rev); publishing on
        // each keeps watchers simple. Keepalives come at 4 Hz, events at commit rate.
        // Liveness itself is derived by watchers from `last_ka`, so idle ticks publish
        // nothing.
        let v = match welcome.lock() {
            Ok(w) => view_of(&mirror, &w),
            Err(_) => mirror.view(),
        };
        view_tx.send_replace(Arc::new(v));
    }
}

/// The request half against a ROUTER the test answers by hand: a reply the daemon sent but
/// the network lost, and a daemon that never answers. A working daemon does neither on
/// demand; that it executes a resent id once is the daemon's own test (`ac2d` `dedup`).
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod retry_tests;
