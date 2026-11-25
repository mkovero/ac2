//! Trace store and sessions on the fake rig: capture keeps the shown result with its
//! metadata, averaging and math traces of stored traces use the shared time base, export re-imports exactly,
//! imports refuse bad files with typed errors, and a session reload comes up disarmed in a
//! new epoch with every measurement and trace as saved.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_audio::FakeDriver;
use ac2_proto::frame::FrameData;
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ErrorDetail, ImportProblem};
use ac2d::Daemon;
use common::*;

mod capture;
mod owners;
mod session;
mod smoothing;
mod spectrum;
mod stopped;
mod target;

const T: Duration = Duration::from_secs(10);

fn grid_freq(i: usize) -> f64 {
    1000.0 * 2f64.powf((i as f64 - 336.0) / 48.0)
}

fn band() -> Vec<usize> {
    (0..720)
        .filter(|&i| (300.0..=800.0).contains(&grid_freq(i)))
        .collect()
}

/// `raw` (as captured: unsmoothed, NaN gaps) smoothed by ac2-core directly: magnitude dB and
/// phase degrees.
fn core_smoothed(raw: &TraceData, s: Smoothing) -> (Vec<f64>, Vec<f64>) {
    use ac2_core::smoothing as sm;
    let grid = ac2_core::grid::LogGrid {
        ppo: 48,
        k_min: -336,
        k_max: 383,
    };
    let phase = raw.phase_deg.as_ref().unwrap();
    let h: Vec<num_complex::Complex64> = raw
        .mag_db
        .iter()
        .zip(phase)
        .map(|(m, p)| {
            num_complex::Complex64::from_polar(
                10f64.powf(f64::from(*m) / 20.0),
                f64::from(*p).to_radians(),
            )
        })
        .collect();
    let valid: Vec<bool> = h.iter().map(|z| z.re.is_finite()).collect();
    let coh = vec![1.0; h.len()];
    let fraction = match s.fraction {
        SmoothingFraction::Third => sm::SmoothingFraction::Third,
        SmoothingFraction::Sixth => sm::SmoothingFraction::Sixth,
        SmoothingFraction::Twelfth => sm::SmoothingFraction::Twelfth,
        SmoothingFraction::TwentyFourth => sm::SmoothingFraction::TwentyFourth,
        SmoothingFraction::FortyEighth => sm::SmoothingFraction::FortyEighth,
    };
    let mode = match s.mode {
        SmoothingMode::Magnitude => sm::SmoothingMode::Magnitude,
        SmoothingMode::MagnitudePhase => sm::SmoothingMode::MagnitudePhase,
    };
    let out = sm::Smoother::new(grid, fraction).smooth(
        sm::TfColumns {
            h: &h,
            coherence: &coh,
            valid: &valid,
        },
        mode,
    );
    out.h
        .iter()
        .map(|z| (20.0 * z.norm().log10(), z.arg().to_degrees()))
        .unzip()
}

/// Smallest difference between two angles in degrees.
fn deg_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

fn data(c: &mut Client, id: TraceId) -> TraceData {
    c.trace_data(Command::TraceGet { trace: id })
}

fn traces(c: &mut Client) -> Vec<TraceMeta> {
    c.traces(Command::TraceList)
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

/// A math trace `a op b` of stored traces, started, run for a moment (it publishes on
/// the audio clock) and captured.
fn math_capture(r: &mut Rig, a: TraceId, op: MathOp, b: TraceId, name: &str) -> TraceMeta {
    let c = &mut r.c;
    let m = c.measurement(Command::MeasCreate {
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
    });
    c.ok(Command::MeasStart { meas: m.id });
    for _ in 0..2 {
        run(&mut r.d, 0.5);
        c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    let t = c.trace(Command::TraceCapture {
        meas: m.id,
        name: name.into(),
        slot: None,
    });
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
    c.ok(Command::StreamOpen {
        config: stream(true),
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
    let tok = c.acquire();
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
    c.trace(Command::TraceUpdate {
        trace: t.id,
        edit: e,
    })
}
