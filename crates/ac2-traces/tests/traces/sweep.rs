//! Sweep and impulse-response exports and their session round trips.

use super::*;

pub(super) fn sweep_trace(id: u32) -> StoredTrace {
    let g = grid();
    let n = frequencies(&g).len();
    let source = TraceSource::Sweep {
        meas: ac2_proto::units::MeasId(1),
        meas_name: "Sweep".into(),
        number: 1,
        run: SweepId(1),
        epoch: SessionEpoch(2),
        sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
        level: Dbfs(-50.0),
        repeats: 2,
        lf_harmonics: ac2_proto::model::LfHarmonics::Fine,
        reference_input: 1,
        measurement_input: 0,
    };
    let ramp = |a: f32| (0..n).map(|i| a - i as f32 * 0.01).collect::<Vec<f32>>();
    let mut level = ramp(-40.0);
    level[n - 1] = f32::NAN;
    let curve = |l: Vec<f32>| DistortionCurve {
        floor_db: l.iter().map(|v| v - 30.0).collect(),
        level_db: l,
    };
    StoredTrace {
        meta: meta(id, source, 0.0033, TraceKind::Sweep, &g),
        grid: g,
        columns: Columns {
            mag_db: ramp(-6.0),
            phase_deg: Some(ramp(10.0)),
            coherence: None,
        },
        sweep: Some(SweepData {
            harmonics: vec![
                HarmonicCurve {
                    order: 2,
                    curve: curve(level.clone()),
                },
                HarmonicCurve {
                    order: 3,
                    curve: curve(ramp(-50.0)),
                },
            ],
            thd: curve(ramp(-39.6)),
            ir: TraceIr {
                t0: Seconds(-0.75),
                dt: Seconds(1.0 / 48_000.0),
                linear: vec![0.0, 0.5, -0.25, 0.125],
                etc_db: vec![-200.0, -6.0, -12.0, -18.0],
            },
            info: SweepInfo {
                sample_rate: Hz(48_000.0),
                rate: Seconds(0.45),
                duration: Seconds(3.1),
                repeats: 2,
                arrival: Seconds(0.0033),
                reference_level: Db(2.3),
                window_pre: Seconds(0.008),
                window_post: Seconds(0.09),
                gate_pre: Seconds(0.03),
                gate: Seconds(0.88),
                floor_margin: Db(6.0),
                clipped: false,
            },
            room: Some(room()),
            unresolved: Some(SweepUnresolved {
                response: Unresolved {
                    resolution: Resolution::FortyEighth,
                    ranges: Vec::new(),
                },
                harmonics: Unresolved {
                    resolution: Resolution::FortyEighth,
                    ranges: vec![FreqRange {
                        lo: Hz(20.0),
                        hi: Hz(350.5),
                    }],
                },
            }),
        }),
        ir: None,
        mic_curve: None,
    }
}

fn room() -> RoomAcoustics {
    let band = |centre: Option<f64>, t: f64| RoomBand {
        centre: centre.map(Hz),
        onset: Seconds(-0.000_125),
        truncation: Seconds(0.875),
        decay_range: Some(Db(52.25)),
        edt: RoomValue::Value { value: t * 0.9 },
        t20: RoomValue::Value { value: t },
        t30: RoomValue::Refused {
            reason: RoomRefusal::InsufficientRange {
                range: Db(42.5),
                needed: Db(45.0),
            },
        },
        c50: RoomValue::Value { value: -1.25 },
        c80: RoomValue::Value { value: 2.5 },
        d50: RoomValue::Value { value: 0.4375 },
        curvature: None,
    };
    RoomAcoustics {
        broadband: band(None, 0.95),
        octave: vec![band(Some(1000.0), 0.9)],
        third: vec![RoomBand {
            edt: RoomValue::Refused {
                reason: RoomRefusal::FilterLimited {
                    bandwidth_decay: 6.5,
                },
            },
            ..band(Some(1000.0), 0.9)
        }],
        span_end: Seconds(0.99),
    }
}

/// A sweep export carries every distortion curve, its analysis facts and impulse response:
/// it re-imports as the same sweep (bit for bit), with its delay; a session keeps it in the
/// one CSV.
#[test]
fn sweep_csv_and_session_round_trip() {
    let t = sweep_trace(5);
    let csv = export_csv(&t);
    assert!(csv.contains("# kind: sweep"));
    assert!(
        csv.contains("# sweep_info: {\"sample_rate\":48000.0,"),
        "{}",
        &csv[..1500]
    );
    assert!(csv.contains("# sweep_ir: {\"t0\":-0.75,\"dt\":"));
    assert!(
        csv.contains(
            "freq_hz,mag_db,phase_deg,h2_db,h2_floor_db,h3_db,h3_floor_db,thd_db,thd_floor_db\n"
        ),
        "{}",
        &csv[..800]
    );
    assert!(csv.contains("\nt_s,linear,etc_db\n-0.75,0,-200\n"));
    // The room parameters: one JSON line, and a table for reading after the IR.
    assert!(csv.contains("\n# room_metrics: {\"broadband\":{\"centre\":null,"));
    assert!(csv.contains(
        "\n# band_hz,edt_s,t20_s,t30_s,c50_db,c80_db,d50,decay_range_db,curvature_pct,onset_s,\
         truncation_s\n# broadband,0.855,0.95,refused:decay_range_42.5_of_45_db,-1.25,2.5,0.4375,\
         52.25,nan,-0.000125,0.875\n# 1000,"
    ));
    assert!(csv.contains("\n# 1000,refused:decay_too_short_for_band,0.9,"));
    let imp = import(csv.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Trace).unwrap();
    assert_eq!(imp.kind, TraceKind::Sweep);
    assert_eq!(imp.grid, t.grid);
    assert_eq!(imp.delay, Some(Seconds(0.0033)));
    assert!(imp.notes.is_empty());
    let s = t.sweep.as_ref().unwrap();
    let back = imp.sweep.unwrap();
    assert_eq!(back.harmonics.len(), 2);
    assert_eq!(back.harmonics[1], s.harmonics[1]);
    assert!(back.harmonics[0].curve.level_db.last().unwrap().is_nan());
    assert_eq!(back.thd, s.thd);
    assert_eq!(back.ir, s.ir);
    assert_eq!(back.info, s.info);
    assert_eq!(back.room, s.room);
    assert!(s.unresolved.is_some());
    assert_eq!(back.unresolved, s.unresolved);
    // An export written before sweeps said where they are unresolved (v4) is the same
    // sweep without that.
    let v4: String = csv
        .replacen("# ac2 trace export v5", "# ac2 trace export v4", 1)
        .lines()
        .filter(|l| !l.starts_with("# sweep_unresolved"))
        .map(|l| format!("{l}\n"))
        .collect();
    let old = import(v4.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Trace).unwrap();
    let old = old.sweep.unwrap();
    assert!(old.unresolved.is_none());
    assert_eq!(old.room, s.room);
    // An export written before the room parameters (v2) is the same sweep without them.
    let v2: String = csv
        .replacen("# ac2 trace export v5", "# ac2 trace export v2", 1)
        .lines()
        .filter(|l| !l.starts_with("# room_metrics"))
        .map(|l| format!("{l}\n"))
        .collect();
    let old = import(v2.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Trace).unwrap();
    let old = old.sweep.unwrap();
    assert!(old.room.is_none());
    assert_eq!(old.info, s.info);
    // As a target it is a magnitude only, with nothing of the sweep.
    let tgt = import(csv.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Target).unwrap();
    assert_eq!(tgt.kind, TraceKind::Target);
    assert!(tgt.sweep.is_none() && tgt.delay.is_none());

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("sweep");
    let sess = Session {
        saved_at: WallNs(1_790_000_000_000_000_001),
        measurements: vec![],
        spl_logs: vec![],
        traces: vec![t.clone()],
    };
    session::save(&dir, &sess).unwrap();
    let files: Vec<_> = std::fs::read_dir(dir.join("traces")).unwrap().collect();
    assert_eq!(files.len(), 1, "one CSV per trace, no sidecar");
    let back = session::load(&dir).unwrap();
    let b = &back.traces[0];
    assert_eq!(b.meta, t.meta);
    let bs = b.sweep.as_ref().unwrap();
    assert_eq!((&bs.ir, &bs.info, &bs.thd), (&s.ir, &s.info, &s.thd));
    assert_eq!(bs.room, s.room);
    assert_eq!(bs.harmonics[1], s.harmonics[1]);
    // The served data carries the sweep.
    assert!(b.data().sweep.is_some());

    // A sweep trace whose CSV lost its impulse response is damaged, not silently a
    // transfer.
    let m = session::read_manifest(&dir).unwrap();
    let p = dir.join(&m.traces[0].file);
    let text = std::fs::read_to_string(&p).unwrap();
    let cut: String = text[..text.find("# impulse response").unwrap()]
        .lines()
        .filter(|l| !l.starts_with("# sweep_ir"))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&p, cut).unwrap();
    assert!(session::load(&dir).is_err());
}

/// A transfer trace as captured with the IR pane's impulse response: 4096 samples at
/// 48 kHz, time zero at the inserted delay, a decaying arrival 2.5 ms after it.
fn transfer_with_ir(id: u32) -> StoredTrace {
    let mut t = delayed(id, 2, 0.0125, 0.0100, 0.95);
    let fs = 48_000.0;
    let n = 4096;
    let linear: Vec<f32> = (0..n)
        .map(|i| {
            let k: i32 = i - 2048 - 120;
            if k < 0 {
                0.0
            } else {
                0.5 * (-(k as f32) / 200.0).exp() * if k % 2 == 0 { 1.0 } else { -0.7 }
            }
        })
        .collect();
    let etc_db = linear
        .iter()
        .map(|v| (20.0 * v.abs().log10()).max(-200.0))
        .collect();
    t.ir = Some(TransferIr {
        sample_rate: Hz(fs),
        ir: TraceIr {
            t0: Seconds(-2048.0 / fs),
            dt: Seconds(1.0 / fs),
            linear,
            etc_db,
        },
    });
    t
}

#[test]
fn transfer_ir_csv_and_session_round_trip() {
    let t = transfer_with_ir(6);
    let csv = export_csv(&t);
    assert!(csv.starts_with("# ac2 trace export v5\n"));
    assert!(
        csv.contains("\n# transfer_ir: {\"sample_rate\":48000.0,\"t0\":-0.042666666666666665,"),
        "{}",
        &csv[..1500]
    );
    assert!(!csv.contains("# sweep_ir"));
    assert!(csv.contains("\nt_s,linear,etc_db\n-0.042666666666666665,0,-200\n"));
    let plain = export_csv(&delayed(7, 2, 0.0125, 0.0100, 0.95));
    // The IR table is the whole difference: its rows bound what a trace adds.
    let added = csv.len() - plain.len();
    assert!(added < 200_000, "the IR adds {added} bytes");
    let imp = import(csv.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Trace).unwrap();
    assert_eq!(imp.kind, TraceKind::Transfer);
    assert_eq!(imp.ir, t.ir, "bit for bit");
    assert!(imp.sweep.is_none());
    // A target keeps the magnitude only.
    let tgt = import(csv.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Target).unwrap();
    assert!(tgt.ir.is_none());
    // An export written before transfer traces kept their IR (v3) imports without one.
    let v3: String = csv[..csv.find("# impulse response").unwrap()]
        .replacen("# ac2 trace export v5", "# ac2 trace export v3", 1)
        .lines()
        .filter(|l| !l.starts_with("# transfer_ir"))
        .map(|l| format!("{l}\n"))
        .collect();
    let old = import(v3.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Trace).unwrap();
    assert_eq!(old.kind, TraceKind::Transfer);
    assert!(old.ir.is_none());
    assert_eq!(old.columns, imp.columns);
    // A transfer IR on another kind, or announced without its table, is refused.
    let wrong = csv.replacen("# kind: transfer", "# kind: spectrum dBFS", 1);
    let e = import(wrong.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Trace).unwrap_err();
    assert_eq!(e.problem, ImportProblem::BadHeader, "{e}");
    let cut = &csv[..csv.find("# impulse response").unwrap()];
    let e = import(cut.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Trace).unwrap_err();
    assert_eq!(e.problem, ImportProblem::NoData, "{e}");
    assert!(e.msg.contains("transfer_ir announced"), "{e}");

    // A session keeps it beside a trace captured without one.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("ir");
    let without = delayed(7, 2, 0.0125, 0.0100, 0.95);
    let sess = Session {
        saved_at: WallNs(1_790_000_000_000_000_002),
        measurements: vec![],
        spl_logs: vec![],
        traces: vec![t.clone(), without.clone()],
    };
    session::save(&dir, &sess).unwrap();
    let back = session::load(&dir).unwrap();
    assert_eq!(back.traces.len(), 2);
    assert_eq!(back.traces[0].ir, t.ir);
    assert_eq!(back.traces[0].data().ir, t.ir, "served with the trace");
    assert_eq!(back.traces[1].ir, None);
    assert_eq!(back.traces[1].columns, without.columns);
}

/// Field exports written before the analysis facts were exported (v1): the transfer
/// function with its delay, the distortion dropped with a note saying why.
#[test]
fn v1_sweep_export_imports_as_transfer_with_a_note() {
    for (f, delay_ms) in [
        ("ac2-v1-sweep1.csv", Some(3.333_333_333_333_333_5)),
        ("ac2-v1-sweep3-94cm-8x.csv", None),
    ] {
        let imp = import(&fixture(f), ImportFormat::Auto, ImportRole::Trace).unwrap();
        assert_eq!(imp.format, ImportFormat::Ac2Csv, "{f}");
        assert_eq!(imp.kind, TraceKind::Transfer, "{f}");
        assert!(imp.sweep.is_none());
        assert_eq!(imp.notes, vec![ImportNote::SweepWithoutAnalysis], "{f}");
        assert!(imp.columns.phase_deg.is_some());
        let d = imp.delay.unwrap().0 * 1000.0;
        assert!(d > 0.0, "{f}: delay {d} ms");
        if let Some(want) = delay_ms {
            assert!((d - want).abs() < 1e-12, "{f}: {d}");
        }
    }
}

/// An imported trace keeps the delay its export states, whatever its kind.
#[test]
fn import_keeps_the_delay_of_an_ac2_export() {
    let t = delayed(7, 3, 0.0125, 0.0120, 0.97);
    let imp = import(
        export_csv(&t).as_bytes(),
        ImportFormat::Auto,
        ImportRole::Trace,
    )
    .unwrap();
    assert_eq!(imp.delay, Some(Seconds(0.012)));
    let rew = import(
        &fixture("rew_export.txt"),
        ImportFormat::Auto,
        ImportRole::Trace,
    )
    .unwrap();
    assert_eq!(rew.delay, None);
}

/// A sweep's response averages and divides like a transfer function (same epoch: shared
/// time base).
#[test]
fn sweep_traces_combine_as_transfer_functions() {
    let a = sweep_trace(5);
    let b = sweep_trace(6);
    let d = average(
        &[&a, &b],
        AverageMethod::Complex,
        DelayReference::Trace { trace: TraceId(5) },
    )
    .unwrap();
    assert_eq!(d.kind, TraceKind::Transfer);
    assert!(d.columns.phase_deg.is_some());
    let q = binary(&a, &b, MathOp::Divide).unwrap();
    assert!(
        q.columns
            .mag_db
            .iter()
            .filter(|v| v.is_finite())
            .all(|v| v.abs() < 1e-4)
    );
}
