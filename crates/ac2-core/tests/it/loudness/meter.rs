//! The meter's own rules: reset alignment to the sub-block grid, the discontinuity restart,
//! and independence from the audio block size.

use ac2_core::loudness::{
    HOP_SUB_BLOCKS, LoudnessMeter, MOMENTARY_SUB_BLOCKS, SHORT_TERM_SUB_BLOCKS,
};
use serde_json::json;

use super::{Synth, assert_near, feed, meter};

const FS: u32 = 48000;

fn tone(secs: f64, db: f64) -> Synth {
    Synth::new(
        &json!({"freq_hz": 1000, "channels": 2, "segments": [[secs, db, db]]}),
        FS,
    )
}

/// Feeds one sub-block at a time and returns the reading after each.
fn sub_blocks(
    m: &mut LoudnessMeter,
    s: &mut Synth,
    count: usize,
) -> Vec<ac2_core::loudness::LoudnessReading> {
    let n = m.sub_block_samples();
    (0..count)
        .map(|_| {
            assert_eq!(feed(m, s, n, n), n);
            m.reading()
        })
        .collect()
}

#[test]
fn discontinuity_restarts_the_windows_and_keeps_the_integrated() {
    let mut m = meter(FS, 2);
    let n = m.sub_block_samples();
    let mut s = tone(10.0, -20.0);
    // 100 sub-blocks and half of one: gating blocks end at sub-blocks 40, 50, …, 100.
    let before = feed(&mut m, &mut s, 100 * n + n / 2, 1000);
    let r = m.reading();
    assert_eq!(r.gating_blocks, 7);
    let integrated = r.integrated.expect("integrated");
    let max_m = r.max_momentary;

    m.discontinuity();
    let after = sub_blocks(&mut m, &mut s, 350);
    for (j, r) in after.iter().enumerate() {
        let j = j + 1;
        assert!(
            r.integrated.is_some(),
            "integrated carries on across the gap"
        );
        assert!(r.max_momentary >= max_m, "the maxima carry on");
        assert_eq!(
            r.momentary.is_finite(),
            j >= MOMENTARY_SUB_BLOCKS,
            "momentary after {j} resumed sub-blocks"
        );
        assert_eq!(
            r.short_term.is_finite(),
            j >= SHORT_TERM_SUB_BLOCKS,
            "short-term after {j} resumed sub-blocks"
        );
    }
    let last = after.last().expect("readings");
    // The gating grid restarts at the resumed sample: no block spans the gap and the half
    // sub-block before it is dropped.
    let resumed = (350 - MOMENTARY_SUB_BLOCKS) / HOP_SUB_BLOCKS + 1;
    assert_eq!(last.gating_blocks, 7 + resumed as u64);
    assert_eq!(
        last.duration_samples,
        (before + 350 * n) as u64,
        "metered samples only"
    );
    assert_near(
        "integrated",
        last.integrated.expect("integrated"),
        integrated,
        0.001,
    );
    // The K-filter restarted from zero: its start transient is the only difference.
    assert_near("momentary", last.momentary, integrated, 0.001);
}

#[test]
fn reset_takes_effect_at_the_next_sub_block_boundary() {
    let mut m = meter(FS, 2);
    let n = m.sub_block_samples();
    let mut s = tone(20.0, -20.0);
    feed(&mut m, &mut s, 400 * n + 100, 1000);
    let momentary = m.reading().momentary;
    m.reset();
    assert!(m.reset_pending());
    feed(&mut m, &mut s, 200, 200);
    assert!(
        m.reset_pending(),
        "still inside the sub-block the reset arrived in"
    );
    feed(&mut m, &mut s, 400, 400);
    assert!(!m.reset_pending());
    let r = m.reading();
    // The boundary fell n − 300 samples into the last block.
    assert_eq!(r.duration_samples, (400 - (n - 300)) as u64);
    assert_eq!(r.integrated, None);
    assert_eq!(r.range, None);
    assert_eq!(r.max_momentary, f64::NEG_INFINITY);
    assert_eq!(r.gating_blocks, 0);
    // Momentary is signal state: the reset leaves it running.
    assert_near("momentary", r.momentary, momentary, 1e-6);
    // Finish the partial sub-block, then the first gating block counts 400 ms later.
    feed(&mut m, &mut s, n - r.duration_samples as usize, n);
    let readings = sub_blocks(&mut m, &mut s, MOMENTARY_SUB_BLOCKS + 1);
    for (j, r) in readings.iter().enumerate() {
        let since = j + 2; // sub-blocks completed since the reset
        assert_eq!(
            r.integrated.is_some(),
            since >= MOMENTARY_SUB_BLOCKS,
            "after {since}"
        );
        assert_eq!(r.max_momentary.is_finite(), since >= MOMENTARY_SUB_BLOCKS);
    }
}

#[test]
fn reset_on_a_boundary_takes_effect_at_the_next_block() {
    let mut m = meter(FS, 1);
    let n = m.sub_block_samples();
    let mut s = Synth::new(
        &json!({"freq_hz": 1000, "channels": 1, "segments": [[5.0, -20.0, -20.0]]}),
        FS,
    );
    feed(&mut m, &mut s, 100 * n, n);
    m.reset();
    feed(&mut m, &mut s, 7, 7);
    assert!(!m.reset_pending());
    assert_eq!(m.reading().duration_samples, 7);
}

#[test]
fn readings_do_not_depend_on_the_block_size() {
    let prog = json!({"freq_hz": 1000, "channels": 2,
                      "segments": [[2.0, -30.0, -10.0], [2.0, -40.0, -40.0], [1.5, null, null]]});
    let read = |block: usize| {
        let mut m = meter(44100, 2);
        let mut s = Synth::new(&prog, 44100);
        feed(&mut m, &mut s, usize::MAX, block);
        m.reading()
    };
    let a = read(441);
    for block in [1, 64, 1000, 8192] {
        let b = read(block);
        assert_eq!(b.duration_samples, a.duration_samples);
        assert_eq!(b.gating_blocks, a.gating_blocks);
        for (what, x, y) in [
            (
                "integrated",
                b.integrated.expect("I"),
                a.integrated.expect("I"),
            ),
            ("range", b.range.expect("LRA"), a.range.expect("LRA")),
            ("max momentary", b.max_momentary, a.max_momentary),
            ("max short-term", b.max_short_term, a.max_short_term),
            ("short-term", b.short_term, a.short_term),
        ] {
            assert_near(&format!("{what} at block {block}"), x, y, 1e-9);
        }
    }
}
