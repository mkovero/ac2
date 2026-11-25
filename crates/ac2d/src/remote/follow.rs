//! Following a paired rig to a new address. The app pairs rigs by key and gives each the
//! other's address; a DHCP lease that moves leaves that address dead. The rig's mDNS advert
//! carries its server key, and the key, not the address, is its identity: an advert with the
//! pinned key says where the rig answers now. Nothing else in an advert is trusted (its name
//! and fingerprint are the advertiser's word), and the CURVE handshake on the new address
//! still proves the key.

use std::net::IpAddr;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_discovery::{Browser, Options, Rig, Update};
use ac2_proto::model::{RemoteLink, RigPeer};
use ac2_proto::units::WallNs;

use super::RemoteMsg;
use crate::control::ControlMsg;

/// How long one browse runs before it starts over. A new browse asks again on every
/// interface, so a rig that moved while no announcement reached this one is found within
/// this time, at a few packets a minute.
const REBROWSE: Duration = Duration::from_secs(60);
/// How often the browse thread looks for a stop.
const STOP_POLL: Duration = Duration::from_millis(500);

/// The host (IPv6 without brackets) and port of a `tcp://host:port` endpoint.
fn host_port(ep: &str) -> Option<(&str, u16)> {
    let (host, port) = ep.strip_prefix("tcp://")?.rsplit_once(':')?;
    Some((
        host.trim_start_matches('[').trim_end_matches(']'),
        port.parse().ok()?,
    ))
}

fn endpoint(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("tcp://[{host}]:{port}")
    } else {
        format!("tcp://{host}:{port}")
    }
}

/// Whether this rig's links can reach `a`: their sockets are IPv4 only (libzmq's IPv6
/// option is off), so an IPv6 address never answers here however routable it is.
fn reachable(a: &IpAddr) -> bool {
    a.is_ipv4()
}

/// Where `peer` answers now by `advert`: its ctrl endpoint, when the advert
/// carries the key the app pinned for `peer` and the endpoint `peer` is paired at is not one
/// of the advert's reachable addresses, or is one of `failed`. `None` for another rig's
/// advert, one that resolved no address, one still naming the paired endpoint (a rig on
/// several networks lists all its addresses), or one whose reachable addresses all failed
/// (the link keeps trying the paired one). A rig still at its paired host but on another port
/// keeps the host as paired (a name stays a name).
pub(crate) fn followed(peer: &RigPeer, advert: &Rig, failed: &[IpAddr]) -> Option<String> {
    if advert.advert.server_key != peer.key || advert.addresses.is_empty() {
        return None;
    }
    let (paired, port) = host_port(&peer.ctrl)?;
    let paired_ip = paired.parse::<IpAddr>().ok();
    // Best first, as the advert ranks them.
    let mut usable = advert
        .addresses
        .iter()
        .filter(|a| reachable(a) && !failed.contains(a));
    let host_there = paired.eq_ignore_ascii_case(&advert.host)
        || paired_ip.is_some_and(|ip| usable.clone().any(|a| *a == ip));
    let host = if host_there {
        if port == advert.port {
            return None;
        }
        paired.to_owned()
    } else {
        usable.next()?.to_string()
    };
    // The data endpoint is the next port: an advert at the last port names no rig.
    advert.port.checked_add(1)?;
    Some(endpoint(&host, advert.port))
}

/// What a paired rig's link has shown about the addresses it was followed to since it was
/// last up. An address is given up when the link there fails, so an advert listing it
/// cannot keep the rig on it; the paired address the operator gave is never given up, since
/// a rig that is merely off fails the same way.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Followed {
    gave_up: Vec<IpAddr>,
    /// The address followed last and when the link to it started.
    last: Option<(IpAddr, WallNs)>,
}

impl Followed {
    /// Follows `peer` by `advert` while its link shows `link` (`None`: no link yet): the new
    /// ctrl endpoint, noted as followed at `now`. A link that is up keeps its endpoint whatever
    /// the advert lists.
    pub(crate) fn follow(
        &mut self,
        peer: &RigPeer,
        advert: &Rig,
        link: Option<&RemoteLink>,
        now: WallNs,
    ) -> Option<String> {
        if matches!(link, Some(RemoteLink::Up)) {
            return None;
        }
        let paired = host_port(&peer.ctrl).and_then(|(h, _)| h.parse::<IpAddr>().ok());
        let mut failed = self.gave_up.clone();
        if let Some((ip, at)) = self.last
            && paired == Some(ip)
            && link.is_some_and(|l| fails_since(l, at))
        {
            failed.push(ip);
        }
        let ctrl = followed(peer, advert, &failed)?;
        let to = host_port(&ctrl).and_then(|(h, _)| h.parse::<IpAddr>().ok());
        self.gave_up = failed;
        self.last = to.map(|ip| (ip, now));
        Some(ctrl)
    }
}

/// Whether `link` says its address does not lead to the rig, for a link started at `at`.
/// An unreachable report from before `at` is the previous link's; a rig that turned its
/// network off or has not admitted this one would refuse at any address.
fn fails_since(link: &RemoteLink, at: WallNs) -> bool {
    match link {
        RemoteLink::Unreachable { since } => *since >= at,
        RemoteLink::KeyChanged | RemoteLink::OtherRig { .. } => true,
        _ => false,
    }
}

/// The browse that hands every resolved advert to the control thread, running until
/// dropped.
pub(crate) struct PeerBrowse {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl PeerBrowse {
    pub(crate) fn start(opts: Options, to: Sender<ControlMsg>) -> std::io::Result<Self> {
        let (stop, stopped) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("ac2d-mdns-browse".into())
            .spawn(move || browse(&opts, &to, &stopped))?;
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for PeerBrowse {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn browse(opts: &Options, to: &Sender<ControlMsg>, stopped: &Receiver<()>) {
    let mut failing = false;
    loop {
        let browser = match Browser::start(opts) {
            Ok(b) => {
                failing = false;
                b
            }
            Err(e) => {
                if !failing {
                    tracing::warn!("mDNS browse: {e}; a paired rig that moves is not followed");
                    failing = true;
                }
                match stopped.recv_timeout(REBROWSE) {
                    Err(RecvTimeoutError::Timeout) => continue,
                    _ => return,
                }
            }
        };
        let until = Instant::now() + REBROWSE;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            if !matches!(stopped.try_recv(), Err(TryRecvError::Empty)) {
                return;
            }
            if let Some(Update::Resolved(rig)) = browser.next(left.min(STOP_POLL))
                && to
                    .send(ControlMsg::Remote(RemoteMsg::Advert(Box::new(rig))))
                    .is_err()
            {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use ac2_discovery::Advert;
    use ac2_proto::units::RigId;

    fn peer(ctrl: &str) -> RigPeer {
        RigPeer {
            rig: RigId(2),
            name: "ketunkolo".into(),
            ctrl: ctrl.into(),
            key: "k".repeat(40),
        }
    }

    fn advert(key: &str, host: &str, addresses: &[&str], port: u16) -> Rig {
        Rig {
            instance: "ketunkolo._ac2._tcp.local.".into(),
            advert: Advert {
                name: "ketunkolo".into(),
                version: "0".into(),
                proto: 1,
                fingerprint: "f".into(),
                server_key: key.into(),
            },
            host: host.into(),
            addresses: addresses.iter().map(|a| a.parse().unwrap()).collect(),
            port,
        }
    }

    #[test]
    fn a_rig_with_the_pinned_key_is_followed_to_its_new_address() {
        let p = peer("tcp://10.0.0.5:7000");
        let k = "k".repeat(40);
        assert_eq!(
            followed(&p, &advert(&k, "ketunkolo.local", &["10.0.0.9"], 7000), &[]),
            Some("tcp://10.0.0.9:7000".into())
        );
        // Its ports come from the advert too: the daemon may have been started on others.
        assert_eq!(
            followed(&p, &advert(&k, "ketunkolo.local", &["10.0.0.9"], 7100), &[]),
            Some("tcp://10.0.0.9:7100".into())
        );
    }

    #[test]
    fn a_rig_restarted_on_another_port_is_followed_at_its_host() {
        let k = "k".repeat(40);
        let a = advert(&k, "ketunkolo.local", &["10.0.0.5"], 7100);
        assert_eq!(
            followed(&peer("tcp://ketunkolo.local:7000"), &a, &[]),
            Some("tcp://ketunkolo.local:7100".into())
        );
        assert_eq!(
            followed(&peer("tcp://10.0.0.5:7000"), &a, &[]),
            Some("tcp://10.0.0.5:7100".into())
        );
    }

    #[test]
    fn only_the_pinned_key_moves_a_peer() {
        let p = peer("tcp://10.0.0.5:7000");
        // Another rig, whatever it calls itself.
        let other = advert(&"o".repeat(40), "ketunkolo.local", &["10.0.0.9"], 7000);
        assert_eq!(followed(&p, &other, &[]), None);
    }

    #[test]
    fn a_rig_still_at_its_paired_address_stays() {
        let k = "k".repeat(40);
        // One of several addresses, or the host name it was paired by.
        let a = advert(&k, "ketunkolo.local", &["10.0.0.5", "192.168.1.4"], 7000);
        assert_eq!(followed(&peer("tcp://10.0.0.5:7000"), &a, &[]), None);
        assert_eq!(followed(&peer("tcp://ketunkolo.local:7000"), &a, &[]), None);
        assert_eq!(
            followed(
                &peer("tcp://[fe80::1]:7000"),
                &advert(&k, "h", &["fe80::1"], 7000),
                &[]
            ),
            None
        );
        // An advert that resolved no address says nothing about where the rig is.
        assert_eq!(
            followed(
                &peer("tcp://10.0.0.5:7000"),
                &advert(&k, "h", &[], 7000),
                &[]
            ),
            None
        );
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn an_address_this_rig_cannot_reach_is_never_followed() {
        let k = "k".repeat(40);
        let p = peer("tcp://192.168.9.27:7000");
        // The ULA alone (as a resolve that heard AAAA first gives it): the paired address stays.
        let ula = advert(&k, "pupu.local", &["fddd::27"], 7000);
        assert_eq!(followed(&p, &ula, &[]), None);
        // Both families: the paired IPv4 address is listed, so nothing moves.
        let both = advert(&k, "pupu.local", &["fddd::27", "192.168.9.27"], 7000);
        assert_eq!(followed(&p, &both, &[]), None);
        // A peer saved at the ULA moves to the IPv4 address even though the ULA is listed.
        assert_eq!(
            followed(&peer("tcp://[fddd::27]:7000"), &both, &[]),
            Some("tcp://192.168.9.27:7000".into())
        );
    }

    #[test]
    fn a_link_that_is_up_keeps_its_endpoint() {
        let k = "k".repeat(40);
        let p = peer("tcp://10.0.0.5:7000");
        let elsewhere = advert(&k, "ketunkolo.local", &["10.0.0.9"], 7000);
        let mut f = Followed::default();
        assert_eq!(
            f.follow(&p, &elsewhere, Some(&RemoteLink::Up), WallNs(1)),
            None
        );
        assert_eq!(f, Followed::default());
        // Down, the same advert moves it.
        assert!(
            f.follow(&p, &elsewhere, Some(&RemoteLink::Connecting), WallNs(2))
                .is_some()
        );
    }

    #[test]
    fn a_followed_address_that_never_connects_is_given_up() {
        let k = "k".repeat(40);
        // On two networks; this rig reaches only the second.
        let a = advert(&k, "ketunkolo.local", &["10.0.0.9", "10.1.0.9"], 7000);
        let mut f = Followed::default();
        let mut p = peer("tcp://10.0.0.5:7000");
        let ctrl = f
            .follow(&p, &a, Some(&RemoteLink::Connecting), WallNs(100))
            .unwrap();
        assert_eq!(ctrl, "tcp://10.0.0.9:7000");
        p.ctrl = ctrl;
        // Still connecting, or an unreachable report the previous link sent: it stays.
        for link in [
            None,
            Some(RemoteLink::Connecting),
            Some(RemoteLink::Unreachable { since: WallNs(50) }),
        ] {
            assert_eq!(f.follow(&p, &a, link.as_ref(), WallNs(101)), None);
        }
        // Unreachable there: the next advert (or link report) moves it on.
        let down = RemoteLink::Unreachable { since: WallNs(103) };
        let ctrl = f.follow(&p, &a, Some(&down), WallNs(104)).unwrap();
        assert_eq!(ctrl, "tcp://10.1.0.9:7000");
        p.ctrl = ctrl;
        assert_eq!(f.gave_up, vec![ip("10.0.0.9")]);
        // Both failed: it stays where it is, its link retrying, until an advert names another.
        let down = RemoteLink::Unreachable { since: WallNs(110) };
        assert_eq!(f.follow(&p, &a, Some(&down), WallNs(111)), None);
        let moved = advert(&k, "ketunkolo.local", &["10.2.0.9"], 7000);
        assert!(f.follow(&p, &moved, Some(&down), WallNs(112)).is_some());
    }

    #[test]
    fn the_operators_address_is_kept_while_the_rig_is_off() {
        let k = "k".repeat(40);
        let p = peer("tcp://10.0.0.5:7000");
        let a = advert(&k, "ketunkolo.local", &["10.0.0.5", "10.1.0.5"], 7000);
        let down = RemoteLink::Unreachable { since: WallNs(5) };
        assert_eq!(
            Followed::default().follow(&p, &a, Some(&down), WallNs(9)),
            None
        );
    }
}
