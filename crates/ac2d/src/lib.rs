//! ac2 daemon: owns the audio session, measurement jobs and state; serves clients over
//! ZeroMQ (PLAN.md §4.3, §6; `docs/protocol.md`; `docs/design/q2-q5-q6-protocol.md`).
//!
//! # Threads
//!
//! ```text
//!                 ┌──────────── ac2d-io ─────────────┐
//!  clients ◄────► │ ROUTER (ctrl)   XPUB (data)       │ ◄── inproc PULL ◄── PUSH (control,
//!                 │ latest slot per topic, rate limit │                     each job)
//!                 └───────┬───────────────────────────┘
//!                         │ requests (mpsc)
//!                 ┌───────▼────── ac2d-control ──────┐
//!                 │ decode, dedup, serial commits,    │── events/replies/ka ──► PUSH
//!                 │ lease, session, job lifecycle     │
//!                 └───────┬───────────────────────────┘
//!                         │ open / close
//!  audio callback ──► capture ring ──► ac2d-fanout ──► channel per job ──► ac2d-meas-N ──► PUSH
//!  (no allocation)                     one Arc<Batch>                      ac2d-timing
//!                                      per hand-off,
//!                                      session meters ──────────────────────────────────► PUSH
//! ```
//!
//! Only the I/O thread touches client-facing sockets, and it never blocks on anything but
//! `zmq_poll`. The control thread is a plain thread with one input channel: commands are
//! short (DSP runs on job threads), and one consumer yields serial commits and a total
//! event order without locks. Generator control reaches the audio callback only through
//! atomics and a wait-free source queue; the lease deadline is enforced inside the output
//! path itself ([`stimulus`]).
#![deny(unsafe_code)]

mod authlog;
mod autosave;
mod backend;
mod burst;
mod cadence;
mod calstore;
pub mod config;
mod control;
mod conv;
mod dedup;
mod detect;
mod fanout;
mod firewall;
mod io;
mod jobs;
pub mod keys;
mod leq_history;
mod leq_log;
mod outbox;
mod preview;
mod rig;
mod session;
mod state;
mod stimulus;
mod sweep;
mod util;

use std::fmt;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

use ac2_proto::units::DaemonIncarnation;
use ac2_zmq::{Context, PublicKey, SecureContext, Socket, SocketType, TcpLiveness};

pub use config::{
    Advertise, AutosaveConfig, DEFAULT_PORT, DaemonConfig, DedupLimits, Listen, ListenError,
    LocalClock, NetworkSecurity, ReplayLimits, pid_file, runtime_dir,
};

pub use backend::{
    BackendChoice, FAKE_RIG, FAKE_RIG_DISTORTION, FAKE_RIG_HALL, backend, backends, fake_rig,
    fake_rig_endpoints,
};
use control::{Control, ControlMsg, Setup};
use io::{Interest, IoSockets};
use outbox::Outbox;

/// ZAP domain of the daemon's CURVE sockets.
const ZAP_DOMAIN: &str = "ac2";
/// Kernel send buffer of the data socket in network mode (Q2): a small buffer keeps the
/// backlog in front of a slow link short, so the latest-slot publisher stays fresh.
const NETWORK_SNDBUF: u32 = 64 * 1024;
/// Dead-peer detection on both client-facing sockets (TCP only). A client that vanishes
/// without closing its connection (lid closed, out of Wi-Fi range) would otherwise keep its
/// queue full of frames, its subscriptions (and so the optional work done for them) and its
/// pipe for as long as the kernel retransmits, about a quarter of an hour. With these an
/// idle connection is probed after 5 s and closed after three unanswered probes, and one
/// whose frames go unacknowledged for 15 s is closed; the limits leave room for a Wi-Fi
/// link that stalls for a few seconds.
const PEER_LIVENESS: TcpLiveness = TcpLiveness {
    idle: Duration::from_secs(5),
    interval: Duration::from_secs(1),
    count: 3,
    max_unacked: Duration::from_secs(15),
};

/// Why the daemon did not start.
#[derive(Debug)]
pub enum StartError {
    /// Transport configuration not servable.
    Listen(ListenError),
    /// Global maximum level invalid.
    Level(String),
    /// Key files.
    Keys(keys::KeyFileError),
    /// Socket setup or bind failed.
    Zmq {
        /// What was being done.
        what: String,
        /// Cause.
        source: ac2_zmq::Error,
    },
    /// A thread could not be spawned, or a directory not created.
    Io(std::io::Error),
    /// No audio backend was given.
    NoBackend,
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Listen(e) => write!(f, "{e}"),
            Self::Level(e) => write!(f, "max level: {e}"),
            Self::Keys(e) => write!(f, "{e}"),
            Self::Zmq { what, source } => write!(f, "{what}: {source}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::NoBackend => f.write_str("no audio backend"),
        }
    }
}

impl std::error::Error for StartError {}

fn zerr(what: impl Into<String>) -> impl FnOnce(ac2_zmq::Error) -> StartError {
    let what = what.into();
    move |source| StartError::Zmq { what, source }
}

/// The daemon.
#[derive(Debug)]
pub struct Daemon;

/// A running daemon. Dropping it shuts the daemon down (output faded, stream closed,
/// sockets closed with linger 0).
pub struct Handle {
    incarnation: DaemonIncarnation,
    ctrl: String,
    data: String,
    ctx: Context,
    server_key: Option<PublicKey>,
    tx: Sender<ControlMsg>,
    control: Option<JoinHandle<()>>,
    io: Option<JoinHandle<()>>,
    advert: Option<ac2_discovery::Advertiser>,
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handle")
            .field("incarnation", &self.incarnation)
            .field("ctrl", &self.ctrl)
            .field("data", &self.data)
            .finish_non_exhaustive()
    }
}

/// Asks a running daemon to shut down; cheap to clone, usable from any thread (signal
/// handlers).
#[derive(Clone, Debug)]
pub struct Stopper(Sender<ControlMsg>);

impl fmt::Debug for ControlMsg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ControlMsg")
    }
}

impl Stopper {
    /// Requests an orderly shutdown.
    pub fn stop(&self) {
        let _ = self.0.send(ControlMsg::Shutdown);
    }
}

impl Handle {
    /// Daemon incarnation (random per start).
    pub fn incarnation(&self) -> u64 {
        self.incarnation.0
    }

    /// The ctrl (ROUTER) endpoint as bound (a `tcp://…:0` request shows its real port).
    pub fn ctrl_endpoint(&self) -> &str {
        &self.ctrl
    }

    /// The data (XPUB) endpoint as bound.
    pub fn data_endpoint(&self) -> &str {
        &self.data
    }

    /// The daemon's ZeroMQ context; `inproc://` clients must create their sockets from it.
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// The server's CURVE public key (network mode).
    pub fn server_public_key(&self) -> Option<PublicKey> {
        self.server_key
    }

    /// The mDNS full name this daemon advertises under (network mode with
    /// [`DaemonConfig::advertise`]), once registered.
    pub fn advertised_as(&self) -> Option<&str> {
        self.advert
            .as_ref()
            .map(ac2_discovery::Advertiser::fullname)
    }

    /// A handle that can request shutdown from elsewhere.
    pub fn stopper(&self) -> Stopper {
        Stopper(self.tx.clone())
    }

    /// Blocks until the daemon has stopped (after [`Stopper::stop`] or a fatal error).
    pub fn wait(mut self) {
        self.join();
    }

    /// Shuts the daemon down and waits for it.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(ControlMsg::Shutdown);
        self.join();
    }

    fn join(&mut self) {
        // The advert lives as long as the daemon serves: only once control has stopped
        // does the goodbye go out, still before the sockets close, so browsers drop the rig
        // at once.
        if let Some(t) = self.control.take() {
            let _ = t.join();
        }
        self.advert = None;
        if let Some(t) = self.io.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if self.control.is_some() {
            let _ = self.tx.send(ControlMsg::Shutdown);
            self.join();
        }
    }
}

#[cfg(unix)]
fn prepare_ipc(endpoint: &str) -> Result<(), StartError> {
    if let Some(path) = endpoint.strip_prefix("ipc://")
        && let Some(dir) = std::path::Path::new(path).parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).map_err(StartError::Io)?;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_ipc(_endpoint: &str) -> Result<(), StartError> {
    Ok(())
}

fn data_socket_options(xpub: &Socket, network: bool) -> Result<(), StartError> {
    xpub.set_xpub_verbose(true).map_err(zerr("XPUB_VERBOSE"))?;
    xpub.set_send_hwm(io::DATA_SNDHWM).map_err(zerr("SNDHWM"))?;
    xpub.set_tcp_liveness(Some(PEER_LIVENESS))
        .map_err(zerr("XPUB liveness"))?;
    if network {
        xpub.set_send_buffer(Some(NETWORK_SNDBUF))
            .map_err(zerr("SNDBUF"))?;
    }
    Ok(())
}

impl Daemon {
    /// Binds the sockets and starts the I/O and control threads. No audio session is open
    /// until a client sends `session.open`.
    pub fn start(config: DaemonConfig) -> Result<Handle, StartError> {
        config.listen.validate().map_err(StartError::Listen)?;
        if config.backends.is_empty() {
            return Err(StartError::NoBackend);
        }
        let bound_level = stimulus::peak_limit(config.max_level_dbfs)
            .map_err(|e| StartError::Level(e.to_string()))?;
        if config.max_level_dbfs > 0.0 {
            return Err(StartError::Level(
                "the global maximum must be at or below 0 dBFS".into(),
            ));
        }
        let incarnation = DaemonIncarnation(util::random_u64());
        let network = config.listen.is_network();

        // Network mode: what `server.*` reports and changes while the daemon runs.
        let mut network_parts = None;
        let (ctx, secure, router, xpub, server_key) = match &config.listen {
            Listen::Inproc { name } => {
                let ctx = Context::new().map_err(zerr("context"))?;
                let router = ctx.socket(SocketType::Router).map_err(zerr("ROUTER"))?;
                let xpub = ctx.socket(SocketType::XPub).map_err(zerr("XPUB"))?;
                let _ = name;
                (ctx, None, router, xpub, None)
            }
            Listen::Local { ctrl, data } => {
                prepare_ipc(ctrl)?;
                prepare_ipc(data)?;
                let ctx = Context::new().map_err(zerr("context"))?;
                let router = ctx.socket(SocketType::Router).map_err(zerr("ROUTER"))?;
                let xpub = ctx.socket(SocketType::XPub).map_err(zerr("XPUB"))?;
                (ctx, None, router, xpub, None)
            }
            Listen::Network { security, .. } => {
                let kp = keys::load_or_create_server_keys(&security.server_key_file)
                    .map_err(StartError::Keys)?;
                let authorized = keys::load_or_create_authorized(&security.authorized_clients_file)
                    .map_err(StartError::Keys)?;
                tracing::info!(
                    "network mode: {} authorized client(s); server key {} (fingerprint {})",
                    authorized.len(),
                    kp.public.to_z85(),
                    kp.public.fingerprint()
                );
                let log = authlog::AuthLog::new(&security.authorized_clients_file);
                let refused = log.refused();
                let sc = SecureContext::new(ZAP_DOMAIN, authorized, move |d| log.record(d))
                    .map_err(zerr("ZAP handler"))?;
                network_parts = Some((
                    sc.authorized_handle(),
                    security.authorized_clients_file.clone(),
                    refused,
                ));
                let router = sc
                    .curve_server_socket(SocketType::Router, &kp)
                    .map_err(zerr("CURVE ROUTER"))?;
                let xpub = sc
                    .curve_server_socket(SocketType::XPub, &kp)
                    .map_err(zerr("CURVE XPUB"))?;
                (
                    sc.context().clone(),
                    Some(sc),
                    router,
                    xpub,
                    Some(kp.public),
                )
            }
        };
        router
            .set_router_mandatory(true)
            .map_err(zerr("ROUTER_MANDATORY"))?;
        router
            .set_tcp_liveness(Some(PEER_LIVENESS))
            .map_err(zerr("ROUTER liveness"))?;
        data_socket_options(&xpub, network)?;
        let (ctrl_ep, data_ep) = match &config.listen {
            Listen::Inproc { name } => (
                format!("inproc://{name}/ctrl"),
                format!("inproc://{name}/data"),
            ),
            Listen::Local { ctrl, data } | Listen::Network { ctrl, data, .. } => {
                (ctrl.clone(), data.clone())
            }
        };
        router
            .bind(&ctrl_ep)
            .map_err(zerr(format!("bind {ctrl_ep}")))?;
        xpub.bind(&data_ep)
            .map_err(zerr(format!("bind {data_ep}")))?;
        let ctrl = router.last_endpoint().unwrap_or(ctrl_ep);
        let data = xpub.last_endpoint().unwrap_or(data_ep);
        if network {
            firewall::report(&ctrl, &data);
        }

        let pull_ep = format!("inproc://ac2d-{:016x}/out", incarnation.0);
        let pull = ctx.socket(SocketType::Pull).map_err(zerr("PULL"))?;
        pull.set_recv_hwm(100_000).map_err(zerr("RCVHWM"))?;
        pull.bind(&pull_ep).map_err(zerr("bind internal pipe"))?;
        let outbox = Outbox::connect(&ctx, &pull_ep, 100_000).map_err(zerr("internal pipe"))?;

        let advertised_as = Arc::new(std::sync::Mutex::new(None));
        let server = match (&config.listen, network_parts, server_key) {
            (Listen::Network { .. }, Some((authorized, authorized_file, refused)), Some(key)) => {
                control::rig::ServerSetup::Network(Box::new(control::rig::NetworkSetup {
                    ctrl: ctrl.clone(),
                    data: data.clone(),
                    server_key: key,
                    authorized,
                    authorized_file,
                    refused,
                    advertised_as: Arc::clone(&advertised_as),
                }))
            }
            (Listen::Inproc { .. }, ..) => control::rig::ServerSetup::Embedded,
            _ => control::rig::ServerSetup::Local { ctrl: ctrl.clone() },
        };
        let fps = config
            .publish_fps
            .unwrap_or(if network { 30 } else { 60 })
            .max(1);
        let interest = Arc::new(Interest::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let io = io::spawn(
            IoSockets {
                router,
                xpub,
                pull,
                secure,
            },
            tx.clone(),
            Arc::clone(&interest),
            fps,
        )
        .map_err(StartError::Io)?;
        let control = Control::new(Setup {
            backends: config.backends.clone(),
            incarnation,
            ceiling_dbfs: config.max_level_dbfs,
            max_level: bound_level,
            ceiling_bound: config.max_level_dbfs,
            bound_level,
            rig_settings: config.rig_settings.clone(),
            server,
            lease_expiry: config.lease_expiry,
            keepalive: config.keepalive,
            replay: config.replay,
            dedup: config.dedup,
            ctx: ctx.clone(),
            endpoint: pull_ep,
            interest,
            fps,
            outbox,
            to_self: tx.clone(),
            session_dir: config.session_dir.clone(),
            network,
            cal_store: config.cal_store.clone(),
            autosave: config.autosave.clone(),
            recording_dir: config.recording_dir.clone(),
            local_clock: config.local_clock,
        });
        let control = std::thread::Builder::new()
            .name("ac2d-control".into())
            .spawn(move || control.run(&rx))
            .map_err(StartError::Io)?;
        let advert = match (&config.advertise, server_key) {
            (Some(a), Some(key)) => advertise(a, &key, &ctrl),
            _ => None,
        };
        if let Some(a) = &advert {
            *advertised_as
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(a.fullname().to_owned());
        }
        tracing::info!(
            "ac2d {} up: ctrl {ctrl}, data {data}, backends {:?}, incarnation {:016x}",
            env!("CARGO_PKG_VERSION"),
            config.backends.iter().map(|b| b.kind()).collect::<Vec<_>>(),
            incarnation.0
        );
        // Say at once why a backend cannot be used, and what to do about it, instead of
        // leaving the operator to find out from an empty device list.
        for b in &config.backends {
            if let Err(ac2_audio::AudioError::Unavailable { backend, reason }) = b.enumerate() {
                tracing::warn!("{backend:?} unavailable: {reason}");
            }
        }
        Ok(Handle {
            incarnation,
            ctrl,
            data,
            ctx,
            server_key,
            tx,
            control: Some(control),
            io: Some(io),
            advert,
        })
    }
}

/// Registers the mDNS advert of a network-mode daemon bound at `ctrl`. A failure is logged
/// and otherwise ignored: discovery is a convenience, the daemon serves paired clients
/// without it.
fn advertise(a: &Advertise, key: &PublicKey, ctrl: &str) -> Option<ac2_discovery::Advertiser> {
    let (host, port) = ctrl
        .strip_prefix("tcp://")
        .and_then(|r| r.rsplit_once(':'))
        .and_then(|(h, p)| Some((h, p.parse::<u16>().ok()?)))?;
    let advert = ac2_discovery::Advert {
        name: ac2_discovery::instance_name(&a.name),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        proto: ac2_proto::PROTO_VERSION,
        fingerprint: key.fingerprint(),
    };
    let report = |e: ac2_discovery::AdvertEvent| match e {
        ac2_discovery::AdvertEvent::Error(e) => {
            tracing::warn!("mDNS responder: {e}; the rig may not be discoverable");
        }
        ac2_discovery::AdvertEvent::AddressAdded(a) => {
            tracing::info!("mDNS: advertising on {a}");
        }
        ac2_discovery::AdvertEvent::AddressRemoved(a) => {
            tracing::info!("mDNS: {a} went away");
        }
        ac2_discovery::AdvertEvent::Renamed { from, to } => {
            tracing::warn!("mDNS: {from:?} is taken on this network; advertising as {to:?}");
        }
    };
    match ac2_discovery::Advertiser::start(
        &advert,
        port,
        &ac2_discovery::Bind::from_listen_host(host),
        &a.mdns,
        report,
    ) {
        Ok(adv) => {
            tracing::info!(
                "mDNS: advertising {:?} ({}) on port {port}",
                advert.name,
                advert.fingerprint
            );
            Some(adv)
        }
        Err(e) => {
            tracing::warn!(
                "{e}: the rig is not advertised, `ac2 discover` will not list it (clients can \
                 still connect with --remote <address>)"
            );
            None
        }
    }
}
