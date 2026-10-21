//! Capture, averaging, math channels, export/import of stored traces, and transfer IRs.

use super::*;

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
        csv.starts_with("# ac2 trace export v6\n# name: aligned\n"),
        "{csv}"
    );
    assert!(csv.contains("# source: captured from \"main\" (measurement 1)"));
    let imp = trace(c.ok(Command::TraceImport {
        file_name: file_name.clone(),
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        ir: None,
        content,
        owner: ac2_proto::model::TraceOwner::Imported,
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
        ir: None,
        content: Blob(fixture("house_curve.txt")),
        owner: ac2_proto::model::TraceOwner::Imported,
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
                    scope: ac2_proto::model::MeasScope::Session,
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
            ir: None,
            content: Blob(b"* REW\n20 1\n30 2\n25 3\n".to_vec()),
            owner: ac2_proto::model::TraceOwner::Imported,
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
        ir: None,
        content: Blob(fixture("ac2-v1-sweep1.csv")),
        owner: ac2_proto::model::TraceOwner::Imported,
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

/// Time of the largest |sample| of a stored transfer IR, seconds re the trace's delay.
fn ir_peak_s(ir: &TransferIr) -> f64 {
    let (i, _) = ir
        .ir
        .linear
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap();
    ir.ir.t0.0 + i as f64 * ir.ir.dt.0
}

/// A transfer capture keeps the IR the IR pane shows: every sample, time zero at the
/// inserted delay (the trace's `delay`), so the acoustic arrival sits at its path delay
/// less the inserted delay. It survives export and import. A measurement whose reference
/// never carried signal has no IR to keep.
#[test]
fn transfer_capture_keeps_its_ir() {
    let mut r = rig("trace-ir");
    let c = &mut r.c;
    let fs = f64::from(FS);
    let path_s = f64::from(ACOUSTIC_DELAY) / fs;
    let raw = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "raw".into(),
        slot: Some(1),
    }));
    let ir = data(c, raw.id).ir.expect("captured with an IR");
    assert_eq!(ir.sample_rate, Hz(fs));
    assert_eq!(ir.ir.linear.len(), 4096);
    assert_eq!(ir.ir.etc_db.len(), 4096);
    // Live `ir_linear` goes as 16-bit mantissas of one power of two per frame
    // (docs/protocol.md §5.2); the capture comes from the analysis and keeps every f32.
    let peak = ir
        .ir
        .linear
        .iter()
        .fold(0.0f64, |m, x| m.max(f64::from(x.abs())));
    let step = 2f64.powf((peak / 32_765.0).log2().ceil());
    let off_grid = ir
        .ir
        .linear
        .iter()
        .filter(|x| (f64::from(**x) / step).fract() != 0.0)
        .count();
    assert!(
        off_grid > 4096 / 2,
        "the capture lost precision: {off_grid}"
    );
    assert!((ir.ir.dt.0 - 1.0 / fs).abs() < 1e-15);
    assert!((ir.ir.t0.0 + 2048.0 / fs).abs() < 1e-12);
    let peak = ir_peak_s(&ir);
    assert!((peak - path_s).abs() < 0.5 / fs, "peak {peak}");

    // Aligned: the arrival moves to time zero with the inserted delay.
    let rev = match c.ok(Command::DelaySet {
        meas: MeasId(1),
        delay: Seconds(path_s),
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
    let air = data(c, aligned.id).ir.expect("captured with an IR");
    assert!((aligned.delay.0 - path_s).abs() < 1e-12);
    let peak = ir_peak_s(&air);
    assert!(peak.abs() < 0.5 / fs, "peak {peak}");

    // Export and import keep it bit for bit.
    let content = match c.ok(Command::TraceExport {
        trace: aligned.id,
        format: ExportFormat::Ac2Csv,
    }) {
        ReplyBody::Export { content, .. } => content,
        other => panic!("{other:?}"),
    };
    let back = trace(c.ok(Command::TraceImport {
        file_name: "aligned.csv".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        ir: None,
        content,
        owner: ac2_proto::model::TraceOwner::Imported,
    }));
    assert_eq!(data(c, back.id).ir, Some(air));

    // The stimulus stops (and has faded out) before a second measurement starts: its
    // reference never carries signal, so it has a result but no IR.
    c.ok(Command::GenSet {
        lease_token: r.tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: false,
        },
    });
    run(&mut r.d, 1.0);
    c.ok(Command::MeasCreate {
        config: transfer("no-ref"),
    });
    c.ok(Command::MeasStart { meas: MeasId(2) });
    for _ in 0..4 {
        run(&mut r.d, 0.5);
        c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    let none = trace(c.ok(Command::TraceCapture {
        meas: MeasId(2),
        name: "no reference".into(),
        slot: None,
    }));
    assert_eq!(none.kind, TraceKind::Transfer);
    assert_eq!(data(c, none.id).ir, None);
    r.h.shutdown();
}
