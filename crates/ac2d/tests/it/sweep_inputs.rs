//! One sweep, several mics: one play records every measurement input against the shared
//! reference, each into its own trace named for its mic and carrying its own input's mic
//! curve; inputs that make no run are refused; the run survives a session save and load.
#![allow(clippy::unwrap_used)]

use crate::common;
use crate::sweep::{LEVEL, arm, create, csv_columns, request, run_until, start};

use ac2_audio::fake::{FakeDrive, FakePath};
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_core::mic_curve::MicCurve;
use ac2_proto::model::{
    MeasConfig, MeasKind, SessionRef, SweepConfig, SweepStatus, TraceData, TraceMeta, TraceSource,
};
use ac2_proto::units::{Blob, Hz, LeaseToken, TraceId};
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2d::Daemon;
use common::*;

/// Mic 2 stands farther away than mic 1: quieter and later.
const FAR_GAIN: f32 = 0.25;
const FAR_DELAY: u32 = 240;

/// Out 1 → in 1 loopback; out 1 → in 2 and in 3, two mics at two distances.
fn two_mic_rig() -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: FakeDrive::Manual,
        seed: 21,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY + ACOUSTIC_DELAY, vec![ACOUSTIC_GAIN], 1e-5),
            FakePath::acoustic(0, 2, LOOP_DELAY + FAR_DELAY, vec![FAR_GAIN], 1e-5),
        ],
        ..FakeConfig::default()
    })
    .unwrap()
}

fn import_curve(c: &mut Client, mic: &str, input: u16, curve: &str) {
    c.ok(Command::CalCurveImport {
        mic: mic.into(),
        label: None,
        file_name: format!("{mic}.frd"),
        content: Blob(curve.as_bytes().to_vec()),
        input: Some(input),
    });
}

fn data(c: &mut Client, trace: TraceId) -> TraceData {
    match c.ok(Command::TraceGet { trace }) {
        ReplyBody::TraceData(t) => *t,
        other => panic!("{other:?}"),
    }
}

fn state(c: &mut Client) -> ac2_proto::model::State {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    }
}

/// The trace's frequencies and magnitude as measured (the CSV export keeps the columns as
/// measured).
fn csv_mag(c: &mut Client, trace: TraceId) -> (Vec<f64>, Vec<f64>) {
    let csv = match c.ok(Command::TraceExport {
        trace,
        format: ac2_proto::model::ExportFormat::Ac2Csv,
    }) {
        ReplyBody::Export { content, .. } => String::from_utf8(content.0).unwrap(),
        other => panic!("{other:?}"),
    };
    let mut cols = csv_columns(&csv);
    (
        cols.remove("freq_hz").unwrap(),
        cols.remove("mag_db").unwrap(),
    )
}

fn two_mic_setup(
    name: &str,
    dir: &std::path::Path,
) -> (
    ac2d::Handle,
    Client,
    Sub,
    Sub,
    ac2_audio::FakeDriver,
    LeaseToken,
) {
    init_log();
    let backend = two_mic_rig();
    let mut cfg = config(backend.clone(), inproc(name));
    cfg.session_dir = dir.join("sessions");
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    let ka = Sub::connect(h.context(), h.data_endpoint(), &[b"ka"]);
    c.ok(Command::Hello {
        client: "sweep inputs test".into(),
    });
    c.ok(Command::SessionOpen {
        config: ac2_proto::model::SessionConfig {
            input_channels: vec![0, 1, 2],
            ..session(true)
        },
    });
    let d = driver(&backend);
    let token = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    (h, c, sub, ka, d, token)
}

fn sweep_source(m: &TraceMeta) -> (u16, u16, u32) {
    match &m.source {
        TraceSource::Sweep {
            reference_input,
            measurement_input,
            number,
            ..
        } => (*reference_input, *measurement_input, *number),
        other => panic!("{other:?}"),
    }
}

#[test]
fn one_sweep_records_each_mic_into_its_own_trace_with_its_own_curve() {
    // Two capsules, two curves: +6 dB from 4 kHz on the near mic, −4 dB on the far one;
    // both flat to 1 kHz (the normalisation point without a calibration).
    const NEAR: &str = "* near\n20 0\n1000 0\n4000 6\n24000 6\n";
    const FAR: &str = "* far\n20 0\n1000 0\n4000 -4\n24000 -4\n";
    let dir = tempfile::tempdir().unwrap();
    let (_h, mut c, sub, ka, mut d, token) = two_mic_setup("sweep-inputs", dir.path());
    import_curve(&mut c, "M30", 1, NEAR);
    import_curve(&mut c, "M50", 2, FAR);
    let near = MicCurve::from_points(&[(20.0, 0.0), (1000.0, 0.0), (4000.0, 6.0), (24000.0, 6.0)])
        .unwrap()
        .normalised(1000.0);
    let far = MicCurve::from_points(&[(20.0, 0.0), (1000.0, 0.0), (4000.0, -4.0), (24000.0, -4.0)])
        .unwrap()
        .normalised(1000.0);

    let config = SweepConfig {
        measurement_inputs: vec![1, 2],
        ..request(LEVEL)
    };
    let meas = create(&mut c, "two mics", config.clone());
    arm(&mut c, token);
    let r = start(&mut c, token, meas);
    assert_eq!(r.measurement_inputs, vec![1, 2]);
    let done = run_until(&mut d, &mut c, &sub, &ka, token, |x| {
        x.id == r.id && !x.active()
    });
    let SweepStatus::Done { traces } = done.status else {
        panic!("sweep failed: {:?}", done.status);
    };
    assert_eq!(traces.len(), 2, "one trace per mic");

    let a = data(&mut c, traces[0]);
    let b = data(&mut c, traces[1]);
    assert_eq!(sweep_source(&a.meta), (0, 1, 1));
    assert_eq!(sweep_source(&b.meta), (0, 2, 1), "one run, one number");
    // Each trace says its mic.
    assert_eq!(a.meta.edit.name, "Run 1 · M30");
    assert_eq!(b.meta.edit.name, "Run 1 · M50");
    // Each carries its own input's mic and curve (applied after capture).
    for (t, mic) in [(&a, "M30"), (&b, "M50")] {
        assert_eq!(t.meta.mic.as_ref().map(|m| m.name.as_str()), Some(mic));
        let mc = t
            .meta
            .mic_curve
            .as_ref()
            .expect("the input's curve applied");
        assert_eq!(mc.mic, mic);
        assert_eq!(mc.f_norm, Hz(1000.0));
        assert_eq!(t.meta.cal, ac2_proto::model::CalState::Uncalibrated);
    }
    // Each its own path: the far mic arrives later.
    let late = (b.meta.delay.0 - a.meta.delay.0) * f64::from(FS);
    let want = f64::from(FAR_DELAY - ACOUSTIC_DELAY);
    assert!((late - want).abs() < 1.0, "{late} samples, want {want}");

    // Each trace shows its own curve on its own measured columns.
    let mut checked = [0usize; 2];
    let mut level = [Vec::new(), Vec::new()];
    for (k, (t, curve)) in [(&a, &near), (&b, &far)].into_iter().enumerate() {
        let (f, raw) = csv_mag(&mut c, t.meta.id);
        assert_eq!(raw.len(), t.mag_db.len());
        for (i, fi) in f.iter().enumerate() {
            let shown = f64::from(t.mag_db[i]);
            if raw[i].is_finite() && shown.is_finite() {
                let want = curve.db(*fi);
                assert!(
                    (raw[i] - shown - want).abs() < 1e-3,
                    "trace {k} at {fi} Hz: {} → {shown}, curve {want}",
                    raw[i]
                );
                if want.abs() > 1.0 {
                    checked[k] += 1;
                }
                if (500.0..2000.0).contains(fi) {
                    level[k].push(raw[i]);
                }
            }
        }
    }
    assert!(checked.iter().all(|n| *n > 20), "{checked:?}");
    // Mic 2 reads its own path's gain, not mic 1's.
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let gap = mean(&level[0]) - mean(&level[1]);
    let want = 20.0 * f64::from(ACOUSTIC_GAIN / FAR_GAIN).log10();
    assert!(
        (gap - want).abs() < 0.5,
        "{gap:.2} dB apart, want {want:.2}"
    );

    // The measurement counts one run, the run's traces both under it.
    let st = state(&mut c);
    let owned: Vec<TraceId> = st
        .traces
        .iter()
        .filter(|t| t.edit.owner == ac2_proto::model::TraceOwner::Meas { meas })
        .map(|t| t.id)
        .collect();
    assert_eq!(owned, traces);

    // Saved and loaded, the measurement keeps its inputs and each trace its input and curve.
    c.ok(Command::FileSave {
        session: SessionRef::Name {
            name: "two mics".into(),
        },
        overwrite: false,
    });
    c.ok(Command::MeasDelete {
        meas,
        traces: ac2_proto::model::OwnedTraces::Delete,
    });
    c.ok(Command::FileLoad {
        session: SessionRef::Name {
            name: "two mics".into(),
        },
        discard: true,
    });
    let st = state(&mut c);
    let m = st
        .measurements
        .iter()
        .find(|m| m.config.name == "two mics")
        .expect("loaded");
    assert_eq!(m.config.kind, MeasKind::Sweep { config });
    let mut loaded: Vec<&TraceMeta> = st
        .traces
        .iter()
        .filter(|t| matches!(t.source, TraceSource::Sweep { .. }))
        .collect();
    loaded.sort_by_key(|t| sweep_source(t).1);
    assert_eq!(loaded.len(), 2);
    for (t, (input, mic, name)) in loaded
        .iter()
        .zip([(1, "M30", "Run 1 · M30"), (2, "M50", "Run 1 · M50")])
    {
        assert_eq!(sweep_source(t).1, input);
        assert_eq!(t.edit.name, name);
        assert_eq!(t.mic_curve.as_ref().map(|m| m.mic.as_str()), Some(mic));
    }
}

#[test]
fn inputs_that_make_no_run_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (_h, mut c, _sub, _ka, _d, token) = two_mic_setup("sweep-inputs-refused", dir.path());
    let make = |inputs: Vec<u16>| MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name: "bad".into(),
        kind: MeasKind::Sweep {
            config: SweepConfig {
                measurement_inputs: inputs,
                ..request(LEVEL)
            },
        },
    };
    for (inputs, says) in [
        (vec![], "at least one measurement input"),
        (vec![1, 1], "measurement input 2 is listed twice"),
        (vec![1, 0], "input 1 is the reference"),
        ((1..=17).collect(), "at most 16"),
    ] {
        let e = c
            .call(Command::MeasCreate {
                config: make(inputs.clone()),
            })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{inputs:?}: {e:?}");
        assert!(e.msg.contains(says), "{inputs:?}: {}", e.msg);
    }
    // An update is checked as a create is.
    let meas = create(
        &mut c,
        "good",
        SweepConfig {
            measurement_inputs: vec![1, 2],
            ..request(LEVEL)
        },
    );
    let e = c.update(meas, make(vec![2, 2])).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    // An input the session does not capture is refused at the run, before anything plays.
    let uncaptured = create(
        &mut c,
        "uncaptured",
        SweepConfig {
            measurement_inputs: vec![1, 3],
            ..request(LEVEL)
        },
    );
    arm(&mut c, token);
    let e = c
        .call(Command::SweepRun {
            lease_token: token,
            meas: uncaptured,
            name: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    assert!(
        e.msg.contains("measurement input 4 is not captured"),
        "{}",
        e.msg
    );
    let st = state(&mut c);
    assert!(!st.generator.firing, "nothing played");
}
