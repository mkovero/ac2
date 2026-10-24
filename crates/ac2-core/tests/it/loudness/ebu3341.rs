//! EBU Tech 3341 cases 1–5, 9, 11, 12, 14 (synthesised from the text by refgen): the
//! published reading within ±0.1 LU, and the unbinned reference for the same signal.

use super::{BLOCK, RATES, Synth, assert_near, feed, meter, param, run, set};

/// Binned integrated against unbinned: only blocks in the gate's bin can move it.
const INTEGRATED_PARITY_LU: f64 = 0.01;
/// Momentary and short-term are exact sums: only rounding separates the two.
const WINDOW_PARITY_LU: f64 = 1e-6;
const PUBLISHED_LU: f64 = 0.1;

fn case_1_to_5(case: &str) {
    let g = set("loudness_ebu3341");
    let c = param(&g, &format!("cases/{case}"));
    for fs in RATES {
        let r = run(&c["programme"], fs);
        let scalar = |q: &str| g.scalar(&format!("c{case}_{fs}_{q}")).expect("scalar");
        let integrated = r.integrated.expect("integrated");
        let what = |q: &str| format!("3341 case {case} at {fs} Hz {q}");
        assert_near(
            &what("integrated vs reference"),
            integrated,
            scalar("integrated"),
            INTEGRATED_PARITY_LU,
        );
        assert_near(
            &what("momentary vs reference"),
            r.momentary,
            scalar("momentary"),
            WINDOW_PARITY_LU,
        );
        assert_near(
            &what("short-term vs reference"),
            r.short_term,
            scalar("short_term"),
            WINDOW_PARITY_LU,
        );
        let published = &c["published"];
        assert_near(
            &what("integrated"),
            integrated,
            published["integrated"].as_f64().expect("I"),
            PUBLISHED_LU,
        );
        if let Some(m) = published.get("momentary").and_then(|v| v.as_f64()) {
            assert_near(&what("momentary"), r.momentary, m, PUBLISHED_LU);
            assert_near(&what("max momentary"), r.max_momentary, m, PUBLISHED_LU);
        }
        if let Some(s) = published.get("short_term").and_then(|v| v.as_f64()) {
            assert_near(&what("short-term"), r.short_term, s, PUBLISHED_LU);
            assert_near(&what("max short-term"), r.max_short_term, s, PUBLISHED_LU);
        }
    }
}

#[test]
fn case_1_momentary_short_term_integrated() {
    case_1_to_5("1");
}

#[test]
fn case_2_momentary_short_term_integrated() {
    case_1_to_5("2");
}

#[test]
fn case_3_integrated_relative_gate() {
    case_1_to_5("3");
}

#[test]
fn case_4_integrated_absolute_gate() {
    case_1_to_5("4");
}

#[test]
fn case_5_integrated_keeps_the_quieter_tones() {
    case_1_to_5("5");
}

/// Every sub-block value from `from` on, read after each sub-block.
fn per_sub_block(case: &str, short: bool) -> Vec<f64> {
    let g = set("loudness_ebu3341");
    let fs = 48000;
    let mut s = Synth::new(param(&g, &format!("{case}/programme")), fs);
    let from = param(&g, &format!("{case}/from_sub_block"))
        .as_u64()
        .expect("from") as usize;
    let mut m = meter(fs, 2);
    let n = m.sub_block_samples();
    let mut out = Vec::new();
    let mut j = 0;
    while feed(&mut m, &mut s, n, n) == n {
        j += 1;
        if j >= from {
            let r = m.reading();
            out.push(if short { r.short_term } else { r.momentary });
        }
    }
    out
}

#[test]
fn case_9_short_term_constant_after_3_s() {
    let g = set("loudness_ebu3341");
    let got = per_sub_block("case9", true);
    g.assert_f64("case9_short_term", &got);
    for (j, &v) in got.iter().enumerate() {
        assert_near(
            &format!("case 9 short-term at sub-block {}", j + 300),
            v,
            -23.0,
            PUBLISHED_LU,
        );
    }
}

/// Its 0.4 s period does not divide 3 s, so short-term is not asserted.
#[test]
fn case_12_momentary_constant_after_1_s() {
    let g = set("loudness_ebu3341");
    let got = per_sub_block("case12", false);
    g.assert_f64("case12_momentary", &got);
    for (j, &v) in got.iter().enumerate() {
        assert_near(
            &format!("case 12 momentary at sub-block {}", j + 100),
            v,
            -23.0,
            PUBLISHED_LU,
        );
    }
}

/// The live form: one signal after a lead-in and a reset, the maximum read at the end of
/// each segment. Phase 240 puts the reset 5 ms before the signal: the grid's worst phase.
fn live(case: &str) {
    let g = set("loudness_ebu3341");
    let fs = 48000;
    let c = param(&g, &format!("live/case{case}"));
    let published: Vec<f64> = c["published"]
        .as_array()
        .expect("published")
        .iter()
        .map(|v| v.as_f64().expect("value"))
        .collect();
    let want = g.f64(&format!("case{case}_maxima")).expect("maxima");
    let short = c["quantity"] == "max_short_term";
    let mut rows = Vec::new();
    for phase in [0usize, 240] {
        let mut m = meter(fs, 2);
        let mut lead = Synth::new(param(&g, "live/lead_in"), fs);
        feed(&mut m, &mut lead, usize::MAX, BLOCK);
        m.reset();
        if phase > 0 {
            let silence = vec![0.0f32; phase];
            m.process(&[&silence, &silence]);
            assert!(
                !m.reset_pending(),
                "the reset took effect before the signal"
            );
        }
        let mut s = Synth::new(&c["programme"], fs);
        let lengths = s.segment_lengths();
        for (i, seg) in lengths.chunks(3).enumerate() {
            feed(&mut m, &mut s, seg.iter().sum(), BLOCK);
            let r = m.reading();
            let got = if short {
                r.max_short_term
            } else {
                r.max_momentary
            };
            assert_near(
                &format!("3341 case {case} phase {phase} segment {i}"),
                got,
                published[i],
                PUBLISHED_LU,
            );
            rows.push(got);
        }
    }
    g.assert_f64(&format!("case{case}_maxima"), &rows);
    assert_eq!(rows.len(), want.len());
}

#[test]
fn case_11_live_max_short_term() {
    live("11");
}

#[test]
fn case_14_live_max_momentary() {
    live("14");
}
