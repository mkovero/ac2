//! Averaging and math on stored traces with analytic expectations.

use super::*;

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

/// `a op b` of two stored traces on `a`'s grid, as a math channel of them computes it.
pub(super) fn binary(
    a: &StoredTrace,
    b: &StoredTrace,
    op: MathOp,
) -> Result<MathResult, math::MathError> {
    let f = frequencies(&a.grid);
    let (ca, cb) = (trace_on_grid(a, &a.grid, &f), trace_on_grid(b, &a.grid, &f));
    let input = |t: &StoredTrace, c| Input {
        columns: c,
        delay: t.meta.delay.0,
        time_base: t.meta.source.shared_epoch(),
    };
    math::transfer(
        Combine::Binary(op),
        &[input(a, &ca), input(b, &cb)],
        &a.grid,
        &f,
        ac2_core::average::DelayReference::Trace(0),
    )
}

#[test]
fn a_over_b_of_stored_traces() {
    let mut a = delayed(1, 2, 0.0110, 0.0110, 0.9);
    let b = delayed(2, 2, 0.0100, 0.0100, 0.9);
    a.columns.mag_db.iter_mut().for_each(|v| *v = -6.0);
    // On the shared time base the ratio shows A's 1 ms later arrival.
    let q = binary(&a, &b, MathOp::Divide).unwrap();
    assert_eq!(q.phase, PhaseBasis::SharedTimeBase);
    let i = col(&a.grid, 250.0);
    assert!((q.columns.mag_db[i] + 6.0).abs() < 1e-4);
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
                ir_file: None,
            },
            0.0,
            TraceKind::Target,
            &target.grid,
        ),
        grid: target.grid,
        columns: target.columns,
        sweep: None,
        ir: None,
        mic_curve: None,
    };
    let d = binary(&a, &tt, MathOp::Divide).unwrap();
    let i = col(&a.grid, 1000.0);
    assert!((d.columns.mag_db[i] + 6.0).abs() < 1e-4);
    assert!(d.columns.phase_deg.is_none());
    assert_eq!(d.phase, PhaseBasis::NoPhase);
    // A sum adds complex values: a target has none to add.
    assert_eq!(
        binary(&a, &tt, MathOp::Add),
        Err(math::MathError::NoPhase(1))
    );
}

/// A spectrum or RTA trace at -20 dB on `g`.
pub(super) fn level_trace(id: u32, kind: TraceKind, g: GridDef) -> StoredTrace {
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
        ir: None,
        mic_curve: None,
    }
}

pub(super) fn third_octaves(lo: i32, hi: i32) -> GridDef {
    GridDef::IecBands {
        fraction: BandFraction::Third,
        centres: (lo..=hi)
            .map(|x| Hz(1000.0 * 10f64.powf(f64::from(x) / 10.0)))
            .collect(),
    }
}

/// Band powers and FFT bins are not resampled: averaging spectra / RTA on different grids is
/// refused with a typed error, while the same operation on one grid works. Transfer traces on different grids are resampled instead (above).
#[test]
fn spectrum_and_rta_averages_across_grids_are_refused() {
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
                level: SpecLevel::Tone,
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
                level: SpecLevel::Tone,
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
    }
}

/// Live spatial average path: columns already on one grid and time base, each with its own
/// inserted delay. ±3 dB flat members average by power to 10·log10((10^0.3 + 10^−0.3)/2).
#[test]
fn time_base_average_of_plus_minus_three_db() {
    use ac2_core::average::DelayReference as CoreRef;
    let g = grid();
    let f = frequencies(&g);
    let flat = |db: f32| Columns {
        mag_db: vec![db; f.len()],
        phase_deg: Some(vec![0.0; f.len()]),
        coherence: Some(vec![0.9; f.len()]),
    };
    let cols = [flat(3.0), flat(-3.0)];
    let r = ac2_traces::ops::average_on_time_base(
        &g,
        &f,
        &cols,
        &[0.0, 0.0],
        AverageMethod::Power,
        CoreRef::Trace(0),
    )
    .unwrap();
    let want = 10.0 * ((10f64.powf(0.3) + 10f64.powf(-0.3)) / 2.0).log10();
    let i = col(&g, 1000.0);
    assert!((f64::from(r.columns.mag_db[i]) - want).abs() < 1e-5);
    assert!((r.columns.coherence.as_ref().unwrap()[i] - 0.9).abs() < 1e-6);

    // A member measured with its own inserted delay τ (stored flat) is re-referred to the
    // first member's zero delay: the complex mean is the comb |cos(π f τ)|, null at 1/(2τ).
    let tau = 0.5e-3;
    let flat0 = flat(0.0);
    let r = ac2_traces::ops::average_on_time_base(
        &g,
        &f,
        &[flat0.clone(), flat0],
        &[0.0, tau],
        AverageMethod::Complex,
        CoreRef::Trace(0),
    )
    .unwrap();
    for (k, hz) in f.iter().enumerate() {
        let want = (std::f64::consts::PI * hz * tau).cos().abs();
        let got = 10f64.powf(f64::from(r.columns.mag_db[k]) / 20.0);
        assert!((got - want).abs() < 1e-4, "{hz} Hz: {got} vs {want}");
    }
    assert_eq!(r.delay, Seconds(0.0));
}
