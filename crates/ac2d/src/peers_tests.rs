//! The data ROUTER's peers over real sockets (TCP on loopback): a peer that vanished is
//! dropped while the others are served and its reconnect is a new peer; events past the
//! bound bring `resync_required`; a full window of the smallest frames never meets a full
//! send queue.
#![allow(clippy::unwrap_used)]

use super::*;

use std::sync::Arc;

use ac2_proto::frame::Frame;
use ac2_proto::{
    CodedFrame, DataRequest, WireFrame, decode_notice, decode_wire, encode_data_request,
};
use ac2_zmq::{Context, SocketType};

use crate::io::Interest;

const WAIT: Duration = Duration::from_secs(2);

/// A data ROUTER with its peers, and one topic whose frames differ only in `seq` (the
/// smallest deltas a topic makes).
struct Rig {
    ctx: Context,
    router: Socket,
    endpoint: String,
    peers: Peers,
    interest: Interest,
    slots: HashMap<Topic, Slot>,
    order: Vec<Topic>,
    put: u64,
    frame: Frame,
    topic: Topic,
}

fn up(sock: &Socket, msg: DataUp) {
    sock.send(&[encode_data_request(&DataRequest::new(msg)).unwrap()])
        .unwrap();
}

fn part(b: &[u8]) -> Part {
    Part::copy_from(b).unwrap()
}

impl Rig {
    fn new() -> Self {
        let ctx = Context::new().unwrap();
        let router = ctx.socket(SocketType::Router).unwrap();
        router.set_router_mandatory(true).unwrap();
        router.set_send_hwm(DATA_SNDHWM).unwrap();
        router.bind("tcp://127.0.0.1:*").unwrap();
        let endpoint = router.last_endpoint().unwrap();
        let frame = ac2_proto::samples::tf_frame();
        let topic = CodedFrame::from_frame(&frame).unwrap().topic;
        Self {
            ctx,
            router,
            endpoint,
            peers: Peers::default(),
            interest: Interest::default(),
            slots: HashMap::new(),
            order: vec![topic],
            put: 0,
            frame,
            topic,
        }
    }

    /// A DEALER stating `window` and subscribing to the topic and `evt`, once the daemon
    /// has its requests.
    fn dealer(&mut self, window: u32) -> Socket {
        let d = self.ctx.socket(SocketType::Dealer).unwrap();
        d.set_linger(Some(Duration::ZERO)).unwrap();
        d.connect(&self.endpoint).unwrap();
        up(&d, DataUp::Window { bytes: window });
        for sub in [
            Subscription::Topic(self.topic),
            Subscription::Topic(Topic::Evt),
        ] {
            up(&d, DataUp::Subscribe { sub });
        }
        let n = self.peers.len();
        self.pump_until(|r| r.peers.len() > n && r.interest.count(b"evt") > n);
        d
    }

    /// Takes the peers' requests.
    fn pump(&mut self) {
        while let Some(m) = self.router.recv_timeout(Duration::from_millis(5)).unwrap() {
            self.peers.request(
                &self.router,
                m.into_frames(),
                &self.interest,
                Instant::now(),
            );
        }
    }

    fn pump_until(&mut self, done: impl Fn(&Self) -> bool) {
        let end = Instant::now() + WAIT;
        while !done(self) {
            assert!(Instant::now() < end, "requests never arrived");
            self.pump();
        }
    }

    /// The topic's next frame into its slot.
    fn put(&mut self) {
        self.frame.stamp.seq += 1;
        self.put += 1;
        let slot = Slot {
            frame: Arc::new(CodedFrame::from_frame(&self.frame).unwrap()),
            put: self.put,
        };
        self.slots.insert(self.topic, slot);
    }

    fn serve(&mut self) {
        self.peers.serve(
            &self.router,
            &self.slots,
            &self.order,
            Instant::now(),
            Duration::ZERO,
            &self.interest,
        );
    }

    fn outside(&mut self, topic: &[u8], kind: Outside) {
        self.peers.outside(
            &self.router,
            &[part(topic), part(b"x")],
            kind,
            &self.interest,
        );
    }

    fn peer(&self, i: usize) -> &Peer {
        let mut v: Vec<&Peer> = self.peers.map.values().collect();
        v.sort_by_key(|p| p.heard);
        v[i]
    }
}

/// The next message on `d`, its parts.
fn recv(d: &Socket) -> Vec<Vec<u8>> {
    d.recv_timeout(WAIT)
        .unwrap()
        .expect("a message")
        .into_frames()
}

/// The next frame on `d`, credited.
fn frame(d: &Socket) -> WireFrame {
    let m = recv(d);
    let bytes: usize = m.iter().map(Vec::len).sum();
    up(
        d,
        DataUp::Credit {
            bytes: frame_charge(bytes),
            events: 0,
        },
    );
    let parts: Vec<&[u8]> = m.iter().map(Vec::as_slice).collect();
    decode_wire(&parts).unwrap()
}

/// A peer whose connection closed is dropped at the first send that fails `EHOSTUNREACH`,
/// subscriptions and all, while the other is served throughout; a new connection is a new
/// peer, whose subscription brings a keyframe.
#[test]
fn a_vanished_peer_is_dropped_and_its_reconnect_is_a_new_peer() {
    let mut r = Rig::new();
    let a = r.dealer(DATA_WINDOW_DEFAULT);
    let b = r.dealer(DATA_WINDOW_DEFAULT);
    let tf = r.topic.to_bytes();
    assert_eq!(r.interest.count(&tf), 2);
    r.put();
    r.serve();
    assert!(matches!(frame(&a), WireFrame::Key(_)));
    assert!(matches!(frame(&b), WireFrame::Key(_)));

    drop(b);
    // libzmq sees the connection close on its own I/O thread; until then sends to it queue.
    let end = Instant::now() + WAIT;
    while r.peers.len() == 2 {
        assert!(Instant::now() < end, "the closed peer was never dropped");
        r.put();
        r.serve();
        let f = frame(&a);
        assert_eq!(
            f.seq(),
            r.frame.stamp.seq,
            "the live peer is served each frame"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(r.interest.count(&tf), 1);
    assert_eq!(r.interest.count(b"evt"), 1);
    assert_eq!(r.interest.count(&Topic::Ka.to_bytes()), 1);

    let c = r.dealer(DATA_WINDOW_DEFAULT);
    assert_eq!(r.peers.len(), 2);
    r.put();
    r.serve();
    let seq = r.frame.stamp.seq;
    let fc = frame(&c);
    assert!(
        matches!(fc, WireFrame::Key(_)),
        "the reconnect starts on a keyframe"
    );
    assert_eq!(fc.seq(), seq);
    assert_eq!(frame(&a).seq(), seq);
}

/// A peer that acknowledges nothing receives [`EVENT_BOUND`] events; the rest are dropped,
/// and once it credits what it read the first thing it is sent is `resync_required`.
#[test]
fn events_past_the_bound_bring_resync_required() {
    let mut r = Rig::new();
    let a = r.dealer(DATA_WINDOW_DEFAULT);
    for _ in 0..EVENT_BOUND + 40 {
        r.outside(b"evt", Outside::Event);
    }
    assert!(r.peer(0).resync_owed);
    let mut events = 0;
    while let Some(m) = a.recv_timeout(Duration::from_millis(100)).unwrap() {
        assert_eq!(m.frames()[0], b"evt");
        events += 1;
    }
    assert_eq!(events, EVENT_BOUND);
    // A keepalive past the bound is dropped without owing anything.
    r.outside(b"ka", Outside::Ka);
    assert!(a.recv_timeout(Duration::from_millis(50)).unwrap().is_none());

    up(&a, DataUp::Credit { bytes: 0, events });
    r.pump_until(|r| !r.peer(0).resync_owed);
    let m = recv(&a);
    assert_eq!(m[0], FLOW_TOPIC_BYTES);
    assert_eq!(decode_notice(&m[1]).unwrap(), FlowNotice::ResyncRequired);
    r.outside(b"evt", Outside::Event);
    assert_eq!(recv(&a)[0], b"evt");
}

const FLOW_TOPIC_BYTES: &[u8] = ac2_proto::flow::FLOW_TOPIC;

/// A peer with the largest window that reads nothing: the daemon fills the window with
/// the smallest deltas and the event bound with events, and every one of them is queued:
/// credit stops the sends, never a full queue.
#[test]
fn a_full_window_of_the_smallest_frames_never_meets_a_full_queue() {
    let mut r = Rig::new();
    let a = r.dealer(DATA_WINDOW_MAX);
    let mut sent = 0u64;
    loop {
        r.put();
        let before = r.peer(0).in_flight;
        r.serve();
        let p = r.peer(0);
        if p.blocked {
            // Held back by credit: staged for the next one, not refused by the socket.
            assert!(p.staged.is_some(), "a frame refused below the window");
            let (_, _, e) = p.staged.as_ref().unwrap();
            assert!(p.in_flight + u64::from(frame_charge(e.len)) > u64::from(DATA_WINDOW_MAX));
            break;
        }
        assert!(p.in_flight > before, "frame {} not sent", sent + 1);
        sent += 1;
        assert!(sent < 100_000);
    }
    for _ in 0..EVENT_BOUND {
        r.outside(b"evt", Outside::Event);
    }
    let p = r.peer(0);
    assert_eq!(p.events_out, EVENT_BOUND);
    assert!(!p.resync_owed, "an event refused below the bound");
    // Everything queued arrives, the smallest frames among them.
    let (mut frames, mut events, mut largest) = (0u64, 0u32, 0usize);
    while let Some(m) = a.recv_timeout(Duration::from_millis(200)).unwrap() {
        if m.frames()[0] == b"evt" {
            events += 1;
        } else {
            frames += 1;
            largest = largest.max(m.frames().iter().map(Vec::len).sum());
        }
    }
    assert_eq!((frames, events), (sent, EVENT_BOUND));
    assert!(sent > 1_000, "{sent} frames of up to {largest} B");
}

/// libzmq's writer counts what the connection took from the queue only every half
/// queue; twice the outstanding bound keeps a credited message off a full queue.
#[test]
fn send_queue_holds_twice_what_credit_lets_out() {
    let outstanding = DATA_WINDOW_MAX / FRAME_CHARGE_MIN + 1 + EVENT_BOUND;
    let lwm = DATA_SNDHWM.div_ceil(2);
    assert!(outstanding + lwm - 1 < DATA_SNDHWM);
}
