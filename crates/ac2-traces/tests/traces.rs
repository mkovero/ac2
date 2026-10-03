//! Import of realistic analyzer exports, ac2 CSV round trips, averaging and A−B math with
//! analytic expectations, and session directory round trips.
#![allow(clippy::unwrap_used)]

use std::path::Path;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{GridDef, ImportProblem};
use ac2_traces::columns::{Columns, StoredTrace, frequencies};
use ac2_traces::ops::{OpError, average, math};
use ac2_traces::session::{self, SavedDelay, SavedMeasurement, Session, SessionError};
use ac2_traces::text::{export_csv, import};

fn fixture(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(p).unwrap()
}

fn grid() -> GridDef {
    GridDef::Log {
        ppo: 48,
        k_min: -240,
        k_max: 239,
    }
}

fn edit(name: &str) -> TraceEdit {
    TraceEdit {
        name: name.into(),
        color: Rgb { r: 1, g: 2, b: 3 },
        visible: true,
        locked: false,
        order: 0,
        offset: Db(0.0),
        polarity: Polarity::Normal,
        delay_nudge: Seconds(0.0),
        slot: None,
        smoothing: None,
    }
}

fn meta(id: u32, source: TraceSource, delay: f64, kind: TraceKind, g: &GridDef) -> TraceMeta {
    TraceMeta {
        id: TraceId(id),
        edit: edit(&format!("t{id}")),
        kind,
        source,
        grid_id: g.id(),
        delay: Seconds(delay),
        depth: Some(DepthPolicy::EqualConfidence),
        cal: CalState::Uncalibrated,
        mic: None,
        mic_curve: None,
        created_at: WallNs(1_790_000_000_000_000_000),
    }
}

fn captured(epoch: u32) -> TraceSource {
    TraceSource::Captured {
        meas: MeasId(1),
        meas_name: "main".into(),
        epoch: SessionEpoch(epoch),
        at_sample: SampleIndex(48_000),
    }
}

/// A flat 0 dB path arriving `arrival` s late, captured with `inserted` s of reference
/// delay: stored phase = −360·f·(arrival − inserted).
fn delayed(id: u32, epoch: u32, arrival: f64, inserted: f64, coh: f32) -> StoredTrace {
    let g = grid();
    let f = frequencies(&g);
    StoredTrace {
        meta: meta(id, captured(epoch), inserted, TraceKind::Transfer, &g),
        columns: Columns {
            mag_db: vec![0.0; f.len()],
            phase_deg: Some(
                f.iter()
                    .map(|f| {
                        ac2_traces::columns::wrap_deg(-360.0 * f * (arrival - inserted)) as f32
                    })
                    .collect(),
            ),
            coherence: Some(vec![coh; f.len()]),
        },
        grid: g,
        sweep: None,
        mic_curve: None,
    }
}

fn col(g: &GridDef, hz: f64) -> usize {
    frequencies(g)
        .iter()
        .position(|f| (f - hz).abs() < 1e-6 * hz)
        .unwrap()
}

// ---- import ------------------------------------------------------------------------

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
    let e = bad("# ac2 trace export v3\nfreq_hz,mag_db\n20,1\n30,2\n");
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
    assert!(csv.starts_with("# ac2 trace export v2\n# name: Main L, pre EQ\n"));
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

// ---- averaging and math ------------------------------------------------------------

#[test]
fn complex_average_rereferes_to_the_reference_delay() {
    // Same 10 ms arrival captured with two different inserted delays: re-referred to one
    // delay they agree, so the complex average is the same flat 0 dB response.
    let a = delayed(1, 2, 0.010, 0.010, 0.9);
    let b = delayed(2, 2, 0.010, 0.0095, 0.9);
    let r = average(
        &[&a, &b],
        AverageMethod::Complex,
        DelayReference::Trace { trace: TraceId(1) },
    )
    .unwrap();
    assert_eq!(r.delay, Seconds(0.010));
    let i = col(&r.grid, 2000.0);
    assert!(r.columns.mag_db[i].abs() < 1e-3, "{}", r.columns.mag_db[i]);
    assert!(r.columns.phase_deg.as_ref().unwrap()[i].abs() < 1e-2);
    // Referred to 9.5 ms instead, the average shows the 0.5 ms arrival: −360°·f·0.5 ms.
    let r = average(
        &[&a, &b],
        AverageMethod::Complex,
        DelayReference::Fixed {
            delay: Seconds(0.0095),
        },
    )
    .unwrap();
    let i = col(&r.grid, 500.0);
    let expect = ac2_traces::columns::wrap_deg(-360.0 * 500.0 * 0.0005);
    let got = f64::from(r.columns.phase_deg.as_ref().unwrap()[i]);
    assert!((got - expect).abs() < 0.01, "{got} vs {expect}");
}

#[test]
fn power_average_of_levels() {
    let mut a = delayed(1, 2, 0.0, 0.0, 0.9);
    let mut b = delayed(2, 2, 0.0, 0.0, 0.9);
    a.columns.mag_db.iter_mut().for_each(|v| *v = 0.0);
    b.columns.mag_db.iter_mut().for_each(|v| *v = -100.0);
    let r = average(
        &[&a, &b],
        AverageMethod::Power,
        DelayReference::Trace { trace: TraceId(1) },
    )
    .unwrap();
    // (1 + 1e-10) / 2 in power ≈ −3.01 dB.
    assert!((r.columns.mag_db[100] + 3.0103).abs() < 1e-3);
}

#[test]
fn phase_methods_need_a_shared_time_base() {
    let a = delayed(1, 2, 0.0, 0.0, 0.9);
    let b = delayed(2, 3, 0.0, 0.0, 0.9);
    let refd = DelayReference::Trace { trace: TraceId(1) };
    assert_eq!(
        average(&[&a, &b], AverageMethod::Complex, refd),
        Err(OpError::NoSharedTimeBase)
    );
    assert_eq!(
        average(&[&a, &b], AverageMethod::CoherenceWeighted, refd),
        Err(OpError::NoSharedTimeBase)
    );
    // Power still works, magnitude only.
    let r = average(&[&a, &b], AverageMethod::Power, refd).unwrap();
    assert!(r.columns.phase_deg.is_none());
    assert_eq!(
        average(&[&a], AverageMethod::Power, refd),
        Err(OpError::TooFewTraces)
    );
    assert_eq!(
        average(&[&a, &a], AverageMethod::Power, refd),
        Err(OpError::Duplicate(TraceId(1)))
    );
    let c = delayed(3, 2, 0.0, 0.0, 0.9);
    assert_eq!(
        average(
            &[&a, &c],
            AverageMethod::Complex,
            DelayReference::Trace { trace: TraceId(9) }
        ),
        Err(OpError::ReferenceNotInput(TraceId(9)))
    );
}

#[test]
fn coherence_weighting_favours_the_coherent_trace() {
    let mut a = delayed(1, 2, 0.0, 0.0, 0.99);
    let mut b = delayed(2, 2, 0.0, 0.0, 0.5);
    a.columns.mag_db.iter_mut().for_each(|v| *v = 0.0);
    b.columns.mag_db.iter_mut().for_each(|v| *v = -20.0);
    let r = average(
        &[&a, &b],
        AverageMethod::CoherenceWeighted,
        DelayReference::Trace { trace: TraceId(1) },
    )
    .unwrap();
    // Weights 99 and 1: (99·1 + 1·0.1) / 100 = 0.991 → −0.078 dB.
    assert!(
        (r.columns.mag_db[200] + 0.0785).abs() < 1e-3,
        "{}",
        r.columns.mag_db[200]
    );
}

#[test]
fn a_minus_b() {
    let mut a = delayed(1, 2, 0.0110, 0.0110, 0.9);
    let b = delayed(2, 2, 0.0100, 0.0100, 0.9);
    a.columns.mag_db.iter_mut().for_each(|v| *v = -6.0);
    let d = math(&a, &b, MathOp::MagnitudeDifference).unwrap();
    assert!(d.columns.phase_deg.is_none());
    assert_eq!(d.columns.mag_db[50], -6.0);
    // Complex division on the shared time base shows A's 1 ms later arrival.
    let q = math(&a, &b, MathOp::ComplexDivision).unwrap();
    let i = col(&q.grid, 250.0);
    let expect = ac2_traces::columns::wrap_deg(-360.0 * 250.0 * 0.001);
    let got = f64::from(q.columns.phase_deg.as_ref().unwrap()[i]);
    assert!((got - expect).abs() < 0.01, "{got} vs {expect}");

    // Against a target on another grid: resampled, magnitude only.
    let target = import(
        &fixture("house_curve.txt"),
        ImportFormat::Auto,
        ImportRole::Target,
    )
    .unwrap();
    let tt = StoredTrace {
        meta: meta(
            3,
            TraceSource::Imported {
                file_name: "house_curve.txt".into(),
                format: ImportFormat::AnalyzerText,
                notes: vec![],
            },
            0.0,
            TraceKind::Target,
            &target.grid,
        ),
        grid: target.grid,
        columns: target.columns,
        sweep: None,
        mic_curve: None,
    };
    let d = math(&a, &tt, MathOp::MagnitudeDifference).unwrap();
    let i = col(&d.grid, 1000.0);
    assert!((d.columns.mag_db[i] + 6.0).abs() < 1e-4);
    assert_eq!(
        math(&a, &tt, MathOp::ComplexDivision),
        Err(OpError::NoPhase(TraceId(3)))
    );
}

/// A spectrum or RTA trace at -20 dB on `g`.
fn level_trace(id: u32, kind: TraceKind, g: GridDef) -> StoredTrace {
    let n = frequencies(&g).len();
    StoredTrace {
        meta: meta(id, captured(2), 0.0, kind, &g),
        columns: Columns {
            mag_db: vec![-20.0; n],
            phase_deg: None,
            coherence: None,
        },
        grid: g,
        sweep: None,
        mic_curve: None,
    }
}

fn third_octaves(lo: i32, hi: i32) -> GridDef {
    GridDef::IecBands {
        fraction: BandFraction::Third,
        centres: (lo..=hi)
            .map(|x| Hz(1000.0 * 10f64.powf(f64::from(x) / 10.0)))
            .collect(),
    }
}

/// Band powers and FFT bins are not resampled: averaging or subtracting spectra / RTA on
/// different grids is refused with a typed error, while the same operations
/// on one grid work. Transfer traces on different grids are resampled instead (above).
#[test]
fn spectrum_and_rta_math_across_grids_is_refused() {
    let refd = DelayReference::Trace { trace: TraceId(1) };
    let cases = [
        (
            TraceKind::Rta {
                scale: LevelScale::Dbfs,
            },
            third_octaves(-17, 13),
            third_octaves(-10, 13),
        ),
        (
            TraceKind::Rta {
                scale: LevelScale::Dbfs,
            },
            third_octaves(-17, 13),
            GridDef::IecBands {
                fraction: BandFraction::Octave,
                centres: vec![Hz(125.0), Hz(250.0), Hz(500.0), Hz(1000.0)],
            },
        ),
        (
            TraceKind::Spectrum {
                scale: LevelScale::Dbfs,
            },
            GridDef::Linear {
                fs: Hz(48_000.0),
                n: 8192,
            },
            GridDef::Linear {
                fs: Hz(48_000.0),
                n: 16_384,
            },
        ),
        (
            TraceKind::Spectrum {
                scale: LevelScale::Dbfs,
            },
            GridDef::Linear {
                fs: Hz(48_000.0),
                n: 8192,
            },
            GridDef::Linear {
                fs: Hz(44_100.0),
                n: 8192,
            },
        ),
    ];
    for (kind, ga, gb) in cases {
        let a = level_trace(1, kind, ga.clone());
        let b = level_trace(2, kind, gb);
        assert_eq!(
            average(&[&a, &b], AverageMethod::Power, refd),
            Err(OpError::GridMismatch),
            "{kind:?}"
        );
        assert_eq!(
            math(&a, &b, MathOp::MagnitudeDifference),
            Err(OpError::GridMismatch),
            "{kind:?}"
        );
        assert!(
            OpError::GridMismatch
                .to_string()
                .contains("must share one grid"),
            "the message says why"
        );
        // The same grid combines.
        let same = level_trace(3, kind, ga);
        let r = average(&[&a, &same], AverageMethod::Power, refd).unwrap();
        assert!((r.columns.mag_db[0] + 20.0).abs() < 1e-9);
        let d = math(&a, &same, MathOp::MagnitudeDifference).unwrap();
        assert_eq!(d.columns.mag_db[0], 0.0);
    }
}

// ---- display smoothing -------------------------------------------------------------

fn sixth() -> Smoothing {
    Smoothing {
        fraction: SmoothingFraction::Sixth,
        mode: SmoothingMode::Magnitude,
    }
}

/// A flat trace with a one-column +12 dB spike at 1 kHz.
fn spiky(id: u32) -> StoredTrace {
    let mut t = delayed(id, 2, 0.010, 0.010, 0.9);
    let i = col(&t.grid, 1000.0);
    t.columns.mag_db[i] = 12.0;
    t
}

#[test]
fn smoothing_is_applied_when_served_and_never_stored() {
    let mut t = spiky(1);
    let i = col(&t.grid, 1000.0);
    assert_eq!(t.data().mag_db[i], 12.0, "unsmoothed until asked");
    t.meta.edit.smoothing = Some(sixth());
    let shown = t.data();
    assert!(
        shown.mag_db[i] < 8.0 && shown.mag_db[i] > 2.0,
        "{}",
        shown.mag_db[i]
    );
    assert!(
        shown.mag_db[i + 2] > 0.1,
        "the spike spreads to its neighbours"
    );
    // The stored columns are untouched, and a change of mind is just another setting.
    assert_eq!(t.columns.mag_db[i], 12.0);
    t.meta.edit.smoothing = Some(Smoothing {
        fraction: SmoothingFraction::FortyEighth,
        mode: SmoothingMode::Magnitude,
    });
    // At 1/48 octave on a 48 ppo grid the kernel is the identity.
    assert!((t.data().mag_db[i] - 12.0).abs() < 1e-4);
    // Spectra and RTA bands are levels: smoothing does not apply to them.
    let mut l = level_trace(
        3,
        TraceKind::Rta {
            scale: LevelScale::Dbfs,
        },
        third_octaves(-10, 10),
    );
    l.meta.edit.smoothing = Some(sixth());
    assert_eq!(l.data().mag_db, l.columns.mag_db);
}

#[test]
fn average_and_a_minus_b_combine_unsmoothed_columns() {
    let a = spiky(1);
    let b = delayed(2, 2, 0.010, 0.010, 0.9);
    let (mut sa, mut sb) = (a.clone(), b.clone());
    sa.meta.edit.smoothing = Some(sixth());
    sb.meta.edit.smoothing = Some(sixth());
    let reference = DelayReference::Trace { trace: TraceId(1) };
    let plain = average(&[&a, &b], AverageMethod::Power, reference).unwrap();
    let smoothed = average(&[&sa, &sb], AverageMethod::Power, reference).unwrap();
    assert_eq!(plain.columns, smoothed.columns);
    let d = math(&sa, &sb, MathOp::MagnitudeDifference).unwrap();
    let i = col(&d.grid, 1000.0);
    assert!(
        (d.columns.mag_db[i] - 12.0).abs() < 1e-4,
        "{}",
        d.columns.mag_db[i]
    );
}

// ---- sessions ----------------------------------------------------------------------

fn session_sample() -> Session {
    let mut a = delayed(4, 2, 0.0125, 0.0125, 0.95);
    a.meta.edit.slot = Some(1);
    a.columns.mag_db[200] = 9.0;
    a.meta.edit.smoothing = Some(Smoothing {
        fraction: SmoothingFraction::Third,
        mode: SmoothingMode::MagnitudePhase,
    });
    a.meta.edit.visible = false;
    let b = delayed(9, 2, 0.0100, 0.0100, 0.9);
    Session {
        saved_at: WallNs(1_790_000_000_123_456_789),
        measurements: vec![SavedMeasurement {
            id: MeasId(1),
            config: MeasConfig {
                name: "main".into(),
                kind: MeasKind::Transfer {
                    config: TransferConfig {
                        reference_input: 0,
                        measurement_input: 1,
                        averaging: TfAveraging::Fifo { blocks: 8 },
                        grid: LogGridSpec {
                            ppo: 48,
                            k_min: -240,
                            k_max: 239,
                        },
                        smoothing: None,
                        depth: DepthPolicy::EqualConfidence,
                    },
                },
            },
            running: true,
            frozen: false,
            delay: Some(SavedDelay {
                applied: Seconds(0.0125),
                tracking: false,
            }),
        }],
        traces: vec![a, b],
    }
}

#[test]
fn session_round_trip_and_generations() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("show");
    let s = session_sample();
    session::save(&dir, &s).unwrap();
    let back = session::load(&dir).unwrap();
    assert_eq!(back.saved_at, s.saved_at);
    assert_eq!(back.measurements, s.measurements);
    assert_eq!(back.traces.len(), 2);
    // Columns come back unsmoothed (bit for bit) with the smoothing as an edit.
    assert_eq!(
        back.traces[0].meta.edit.smoothing,
        s.traces[0].meta.edit.smoothing
    );
    assert_eq!(back.traces[0].columns.mag_db[200], 9.0);
    for (x, y) in back.traces.iter().zip(&s.traces) {
        assert_eq!(x.meta, y.meta);
        assert_eq!(x.grid, y.grid);
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&x.columns.mag_db), bits(&y.columns.mag_db));
        assert_eq!(
            bits(x.columns.phase_deg.as_ref().unwrap()),
            bits(y.columns.phase_deg.as_ref().unwrap())
        );
    }
    // A second save with one trace leaves only that generation's file behind.
    let mut s2 = s.clone();
    s2.saved_at = WallNs(s.saved_at.0 + 1);
    s2.traces.truncate(1);
    session::save(&dir, &s2).unwrap();
    let files: Vec<_> = std::fs::read_dir(dir.join("traces"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, vec![format!("{}-4.csv", s2.saved_at.0)]);
    assert_eq!(session::load(&dir).unwrap().traces.len(), 1);
    let listed = session::list(tmp.path()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, "show");
}

#[test]
fn session_refusals() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("s");
    session::save(&dir, &session_sample()).unwrap();
    let m = dir.join(session::MANIFEST);
    let text = std::fs::read_to_string(&m).unwrap();
    // A session of the previous format (a capture's curve named without label and hash) is refused
    // with its version named, never read best-effort.
    std::fs::write(&m, text.replace("\"version\": 6", "\"version\": 5")).unwrap();
    let e = session::load(&dir).unwrap_err();
    assert_eq!(
        e,
        SessionError::Version {
            path: dir.clone(),
            found: 5
        }
    );
    assert!(e.to_string().contains("reads version 6 only"), "{e}");
    assert_eq!(
        session::load(&tmp.path().join("missing")),
        Err(SessionError::NotFound(tmp.path().join("missing")))
    );
    // A directory with other files is not overwritten.
    let other = tmp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("notes.txt"), "x").unwrap();
    assert_eq!(
        session::save(&other, &session_sample()),
        Err(SessionError::NotASession(other.clone()))
    );
    assert!(session::validate_name("friday show-2").is_ok());
    for bad in ["", "../x", "a/b", ".hidden", "a\\b", " x"] {
        assert!(session::validate_name(bad).is_err(), "{bad:?}");
    }
}

fn sweep_trace(id: u32) -> StoredTrace {
    let g = grid();
    let n = frequencies(&g).len();
    let source = TraceSource::IrCapture {
        run: SweepId(1),
        epoch: SessionEpoch(2),
        sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
        level: Dbfs(-50.0),
        repeats: 2,
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
            ir: SweepIr {
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
        }),
        mic_curve: None,
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
    // As a target it is a magnitude only, with nothing of the sweep.
    let tgt = import(csv.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Target).unwrap();
    assert_eq!(tgt.kind, TraceKind::Target);
    assert!(tgt.sweep.is_none() && tgt.delay.is_none());

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("sweep");
    let sess = Session {
        saved_at: WallNs(1_790_000_000_000_000_001),
        measurements: vec![],
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

fn curve_points() -> Vec<[f64; 2]> {
    // A mic reading +3 dB at 10 kHz and −2 dB at 50 Hz, flat at 1 kHz.
    vec![[50.0, -2.0], [1000.0, 0.0], [10_000.0, 3.0]]
}

fn curve_ref() -> MicCurveRef {
    MicCurveRef {
        label: "90°".into(),
        file_name: "MM1-34804.txt".into(),
        content_hash: "0123456789abcdef".into(),
        points: 3,
        f_lo: Hz(50.0),
        f_hi: Hz(10_000.0),
        imported_at: WallNs(1),
        stated_sensitivity: Some(15.0),
    }
}

fn with_curve(mut t: StoredTrace) -> StoredTrace {
    t.mic_curve = Some(ac2_traces::mic::correction(&curve_points(), 1000.0).unwrap());
    t.meta.mic_curve = Some(Box::new(TraceMicCurve {
        mic: "MM1 34804".into(),
        curve: curve_ref(),
        f_norm: Hz(1000.0),
    }));
    t
}

/// A mic curve applied after capture is a display edit: the served magnitude is corrected
/// (the curve's values at its points, 0 at f_norm), the stored columns, phase and coherence
/// are not; exports state it; a session keeps it with its points.
#[test]
fn mic_curve_on_a_stored_trace_is_a_display_edit() {
    let raw = delayed(4, 2, 0.0125, 0.0125, 0.95);
    let t = with_curve(raw.clone());
    let g = &t.grid;
    let d = t.data();
    // Flat 0 dB measured: the served magnitude is minus the curve, 0 at 1 kHz, −3 dB from
    // 10 kHz up, +2 dB from 50 Hz down (held flat outside the points).
    let k = t.mic_curve.clone().unwrap();
    for (f, v) in frequencies(g).iter().zip(&d.mag_db) {
        assert!((f64::from(*v) + k.db(*f)).abs() < 1e-5, "{f}: {v}");
    }
    assert_eq!(d.mag_db[col(g, 1000.0)], 0.0);
    assert!((d.mag_db[col(g, 16_000.0)] + 3.0).abs() < 1e-5);
    assert!((d.mag_db[col(g, 31.25)] - 2.0).abs() < 1e-5);
    assert_eq!(t.columns, raw.columns);
    assert_eq!(d.phase_deg, raw.data().phase_deg);
    assert_eq!(d.coherence, raw.data().coherence);
    // Export: raw columns, the curve named.
    let csv = export_csv(&t);
    assert!(
        csv.contains(
            "# mic: MM1 34804 (curve: 90°, applied after capture as a display edit, \
             not in the columns; 0 dB at 1000 Hz"
        ),
        "{}",
        &csv[..1500]
    );
    let back = import(csv.as_bytes(), ImportFormat::Auto, ImportRole::Trace).unwrap();
    assert_eq!(back.columns.mag_db, raw.columns.mag_db);
    assert_eq!(back.notes, vec![ImportNote::MicCurveNotApplied]);
    // Session: the curve and its points come back.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("mic");
    session::save(
        &dir,
        &Session {
            saved_at: WallNs(5),
            measurements: vec![],
            traces: vec![t.clone()],
        },
    )
    .unwrap();
    let l = session::load(&dir).unwrap();
    assert_eq!(l.traces[0], t);
    assert_eq!(l.traces[0].data(), d);
    // Averages combine corrected columns.
    let other = with_curve(delayed(5, 2, 0.0125, 0.0125, 0.95));
    let avg = average(
        &[&t, &other],
        AverageMethod::Power,
        DelayReference::Trace { trace: TraceId(4) },
    )
    .unwrap();
    let i = col(g, 16_000.0);
    assert!(
        (avg.columns.mag_db[i] + 3.0).abs() < 1e-4,
        "{}",
        avg.columns.mag_db[i]
    );
    // ... and say which curve their columns carry.
    let baked = ac2_traces::mic::bake(&t);
    assert_eq!(baked.meta.mic.unwrap().curve, Some(curve_ref()));
    assert!(baked.meta.mic_curve.is_none());
}

/// A trace whose capture applied a curve, a target and a locked trace refuse a curve.
#[test]
fn mic_curve_refusals() {
    use ac2_traces::mic::{MicCurveError, check};
    let mut t = delayed(4, 2, 0.0125, 0.0125, 0.95).meta;
    assert_eq!(check(&t), Ok(()));
    t.mic = Some(MicState {
        name: "MM1".into(),
        curve: Some(curve_ref()),
    });
    let e = check(&t).unwrap_err();
    assert!(matches!(e, MicCurveError::InColumns { .. }));
    assert!(e.to_string().contains("correct twice"), "{e}");
    t.mic = None;
    t.kind = TraceKind::Target;
    assert_eq!(check(&t), Err(MicCurveError::Target));
    t.kind = TraceKind::Transfer;
    t.edit.locked = true;
    assert_eq!(check(&t), Err(MicCurveError::Locked));
}

/// A sweep's distortion through the curve: order n at f moves by c(f) − c(n·f), and THD
/// is the power sum of the corrected orders.
#[test]
fn mic_curve_corrects_sweep_distortion() {
    let t = with_curve(sweep_trace(5));
    let k = t.mic_curve.clone().unwrap();
    let s = t.sweep.as_ref().unwrap();
    let d = t.data().sweep.unwrap();
    let f = frequencies(&t.grid);
    let i = col(&t.grid, 1000.0);
    for (h, raw) in d.harmonics.iter().zip(&s.harmonics) {
        let n = f64::from(h.order);
        let want = f64::from(raw.curve.level_db[i]) - (k.db(n * f[i]) - k.db(f[i]));
        assert!((f64::from(h.curve.level_db[i]) - want).abs() < 1e-4);
        assert!(h.curve.level_db[i] != raw.curve.level_db[i]);
    }
    let p: f64 = d
        .harmonics
        .iter()
        .map(|h| 10f64.powf(f64::from(h.curve.level_db[i]) / 10.0))
        .sum();
    assert!((f64::from(d.thd.level_db[i]) - 10.0 * p.log10()).abs() < 1e-4);
    // The impulse response is never touched.
    assert_eq!(d.ir, s.ir);
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
    let q = math(&a, &b, MathOp::ComplexDivision).unwrap();
    assert!(
        q.columns
            .mag_db
            .iter()
            .filter(|v| v.is_finite())
            .all(|v| v.abs() < 1e-4)
    );
}
