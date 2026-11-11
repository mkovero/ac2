//! The clocks entity (`docs/design/multi-rig.md` §4), from an empty daemon on a fake rig
//! whose device clock runs a known amount fast against true time and whose stamp clock (the
//! host crystal) another: no figure before a minute of audio, then the device's skew
//! against the disciplined clock, not the crystal, and nothing once the audio closes.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_audio::fake::FakeDrive;
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::model::{ClockBasis, Clocks, SampleClock, SystemClock};
use ac2_proto::{Change, Command};
use ac2d::Daemon;
use common::*;

/// `seconds` of audio in steps of a second (the capture ring holds about two): nothing
/// here runs on the audio but the meters, so the steps can be long.
fn audio(d: &mut FakeDriver, seconds: u32) {
    for _ in 0..seconds {
        d.run_blocks(u64::from(FS / BLOCK));
        taken(d);
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

/// The capture clock's reading on a fake rig whose device runs `audio_ppm` fast against
/// true time and whose stamp clock `crystal_ppm` fast, after 70 s of audio, with the clocks
/// entity checked along the way.
fn run(name: &str, audio_ppm: f64, crystal_ppm: f64) -> (f64, ClockBasis) {
    let backend = FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 2,
        outputs: 2,
        drive: FakeDrive::Manual,
        audio_clock_ppm: audio_ppm,
        host_crystal_ppm: crystal_ppm,
        ..FakeConfig::default()
    })
    .unwrap();
    let h = Daemon::start(config(backend.clone(), inproc(name))).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    let before = c.state().clocks;
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
        c.state().clocks.capture.map(|d| d.rate),
        Some(SampleClock::Measuring)
    );
    audio(&mut d, 40);
    let measured = clocks(&sub, |c| {
        matches!(
            c.capture.as_ref().map(|d| d.rate),
            Some(SampleClock::Measured { .. })
        )
    });
    let Some(SampleClock::Measured { ppm, span, against }) = measured.capture.map(|d| d.rate)
    else {
        unreachable!()
    };
    assert!(span.0 > 54.0 && span.0 <= 60.0, "{span:?}");

    c.ok(Command::SessionClose);
    clocks(&sub, |c| c.capture.is_none());
    (ppm, against)
}

#[test]
fn the_capture_clock_reads_its_skew_against_the_system_clock_after_a_minute() {
    init_log();
    let (ppm, against) = run("clocks", 25.0, 0.0);
    assert!((ppm - 25.0).abs() < 0.05, "{ppm} ppm");
    assert_eq!(against, ClockBasis::SystemClock);
}

/// A device locked to the reference time (ADAT from the reference) on a host whose crystal
/// runs 8.2 ppm slow: +8.2 against the crystal, 0 against the disciplined clock.
#[test]
fn a_locked_device_on_a_slow_crystal_reads_no_skew() {
    init_log();
    let (ppm, against) = run("clocks-locked", 0.0, -8.2);
    assert!(ppm.abs() < 0.05, "{ppm} ppm");
    assert_eq!(against, ClockBasis::SystemClock);
}

/// A free-running device 5.6 ppm fast on a host whose crystal runs 0.64 ppm fast: +5.0
/// against the crystal, +5.6 against the disciplined clock.
#[test]
fn a_free_running_device_reads_its_own_skew_not_the_crystals() {
    init_log();
    let (ppm, against) = run("clocks-free", 5.6, 0.64);
    assert!((ppm - 5.6).abs() < 0.05, "{ppm} ppm");
    assert_eq!(against, ClockBasis::SystemClock);
}
