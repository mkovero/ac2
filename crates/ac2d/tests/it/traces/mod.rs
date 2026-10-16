//! Trace store and sessions on the fake rig: capture keeps the shown result with its
//! metadata, averaging and math channels of stored traces use the shared time base, export re-imports exactly,
//! imports refuse bad files with typed errors, and a session reload comes up disarmed in a
//! new epoch with every measurement and trace as saved.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::Path;
use std::time::Duration;

use ac2_audio::FakeDriver;
use ac2_proto::frame::FrameData;
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ErrorDetail, ImportProblem, ReplyBody};
use ac2d::Daemon;
use common::*;

mod capture;
mod owners;
mod session;
mod smoothing;
mod spectrum;

const T: Duration = Duration::from_secs(10);

fn grid_freq(i: usize) -> f64 {
    1000.0 * 2f64.powf((i as f64 - 240.0) / 48.0)
}

fn band() -> Vec<usize> {
    (0..480)
        .filter(|&i| (300.0..=800.0).contains(&grid_freq(i)))
        .collect()
}

fn trace(r: ReplyBody) -> TraceMeta {
    match r {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    }
}

fn data(c: &mut Client, id: TraceId) -> TraceData {
    match c.ok(Command::TraceGet { trace: id }) {
        ReplyBody::TraceData(d) => *d,
        other => panic!("{other:?}"),
    }
}

fn traces(c: &mut Client) -> Vec<TraceMeta> {
    match c.ok(Command::TraceList) {
        ReplyBody::Traces(t) => t,
        other => panic!("{other:?}"),
    }
}

fn state(c: &mut Client) -> State {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    }
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ac2-traces/tests/fixtures")
            .join(name),
    )
    .unwrap()
}

/// Runs audio (refreshing the lease) until a TF frame of config rev ≥ `rev` covers it.
fn settle(
    d: &mut FakeDriver,
    c: &mut Client,
    sub: &Sub,
    tok: LeaseToken,
    rev: u64,
) -> ac2_proto::Frame {
    for _ in 0..5 {
        run(d, 0.5);
        c.ok(Command::GenRefresh { lease_token: tok });
    }
    let end = end_sample(d);
    sub.frame(T, |f| {
        matches!(f.data, FrameData::Tf(_))
            && f.stamp.audio_sample.0 + 1 >= end
            && f.stamp.config_rev.0 >= rev
    })
    .expect("settled tf frame")
}

/// A math channel `a op b` of stored traces, started, run for a moment (it publishes on
/// the audio clock) and captured.
fn math_capture(r: &mut Rig, a: TraceId, op: MathOp, b: TraceId, name: &str) -> TraceMeta {
    let c = &mut r.c;
    let m = match c.ok(Command::MeasCreate {
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: name.into(),
            kind: MeasKind::Math {
                config: MathConfig::of(
                    ac2_proto::model::TraceOwner::Imported,
                    MathDomain::Transfer,
                    MathExpr::Binary {
                        a: Operand::Trace { trace: a },
                        op,
                        b: Operand::Trace { trace: b },
                    },
                ),
            },
        },
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    c.ok(Command::MeasStart { meas: m.id });
    for _ in 0..2 {
        run(&mut r.d, 0.5);
        c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    let t = trace(c.ok(Command::TraceCapture {
        meas: m.id,
        name: name.into(),
        slot: None,
    }));
    c.ok(Command::MeasDelete {
        meas: m.id,
        traces: ac2_proto::model::OwnedTraces::Keep,
    });
    t
}

struct Rig {
    h: ac2d::Handle,
    backend: ac2_audio::FakeBackend,
    c: Client,
    sub: Sub,
    tok: LeaseToken,
    d: FakeDriver,
    _dir: tempfile::TempDir,
}

fn rig(name: &str) -> Rig {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), inproc(name));
    cfg.lease_expiry = Duration::from_secs(60);
    cfg.session_dir = dir.path().join("sessions");
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"d/1/tf"]);
    c.ok(Command::SessionOpen {
        config: session(true),
    });
    c.ok(Command::MeasCreate {
        config: transfer("main"),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    // No result yet: nothing to capture.
    let e = c
        .call(Command::TraceCapture {
            meas: MeasId(1),
            name: "early".into(),
            slot: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
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
    settle(&mut d, &mut c, &sub, tok, 0);
    Rig {
        h,
        backend,
        c,
        sub,
        tok,
        d,
        _dir: dir,
    }
}

fn set_smoothing(c: &mut Client, t: &TraceMeta, s: Option<Smoothing>) -> TraceMeta {
    let mut e = t.edit.clone();
    e.smoothing = s;
    trace(c.ok(Command::TraceUpdate {
        trace: t.id,
        edit: e,
    }))
}
