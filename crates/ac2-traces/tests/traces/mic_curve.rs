//! Mic curves applied to stored traces as a display edit.

use super::*;

pub(super) fn curve_points() -> Vec<[f64; 2]> {
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

pub(super) fn with_curve(mut t: StoredTrace) -> StoredTrace {
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
            spl_logs: vec![],
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
