//! The cursor in 3D: a click picks a spike, the cursor marks its cell and reads it on the
//! cursor values' plate.

use super::points::{chrome, flat_room, scene_of, scene_posed};
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

/// A pointer at `pos` on a display of one pixel a point.
fn pt(pos: [f32; 2]) -> Pointer3d {
    Pointer3d {
        pos,
        px_per_pt: 1.0,
    }
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
        let at = pick(&f, sk, pt(p)).expect("on a spike");
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
    assert_eq!(pick(&f, sk, pt([f.plot.x + 2.0, f.plot.y + 2.0])), None);
    assert_eq!(
        pick(&f, sk, pt(tip(&s, sk, col_of(sk, 2000.0), 0))).map(|p| p.hz < 5000.0),
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
    let end = sk.newest_end_s().expect("frames");
    view.spectrum.spectrograph.cursor_at_s = Some(end - row_age(sk, 10));
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
    view.spectrum.spectrograph.cursor_at_s = None;
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
    let at = pick(&f, sk, pt(p)).expect("on a spike");
    let (c, k) = cell_at(sk, at.hz, at.before_s).expect("a cell");
    assert_eq!(
        level(c, k),
        -10.0,
        "picked ({c}, {k}), clicked the tip of ({col}, {j})"
    );
    // Beside the forest's spikes, on the short tip alone, it is the short one.
    let front = tip(&s, sk, col, 0);
    let at = pick(&f, sk, pt(front)).expect("on a spike");
    assert_eq!(cell_at(sk, at.hz, at.before_s), Some((col, 0)));
}

/// Levels at random over −90 … 0 dB below 5 kHz (some under the range's floor), cells
/// without a level here and there, and a gap of empty rows.
fn noisy(tier: SkylineTier) -> (SpectrographHistory, Skyline) {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&log_bins());
    let mut r = Lcg(7);
    let n = h.slots() as u64 + 1;
    for slot in 0..n {
        if slot == n / 2 {
            h.mark_break();
            continue;
        }
        if (n / 2..n / 2 + 20).contains(&slot) {
            continue;
        }
        let l: Vec<f32> = f
            .edges
            .iter()
            .map(|e| match (e.0 < 5000.0, r.next(10)) {
                (false, _) | (_, 0) => f32::NAN,
                _ => -(r.next(90) as f32),
            })
            .collect();
        f.push_with(&mut h, slot, &l, None);
    }
    let mut s = Skyline::default();
    s.update(&h, full_range(), tier);
    (h, s)
}

/// Wherever a pointer picks, the pick, the cursor put from it and the plate it reads are
/// all the one spike under the pointer: from every pose (a low oblique one among them), at
/// every scroll lead and tier.
#[test]
fn a_pick_reads_back_as_the_spike_under_the_pointer() {
    let full = noisy(SkylineTier::Full);
    let lite = noisy(SkylineTier::Lite);
    let poses = [
        (0.0, 12.0, 0.0),
        (-35.0, 4.0, 2.0),
        (50.0, 2.0, -3.0),
        (20.0, 60.0, 0.0),
        (-80.0, 30.0, 4.0),
    ];
    let mut picks = 0;
    for (yaw_deg, pitch_deg, zoom_step) in poses {
        let pose = Pose {
            yaw_deg,
            pitch_deg,
            zoom_step,
        };
        for (lead, tier, px_per_pt) in [
            (0.0, SkylineTier::Full, 1.0),
            (-0.6, SkylineTier::Full, 2.0),
            (5.0, SkylineTier::Lite, 1.25),
        ] {
            let b = if tier == SkylineTier::Full {
                &full
            } else {
                &lite
            };
            let sk = &b.1;
            let view = chrome(PlotChrome::Full);
            let s = scene_posed(b, None, &view, 0.0, (pose, lead, tier));
            let f = s.pick.expect("a picture");
            let pj = Projector::new(f.view_proj, f.plot);
            // Pointers at random over the plot: a pick walks every cell, slow unoptimised.
            let mut r = Lcg(yaw_deg.to_bits() ^ pitch_deg.to_bits() ^ lead.to_bits());
            for _ in 0..120 {
                let p = [
                    f.plot.x + r.next(f.plot.w as u64) as f32,
                    f.plot.y + r.next(f.plot.h as u64) as f32,
                ];
                let ptr = Pointer3d { pos: p, px_per_pt };
                let Some(at) = pick(&f, sk, ptr) else {
                    continue;
                };
                picks += 1;
                let (col, j) = at.cell;
                let why = format!("pose {pose:?} lead {lead} at {p:?}: {at:?}");
                assert_eq!(cell_at(sk, at.hz, at.before_s), Some(at.cell), "{why}");
                let v = sk.row_back(j).expect("a row")[col];
                assert!(v.is_finite(), "{why}");
                let (foot, top) = spike(&f, sk.cols(), sk.rows(), col, j, v).expect("{why}");
                let (a, t) = (pj.point(foot).expect("foot"), pj.point(top).expect("tip"));
                assert!(to_segment(p, a, t) <= PICK_PX, "{why}");
                // The pointer there marks that spike, and put there the cursor marks it
                // and reads its level (a scene for every few picks: a scene is slow
                // unoptimised, and it reads the cell as `cell_at` does).
                if picks % 17 != 1 {
                    continue;
                }
                let mut view = view;
                view.pointer_3d = Some(ptr);
                assert_eq!(
                    scene_posed(b, None, &view, 0.0, (pose, lead, tier)).hover,
                    Some(at)
                );
                view.cursor_hz = Some(at.hz);
                view.spectrum.spectrograph.cursor_at_s = Some(at.at_s);
                let put = scene_posed(b, None, &view, 0.0, (pose, lead, tier));
                let cur = put.cursor.expect("the cursor");
                assert_eq!(cur.cell, at.cell, "{why}");
                assert!(!cur.lines[1].ends_with('—'), "{why}: {:?}", cur.lines);
            }
        }
    }
    assert!(picks > 400, "{picks} picks");
}

/// Live, the cursor stays on the frame it was put on: as newer frames arrive its row moves
/// back, and it reads that row's level at a growing age, until the row leaves the history.
#[test]
fn live_the_cursor_holds_its_frame_as_rows_move_back() {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&log_bins());
    let k = bucket_slots(h.slots(), SkylineTier::Full) as u64;
    // Each row's own level, so the row the cursor reads is known by its level.
    let level = |slot: u64| -10.0 - (slot / k % 50) as f32;
    let mut slot = 0;
    let mut feed = |h: &mut SpectrographHistory, to: u64| {
        while slot <= to {
            f.push(h, slot, level(slot));
            slot += 1;
        }
    };
    let n = h.slots() as u64;
    feed(&mut h, n + k / 2);
    let mut sk = Skyline::default();
    sk.update(&h, full_range(), SkylineTier::Full);
    let view = chrome(PlotChrome::Full);
    let b = (h.clone(), sk.clone());
    let s = scene_of(&b, None, &view, 0.0);
    let col = col_of(&sk, 1000.0);
    let at = pick(&s.pick.expect("a picture"), &sk, pt(tip(&s, &sk, col, 10))).expect("a spike");
    let mut view = view;
    view.cursor_hz = Some(at.hz);
    view.spectrum.spectrograph.cursor_at_s = Some(at.at_s);
    let reads = |b: &(SpectrographHistory, Skyline)| {
        let cur = scene_of(b, None, &view, 0.0).cursor.expect("the cursor");
        let v = b.1.row_back(cur.cell.1).expect("a row")[cur.cell.0];
        (cur.cell, v, cur.before_s)
    };
    let (cell, v, age) = reads(&b);
    assert_eq!(cell, at.cell);
    // Three rows and a bit later: the same frame, three rows back, older by that much.
    feed(&mut h, n + k / 2 + 3 * k + 1);
    sk.update(&h, full_range(), SkylineTier::Full);
    let later = (h.clone(), sk.clone());
    let (cell2, v2, age2) = reads(&later);
    assert_eq!(v2, v, "the same row's level");
    assert_eq!(cell2, (cell.0, cell.1 + 3));
    assert!(
        (age2 - age - (3 * k + 1) as f64 * h.slot_s()).abs() < 1e-9,
        "{age} → {age2}"
    );
    // Once its frame has left the history, no cursor is drawn.
    feed(&mut h, 3 * n);
    sk.update(&h, full_range(), SkylineTier::Full);
    assert!(scene_of(&(h, sk), None, &view, 0.0).cursor.is_none());
}

/// The pointer's spike is marked over the picture, lighter than the cursor's; on the
/// cursor's own cell only the cursor's mark is drawn.
#[test]
fn the_pointer_marks_the_spike_a_click_would_pick() {
    let b = flat_room();
    let sk = &b.1;
    let mut view = chrome(PlotChrome::Full);
    let s = scene_of(&b, None, &view, 0.0);
    let col = col_of(sk, 1000.0);
    let p = pt(tip(&s, sk, col, 0));
    let lines = |s: &Spectrograph3dScene| {
        s.scene
            .layers
            .iter()
            .map(|l| l.polylines.len())
            .sum::<usize>()
    };
    let bare = lines(&s);
    view.pointer_3d = Some(p);
    let over = scene_of(&b, None, &view, 0.0);
    let at = over.hover.expect("the pointer's spike");
    assert_eq!(
        at,
        pick(&s.pick.expect("a picture"), sk, p).expect("a pick")
    );
    assert_eq!(at.cell, (col, 0));
    assert_eq!(lines(&over), bare + 2, "its line and the ring at its tip");
    // A click puts the cursor there: its mark alone (edge and line, edge and box).
    view.cursor_hz = Some(at.hz);
    view.spectrum.spectrograph.cursor_at_s = Some(at.at_s);
    let put = scene_of(&b, None, &view, 0.0);
    assert_eq!(put.cursor.as_ref().map(|c| c.cell), Some(at.cell));
    assert_eq!(lines(&put), bare + 4);
    // Off every spike, or with the pointer gone, nothing is marked.
    view.pointer_3d = Some(pt([s.plot.x + 2.0, s.plot.y + 2.0]));
    assert_eq!(scene_of(&b, None, &view, 0.0).hover, None);
}
