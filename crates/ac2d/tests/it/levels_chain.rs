//! The meter topics as delta chains: with their readings in arrays, `stream/levels` and an
//! admitted rig's `source/levels` go mostly as deltas; a peer that loses frames and never
//! asks holds a usable frame again within the keyframe interval and never one built on the
//! wrong base; a late subscriber draws within it; a delta carries the timing mark.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, Pace};
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_proto::frame::FrameMeta;
use ac2_proto::model::{InputKind, InputSetup, Pair, StreamConfig};
use ac2_proto::{CodedFrame, Command, DecodeError, Topic, WireFrame, decode_wire};
use ac2_zmq::{AuthorizedKeys, CurveClient, KeyPair, Role};
use ac2d::{Daemon, Handle, NetworkSecurity};
use common::*;

/// Longest a topic goes without a keyframe (`ac2d`'s `KEY_INTERVAL`).
const KEY_INTERVAL: Duration = Duration::from_millis(500);
/// Slack for a machine busy with other work.
const SLACK: Duration = Duration::from_millis(400);

/// Eight inputs, each with its own noise: every meter, correlation and loudness reading
/// moves from frame to frame.
fn noisy_rig() -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 8,
        outputs: 2,
        drive: FakeDrive::Thread(Pace::Realtime),
        seed: 11,
        input_noise_rms: 1e-2,
        ..FakeConfig::default()
    })
    .unwrap()
}

/// A networked daemon metering eight line inputs in three pairs (five loudness sources),
/// and the CURVE client of a rig it admits.
fn start() -> (Handle, Client, CurveClient, tempfile::TempDir) {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let rig_keys = KeyPair::generate().unwrap();
    let mut keys = AuthorizedKeys::new();
    keys.insert("rig-b", rig_keys.public, Role::Rig(2)).unwrap();
    let security = NetworkSecurity {
        server_key_file: dir.path().join("server.key"),
        authorized_clients_file: dir.path().join("authorized_clients"),
    };
    keys.save(&security.authorized_clients_file).unwrap();
    let h = Daemon::start(net_config(noisy_rig(), security)).unwrap();
    let mut c = Client::connect(h.context(), h.ctrl_endpoint());
    c.ok(Command::StreamOpen {
        config: StreamConfig {
            input_channels: (0..8).collect(),
            ..stream(false)
        },
    });
    let pair = |left: u16| Pair {
        left,
        right: left + 1,
        name: None,
    };
    c.ok(Command::StreamInputs {
        inputs: (0..8)
            .map(|channel| InputSetup {
                channel,
                kind: InputKind::Line,
                pair: None,
            })
            .collect(),
        pairs: vec![pair(0), pair(2), pair(4)],
    });
    let rig = CurveClient {
        keys: rig_keys,
        server_key: h.server_public_key().unwrap(),
    };
    (h, c, rig, dir)
}

fn levels_sub(h: &Handle) -> Sub {
    Sub::connect(h.context(), h.data_endpoint(), &[b"stream/levels"])
}

fn rig_sub(h: &Handle, rig: &CurveClient) -> Sub {
    Sub::connect_with(
        h.context(),
        net_endpoints(h).1.as_str(),
        &[b"source/levels"],
        Some(rig),
        None,
    )
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
struct Peer {
    topic: Topic,
    held: Option<CodedFrame>,
}

impl Peer {
    fn new(topic: Topic) -> Self {
        Self { topic, held: None }
    }

    fn take(&mut self, parts: &[Vec<u8>]) -> Got {
        let parts: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        let w = decode_wire(&parts).unwrap();
        assert_eq!(w.topic(), self.topic);
        let key = matches!(w, WireFrame::Key(_));
        match w.apply(self.held.as_ref()) {
            Ok(c) => {
                // Every frame applied decodes to values.
                c.to_frame().unwrap();
                self.held = Some(c.clone());
                if key { Got::Key(c) } else { Got::Delta(c) }
            }
            Err(DecodeError::NoBase | DecodeError::WrongBase { .. }) => Got::Refused,
            Err(e) => panic!("{e}"),
        }
    }
}

/// The next message of `topic`, credited; keepalives are skipped.
fn recv(s: &Sub, topic: Topic, timeout: Duration) -> Option<Vec<Vec<u8>>> {
    let end = Instant::now() + timeout;
    loop {
        let m = s.raw(end.saturating_duration_since(Instant::now()))?;
        if m.first().is_some_and(|t| *t == topic.to_bytes()) {
            return Some(m);
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

/// `stream/levels` to a lossless peer and to one that drops runs of 1, 3 and 20 messages
/// and never asks for a keyframe: most frames go as deltas, every frame the lossy peer holds
/// is bit for bit the lossless one's of that `seq`, every loss is refused (never applied),
/// and the periodic keyframe brings it back within the keyframe interval.
#[test]
fn slow_a_meter_chain_recovers_from_loss_without_asking() {
    let (h, _c, _rig, _dir) = start();
    let (good, lossy) = (levels_sub(&h), levels_sub(&h));
    let (mut g, mut l) = (
        Peer::new(Topic::StreamLevels),
        Peer::new(Topic::StreamLevels),
    );
    let mut lossless: BTreeMap<u64, CodedFrame> = BTreeMap::new();
    let mut checks: Vec<(u64, CodedFrame)> = Vec::new();
    let runs = [(15usize, 1usize), (40, 3), (70, 20)];
    let dropped = |i: usize| runs.iter().any(|&(at, n)| (at..at + n).contains(&i));
    let (mut i, mut keys, mut deltas, mut refused) = (0usize, 0u32, 0u32, 0u32);
    let mut broke: Option<Instant> = None;
    let mut resyncs: Vec<Duration> = Vec::new();
    let t0 = Instant::now();
    while i < 130 && t0.elapsed() < Duration::from_secs(60) {
        while let Some(m) = recv(&good, Topic::StreamLevels, Duration::ZERO) {
            let first = g.held.is_none();
            match g.take(&m) {
                Got::Key(c) | Got::Delta(c) => {
                    lossless.insert(c.stamp.seq, c);
                }
                Got::Refused if first => {}
                Got::Refused => panic!("the lossless peer lost its chain"),
            }
        }
        let Some(m) = recv(&lossy, Topic::StreamLevels, Duration::from_millis(50)) else {
            continue;
        };
        i += 1;
        if dropped(i) {
            broke.get_or_insert_with(Instant::now);
            continue;
        }
        match l.take(&m) {
            Got::Key(x) => {
                keys += 1;
                if let Some(at) = broke.take() {
                    resyncs.push(at.elapsed());
                }
                checks.push((x.stamp.seq, x));
            }
            Got::Delta(x) => {
                deltas += 1;
                assert!(broke.is_none(), "a delta applied across a loss");
                checks.push((x.stamp.seq, x));
            }
            Got::Refused => refused += 1,
        }
    }
    assert!(i >= 130, "only {i} messages in a minute");
    let last = checks.last().map_or(0, |(s, _)| *s);
    while lossless.last_key_value().is_none_or(|(s, _)| *s < last) {
        let m = recv(&good, Topic::StreamLevels, Duration::from_secs(5)).expect("frames");
        if let Got::Key(c) | Got::Delta(c) = g.take(&m) {
            lossless.insert(c.stamp.seq, c);
        }
    }
    for (seq, x) in &checks {
        same(x, lossless.get(seq).expect("seen"));
    }
    assert!(refused >= 3, "{refused} refused: every loss is noticed");
    assert!(deltas > 2 * keys, "{deltas} deltas, {keys} keyframes");
    assert_eq!(resyncs.len(), runs.len(), "{resyncs:?}");
    resyncs.sort();
    let typical = resyncs[resyncs.len() / 2];
    assert!(
        typical < KEY_INTERVAL + SLACK,
        "back in step after {typical:?} typically: {resyncs:?}"
    );
    assert!(resyncs[resyncs.len() - 1] < 4 * KEY_INTERVAL, "{resyncs:?}");
    h.shutdown();
}

/// An admitted rig that subscribes to `source/levels` late draws from a keyframe within the
/// keyframe interval, then receives deltas, each carrying its own timing mark.
#[test]
fn slow_a_late_rig_draws_source_levels_and_deltas_carry_the_mark() {
    let (h, _c, rig, _dir) = start();
    // The meters run a while before the rig asks.
    let early = levels_sub(&h);
    recv(&early, Topic::StreamLevels, Duration::from_secs(5)).expect("meters");
    std::thread::sleep(Duration::from_millis(1500));
    let t0 = Instant::now();
    let s = rig_sub(&h, &rig);
    let mut p = Peer::new(Topic::SourceLevels);
    let first = loop {
        let m = recv(&s, Topic::SourceLevels, Duration::from_secs(5)).expect("a keyframe");
        match p.take(&m) {
            Got::Key(k) => break k,
            Got::Delta(_) => panic!("applied without a base"),
            Got::Refused => {}
        }
    };
    let took = t0.elapsed();
    assert!(took < KEY_INTERVAL + SLACK, "drew after {took:?}");
    let marks = |c: &CodedFrame| match &c.meta {
        FrameMeta::SourceLevels(m) => m.mark.map(|k| k.sample),
        m => panic!("{m:?}"),
    };
    // The mark comes once the rig's stamp clock is mapped onto system time (a second or
    // two after the stream opens), then with every frame, newer each time: it rides in the
    // delta, not the keyframe's.
    let mut seen = vec![marks(&first)];
    let mut marked_deltas = 0;
    let t0 = Instant::now();
    while marked_deltas < 10 && t0.elapsed() < Duration::from_secs(20) {
        let m = recv(&s, Topic::SourceLevels, Duration::from_secs(5)).expect("frames");
        match p.take(&m) {
            Got::Delta(d) => {
                let k = marks(&d);
                marked_deltas += usize::from(k.is_some());
                seen.push(k);
            }
            Got::Key(k) => seen.push(marks(&k)),
            Got::Refused => panic!("the rig lost its chain"),
        }
    }
    assert_eq!(marked_deltas, 10, "{seen:?}");
    let from = seen.iter().position(Option::is_some).unwrap();
    assert!(seen[from..].iter().all(Option::is_some), "{seen:?}");
    assert!(seen[from..].windows(2).all(|w| w[1] > w[0]), "{seen:?}");
    h.shutdown();
}
