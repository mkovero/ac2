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
use ac2_proto::model::RigPeer;

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

/// Where `peer` answers now by `advert`: its ctrl and data endpoints, when the advert
/// carries the key the app pinned for `peer` and not the endpoint `peer` was paired at.
/// `None` for another rig's advert, one that resolved no address, or one still naming the
/// paired endpoint (a rig on several networks lists all its addresses). A rig still at its
/// paired host but on another port keeps the host as paired (a name stays a name).
pub(crate) fn followed(peer: &RigPeer, advert: &Rig) -> Option<(String, String)> {
    if advert.advert.server_key != peer.key || advert.addresses.is_empty() {
        return None;
    }
    let (paired, port) = host_port(&peer.ctrl)?;
    let host_there = paired.eq_ignore_ascii_case(&advert.host)
        || paired
            .parse::<IpAddr>()
            .is_ok_and(|ip| advert.addresses.contains(&ip));
    if host_there && port == advert.port {
        return None;
    }
    let host = if host_there {
        paired.to_owned()
    } else {
        advert.connect_host()
    };
    let data = advert.port.checked_add(1)?;
    Some((endpoint(&host, advert.port), endpoint(&host, data)))
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
            data: ctrl.replace("7000", "7001"),
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
            followed(&p, &advert(&k, "ketunkolo.local", &["10.0.0.9"], 7000)),
            Some(("tcp://10.0.0.9:7000".into(), "tcp://10.0.0.9:7001".into()))
        );
        // Its ports come from the advert too: the daemon may have been started on others.
        assert_eq!(
            followed(&p, &advert(&k, "ketunkolo.local", &["fd00::9"], 7100)),
            Some(("tcp://[fd00::9]:7100".into(), "tcp://[fd00::9]:7101".into()))
        );
    }

    #[test]
    fn a_rig_restarted_on_another_port_is_followed_at_its_host() {
        let k = "k".repeat(40);
        let a = advert(&k, "ketunkolo.local", &["10.0.0.5"], 7100);
        assert_eq!(
            followed(&peer("tcp://ketunkolo.local:7000"), &a),
            Some((
                "tcp://ketunkolo.local:7100".into(),
                "tcp://ketunkolo.local:7101".into()
            ))
        );
        assert_eq!(
            followed(&peer("tcp://10.0.0.5:7000"), &a),
            Some(("tcp://10.0.0.5:7100".into(), "tcp://10.0.0.5:7101".into()))
        );
    }

    #[test]
    fn only_the_pinned_key_moves_a_peer() {
        let p = peer("tcp://10.0.0.5:7000");
        // Another rig, whatever it calls itself.
        let other = advert(&"o".repeat(40), "ketunkolo.local", &["10.0.0.9"], 7000);
        assert_eq!(followed(&p, &other), None);
    }

    #[test]
    fn a_rig_still_at_its_paired_address_stays() {
        let k = "k".repeat(40);
        // One of several addresses, or the host name it was paired by.
        let a = advert(&k, "ketunkolo.local", &["10.0.0.5", "192.168.1.4"], 7000);
        assert_eq!(followed(&peer("tcp://10.0.0.5:7000"), &a), None);
        assert_eq!(followed(&peer("tcp://ketunkolo.local:7000"), &a), None);
        assert_eq!(
            followed(
                &peer("tcp://[fe80::1]:7000"),
                &advert(&k, "h", &["fe80::1"], 7000)
            ),
            None
        );
        // An advert that resolved no address says nothing about where the rig is.
        assert_eq!(
            followed(&peer("tcp://10.0.0.5:7000"), &advert(&k, "h", &[], 7000)),
            None
        );
    }
}
