//! Bounded-freshness publishing (Q2): a stalled subscriber recovers to fresh frames within
//! one drain plus one publish period; a late subscriber gets the latest slot at once.
#![allow(clippy::unwrap_used)]

use crate::common;

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
    // 1.2 s of SPL frames at 20 per second, of which the queue held two.
    assert!(
        fresh.stamp.seq > first.stamp.seq + 15,
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
    // Levels are only metered out while someone receives them.
    let (mut c, early) = connect(&h, &[b"d/1/spl", b"d/1/levels"]);
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

/// Captured audio is handed on in batches, not per device period: meters and measurements
/// must still publish at their rates (30 Hz session meters, 20 Hz SPL),
/// and every frame of a topic must cover newer audio than the one before.
#[test]
fn batched_hand_off_keeps_publish_rates() {
    init_log();
    let backend = realtime_rig();
    let h = Daemon::start(config(backend, inproc("rates"))).unwrap();
    let (mut c, sub) = connect(&h, &[b"session/levels", b"d/1/spl"]);
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    c.ok(Command::MeasCreate {
        config: spl("meter", 0),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    // Settle: the first frames of each topic arrive.
    sub.frame(T, |f| matches!(f.data, FrameData::Spl(_)))
        .expect("spl frames");
    sub.frame(T, |f| matches!(f.data, FrameData::SessionLevels(_)))
        .expect("session meter frames");
    let window = Duration::from_secs(3);
    let t0 = Instant::now();
    let (mut levels, mut spl) = (0u32, 0u32);
    let (mut last_levels, mut last_spl) = (0u64, 0u64);
    while t0.elapsed() < window {
        let Some(DataMessage::Frame(f)) = sub.next(T) else {
            continue;
        };
        let at = f.stamp.audio_sample.0;
        match f.data {
            FrameData::SessionLevels(_) => {
                assert!(at > last_levels, "session meters went back in time");
                last_levels = at;
                levels += 1;
            }
            FrameData::Spl(_) => {
                assert!(at > last_spl, "spl went back in time");
                last_spl = at;
                spl += 1;
            }
            _ => {}
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    let (lr, sr) = (f64::from(levels) / secs, f64::from(spl) / secs);
    assert!((25.0..=33.0).contains(&lr), "session meters at {lr:.1} Hz");
    // SPL frames are capped at 20 Hz (the meter's interval figures hold every peak).
    assert!((15.0..=21.0).contains(&sr), "spl at {sr:.1} Hz");
    h.shutdown();
}
