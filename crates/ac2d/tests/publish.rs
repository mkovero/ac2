//! Bounded-freshness publishing (Q2): a stalled subscriber recovers to fresh frames within
//! one drain plus one publish period; a late subscriber gets the latest slot at once.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use ac2_proto::frame::FrameData;
use ac2_proto::units::MeasId;
use ac2_proto::{Command, DataMessage, decode_data_message};
use ac2_zmq::drain_latest;
use ac2d::{Daemon, Listen};
use common::*;

const T: Duration = Duration::from_secs(5);

fn seq_of(m: &ac2_zmq::Message) -> Option<u64> {
    let parts: Vec<&[u8]> = m.frames().iter().map(Vec::as_slice).collect();
    match decode_data_message(&parts).ok()? {
        DataMessage::Frame(f) => Some(f.stamp.seq),
        DataMessage::Event(e) => Some(e.rev.0),
    }
}

fn stalled_subscriber_recovers(listen: Listen) {
    init_log();
    let h = Daemon::start(config(realtime_rig(), listen)).unwrap();
    let mut c = Client::connect(h.context(), h.ctrl_endpoint());
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    c.ok(Command::MeasCreate {
        config: spl("meter", 0),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });

    // A subscriber with a tiny receive queue that stops reading.
    let sub = Sub::connect_with(h.context(), h.data_endpoint(), &[b"d/1/spl"], None, Some(2));
    let first = sub
        .frame(T, |f| matches!(f.data, FrameData::Spl(_)))
        .expect("first frame");
    std::thread::sleep(Duration::from_millis(1200));

    // One drain discards the backlog …
    let t0 = Instant::now();
    let d = drain_latest(&sub.sock, 10_000, seq_of).unwrap();
    assert!(d.emptied);
    assert_eq!(d.malformed, 0);
    // … and the next publish period brings a fresh frame.
    let fresh = sub
        .frame(Duration::from_millis(500), |f| {
            let age_ns = wall_now_ns().saturating_sub(f.stamp.capture_wall_ns.0);
            age_ns < 150_000_000
        })
        .expect("fresh frame after one drain");
    let took = t0.elapsed();
    assert!(
        took < Duration::from_millis(200),
        "recovered in {took:?} (drain + publish period)"
    );
    assert!(
        fresh.stamp.seq > first.stamp.seq + 30,
        "newer frames were skipped, not queued"
    );
    h.shutdown();
}

#[test]
fn stalled_subscriber_recovers_inproc() {
    stalled_subscriber_recovers(inproc("stall"));
}

#[test]
fn stalled_subscriber_recovers_tcp() {
    stalled_subscriber_recovers(local_tcp());
}

#[test]
fn late_subscriber_gets_latest_slot() {
    init_log();
    let backend = manual_rig();
    let h = Daemon::start(config(backend.clone(), inproc("late"))).unwrap();
    let (mut c, early) = connect(&h, &[b"d/1/spl"]);
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    c.ok(Command::MeasCreate {
        config: spl("meter", 0),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    let mut d = driver(&backend);
    run(&mut d, 0.2);
    let end = end_sample(&d);
    let last = early
        .frame(T, |f| {
            matches!(f.data, FrameData::Spl(_)) && f.stamp.audio_sample.0 + 1 == end
        })
        .expect("frame up to the end");
    // The device is idle now: no new frame will be produced.
    std::thread::sleep(Duration::from_millis(100));

    let t0 = Instant::now();
    let late = Sub::connect(h.context(), h.data_endpoint(), &[b"d/1/"]);
    // Both of the measurement's streams (spl, levels) are under the prefix, in either order.
    let mut levels = false;
    let got = late
        .frame(T, |f| {
            levels |= matches!(f.data, FrameData::Levels(_));
            matches!(f.data, FrameData::Spl(_))
        })
        .expect("latest slot re-sent on subscribe");
    assert!(t0.elapsed() < Duration::from_millis(500));
    assert_eq!(got.stamp.seq, last.stamp.seq);
    assert_eq!(got.stamp.audio_sample, last.stamp.audio_sample);
    if !levels {
        late.frame(T, |f| matches!(f.data, FrameData::Levels(_)))
            .expect("levels slot too");
    }
    h.shutdown();
}
