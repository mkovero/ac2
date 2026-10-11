//! Import of realistic analyzer exports and ac2 CSV round trips.

use super::*;

#[test]
fn rew_export_with_star_comments_and_crlf() {
    let t = import(
        &fixture("rew_export.txt"),
        ImportFormat::Auto,
        ImportRole::Trace,
    )
    .unwrap();
    assert_eq!(t.format, ImportFormat::AnalyzerText);
    assert_eq!(t.kind, TraceKind::Transfer);
    assert_eq!(t.rows, 120);
    let GridDef::Log { ppo, .. } = t.grid else {
        panic!("{:?}", t.grid)
    };
    assert_eq!(ppo, 48);
    // 1 kHz is a row of the file and a column of the grid: exact.
    let i = col(&t.grid, 1000.0);
    assert!((t.columns.mag_db[i] - 80.0).abs() < 1e-3);
    assert!(t.columns.phase_deg.is_some());
    assert!(t.columns.coherence.is_none());
}

#[test]
fn smaart_tab_export_maps_columns_by_header() {
    let t = import(
        &fixture("smaart_tf.txt"),
        ImportFormat::AnalyzerText,
        ImportRole::Trace,
    )
    .unwrap();
    let coh = t.columns.coherence.as_ref().unwrap();
    let finite: Vec<f32> = coh.iter().copied().filter(|v| v.is_finite()).collect();
    assert!(!finite.is_empty());
    assert!(finite.iter().all(|v| (0.6..=1.0).contains(v)), "{finite:?}");
    // Outside the file's 31.25 Hz … 16 kHz span the grid has no values.
    assert!(t.columns.mag_db[col(&t.grid, 1000.0 * 2f64.powf(-5.0))].is_finite());
    let first = frequencies(&t.grid)[0];
    assert!(first >= 31.25 - 1e-9, "{first}");
}

#[test]
fn semicolon_decimal_comma_latin1() {
    let t = import(
        &fixture("semicolon_decimal_comma.csv"),
        ImportFormat::Auto,
        ImportRole::Trace,
    )
    .unwrap();
    assert_eq!(t.rows, 17);
    let i = col(&t.grid, 1000.0);
    assert!(
        (t.columns.mag_db[i] + 2.5).abs() < 1e-4,
        "{}",
        t.columns.mag_db[i]
    );
    assert!((t.columns.phase_deg.as_ref().unwrap()[i] - 12.0).abs() < 1e-4);
}

#[test]
fn coherence_in_percent_is_scaled() {
    let t = import(
        &fixture("coh_percent.csv"),
        ImportFormat::Auto,
        ImportRole::Trace,
    )
    .unwrap();
    let c = t.columns.coherence.unwrap();
    assert!(
        c.iter()
            .filter(|v| v.is_finite())
            .all(|v| (0.0..=1.0).contains(v))
    );
}

#[test]
fn target_curve_is_magnitude_only_and_interpolates_in_log_f() {
    let t = import(
        &fixture("house_curve.txt"),
        ImportFormat::Auto,
        ImportRole::Target,
    )
    .unwrap();
    assert_eq!(t.kind, TraceKind::Target);
    assert!(t.columns.phase_deg.is_none() && t.columns.coherence.is_none());
    let f = frequencies(&t.grid);
    let at = |hz: f64| {
        let i = f
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 / hz).ln().abs().total_cmp(&(b.1 / hz).ln().abs()))
            .unwrap()
            .0;
        (f[i], t.columns.mag_db[i])
    };
    let (f1k, v1k) = at(1000.0);
    assert!((f1k - 1000.0).abs() < 1e-9 && v1k.abs() < 1e-6);
    // Halfway between 1 k and 10 k in log frequency is half of −3 dB.
    let (fm, vm) = at(1000.0 * 10f64.sqrt());
    let expect = -3.0 * (fm / 1000.0).log10();
    assert!((f64::from(vm) - expect).abs() < 1e-4, "{vm} vs {expect}");
}

#[test]
fn typed_refusals() {
    let bad = |s: &str| import(s.as_bytes(), ImportFormat::Auto, ImportRole::Trace).unwrap_err();
    let e = bad("20 1\n30 2\n25 3\n");
    assert_eq!((e.line, e.problem), (Some(3), ImportProblem::NotAscending));
    let e = bad("# x\n20 1\n30 2 5\n");
    assert_eq!((e.line, e.problem), (Some(3), ImportProblem::ColumnCount));
    let e = bad("20 1\n30 abc\n");
    assert_eq!((e.line, e.problem), (Some(2), ImportProblem::BadNumber));
    let e = bad("-20 1\n30 2\n");
    assert_eq!((e.line, e.problem), (Some(1), ImportProblem::OutOfRange));
    let e = bad("# nothing here\n\n");
    assert_eq!(e.problem, ImportProblem::NoData);
    let e = bad("20 1 0 1.5\n30 2 0 0.5\n");
    assert_eq!(e.problem, ImportProblem::BadCoherence);
    let e = bad("# ac2 trace export v6\nfreq_hz,mag_db\n20,1\n30,2\n");
    assert_eq!((e.line, e.problem), (Some(1), ImportProblem::BadHeader));
    let e = import(b"20 1\n30 2\n", ImportFormat::Ac2Csv, ImportRole::Trace).unwrap_err();
    assert_eq!(e.problem, ImportProblem::BadHeader);
    let e = import(b"\x00\x01\x02", ImportFormat::Auto, ImportRole::Trace).unwrap_err();
    assert_eq!(e.problem, ImportProblem::NotText);
    let e = bad("20\n30\n");
    assert_eq!(e.problem, ImportProblem::NoData);
}

#[test]
fn ac2_csv_round_trips_bit_for_bit() {
    let mut t = delayed(7, 3, 0.0125, 0.0120, 0.97);
    t.columns.mag_db[10] = f32::NAN;
    t.columns.mag_db[11] = -2.718_281_7e-3;
    t.meta.edit.name = "Main L, pre EQ".into();
    let csv = export_csv(&t);
    assert!(csv.starts_with("# ac2 trace export v5\n# name: Main L, pre EQ\n"));
    assert!(csv.contains("# delay_ms: 12\n"));
    assert!(csv.contains("freq_hz,mag_db,phase_deg,coherence\n"));
    let back = import(csv.as_bytes(), ImportFormat::Auto, ImportRole::Trace).unwrap();
    assert_eq!(back.format, ImportFormat::Ac2Csv);
    assert_eq!(back.name.as_deref(), Some("Main L, pre EQ"));
    assert_eq!(back.grid, t.grid);
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&back.columns.mag_db), bits(&t.columns.mag_db));
    assert_eq!(
        bits(back.columns.phase_deg.as_ref().unwrap()),
        bits(t.columns.phase_deg.as_ref().unwrap())
    );
    assert_eq!(
        bits(back.columns.coherence.as_ref().unwrap()),
        bits(t.columns.coherence.as_ref().unwrap())
    );
}

#[test]
fn linear_grid_spectrum_round_trips_with_dc_bin() {
    let g = GridDef::Linear {
        fs: Hz(48_000.0),
        n: 64,
    };
    let n = frequencies(&g).len();
    let t = StoredTrace {
        meta: meta(
            3,
            captured(1),
            0.0,
            TraceKind::Spectrum {
                scale: LevelScale::Dbfs,
                level: SpecLevel::Tone,
            },
            &g,
        ),
        grid: g.clone(),
        columns: Columns {
            mag_db: (0..n).map(|i| -(i as f32)).collect(),
            phase_deg: None,
            coherence: None,
        },
        sweep: None,
        ir: None,
        mic_curve: None,
    };
    let back = import(
        export_csv(&t).as_bytes(),
        ImportFormat::Ac2Csv,
        ImportRole::Trace,
    )
    .unwrap();
    assert_eq!(back.grid, g);
    assert_eq!(back.kind, t.meta.kind);
    assert_eq!(back.columns.mag_db, t.columns.mag_db);
}
