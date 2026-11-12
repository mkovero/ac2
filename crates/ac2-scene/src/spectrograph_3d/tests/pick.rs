//! The cursor in 3D: a click picks a spike, the cursor marks its cell and reads it on the
//! cursor values' plate.

use super::points::{chrome, flat_room, scene_of};
use super::*;
use crate::plate::PlateKind;
use crate::spectrograph_3d::pick::{spike, to_segment};
use crate::spectrograph_3d::scene::Projector;

/// Where the tip of the spike at column `col`, `j` rows back stands on screen.
fn tip(s: &Spectrograph3dScene, sk: &Skyline, col: usize, j: usize) -> [f32; 2] {
    let f = s.pick.expect("a picture");
    let v = sk.row_back(j).expect("a row")[col];
    let (_, top) = spike(&f, sk.cols(), sk.rows(), col, j, v).expect("a spike");
    Projector::new(f.view_proj, f.plot)
        .point(top)
        .expect("before the eye")
}

fn col_of(sk: &Skyline, hz: f64) -> usize {
    cell_at(sk, hz, 0.0).expect("in the span").0
}

#[test]
fn a_click_on_a_spikes_tip_picks_that_cell() {
    let b = flat_room();
    let sk = &b.1;
    let s = scene_of(&b, None, &chrome(PlotChrome::Full), 0.0);
    let f = s.pick.expect("a picture");
    let col = col_of(sk, 1000.0);
    // The front row's tip is its own; further back, the tips of a column and its
    // neighbours stand within a pixel or two of each other from the default camera, their
    // bright caps overlapping, and a nearer row's is less faded: the brightest at the
    // pointer is picked, as it is seen.
    for j in [0, 10, 40] {
        let p = tip(&s, sk, col, j);
        let at = pick(&f, sk, p).expect("on a spike");
        let (c, k) = cell_at(sk, at.hz, at.before_s).expect("a cell");
        assert!(c.abs_diff(col) <= usize::from(j > 0), "row {j}: column {c}");
        assert!(k <= j, "row {j}: picked {k}, behind it");
        let q = tip(&s, sk, c, k);
        assert!(
            (q[0] - p[0]).hypot(q[1] - p[1]) <= 2.0,
            "row {j}: picked {k}'s tip {q:?}, clicked {p:?}"
        );
        if j == 0 {
            assert_eq!(k, 0);
        }
        assert!((at.hz - col_hz(sk, c).expect("span")).abs() < 1e-9);
        assert!((at.before_s - row_age(sk, k)).abs() < 1e-9);
    }
    // Off every spike: the field above them, and the columns without a level (≥ 5 kHz).
    assert_eq!(pick(&f, sk, [f.plot.x + 2.0, f.plot.y + 2.0]), None);
    assert_eq!(
        pick(&f, sk, tip(&s, sk, col_of(sk, 2000.0), 0)).map(|p| p.hz < 5000.0),
        Some(true)
    );
}

#[test]
fn a_row_and_column_read_back_to_their_cell() {
    let (_, sk) = flat_room();
    for j in 0..sk.rows() {
        assert_eq!(cell_at(&sk, 1000.0, row_age(&sk, j)).map(|c| c.1), Some(j));
    }
    for col in 0..sk.cols() {
        let hz = col_hz(&sk, col).expect("span");
        assert_eq!(cell_at(&sk, hz, 0.0).map(|c| c.0), Some(col));
    }
    assert_eq!(cell_at(&sk, 10.0, 0.0), None, "under the span");
    assert_eq!(cell_at(&sk, 1000.0, 1e6), None, "older than the ring");
}

#[test]
fn the_cursor_marks_its_cell_and_reads_it_on_the_plate() {
    let b = flat_room();
    let sk = &b.1;
    let col = col_of(sk, 1000.0);
    let mut view = chrome(PlotChrome::Full);
    view.cursor_hz = col_hz(sk, col);
    view.spectrum.spectrograph.cursor_s = Some(row_age(sk, 10));
    let s = scene_of(&b, None, &view, 0.0);
    let cur = s.cursor.as_ref().expect("the cursor");
    assert_eq!(cur.cell, (col, 10));
    assert_eq!(cur.lines.len(), 2);
    assert!(cur.lines[0].contains("Hz"), "{:?}", cur.lines);
    assert!(
        cur.lines[1].contains(" ago  ") && cur.lines[1].contains("−30.0"),
        "{:?}",
        cur.lines
    );
    let t = cur.tip.expect("on screen");
    let want = tip(&s, sk, col, 10);
    assert!((t[0] - want[0]).abs() < 0.01 && (t[1] - want[1]).abs() < 0.01);
    let kinds: Vec<_> = s.plates().iter().map(|p| p.kind).collect();
    assert_eq!(kinds, [PlateKind::Legend, PlateKind::Readout]);
    // At rest, without the axes, the cursor stays as the points do.
    let rest = scene_of(&b, None, &view, 0.0);
    assert!(rest.cursor.is_some());
    // Frequency alone (the cursor's toggle): the newest row.
    view.spectrum.spectrograph.cursor_s = None;
    let newest = scene_of(&b, None, &view, 0.0);
    assert_eq!(newest.cursor.expect("the cursor").cell, (col, 0));
    // Traces only (T): no cursor and no plate for it.
    view.chrome = PlotChrome::Bare;
    let bare = scene_of(&b, None, &view, 1.0);
    assert!(bare.cursor.is_none() && bare.readout_box.is_none());
}

/// A forest of tall spikes (−10 dB) below 5 kHz, with a band of short ones (−60 dB) a
/// third of the way back in time.
fn forest() -> (SpectrographHistory, Skyline) {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&log_bins());
    let n = h.slots() as u64 + 1;
    for slot in 0..n {
        let short = (n / 3..n / 2).contains(&slot);
        let l: Vec<f32> = f
            .edges
            .iter()
            .map(|e| match (e.0 < 5000.0, short) {
                (false, _) => f32::NAN,
                (true, true) => -60.0,
                (true, false) => -10.0,
            })
            .collect();
        f.push_with(&mut h, slot, &l, None);
    }
    let mut s = Skyline::default();
    s.update(&h, full_range(), SkylineTier::Full);
    (h, s)
}

#[test]
fn a_click_on_a_tall_spikes_body_picks_it_over_a_short_tip_there() {
    let b = forest();
    let sk = &b.1;
    let s = scene_of(&b, None, &chrome(PlotChrome::Full), 0.0);
    let f = s.pick.expect("a picture");
    let pj = Projector::new(f.view_proj, f.plot);
    let col = col_of(sk, 1000.0);
    let level = |c: usize, j: usize| sk.row_back(j).expect("a row")[c];
    // A short spike whose tip stands on a tall one's body: the tall one is what the pixel
    // shows, and the old rule (tip nearest the pointer) took the short one.
    let on_body = |p: [f32; 2]| {
        (0..sk.rows()).any(|j| {
            (col.saturating_sub(1)..=col + 1).any(|c| {
                let v = level(c, j);
                v == -10.0
                    && spike(&f, sk.cols(), sk.rows(), c, j, v).is_some_and(|(a, t)| {
                        let (a, t) = (pj.point(a).expect("foot"), pj.point(t).expect("tip"));
                        to_segment(p, a, t) < 0.5 && (t[0] - p[0]).hypot(t[1] - p[1]) > 10.0
                    })
            })
        })
    };
    let (j, p) = (0..sk.rows())
        .filter(|&j| level(col, j) == -60.0)
        .map(|j| (j, tip(&s, sk, col, j)))
        .find(|(_, p)| on_body(*p))
        .expect("a short tip on a tall body");
    let at = pick(&f, sk, p).expect("on a spike");
    let (c, k) = cell_at(sk, at.hz, at.before_s).expect("a cell");
    assert_eq!(
        level(c, k),
        -10.0,
        "picked ({c}, {k}), clicked the tip of ({col}, {j})"
    );
    // Beside the forest's spikes, on the short tip alone, it is the short one.
    let front = tip(&s, sk, col, 0);
    let at = pick(&f, sk, front).expect("on a spike");
    assert_eq!(cell_at(sk, at.hz, at.before_s), Some((col, 0)));
}
