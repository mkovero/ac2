//! Trace store and sessions on the fake rig: capture keeps the shown result with its
//! metadata, averaging and A−B math use the shared time base, export re-imports exactly,
//! imports refuse bad files with typed errors, and a session reload comes up disarmed in a
//! new epoch with every measurement and trace as saved.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::time::Duration;

use ac2_audio::FakeDriver;
use ac2_proto::frame::FrameData;
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ErrorDetail, ImportProblem, ReplyBody};
use ac2d::Daemon;
use common::*;

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
        ReplyBody::TraceData(d) => d,
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
fn settle(d: &mut FakeDriver, c: &mut Client, sub: &Sub, tok: LeaseToken, rev: u64) {
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
    .expect("settled tf frame");
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

#[test]
fn capture_average_math_export_import() {
    let mut r = rig("traces");
    let c = &mut r.c;
    let gain_db = 20.0 * f32::log10(ACOUSTIC_GAIN);
    let epoch = state(c).session.epoch;

    // Unaligned capture into slot 1.
    let raw = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "raw".into(),
        slot: Some(1),
    }));
    assert_eq!(raw.kind, TraceKind::Transfer);
    assert_eq!(
        raw.source,
        TraceSource::Captured {
            meas: MeasId(1),
            meas_name: "main".into(),
            epoch,
            at_sample: match raw.source {
                TraceSource::Captured { at_sample, .. } => at_sample,
                _ => unreachable!(),
            },
        }
    );
    assert_eq!(raw.delay, Seconds(0.0));
    assert_eq!(raw.depth, Some(DepthPolicy::EqualConfidence));
    assert_eq!(raw.cal, CalState::Uncalibrated);
    assert_eq!(raw.edit.slot, Some(1));
    let rd = data(c, raw.id);
    for i in band() {
        assert!(
            (rd.mag_db[i] - gain_db).abs() < 0.3,
            "{} Hz: {}",
            grid_freq(i),
            rd.mag_db[i]
        );
    }

    // Aligned capture into slot 2.
    let delay = f64::from(ACOUSTIC_DELAY) / f64::from(FS);
    let rev = match c.ok(Command::DelaySet {
        meas: MeasId(1),
        delay: Seconds(delay),
    }) {
        ReplyBody::Measurement(m) => m.config_rev.0,
        other => panic!("{other:?}"),
    };
    settle(&mut r.d, c, &r.sub, r.tok, rev);
    let aligned = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "aligned".into(),
        slot: Some(2),
    }));
    assert!((aligned.delay.0 - delay).abs() < 1e-12);

    // Complex average referred to the aligned trace's delay: both describe the same
    // arrival, so the average is the path itself with flat phase.
    let avg = trace(c.ok(Command::TraceAverage {
        traces: vec![raw.id, aligned.id],
        method: AverageMethod::Complex,
        reference: DelayReference::Trace { trace: aligned.id },
        name: "avg".into(),
    }));
    assert!(matches!(avg.source, TraceSource::Average { .. }));
    assert!((avg.delay.0 - delay).abs() < 1e-12);
    let ad = data(c, avg.id);
    let ap = ad.phase_deg.as_ref().unwrap();
    for i in band() {
        assert!((ad.mag_db[i] - gain_db).abs() < 0.5, "{}", ad.mag_db[i]);
        assert!(ap[i].abs() < 5.0, "{} Hz: {}", grid_freq(i), ap[i]);
    }

    // A / B on the shared time base: same path, so 0 dB and 0°.
    let q = trace(c.ok(Command::TraceMath {
        a: aligned.id,
        b: raw.id,
        op: MathOp::ComplexDivision,
        name: "q".into(),
    }));
    let qd = data(c, q.id);
    for i in band() {
        assert!(qd.mag_db[i].abs() < 0.5, "{}", qd.mag_db[i]);
        assert!(qd.phase_deg.as_ref().unwrap()[i].abs() < 5.0);
    }
    let diff = trace(c.ok(Command::TraceMath {
        a: aligned.id,
        b: raw.id,
        op: MathOp::MagnitudeDifference,
        name: "diff".into(),
    }));
    assert!(data(c, diff.id).phase_deg.is_none());

    // A capture into slot 1 takes the slot from "raw".
    let again = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "again".into(),
        slot: Some(1),
    }));
    let all = traces(c);
    let slot_of = |id| all.iter().find(|t| t.id == id).unwrap().edit.slot;
    assert_eq!(slot_of(raw.id), None);
    assert_eq!(slot_of(again.id), Some(1));
    assert_eq!(slot_of(aligned.id), Some(2));

    // Locks protect the curve and deletion, not visibility.
    let mut e = raw.edit.clone();
    e.slot = None;
    e.locked = true;
    c.ok(Command::TraceUpdate {
        trace: raw.id,
        edit: e.clone(),
    });
    let refused = c.call(Command::TraceDelete { trace: raw.id }).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Refused);
    let mut renamed = e.clone();
    renamed.name = "other".into();
    let refused = c
        .call(Command::TraceUpdate {
            trace: raw.id,
            edit: renamed,
        })
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Refused);
    let mut hidden = e.clone();
    hidden.visible = false;
    c.ok(Command::TraceUpdate {
        trace: raw.id,
        edit: hidden,
    });

    // Export → import is exact on the same grid; the copy is independent.
    let (file_name, content) = match c.ok(Command::TraceExport {
        trace: aligned.id,
        format: ExportFormat::Ac2Csv,
    }) {
        ReplyBody::Export { file_name, content } => (file_name, content),
        other => panic!("{other:?}"),
    };
    assert_eq!(file_name, "aligned.csv");
    let csv = String::from_utf8(content.0.clone()).unwrap();
    assert!(
        csv.starts_with("# ac2 trace export v1\n# name: aligned\n"),
        "{csv}"
    );
    assert!(csv.contains("# source: captured from \"main\" (measurement 1)"));
    let imp = trace(c.ok(Command::TraceImport {
        file_name: file_name.clone(),
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        content,
    }));
    assert_eq!(imp.grid_id, aligned.grid_id);
    assert_eq!(imp.edit.name, "aligned");
    assert!(matches!(
        imp.source,
        TraceSource::Imported {
            format: ImportFormat::Ac2Csv,
            ..
        }
    ));
    let (x, y) = (data(c, imp.id), data(c, aligned.id));
    let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&x.mag_db), bits(&y.mag_db));
    assert_eq!(
        bits(x.phase_deg.as_ref().unwrap()),
        bits(y.phase_deg.as_ref().unwrap())
    );

    // A target curve: magnitude only.
    let tgt = trace(c.ok(Command::TraceImport {
        file_name: "house_curve.txt".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Target,
        content: Blob(fixture("house_curve.txt")),
    }));
    assert_eq!(tgt.kind, TraceKind::Target);
    assert_eq!(tgt.edit.name, "house_curve");
    let td = data(c, tgt.id);
    assert!(td.phase_deg.is_none() && td.coherence.is_none());
    // Phase methods refuse a mix of time bases; complex division needs phase.
    let e = c
        .call(Command::TraceAverage {
            traces: vec![aligned.id, imp.id],
            method: AverageMethod::Complex,
            reference: DelayReference::Trace { trace: aligned.id },
            name: "x".into(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    let e = c
        .call(Command::TraceMath {
            a: aligned.id,
            b: tgt.id,
            op: MathOp::ComplexDivision,
            name: "x".into(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);

    // Typed refusal of a bad file.
    let e = c
        .call(Command::TraceImport {
            file_name: "bad.txt".into(),
            format: ImportFormat::AnalyzerText,
            role: ImportRole::Trace,
            content: Blob(b"* REW\n20 1\n30 2\n25 3\n".to_vec()),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert_eq!(
        e.detail,
        Some(ErrorDetail::Import {
            line: Some(4),
            problem: ImportProblem::NotAscending
        })
    );
    // Unknown ids.
    let e = c
        .call(Command::TraceGet {
            trace: TraceId(999),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    drop(r.backend);
    r.h.shutdown();
}

#[test]
fn session_save_load_round_trip() {
    let mut r = rig("sessions");
    let c = &mut r.c;
    let a = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "pre".into(),
        slot: Some(3),
    }));
    let tgt = trace(c.ok(Command::TraceImport {
        file_name: "house_curve.txt".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Target,
        content: Blob(fixture("house_curve.txt")),
    }));
    let mut e = tgt.edit.clone();
    e.visible = false;
    e.offset = Db(-2.0);
    c.ok(Command::TraceUpdate {
        trace: tgt.id,
        edit: e,
    });
    c.ok(Command::DelaySet {
        meas: MeasId(1),
        delay: Seconds(0.0025),
    });
    let saved = state(c);
    let saved_data: Vec<TraceData> = saved.traces.iter().map(|t| data(c, t.id)).collect();
    let info = match c.ok(Command::FileSave {
        session: SessionRef::Name {
            name: "friday show".into(),
        },
    }) {
        ReplyBody::SessionFile(f) => f,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        (info.name.as_str(), info.measurements, info.traces),
        ("friday show", 1, 2)
    );

    // Change things, then load.
    c.ok(Command::TraceDelete { trace: a.id });
    c.ok(Command::MeasCreate {
        config: transfer("other"),
    });
    match c.ok(Command::FileList) {
        ReplyBody::Sessions(l) => {
            assert_eq!(l.len(), 1);
            assert_eq!(l[0].name, "friday show");
        }
        other => panic!("{other:?}"),
    }
    let before = state(c);
    assert!(before.generator.armed && before.generator.owner.is_some());
    match c.ok(Command::FileLoad {
        session: SessionRef::Name {
            name: "friday show".into(),
        },
    }) {
        ReplyBody::SessionFile(f) => assert_eq!(f.traces, 2),
        other => panic!("{other:?}"),
    }
    let after = state(c);
    // Disarmed, no owner, new epoch; the old lease is gone.
    assert!(!after.generator.armed && !after.generator.firing);
    assert_eq!(after.generator.owner, None);
    assert!(after.session.epoch > before.session.epoch);
    assert!(after.session.open.is_some());
    let e = c
        .call(Command::GenRefresh { lease_token: r.tok })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::LeaseRequired);
    // Measurements and traces as saved.
    assert_eq!(after.measurements.len(), 1);
    let (m0, s0) = (&after.measurements[0], &saved.measurements[0]);
    assert_eq!(
        (m0.id, &m0.config, m0.running),
        (s0.id, &s0.config, s0.running)
    );
    assert_eq!(m0.delay.as_ref().unwrap().applied, Seconds(0.0025));
    assert_eq!(after.traces, saved.traces);
    for (t, d) in saved.traces.iter().zip(&saved_data) {
        let back = data(c, t.id);
        let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&back.mag_db), bits(&d.mag_db));
        assert_eq!(back.meta, d.meta);
    }

    // The restarted measurement runs in the new epoch; a fresh capture there is not in the
    // loaded trace's time base.
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
    let mut d = driver(&r.backend);
    settle(&mut d, c, &r.sub, tok, 0);
    let fresh = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "post".into(),
        slot: None,
    }));
    let epoch_of = |t: &TraceMeta| match t.source {
        TraceSource::Captured { epoch, .. } => epoch,
        _ => unreachable!(),
    };
    assert!(epoch_of(&fresh) > epoch_of(&a));
    assert!(fresh.id.0 > tgt.id.0, "ids continue after the loaded ones");

    // Refusals leave the state alone.
    let n = traces(c).len();
    let dir = r._dir.path().join("sessions").join("friday show");
    let manifest = dir.join("session.json");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, text.replace("\"version\": 1", "\"version\": 7")).unwrap();
    let e = c
        .call(Command::FileLoad {
            session: SessionRef::Name {
                name: "friday show".into(),
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    assert_eq!(
        e.detail,
        Some(ErrorDetail::SessionVersion {
            found: 7,
            supported: 1
        })
    );
    assert_eq!(traces(c).len(), n);
    for bad in [
        SessionRef::Name {
            name: "../escape".into(),
        },
        SessionRef::Path {
            path: "relative/dir".into(),
        },
    ] {
        let e = c.call(Command::FileSave { session: bad }).unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    }
    let e = c
        .call(Command::FileLoad {
            session: SessionRef::Name {
                name: "missing".into(),
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    // An absolute path works for a local client.
    let p = r._dir.path().join("by-path");
    match c.ok(Command::FileSave {
        session: SessionRef::Path {
            path: p.to_string_lossy().into_owned(),
        },
    }) {
        ReplyBody::SessionFile(f) => assert_eq!(f.name, "by-path"),
        other => panic!("{other:?}"),
    }
    r.h.shutdown();
}
