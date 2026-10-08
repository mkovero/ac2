//! Daemon configuration: backend, transports, limits.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ac2_audio::Backend;

/// Everything [`crate::Daemon::start`] needs.
#[derive(Clone)]
pub struct DaemonConfig {
    /// The audio backends offered, the default one (used when `session.open` names none)
    /// first. Chosen explicitly by the caller: there is no default and no fallback (a fake
    /// device must never stand in for a missing real one). See [`crate::backends`].
    pub backends: Vec<Arc<dyn Backend>>,
    /// Where the ctrl and data sockets listen.
    pub listen: Listen,
    /// The hard upper bound of the system max level, dBFS RMS (0 dBFS = RMS of a full-scale
    /// sine). The level clients set at run time (`gen.ceiling`, kept in `rig_settings`)
    /// never exceeds it; requests above the level in force are refused, and the output path
    /// enforces the matching sample peak.
    pub max_level_dbfs: f64,
    /// Rig settings file (system max level, output labels); `None` keeps them in memory
    /// only, and the system max level starts at `max_level_dbfs`.
    pub rig_settings: Option<PathBuf>,
    /// Stimulus lease expiry after the last refresh (Q6: 1.5 s).
    pub lease_expiry: Duration,
    /// State-event replay buffer.
    pub replay: ReplayLimits,
    /// Per-client request-id dedup window.
    pub dedup: DedupLimits,
    /// Frames per second per topic; `None` = 60 for local transports, 30 for network.
    pub publish_fps: Option<u32>,
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
    /// mDNS advert of a network-mode daemon (`_ac2._tcp`); ignored in local modes, which
    /// are not reachable from the network. `None` advertises nothing.
    pub advertise: Option<Advertise>,
    /// Local time of day, which decides each band-meter second's day or night limits.
    pub local_clock: LocalClock,
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
    /// The autosave (a session directory); its backup and set-aside copies live beside it
    /// (`<dir>.prev`, …).
    pub dir: PathBuf,
    /// Load the autosave at start (disarmed, as `file.load`). `false` moves it aside to
    /// `<dir>.unrestored` and starts empty.
    pub restore: bool,
}

/// What a network-mode daemon advertises over mDNS. The advert names the rig and its key
/// fingerprint; it never grants anything (clients still need a pinned key and an entry in
/// the authorized-clients file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advertise {
    /// Rig name shown by `ac2 discover` and the UI.
    pub name: String,
    /// mDNS port and interfaces (tests confine it to loopback).
    pub mdns: ac2_discovery::Options,
}

impl fmt::Debug for DaemonConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DaemonConfig")
            .field(
                "backends",
                &self.backends.iter().map(|b| b.kind()).collect::<Vec<_>>(),
            )
            .field("listen", &self.listen)
            .field("max_level_dbfs", &self.max_level_dbfs)
            .field("rig_settings", &self.rig_settings)
            .field("lease_expiry", &self.lease_expiry)
            .field("replay", &self.replay)
            .field("dedup", &self.dedup)
            .field("publish_fps", &self.publish_fps)
            .field("keepalive", &self.keepalive)
            .field("session_dir", &self.session_dir)
            .field("cal_store", &self.cal_store)
            .field("autosave", &self.autosave)
            .field("recording_dir", &self.recording_dir)
            .field("advertise", &self.advertise)
            .field("local_clock", &self.local_clock)
            .finish()
    }
}

impl DaemonConfig {
    /// Default limits (Q2/Q5/Q6) for `backend` (the only one offered) listening on
    /// `listen`, with a global maximum of `max_level_dbfs`.
    pub fn new(backend: Arc<dyn Backend>, listen: Listen, max_level_dbfs: f64) -> Self {
        Self {
            backends: vec![backend],
            listen,
            max_level_dbfs,
            rig_settings: None,
            lease_expiry: Duration::from_millis(1500),
            replay: ReplayLimits::default(),
            dedup: DedupLimits::default(),
            publish_fps: None,
            keepalive: Duration::from_millis(250),
            session_dir: ac2_paths::session_dir(),
            cal_store: None,
            autosave: None,
            recording_dir: None,
            advertise: None,
            local_clock: LocalClock::Host,
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

/// Transports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listen {
    /// In-process only (`inproc://<name>/ctrl`, `…/data`), for an embedded daemon. Clients
    /// must use [`crate::Handle::context`].
    Inproc {
        /// Unique name within the process.
        name: String,
    },
    /// Local machine only: `ipc://` endpoints, or `tcp://` on a loopback address (the
    /// Windows default). No authentication: the OS user boundary is the trust boundary.
    Local {
        /// Ctrl (ROUTER) endpoint.
        ctrl: String,
        /// Data (XPUB) endpoint.
        data: String,
    },
    /// Network mode: `tcp://` on any interface, CURVE on both sockets, every client
    /// authenticated by the ZAP handler against the authorized-clients file.
    Network {
        /// Ctrl (ROUTER) endpoint.
        ctrl: String,
        /// Data (XPUB) endpoint.
        data: String,
        /// Key files.
        security: NetworkSecurity,
    },
}

/// Key files of network mode.
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

/// Ctrl port of the Windows local endpoints and the network-mode default; data is + 1.
pub const DEFAULT_PORT: u16 = 47_820;

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

fn tcp_host_port(endpoint: &str) -> Option<(&str, &str)> {
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
    /// `data.sock` on Unix, `tcp://127.0.0.1:47820` / `:47821` on Windows.
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
                ctrl: format!("tcp://127.0.0.1:{DEFAULT_PORT}"),
                data: format!("tcp://127.0.0.1:{}", DEFAULT_PORT + 1),
            }
        }
    }

    /// Network mode from `tcp://<iface>[:<port>]`: ctrl on `port` (default
    /// [`DEFAULT_PORT`]), data on `port + 1`.
    pub fn network(endpoint: &str, security: NetworkSecurity) -> Result<Self, ListenError> {
        let bad = || ListenError(format!("{endpoint}: expected tcp://<iface>[:<port>]"));
        let rest = endpoint.strip_prefix("tcp://").ok_or_else(bad)?;
        let with_port = if tcp_host_port(endpoint).is_some_and(|(h, p)| {
            !h.is_empty() && !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit())
        }) {
            endpoint.to_owned()
        } else if rest.starts_with('[') || !rest.contains(':') {
            format!("tcp://{rest}:{DEFAULT_PORT}")
        } else {
            return Err(bad());
        };
        let (host, port) = tcp_host_port(&with_port).ok_or_else(bad)?;
        let port: u16 = port
            .parse()
            .map_err(|_| ListenError(format!("{endpoint}: bad port")))?;
        let data_port = port
            .checked_add(1)
            .filter(|_| port != 0)
            .ok_or_else(|| ListenError(format!("{endpoint}: need a fixed port below 65535")))?;
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        Ok(Self::Network {
            ctrl: format!("tcp://{host}:{port}"),
            data: format!("tcp://{host}:{data_port}"),
            security,
        })
    }

    /// Checks that the endpoints fit the mode: local endpoints are `ipc://` or loopback TCP,
    /// network endpoints are TCP.
    pub fn validate(&self) -> Result<(), ListenError> {
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
                             other interfaces need network mode (CURVE)"
                        )));
                    }
                }
            }
            Self::Network { ctrl, data, .. } => {
                for e in [ctrl, data] {
                    if tcp_host_port(e).is_none() {
                        return Err(ListenError(format!("{e}: network mode needs tcp://")));
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether this is network mode.
    pub fn is_network(&self) -> bool {
        matches!(self, Self::Network { .. })
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
        assert!(ok.validate().is_ok());
        let bad = Listen::Local {
            ctrl: "tcp://0.0.0.0:1".into(),
            data: "ipc:///tmp/x".into(),
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn network_ports() {
        let sec = NetworkSecurity {
            server_key_file: "k".into(),
            authorized_clients_file: "a".into(),
        };
        let l = Listen::network("tcp://0.0.0.0:5000", sec.clone()).expect("valid");
        assert_eq!(
            l,
            Listen::Network {
                ctrl: "tcp://0.0.0.0:5000".into(),
                data: "tcp://0.0.0.0:5001".into(),
                security: sec.clone()
            }
        );
        assert!(Listen::network("tcp://0.0.0.0:65535", sec.clone()).is_err());
        assert!(matches!(
            Listen::network("tcp://10.0.0.2", sec.clone()),
            Ok(Listen::Network { ctrl, data, .. })
                if ctrl == "tcp://10.0.0.2:47820" && data == "tcp://10.0.0.2:47821"
        ));
        assert!(Listen::network("ipc:///x", sec).is_err());
    }
}
