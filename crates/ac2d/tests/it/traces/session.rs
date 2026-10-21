//! Session save/load round trip and a mic curve on a stored trace.

use super::*;

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
        ir: None,
        content: Blob(fixture("house_curve.txt")),
        owner: ac2_proto::model::TraceOwner::Imported,
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
            scope: ac2_proto::model::MeasScope::Session,
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
            scope: ac2_proto::model::MeasScope::Session,
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
                    lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
                    resolution: ac2_proto::model::Resolution::FortyEighth,
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
        overwrite: false,
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
        discard: true,
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
        text.replace(
            &format!("\"version\": {}", ac2_traces::session::VERSION),
            &format!("\"version\": {}", ac2_traces::session::VERSION + 1),
        ),
    )
    .unwrap();
    let e = c
        .call(Command::FileLoad {
            session: SessionRef::Name {
                name: "friday show".into(),
            },
            discard: true,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    assert_eq!(
        e.detail,
        Some(ErrorDetail::SessionVersion {
            found: ac2_traces::session::VERSION + 1,
            supported: ac2_traces::session::VERSION
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
        let e = c
            .call(Command::FileSave {
                session: bad,
                overwrite: false,
            })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    }
    let e = c
        .call(Command::FileLoad {
            session: SessionRef::Name {
                name: "missing".into(),
            },
            discard: true,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    // An absolute path works for a local client.
    let p = r._dir.path().join("by-path");
    match c.ok(Command::FileSave {
        session: SessionRef::Path {
            path: p.to_string_lossy().into_owned(),
        },
        overwrite: false,
    }) {
        ReplyBody::SessionFile(f) => assert_eq!(f.name, "by-path"),
        other => panic!("{other:?}"),
    }
    r.h.shutdown();
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
        overwrite: false,
    });
    c.ok(Command::FileLoad {
        session: SessionRef::Name { name: "mic".into() },
        discard: true,
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
