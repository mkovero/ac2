//! The clocks entity (`docs/design/multi-rig.md` §4), from an empty daemon on a fake rig
//! whose device clock runs a known amount fast against the host clock: no figure before a
//! minute of audio, then the skew, and nothing once the audio closes.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_audio::fake::FakeDrive;
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::model::{Clocks, SampleClock, State, SystemClock};
use ac2_proto::{Change, Command, ReplyBody};
use ac2d::Daemon;
use common::*;

const SKEW_PPM: f64 = 25.0;

/// `seconds` of audio in steps of a second (the capture ring holds about two): nothing
/// here runs on the audio but the meters, so the steps can be long.
fn audio(d: &mut FakeDriver, seconds: u32) {
    for _ in 0..seconds {
        d.run_blocks(u64::from(FS / BLOCK));
        taken(d);
    }
}

fn state(c: &mut Client) -> State {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    }
}

/// The next clocks entity `pred` accepts, from the events.
fn clocks(sub: &Sub, mut pred: impl FnMut(&Clocks) -> bool) -> Clocks {
    let e = sub
        .event(
            Duration::from_secs(30),
            |e| matches!(&e.change, Change::Clocks(c) if pred(c)),
        )
        .expect("clocks event");
    match e.change {
        Change::Clocks(c) => c,
        _ => unreachable!(),
    }
}

#[test]
fn the_capture_clock_reads_its_skew_against_the_host_after_a_minute() {
    init_log();
    let backend = FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 2,
        outputs: 2,
        drive: FakeDrive::Manual,
        host_clock_ppm: SKEW_PPM,
        ..FakeConfig::default()
    })
    .unwrap();
    let h = Daemon::start(config(backend.clone(), inproc("clocks"))).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    let before = state(&mut c).clocks;
    assert_eq!(before.capture, None, "no audio open, no capture clock");
    if cfg!(target_os = "linux") {
        assert_ne!(before.system, SystemClock::NotStated, "Linux states it");
    } else {
        assert_eq!(before.system, SystemClock::NotStated);
    }

    c.ok(Command::SessionOpen {
        config: session(false),
    });
    let mut d = driver(&backend);
    let opened = clocks(&sub, |c| c.capture.is_some());
    assert_eq!(
        opened.capture.as_ref().map(|d| d.rate),
        Some(SampleClock::Measuring)
    );

    // Half a minute is no reading yet.
    audio(&mut d, 30);
    assert_eq!(
        state(&mut c).clocks.capture.map(|d| d.rate),
        Some(SampleClock::Measuring)
    );
    audio(&mut d, 40);
    let measured = clocks(&sub, |c| {
        matches!(
            c.capture.as_ref().map(|d| d.rate),
            Some(SampleClock::Measured { .. })
        )
    });
    let Some(SampleClock::Measured { ppm, span }) = measured.capture.map(|d| d.rate) else {
        unreachable!()
    };
    assert!((ppm - SKEW_PPM).abs() < 0.05, "{ppm} ppm");
    assert!(span.0 > 54.0 && span.0 <= 60.0, "{span:?}");

    c.ok(Command::SessionClose);
    clocks(&sub, |c| c.capture.is_none());
}
