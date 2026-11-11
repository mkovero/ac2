//! Bounded-freshness publishing (Q2): a stalled subscriber's backlog is bounded by its
//! window, and it recovers to fresh frames within one drain plus one publish period; a late
//! subscriber gets the latest slot at once.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_proto::flow::DATA_WINDOW_MIN;
use ac2_proto::frame::FrameData;
use ac2_proto::model::{MeasConfig, MeasKind, SpectrumConfig};
use ac2_proto::units::MeasId;
use ac2_proto::{Command, DataMessage};
use ac2d::{Daemon, Listen};
use common::*;

const T: Duration = Duration::from_secs(5);
/// Stalls a subscriber may take to show one recovery within the bounds.
const ATTEMPTS: usize = 3;

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

    // A subscriber with a tiny receive queue and the smallest window that stops reading.
    let sub = Sub::connect_with(h.context(), h.data_endpoint(), &[b"d/1/spl"], None, Some(2));
    sub.set_window(DATA_WINDOW_MIN);
    let mut first = sub
        .frame(T, |f| matches!(f.data, FrameData::Spl(_)))
        .expect("first frame");
    // The bounds are wall-clock time on a machine that may be busy with other work: a
    // descheduled daemon or test thread misses them without anything having queued. So the
    // stall is repeated a few times and one recovery within the bounds suffices; a backlog
    // that is queued instead of skipped misses them every time.
    let mut misses = Vec::new();
    for _ in 0..ATTEMPTS {
        std::thread::sleep(Duration::from_millis(1200));

        // One drain reads the backlog, which the window bounds: the frames formed while
        // the subscriber had no credit replaced each other in the daemon's slot …
        let t0 = Instant::now();
        sub.auto_credit.set(false);
        while sub.next(Duration::ZERO).is_some() {}
        let backlog = sub.owed().0;
        sub.auto_credit.set(true);
        sub.credit();
        assert!(
            backlog <= DATA_WINDOW_MIN,
            "{backlog} bytes queued past a {DATA_WINDOW_MIN} byte window"
        );
        // … and the next publish period brings a fresh frame, a delta on the frame the
        // daemon last handed over for this peer.
        let fresh = sub.frame(Duration::from_millis(500), |f| {
            let age_ns = wall_now_ns().saturating_sub(f.stamp.capture_wall_ns.0);
            age_ns < 150_000_000
        });
        let took = t0.elapsed();
        match fresh {
            Some(fresh) if took < Duration::from_millis(200) => {
                // 1.2 s of SPL frames at 20 per second, of which the queue held two.
                assert!(
                    fresh.stamp.seq > first.stamp.seq + 15,
                    "newer frames were skipped, not queued"
                );
                h.shutdown();
                return;
            }
            Some(fresh) => {
                misses.push(format!("recovered in {took:?}"));
                first = fresh;
            }
            None => {
                misses.push("no fresh frame within 500 ms".into());
                first = sub
                    .frame(T, |f| matches!(f.data, FrameData::Spl(_)))
                    .expect("a frame");
            }
        }
    }
    panic!("never recovered within one drain plus a publish period (200 ms): {misses:?}");
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

/// A spectrum slower than the frame rate goes out on the hand-off that formed it: at
/// 10 frames/s and a 65 536-point window at 48 kHz (a new spectrum every 8448 samples,
/// 176 ms), each frame lags the end of its window by under a hand-off, where the 100 ms
/// frame grid alone would hold it for up to a period.
#[test]
fn a_slow_spectrum_goes_when_it_is_formed() {
    init_log();
    let mut cfg = config(realtime_rig(), inproc("slow-spectrum"));
    cfg.publish_fps = 10;
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"d/1/spec"]);
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    c.ok(Command::MeasCreate {
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "spectrum".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig {
                    resolution: ac2_proto::model::SpecResolution::Fixed { fft_len: 65_536 },
                    ..SpectrumConfig::on_input(0)
                },
            },
        },
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    let hop = u64::from(SpectrumConfig::display_hop(65_536, FS, BLOCK));
    // A hand-off is a device period; a loaded machine may add a few blocks.
    let tolerance = u64::from(FS) * 30 / 1000;
    let (mut frames, mut prompt) = (0u32, 0u32);
    let t0 = Instant::now();
    while frames < 12 && t0.elapsed() < Duration::from_secs(10) {
        let Some(f) = sub.frame(T, |f| matches!(f.data, FrameData::Spec(_))) else {
            break;
        };
        // Spectra are formed every `hop` samples once the first window is full.
        let since = f.stamp.audio_sample.0 + 1 - f.stamp.config_applied_at.0;
        let lag = since % hop;
        frames += 1;
        if lag <= tolerance {
            prompt += 1;
        }
    }
    assert!(frames >= 12, "{frames} spectra");
    // On the grid, lags spread evenly over a 100 ms period: about a third within 30 ms.
    assert!(
        prompt >= 10,
        "{prompt} of {frames} spectra within 30 ms of their window"
    );
    h.shutdown();
}

/// A live spectrum, one FFT or multi-resolution, frames every whole number of device periods
/// nearest a display frame (768 samples, 3 × 256, at 48 kHz): consecutive frames step by
/// exactly that, and each goes out once the block that ends it is handed on, before the
/// device has delivered any audio after it. Checked in samples: the driver is stepped a
/// block at a time and does not step past a frame's end until that frame has arrived.
#[test]
fn a_live_spectrum_frames_every_hop_once_its_last_block_is_in() {
    use ac2_proto::model::SpecResolution;
    init_log();
    let backend = manual_rig();
    let h = Daemon::start(config(backend.clone(), inproc("frame-hop"))).unwrap();
    let (mut c, sub) = connect(&h, &[b"d/1/spec", b"d/2/spec"]);
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    for (name, resolution) in [
        ("one fft", SpecResolution::Fixed { fft_len: 4096 }),
        ("multi", SpecResolution::MULTI),
    ] {
        c.ok(Command::MeasCreate {
            config: MeasConfig {
                scope: ac2_proto::model::MeasScope::Session,
                name: name.into(),
                kind: MeasKind::Spectrum {
                    config: SpectrumConfig {
                        resolution,
                        ..SpectrumConfig::on_input(1)
                    },
                },
            },
        });
    }
    c.ok(Command::MeasStart { meas: MeasId(1) });
    c.ok(Command::MeasStart { meas: MeasId(2) });
    let hop = u64::from(SpectrumConfig::frame_hop(FS, BLOCK));
    assert_eq!(hop, 768);
    let mut d = driver(&backend);
    // Past the multi-resolution ladder's longest window, so every band has a spectrum.
    run(&mut d, 1.5);
    let spec_end = |f: &ac2_proto::frame::Frame, m: MeasId| match &f.data {
        FrameData::Spec(s) if s.meas == m => Some(f.stamp.audio_sample.0 + 1),
        _ => None,
    };
    // Reads frames until each of `want` has one ending at or past its sample; the first
    // such end of each.
    let until = |want: &[(MeasId, u64)]| -> Vec<u64> {
        let mut got: Vec<Option<u64>> = vec![None; want.len()];
        while got.iter().any(Option::is_none) {
            let f = sub
                .frame(T, |f| matches!(f.data, FrameData::Spec(_)))
                .unwrap_or_else(|| panic!("no frames for {want:?}, got {got:?}"));
            for ((m, at), g) in want.iter().zip(&mut got) {
                if g.is_none() && spec_end(&f, *m).is_some_and(|e| e >= *at) {
                    *g = spec_end(&f, *m);
                }
            }
        }
        got.into_iter().flatten().collect()
    };
    // The newest frame of each, on its grid point at or before the audio so far.
    let end = end_sample(&d);
    let ids = [MeasId(1), MeasId(2)];
    let newest = until(&ids.map(|m| (m, (end + 1).saturating_sub(hop))));
    let mut next: Vec<(MeasId, u64)> = ids
        .into_iter()
        .zip(newest)
        .map(|(m, e)| {
            assert!(e <= end && e % u64::from(BLOCK) == 0, "{m:?} ends at {e}");
            (m, e + hop)
        })
        .collect();
    for _ in 0..60 {
        let target = next.iter().map(|(_, e)| *e).min().unwrap();
        while end_sample(&d) < target {
            d.run_blocks(1);
            taken(&d);
        }
        assert_eq!(end_sample(&d), target, "grid points fall on block ends");
        let due: Vec<(MeasId, u64)> = next.iter().copied().filter(|(_, e)| *e == target).collect();
        // Nothing past `target` was captured, so a frame here went within a block.
        for ((m, _), e) in due.iter().zip(until(&due)) {
            assert_eq!(e, target, "{m:?}");
        }
        for (_, e) in next.iter_mut().filter(|(_, e)| *e == target) {
            *e += hop;
        }
    }
    h.shutdown();
}
