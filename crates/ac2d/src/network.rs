//! The network listener: where it binds, how it is doing, and the CURVE pair it binds.
//!
//! The daemon always serves its local pair ([`crate::Listen`]); the network pair is an
//! optional listener beside it, made in a context of its own on each attach and closed on
//! each detach. No failure of the network stops the daemon: a bind that fails, an address
//! that goes away and a dead ZAP handler all end in [`NetStatus::Waiting`] or
//! [`NetStatus::Failed`], retried on [`Backoff`], so an unattended rig comes back by itself.

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use ac2_zmq::{AuthorizedHandle, KeyPair, SecureContext, Socket, SocketType};

use crate::config::{DEFAULT_PORT, Listen, ListenError, tcp_host_port};

/// What the network binds to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetBind {
    /// Every IPv4 interface (`tcp://*`).
    All,
    /// One interface by name, resolved by the daemon to its address at each bind: its first
    /// IPv4 address, else its first global IPv6 one.
    Interface(String),
    /// One address of this host.
    Addr(IpAddr),
}

/// The network's ctrl port; data is on the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetPort {
    /// A fixed port (see [`check_port`]).
    Port(u16),
    /// Each socket on a port the OS picks, on every bind: for daemons inside tests, which
    /// run side by side. Never from the command line (clients find data on ctrl + 1).
    Any,
}

/// Where the network listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetListen {
    /// Interface or address.
    pub bind: NetBind,
    /// Ctrl port.
    pub port: NetPort,
}

impl NetListen {
    /// From `tcp://<interface or address>[:<port>]` (`ac2d --listen`): ctrl on `port`
    /// (default [`DEFAULT_PORT`]), data on `port + 1`.
    pub fn parse(endpoint: &str) -> Result<Self, ListenError> {
        let bad = || ListenError(format!("{endpoint}: expected tcp://<iface>[:<port>]"));
        let rest = endpoint.strip_prefix("tcp://").ok_or_else(bad)?;
        let (host, port) = match tcp_host_port(endpoint) {
            Some((h, p))
                if !h.is_empty() && !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) =>
            {
                let port = p
                    .parse()
                    .map_err(|_| ListenError(format!("{endpoint}: bad port")))?;
                (h, port)
            }
            _ if rest.starts_with('[') || !rest.contains(':') => (
                rest.trim_start_matches('[').trim_end_matches(']'),
                DEFAULT_PORT,
            ),
            _ => return Err(bad()),
        };
        if host.is_empty() {
            return Err(bad());
        }
        let bind = match host {
            "*" | "0.0.0.0" => NetBind::All,
            h => h
                .parse()
                .map_or_else(|_| NetBind::Interface(h.to_owned()), NetBind::Addr),
        };
        Ok(Self {
            bind,
            port: NetPort::Port(port),
        })
    }
}

/// Whether and how the daemon serves the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkChoice {
    /// Local only.
    Off,
    /// Fixed for the daemon's life (`ac2d --listen`).
    Fixed(NetListen),
}

/// Why the network is not listening.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetReason {
    /// Another listener holds the port.
    AddressInUse,
    /// No interface of that name on this host.
    InterfaceMissing,
    /// The address (or any usable address of the interface) is not on this host.
    AddressMissing,
    /// The ZAP handler that authenticates the network's clients stopped.
    ZapFailed,
    /// No server key to serve CURVE with (no key files: an in-process or ephemeral daemon).
    NoServerKey,
    /// The I/O thread could not be handed the pair (its internal pipe stayed full).
    NotHandedOver,
    /// Anything else.
    Other {
        /// What failed.
        detail: String,
    },
}

impl fmt::Display for NetReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddressInUse => f.write_str("the port is in use"),
            Self::InterfaceMissing => f.write_str("no such interface"),
            Self::AddressMissing => f.write_str("the address is not on this host"),
            Self::ZapFailed => f.write_str("the authentication handler stopped"),
            Self::NoServerKey => f.write_str("no server key"),
            Self::NotHandedOver => f.write_str("the I/O thread did not take the network sockets"),
            Self::Other { detail } => f.write_str(detail),
        }
    }
}

/// How the network is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetStatus {
    /// Not served.
    Off,
    /// Bound: the endpoints as bound.
    Listening {
        /// Ctrl endpoint.
        ctrl: String,
        /// Data endpoint.
        data: String,
    },
    /// Not bound yet; retried.
    Waiting {
        /// Why.
        reason: NetReason,
    },
    /// Was bound and failed; retried with a new context.
    Failed {
        /// Why.
        reason: NetReason,
    },
}

/// One interface of the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetInterface {
    /// OS name (`eth0`, `en0`, `Ethernet 2`).
    pub name: String,
    /// Its addresses.
    pub addrs: Vec<IpAddr>,
    /// Operationally up.
    pub up: bool,
}

/// Lists the host's interfaces; injectable so tests make interfaces appear and vanish.
#[derive(Clone)]
pub struct Interfaces(Arc<dyn Fn() -> Vec<NetInterface> + Send + Sync>);

impl fmt::Debug for Interfaces {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Interfaces")
    }
}

impl Default for Interfaces {
    fn default() -> Self {
        Self::host()
    }
}

impl Interfaces {
    /// The host's own (`if-addrs`), loopback included: an address is checked against it.
    pub fn host() -> Self {
        Self::new(host_interfaces)
    }

    /// A lister of the caller's.
    pub fn new(f: impl Fn() -> Vec<NetInterface> + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub(crate) fn list(&self) -> Vec<NetInterface> {
        (self.0)()
    }
}

fn host_interfaces() -> Vec<NetInterface> {
    let all = match if_addrs::get_if_addrs() {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("cannot list the network interfaces: {e}");
            return Vec::new();
        }
    };
    let mut out: Vec<NetInterface> = Vec::new();
    for i in all {
        let up = i.is_oper_up();
        let ip = i.ip();
        match out.iter_mut().find(|n| n.name == i.name) {
            Some(n) => {
                n.addrs.push(ip);
                n.up |= up;
            }
            None => out.push(NetInterface {
                name: i.name,
                addrs: vec![ip],
                up,
            }),
        }
    }
    out
}

/// Retry delays of a network that is not listening: from `first`, doubling up to `max`.
/// While listening, the bound address is checked every `first`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    /// First delay, and the check period while listening.
    pub first: Duration,
    /// Longest delay.
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(2),
            max: Duration::from_secs(30),
        }
    }
}

impl Backoff {
    pub(crate) fn after(self, delay: Duration) -> Duration {
        (delay * 2).min(self.max).max(self.first)
    }
}

/// The one check of a network port, at start (`--listen`) and for any later setting: not
/// 0, not 65535 (data needs the port after it), and with the local pair on TCP (Windows,
/// or `--ctrl/--data` on tcp) neither it nor the next one a local port.
pub fn check_port(port: u16, local: &Listen) -> Result<(), ListenError> {
    if port == 0 || port == u16::MAX {
        return Err(ListenError(format!(
            "port {port}: need a fixed port from 1 to 65534 (data is on the next one)"
        )));
    }
    if let Listen::Local { ctrl, data } = local {
        for e in [ctrl, data] {
            if let Some(p) = tcp_host_port(e).and_then(|(_, p)| p.parse::<u16>().ok())
                && p != 0
                && (p == port || p == port + 1)
            {
                return Err(ListenError(format!(
                    "port {port}: the network's ports {port} and {} must not be the local \
                     endpoint {e}",
                    port + 1
                )));
            }
        }
    }
    Ok(())
}

/// What a bind resolves to on the host now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Host {
    Any,
    Addr(IpAddr),
}

fn global_v6(a: &IpAddr) -> bool {
    // Link-local addresses need a scope id that clients rarely carry.
    matches!(a, IpAddr::V6(v6) if !v6.is_loopback() && (v6.segments()[0] & 0xffc0) != 0xfe80)
}

pub(crate) fn resolve(bind: &NetBind, ifs: &[NetInterface]) -> Result<Host, NetReason> {
    match bind {
        NetBind::All => Ok(Host::Any),
        NetBind::Addr(a) if a.is_unspecified() => Ok(Host::Addr(*a)),
        NetBind::Addr(a) => {
            if ifs.iter().any(|i| i.addrs.contains(a)) {
                Ok(Host::Addr(*a))
            } else {
                Err(NetReason::AddressMissing)
            }
        }
        NetBind::Interface(name) => {
            let i = ifs
                .iter()
                .find(|i| i.name == *name)
                .ok_or(NetReason::InterfaceMissing)?;
            i.addrs
                .iter()
                .find(|a| a.is_ipv4())
                .or_else(|| i.addrs.iter().find(|a| global_v6(a)))
                .map(|a| Host::Addr(*a))
                .ok_or(NetReason::AddressMissing)
        }
    }
}

fn endpoint(host: Host, port: Option<u16>) -> String {
    let port = port.unwrap_or(0);
    match host {
        Host::Any => format!("tcp://*:{port}"),
        Host::Addr(IpAddr::V4(a)) => format!("tcp://{a}:{port}"),
        Host::Addr(IpAddr::V6(a)) => format!("tcp://[{a}]:{port}"),
    }
}

/// The network pair as bound: the context that authenticates it goes with it, dropped after
/// the sockets.
pub(crate) struct Pair {
    pub(crate) router: Socket,
    pub(crate) data: Socket,
    pub(crate) secure: SecureContext,
    pub(crate) ctrl_ep: String,
    pub(crate) data_ep: String,
}

fn other(what: impl Into<String>) -> impl FnOnce(ac2_zmq::Error) -> NetReason {
    let what = what.into();
    move |e| match e {
        ac2_zmq::Error::AddressInUse => NetReason::AddressInUse,
        e => NetReason::Other {
            detail: format!("{what}: {e}"),
        },
    }
}

/// How long a closing pair keeps trying to deliver what it queued (the last events).
const NET_LINGER: Duration = Duration::from_millis(200);

/// Makes a new secure context and binds both network sockets on `host`. A failure drops
/// what was made at once.
pub(crate) fn bind_pair(
    host: Host,
    port: NetPort,
    keys: &KeyPair,
    authorized: &AuthorizedHandle,
    audit: Arc<crate::authlog::AuthLog>,
) -> Result<Pair, NetReason> {
    let secure = SecureContext::with_keys(crate::ZAP_DOMAIN, authorized.clone(), move |d| {
        audit.record(d);
    })
    .map_err(other("ZAP handler"))?;
    let router = secure
        .curve_server_socket(SocketType::Router, keys)
        .map_err(other("CURVE ROUTER"))?;
    let data = secure
        .curve_server_socket(SocketType::Router, keys)
        .map_err(other("CURVE data ROUTER"))?;
    crate::ctrl_socket_options(&router).map_err(|e| NetReason::Other {
        detail: e.to_string(),
    })?;
    crate::data_socket_options(&data, true).map_err(|e| NetReason::Other {
        detail: e.to_string(),
    })?;
    for s in [&router, &data] {
        s.set_linger(Some(NET_LINGER))
            .map_err(other("network linger"))?;
    }
    let (c, d) = match port {
        NetPort::Port(p) => (Some(p), Some(p + 1)),
        NetPort::Any => (None, None),
    };
    let ctrl_ep = endpoint(host, c);
    let data_ep = endpoint(host, d);
    router
        .bind(&ctrl_ep)
        .map_err(other(format!("bind {ctrl_ep}")))?;
    data.bind(&data_ep)
        .map_err(other(format!("bind {data_ep}")))?;
    Ok(Pair {
        ctrl_ep: router.last_endpoint().unwrap_or(ctrl_ep),
        data_ep: data.last_endpoint().unwrap_or(data_ep),
        router,
        data,
        secure,
    })
}

/// Closes a detached pair on a thread of its own: its linger lets the last events leave and
/// the context's end waits for it, which the I/O thread must never do.
pub(crate) fn close(pair: Pair) {
    let r = std::thread::Builder::new()
        .name("ac2d-net-close".into())
        .spawn(move || {
            let Pair {
                router,
                data,
                secure,
                ..
            } = pair;
            drop(router);
            drop(data);
            drop(secure);
        });
    if let Err(e) = r {
        tracing::warn!("network pair closed on the I/O thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn ifs() -> Vec<NetInterface> {
        vec![
            NetInterface {
                name: "lo".into(),
                addrs: vec!["127.0.0.1".parse().unwrap(), "::1".parse().unwrap()],
                up: true,
            },
            NetInterface {
                name: "eth0".into(),
                addrs: vec![
                    "fe80::1".parse().unwrap(),
                    "2001:db8::5".parse().unwrap(),
                    "10.0.0.2".parse().unwrap(),
                ],
                up: true,
            },
            NetInterface {
                name: "wg0".into(),
                addrs: vec!["fe80::2".parse().unwrap(), "2001:db8::9".parse().unwrap()],
                up: true,
            },
        ]
    }

    #[test]
    fn an_interface_resolves_to_its_first_ipv4_else_a_global_ipv6() {
        let r = |b: NetBind| resolve(&b, &ifs());
        assert_eq!(
            r(NetBind::Interface("eth0".into())),
            Ok(Host::Addr("10.0.0.2".parse().unwrap()))
        );
        assert_eq!(
            r(NetBind::Interface("wg0".into())),
            Ok(Host::Addr("2001:db8::9".parse().unwrap()))
        );
        assert_eq!(
            r(NetBind::Interface("eth9".into())),
            Err(NetReason::InterfaceMissing)
        );
        assert_eq!(
            r(NetBind::Addr("10.0.0.3".parse().unwrap())),
            Err(NetReason::AddressMissing)
        );
        assert_eq!(r(NetBind::All), Ok(Host::Any));
    }

    #[test]
    fn listen_forms() {
        let p = |s: &str| NetListen::parse(s);
        assert_eq!(
            p("tcp://0.0.0.0:5000"),
            Ok(NetListen {
                bind: NetBind::All,
                port: NetPort::Port(5000)
            })
        );
        assert_eq!(
            p("tcp://eth0"),
            Ok(NetListen {
                bind: NetBind::Interface("eth0".into()),
                port: NetPort::Port(DEFAULT_PORT)
            })
        );
        assert_eq!(
            p("tcp://[::1]:7").map(|l| l.bind),
            Ok(NetBind::Addr("::1".parse().unwrap()))
        );
        assert!(p("ipc:///x").is_err());
        assert!(p("tcp://:5").is_err());
    }

    #[test]
    fn one_port_check() {
        let local = Listen::Local {
            ctrl: "tcp://127.0.0.1:6000".into(),
            data: "tcp://127.0.0.1:6001".into(),
        };
        assert!(check_port(0, &local).is_err());
        assert!(check_port(u16::MAX, &local).is_err());
        assert!(check_port(5999, &local).is_err());
        assert!(check_port(6001, &local).is_err());
        assert!(check_port(6002, &local).is_ok());
        let ipc = Listen::Local {
            ctrl: "ipc:///run/ctrl".into(),
            data: "ipc:///run/data".into(),
        };
        assert!(check_port(6000, &ipc).is_ok());
    }
}
