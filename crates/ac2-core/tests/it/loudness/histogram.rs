//! The 0.01 LU histograms against an unbinned computation: blocks crowding both relative
//! gates (the stated bound), a quiet programme where the absolute gate governs, and input
//! above the binned range.

use super::{BLOCK, Synth, assert_near, feed, meter, param, run, set};

#[test]
fn dense_gates_stay_within_the_stated_bound() {
    let g = set("loudness_histogram");
    let s = |n: &str| g.scalar(n).expect("scalar");
    // The vector is only a check if the gate's bin is populated.
    assert!(s("dense_integrated_gate_bin_count") >= 3.0);
    assert!(s("dense_range_gate_bin_count") >= 3.0);
    let r = run(param(&g, "dense/programme"), 48000);
    for (name, got, widen) in [
        ("integrated", r.integrated.expect("integrated"), 0.0),
        ("range", r.range.expect("range"), 0.01),
    ] {
        let a = s(&format!("dense_{name}_with_gate_bin"));
        let b = s(&format!("dense_{name}_without_gate_bin"));
        let (lo, hi) = (a.min(b) - widen, a.max(b) + widen);
        assert!(
            (lo..=hi).contains(&got),
            "dense {name}: {got:.6} outside [{lo:.6}, {hi:.6}] (unbinned {:.6})",
            s(&format!("dense_{name}"))
        );
    }
}

#[test]
fn quiet_programme_absolute_gate_governs() {
    let g = set("loudness_histogram");
    let s = |n: &str| g.scalar(n).expect("scalar");
    assert!(s("quiet_integrated_gate") < -70.0 && s("quiet_range_gate") < -70.0);
    let r = run(param(&g, "quiet/programme"), 48000);
    // Every kept block counts whole: the energy sums are exact.
    assert_near(
        "quiet integrated",
        r.integrated.expect("integrated"),
        s("quiet_integrated"),
        1e-9,
    );
    assert_near(
        "quiet range",
        r.range.expect("range"),
        s("quiet_range"),
        0.01,
    );
}

fn loud(level_db: f64) -> serde_json::Value {
    serde_json::json!({"freq_hz": 1000, "channels": 2, "segments": [[5.0, level_db, level_db]]})
}

/// f32 input above full scale lands in the overflow bins, read as their energy mean.
#[test]
fn above_the_binned_range_reads_the_overflow_energy_mean() {
    let r = run(&loud(15.0), 48000);
    assert_near(
        "integrated",
        r.integrated.expect("integrated"),
        15.0 + 0.0067,
        0.01,
    );
    assert_near("range", r.range.expect("range"), 0.0, 1e-9);
    assert_near("short-term", r.short_term, 15.0067, 0.01);
}

/// Silence reaches no gate: integrated and range stay `None`, the windows read −∞.
#[test]
fn silence_is_waiting_for_programme() {
    let mut m = meter(48000, 2);
    let mut s = Synth::new(
        &serde_json::json!({"freq_hz": 1000, "channels": 2, "segments": [[5.0, null, null]]}),
        48000,
    );
    feed(&mut m, &mut s, usize::MAX, BLOCK);
    let r = m.reading();
    assert_eq!(r.integrated, None);
    assert_eq!(r.range, None);
    assert_eq!(r.momentary, f64::NEG_INFINITY);
    assert_eq!(r.short_term, f64::NEG_INFINITY);
    assert_eq!(r.max_momentary, f64::NEG_INFINITY);
    assert!(r.gating_blocks > 0, "blocks were formed and gated out");
}
