//! The cursor and the pointer's spike in the 3D view. The cursor is a cell's frame: put by a
//! click on the spike the pointer was marking, it stays on that frame, moving back with its
//! row as newer ones arrive, and reads on the cursor values' plate as on every view.

use super::pick::{cell_at, spike};
use super::scene::Projector;
use super::*;

/// Half the cursor's box at the spike's tip, pixels: a peak point's box and a pixel more,
/// so the two read apart on one spike.
const BOX_HALF: f32 = 5.0;

/// The cursor's spike, pixels: about three times a spike's widest, so it stands out of a
/// forest of them, on a dark edge as wide again that parts it from the bright spikes
/// around and behind it (the ice map's loudest are near white, as the cursor is).
const CURSOR_W: f32 = 2.5;
const EDGE_W: f32 = 2.0;

/// The pointer's spike: a lighter line than the cursor's, and a ring at its tip where the
/// cursor has a box, so the two read apart when both are on screen.
const HOVER_W: f32 = 1.5;
const HOVER_ALPHA: f32 = 0.75;
const RING_R: f32 = 5.0;
const RING_SIDES: usize = 16;

/// The cursor's colour at full strength: the dark theme's (the field is dark in every
/// theme).
fn cursor_colour() -> Color {
    let c = Theme::dark().cursor.color;
    Color::rgb(c.r, c.g, c.b)
}

/// The cursor (the view's frequency and time) on the 3D picture `f` of `s`: its cell's
/// spike drawn in the cursor's colour with a box at its tip (at its foot when the cell has
/// no spike), and its lines for the plate. `None` as [`cursor_at`].
pub(super) fn cursor_cell(
    c: &mut Canvas,
    f: &PickFrame,
    s: &Spectrograph3dInput<'_>,
    view: &ViewState,
) -> Option<Cursor3d> {
    let sk = s.skyline;
    let (hz, before_s, (col, j)) = cursor_at(sk, view)?;
    let v = sk
        .row_back(j)
        .and_then(|r| r.get(col).copied())
        .unwrap_or(f32::NAN);
    let shown = if v.is_finite() {
        f64::from(v) + s.offset_db
    } else {
        f64::NAN
    };
    let level = crate::spectrograph::cursor_level(shown, s.history.unit_noted().as_deref());
    let lines = vec![
        crate::format::freq_readout(hz),
        format!("{} ago  {level}", crate::format::age(before_s)),
    ];
    let pj = Projector::new(f.view_proj, f.plot);
    let (a, b) = ends(f, sk, (col, j), v, &pj);
    let colour = cursor_colour();
    let tip = b.filter(|q| pj.inside(*q));
    if let (Some(a), Some(b)) = (a, b) {
        let edge = Stroke::solid(FIELD.with_alpha(0.85), CURSOR_W + 2.0 * EDGE_W);
        c.overlay
            .polylines
            .push(Polyline::clipped(vec![a, b], edge, f.plot));
        c.overlay.polylines.push(Polyline::clipped(
            vec![a, b],
            Stroke::solid(colour, CURSOR_W),
            f.plot,
        ));
    }
    if let Some([x, y]) = tip {
        let (x0, y0, x1, y1) = (x - BOX_HALF, y - BOX_HALF, x + BOX_HALF, y + BOX_HALF);
        let square = vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]];
        c.overlay.polylines.push(Polyline::clipped(
            square.clone(),
            Stroke::solid(FIELD.with_alpha(0.85), 1.5 + 2.0 * EDGE_W),
            f.plot,
        ));
        c.overlay.polylines.push(Polyline::clipped(
            square,
            Stroke::solid(colour, 1.5),
            f.plot,
        ));
    }
    Some(Cursor3d {
        freq_hz: hz,
        before_s,
        cell: (col, j),
        lines,
        tip,
    })
}

/// The cursor of `view` on skyline `sk`: its frequency, its age (seconds before the newest
/// frame) and its cell. `None` without a cursor, or off the skyline (outside its span, or
/// its frame older than the history).
pub(super) fn cursor_at(sk: &Skyline, view: &ViewState) -> Option<(f64, f64, (usize, usize))> {
    let hz = view.cursor_hz?;
    // Put on frequency alone (Shift+T), the cursor reads the newest row.
    let before_s = match view.spectrum.spectrograph.cursor_at_s {
        Some(at) => (sk.newest_end_s()? - at).max(0.0),
        None => 0.0,
    };
    Some((hz, before_s, cell_at(sk, hz, before_s)?))
}

/// The spike the pointer is on (`at`, picked from this picture): a light line along it and
/// a ring at its tip, what a click would put the cursor on. Not drawn on the cursor's own
/// cell, which its mark already shows.
pub(super) fn hover_cell(
    c: &mut Canvas,
    f: &PickFrame,
    sk: &Skyline,
    at: &Picked,
    cursor: Option<(usize, usize)>,
) {
    if cursor == Some(at.cell) {
        return;
    }
    let (col, j) = at.cell;
    let v = sk
        .row_back(j)
        .and_then(|r| r.get(col).copied())
        .unwrap_or(f32::NAN);
    let pj = Projector::new(f.view_proj, f.plot);
    let (Some(a), Some(b)) = ends(f, sk, at.cell, v, &pj) else {
        return;
    };
    let stroke = Stroke::solid(cursor_colour().with_alpha(HOVER_ALPHA), HOVER_W);
    c.overlay
        .polylines
        .push(Polyline::clipped(vec![a, b], stroke, f.plot));
    let ring = (0..=RING_SIDES)
        .map(|i| {
            let t = i as f32 / RING_SIDES as f32 * std::f32::consts::TAU;
            [b[0] + RING_R * t.cos(), b[1] + RING_R * t.sin()]
        })
        .collect();
    c.overlay
        .polylines
        .push(Polyline::clipped(ring, stroke, f.plot));
}

/// The foot and tip on screen of cell `(col, j)` at level `v`; a cell without a spike
/// has both at its foot.
fn ends(
    f: &PickFrame,
    sk: &Skyline,
    (col, j): (usize, usize),
    v: f32,
    pj: &Projector,
) -> (Option<[f32; 2]>, Option<[f32; 2]>) {
    let (cols, rows) = (sk.cols(), sk.rows());
    let foot = spike_foot(cols, rows, (col, j), f64::from(f.phase), f.newest_id);
    let (foot, top) = spike(f, cols, rows, col, j, v).unwrap_or((foot, foot));
    (pj.point(foot), pj.point(top))
}
