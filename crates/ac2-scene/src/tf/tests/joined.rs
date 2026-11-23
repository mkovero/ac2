//! A live curve where the grid is finer than its bins: the estimates are joined across the
//! thinned columns, each drawn as a point, and jumps and peak tracking read them as
//! neighbours.

use super::*;
use crate::cursor_jump::{Landmark, Step};
use ac2_proto::model::{FreqRange, Resolution, Unresolved};
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

fn range(c: &Cols) -> Unresolved {
    let mid = |i: usize| (c.freqs[i] * c.freqs[i + 1]).sqrt();
    Unresolved {
        resolution: Resolution::FortyEighth,
        ranges: vec![FreqRange {
            lo: Hz(mid(LO - 1)),
            hi: Hz(mid(HI)),
        }],
    }
}

fn joined<'a>(c: &'a Cols, un: &'a Unresolved) -> TfTrace<'a> {
    TfTrace {
        tracks: true,
        unresolved: Some(crate::unresolved::Source {
            unresolved: un,
            fill: crate::unresolved::Fill::Joined {
                freqs: &c.freqs,
                validity: &c.validity,
            },
        }),
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
fn the_mark_names_the_bin_width() {
    // 1 Hz bins (48 kHz / 49152) on a 1/48-octave grid from 20 Hz, unresolved below the
    // frequency where a column grows as wide as a bin.
    let df = 48_000.0 / 49_152.0;
    let freqs: Vec<f64> = (0..400)
        .map(|i| 20.0 * 2f64.powf(f64::from(i) / 48.0))
        .collect();
    let mut c = cols(freqs.len());
    for (i, f) in freqs.iter().enumerate() {
        let (lo, hi) = (f * 2f64.powf(-1.0 / 96.0), f * 2f64.powf(1.0 / 96.0));
        if (lo / df).ceil() * df >= hi {
            c.validity[i] = ValidityMask::THINNED;
        }
    }
    c.freqs = freqs;
    let un = Unresolved {
        resolution: Resolution::FortyEighth,
        ranges: vec![FreqRange {
            lo: Hz(c.freqs[0]),
            hi: Hz(df / (2f64.powf(1.0 / 48.0) - 1.0)),
        }],
    };
    let v = ViewState {
        cursor_hz: Some(40.0),
        ..ViewState::default()
    };
    let s = scene(&joined(&c, &un), &v);
    let texts: Vec<&str> = s.unresolved.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, ["below 67 Hz: 1 Hz bins"]);
    let cur = s.cursor.as_ref().expect("cursor");
    assert!(cur.freq.ends_with(" · 1 Hz bins"), "{}", cur.freq);
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
