//! Trace store and sessions on the fake rig: capture keeps the shown result with its
//! metadata, averaging and math channels of stored traces use the shared time base, export re-imports exactly,
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

    // A ÷ B on the shared time base: same path, so 0 dB and 0°.
    let q = math_capture(&mut r, aligned.id, MathOp::Divide, raw.id, "q");
    let c = &mut r.c;
    assert!(matches!(
        &q.source,
        TraceSource::Math {
            phase: PhaseBasis::SharedTimeBase,
            ..
        }
    ));
    let qd = data(c, q.id);
    for i in band() {
        assert!(qd.mag_db[i].abs() < 0.5, "{}", qd.mag_db[i]);
        assert!(qd.phase_deg.as_ref().unwrap()[i].abs() < 5.0);
    }

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
        csv.starts_with("# ac2 trace export v3\n# name: aligned\n"),
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
    // Phase methods refuse a mix of time bases; a sum needs phase.
    let e = c
        .call(Command::TraceAverage {
            traces: vec![aligned.id, imp.id],
            method: AverageMethod::Complex,
            reference: DelayReference::Trace { trace: aligned.id },
            name: "x".into(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    for (b, op) in [(tgt.id, MathOp::Add), (imp.id, MathOp::Subtract)] {
        let e = c
            .call(Command::MeasCreate {
                config: MeasConfig {
                    name: "x".into(),
                    kind: MeasKind::Math {
                        config: MathConfig::of(
                            ac2_proto::model::TraceOwner::Imported,
                            MathDomain::Transfer,
                            MathExpr::Binary {
                                a: Operand::Trace { trace: aligned.id },
                                op,
                                b: Operand::Trace { trace: b },
                            },
                        ),
                    },
                },
            })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{op:?}: {e:?}");
    }

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

fn sixth() -> Smoothing {
    Smoothing {
        fraction: SmoothingFraction::Sixth,
        mode: SmoothingMode::MagnitudePhase,
    }
}

/// `raw` (as captured: unsmoothed, NaN gaps) smoothed by ac2-core directly: magnitude dB and
/// phase degrees.
fn core_smoothed(raw: &TraceData, s: Smoothing) -> (Vec<f64>, Vec<f64>) {
    use ac2_core::smoothing as sm;
    let grid = ac2_core::grid::LogGrid {
        ppo: 48,
        k_min: -240,
        k_max: 239,
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
        SmoothingFraction::Sixth => sm::SmoothingFraction::Sixth,
        other => unimplemented!("{other:?}"),
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

/// Mean squared second difference over the band: how rough a curve is.
fn roughness(v: &[f32]) -> f64 {
    let b = band();
    b.windows(3)
        .map(|w| {
            let d = f64::from(v[w[0]]) - 2.0 * f64::from(v[w[1]]) + f64::from(v[w[2]]);
            d * d
        })
        .sum::<f64>()
        / b.len() as f64
}

fn set_smoothing(c: &mut Client, t: &TraceMeta, s: Option<Smoothing>) -> TraceMeta {
    let mut e = t.edit.clone();
    e.smoothing = s;
    trace(c.ok(Command::TraceUpdate {
        trace: t.id,
        edit: e,
    }))
}

/// Smoothing changes on a running transfer measurement without restarting its averages;
/// captures keep the unsmoothed curve, are served smoothed as the live curve was, and can be
/// re-smoothed at any time; averages combine the unsmoothed columns.
#[test]
fn live_smoothing_and_resmoothed_captures() {
    let mut r = rig("smoothing");
    let c = &mut r.c;
    let settled = settle(&mut r.d, c, &r.sub, r.tok, 0);
    // Restarted averages would leave the deep (low-frequency) stages settling again: only
    // 0.1 s of audio runs after the change.
    let settling = |f: &ac2_proto::Frame| match &f.data {
        FrameData::Tf(t) => t
            .validity
            .iter()
            .filter(|m| m.contains(ac2_proto::frame::ValidityMask::SETTLING))
            .count(),
        _ => unreachable!(),
    };
    let settling_before = settling(&settled);

    // Live change: same job, averages kept, new rev, frames say so.
    let mut cfg = transfer("main");
    if let MeasKind::Transfer { config } = &mut cfg.kind {
        config.smoothing = Some(sixth());
    }
    let m = match c.ok(Command::MeasUpdate {
        meas: MeasId(1),
        config: cfg,
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    assert!(m.running);
    run(&mut r.d, 0.1);
    c.ok(Command::GenRefresh { lease_token: r.tok });
    let first = r
        .sub
        .frame(T, |f| {
            matches!(f.data, FrameData::Tf(_)) && f.stamp.config_rev.0 >= m.config_rev.0
        })
        .expect("frame with the new smoothing");
    let FrameData::Tf(tf) = &first.data else {
        unreachable!()
    };
    assert_eq!(tf.meta.smoothing, Some(sixth()));
    assert!(
        settling(&first) <= settling_before,
        "averages restarted: {} columns settling, {settling_before} before",
        settling(&first)
    );
    let shown = settle(&mut r.d, c, &r.sub, r.tok, m.config_rev.0);
    let FrameData::Tf(shown) = shown.data else {
        unreachable!()
    };

    // The capture shows what the live curve showed…
    let a = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "a".into(),
        slot: Some(1),
    }));
    assert_eq!(a.edit.smoothing, Some(sixth()));
    let smoothed = data(c, a.id);
    for i in band() {
        assert!(
            (smoothed.mag_db[i] - shown.mag[i]).abs() < 1e-3,
            "{} Hz: {} vs live {}",
            grid_freq(i),
            smoothed.mag_db[i],
            shown.mag[i]
        );
    }
    // …but holds it unsmoothed: switched off, the curve is rougher; switched back, the same.
    let a = set_smoothing(c, &a, None);
    let raw = data(c, a.id);
    // The live frame's magnitude and phase are the raw columns smoothed by ac2-core: phase
    // is smoothed by default, and the capture holds exactly what the frame was made from.
    let (want_mag, want_phase) = core_smoothed(&raw, sixth());
    let mut moved: f64 = 0.0;
    let mut compared = 0;
    for i in 0..raw.mag_db.len() {
        if !(shown.phase[i].is_finite() && raw.phase_deg.as_ref().unwrap()[i].is_finite()) {
            continue;
        }
        compared += 1;
        let p = f64::from(shown.phase[i]);
        assert!(
            deg_diff(p, want_phase[i]) < 0.05,
            "{} Hz: live phase {p} vs ac2-core {}",
            grid_freq(i),
            want_phase[i]
        );
        assert!((f64::from(shown.mag[i]) - want_mag[i]).abs() < 1e-3);
        moved = moved.max(deg_diff(p, f64::from(raw.phase_deg.as_ref().unwrap()[i])));
    }
    assert!(compared > 300, "{compared} columns compared");
    assert!(
        moved > 1.0,
        "phase was not smoothed: at most {moved}° from raw"
    );
    assert!(
        roughness(&raw.mag_db) > 2.0 * roughness(&smoothed.mag_db),
        "{} vs {}",
        roughness(&raw.mag_db),
        roughness(&smoothed.mag_db)
    );
    let a = set_smoothing(c, &a, Some(sixth()));
    let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&data(c, a.id).mag_db), bits(&smoothed.mag_db));

    // A second capture of the same result; their average uses the unsmoothed columns and
    // starts with the smoothing the inputs share.
    let b = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "b".into(),
        slot: Some(2),
    }));
    let avg = trace(c.ok(Command::TraceAverage {
        traces: vec![a.id, b.id],
        method: AverageMethod::Power,
        reference: DelayReference::Trace { trace: a.id },
        name: "avg".into(),
    }));
    assert_eq!(avg.edit.smoothing, Some(sixth()));
    let avg = set_smoothing(c, &avg, None);
    let ad = data(c, avg.id);
    for i in band() {
        assert!(
            (ad.mag_db[i] - raw.mag_db[i]).abs() < 1e-3,
            "{}",
            ad.mag_db[i]
        );
    }

    // Smoothing is a protected edit and applies to transfer curves only.
    let mut locked = a.edit.clone();
    locked.locked = true;
    c.ok(Command::TraceUpdate {
        trace: a.id,
        edit: locked.clone(),
    });
    locked.smoothing = None;
    let e = c
        .call(Command::TraceUpdate {
            trace: a.id,
            edit: locked,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    let tgt = trace(c.ok(Command::TraceImport {
        file_name: "house_curve.txt".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Target,
        content: Blob(fixture("house_curve.txt")),
    }));
    let mut te = tgt.edit.clone();
    te.smoothing = Some(sixth());
    let e = c
        .call(Command::TraceUpdate {
            trace: tgt.id,
            edit: te,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    drop(r.backend);
    r.h.shutdown();
}

/// A narrowband spectrum's smoothing changes live; frames say it is applied; a capture
/// holds the unsmoothed bins, starts with the measurement's smoothing and is served as the
/// live frame showed it; the live frame is the ac2-core linear-bin smoother of the raw bins.
#[test]
fn spectrum_smoothing_live_and_captured() {
    let mut r = rig("spec-smoothing");
    let (mut c2, sub) = connect(&r.h, &[b"d/2/spec"]);
    let spec = |smoothing| MeasConfig {
        name: "fft".into(),
        kind: MeasKind::Spectrum {
            config: SpectrumConfig {
                fft_len: 4096,
                smoothing,
                ..SpectrumConfig::on_input(1)
            },
        },
    };
    c2.ok(Command::MeasCreate { config: spec(None) });
    c2.ok(Command::MeasStart { meas: MeasId(2) });
    run(&mut r.d, 0.5);
    let m = match c2.ok(Command::MeasUpdate {
        meas: MeasId(2),
        config: spec(Some(SmoothingFraction::Sixth)),
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    assert!(m.running);
    for _ in 0..3 {
        run(&mut r.d, 0.3);
        r.c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    let end = end_sample(&r.d);
    let f = sub
        .frame(T, |f| {
            matches!(f.data, FrameData::Spec(_))
                && f.stamp.config_rev.0 >= m.config_rev.0
                && f.stamp.audio_sample.0 + 1 >= end
        })
        .expect("smoothed spec frame");
    let FrameData::Spec(live) = f.data else {
        unreachable!()
    };
    assert_eq!(live.meta.smoothing, Some(SmoothingFraction::Sixth));

    let t = trace(c2.ok(Command::TraceCapture {
        meas: MeasId(2),
        name: "fft".into(),
        slot: None,
    }));
    let power = Smoothing {
        fraction: SmoothingFraction::Sixth,
        mode: SmoothingMode::Magnitude,
    };
    assert_eq!(t.edit.smoothing, Some(power));
    // Every bin, on the FFT's own grid.
    let bins = ac2_proto::GridDef::Linear {
        fs: Hz(f64::from(FS)),
        n: 4096,
    };
    assert_eq!(t.grid_id, bins.id());
    let served = data(&mut c2, t.id);
    let t = set_smoothing(&mut c2, &t, None);
    let raw = data(&mut c2, t.id);
    let p: Vec<f64> = raw
        .mag_db
        .iter()
        .map(|l| 10f64.powf(f64::from(*l) / 10.0))
        .collect();
    let valid: Vec<bool> = raw.mag_db.iter().map(|l| l.is_finite()).collect();
    let want = ac2_core::smoothing::LinearSmoother::new(
        raw.mag_db.len(),
        ac2_core::smoothing::SmoothingFraction::Sixth,
    )
    .smooth(&p, &valid);
    // The live frame is the smoothed bins gathered into display columns, each the highest
    // level among its bins; the capture keeps every bin and smooths to the same curve.
    let cols = ac2_proto::BinColumns::new(f64::from(FS), 4096, 96);
    let log_bins = ac2_proto::GridDef::LogBins {
        fs: Hz(f64::from(FS)),
        n: 4096,
        ppo: 96,
    };
    assert_eq!(f.stamp.grid_id, Some(log_bins.id()));
    assert_eq!(live.level.len(), cols.len());
    assert_eq!(raw.mag_db.len(), 2049);
    for (c, w) in cols.first_bin.windows(2).enumerate() {
        let bins = w[0] as usize..w[1] as usize;
        let top = |v: &dyn Fn(usize) -> f64| {
            bins.clone()
                .filter(|&k| k > 0 && valid[k])
                .map(v)
                .fold(f64::NAN, f64::max)
        };
        let want_c = top(&|k| 10.0 * want[k].log10());
        if want_c.is_nan() {
            continue;
        }
        assert!(
            (f64::from(live.level[c]) - want_c).abs() < 1e-3,
            "column {c}: live {} vs ac2-core {want_c}",
            live.level[c]
        );
        let served_c = top(&|k| f64::from(served.mag_db[k]));
        assert!((served_c - want_c).abs() < 1e-3, "column {c}");
    }
    let (mut rough_raw, mut rough_live) = (0.0, 0.0);
    for k in 101..raw.mag_db.len() {
        if valid[k] && valid[k - 1] {
            rough_raw += f64::from(raw.mag_db[k] - raw.mag_db[k - 1]).powi(2);
            rough_live += f64::from(served.mag_db[k] - served.mag_db[k - 1]).powi(2);
        }
    }
    assert!(rough_live * 10.0 < rough_raw, "{rough_live} vs {rough_raw}");

    // Re-smoothed at another width; either mode, as a spectrum has no phase.
    let third = Smoothing {
        fraction: SmoothingFraction::Third,
        mode: SmoothingMode::MagnitudePhase,
    };
    let t = set_smoothing(&mut c2, &t, Some(third));
    assert_ne!(data(&mut c2, t.id).mag_db, served.mag_db);
    drop(r.backend);
    r.h.shutdown();
}

/// A default 65 536-point spectrum publishes display columns, each the highest bin level in
/// it, and captures every bin: the capture of the last frame holds exactly the bins the
/// frame's columns were gathered from, and a tone between bins keeps its level live.
#[test]
fn spectrum_live_columns_are_the_captured_bins_maxima() {
    let mut r = rig("spec-columns");
    let (mut c2, sub) = connect(&r.h, &[b"d/2/spec"]);
    r.c.ok(Command::GenSet {
        lease_token: r.tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Sine { freq: Hz(1234.5) },
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    c2.ok(Command::MeasCreate {
        config: MeasConfig {
            name: "fft".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig::on_input(1),
            },
        },
    });
    c2.ok(Command::MeasStart { meas: MeasId(2) });
    for _ in 0..4 {
        run(&mut r.d, 0.5);
        r.c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    // No more audio: the last frame published is the one a capture stores.
    let mut last = None;
    while let Some(f) = sub.frame(Duration::from_millis(500), |f| {
        matches!(f.data, FrameData::Spec(_))
    }) {
        last = Some(f);
    }
    let f = last.expect("spec frame");
    let FrameData::Spec(live) = f.data else {
        unreachable!()
    };
    let t = trace(c2.ok(Command::TraceCapture {
        meas: MeasId(2),
        name: "fft".into(),
        slot: None,
    }));
    let bins = data(&mut c2, t.id).mag_db;
    assert_eq!(bins.len(), 32_769);
    let cols = ac2_proto::BinColumns::new(f64::from(FS), 65_536, 96);
    assert_eq!(live.level.len(), cols.len());
    for (c, w) in cols.first_bin.windows(2).enumerate() {
        let top = bins[w[0] as usize..w[1] as usize]
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .fold(f32::NAN, f32::max);
        assert!(
            live.level[c] == top || (live.level[c].is_nan() && top.is_nan()),
            "column {c}: {} vs {top}",
            live.level[c]
        );
    }
    // The tone: its peak bin, half a bin off centre, reads the same live.
    let peak = |v: &[f32]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    assert_eq!(peak(&live.level), peak(&bins));
    assert!(peak(&bins) > -40.0, "{}", peak(&bins));
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
    // Display smoothing is an edit: saved with the trace, applied again after the load.
    let mut ae = a.edit.clone();
    ae.smoothing = Some(Smoothing {
        fraction: SmoothingFraction::Third,
        mode: SmoothingMode::MagnitudePhase,
    });
    c.ok(Command::TraceUpdate {
        trace: a.id,
        edit: ae,
    });
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
    // Owners travel with the session: a math channel under the transfer measurement, a
    // sweep measurement (settings only) owning nothing yet.
    c.ok(Command::MeasCreate {
        config: MeasConfig {
            name: "pre ÷ target".into(),
            kind: MeasKind::Math {
                config: MathConfig::of(
                    TraceOwner::Meas { meas: MeasId(1) },
                    MathDomain::Transfer,
                    MathExpr::Binary {
                        a: Operand::Trace { trace: a.id },
                        op: MathOp::Divide,
                        b: Operand::Trace { trace: tgt.id },
                    },
                ),
            },
        },
    });
    c.ok(Command::MeasCreate {
        config: MeasConfig {
            name: "Genelec 1 m".into(),
            kind: MeasKind::Sweep {
                config: SweepConfig {
                    reference_input: 0,
                    measurement_input: 1,
                    outputs: vec![0],
                    level: Dbfs(-50.0),
                    sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
                    repeats: 1,
                    gate: None,
                    tail: Some(Seconds(2.0)),
                },
            },
        },
    });
    assert_eq!(a.edit.owner, TraceOwner::Meas { meas: MeasId(1) });
    assert_eq!(tgt.edit.owner, TraceOwner::Imported);
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
        ("friday show", 3, 2)
    );

    // Change things, then load.
    c.ok(Command::MeasDelete {
        meas: MeasId(2),
        traces: OwnedTraces::Delete,
    });
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
    // Measurements and traces as saved, owners included.
    assert_eq!(after.measurements.len(), 3);
    for (m0, s0) in after.measurements.iter().zip(&saved.measurements) {
        assert_eq!(
            (m0.id, &m0.config, m0.running),
            (s0.id, &s0.config, s0.running)
        );
    }
    let m0 = &after.measurements[0];
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
    std::fs::write(
        &manifest,
        text.replace("\"version\": 17", "\"version\": 18"),
    )
    .unwrap();
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
            found: 18,
            supported: 17
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

/// Who owns what: a capture is filed under its measurement, a math channel's capture under
/// the math channel's owner, an average with its inputs' common owner; a trace moves by
/// `trace.update`, a math channel by `meas.update`; deleting a measurement keeps what it
/// owns (in the imported group) or deletes it, and refuses while a math channel that stays
/// would lose an operand.
#[test]
fn owners_capture_move_and_delete() {
    let mut r = rig("owners");
    let c = &mut r.c;
    let main = TraceOwner::Meas { meas: MeasId(1) };
    let cap = |c: &mut Client, meas: MeasId, name: &str| {
        trace(c.ok(Command::TraceCapture {
            meas,
            name: name.into(),
            slot: None,
        }))
    };
    let pre = cap(c, MeasId(1), "pre-EQ");
    let post = cap(c, MeasId(1), "post-EQ");
    assert_eq!((pre.edit.owner, post.edit.owner), (main, main));

    // A math channel made on the measurement: listed and captured under it.
    let math = match c.ok(Command::MeasCreate {
        config: MeasConfig {
            name: "pre ÷ post".into(),
            kind: MeasKind::Math {
                config: MathConfig::of(
                    main,
                    MathDomain::Transfer,
                    MathExpr::Binary {
                        a: Operand::Trace { trace: pre.id },
                        op: MathOp::Divide,
                        b: Operand::Trace { trace: post.id },
                    },
                ),
            },
        },
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    c.ok(Command::MeasStart { meas: math.id });
    for _ in 0..2 {
        run(&mut r.d, 0.5);
        c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    let captured = cap(c, math.id, "pre ÷ post 21:04");
    assert_eq!(captured.edit.owner, main);
    let avg = trace(c.ok(Command::TraceAverage {
        traces: vec![pre.id, post.id],
        method: AverageMethod::Power,
        reference: DelayReference::Trace { trace: pre.id },
        name: "avg".into(),
    }));
    assert_eq!(avg.edit.owner, main);

    // Owners must exist and must not be math channels; math cannot own math.
    let mv = |c: &mut Client, t: &TraceMeta, owner: TraceOwner| {
        let mut e = t.edit.clone();
        e.owner = owner;
        c.call(Command::TraceUpdate {
            trace: t.id,
            edit: e,
        })
    };
    let e = mv(c, &avg, TraceOwner::Meas { meas: MeasId(99) }).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound, "{}", e.msg);
    let e = mv(c, &avg, TraceOwner::Meas { meas: math.id }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{}", e.msg);
    let avg = trace(mv(c, &avg, TraceOwner::Imported).unwrap());
    assert_eq!(avg.edit.owner, TraceOwner::Imported);
    let mut under_math = MathConfig::of(
        TraceOwner::Meas { meas: math.id },
        MathDomain::Transfer,
        MathExpr::Binary {
            a: Operand::Trace { trace: pre.id },
            op: MathOp::Divide,
            b: Operand::Trace { trace: avg.id },
        },
    );
    let e = c
        .call(Command::MeasCreate {
            config: MeasConfig {
                name: "nested".into(),
                kind: MeasKind::Math {
                    config: under_math.clone(),
                },
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{}", e.msg);

    // A math channel moves without restarting: its config rev stays.
    let MeasKind::Math { config } = &math.config.kind else {
        unreachable!()
    };
    let moved = match c.ok(Command::MeasUpdate {
        meas: math.id,
        config: MeasConfig {
            name: math.config.name.clone(),
            kind: MeasKind::Math {
                config: MathConfig {
                    owner: TraceOwner::Imported,
                    ..config.clone()
                },
            },
        },
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    let before_rev = state(c)
        .measurements
        .iter()
        .find(|m| m.id == math.id)
        .unwrap()
        .config_rev;
    assert_eq!(moved.config_rev, before_rev);
    assert!(moved.running);
    // Back under the measurement for the delete below.
    c.ok(Command::MeasUpdate {
        meas: math.id,
        config: math.config.clone(),
    });

    // A second measurement and a math channel elsewhere computing from one of its traces.
    c.ok(Command::MeasCreate {
        config: transfer("side"),
    });
    let side = state(c).measurements.iter().map(|m| m.id).max().unwrap();
    c.ok(Command::MeasStart { meas: side });
    for _ in 0..2 {
        run(&mut r.d, 0.5);
        c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    let s1 = cap(c, side, "side 1");
    under_math.owner = TraceOwner::Imported;
    under_math.expr = MathExpr::Binary {
        a: Operand::Trace { trace: s1.id },
        op: MathOp::Divide,
        b: Operand::Trace { trace: avg.id },
    };
    under_math.reference = MathReference::Operand {
        operand: Operand::Trace { trace: s1.id },
    };
    let user = match c.ok(Command::MeasCreate {
        config: MeasConfig {
            name: "side ÷ avg".into(),
            kind: MeasKind::Math { config: under_math },
        },
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    // Deleting side with its traces would take an operand from a math channel that stays.
    let e = c
        .call(Command::MeasDelete {
            meas: side,
            traces: OwnedTraces::Delete,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    assert!(e.msg.contains("side ÷ avg"), "{}", e.msg);
    // Kept, they move to the imported group and the math channel keeps its operand.
    c.ok(Command::MeasDelete {
        meas: side,
        traces: OwnedTraces::Keep,
    });
    let st = state(c);
    assert_eq!(
        st.traces.iter().find(|t| t.id == s1.id).unwrap().edit.owner,
        TraceOwner::Imported
    );
    c.ok(Command::MeasDelete {
        meas: user.id,
        traces: OwnedTraces::Keep,
    });

    // The main measurement with its traces: the math channel under it goes too.
    c.ok(Command::MeasDelete {
        meas: MeasId(1),
        traces: OwnedTraces::Delete,
    });
    let st = state(c);
    assert!(
        st.measurements
            .iter()
            .all(|m| m.id != MeasId(1) && m.id != math.id)
    );
    let left: Vec<&str> = st.traces.iter().map(|t| t.edit.name.as_str()).collect();
    assert_eq!(left, ["avg", "side 1"]);
}

/// A mic curve put on a trace captured without one: the served magnitude is corrected by
/// the store's curve (0 dB at 1 kHz), the columns stay as measured (export, removal), the
/// curve survives a session reload; a capture that already has the curve refuses a second.
#[test]
fn mic_curve_on_a_stored_trace() {
    // +6 dB above 8 kHz, flat up to 4 kHz.
    const CURVE: &str = "* test mic\n20 0\n1000 0\n4000 0\n8000 6\n24000 6\n";
    let mut r = rig("trace-mic");
    let c = &mut r.c;
    let raw = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "before cal".into(),
        slot: None,
    }));
    assert_eq!(raw.mic, None);
    let raw_data = data(c, raw.id);
    // No curve for that mic yet.
    let m30 = || {
        Some(ac2_proto::model::MicCurveId {
            mic: "M30".into(),
            label: "M30".into(),
        })
    };
    let e = c
        .call(Command::TraceMicCurve {
            trace: raw.id,
            curve: m30(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound, "{e:?}");
    c.ok(Command::CalCurveImport {
        mic: "M30".into(),
        label: None,
        file_name: "M30.frd".into(),
        content: Blob(CURVE.as_bytes().to_vec()),
        input: Some(1),
    });
    let t = trace(c.ok(Command::TraceMicCurve {
        trace: raw.id,
        curve: m30(),
    }));
    let mc = t.mic_curve.clone().unwrap();
    assert_eq!((mc.mic.as_str(), mc.curve.label.as_str()), ("M30", "M30"));
    assert_eq!(mc.f_norm, Hz(1000.0));
    let d = data(c, raw.id);
    for i in 0..480 {
        let f = grid_freq(i);
        let want = if f <= 4000.0 {
            0.0
        } else if f >= 8000.0 {
            6.0
        } else {
            6.0 * (f / 4000.0).log2()
        };
        let (a, b) = (raw_data.mag_db[i], d.mag_db[i]);
        if a.is_finite() {
            assert!((f64::from(a - b) - want).abs() < 1e-3, "{f} Hz: {a} → {b}");
        }
    }
    let bits = |v: &Option<Vec<f32>>| {
        v.as_ref()
            .map(|v| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>())
    };
    assert_eq!(bits(&d.phase_deg), bits(&raw_data.phase_deg));
    // The export keeps the measured columns and names the curve.
    let csv = match c.ok(Command::TraceExport {
        trace: raw.id,
        format: ExportFormat::Ac2Csv,
    }) {
        ReplyBody::Export { content, .. } => String::from_utf8(content.0).unwrap(),
        other => panic!("{other:?}"),
    };
    assert!(
        csv.contains("# mic: M30 (curve: M30, applied after capture as a display edit"),
        "{}",
        &csv[..1200]
    );
    // A capture taken now has the curve in its columns: no second correction.
    let with = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "after cal".into(),
        slot: None,
    }));
    assert_eq!(
        with.mic
            .as_ref()
            .and_then(|m| m.curve.as_ref())
            .map(|c| c.label.as_str()),
        Some("M30")
    );
    let e = c
        .call(Command::TraceMicCurve {
            trace: with.id,
            curve: m30(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    assert!(e.msg.contains("twice"), "{}", e.msg);
    // Saved and loaded: the curve (with its points) comes back.
    c.ok(Command::FileSave {
        session: SessionRef::Name { name: "mic".into() },
    });
    c.ok(Command::FileLoad {
        session: SessionRef::Name { name: "mic".into() },
    });
    let reloaded = traces(c).into_iter().find(|x| x.id == raw.id).unwrap();
    assert_eq!(reloaded.mic_curve, t.mic_curve);
    assert_eq!(
        bits(&Some(data(c, raw.id).mag_db)),
        bits(&Some(d.mag_db.clone()))
    );
    // Removed: the served data is the measured one again.
    let off = trace(c.ok(Command::TraceMicCurve {
        trace: raw.id,
        curve: None,
    }));
    assert!(off.mic_curve.is_none());
    assert_eq!(
        bits(&Some(data(c, raw.id).mag_db)),
        bits(&Some(raw_data.mag_db.clone()))
    );
    let e = c
        .call(Command::TraceMicCurve {
            trace: raw.id,
            curve: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    r.h.shutdown();
}

/// A field sweep export from before the analysis facts were exported: its transfer
/// function with the delay it states, and a note saying why the distortion is not there.
#[test]
fn old_sweep_export_imports_as_transfer_with_its_delay() {
    let mut r = rig("trace-v1-sweep");
    let c = &mut r.c;
    let t = trace(c.ok(Command::TraceImport {
        file_name: "ac2-v1-sweep1.csv".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        content: Blob(fixture("ac2-v1-sweep1.csv")),
    }));
    assert_eq!(t.kind, TraceKind::Transfer);
    assert!((t.delay.0 * 1000.0 - 3.333_333_333_333_333_5).abs() < 1e-12);
    assert!(matches!(
        &t.source,
        TraceSource::Imported { notes, .. } if notes == &[ImportNote::SweepWithoutAnalysis]
    ));
    assert!(data(c, t.id).sweep.is_none());
    r.h.shutdown();
}
