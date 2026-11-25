//! Where a rig is, as the app shows it: by the host name of its mDNS advert once one was
//! seen for it, else by the address the operator typed. The numeric address stays in the
//! hovers and on Settings › Connection.
//!
//! The app never resolves a name itself: an advert is what a browse delivered, taken when
//! the rig is picked from the connect dialog's list or noted while a browse runs.
//!
//! The same adverts say where a held rig answers now. Its key, not its address, is its
//! identity: an advert with the fingerprint pinned for a rig whose link is down, listing
//! IPv4 addresses that are not the one it is held at, moves the rig there. Nothing else in
//! an advert is trusted, and the CURVE handshake with the pinned key still proves the rig:
//! a false advert can only point a link at an address that never answers as that key.

use std::net::IpAddr;

use super::*;
use crate::prefs::RigAddr;

/// A rig's mDNS advert, as far as it says where the rig is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdvertSeen {
    /// The mDNS host name (`pupu.local`).
    pub host: String,
    /// The fingerprint of the server key it advertises.
    pub fingerprint: String,
    /// Its addresses, IPv4 first.
    pub addresses: Vec<IpAddr>,
    /// Its ctrl port.
    pub port: u16,
}

/// A held rig to reach at another address: the reducer moved it, the app starts its link
/// there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RigMoved {
    pub slot: u32,
    pub addr: RigAddr,
}

impl AdvertSeen {
    /// The advert is of the daemon reached at `host` with the key pinned as `fingerprint`:
    /// the key says which rig it is wherever it answers, an address only while no key is
    /// pinned for it.
    fn is_of(&self, host: &str, fingerprint: Option<&str>) -> bool {
        match fingerprint {
            Some(f) => f == self.fingerprint,
            None => self.host.eq_ignore_ascii_case(host) || self.lists(host),
        }
    }

    /// Whether `host` is one of the advert's addresses.
    fn lists(&self, host: &str) -> bool {
        host.parse::<IpAddr>()
            .is_ok_and(|ip| self.addresses.contains(&ip))
    }

    /// Where a rig held at `host:port` answers by this advert, when not there: the first
    /// IPv4 address it lists, at its port. `None` while the advert still lists `host` (or
    /// names it, a name staying a name) at `port`: a rig on several networks lists all its
    /// addresses, and the one held is as good as any. The app's links are IPv4 only, so an
    /// advert of IPv6 addresses alone moves nothing.
    fn moved_from(&self, host: &str, port: u16) -> Option<(String, u16)> {
        let host_there = self.host.eq_ignore_ascii_case(host)
            || host
                .parse::<IpAddr>()
                .is_ok_and(|ip| ip.is_ipv4() && self.addresses.contains(&ip));
        if host_there {
            return (port != self.port).then(|| (host.to_owned(), self.port));
        }
        let v4 = self.addresses.iter().find(|a| a.is_ipv4())?;
        Some((v4.to_string(), self.port))
    }
}

impl AppState {
    /// The host rig `r`'s daemon is shown at; `None` for a daemon on this computer: one on
    /// its own transport, or reached at a loopback address, which names no other host.
    pub fn rig_host(&self, r: &RigState) -> Option<String> {
        let address = r.daemon_addr.as_deref()?;
        let loopback = address.eq_ignore_ascii_case("localhost")
            || address
                .parse::<std::net::IpAddr>()
                .is_ok_and(|a| a.is_loopback());
        if loopback {
            return None;
        }
        let mdns = match &r.addr {
            Some(RigAddr::Network { mdns_host, .. }) => mdns_host.as_deref(),
            _ => None,
        };
        Some(ac2_scene::top_bar::host_name(address, mdns))
    }

    /// Whether a held rig could follow its advert: one reached over the network with a
    /// pinned key whose link is down. The app browses while one is.
    pub fn wants_adverts(&self) -> bool {
        self.rigs().any(|r| {
            matches!(
                r.addr,
                Some(RigAddr::Network {
                    fingerprint: Some(_),
                    ..
                })
            ) && follows(&r.conn)
        })
    }

    /// The adverts a browse lists: a rig held at a typed address whose advert is among
    /// them is shown by that host name from now on, and remembered so. A rig whose link is
    /// down and whose advert (by its pinned key) lists other addresses moves to the first
    /// IPv4 one, remembered so too: the rigs moved, for the app to link again. A connected
    /// rig stays where it answers whatever an advert says, and no rig moves to where
    /// another held rig already is (one address is one daemon).
    pub fn adverts_seen(&mut self, seen: &[AdvertSeen]) -> Vec<RigMoved> {
        let mut named = false;
        let mut moved = Vec::new();
        let held: Vec<(u32, RigAddr)> = self
            .rigs()
            .filter_map(|r| Some((r.slot, r.addr.clone()?)))
            .collect();
        for r in std::iter::once(&mut self.rig).chain(&mut self.others.background) {
            let follows = follows(&r.conn);
            let Some(RigAddr::Network {
                host,
                port,
                fingerprint,
                mdns_host,
            }) = &mut r.addr
            else {
                continue;
            };
            let Some(a) = seen.iter().find(|a| a.is_of(host, fingerprint.as_deref())) else {
                continue;
            };
            if mdns_host.as_deref() != Some(a.host.as_str()) {
                *mdns_host = Some(a.host.clone());
                named = true;
            }
            if !follows || fingerprint.as_deref() != Some(a.fingerprint.as_str()) {
                continue;
            }
            let Some((to_host, to_port)) = a.moved_from(host, *port) else {
                continue;
            };
            let to = RigAddr::Network {
                host: to_host.clone(),
                port: to_port,
                fingerprint: fingerprint.clone(),
                mdns_host: mdns_host.clone(),
            };
            if held
                .iter()
                .any(|(slot, at)| *slot != r.slot && at.same_place(&to))
            {
                continue;
            }
            *host = to_host;
            *port = to_port;
            moved.push(RigMoved {
                slot: r.slot,
                addr: to,
            });
        }
        if !named && moved.is_empty() {
            return moved;
        }
        // The same rigs as remembered: the next start shows the name before any advert,
        // and reaches a moved rig where it answers now.
        let held: Vec<(RigId, RigAddr)> = self
            .rigs()
            .filter_map(|r| Some((r.id, r.addr.clone()?)))
            .collect();
        for p in &mut self.prefs.rigs {
            if let Some((_, addr)) = held.iter().find(|(id, _)| *id == p.id)
                && p.addr != *addr
            {
                p.addr = addr.clone();
                self.prefs_dirty = true;
            }
        }
        moved
    }
}

/// Whether a rig with link state `conn` may move: its link is down for want of an answer.
/// One that answers, even to refuse this app's key, is where it is.
fn follows(conn: &ConnState) -> bool {
    matches!(
        conn.down(),
        Some(ac2_scene::chrome::RigDown::Unreachable | ac2_scene::chrome::RigDown::NetworkOff)
    )
}
