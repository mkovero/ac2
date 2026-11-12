//! The cursor in the 3D view. Live, which spike a pointer is over is ambiguous: rows behind
//! show through the gaps and move. Stopped (S), the picture stands still, and a click picks
//! the spike nearest the pointer; the cursor is then that cell, marked on its spike and
//! read on the cursor values' plate as on every view. Live, the cursor stays at its age
//! while the rows move through it.

use super::scene::Projector;
use super::*;

/// What a click needs of the picture it lands on: the projection and the rows' places.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickFrame {
    pub view_proj: ViewProj,
    pub plot: Rect,
    /// How far the front row stood back, rows ([`DrawnRows::phase`]).
    pub phase: f32,
    /// The level range the heights span, as stored (the display offset taken off).
    pub range: [f64; 2],
}

/// A picked cell: its column's centre frequency and its row's middle age, seconds before
/// the newest frame (what the 2D view's cursor reads).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Picked {
    pub hz: f64,
    pub before_s: f64,
}

/// How near a spike the pointer must be, pixels: about a spike's width either side and
/// a little more, small enough not to reach the next column on a wide pane.
pub const PICK_PX: f32 = 6.0;

/// Spikes this near alike, pixels, are both under the pointer.
const TIE_PX: f32 = 1.0;

/// The spike of skyline `sk`, drawn by `f`, nearest `pos` within [`PICK_PX`]: a spike is
/// its line from the floor to its tip, as drawn. Of spikes as near, the one whose tip is
/// nearer the pointer, then the one nearer the eye. `None` off every spike.
pub fn pick(f: &PickFrame, sk: &Skyline, pos: [f32; 2]) -> Option<Picked> {
    let (cols, rows) = (sk.cols(), sk.rows());
    let spans = f.range[1].partial_cmp(&f.range[0]) == Some(std::cmp::Ordering::Greater);
    if cols == 0 || rows == 0 || !spans {
        return None;
    }
    let pj = Projector::new(f.view_proj, f.plot);
    // (distance to the spike, to its tip, depth from the eye, column, row back)
    let mut best: Option<(f32, f32, f64, usize, usize)> = None;
    for j in 0..rows {
        let Some(row) = sk.row_back(j) else { continue };
        for (col, &v) in row.iter().enumerate().take(cols) {
            let Some((foot, tip)) = spike(f, cols, rows, col, j, v) else {
                continue;
            };
            let (Some(a), Some(b)) = (pj.point(foot), pj.point(tip)) else {
                continue;
            };
            let d = to_segment(pos, a, b);
            if d > PICK_PX {
                continue;
            }
            let t = to_segment(pos, b, b);
            let w = f.view_proj.clip(foot)[3];
            // Spikes as near: the one whose tip is at the pointer (a level is read at the
            // tip), then the nearer the eye.
            let better = best.is_none_or(|(bd, bt, bw, _, _)| {
                d < bd - TIE_PX
                    || (d <= bd + TIE_PX && (t < bt - TIE_PX || (t <= bt + TIE_PX && w < bw)))
            });
            if better {
                best = Some((d, t, w, col, j));
            }
        }
    }
    let (_, _, _, col, j) = best?;
    Some(Picked {
        hz: col_hz(sk, col)?,
        before_s: row_age(sk, j),
    })
}

/// The cell the cursor (`hz`, `before_s` seconds before the newest frame) is in: its
/// column and how many rows back from the newest. `None` outside the span or the ring.
pub fn cell_at(sk: &Skyline, hz: f64, before_s: f64) -> Option<(usize, usize)> {
    let (lo, hi) = sk.span_hz()?;
    let cols = sk.cols();
    if cols == 0 || !(lo..=hi).contains(&hz) || !before_s.is_finite() || before_s < 0.0 {
        return None;
    }
    let col = (((hz / lo).ln() / (hi / lo).ln() * cols as f64).floor() as usize).min(cols - 1);
    let row_s = sk.row_s();
    let newest = f64::from(sk.phase()) * row_s;
    let j = if before_s < newest || row_s <= 0.0 {
        0
    } else {
        ((before_s - newest) / row_s).floor() as usize + 1
    };
    (j < sk.rows()).then_some((col, j))
}

/// Centre frequency of column `col`, Hz (log-spaced columns).
pub fn col_hz(sk: &Skyline, col: usize) -> Option<f64> {
    let (lo, hi) = sk.span_hz()?;
    Some(lo * (hi / lo).powf((col as f64 + 0.5) / sk.cols().max(1) as f64))
}

/// Middle age of the row `j` rows back, seconds before the newest frame: the newest row
/// holds only the slots filled so far ([`Skyline::phase`]), each row behind a whole row.
pub fn row_age(sk: &Skyline, j: usize) -> f64 {
    let f = f64::from(sk.phase());
    let row_s = sk.row_s();
    if j == 0 {
        f * row_s / 2.0
    } else {
        (f + j as f64 - 0.5) * row_s
    }
}

/// The spike of the cell at column `col`, `j` rows back, level `v` (as stored), as the
/// renderer stands it: its foot on the floor and its tip. `None` for a cell without one
/// (no level, or at or under the range's floor).
pub(super) fn spike(
    f: &PickFrame,
    cols: usize,
    rows: usize,
    col: usize,
    j: usize,
    v: f32,
) -> Option<(V3, V3)> {
    if !v.is_finite() {
        return None;
    }
    let n = ((f64::from(v) - f.range[0]) / (f.range[1] - f.range[0])).clamp(0.0, 1.0);
    if n <= 0.0 {
        return None;
    }
    let pitch = 2.0 * FLOOR_HALF_W / cols as f64;
    let x = -FLOOR_HALF_W + (col as f64 + 0.5) * pitch;
    let z = (j as f64 + f64::from(f.phase)) * FLOOR_DEPTH / rows as f64;
    Some(([x, 0.0, z], [x, MAX_HEIGHT * n, z]))
}

/// Half the cursor's box at the spike's tip, pixels: a peak point's box and a pixel more,
/// so the two read apart on one spike.
const BOX_HALF: f32 = 5.0;

/// The cursor (the view's frequency and time) on the 3D picture `f` of `s`: its cell's
/// spike drawn in the cursor's colour with a box at its tip (at its foot when the cell has
/// no spike), and its lines for the plate. `None` without a cursor or off the skyline.
pub(super) fn cursor_cell(
    c: &mut Canvas,
    f: &PickFrame,
    s: &Spectrograph3dInput<'_>,
    view: &ViewState,
) -> Option<Cursor3d> {
    let sk = s.skyline;
    let hz = view.cursor_hz?;
    // Put on frequency alone (Shift+T), the cursor reads the newest row.
    let before_s = view.spectrum.spectrograph.cursor_s.unwrap_or(0.0);
    let (col, j) = cell_at(sk, hz, before_s)?;
    let (cols, rows) = (sk.cols(), sk.rows());
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
    let pitch = 2.0 * FLOOR_HALF_W / cols as f64;
    let foot = [
        -FLOOR_HALF_W + (col as f64 + 0.5) * pitch,
        0.0,
        (j as f64 + f64::from(f.phase)) * FLOOR_DEPTH / rows as f64,
    ];
    let (foot, top) = spike(f, cols, rows, col, j, v).unwrap_or((foot, foot));
    // The field is dark in every theme: the dark theme's cursor colour.
    let stroke = Theme::dark().cursor;
    let (a, b) = (pj.point(foot), pj.point(top));
    if let (Some(a), Some(b)) = (a, b) {
        c.overlay
            .polylines
            .push(Polyline::clipped(vec![a, b], stroke, f.plot));
    }
    let tip = b.filter(|q| pj.inside(*q));
    if let Some([x, y]) = tip {
        let (x0, y0, x1, y1) = (x - BOX_HALF, y - BOX_HALF, x + BOX_HALF, y + BOX_HALF);
        c.overlay.polylines.push(Polyline::clipped(
            vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]],
            Stroke::solid(stroke.color, 1.5),
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

/// Distance from `p` to the segment `a`–`b`, pixels.
fn to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (x, y) = (a[0] + t * dx, a[1] + t * dy);
    ((p[0] - x).powi(2) + (p[1] - y).powi(2)).sqrt()
}
