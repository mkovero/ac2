//! The client: connect, typed calls with retry, mirrored state, data subscriptions.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac2_proto::model::State;
use ac2_proto::units::{ClientId, RequestId, Rev};
use ac2_proto::{
    Command, ErrorCode, GridDef, GridId, ReplyBody, Request, StateSnapshot, Subscription, Welcome,
    encode_request,
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
}

/// First retry after a lost or refused connection.
const RECONNECT_FIRST: Duration = Duration::from_millis(100);
/// Upper bound of the reconnect back-off: an unreachable daemon (a Pi switched off, out of
/// Wi-Fi range) costs one attempt per socket every 2 s instead of ten a second, while a
/// daemon that restarts is found again within that.
const RECONNECT_MAX: Duration = Duration::from_secs(2);
/// Dead-peer detection towards the daemon (TCP only): a daemon host that vanished without
/// closing the connection (power cut, network gone) leaves it open for as long as the
/// kernel retransmits, and a SUB, which never sends, would not notice even then. With
/// these an idle connection is probed after 5 s and closed after three unanswered probes,
/// one with unacknowledged data after 15 s, and the reconnect loop takes over, so a daemon
/// that comes back is reached again.
const DAEMON_LIVENESS: TcpLiveness = TcpLiveness {
    idle: Duration::from_secs(5),
    interval: Duration::from_secs(1),
    count: 3,
    max_unacked: Duration::from_secs(15),
};
/// Receive queue of the data SUB, in messages. It is drained whole on each `latest()`, so it
/// must hold what arrives between two drains (a publish round of every subscribed topic);
/// a smaller queue would stop libzmq reading the connection mid-round and push the backlog
/// back into the daemon's queue, where it ages.
const DATA_RCVHWM: u32 = 64;

/// The data SUB's `ZMQ_FD`, owned by libzmq: registered with the runtime for readiness only,
/// never read or closed here.
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
        }
    }

    /// What follows "… is not responding" when the daemon does not answer (empty for a
    /// local daemon): with CURVE, a refused key looks exactly like silence, so this names
    /// the client's fingerprint and where to authorize it; for another host, the firewall
    /// hint follows.
    pub fn not_responding_hints(&self) -> String {
        let mut s = String::new();
        if let Some(c) = &self.curve {
            s.push_str(" — ");
            s.push_str(&crate::keys::unauthorized_hint(c));
        }
        if let Some(h) = self.endpoints.firewall_hint() {
            s.push_str("; ");
            s.push_str(&h);
        }
        s
    }
}

/// The request half: ids, encoding, retry. Shared with the sync and lease tasks.
#[derive(Debug)]
pub(crate) struct CallCore {
    io: Io,
    next_id: AtomicU64,
    retry: Retry,
}

impl CallCore {
    fn next_id(&self) -> RequestId {
        RequestId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) async fn call(
        &self,
        cmd: Command,
        expect_rev: Option<Rev>,
        retry: Retry,
    ) -> Result<ReplyBody, ClientError> {
        let op = cmd.name();
        let mut req = Request::new(self.next_id(), cmd);
        req.expect_rev = expect_rev;
        let bytes = encode_request(&req).map_err(ClientError::Encode)?;
        let (tx, mut rx) = oneshot::channel();
        let id = req.id.0;
        self.io
            .pending
            .lock()
            .map_err(|_| ClientError::Closed)?
            .insert(id, tx);
        // Removes the waiter however this future ends (reply, timeout or cancellation).
        struct Guard<'a>(&'a CallCore, u64);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                if let Ok(mut p) = self.0.io.pending.lock() {
                    p.remove(&self.1);
                }
            }
        }
        let _guard = Guard(self, id);
        let attempts = retry.retries.saturating_add(1);
        for _ in 0..attempts {
            self.io.send(&bytes)?;
            match tokio::time::timeout(retry.timeout, &mut rx).await {
                Ok(Ok(reply)) => {
                    return match reply?.result {
                        Ok(body) => Ok(body),
                        Err(e) => Err(ClientError::from_proto(e)),
                    };
                }
                Ok(Err(_)) => return Err(ClientError::Closed),
                Err(_) => continue,
            }
        }
        Err(ClientError::Timeout { op, attempts })
    }

    /// Sends without waiting for the reply (used from `Drop`).
    pub(crate) fn send_nowait(&self, cmd: Command) {
        let req = Request::new(self.next_id(), cmd);
        if let Ok(bytes) = encode_request(&req) {
            let _ = self.io.send(&bytes);
        }
    }
}

/// Name of a reply body, for errors.
pub fn body_name(b: &ReplyBody) -> &'static str {
    match b {
        ReplyBody::Ack { .. } => "ack",
        ReplyBody::Welcome(_) => "welcome",
        ReplyBody::Backends(_) => "backends",
        ReplyBody::Preview(_) => "preview",
        ReplyBody::LoopbackDetection(_) => "loopback_detection",
        ReplyBody::Session(_) => "session",
        ReplyBody::Lease(_) => "lease",
        ReplyBody::Generator(_) => "generator",
        ReplyBody::Measurement(_) => "measurement",
        ReplyBody::DelayFinding(_) => "delay_finding",
        ReplyBody::Trace(_) => "trace",
        ReplyBody::Traces(_) => "traces",
        ReplyBody::TraceData(_) => "trace_data",
        ReplyBody::Export { .. } => "export",
        ReplyBody::Calibration(_) => "calibration",
        ReplyBody::Mic(_) => "mic",
        ReplyBody::Calibrations { .. } => "calibrations",
        ReplyBody::Inputs(_) => "inputs",
        ReplyBody::Outputs(_) => "outputs",
        ReplyBody::Server(_) => "server",
        ReplyBody::SplLogPage(_) => "spl_log_page",
        ReplyBody::SplHistory(_) => "spl_history",
        ReplyBody::SplBandLog(_) => "spl_band_log",
        ReplyBody::Snapshot(_) => "snapshot",
        ReplyBody::Events(_) => "events",
        ReplyBody::Grid(_) => "grid",
        ReplyBody::SessionFile(_) => "session_file",
        ReplyBody::Sessions(_) => "sessions",
        ReplyBody::Sweep(_) => "sweep",
        ReplyBody::Recording(_) => "recording",
        ReplyBody::Recordings(_) => "recordings",
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
    /// Registration of the data SUB's notification descriptor with the runtime, made on the
    /// first [`Client::data_changed`]. Declared before `data` so it is deregistered before
    /// the socket (and its descriptor) closes.
    #[cfg(unix)]
    data_fd: std::sync::OnceLock<tokio::io::unix::AsyncFd<NotifyFd>>,
    data: Mutex<DataState>,
    grids: Mutex<HashMap<GridId, Arc<GridDef>>>,
    sync_task: JoinHandle<()>,
    name: String,
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
        let ctx = match &cfg.context {
            Some(c) => c.clone(),
            None => Context::new()?,
        };
        let curve = cfg.curve.as_ref();
        let dealer = ctx.socket(SocketType::Dealer)?;
        // A lease release queued on shutdown still leaves.
        dealer.set_linger(Some(Duration::from_millis(500)))?;
        liveness(&dealer)?;
        if let Some(c) = curve {
            dealer.set_curve_client(c)?;
        }
        dealer.connect(&cfg.endpoints.ctrl)?;

        // Q5 step 1: evt and ka are subscribed before anything else is asked.
        let sync_sub = ctx.socket(SocketType::Sub)?;
        liveness(&sync_sub)?;
        if let Some(c) = curve {
            sync_sub.set_curve_client(c)?;
        }
        for s in Subscription::sync_set() {
            sync_sub.subscribe(&s.prefix())?;
        }
        sync_sub.connect(&cfg.endpoints.data)?;

        // Data frames get their own SUB so that a burst of frames never delays an event.
        let data_sub = ctx.socket(SocketType::Sub)?;
        liveness(&data_sub)?;
        data_sub.set_recv_hwm(DATA_RCVHWM)?;
        if let Some(c) = curve {
            data_sub.set_curve_client(c)?;
        }
        data_sub.connect(&cfg.endpoints.data)?;

        let (sync_tx, sync_rx) = mpsc::unbounded_channel();
        let io = Io::start(&ctx, dealer, sync_sub, sync_tx)?;
        let core = Arc::new(CallCore {
            io,
            // Random start: under CURVE two processes share one client id (the key name), and
            // the daemon dedups by (client id, request id).
            next_id: AtomicU64::new(random_u64() >> 1),
            retry: cfg.retry,
        });
        let welcome = hello(&core, &cfg.name).await?;
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
            data: Mutex::new(DataState::new(data_sub)),
            grids: Mutex::new(HashMap::new()),
            sync_task,
            name: cfg.name,
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

    /// The name sent in `hello`.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Sends `cmd` with the default retry policy.
    pub async fn call(&self, cmd: Command) -> Result<ReplyBody, ClientError> {
        let r = self.inner.core.retry;
        self.inner.core.call(cmd, None, r).await
    }

    /// Sends a mutation guarded by `expect_rev`: refused with `conflict` if the daemon state
    /// has moved past it.
    pub async fn call_expect(
        &self,
        cmd: Command,
        expect_rev: Rev,
    ) -> Result<ReplyBody, ClientError> {
        let r = self.inner.core.retry;
        self.inner.core.call(cmd, Some(expect_rev), r).await
    }

    /// Sends `cmd` with an explicit retry policy and optional `expect_rev`.
    pub async fn call_with(
        &self,
        cmd: Command,
        expect_rev: Option<Rev>,
        retry: Retry,
    ) -> Result<ReplyBody, ClientError> {
        self.inner.core.call(cmd, expect_rev, retry).await
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

    /// Ctrl and data messages dropped as malformed by the I/O thread so far.
    pub fn malformed_ctrl(&self) -> u64 {
        self.inner.core.io.malformed.load(Ordering::Relaxed)
    }

    // ----- data ------------------------------------------------------------------------

    /// Subscribes the data socket to `sub` (reference counted per prefix).
    pub fn subscribe(&self, sub: Subscription) -> Result<(), ClientError> {
        self.lock_data()?.subscribe(&sub.prefix())
    }

    /// Undoes one [`Client::subscribe`] of `sub`; its kept frames are dropped when the last
    /// subscription of the prefix goes.
    pub fn unsubscribe(&self, sub: Subscription) -> Result<(), ClientError> {
        self.lock_data()?.unsubscribe(&sub.prefix())
    }

    /// The newest frame per topic, with age and STALE computed now. Only the newest
    /// message per topic received since the previous call is decoded. Frames of an older
    /// session epoch or another incarnation are discarded.
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
    /// `latest` then reports.
    pub async fn data_changed(&self) {
        #[cfg(unix)]
        loop {
            // The descriptor only signals changes; the queue itself is checked first and
            // after every wake.
            let fd = match self.data_has_message() {
                Ok(true) | Err(_) => return,
                Ok(false) => match self.data_notify_fd() {
                    Ok(fd) => fd,
                    Err(_) => return,
                },
            };
            match fd.readable().await {
                Ok(mut ready) => ready.clear_ready(),
                Err(_) => return,
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
            .and_then(|d| Ok(d.socket().wait_readable(timeout)?))
            .unwrap_or(true)
    }

    fn data_has_message(&self) -> Result<bool, ClientError> {
        Ok(self.lock_data()?.socket().has_message()?)
    }

    #[cfg(unix)]
    fn data_notify_fd(&self) -> Result<&tokio::io::unix::AsyncFd<NotifyFd>, ClientError> {
        if let Some(fd) = self.inner.data_fd.get() {
            return Ok(fd);
        }
        let raw = self.lock_data()?.socket().notify_fd()?;
        let fd =
            tokio::io::unix::AsyncFd::with_interest(NotifyFd(raw), tokio::io::Interest::READABLE)
                .map_err(|e| ClientError::Invalid(format!("data socket descriptor: {e}")))?;
        // A concurrent first call may have registered it already; keep that one.
        let _ = self.inner.data_fd.set(fd);
        self.inner.data_fd.get().ok_or(ClientError::Closed)
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

/// The mirror view with the identity of the `welcome` that belongs to its incarnation.
fn view_of(mirror: &Mirror, w: &Welcome) -> MirrorView {
    let mut v = mirror.view();
    if v.incarnation.is_none_or(|i| i == w.daemon_incarnation) {
        v.client_id = Some(w.client_id.clone());
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
            None,
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
                    (mirror.on_ka(&stamp, meta, at, local_wall_ns), true)
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
                Need::Snapshot => match core.call(Command::StateSnapshot, None, core.retry).await {
                    Ok(ReplyBody::Snapshot(s)) => mirror.on_snapshot(*s, Instant::now()),
                    _ => {
                        mirror.on_request_failed(Instant::now());
                        None
                    }
                },
                Need::Since(rev) => {
                    match core
                        .call(Command::StateSince { rev }, None, core.retry)
                        .await
                    {
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
