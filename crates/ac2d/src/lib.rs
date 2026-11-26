//! ac2 daemon: owns the audio I/O, measurement jobs and state; serves clients over
//! ZeroMQ (PLAN.md §4.3, §6; `docs/protocol.md`; `docs/design/q2-q5-q6-protocol.md`).
//!
//! # Threads
//!
//! ```text
//!                 ┌──────────── ac2d-io ─────────────┐
//!  clients ◄────► │ ROUTER (ctrl)   ROUTER (data)     │ ◄── inproc PULL ◄── PUSH (control,
//!                 │ latest slot per topic, per-peer   │                     each job)
//!                 │ credit and delta chains           │
//!                 └───────┬───────────────────────────┘
//!                         │ requests (mpsc)
//!                 ┌───────▼────── ac2d-control ──────┐
//!                 │ decode, dedup, serial commits,    │── events/replies/ka ──► PUSH
//!                 │ lease, stream, job lifecycle     │
//!                 └───────┬───────────────────────────┘
//!                         │ open / close
//!  audio callback ──► capture ring ──► ac2d-fanout ──► channel per job ──► ac2d-meas-N ──► PUSH
//!  (no allocation)                     one Arc<Batch>                      ac2d-timing
//!                                      per hand-off,
//!                                      stream meters ──────────────────────────────────► PUSH
//! ```
//!
//! Only the I/O thread touches client-facing sockets, and it never blocks on anything but
//! `zmq_poll`. The control thread is a plain thread with one input channel: commands are
//! short (DSP runs on job threads, session file and archive work on `ac2d-file` workers
//! that report back on the channel, the autosave and the working state's small files on
//! the `ac2d-autosave` writer; the control thread itself reads and writes no files once it
//! serves requests, apart from starting a raw recording), and one consumer yields serial commits and a total
//! event order without locks. Generator control reaches the audio callback only through
//! atomics and a wait-free source queue; the lease deadline is enforced inside the output
//! path itself ([`stimulus`]).
#![deny(unsafe_code)]

mod authlog;
mod autosave;
mod backend;
mod batch_pool;
mod burst;
mod cadence;
mod calstore;
mod capture_clock;
mod chain;
pub mod config;
mod control;
mod conv;
mod damage;
mod dedup;
mod detect;
mod dmx;
mod fanout;
mod firewall;
mod health;
mod hop_wake;
mod hostclock;
pub mod instance;
mod io;
mod jobs;
pub mod keys;
mod leq_history;
mod leq_log;
pub mod network;
mod network_store;
mod osc;
mod outbox;
mod own_sessions;
mod peers;
mod preview;
mod refusals;
mod remote;
mod rig;
mod rig_id;
mod route;
mod runtime_dir;
mod sim;
mod state;
mod stimulus;
mod stream;
mod sweep;
mod sysclock;
mod util;

use std::fmt;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

use ac2_proto::units::DaemonIncarnation;
use ac2_zmq::{Context, PublicKey, Socket, SocketType, TcpLiveness};

pub use config::{
    AutosaveConfig, DEFAULT_PORT, DaemonConfig, DedupLimits, Listen, ListenError, LocalClock, Mdns,
    NetworkSecurity, ReplayLimits, RigIdSource, SystemClockSource, pid_file, runtime_dir,
};
pub use network::{
    Backoff, Interfaces, NetBind, NetInterface, NetListen, NetPort, NetReason, NetStatus,
    NetworkChoice, check_port,
};

pub use backend::{
    BackendChoice, FAKE_RIG, FAKE_RIG_DISTORTION, FAKE_RIG_HALL, backend, backends, fake_rig,
    fake_rig_endpoints,
};
use control::{Control, ControlMsg, Setup};
use io::{Interest, IoSockets};
use outbox::Outbox;

pub use control::{FreeSpace, TransferLimits};
pub use dmx::DmxPorts;

/// The tracing target of the audit trail: who raised the ceiling, authorized, demoted or
/// revoked a client, made sound or stopped it. `ac2d` writes its events to a file of their
/// own (`ac2_paths::log::AuditLog`).
pub const AUDIT_TARGET: &str = "ac2d::audit";

/// ZAP domain of the daemon's CURVE sockets.
pub(crate) const ZAP_DOMAIN: &str = "ac2";
/// Kernel send buffer of the network data socket (Q2): a small buffer keeps the
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
    /// The directory of the local sockets is not this user's alone, or a socket path is too
    /// long for the socket address.
    RuntimeDir {
        /// The directory.
        dir: std::path::PathBuf,
        /// What is wrong with it.
        problem: String,
    },
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
            Self::RuntimeDir { dir, problem } => {
                write!(f, "socket directory {}: {problem}", dir.display())
            }
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
    net: control::network::NetView,
    tx: Sender<ControlMsg>,
    control: Option<JoinHandle<Ended>>,
    io: Option<JoinHandle<()>>,
    /// Held for the daemon's lifetime: frame hand-offs, publish gaps and job timers are all
    /// timed waits, which Windows otherwise ends only on its 15.6 ms tick.
    _timer: ac2_audio::timer::FineResolution,
    /// The session directory made for a daemon given none: removed once the threads that
    /// write it have ended (fields drop after [`Handle`]'s own drop joins them).
    _own_sessions: Option<own_sessions::OwnSessions>,
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

/// How a daemon's run ended, as [`Handle::wait`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// An orderly stop: [`Stopper::stop`], [`Handle::shutdown`] or the handle dropped.
    Stopped,
    /// The daemon could not go on (its I/O thread failed, its control thread panicked):
    /// why, as logged. A supervisor must be able to tell this from a stop it asked for.
    Fatal(String),
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

    /// The local ctrl (ROUTER) endpoint as bound (a `tcp://…:0` request shows its real
    /// port).
    pub fn ctrl_endpoint(&self) -> &str {
        &self.ctrl
    }

    /// The local data (ROUTER) endpoint as bound.
    pub fn data_endpoint(&self) -> &str {
        &self.data
    }

    /// The daemon's ZeroMQ context; `inproc://` clients must create their sockets from it.
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// The server's CURVE public key, once one exists (the network was on at least once).
    pub fn server_public_key(&self) -> Option<PublicKey> {
        self.net.server_key()
    }

    /// What the network listener is doing now.
    pub fn network_status(&self) -> NetStatus {
        self.net.status()
    }

    /// The mDNS full name this daemon advertises under while the network listens (with
    /// [`DaemonConfig::mdns`] advertising), once registered.
    pub fn advertised_as(&self) -> Option<String> {
        self.net.advertised_as()
    }

    /// Makes the ZAP handler of the network pair now attached fail, as a crash of it would:
    /// the daemon must close that pair, keep serving locally and rebind on its own.
    ///
    /// # Errors
    /// No pair is attached, or the handler did not stop.
    #[cfg(feature = "fault-injection")]
    pub fn fail_zap_handler(&self) -> Result<(), String> {
        let fault = self
            .net
            .fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or("no network pair attached")?;
        fault.fail().map_err(|e| e.to_string())
    }

    /// A handle that can request shutdown from elsewhere.
    pub fn stopper(&self) -> Stopper {
        Stopper(self.tx.clone())
    }

    /// Blocks until the daemon has stopped (after [`Stopper::stop`] or a fatal error) and
    /// says which it was.
    #[must_use]
    pub fn wait(mut self) -> Ended {
        self.join()
    }

    /// Shuts the daemon down and waits for it.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(ControlMsg::Shutdown);
        self.join();
    }

    fn join(&mut self) -> Ended {
        let ended = match self.control.take() {
            Some(t) => t
                .join()
                .unwrap_or_else(|_| Ended::Fatal("the control thread panicked".into())),
            None => Ended::Stopped,
        };
        if let Some(t) = self.io.take() {
            let _ = t.join();
            // libzmq leaves a bound ipc socket's file behind when the socket closes; the
            // io thread held both, so they are closed now. A file left by a daemon that
            // died is bound over by the next one instead.
            for e in [&self.ctrl, &self.data] {
                if let Some(file) = ac2_paths::socket_dir::socket_file(e) {
                    let _ = std::fs::remove_file(file);
                }
            }
        }
        ended
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

fn prepare_ipc(endpoint: &str) -> Result<(), StartError> {
    runtime_dir::prepare(endpoint)
}

/// Options of a ctrl ROUTER, local or network, set before it binds.
pub(crate) fn ctrl_socket_options(router: &Socket) -> Result<(), StartError> {
    router
        .set_router_mandatory(true)
        .map_err(zerr("ROUTER_MANDATORY"))?;
    // libzmq would otherwise buffer a part of any size before the ctrl bound is checked.
    router
        .set_max_message_size(Some(
            ac2_proto::MAX_CTRL_BYTES + ac2_proto::TRANSPORT_SLACK_BYTES,
        ))
        .map_err(zerr("ROUTER MAXMSGSIZE"))?;
    router
        .set_tcp_liveness(Some(PEER_LIVENESS))
        .map_err(zerr("ROUTER liveness"))
}

pub(crate) fn data_socket_options(data: &Socket, network: bool) -> Result<(), StartError> {
    // A send to a vanished connection then fails `EHOSTUNREACH`, which is how a peer is
    // known to be gone; a full queue fails `EAGAIN` instead of dropping silently.
    data.set_router_mandatory(true)
        .map_err(zerr("data ROUTER_MANDATORY"))?;
    // A client sends only small requests here; a larger part closes its connection
    // before it is buffered.
    data.set_max_message_size(Some(
        ac2_proto::MAX_DATA_REQUEST_BYTES + ac2_proto::TRANSPORT_SLACK_BYTES,
    ))
    .map_err(zerr("data MAXMSGSIZE"))?;
    // Set once before the socket binds. It is never changed on the live socket: libzmq
    // applies a new SNDHWM to existing pipes by sending commands to their peers, and such a
    // command racing with a disconnecting peer's pipe teardown is a use-after-free in
    // libzmq 4.3.5's I/O thread.
    data.set_send_hwm(peers::DATA_SNDHWM)
        .map_err(zerr("SNDHWM"))?;
    data.set_tcp_liveness(Some(PEER_LIVENESS))
        .map_err(zerr("data liveness"))?;
    if network {
        data.set_send_buffer(Some(NETWORK_SNDBUF))
            .map_err(zerr("SNDBUF"))?;
    }
    Ok(())
}

impl Daemon {
    /// Binds the sockets and starts the I/O and control threads. No audio I/O is open
    /// until a client sends `stream.open`.
    pub fn start(config: DaemonConfig) -> Result<Handle, StartError> {
        config
            .listen
            .validate(&config.network)
            .map_err(StartError::Listen)?;
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
        let (session_dir, own_sessions) = match &config.session_dir {
            Some(d) => (d.clone(), None),
            None => {
                let o = own_sessions::OwnSessions::make().map_err(StartError::Io)?;
                (o.path().to_owned(), Some(o))
            }
        };
        let incarnation = DaemonIncarnation(util::random_u64());
        let (rig_id, rig_id_damage) = rig_id::load(&config.rig_id);

        // The local pair lives as long as the daemon, on a plain context that also owns the
        // internal pipe and every outgoing link; the network pair comes and goes on its own.
        let ctx = Context::new().map_err(zerr("context"))?;
        let router = ctx.socket(SocketType::Router).map_err(zerr("ROUTER"))?;
        let data_sock = ctx
            .socket(SocketType::Router)
            .map_err(zerr("data ROUTER"))?;
        ctrl_socket_options(&router)?;
        data_socket_options(&data_sock, false)?;
        let (ctrl_ep, data_ep) = match &config.listen {
            Listen::Inproc { name } => (
                format!("inproc://{name}/ctrl"),
                format!("inproc://{name}/data"),
            ),
            Listen::Local { ctrl, data } => {
                prepare_ipc(ctrl)?;
                prepare_ipc(data)?;
                (ctrl.clone(), data.clone())
            }
        };
        router
            .bind(&ctrl_ep)
            .map_err(zerr(format!("bind {ctrl_ep}")))?;
        data_sock
            .bind(&data_ep)
            .map_err(zerr(format!("bind {data_ep}")))?;
        let ctrl = router.last_endpoint().unwrap_or(ctrl_ep);
        let data = data_sock.last_endpoint().unwrap_or(data_ep);

        let pull_ep = format!("inproc://ac2d-{:016x}/out", incarnation.0);
        let pull = ctx.socket(SocketType::Pull).map_err(zerr("PULL"))?;
        pull.set_recv_hwm(100_000).map_err(zerr("RCVHWM"))?;
        pull.bind(&pull_ep).map_err(zerr("bind internal pipe"))?;
        let outbox = Outbox::connect(&ctx, &pull_ep, 100_000).map_err(zerr("internal pipe"))?;

        let (tx, rx) = std::sync::mpsc::channel();
        let attach = io::AttachQueue::default();
        let (mut network, damage) = control::network::NetCtl::new(control::network::NetSetup {
            choice: config.network.clone(),
            files: config.keys.clone(),
            interfaces: config.interfaces.clone(),
            retry: config.net_retry,
            mdns: config.mdns.clone(),
            name: config.name.clone(),
            attach: attach.clone(),
            to_control: tx.clone(),
        })?;
        let mut key_damage: Vec<_> = rig_id_damage.into_iter().collect();
        key_damage.extend(damage);
        // The first bind before anyone is told the daemon is up, so a network that can be
        // served is served from the start; one that cannot is retried by control.
        network.attempt(&outbox, std::time::Instant::now());
        let net = network.view();
        let server = control::rig::Server {
            local: match &config.listen {
                Listen::Inproc { .. } => control::rig::LocalSetup::Embedded,
                Listen::Local { .. } => control::rig::LocalSetup::Local {
                    ctrl: ctrl.clone(),
                    data: data.clone(),
                },
            },
            network,
        };
        let timer = ac2_audio::timer::fine_resolution();
        let fps = config.publish_fps.max(1);
        let interest = Arc::new(Interest::default());
        let (frames, frames_rx) = outbox::frame_channel();
        let io = io::spawn(
            IoSockets {
                router,
                data: data_sock,
                pull,
                frames: frames_rx,
                attach,
            },
            tx.clone(),
            Arc::clone(&interest),
            fps,
        )
        .map_err(StartError::Io)?;
        let control = Control::new(Setup {
            backends: config.backends.clone(),
            incarnation,
            rig_id,
            name: config.name.clone(),
            ceiling_dbfs: config.max_level_dbfs,
            max_level: bound_level,
            ceiling_bound: config.max_level_dbfs,
            bound_level,
            rig_settings: config.rig_settings.clone(),
            rig_logs: config.rig_logs.clone(),
            server,
            lease_expiry: config.lease_expiry,
            transfers: config.transfers.clone(),
            keepalive: config.keepalive,
            replay: config.replay,
            dedup: config.dedup,
            ctx: ctx.clone(),
            endpoint: pull_ep,
            interest,
            fps,
            frames,
            outbox,
            to_self: tx.clone(),
            session_dir,
            cal_store: config.cal_store.clone(),
            autosave: config.autosave.clone(),
            recording_dir: config.recording_dir.clone(),
            plan_dir: config.plan_dir.clone(),
            local_clock: config.local_clock,
            system_clock: config.system_clock,
            systemd_notify: config.systemd_notify,
            dmx: config.dmx,
            damaged: key_damage,
        });
        let control = std::thread::Builder::new()
            .name("ac2d-control".into())
            .spawn(move || control.run(&rx))
            .map_err(StartError::Io)?;
        tracing::info!(
            "ac2d {} up: {:?} (rig id {rig_id}), ctrl {ctrl}, data {data}, backends {:?}, \
             incarnation {:016x}",
            env!("CARGO_PKG_VERSION"),
            config.name,
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
            net,
            tx,
            control: Some(control),
            io: Some(io),
            _timer: timer,
            _own_sessions: own_sessions,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn start(name: &str) -> Handle {
        let listen = Listen::Inproc {
            name: format!("{name}-{}", std::process::id()),
        };
        Daemon::start(DaemonConfig::new(
            Arc::new(fake_rig().unwrap()),
            listen,
            -10.0,
        ))
        .unwrap()
    }

    /// No fake-backend run reaches the io thread's fatal paths (an internal message part
    /// that cannot be made, a dead ZAP handler), so its message is sent here as the io
    /// thread sends it: `wait` must tell it from a stop, which `ac2d` exits 1 on.
    #[test]
    fn a_fatal_end_is_not_a_stop() {
        let h = start("fatal");
        h.tx.send(ControlMsg::Fatal("the io thread failed".into()))
            .unwrap();
        assert_eq!(h.wait(), Ended::Fatal("the io thread failed".into()));
    }

    /// A daemon given no session directory (a simulated rig, a test) saves into one of its
    /// own, never the user's, and removes it as it stops.
    #[test]
    fn a_daemon_given_no_session_dir_owns_one_until_it_stops() {
        let h = start("own-sessions");
        let dir = h._own_sessions.as_ref().unwrap().path().to_owned();
        assert!(dir.starts_with(std::env::temp_dir()), "{}", dir.display());
        assert!(dir.is_dir());
        h.shutdown();
        assert!(!dir.exists());
    }

    #[test]
    fn a_stop_is_an_orderly_end() {
        let h = start("stopped");
        h.stopper().stop();
        assert_eq!(h.wait(), Ended::Stopped);
    }
}
