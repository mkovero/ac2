//! Where the daemon listens.
//!
//! Local mode (the default) never touches the network: `ipc://` sockets in a per-user runtime
//! directory on Unix, `tcp://127.0.0.1` on Windows (where ipc is not used). Remote mode is
//! `tcp://<host>:<port>` for ctrl and `<port + 1>` for data, always with CURVE.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Default ctrl port (local Windows and remote); data is this + 1.
pub const DEFAULT_PORT: u16 = 47_820;

/// Ctrl (ROUTER) and data (XPUB) endpoints of one daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// DEALER connects here.
    pub ctrl: String,
    /// SUB connects here.
    pub data: String,
}

impl Endpoints {
    /// The local daemon's endpoints for this OS and user.
    pub fn local() -> Self {
        #[cfg(unix)]
        {
            Self::local_in(&runtime_dir())
        }
        #[cfg(not(unix))]
        {
            Self::tcp("127.0.0.1", DEFAULT_PORT)
        }
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

/// Per-user directory holding the local daemon's sockets and pid file:
/// `$AC2_RUNTIME_DIR`, else `$XDG_RUNTIME_DIR/ac2`, else `<temp>/ac2-<user>`.
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

#[cfg(test)]
mod tests {
    use super::*;

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
