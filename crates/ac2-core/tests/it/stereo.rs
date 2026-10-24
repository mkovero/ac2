//! The correlator against the refgen vectors (`tools/refgen/sets/stereo.py`) and the
//! design's readings (mono +1, reversed −1, a quiet side named, never a number); the
//! goniometer pick's rotation, auto gain, spread and determinism.

use ac2_core::stereo::{
    Correlation, Correlator, GONIO_POINTS, GonioPicker, Side, landing_intensity,
};
use ac2_testkit::{Comparison, GoldenSet, Tolerance, assert_ok};

const FS: f64 = 48000.0;
const WINDOW_S: f64 = 0.3;
const GATE_DBFS: f64 = -60.0;
const BLOCK: usize = 480;

fn f32s(v: &[f64]) -> Vec<f32> {
    v.iter().map(|&x| x as f32).collect()
}

/// The golden cases from the two noises, the generator's composition rule.
fn cases(g: &GoldenSet) -> Vec<(&'static str, Vec<f32>, Vec<f32>)> {
    let p = g.f64("p").expect("p");
    let q = g.f64("q").expect("q");
    let zeros = vec![0.0; p.len()];
    let mut delayed = vec![0.0; 48];
    delayed.extend_from_slice(&p[..p.len() - 48]);
    let neg: Vec<f64> = p.iter().map(|v| -v).collect();
    let quiet: Vec<f64> = p.iter().map(|v| v * 10f64.powf(-50.0 / 20.0)).collect();
    vec![
        ("mono", f32s(&p), f32s(&p)),
        ("left_only", f32s(&p), f32s(&zeros)),
        ("right_only", f32s(&zeros), f32s(&p)),
        ("reversed", f32s(&p), f32s(&neg)),
        ("independent", f32s(&p), f32s(&q)),
        ("delayed_1ms", f32s(&p), f32s(&delayed)),
        ("gated_left", f32s(&quiet), f32s(&q)),
    ]
}

fn readings(l: &[f32], r: &[f32]) -> Vec<Correlation> {
    let mut c = Correlator::new(FS, WINDOW_S);
    l.chunks(BLOCK)
        .zip(r.chunks(BLOCK))
        .map(|(a, b)| {
            c.process(a, b);
            c.reading(GATE_DBFS)
        })
        .collect()
}

#[test]
fn correlator_matches_golden() {
    let g = GoldenSet::load("stereo_correlation").expect("golden");
    for (name, l, r) in cases(&g) {
        let want_r = g.f64(&format!("{name}_r")).expect("r");
        let want_quiet = g.f64(&format!("{name}_quiet")).expect("quiet");
        let got = readings(&l, &r);
        let got_r: Vec<f64> = got.iter().map(|c| f64::from(c.r.unwrap_or(0.0))).collect();
        assert_ok(Comparison::new(name).close_f64(&want_r, &got_r, Tolerance::abs(1e-4)));
        for (i, (c, &code)) in got.iter().zip(&want_quiet).enumerate() {
            let want = match code as u8 {
                0 => None,
                1 => Some(Side::Left),
                2 => Some(Side::Right),
                _ => unreachable!(),
            };
            assert_eq!(c.quiet, want, "{name} block {i}");
            assert_eq!(
                c.r.is_none(),
                want.is_some(),
                "{name} block {i}: gated has no r"
            );
        }
    }
}

#[test]
fn correlator_reads_mono_reversed_and_quiet_sides() {
    let g = GoldenSet::load("stereo_correlation").expect("golden");
    for (name, l, r) in cases(&g) {
        let last = *readings(&l, &r).last().expect("blocks");
        match name {
            "mono" => assert!((last.r.expect("r") - 1.0).abs() < 1e-5, "{name}: {last:?}"),
            "reversed" => assert!((last.r.expect("r") + 1.0).abs() < 1e-5, "{name}: {last:?}"),
            "left_only" => assert_eq!(last.quiet, Some(Side::Right), "{name}: {last:?}"),
            "right_only" | "gated_left" => {
                assert_eq!(last.quiet, Some(Side::Left), "{name}: {last:?}")
            }
            "independent" => assert!(last.r.expect("r").abs() < 0.2, "{name}: {last:?}"),
            "delayed_1ms" => {
                let r = last.r.expect("r");
                assert!(
                    r > 0.2 && r < 0.8,
                    "{name}: a 1 ms lag on pink noise reads {r}"
                )
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn correlator_window_change_keeps_the_sums() {
    let g = GoldenSet::load("stereo_correlation").expect("golden");
    let p = f32s(&g.f64("p").expect("p"));
    let q = f32s(&g.f64("q").expect("q"));
    let mut c = Correlator::new(FS, WINDOW_S);
    c.process(&p, &q);
    let before = c.reading(GATE_DBFS);
    c.set_window(1.0);
    assert_eq!(c.reading(GATE_DBFS), before);
    // Silence on both: the longer window now forgets more slowly than the short one would.
    let mut slow = c.clone();
    let mut fast = c.clone();
    fast.set_window(WINDOW_S);
    let zeros = vec![0.0f32; BLOCK];
    slow.process(&zeros, &zeros);
    fast.process(&zeros, &zeros);
    assert!(slow.reading(GATE_DBFS).r.is_some());
    assert_eq!(slow.reading(GATE_DBFS).quiet, None);
    assert_eq!(fast.reading(GATE_DBFS).quiet, None);
}

fn tone(f0: f64, amp: f64, n: usize) -> Vec<f32> {
    (0..n)
        .map(|k| (amp * (std::f64::consts::TAU * f0 * k as f64 / FS).sin()) as f32)
        .collect()
}

#[test]
fn gonio_left_only_leans_left_and_mono_stands_upright() {
    let l = tone(1000.0, 0.5, 4800);
    let zeros = vec![0.0f32; 4800];
    let mut out = [[0i8; 2]; GONIO_POINTS];
    let mut picker = GonioPicker::new(FS, 7);
    let gain_db = picker.pick(&l, &zeros, &mut out);
    assert!(
        (gain_db + 6.02).abs() < 0.02,
        "gain {gain_db} dBFS, peak is −6.02"
    );
    for [x, y] in out {
        assert_eq!(
            x, -y,
            "left only lies on the left-leaning diagonal: ({x}, {y})"
        );
    }
    assert!(
        out.iter().any(|&[x, y]| x < -85 && y > 85),
        "the figure reaches the gain radius (127/√2 per axis)"
    );
    let gain_db = picker.pick(&l, &l, &mut out);
    assert!(
        (gain_db - 3.01 + 6.02).abs() < 0.02,
        "mono pair magnitude: {gain_db} dBFS"
    );
    assert!(out.iter().all(|&[x, _]| x == 0), "mono has no width");
    assert!(
        out.iter().any(|&[_, y]| y > 120),
        "mono reaches the top at the gain"
    );
}

#[test]
fn gonio_pick_is_not_a_stride_and_is_deterministic() {
    let l = tone(1000.0, 0.5, 4800);
    let r = tone(1000.0, 0.5, 4800);
    let (mut a, mut b) = ([[0i8; 2]; GONIO_POINTS], [[0i8; 2]; GONIO_POINTS]);
    GonioPicker::new(FS, 1).pick(&l, &r, &mut a);
    GonioPicker::new(FS, 1).pick(&l, &r, &mut b);
    assert_eq!(a, b, "same seed, same points");
    // A fixed stride dividing the 48-sample period would hit a few phases; the random pick
    // covers the line: 25 distinct heights is every value sin takes on 48 phases.
    let mut ys: Vec<i8> = a.iter().map(|p| p[1]).collect();
    ys.sort_unstable();
    ys.dedup();
    assert!(
        ys.len() >= 20,
        "{} distinct heights on a 1 kHz line",
        ys.len()
    );
}

#[test]
fn gonio_gain_decays_to_the_floor() {
    let mut out = [[0i8; 2]; GONIO_POINTS];
    let mut picker = GonioPicker::new(FS, 3);
    let loud = vec![1.0f32; 48];
    let zeros = vec![0.0f32; 48];
    assert!(
        picker.pick(&loud, &zeros, &mut out).abs() < 1e-3,
        "a full-scale side: 0 dBFS"
    );
    let silence = vec![0.0f32; 48000];
    let after_1s = picker.pick(&silence, &silence, &mut out);
    assert!(
        (after_1s + 8.686).abs() < 0.01,
        "after 1 s: {after_1s} dBFS, want −8.686 (1/e)"
    );
    for _ in 0..10 {
        picker.pick(&silence, &silence, &mut out);
    }
    assert_eq!(picker.pick(&silence, &silence, &mut out), -60.0);
    assert!(out.iter().all(|&p| p == [0, 0]));
    // Short and empty blocks fill every point without panicking.
    let short = tone(1000.0, 0.5, 10);
    picker.pick(&short, &short, &mut out);
    assert!(out.iter().all(|&[x, _]| x == 0));
    let gain_db = 20.0 * picker.gain().log10() as f32;
    assert_eq!(
        picker.pick(&[], &[], &mut out),
        gain_db,
        "an empty block keeps the gain"
    );
    assert!(out.iter().all(|&p| p == [0, 0]));
}

#[test]
fn landing_intensity_spans_the_window() {
    assert_eq!(landing_intensity([127, 0]), 1.0);
    assert_eq!(landing_intensity([0, 0]), 0.0);
    assert!(
        (landing_intensity([90, 90]) - 1.0).abs() < 0.01,
        "at the gain radius"
    );
    let at_minus_15 = landing_intensity([0, 23]);
    assert!((at_minus_15 - 0.5).abs() < 0.02, "−14.8 dB: {at_minus_15}");
    assert!(landing_intensity([3, 2]) < 0.02, "below −30 dB");
}
