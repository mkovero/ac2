//! Daemon configuration: backend, transports, limits.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ac2_audio::Backend;

pub use crate::network::{NetBind, NetListen, NetPort, NetworkChoice};

/// Everything [`crate::Daemon::start`] needs.
#[derive(Clone)]
pub struct DaemonConfig {
    /// The audio backends offered, the default one (used when `session.open` names none)
    /// first. Chosen explicitly by the caller: there is no default and no fallback (a fake
    /// device must never stand in for a missing real one). See [`crate::backends`].
    pub backends: Vec<Arc<dyn Backend>>,
    /// Where the local ctrl and data sockets listen, for the daemon's whole life.
    pub listen: Listen,
    /// Whether the network listener runs beside the local one, and where.
    pub network: NetworkChoice,
    /// The server key pair and authorized-clients files. The key is loaded when it exists
    /// (outgoing rig links use it with the network off too) and generated when the network
    /// first comes on; `None`: no keys at all (in-process daemons).
    pub keys: Option<NetworkSecurity>,
    /// The host's interfaces, read at each bind and while bound.
    pub interfaces: crate::network::Interfaces,
    /// Retry delays of a network that is not listening.
    pub net_retry: crate::network::Backoff,
    /// The hard upper bound of the system max level, dBFS RMS (0 dBFS = RMS of a full-scale
    /// sine). The level clients set at run time (`gen.ceiling`, kept in `rig_settings`)
    /// never exceeds it; requests above the level in force are refused, and the output path
    /// enforces the matching sample peak.
    pub max_level_dbfs: f64,
    /// Rig settings file (system max level, output labels); `None` keeps them in memory
    /// only, and the system max level starts at `max_level_dbfs`.
    pub rig_settings: Option<PathBuf>,
    /// Where the rig's SPL meters keep their logs (in the session directory format, apart
    /// from the autosave so that no load, new session or set-aside autosave ends them);
    /// `None` keeps them in memory only.
    pub rig_logs: Option<PathBuf>,
    /// Stimulus lease expiry after the last refresh (Q6: 1.5 s).
    pub lease_expiry: Duration,
    /// State-event replay buffer.
    pub replay: ReplayLimits,
    /// Per-client request-id dedup window.
    pub dedup: DedupLimits,
    /// Most frames per second per topic, local and network alike: the packed frames
    /// (`docs/design/wire-slim.md`) keep a full tuning session at this rate within a Wi-Fi or
    /// VPN link.
    pub publish_fps: u32,
    /// Keepalive period (Q2: 250 ms).
    pub keepalive: Duration,
    /// Where `file.save` / `file.load` put sessions given by name.
    pub session_dir: PathBuf,
    /// Calibration store file (`docs/design/q7-calibration.md` §7); `None` keeps
    /// calibrations in memory only.
    pub cal_store: Option<PathBuf>,
    /// Autosave of the measurements and traces; `None` keeps them in memory only.
    pub autosave: Option<AutosaveConfig>,
    /// Where raw capture files are written (`rec.start`) and found by name
    /// (`session.replay`); unfinished ones found there at start are finished as
    /// interrupted. `None`: this daemon does not record.
    pub recording_dir: Option<PathBuf>,
    /// The plan library's directory (`plan.*`); `None` keeps plans in memory only.
    pub plan_dir: Option<PathBuf>,
    /// The rig's name, told every client at hello and advertised while the network listens; display
    /// only (default *ac2 on <hostname>*).
    pub name: String,
    /// Where the daemon's identity comes from, loaded at every start in every mode.
    pub rig_id: RigIdSource,
    /// mDNS: the advert while the network listens, and the browse that follows paired rigs
    /// to new addresses. `None`: no mDNS at all.
    pub mdns: Option<Mdns>,
    /// Local time of day, which decides each band-meter second's day or night limits.
    pub local_clock: LocalClock,
    /// Tell systemd when the daemon serves, ping its watchdog from the control loop and say
    /// when it stops (`ac2_paths::notify`; nothing without `NOTIFY_SOCKET`). Only the `ac2d`
    /// program: a daemon run inside another process (a test) must not ping for its unit.
    pub systemd_notify: bool,
    /// Where the system clock's status comes from.
    pub system_clock: SystemClockSource,
}

/// Where the daemon reads its system clock's status (`docs/design/multi-rig.md` §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SystemClockSource {
    /// What the OS states (`adjtimex` on Linux, nothing elsewhere).
    #[default]
    Os,
    /// A status stated by whoever started the daemon: a test of a rig whose clock is synced
    /// or not, whatever the machine running it says.
    Stated(ac2_proto::model::SystemClock),
}

impl SystemClockSource {
    /// The status now.
    pub(crate) fn read(self) -> ac2_proto::model::SystemClock {
        match self {
            Self::Os => crate::sysclock::read(),
            Self::Stated(c) => c,
        }
    }
}

/// How the daemon tells the local time of day from its wall clock: the band meter's day
/// (07–22) and night (22–07) limits follow local time (`docs/design/band-leq.md`, *Day
/// and night*).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LocalClock {
    /// The host's time zone rules (daylight saving included).
    #[default]
    Host,
    /// A fixed offset east of UTC: a rig whose host keeps UTC, and tests that place the
    /// daemon at a chosen time of day.
    FixedOffset {
        /// Seconds east of UTC (−18 h … +18 h).
        east_s: i32,
    },
}

impl LocalClock {
    /// Seconds since local midnight of the wall time `wall_ns` (Unix ns).
    pub fn seconds_of_day(self, wall_ns: u64) -> u32 {
        let secs = i64::try_from(wall_ns / 1_000_000_000).unwrap_or(i64::MAX);
        match self {
            LocalClock::Host => {
                use chrono::{TimeZone, Timelike};
                match chrono::Local.timestamp_opt(secs, 0) {
                    chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => {
                        t.num_seconds_from_midnight()
                    }
                    // A Unix instant always has a local time; UTC if the zone says not.
                    chrono::LocalResult::None => secs.rem_euclid(86_400) as u32,
                }
            }
            LocalClock::FixedOffset { east_s } => {
                (secs + i64::from(east_s)).rem_euclid(86_400) as u32
            }
        }
    }
}

/// Where the daemon autosaves, and whether it loads that autosave when it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutosaveConfig {
    /// The autosave (a session directory); the previous manifest stays inside it
    /// (`session.prev.json`), set-aside copies live beside it (`<dir>.damaged`, …).
    pub dir: PathBuf,
    /// Load the autosave at start (disarmed, as `file.load`). `false` moves it aside to
    /// `<dir>.unrestored` and starts empty.
    pub restore: bool,
}

/// Where a daemon's [`RigId`](ac2_proto::units::RigId) comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RigIdSource {
    /// Kept in this file (`ac2_paths::rig_id`): made if absent, set aside and made anew if
    /// unreadable.
    File(PathBuf),
    /// Random per start: a daemon of which nothing persists (the simulated rig, tests).
    Ephemeral,
}

/// The daemon's mDNS. The advert names the rig ([`DaemonConfig::name`]) and its key
/// fingerprint; it never grants anything (clients still need a pinned key and an entry in
/// the authorized-clients file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mdns {
    /// mDNS port and interfaces (tests confine it to loopback).
    pub options: ac2_discovery::Options,
    /// Advertise while the network listens (`--no-mdns` clears it; the browse that follows
    /// paired rigs runs either way).
    pub advertise: bool,
}

impl fmt::Debug for DaemonConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DaemonConfig")
            .field(
                "backends",
                &self.backends.iter().map(|b| b.kind()).collect::<Vec<_>>(),
            )
            .field("listen", &self.listen)
            .field("network", &self.network)
            .field("keys", &self.keys)
            .field("net_retry", &self.net_retry)
            .field("max_level_dbfs", &self.max_level_dbfs)
            .field("rig_settings", &self.rig_settings)
            .field("rig_logs", &self.rig_logs)
            .field("lease_expiry", &self.lease_expiry)
            .field("replay", &self.replay)
            .field("dedup", &self.dedup)
            .field("publish_fps", &self.publish_fps)
            .field("keepalive", &self.keepalive)
            .field("session_dir", &self.session_dir)
            .field("cal_store", &self.cal_store)
            .field("autosave", &self.autosave)
            .field("recording_dir", &self.recording_dir)
            .field("plan_dir", &self.plan_dir)
            .field("name", &self.name)
            .field("rig_id", &self.rig_id)
            .field("mdns", &self.mdns)
            .field("local_clock", &self.local_clock)
            .field("system_clock", &self.system_clock)
            .field("systemd_notify", &self.systemd_notify)
            .finish()
    }
}

impl DaemonConfig {
    /// Default [`Self::publish_fps`]: the display refresh the UI interpolates to.
    pub const PUBLISH_FPS: u32 = 60;

    /// Default limits (Q2/Q5/Q6) for `backend` (the only one offered) listening on
    /// `listen`, with a global maximum of `max_level_dbfs`; an [`RigIdSource::Ephemeral`]
    /// id, which a daemon that keeps state sets to its file.
    pub fn new(backend: Arc<dyn Backend>, listen: Listen, max_level_dbfs: f64) -> Self {
        Self {
            backends: vec![backend],
            listen,
            network: NetworkChoice::Off,
            keys: None,
            interfaces: crate::network::Interfaces::host(),
            net_retry: crate::network::Backoff::default(),
            max_level_dbfs,
            rig_settings: None,
            rig_logs: None,
            lease_expiry: Duration::from_millis(1500),
            replay: ReplayLimits::default(),
            dedup: DedupLimits::default(),
            publish_fps: Self::PUBLISH_FPS,
            keepalive: Duration::from_millis(250),
            session_dir: ac2_paths::session_dir(),
            cal_store: None,
            autosave: None,
            recording_dir: None,
            plan_dir: None,
            name: ac2_discovery::default_rig_name(),
            rig_id: RigIdSource::Ephemeral,
            mdns: None,
            local_clock: LocalClock::Host,
            systemd_notify: false,
            system_clock: SystemClockSource::Os,
        }
    }
}

/// Replay buffer bounds (Q5 decision 5a): whichever holds fewer events wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayLimits {
    /// Most events kept.
    pub events: usize,
    /// Oldest event age kept.
    pub age: Duration,
}

impl Default for ReplayLimits {
    fn default() -> Self {
        Self {
            events: 1024,
            age: Duration::from_secs(60),
        }
    }
}

/// Request-id dedup window per client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DedupLimits {
    /// Most ids remembered per client.
    pub ids: usize,
    /// How long an id is remembered.
    pub age: Duration,
}

impl Default for DedupLimits {
    fn default() -> Self {
        Self {
            ids: 256,
            age: Duration::from_secs(30),
        }
    }
}

/// The local transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listen {
    /// In-process only (`inproc://<name>/ctrl`, `…/data`), for a daemon inside a test.
    /// Clients must use [`crate::Handle::context`].
    Inproc {
        /// Unique name within the process.
        name: String,
    },
    /// Local machine only: `ipc://` endpoints, or `tcp://` on a loopback address (the
    /// Windows default). No authentication: the OS user boundary is the trust boundary.
    Local {
        /// Ctrl (ROUTER) endpoint.
        ctrl: String,
        /// Data (ROUTER) endpoint.
        data: String,
    },
}

/// Key files of the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkSecurity {
    /// Server CURVE key pair; generated on first run (atomic write, 0600 on Unix).
    pub server_key_file: PathBuf,
    /// Authorized client keys (`<name> <Z85 key>` per line); created empty if missing.
    pub authorized_clients_file: PathBuf,
}

/// A transport configuration that cannot be served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenError(pub String);

impl fmt::Display for ListenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ListenError {}

/// The network's default ctrl port; data is + 1.
pub const DEFAULT_PORT: u16 = 47_820;

/// Ctrl port of the Windows local endpoints (data + 1), apart from the network's default so
/// the two never clash.
pub const LOCAL_PORT: u16 = 47_822;

/// Per-user directory of the local daemon's sockets and pid file: `$AC2_RUNTIME_DIR`, else
/// `$XDG_RUNTIME_DIR/ac2`, else `<temp>/ac2-<user>` (the same rule as `ac2-client`).
pub fn runtime_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("AC2_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("ac2");
    }
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_owned());
    std::env::temp_dir().join(format!("ac2-{user}"))
}

/// The pid file of the local daemon, in [`runtime_dir`].
pub fn pid_file() -> PathBuf {
    runtime_dir().join("ac2d.pid")
}

pub(crate) fn tcp_host_port(endpoint: &str) -> Option<(&str, &str)> {
    let rest = endpoint.strip_prefix("tcp://")?;
    if let Some(v6) = rest.strip_prefix('[') {
        let (host, tail) = v6.split_once(']')?;
        let port = tail.strip_prefix(':')?;
        Some((host, port))
    } else {
        rest.rsplit_once(':')
    }
}

fn is_loopback_tcp(endpoint: &str) -> bool {
    matches!(
        tcp_host_port(endpoint),
        Some(("127.0.0.1" | "::1" | "localhost", _))
    )
}

impl Listen {
    /// The platform's default local endpoints: `ipc://<runtime dir>/ctrl.sock` and
    /// `data.sock` on Unix, `tcp://127.0.0.1:47822` / `:47823` on Windows.
    pub fn default_local() -> Self {
        #[cfg(unix)]
        {
            let dir = runtime_dir();
            Self::Local {
                ctrl: format!("ipc://{}", dir.join("ctrl.sock").display()),
                data: format!("ipc://{}", dir.join("data.sock").display()),
            }
        }
        #[cfg(not(unix))]
        {
            Self::Local {
                ctrl: format!("tcp://127.0.0.1:{LOCAL_PORT}"),
                data: format!("tcp://127.0.0.1:{}", LOCAL_PORT + 1),
            }
        }
    }

    /// Checks that the endpoints are local (`ipc://` or loopback TCP), and that `network`
    /// fits beside them ([`crate::network::check_port`]).
    pub fn validate(&self, network: &NetworkChoice) -> Result<(), ListenError> {
        if let NetworkChoice::Fixed(NetListen {
            port: NetPort::Port(p),
            ..
        }) = network
        {
            crate::network::check_port(*p, self)?;
        }
        match self {
            Self::Inproc { name } => {
                if name.is_empty() || name.contains('\0') {
                    return Err(ListenError("inproc name must be non-empty".into()));
                }
            }
            Self::Local { ctrl, data } => {
                for e in [ctrl, data] {
                    let ok = e.starts_with("ipc://") || is_loopback_tcp(e);
                    if !ok {
                        return Err(ListenError(format!(
                            "{e}: local mode serves only ipc:// or tcp:// on a loopback address; \
                             other interfaces are the network's (CURVE)"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_refuses_non_loopback() {
        let ok = Listen::Local {
            ctrl: "tcp://127.0.0.1:1".into(),
            data: "ipc:///tmp/x".into(),
        };
        assert!(ok.validate(&NetworkChoice::Off).is_ok());
        let bad = Listen::Local {
            ctrl: "tcp://0.0.0.0:1".into(),
            data: "ipc:///tmp/x".into(),
        };
        assert!(bad.validate(&NetworkChoice::Off).is_err());
    }

    #[test]
    fn a_fixed_network_must_fit_beside_local_tcp() {
        let local = Listen::Local {
            ctrl: "tcp://127.0.0.1:47820".into(),
            data: "tcp://127.0.0.1:47821".into(),
        };
        let at = |port| {
            NetworkChoice::Fixed(NetListen {
                bind: NetBind::All,
                port: NetPort::Port(port),
            })
        };
        assert!(local.validate(&at(DEFAULT_PORT)).is_err());
        assert!(local.validate(&at(47_819)).is_err());
        assert!(local.validate(&at(47_822)).is_ok());
        assert!(local.validate(&at(0)).is_err());
    }
}
