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
    attach: mpsc::Sender<Attach>,
    control: mpsc::Receiver<ControlMsg>,
    server: KeyPair,
    client: KeyPair,
    authorized: AuthorizedHandle,
    audit: Arc<AuthLog>,
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
        let (attach, attach_rx) = mpsc::channel();
        let (to_control, control) = mpsc::channel();
        let io = spawn(
            IoSockets {
                router,
                data,
                pull,
                frames: frames_rx,
                attach: attach_rx,
            },
            to_control,
            Arc::new(Interest::default()),
            10,
        )
        .unwrap();
        let client = KeyPair::generate().unwrap();
        let mut keys = AuthorizedKeys::new();
        keys.insert("app", client.public).unwrap();
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
            _frames: frames,
            _io: io,
            _dir: dir,
        }
    }

    /// Binds a network pair on loopback and attaches it as `epoch`; its ctrl endpoint.
    fn attach(&self, epoch: u64) -> String {
        let pair = bind_pair(
            Host::Addr(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            NetPort::Any,
            &self.server,
            &self.authorized,
            Arc::clone(&self.audit),
        )
        .unwrap();
        let ctrl = pair.ctrl_ep.clone();
        let epoch = NetEpoch(epoch);
        self.attach.send(Attach { epoch, pair }).unwrap();
        self.outbox.attach(epoch);
        ctrl
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
    rig.outbox.detach(NetEpoch(1));
    let second = rig.attach(2);
    let new = rig.dealer(b"c", true, &second);
    let (fresh, _) = rig.request(&new, b"asked on 2");
    assert_eq!(stale.id, fresh.id);
    rig.outbox.reply(&stale, b"meant for 1");
    rig.outbox.reply(&fresh, b"meant for 2");
    assert_eq!(reply_of(&new, WAIT).as_deref(), Some(&b"meant for 2"[..]));
    assert_eq!(reply_of(&new, Duration::from_millis(200)), None);
}
