use std::sync::atomic::{AtomicUsize, Ordering};

use ac2_client::TopicFrame;
use ac2_proto::frame::{ClipFlags, LevelsMeta, SessionLevelsFrame};
use ac2_proto::units::MeasId;
use ac2_proto::{Frame, FrameData};

use super::*;

fn out() -> (Out, Shared, Arc<AtomicUsize>) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let inbox = Shared::default();
    let out = Out {
        inbox: inbox.clone(),
        wake: Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }),
    };
    (out, inbox, wakes)
}

/// A frame on `topic` with sequence number `seq` (the body is display material only).
fn frame(topic: Topic, seq: u64) -> TopicFrame {
    let mut stamp = ac2_proto::samples::stamp(None);
    stamp.seq = seq;
    TopicFrame {
        topic,
        frame: Arc::new(Frame {
            stamp,
            data: FrameData::SessionLevels(SessionLevelsFrame {
                meta: LevelsMeta { channels: vec![0] },
                peak: vec![-20.0],
                rms: vec![-30.0],
                clip: vec![ClipFlags::NONE],
            }),
        }),
        received: Instant::now(),
        since_new: Duration::ZERO,
        age: Some(0.5),
        stale: false,
    }
}

fn snapshot(frames: &[TopicFrame]) -> Arc<DataSnapshot> {
    Arc::new(DataSnapshot {
        latest: Latest {
            frames: frames
                .iter()
                .map(|f| (f.topic.to_string().into(), f.clone()))
                .collect(),
            ..Latest::default()
        },
        grids: BTreeMap::new(),
        drained: Instant::now(),
    })
}

const LEQ: Topic = Topic::Data {
    meas: MeasId(4),
    stream: Stream::Leq,
};
const TF: Topic = Topic::Data {
    meas: MeasId(1),
    stream: Stream::Tf,
};

fn data_of(e: &ConnEvent) -> Option<&DataSnapshot> {
    match e {
        ConnEvent::Data(d) => Some(d),
        _ => None,
    }
}

/// A UI that is not drawing holds one snapshot, not a queue of them; the `leq` frames a
/// newer snapshot replaced still reach it, in order, before the newest; the UI is woken
/// once until it drains.
#[test]
fn an_undrained_ui_holds_the_newest_frames_and_every_leq_second() {
    let (out, inbox, wakes) = out();
    for s in 1..=100 {
        // The TF every poll, the Leq once in ten.
        out.data(snapshot(&[frame(TF, s), frame(LEQ, s.div_ceil(10))]), true);
    }
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    let got = lock(&inbox).take();
    let leq_seqs: Vec<u64> = got
        .iter()
        .filter_map(data_of)
        .filter_map(|d| d.latest.get(&LEQ).map(|f| f.frame.stamp.seq))
        .collect();
    assert_eq!(leq_seqs, (1..=10).collect::<Vec<u64>>());
    // The backlog carries only `leq` topics; the last one is the whole newest snapshot.
    let (last, backlog) = got.split_last().expect("events");
    assert!(
        backlog
            .iter()
            .filter_map(data_of)
            .all(|d| d.latest.get(&TF).is_none())
    );
    assert_eq!(
        data_of(last)
            .and_then(|d| d.latest.get(&TF))
            .map(|f| f.frame.stamp.seq),
        Some(100)
    );
    out.data(snapshot(&[frame(TF, 101)]), true);
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
}

/// Quiet updates (keepalive time, unchanged frames) wait for the UI's next pass without
/// waking it; a new connection discards what the old one left untaken.
#[test]
fn quiet_updates_do_not_wake_and_reconnects_discard() {
    let (out, inbox, wakes) = out();
    out.data(snapshot(&[frame(TF, 1)]), false);
    assert_eq!(wakes.load(Ordering::SeqCst), 0);
    assert!(lock(&inbox).data.is_some());
    out.send(ConnEvent::Connecting { target: "t".into() });
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    let i = lock(&inbox);
    assert!(i.data.is_none() && i.mirror.is_none() && i.leq.is_empty());
    assert_eq!(i.events.len(), 1);
}

#[test]
fn ages_count_between_snapshots() {
    let s = snapshot(&[frame(TF, 1)]);
    let later = s.drained + Duration::from_millis(1500);
    let a = s.aged(later);
    let f = a.latest.get(&TF).expect("the TF");
    assert_eq!(f.since_new, Duration::from_millis(1500));
    assert!((f.age.expect("an age") - 2.0).abs() < 1e-9);
    assert_eq!(a.drained, later);
}

/// Frames arriving between drains are drained on arrival, not held to the next tick of
/// the display period; drains stay at least half a period apart.
#[test]
fn frames_are_drained_on_arrival_at_most_twice_a_period() {
    let t0 = tokio::time::Instant::now();
    let ms = |v: u64| Duration::from_millis(v);
    let mut c = PollClock::new(t0);
    c.polled(t0, true, false, DISPLAY_PERIOD);
    assert!(c.watch, "an active link watches for frames");
    assert_eq!(c.next, t0 + DISPLAY_PERIOD);
    // A frame 2 ms after a drain waits only for the half-period floor.
    c.arrived(DISPLAY_PERIOD);
    assert!(!c.watch, "no second wake before the drain");
    assert_eq!(c.next, t0 + DISPLAY_PERIOD / 2);
    // One 20 ms after the drain goes at once.
    let t1 = t0 + ms(40);
    c.polled(t1, true, false, DISPLAY_PERIOD);
    c.arrived(DISPLAY_PERIOD);
    assert!(c.next <= t1 + ms(20));
    // Idle: the frame cuts the idle wait short the same way.
    let t2 = t1 + ms(100);
    c.polled(t2, false, false, DISPLAY_PERIOD);
    assert_eq!(c.next, t2 + IDLE_POLL);
    assert!(c.watch);
    c.arrived(DISPLAY_PERIOD);
    assert_eq!(c.next, t2 + DISPLAY_PERIOD / 2);
}

/// A UI that is not drawing is polled at the idle period only, frames or not.
#[test]
fn a_ui_not_drawing_is_polled_at_the_idle_period() {
    let t0 = tokio::time::Instant::now();
    let mut c = PollClock::new(t0);
    c.polled(t0, true, true, DISPLAY_PERIOD);
    assert!(!c.watch, "arrivals do not wake a link whose UI is away");
    assert_eq!(c.next, t0 + IDLE_POLL);
}

#[test]
fn same_picture_ignores_seq_and_time_only() {
    let a = frame(TF, 1);
    let mut b = frame(TF, 2);
    assert!(same_picture(&a.frame, &b.frame));
    Arc::make_mut(&mut b.frame).stamp.protection = ac2_proto::frame::ProtectionFlags::CLIP;
    assert!(!same_picture(&a.frame, &b.frame));
}
