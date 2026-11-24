//! A live curve where the grid is finer than its bins: the estimates are joined across the
//! thinned columns, each drawn as a point, and jumps and peak tracking read them as
//! neighbours.

use super::*;
use crate::cursor_jump::{Landmark, Step};
use ac2_proto::model::BinRange;
use ac2_proto::units::Hz;

/// Columns 11..=34 (about 100–400 Hz on `cols(100)`) are unresolved, every odd one from 13
/// to 31 is thinned.
const LO: usize = 11;
const HI: usize = 34;

fn thinned_cols() -> Cols {
    let mut c = cols(100);
    for i in (13..=31).step_by(2) {
        c.validity[i] = ValidityMask::THINNED;
        c.mag[i] = f32::NAN;
        c.phase[i] = f32::NAN;
        c.coh[i] = f32::NAN;
    }
    c
}

/// Columns `lo..=hi` of `c` as one range on column edges, with `bin_hz`.
fn span(c: &Cols, lo: usize, hi: usize, bin_hz: f64) -> BinRange {
    let edge = |i: usize| c.freqs[i] * 2f64.powf(1.0 / 24.0);
    BinRange {
        lo: Hz(if lo == 0 { c.freqs[0] } else { edge(lo - 1) }),
        hi: Hz(edge(hi)),
        bin_hz: Hz(bin_hz),
    }
}

fn range(c: &Cols) -> Vec<BinRange> {
    vec![span(c, LO, HI, 8.0)]
}

fn joined<'a>(c: &'a Cols, un: &'a [BinRange]) -> TfTrace<'a> {
    TfTrace {
        tracks: true,
        unresolved: Some(crate::unresolved::Source::Joined(un)),
        ..trace(c, TraceKey::Live(MeasId(1)), 0.0)
    }
}

fn scene(t: &TfTrace<'_>, v: &ViewState) -> TfScene {
    transfer_scene(
        std::slice::from_ref(t),
        &DisplayCache::default(),
        &Status::default(),
        v,
        &Theme::dark(),
        SIZE,
    )
}

#[test]
fn the_line_joins_estimates_across_thinned_columns_only() {
    let mut c = thinned_cols();
    // Inside the range a column with no reference energy still breaks the line, and so does
    // a thinned column outside it.
    c.validity[20] = ValidityMask::NO_REFERENCE;
    c.validity[60] = ValidityMask::THINNED;
    let un = range(&c);
    let s = scene(&joined(&c, &un), &ViewState::default());
    let data = &s.scene.layers[1];
    let mag = &data.polylines[0];
    assert_eq!(mag.alpha.len(), mag.points.len());
    for (pane, line) in data.polylines.iter().take(3).enumerate() {
        let segs = segments(&line.points);
        // 0..=19 less the four thinned (13, 15, 17, 19); 21..=59 less six; 61..=99.
        let lens: Vec<usize> = segs.iter().map(Vec::len).collect();
        assert_eq!(lens, [16, 33, 39], "pane {pane}");
    }
    // One point per estimate in the range, in each pane: 11, the even columns 12..=32
    // but 20, 33, 34.
    assert_eq!(data.points.len(), 3);
    for p in &data.points {
        assert_eq!(p.points.len(), 13);
    }
    // The points sit on the line's vertices.
    for p in &data.points[0].points {
        assert!(mag.points.contains(&p.pos), "{:?}", p.pos);
    }
}

#[test]
fn a_curve_without_a_joined_range_draws_as_before() {
    let c = thinned_cols();
    let s = scene(
        &trace(&c, TraceKey::Live(MeasId(1)), 0.0),
        &ViewState::default(),
    );
    let data = &s.scene.layers[1];
    assert!(data.points.is_empty());
    // Every thinned column breaks the line: ten gaps, eleven runs.
    assert_eq!(segments(&data.polylines[0].points).len(), 11);
}

#[test]
fn no_segment_crosses_a_phase_wrap_between_estimates() {
    let mut c = thinned_cols();
    // 24 and 26 straddle the wrap with thinned 25 between them; 22 → 24 does not wrap.
    for i in 0..c.freqs.len() {
        if c.validity[i] == ValidityMask::NONE {
            c.phase[i] = if i <= 24 { 170.0 } else { -170.0 };
        }
    }
    let un = range(&c);
    let s = scene(&joined(&c, &un), &ViewState::default());
    let phase = &s.scene.layers[1].polylines[1];
    let segs = segments(&phase.points);
    let plot = s.panes[1].plot;
    for seg in &segs {
        for w in seg.windows(2) {
            assert!((w[1][1] - w[0][1]).abs() < plot.h / 2.0, "{seg:?}");
        }
    }
    // Joined everywhere else: one break, at the wrap.
    assert_eq!(segs.len(), 2);
}

#[test]
fn the_marks_name_each_ranges_stated_bins() {
    // Two ranges in one frame, as a 1/96 curve at 48 kHz has above the crossovers: each
    // says the width of the bins serving it, whatever its columns' spacing.
    let c = thinned_cols();
    let un = vec![
        span(&c, LO, HI, 12_000.0 / 4096.0),
        span(&c, 50, 60, 48_000.0 / 4096.0),
    ];
    let v = ViewState {
        cursor_hz: Some(c.freqs[55]),
        ..ViewState::default()
    };
    let s = scene(&joined(&c, &un), &v);
    let texts: Vec<&str> = s.unresolved.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(
        texts,
        ["100 Hz–410 Hz: 2.9 Hz bins", "970 Hz–1.8 kHz: 12 Hz bins"]
    );
    let cur = s.cursor.as_ref().expect("cursor");
    assert!(cur.freq.ends_with(" · 12 Hz bins"), "{}", cur.freq);
}

/// A 10 dB room mode on column 22, whose neighbours 21 and 23 are thinned.
fn mode_cols() -> Cols {
    let mut c = thinned_cols();
    c.mag[22] = 10.0;
    c
}

#[test]
fn a_peak_between_thinned_columns_is_jumped_to() {
    let c = mode_cols();
    let un = range(&c);
    let t = [joined(&c, &un)];
    let shown = Range::new(20.0, 20_000.0);
    let found = jump(
        &t,
        &DisplayCache::default(),
        &ViewState::default(),
        800.0,
        shown,
        Landmark::Peak,
        Step::Up,
    );
    assert_eq!(found, Some(c.freqs[22]));
    // Without the joined range the mode has no finite neighbour to stand above.
    let bare = [trace(&c, TraceKey::Live(MeasId(1)), 0.0)];
    let none = jump(
        &bare,
        &DisplayCache::default(),
        &ViewState::default(),
        800.0,
        shown,
        Landmark::Peak,
        Step::Up,
    );
    assert_eq!(none, None);
}

#[test]
fn peak_tracking_finds_the_mode_between_thinned_columns() {
    let c = mode_cols();
    let un = range(&c);
    let s = scene(&joined(&c, &un), &ViewState::default());
    let e = s.extremes.expect("extremes");
    assert!(
        e.max
            .iter()
            .any(|r| (r.freq_hz / c.freqs[22]).log2().abs() < 1.0 / 24.0),
        "{:?}",
        e.max
    );
}

/// The cursor's mark on the mode the jump finds and the cursor on the next one up: the
/// comb readout reads both as landmarks, as the jumps do.
#[test]
fn the_comb_readout_reads_the_bins_the_jump_lands_on() {
    let mut c = thinned_cols();
    // Three equal modes on estimates 16, 22, 28 between thinned columns.
    for i in [16, 22, 28] {
        c.mag[i] = 10.0;
    }
    let un = range(&c);
    let t = [joined(&c, &un)];
    let cache = DisplayCache::default();
    let shown = Range::new(20.0, 20_000.0);
    let mut v = ViewState::default();
    let first = jump(&t, &cache, &v, 800.0, shown, Landmark::Peak, Step::Up).expect("peak");
    v.cursor_hz = Some(first);
    let second = jump(&t, &cache, &v, 800.0, shown, Landmark::Peak, Step::Up).expect("peak");
    assert_eq!((first, second), (c.freqs[16], c.freqs[22]));
    v.cursor_mark_hz = Some(first);
    v.cursor_hz = Some(second);
    let m = scene(&t[0], &v).mark.expect("mark");
    assert!(m.lines.iter().any(|l| l.starts_with("comb")), "{m:?}");
}

#[test]
fn the_cursor_reads_the_nearest_estimate() {
    let c = thinned_cols();
    let un = range(&c);
    // Just below thinned column 21: the readout snaps to an estimate beside it, never to
    // the empty column the line passes through.
    let v = ViewState {
        cursor_hz: Some(c.freqs[21] * 0.999),
        ..ViewState::default()
    };
    let s = scene(&joined(&c, &un), &v);
    let cur = s.cursor.expect("cursor");
    assert_eq!(cur.freq_hz, c.freqs[20]);
    assert_ne!(cur.rows[0].magnitude, format::db_readout(f64::NAN));
    // Without a joined range the column itself is read.
    let bare = scene(&trace(&c, TraceKey::Live(MeasId(1)), 0.0), &v);
    assert_eq!(bare.cursor.expect("cursor").freq_hz, c.freqs[21]);
}

#[test]
fn a_thinned_column_invalid_for_another_reason_breaks_the_line() {
    let mut c = thinned_cols();
    c.validity[21] = ValidityMask(ValidityMask::THINNED.0 | ValidityMask::PROTECTED.0);
    let un = range(&c);
    let s = scene(&joined(&c, &un), &ViewState::default());
    assert_eq!(segments(&s.scene.layers[1].polylines[0].points).len(), 2);
}

#[test]
fn a_range_at_the_top_of_the_columns_joins_to_the_last() {
    let mut c = thinned_cols();
    let n = c.freqs.len();
    for i in (n - 10..n - 1).step_by(2) {
        c.validity[i] = ValidityMask::THINNED;
    }
    let un = vec![span(&c, LO, HI, 8.0), span(&c, n - 12, n - 1, 900.0)];
    let s = scene(&joined(&c, &un), &ViewState::default());
    let mag = &s.scene.layers[1].polylines[0];
    assert_eq!(segments(&mag.points).len(), 1);
    // The last column is an estimate: a point at the curve's end.
    let last = mag.points.last().expect("points");
    assert!(
        s.scene.layers[1].points[0]
            .points
            .iter()
            .any(|p| p.pos == *last)
    );
}

#[test]
fn the_hold_and_the_overlaid_coherence_join_too() {
    let c = thinned_cols();
    let un = range(&c);
    let hold: Vec<f32> = c.mag.iter().map(|m| m + 3.0).collect();
    let t = TfTrace {
        hold: Some(&hold),
        ..joined(&c, &un)
    };
    let v = ViewState {
        tf: crate::view::TfView {
            coherence_placement: CoherencePlacement::OverlayOnMagnitude,
            ..ViewState::default().tf
        },
        ..ViewState::default()
    };
    let s = scene(&t, &v);
    // Magnitude, its hold, the overlaid coherence, then phase: each one unbroken line.
    for (k, line) in s.scene.layers[1].polylines.iter().take(4).enumerate() {
        assert_eq!(segments(&line.points).len(), 1, "line {k}");
    }
}

/// A linear phase (a pure delay) read through a joined range: the group delay there is the
/// delay, fitted over the bins, and outside the range the same numbers as a curve with no
/// thinning at all.
#[test]
fn group_delay_reads_the_bins_and_leaves_the_rest_alone() {
    let delay = 0.002;
    let linear = |c: &mut Cols| {
        for (i, f) in c.freqs.iter().enumerate() {
            c.phase[i] = crate::trace::wrap_deg(-360.0 * f * delay) as f32;
        }
    };
    let mut full = cols(100);
    linear(&mut full);
    let mut thin = cols(100);
    linear(&mut thin);
    for i in (13..=31).step_by(2) {
        thin.validity[i] = ValidityMask::THINNED;
    }
    let un = range(&thin);
    let style = ViewState::default().tf.coherence;
    let gd_full =
        crate::trace::display_trace(&trace(&full, TraceKey::Live(MeasId(1)), 0.0), None, &style)
            .group_delay_s;
    let gd_thin = crate::trace::display_trace(&joined(&thin, &un), None, &style).group_delay_s;
    for (i, g) in gd_thin.iter().enumerate().take(HI + 1).skip(LO) {
        if i % 2 == 0 || !(13..=31).contains(&i) {
            assert!((g - delay).abs() < 1e-6, "{i}: {g}");
        }
    }
    // Outside the range, beyond the fit's reach into it (±1/24 octave, one column here).
    for i in (0..LO - 1).chain(HI + 2..100) {
        assert_eq!(gd_thin[i].to_bits(), gd_full[i].to_bits(), "{i}");
    }
}
