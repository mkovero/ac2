#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::mpsc;

use ac2_zmq::{AuthorizedHandle, AuthorizedKeys, Context, CurveClient, KeyPair, SocketType};

use super::*;
use crate::authlog::AuthLog;
use crate::network::{Host, NetPort, bind_pair};
use crate::outbox::{self, Outbox};
use crate::route::{Origin, Route};

const WAIT: Duration = Duration::from_secs(5);

struct Rig {
    ctx: Context,
    local_ctrl: String,
    outbox: Outbox,
    attach: AttachQueue,
    control: mpsc::Receiver<ControlMsg>,
    server: KeyPair,
    client: KeyPair,
    authorized: AuthorizedHandle,
    audit: Arc<AuthLog>,
    interest: Arc<Interest>,
    _frames: outbox::FrameTx,
    _io: JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Rig {
    fn start(name: &str) -> Self {
        let ctx = Context::new().unwrap();
        let router = ctx.socket(SocketType::Router).unwrap();
        let data = ctx.socket(SocketType::Router).unwrap();
        crate::ctrl_socket_options(&router).unwrap();
        crate::data_socket_options(&data, false).unwrap();
        let local_ctrl = format!("inproc://io-{name}/ctrl");
        router.bind(&local_ctrl).unwrap();
        data.bind(&format!("inproc://io-{name}/data")).unwrap();
        let pull_ep = format!("inproc://io-{name}/out");
        let pull = ctx.socket(SocketType::Pull).unwrap();
        pull.bind(&pull_ep).unwrap();
        let outbox = Outbox::connect(&ctx, &pull_ep, 1000).unwrap();
        let (frames, frames_rx) = outbox::frame_channel();
        let attach = AttachQueue::default();
        let (to_control, control) = mpsc::channel();
        let interest = Arc::new(Interest::default());
        let io = spawn(
            IoSockets {
                router,
                data,
                pull,
                frames: frames_rx,
                attach: attach.clone(),
            },
            to_control,
            Arc::clone(&interest),
            10,
        )
        .unwrap();
        let client = KeyPair::generate().unwrap();
        let mut keys = AuthorizedKeys::new();
        keys.insert("app", client.public, ac2_zmq::Role::Operator)
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        Self {
            ctx,
            local_ctrl,
            outbox,
            attach,
            control,
            server: KeyPair::generate().unwrap(),
            client,
            authorized: AuthorizedHandle::new(keys),
            audit: Arc::new(AuthLog::new(&dir.path().join("authorized_clients"))),
            interest,
            _frames: frames,
            _io: io,
            _dir: dir,
        }
    }

    /// Binds a network pair on loopback and attaches it as `epoch`; its ctrl endpoint.
    fn attach(&self, epoch: u64) -> String {
        self.attach_pair(epoch).0
    }

    /// [`Self::attach`]; its ctrl and data endpoints.
    fn attach_pair(&self, epoch: u64) -> (String, String) {
        let pair = bind_pair(
            Host::Addr(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            NetPort::Any,
            &self.server,
            &self.authorized,
            Arc::clone(&self.audit),
        )
        .unwrap();
        let eps = (pair.ctrl_ep.clone(), pair.data_ep.clone());
        let epoch = NetEpoch(epoch);
        self.attach.put(Attach { epoch, pair });
        assert!(self.outbox.attach(epoch));
        eps
    }

    fn dealer(&self, rid: &[u8], curve: bool, ep: &str) -> Socket {
        let s = self.ctx.socket(SocketType::Dealer).unwrap();
        s.set_routing_id(rid).unwrap();
        if curve {
            s.set_curve_client(&CurveClient {
                keys: self.client.clone(),
                server_key: self.server.public,
            })
            .unwrap();
        }
        s.connect(ep).unwrap();
        s
    }

    /// Sends `body` from `s` and returns the route control would answer on.
    fn request(&self, s: &Socket, body: &[u8]) -> (Route, crate::route::Peer) {
        s.send(&[body]).unwrap();
        loop {
            match self
                .control
                .recv_timeout(WAIT)
                .expect("no request reached control")
            {
                ControlMsg::Request {
                    route,
                    peer,
                    payload,
                } if payload == body => return (route, peer),
                _ => {}
            }
        }
    }
}

fn reply_of(s: &Socket, wait: Duration) -> Option<Vec<u8>> {
    s.recv_timeout(wait)
        .unwrap()
        .map(|m| m.frames().last().unwrap().clone())
}

#[test]
fn equal_routing_ids_on_both_pairs_reach_their_own_client() {
    let rig = Rig::start("equal");
    let net = rig.attach(1);
    let local = rig.dealer(b"same", false, &rig.local_ctrl);
    let remote = rig.dealer(b"same", true, &net);
    let (lr, lp) = rig.request(&local, b"from local");
    let (nr, np) = rig.request(&remote, b"from network");
    assert_eq!(lr.origin, Origin::Local);
    assert_eq!(nr.origin, Origin::Network(NetEpoch(1)));
    assert_eq!(lr.id, nr.id);
    assert!(lp.is_local());
    assert_eq!(np.curve().map(|(n, _)| n), Some("app"));
    rig.outbox.reply(&nr, b"to network");
    rig.outbox.reply(&lr, b"to local");
    assert_eq!(reply_of(&local, WAIT).as_deref(), Some(&b"to local"[..]));
    assert_eq!(reply_of(&remote, WAIT).as_deref(), Some(&b"to network"[..]));
}

#[test]
fn a_reply_for_an_older_pair_is_dropped_though_its_routing_id_is_reused() {
    let rig = Rig::start("stale");
    let first = rig.attach(1);
    let old = rig.dealer(b"c", true, &first);
    let (stale, _) = rig.request(&old, b"asked on 1");
    drop(old);
    assert!(rig.outbox.detach(NetEpoch(1), false));
    let second = rig.attach(2);
    let new = rig.dealer(b"c", true, &second);
    let (fresh, _) = rig.request(&new, b"asked on 2");
    assert_eq!(stale.id, fresh.id);
    rig.outbox.reply(&stale, b"meant for 1");
    rig.outbox.reply(&fresh, b"meant for 2");
    assert_eq!(reply_of(&new, WAIT).as_deref(), Some(&b"meant for 2"[..]));
    assert_eq!(reply_of(&new, Duration::from_millis(200)), None);
}

#[test]
fn an_attach_without_its_pair_is_reported_back_to_control() {
    let rig = Rig::start("unqueued");
    assert!(rig.outbox.attach(NetEpoch(5)));
    loop {
        if let ControlMsg::NetFailed { epoch, reason } =
            rig.control.recv_timeout(WAIT).expect("control never heard")
        {
            assert_eq!(epoch, NetEpoch(5));
            assert_eq!(reason, NetReason::NotHandedOver);
            break;
        }
    }
}

/// Turned off, the network's data peers hear `network_off` before the pair closes, and
/// what they subscribed no longer counts as wanted.
#[test]
fn turned_off_the_network_peers_are_told_and_their_interest_dropped() {
    use ac2_proto::flow::FlowNotice;
    use ac2_proto::{DataRequest, DataUp, Subscription, Topic, decode_notice, encode_data_request};
    let rig = Rig::start("off");
    let (_, data) = rig.attach_pair(1);
    let d = rig.dealer(b"d", true, &data);
    for msg in [
        DataUp::Window { bytes: 1 << 20 },
        DataUp::Subscribe {
            sub: Subscription::Topic(Topic::Evt),
        },
    ] {
        d.send(&[encode_data_request(&DataRequest::new(msg)).unwrap()])
            .unwrap();
    }
    let t0 = std::time::Instant::now();
    while rig.interest.count(b"evt") == 0 {
        assert!(t0.elapsed() < WAIT, "the subscription never counted");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(rig.outbox.detach(NetEpoch(1), true));
    let mut told = false;
    while let Some(m) = d.recv_timeout(WAIT).unwrap() {
        let f = m.frames();
        if f.len() == 2 && decode_notice(&f[1]).ok() == Some(FlowNotice::NetworkOff) {
            told = true;
            break;
        }
    }
    assert!(told, "no network_off before the pair closed");
    let t0 = std::time::Instant::now();
    while rig.interest.count(b"evt") != 0 {
        assert!(t0.elapsed() < WAIT, "the detached peer is still counted");
        std::thread::sleep(Duration::from_millis(5));
    }
}
