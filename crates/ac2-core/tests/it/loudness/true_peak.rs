//! EBU Tech 3341 true-peak cases on the meters' BS.1770 Annex 2 detector: 15–19 from the
//! text (analytic), 20–23 against refgen's synthesis with both of its expectations.

use ac2_core::ppm::{TruePeak, dbtp};

use super::set;

const FS: f64 = 48000.0;
/// Tech 3341's tolerance: +0.2 / −0.4 dB.
fn within(what: &str, got: f64, want: f64) {
    assert!(
        (want - 0.4..=want + 0.2).contains(&got),
        "{what}: {got:+.3} dBTP vs {want:+.1} +0.2/−0.4"
    );
}

/// `amp · sin(2π n f/fs + φ)` for 1 s with 10 ms raised-cosine fades (the text names a
/// 10 ms fade, not its shape).
fn tone(divisor: f64, amp: f64, phase_deg: f64) -> Vec<f32> {
    let n = FS as usize;
    let fade = (0.01 * FS) as usize;
    (0..n)
        .map(|k| {
            let edge = k.min(n - 1 - k);
            let g = if edge < fade {
                0.5 - 0.5 * (std::f64::consts::PI * edge as f64 / fade as f64).cos()
            } else {
                1.0
            };
            let w = std::f64::consts::TAU * k as f64 / divisor + phase_deg.to_radians();
            (g * amp * w.sin()) as f32
        })
        .collect()
}

fn max_true_peak(x: &[f32]) -> f64 {
    let mut d = TruePeak::new();
    let peak = x.chunks(480).map(|b| d.process(b)).fold(0.0f32, f32::max);
    f64::from(dbtp(peak))
}

#[test]
fn cases_15_to_19_sines_at_fractions_of_the_rate() {
    for (case, divisor, amp, phase, want) in [
        (15, 4.0, 0.5, 0.0, -6.0),
        (16, 4.0, 0.5, 45.0, -6.0),
        (17, 6.0, 0.5, 60.0, -6.0),
        (18, 8.0, 0.5, 67.5, -6.0),
        (19, 4.0, 1.41, 45.0, 3.0),
    ] {
        within(
            &format!("3341 case {case}"),
            max_true_peak(&tone(divisor, amp, phase)),
            want,
        );
    }
}

#[test]
fn cases_20_to_23_spliced_period_at_four_offsets() {
    let g = set("true_peak_ebu3341");
    for case in 20..=23 {
        let x: Vec<f32> = g
            .f64(&format!("case{case}_window"))
            .expect("window")
            .iter()
            .map(|&v| v as f32)
            .collect();
        let got = max_true_peak(&x);
        within(&format!("3341 case {case} (published)"), got, 0.0);
        let own = g
            .scalar(&format!("case{case}_true_peak_x32_dbtp"))
            .expect("x32");
        within(
            &format!("3341 case {case} (refgen x32 {own:+.3})"),
            got,
            own,
        );
    }
}

/// The detector against the annex's sum written out, `Σₘ PHASES[p][m]·x[n − m]` phase by
/// phase: equal to the bit, whatever the block boundaries, so the reading does not depend
/// on how the samples were laid out for the processor.
#[test]
fn reading_is_the_annex_sum_to_the_bit_across_block_splits() {
    use ac2_core::ppm::{TRUE_PEAK_HISTORY, TRUE_PEAK_PHASES};
    // A fixed pseudo-random signal with full-scale excursions (a linear congruential
    // generator: any sequence does, it only has to be the same every run).
    let mut state = 0x2545_f491u32;
    let x: Vec<f32> = (0..4801)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        })
        .collect();
    let direct = |from: usize, to: usize| {
        let mut max = 0.0f32;
        for n in from..to {
            for phase in &TRUE_PEAK_PHASES {
                let mut acc = 0.0f32;
                for (m, &tap) in phase.iter().enumerate().take(TRUE_PEAK_HISTORY) {
                    if n >= m {
                        acc += tap * x[n - m];
                    }
                }
                max = max.max(acc.abs());
            }
        }
        max
    };
    for block in [1usize, 5, 12, 13, 64, 480, 4801] {
        let mut tp = TruePeak::new();
        let mut from = 0;
        for chunk in x.chunks(block) {
            let got = tp.process(chunk);
            let want = direct(from, from + chunk.len());
            assert_eq!(got.to_bits(), want.to_bits(), "block {block}, from {from}");
            from += chunk.len();
        }
    }
}
