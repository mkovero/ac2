//! Where a rig is, as the app shows it: by the host name of its mDNS advert once one was
//! seen for it, else by the address the operator typed. The numeric address stays in the
//! hovers and on Settings › Connection.
//!
//! The app never resolves a name itself: an advert is what the connect dialog's browse
//! delivered, taken when the rig is picked from its list or noted while the dialog is open.

use super::*;
use crate::prefs::RigAddr;

/// A rig's mDNS advert, as far as it says where the rig is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdvertSeen {
    /// The mDNS host name (`pupu.local`).
    pub host: String,
    /// The fingerprint of the server key it advertises.
    pub fingerprint: String,
    /// Its addresses, as text.
    pub addresses: Vec<String>,
}

impl AdvertSeen {
    /// The advert is of the daemon reached at `host` with the key pinned as `fingerprint`:
    /// the key says which rig it is wherever it answers, an address only while no key is
    /// pinned for it.
    fn is_of(&self, host: &str, fingerprint: Option<&str>) -> bool {
        match fingerprint {
            Some(f) => f == self.fingerprint,
            None => {
                self.host.eq_ignore_ascii_case(host) || self.addresses.iter().any(|a| a == host)
            }
        }
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

    /// The adverts the connect dialog's browse lists: a rig held at a typed address whose
    /// advert is among them is shown by that host name from now on, and remembered so.
    pub fn adverts_seen(&mut self, seen: &[AdvertSeen]) {
        let mut named = false;
        for r in std::iter::once(&mut self.rig).chain(&mut self.others.background) {
            let Some(RigAddr::Network {
                host,
                fingerprint,
                mdns_host,
                ..
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
        }
        if !named {
            return;
        }
        // The same rigs as remembered: the next start shows the name before any advert.
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
    }
}
