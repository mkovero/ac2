//! Delta chains over the data socket: a peer that loses frames never holds a frame built on
//! the wrong base, and gets back in step within a keyframe interval (or a round trip after it
//! asks); a late joiner draws from one keyframe; requests within the gap cost one keyframe.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakePath, Pace};
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_proto::units::MeasId;
use ac2_proto::{CodedFrame, Command, DecodeError, Stream, Topic, WireFrame, decode_wire};
use ac2_zmq::Socket;
use ac2d::{Daemon, DaemonConfig, Handle};
use common::*;

const TF: Topic = Topic::Data {
    meas: MeasId(1),
    stream: Stream::Tf,
};
/// Longest a topic goes without a keyframe (`ac2d`'s `KEY_INTERVAL`).
const KEY_INTERVAL: Duration = Duration::from_millis(500);
/// Slack for a machine busy with other work.
const SLACK: Duration = Duration::from_millis(400);

/// Input noise on every channel: the transfer function changes with every frame.
fn noisy_rig() -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 2,
        outputs: 2,
        drive: FakeDrive::Thread(Pace::Realtime),
        seed: 11,
        input_noise_rms: 1e-2,
        paths: vec![FakePath::loopback(0, 0, LOOP_DELAY)],
        ..FakeConfig::default()
    })
    .unwrap()
}

fn start(name: &str) -> (Handle, Client) {
    init_log();
    let mut config = DaemonConfig::new(Arc::new(noisy_rig()), inproc(name), -10.0);
    config.publish_fps = 60;
    let h = Daemon::start(config).unwrap();
    let mut c = Client::connect(h.context(), h.ctrl_endpoint());
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    c.ok(Command::MeasCreate {
        config: transfer("tf"),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    (h, c)
}

fn sub(h: &Handle) -> Socket {
    Sub::connect(h.context(), h.data_endpoint(), &[b"d/1/tf"]).sock
}

/// What one received message was.
enum Got {
    Key(CodedFrame),
    /// Applied on the frame held.
    Delta(CodedFrame),
    /// Not on the frame held (or none held).
    Refused,
}

/// A receiver of one topic, applying in order as the client does.
#[derive(Default)]
struct Peer {
    held: Option<CodedFrame>,
}

impl Peer {
    fn take(&mut self, parts: &[&[u8]]) -> Got {
        let w = decode_wire(parts).unwrap();
        assert_eq!(w.topic(), TF);
        let key = matches!(w, WireFrame::Key(_));
        match w.apply(self.held.as_ref()) {
            Ok(c) => {
                self.held = Some(c.clone());
                if key { Got::Key(c) } else { Got::Delta(c) }
            }
            Err(DecodeError::NoBase | DecodeError::WrongBase { .. }) => Got::Refused,
            Err(e) => panic!("{e}"),
        }
    }
}

/// Frames equal bit for bit (metadata by their bytes: NaN-safe).
fn same(a: &CodedFrame, b: &CodedFrame) {
    let seq = a.stamp.seq;
    assert_eq!(a.stamp, b.stamp, "seq {seq}");
    assert_eq!(a.codes, b.codes, "seq {seq}");
    assert_eq!(a.arrays, b.arrays, "seq {seq}");
    assert_eq!(
        rmp_serde::to_vec(&a.meta).unwrap(),
        rmp_serde::to_vec(&b.meta).unwrap(),
        "seq {seq}"
    );
}

fn recv(s: &Socket, timeout: Duration) -> Option<Vec<Vec<u8>>> {
    s.recv_timeout(timeout)
        .unwrap()
        .map(|m| m.frames().to_vec())
}

fn refs(p: &[Vec<u8>]) -> Vec<&[u8]> {
    p.iter().map(Vec::as_slice).collect()
}

/// A lossless peer and a peer that drops runs of 1, 3 and 30 messages and then every 7th:
/// every frame the lossy peer holds is bit for bit the lossless peer's frame of that `seq`,
/// and after each loss it holds a frame again within a keyframe interval, or a round trip
/// after it asked for a keyframe.
#[test]
fn slow_a_lossy_peer_holds_only_frames_of_the_lossless_one() {
    let (h, mut c) = start("chain-lossy");
    let (good, lossy) = (sub(&h), sub(&h));
    let (mut g, mut l) = (Peer::default(), Peer::default());
    let mut lossless: BTreeMap<u64, CodedFrame> = BTreeMap::new();
    let mut checks: Vec<(u64, CodedFrame)> = Vec::new();
    // Message index → messages dropped from it on; after 130, every 7th.
    let runs = [(20usize, 1usize), (45, 3), (70, 30)];
    let dropped = |i: usize| {
        runs.iter().any(|&(at, n)| (at..at + n).contains(&i)) || (i >= 130 && i.is_multiple_of(7))
    };
    let (mut i, mut asks, mut keys) = (0usize, 0u32, 0u32);
    let mut broke: Option<Instant> = None;
    let mut asked_at: Option<Instant> = None;
    let mut resyncs: Vec<Duration> = Vec::new();
    let t0 = Instant::now();
    while i < 220 && t0.elapsed() < Duration::from_secs(60) {
        // The lossless peer reads everything first, so it holds every `seq` the lossy one
        // can.
        while let Some(m) = recv(&good, Duration::ZERO) {
            let first = g.held.is_none();
            match g.take(&refs(&m)) {
                Got::Key(c) | Got::Delta(c) => {
                    lossless.insert(c.stamp.seq, c);
                }
                // A new subscription may see deltas before its keyframe.
                Got::Refused if first => {}
                Got::Refused => panic!("the lossless peer lost its chain"),
            }
        }
        let Some(m) = recv(&lossy, Duration::from_millis(50)) else {
            continue;
        };
        i += 1;
        if dropped(i) {
            continue;
        }
        let got = l.take(&refs(&m));
        if let Got::Key(_) = got {
            keys += 1;
        }
        match got {
            Got::Key(x) | Got::Delta(x) => {
                if let Some(at) = broke.take() {
                    resyncs.push(at.elapsed());
                }
                checks.push((x.stamp.seq, x));
            }
            Got::Refused => {
                broke.get_or_insert_with(Instant::now);
                // As the client: ask, at most once per 250 ms.
                if asked_at.is_none_or(|t| t.elapsed() >= Duration::from_millis(250)) {
                    asked_at = Some(Instant::now());
                    asks += 1;
                    c.ok(Command::DataKeyframe { topic: TF });
                }
            }
        }
    }
    assert!(i >= 220, "only {i} messages in a minute");
    // The lossless peer up to the last frame the lossy one held.
    let last = checks.last().map_or(0, |(s, _)| *s);
    while lossless.last_key_value().is_none_or(|(s, _)| *s < last) {
        let m = recv(&good, Duration::from_secs(5)).expect("frames");
        if let Got::Key(c) | Got::Delta(c) = g.take(&refs(&m)) {
            lossless.insert(c.stamp.seq, c);
        }
    }
    for (seq, x) in &checks {
        let want = lossless
            .get(seq)
            .unwrap_or_else(|| panic!("seq {seq} not seen"));
        same(x, want);
    }
    assert!(asks >= 4, "{asks} keyframe requests");
    assert!(keys > 0);
    // Wall-clock bounds on a machine that may be busy with other work: a descheduled daemon
    // misses them without anything being wrong, so the typical loss must meet the bound and
    // none may take several keyframe intervals (a chain that never resyncs).
    assert!(!resyncs.is_empty(), "never back in step");
    resyncs.sort();
    let typical = resyncs[resyncs.len() / 2];
    let worst = resyncs[resyncs.len() - 1];
    assert!(
        typical < KEY_INTERVAL + SLACK,
        "back in step after {typical:?} typically: {resyncs:?}"
    );
    assert!(worst < 4 * KEY_INTERVAL, "back in step after {worst:?}");
    h.shutdown();
}

/// A peer that joins a running topic draws from the keyframe its subscription brings, at
/// once; the peer already there receives that keyframe too (one or two in all, with a
/// periodic one falling in the same window), and its chain goes on unbroken.
#[test]
fn a_late_joiner_draws_from_one_keyframe() {
    let (h, _c) = start("chain-late");
    let early = sub(&h);
    let mut e = Peer::default();
    // In step: past the first keyframe and a few deltas.
    let mut deltas = 0;
    while deltas < 5 {
        let m = recv(&early, Duration::from_secs(5)).expect("frames");
        if let Got::Delta(_) = e.take(&refs(&m)) {
            deltas += 1;
        }
    }
    // Let the next periodic keyframe pass, so the window below catches at most one.
    let since_key = loop {
        let m = recv(&early, Duration::from_secs(5)).expect("frames");
        match e.take(&refs(&m)) {
            Got::Key(_) => break Instant::now(),
            Got::Delta(_) => {}
            Got::Refused => panic!("the early peer lost its chain"),
        }
    };
    let t0 = Instant::now();
    let late = sub(&h);
    let mut lt = Peer::default();
    let first = loop {
        let m = recv(&late, Duration::from_secs(5)).expect("the subscription's keyframe");
        match lt.take(&refs(&m)) {
            Got::Key(k) => break k,
            // libzmq may let deltas through before the subscription's keyframe: no base.
            Got::Delta(_) => panic!("applied without a base"),
            Got::Refused => {}
        }
    };
    let took = t0.elapsed();
    assert!(
        took < Duration::from_millis(200) + SLACK,
        "drew after {took:?}"
    );
    // The early peer: every message up to and just past the late one's keyframe.
    let mut early_keys = Vec::new();
    let end = Instant::now() + Duration::from_millis(100);
    while let Some(m) = recv(&early, end.saturating_duration_since(Instant::now())) {
        match e.take(&refs(&m)) {
            Got::Key(k) => early_keys.push(k.stamp.seq),
            Got::Delta(_) => {}
            Got::Refused => panic!("the early peer lost its chain"),
        }
        if e.held
            .as_ref()
            .is_some_and(|f| f.stamp.seq > first.stamp.seq)
        {
            break;
        }
    }
    assert!(
        early_keys.contains(&first.stamp.seq),
        "the late peer's keyframe {} reached the early one: {early_keys:?}",
        first.stamp.seq
    );
    let window = since_key.elapsed();
    // The subscription's keyframe, and a periodic one if the window reached it (two if it
    // reached past a second interval on a stalled machine).
    let periodic = if window >= KEY_INTERVAL { 2 } else { 1 };
    let most = 1 + periodic;
    assert!(
        (1..=most).contains(&early_keys.len()),
        "{early_keys:?} in {window:?}"
    );
    h.shutdown();
}

/// Two `data.keyframe` within the request gap cost one keyframe.
#[test]
fn requests_within_the_gap_cost_one_keyframe() {
    let (h, mut c) = start("chain-gap");
    let s = sub(&h);
    let mut p = Peer::default();
    // Past the first second, where the first, small, transfer functions make a delta larger
    // than their keyframe; then just after a periodic keyframe, so the next periodic one is
    // a keyframe interval away.
    let t0 = Instant::now();
    loop {
        let m = recv(&s, Duration::from_secs(5)).expect("frames");
        if let Got::Key(_) = p.take(&refs(&m))
            && t0.elapsed() > Duration::from_millis(1500)
        {
            break;
        }
    }
    let asked = Instant::now();
    c.ok(Command::DataKeyframe { topic: TF });
    c.ok(Command::DataKeyframe { topic: TF });
    let mut keys = 0;
    let window = Duration::from_millis(400);
    while let Some(m) = recv(&s, window.saturating_sub(asked.elapsed())) {
        if let Got::Key(_) = p.take(&refs(&m)) {
            keys += 1;
        }
    }
    if asked.elapsed() < KEY_INTERVAL {
        assert_eq!(keys, 1, "keyframes within {window:?} of two requests");
    }
    h.shutdown();
}
