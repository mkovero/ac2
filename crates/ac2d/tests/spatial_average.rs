//! Live spatial average on a fake rig with three mic inputs: +3 dB and −3 dB paths average
//! by power to 10·log10((10^0.3 + 10^−0.3)/2), a silent input is left out and said to be,
//! the average captures as a trace naming its members, and the members' invariants hold.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use ac2_audio::fake::FakePath;
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_proto::frame::{Frame, FrameData, MemberStatus, ProtectionFlags, TfFrame, ValidityMask};
use ac2_proto::model::{
    AverageMethod, GeneratorDesired, GeneratorSettings, MeasConfig, MeasKind, Signal,
    SpatialAverageConfig, TraceSource,
};
use ac2_proto::units::{Dbfs, MeasId, Seconds, TraceId};
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2d::Daemon;
use common::*;

const T: Duration = Duration::from_secs(10);

fn rig3() -> FakeBackend {
    let db = |g: f32| 10f32.powf(g / 20.0);
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: ac2_audio::fake::FakeDrive::Manual,
        seed: 11,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY + ACOUSTIC_DELAY, vec![db(3.0)], 1e-5),
            FakePath::acoustic(0, 2, LOOP_DELAY + 2 * ACOUSTIC_DELAY, vec![db(-3.0)], 1e-5),
            // Input 3 hears nothing: its measurement has NO SIGNAL.
        ],
        ..FakeConfig::default()
    })
    .unwrap()
}

fn tf_on(name: &str, input: u16) -> MeasConfig {
    let mut m = transfer(name);
    if let MeasKind::Transfer { config } = &mut m.kind {
        config.measurement_input = input;
    }
    m
}

fn average(name: &str, members: &[u32]) -> MeasConfig {
    MeasConfig {
        name: name.into(),
        kind: MeasKind::SpatialAverage {
            config: SpatialAverageConfig::power_of(members.iter().map(|m| MeasId(*m)).collect()),
        },
    }
}

fn grid_freq(i: usize) -> f64 {
    1000.0 * 2f64.powf((i as f64 - 240.0) / 48.0)
}

fn tf(f: &Frame) -> &TfFrame {
    match &f.data {
        FrameData::Tf(t) => t,
        _ => unreachable!(),
    }
}

#[test]
fn power_average_of_three_positions() {
    init_log();
    let backend = rig3();
    let mut cfg = config(backend.clone(), inproc("avg"));
    cfg.lease_expiry = Duration::from_secs(60);
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"d/4/tf"]);
    let mut s = session(false);
    s.input_channels = vec![0, 1, 2, 3];
    c.ok(Command::SessionOpen { config: s });
    for (i, input) in [1u16, 2, 3].into_iter().enumerate() {
        c.ok(Command::MeasCreate {
            config: tf_on(&format!("Seat {}", i + 1), input),
        });
    }

    // Refused shapes: one member, a duplicate, an unknown member, a non-transfer member.
    for bad in [&[1u32][..], &[1, 1], &[1, 9]] {
        let e = c
            .call(Command::MeasCreate {
                config: average("bad", bad),
            })
            .unwrap_err();
        assert!(
            matches!(e.code, ErrorCode::Invalid | ErrorCode::NotFound),
            "{bad:?}: {e:?}"
        );
    }

    match c.ok(Command::MeasCreate {
        config: average("Audience", &[1, 2, 3]),
    }) {
        ReplyBody::Measurement(m) => {
            assert_eq!(m.id, MeasId(4));
            // Its grid is its members', known before it runs.
            assert!(m.grid_id.is_some());
            assert!(m.delay.is_none());
        }
        other => panic!("{other:?}"),
    }
    // Aligned members: phase flat, every column of the band settles.
    for (meas, n) in [(1u32, 1u32), (2, 2), (3, 1)] {
        c.ok(Command::DelaySet {
            meas: MeasId(meas),
            delay: Seconds(f64::from(n * ACOUSTIC_DELAY) / f64::from(FS)),
        });
    }
    for m in 1..=4 {
        c.ok(Command::MeasStart { meas: MeasId(m) });
    }
    let tok = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    c.ok(Command::GenSet {
        lease_token: tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    let mut d = driver(&backend);
    for _ in 0..6 {
        run(&mut d, 0.5);
        c.ok(Command::GenRefresh { lease_token: tok });
    }
    run(&mut d, 0.5);
    let end = end_sample(&d);
    let f = sub
        .frame(T, |f| {
            matches!(f.data, FrameData::Tf(_)) && f.stamp.audio_sample.0 + 1 >= end
        })
        .expect("average frame");
    let t = tf(&f);
    assert_eq!(f.stamp.protection, ProtectionFlags::NONE);
    let a = t.meta.average.as_ref().expect("average metadata");
    assert_eq!(a.method, AverageMethod::Power);
    let st: Vec<MemberStatus> = a.members.iter().map(|m| m.status).collect();
    assert_eq!(st[..2], [MemberStatus::Included, MemberStatus::Included]);
    assert!(
        matches!(st[2], MemberStatus::Refused { protection } if protection.contains(ProtectionFlags::NO_SIGNAL)),
        "{st:?}"
    );
    assert_eq!(a.included(), 2);
    // The phase is referred to the first member's delay.
    assert_eq!(
        t.meta.delay,
        Seconds(f64::from(ACOUSTIC_DELAY) / f64::from(FS))
    );
    let want = 10.0 * ((10f64.powf(0.3) + 10f64.powf(-0.3)) / 2.0).log10();
    let cols: Vec<usize> = (0..t.mag.len())
        .filter(|&i| (300.0..=5000.0).contains(&grid_freq(i)))
        .filter(|&i| t.validity[i] == ValidityMask::NONE)
        .collect();
    assert!(cols.len() > 100, "valid columns {}", cols.len());
    for &i in &cols {
        assert!(
            (f64::from(t.mag[i]) - want).abs() < 0.3,
            "{:.0} Hz: {} vs {want}",
            grid_freq(i),
            t.mag[i]
        );
    }

    // Captured: a transfer trace on the average's grid naming the two members averaged.
    let trace = match c.ok(Command::TraceCapture {
        meas: MeasId(4),
        name: "audience".into(),
        slot: None,
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    };
    match &trace.source {
        TraceSource::SpatialAverage {
            meas,
            method,
            members,
            ..
        } => {
            assert_eq!(*meas, MeasId(4));
            assert_eq!(*method, AverageMethod::Power);
            let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
            assert_eq!(names, ["Seat 1", "Seat 2"]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(trace.id, TraceId(1));

    // Members stay transfers on the average's grid, and are not deleted under it.
    let e = c.call(Command::MeasDelete { meas: MeasId(2) }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    let mut other_grid = tf_on("Seat 2", 2);
    if let MeasKind::Transfer { config } = &mut other_grid.kind {
        config.grid.ppo = 24;
        config.grid.k_min = -120;
        config.grid.k_max = 119;
    }
    let e = c
        .call(Command::MeasUpdate {
            meas: MeasId(2),
            config: other_grid,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    let e = c.call(Command::MeasReset { meas: MeasId(4) }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");

    // A member stopped is left out as stopped; one usable member is no average.
    c.ok(Command::MeasStop { meas: MeasId(2) });
    run(&mut d, 0.5);
    let end = end_sample(&d);
    let f = sub
        .frame(T, |f| {
            matches!(f.data, FrameData::Tf(_)) && f.stamp.audio_sample.0 + 1 >= end
        })
        .expect("average frame");
    let t = tf(&f);
    let st: Vec<MemberStatus> = t
        .meta
        .average
        .as_ref()
        .unwrap()
        .members
        .iter()
        .map(|m| m.status)
        .collect();
    assert_eq!(st[1], MemberStatus::Stopped);
    assert!(t.mag.iter().all(|m| m.is_nan()));
    assert!(t.validity.iter().all(|v| *v == ValidityMask::FEW_MEMBERS));
    let e = c
        .call(Command::TraceCapture {
            meas: MeasId(4),
            name: "one".into(),
            slot: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");

    // Deleting the average frees its members.
    c.ok(Command::MeasDelete { meas: MeasId(4) });
    c.ok(Command::MeasDelete { meas: MeasId(2) });
    h.shutdown();
}
