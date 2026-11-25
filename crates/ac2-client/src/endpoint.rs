//! Where the daemon listens.
//!
//! Local mode (the default) never touches the network: `ipc://` sockets in a per-user runtime
//! directory closed to other users, on every OS ([`ac2_paths::socket_dir`]). Remote mode is
//! `tcp://<host>:<port>` for ctrl and `<port + 1>` for data, always with CURVE.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Default network ctrl port; data is this + 1.
pub const DEFAULT_PORT: u16 = 47_820;

/// Ctrl (ROUTER) and data (ROUTER) endpoints of one daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// DEALER connects here.
    pub ctrl: String,
    /// Two DEALERs connect here: one for events and keepalives, one for frames.
    pub data: String,
}

impl Endpoints {
    /// The local daemon's endpoints for this OS and user.
    pub fn local() -> Self {
        Self::local_in(&runtime_dir())
    }

    /// Local ipc endpoints inside `dir` (`ctrl.sock`, `data.sock`).
    pub fn local_in(dir: &Path) -> Self {
        Self {
            ctrl: format!("ipc://{}", dir.join("ctrl.sock").display()),
            data: format!("ipc://{}", dir.join("data.sock").display()),
        }
    }

    /// What to check when a daemon on another host does not answer: its firewall must let
    /// both TCP ports in. `None` for local endpoints (ipc, loopback), where no firewall sits
    /// in between.
    pub fn firewall_hint(&self) -> Option<String> {
        let port = |ep: &str| -> Option<(String, u16)> {
            let rest = ep.strip_prefix("tcp://")?;
            let (host, port) = rest.rsplit_once(':')?;
            Some((host.trim_matches(['[', ']']).to_owned(), port.parse().ok()?))
        };
        let (host, ctrl) = port(&self.ctrl)?;
        let (_, data) = port(&self.data)?;
        let loopback = host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|a| a.is_loopback());
        (!loopback).then(|| {
            format!(
                "if the daemon is running, a firewall on its host may be blocking TCP ports \
                 {ctrl} and {data}: allow each port on its own (e.g. `sudo ufw allow \
                 {ctrl}/tcp` and `sudo ufw allow {data}/tcp`)"
            )
        })
    }

    /// A remote daemon.
    pub fn remote(addr: &RemoteAddr) -> Self {
        Self::tcp(&addr.host, addr.port)
    }

    fn tcp(host: &str, port: u16) -> Self {
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        Self {
            ctrl: format!("tcp://{host}:{port}"),
            data: format!("tcp://{host}:{}", port.wrapping_add(1)),
        }
    }
}

/// Per-user directory holding the local daemon's sockets and pid file (one rule for the
/// daemon and its clients).
pub use ac2_paths::runtime_dir;

/// The pid file the local daemon writes in [`runtime_dir`].
pub fn pid_file() -> PathBuf {
    runtime_dir().join("ac2d.pid")
}

/// `host`, `host:port`, `[v6]` or `[v6]:port`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteAddr {
    /// Host name or address (IPv6 without brackets).
    pub host: String,
    /// Ctrl port.
    pub port: u16,
}

impl fmt::Display for RemoteAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

impl FromStr for RemoteAddr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let bad_host = |h: &str| {
            h.is_empty()
                || !h
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
        };
        let port = |p: &str| -> Result<u16, String> {
            match p.parse::<u16>() {
                Ok(0) | Err(_) => Err(format!("invalid port {p:?}")),
                Ok(n) if n == u16::MAX => Err("port 65535 leaves no room for the data port".into()),
                Ok(n) => Ok(n),
            }
        };
        let (host, p) = if let Some(rest) = s.strip_prefix('[') {
            let (h, tail) = rest
                .split_once(']')
                .ok_or_else(|| format!("unclosed `[` in {s:?}"))?;
            match tail {
                "" => (h, None),
                t => (
                    h,
                    Some(
                        t.strip_prefix(':')
                            .ok_or_else(|| format!("expected `:port` after `]` in {s:?}"))?,
                    ),
                ),
            }
        } else {
            match s.split_once(':') {
                Some((h, p)) if !p.contains(':') => (h, Some(p)),
                Some(_) => return Err(format!("IPv6 addresses need brackets: [{s}]")),
                None => (s, None),
            }
        };
        if bad_host(host) {
            return Err(format!("invalid host {host:?}"));
        }
        Ok(Self {
            host: host.to_owned(),
            port: p.map(port).transpose()?.unwrap_or(DEFAULT_PORT),
        })
    }
}

/// The endpoints file the daemon holding the instance lock writes beside it in
/// [`runtime_dir`] (`ac2d::instance::endpoints_file`): a starter whose `ac2d` exited because
/// another runs finds that one here, wherever it listens.
pub fn endpoints_file() -> PathBuf {
    runtime_dir().join("ac2d.endpoints")
}

/// The line `ac2d --print-endpoints` prints and the endpoints file starts with.
const LISTENING: &str = "ac2d: listening ctrl=";

/// The endpoints of an `ac2d: listening ctrl=… data=…` line, as bound. A path may hold
/// spaces (a Windows profile named `John Smith`), so data runs to the end of the line.
pub fn parse_listening(line: &str) -> Option<Endpoints> {
    let (ctrl, data) = line
        .trim_end()
        .strip_prefix(LISTENING)?
        .split_once(" data=")?;
    (!ctrl.is_empty() && !data.is_empty()).then(|| Endpoints {
        ctrl: ctrl.to_owned(),
        data: data.to_owned(),
    })
}

/// The daemon holding the instance lock, as its endpoints file says: its local pair (ipc,
/// no authentication), which every daemon serves whatever its network does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockHolder {
    /// Where to reach it.
    pub endpoints: Endpoints,
}

impl LockHolder {
    /// The endpoints file's text (see [`endpoints_file`]); `None` when it is not one, or
    /// names anything but an ipc pair (no daemon serves its local pair on another
    /// transport).
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let endpoints = parse_listening(lines.next()?)?;
        let ipc = [&endpoints.ctrl, &endpoints.data]
            .iter()
            .all(|e| e.starts_with("ipc://"));
        (ipc && lines.next().is_none()).then_some(Self { endpoints })
    }

    /// Reads [`endpoints_file`]; `None` while there is none (the holder is still binding),
    /// it does not parse, or the directory it lies in is not this user's own (whoever can
    /// write there could point the app at a daemon of their own).
    pub fn read() -> Option<Self> {
        ac2_paths::socket_dir::check(&runtime_dir()).ok()?;
        Self::parse(&std::fs::read_to_string(endpoints_file()).ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_holders_endpoints_file() {
        let ep = |c: &str, d: &str| Endpoints {
            ctrl: c.into(),
            data: d.into(),
        };
        assert_eq!(
            LockHolder::parse(
                "ac2d: listening ctrl=ipc:///run/ac2/ctrl.sock data=ipc:///run/ac2/data.sock\n"
            ),
            Some(LockHolder {
                endpoints: ep("ipc:///run/ac2/ctrl.sock", "ipc:///run/ac2/data.sock")
            })
        );
        let windows = r"ipc://C:\Users\John Smith\AppData\Local\ac2\run";
        assert_eq!(
            LockHolder::parse(&format!(
                "ac2d: listening ctrl={windows}\\ctrl.sock data={windows}\\data.sock\n"
            )),
            Some(LockHolder {
                endpoints: ep(
                    &format!("{windows}\\ctrl.sock"),
                    &format!("{windows}\\data.sock")
                )
            })
        );
        // A local pair is never TCP: a file naming one names no daemon of this user's.
        assert_eq!(
            LockHolder::parse(
                "ac2d: listening ctrl=tcp://127.0.0.1:47822 data=tcp://127.0.0.1:47823\n"
            ),
            None
        );
        // Torn or foreign text names no daemon.
        assert_eq!(
            LockHolder::parse("ac2d: listening ctrl=tcp://1.2.3.4:5\n"),
            None
        );
        assert_eq!(LockHolder::parse("mode=local\n"), None);
        assert_eq!(
            LockHolder::parse("ac2d: listening ctrl=a data=b\nsomething else\n"),
            None
        );
    }

    #[test]
    fn firewall_hint_only_for_other_hosts() {
        let a: RemoteAddr = "rig.local".parse().unwrap_or_else(|e| panic!("{e}"));
        let h = Endpoints::remote(&a).firewall_hint().unwrap_or_default();
        assert!(
            h.contains("47820/tcp") && h.contains("47821/tcp") && h.contains("firewall"),
            "{h}"
        );
        for local in ["127.0.0.1:47820", "localhost", "[::1]:5000"] {
            let a: RemoteAddr = local.parse().unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(Endpoints::remote(&a).firewall_hint(), None, "{local}");
        }
        assert_eq!(
            Endpoints::local_in(Path::new("/run/ac2")).firewall_hint(),
            None
        );
    }

    #[test]
    fn remote_addr_forms() {
        let a: RemoteAddr = "foh-rig.local".parse().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a.port, DEFAULT_PORT);
        assert_eq!(Endpoints::remote(&a).data, "tcp://foh-rig.local:47821");
        let b: RemoteAddr = "10.0.0.2:5000".parse().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((b.host.as_str(), b.port), ("10.0.0.2", 5000));
        let c: RemoteAddr = "[fe80::1]:6000".parse().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(Endpoints::remote(&c).ctrl, "tcp://[fe80::1]:6000");
        assert_eq!(c.to_string(), "[fe80::1]:6000");
        for bad in [
            "",
            "host:0",
            "host:x",
            "fe80::1",
            "[x",
            "h/x",
            "host:65535",
            "[a]b",
        ] {
            assert!(bad.parse::<RemoteAddr>().is_err(), "{bad}");
        }
    }
}
