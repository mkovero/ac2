//! Dynamics exports: the step table and envelopes re-import exactly, every token of a time
//! or level survives, and a session saves the trace as its one CSV.

use super::*;

fn trace(id: u32) -> StoredTrace {
    let info = DynamicsInfo {
        sample_rate: Hz(44_100.0),
        carrier: Hz(44_100.0 / 44.0),
        period: 44,
        onset: DynamicsOnset::Peak,
        pilot: Dbfs(-33.0),
        bursts: vec![Dbfs(-8.0), Dbfs(0.0)],
        durations: vec![Seconds(0.001), Seconds(0.1)],
        rest_max: Seconds(30.0),
        replayed: false,
        reference_input: 6,
        steps: (0..4)
            .map(|i| DynamicsStep {
                level: Dbfs(if i < 2 { -8.0 } else { 0.0 }),
                burst: Seconds(if i % 2 == 0 { 44.0 / 44_100.0 } else { 0.1 }),
                rest: Seconds(2.0 + f64::from(i) / 3.0),
                recovered: i != 3,
            })
            .collect(),
    };
    let times = [
        DynamicsTime::Seconds {
            value: Seconds(0.0123),
        },
        DynamicsTime::WithinPeriod,
        DynamicsTime::CantTell,
        DynamicsTime::NoGainReduction,
    ];
    let levels = [
        DynamicsLevel::Db { value: Db(-61.25) },
        DynamicsLevel::BelowFloor,
        DynamicsLevel::CantTell,
    ];
    let row = |k: usize| {
        let h = |o: usize| DynamicsHarmonics {
            h: [0, 1, 2, 3].map(|j| levels[(k + o + j) % 3]),
            thd: levels[(k + o) % 3],
        };
        DynamicsRow {
            gr_end: Db(6.5 + k as f64 / 7.0),
            gr_settled: (k != 1).then_some(Db(7.0)),
            settled: k.is_multiple_of(2),
            attack_tau: times[k % 4],
            attack_2db: times[(k + 1) % 4],
            settle_0_5db: times[(k + 2) % 4],
            overshoot: (k != 2).then_some(DynamicsOvershoot {
                level: Db(1.5),
                at: Seconds(0.0004),
                reference: Db(0.01),
            }),
            release_tau: times[(k + 3) % 4],
            recovery_2db: times[k % 4],
            recovery_0_5db: times[(k + 1) % 4],
            harmonics: DynamicsWindows {
                attack: h(0),
                settled: h(1),
                release: h(2),
                rest: h(3),
            },
            thump_start: DynamicsThump {
                peak: levels[k % 3],
                rms: levels[(k + 1) % 3],
            },
            thump_end: DynamicsThump {
                peak: levels[(k + 2) % 3],
                rms: levels[k % 3],
            },
            drift: Db(-0.0625 * k as f64),
        }
    };
    let envelope = |k: usize| {
        (0..5)
            .map(|j| DynamicsPoint {
                edge: if j < 3 {
                    DynamicsEdge::Start
                } else {
                    DynamicsEdge::End
                },
                t: -0.05 + j as f32 * 0.0137,
                gr_db: if j == 0 { 0.0 } else { 6.1 / (k + j) as f32 },
                h_db: [-40.0, f32::NAN, -71.3, f32::NAN],
            })
            .collect()
    };
    let inputs = [4u16, 5]
        .iter()
        .map(|&input| DynamicsInput {
            input,
            name: format!("VCA {input}"),
            delay: Seconds(3.0 / 44_100.0),
            g0: Db(-0.02),
            rows: (0..4).map(row).collect(),
        })
        .collect();
    let d = DynamicsData {
        info,
        inputs,
        envelopes: (0..2).map(|_| (0..4).map(envelope).collect()).collect(),
    };
    let grid = ac2_traces::text::dynamics::grid(d.info.carrier);
    let source = TraceSource::Dynamics {
        meas: MeasId(2),
        meas_name: "Studer".into(),
        run: DynamicsId(1),
        number: 1,
        epoch: SessionEpoch(3),
    };
    StoredTrace {
        meta: meta(id, source, 0.0, TraceKind::Dynamics, &grid),
        grid,
        columns: ac2_traces::text::dynamics::columns(),
        sweep: None,
        dynamics: Some(Box::new(d)),
        ir: None,
        mic_curve: None,
    }
}

/// NaN never equals itself: envelopes compare bit for bit.
fn same(a: &DynamicsData, b: &DynamicsData) {
    assert_eq!(a.info, b.info);
    for (x, y) in a.inputs.iter().zip(&b.inputs) {
        assert_eq!(
            (x.input, &x.name, x.delay, x.g0, &x.rows),
            (y.input, &y.name, y.delay, y.g0, &y.rows)
        );
    }
    let bits = |e: &Vec<Vec<DynamicsPoint>>| -> Vec<(DynamicsEdge, u32, u32, [u32; 4])> {
        e.iter()
            .flatten()
            .map(|p| {
                (
                    p.edge,
                    p.t.to_bits(),
                    p.gr_db.to_bits(),
                    p.h_db.map(f32::to_bits),
                )
            })
            .collect()
    };
    assert_eq!(a.envelopes.len(), b.envelopes.len());
    for (x, y) in a.envelopes.iter().zip(&b.envelopes) {
        assert_eq!(bits(x), bits(y));
    }
    assert_eq!(a.inputs.len(), b.inputs.len());
}

#[test]
fn dynamics_csv_and_session_round_trip() {
    let t = trace(9);
    let d = t.dynamics.as_deref().unwrap();
    let csv = export_csv(&t);
    assert!(csv.contains("# kind: dynamics\n"), "{}", &csv[..600]);
    assert!(csv.contains("\n# dynamics_info: {\"sample_rate\":44100.0,"));
    assert!(csv.contains("\n# dynamics_steps: {\"rows\":8}\ninput,step,level_dbfs,"));
    assert!(csv.contains("\n# dynamics_envelopes: {\"rows\":40}\ninput,step,edge,t_s,"));
    for token in [
        "within_period",
        "cant_tell",
        "no_gain_reduction",
        "below_floor",
    ] {
        assert!(csv.contains(token), "{token}");
    }
    let back = import(csv.as_bytes(), ImportFormat::Auto, ImportRole::Trace).unwrap();
    assert_eq!(back.kind, TraceKind::Dynamics);
    assert_eq!(back.grid, t.grid);
    same(d, back.dynamics.as_deref().unwrap());
    // A dynamics export has no magnitude to be a target.
    assert!(import(csv.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Target).is_err());
    // A step row with an unknown token is refused, naming its line.
    let bad = csv.replacen("within_period", "soon", 1);
    let e = import(bad.as_bytes(), ImportFormat::Auto, ImportRole::Trace).unwrap_err();
    assert_eq!(e.problem, ImportProblem::BadNumber);
    assert!(e.line.is_some());
    // A table cut short is refused.
    let cut = &csv[..csv.rfind('\n').unwrap()];
    let cut = &cut[..cut.rfind('\n').unwrap() + 1];
    assert!(import(cut.as_bytes(), ImportFormat::Auto, ImportRole::Trace).is_err());

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("dynamics");
    let sess = Session {
        saved_at: WallNs(1_790_000_000_000_000_001),
        measurements: vec![],
        spl_logs: vec![],
        traces: vec![t.clone()],
        walks: vec![],
        plans: vec![],
        remote_inputs: vec![],
    };
    session::save(&dir, &sess).unwrap();
    let back = session::load(&dir).unwrap();
    let b = &back.traces[0];
    assert_eq!(b.meta, t.meta);
    same(d, b.dynamics.as_deref().unwrap());
    assert!(b.data().dynamics.is_some());
}
