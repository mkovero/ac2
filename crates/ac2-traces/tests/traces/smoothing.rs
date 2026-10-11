//! Display smoothing: applied when served, never stored.

use super::*;

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
fn average_and_math_combine_unsmoothed_columns() {
    let a = spiky(1);
    let b = delayed(2, 2, 0.010, 0.010, 0.9);
    let (mut sa, mut sb) = (a.clone(), b.clone());
    sa.meta.edit.smoothing = Some(sixth());
    sb.meta.edit.smoothing = Some(sixth());
    let reference = DelayReference::Trace { trace: TraceId(1) };
    let plain = average(&[&a, &b], AverageMethod::Power, reference).unwrap();
    let smoothed = average(&[&sa, &sb], AverageMethod::Power, reference).unwrap();
    assert_eq!(plain.columns, smoothed.columns);
    let d = binary(&sa, &sb, MathOp::Divide).unwrap();
    let i = col(&sa.grid, 1000.0);
    assert!(
        (d.columns.mag_db[i] - 12.0).abs() < 1e-4,
        "{}",
        d.columns.mag_db[i]
    );
}
