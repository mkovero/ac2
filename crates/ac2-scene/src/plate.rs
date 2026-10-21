//! The plates over a plot (the legend, the cursor values, the mark's readout): one rule for
//! all of them. A plate sits where the scene puts it until the operator drags it; then it
//! stays where it was dropped, kept per pane. Its grip scales its text, so a readout can be
//! read across a room. A double-click puts it back to its automatic place and size.

use std::ops::RangeInclusive;

use crate::canvas::{anchor, label, text_width};
use crate::legend::{GRIP, INSET, PAD_X, PAD_Y, ROW};
use crate::primitives::{Color, FillRect, HAlign, Layer, Rect, VAlign};
use crate::theme::Theme;

/// Bounds of a plate's text scale: below the lower one the text is smaller than the plot's
/// own labels; above the upper one a few rows fill a small pane.
pub const SCALE_LIMITS: RangeInclusive<f32> = 0.75..=3.0;

/// Which plate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlateKind {
    /// The curves' names (transfer view).
    Legend,
    /// The cursor's frequency and each curve's value there.
    Readout,
    /// The cursor read against its mark (transfer view; the spectrum lists it with the
    /// cursor values).
    Mark,
}

impl PlateKind {
    pub const ALL: [Self; 3] = [Self::Legend, Self::Readout, Self::Mark];

    /// What it is called in a hint.
    pub fn name(self) -> &'static str {
        match self {
            Self::Legend => "legend",
            Self::Readout => "cursor values",
            Self::Mark => "mark readout",
        }
    }
}

/// Where a plate sits and how large its text is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlatePlace {
    /// `None`: where the scene puts it. Otherwise its place in the room the plot leaves
    /// beside it, `[x, y]`, 0 at the left (top) edge and 1 at the right (bottom). A fraction
    /// of the free room, not of the plot or an offset from a corner, because whatever size
    /// the pane takes (a window resized, a pane split) the plate then stays wholly inside
    /// the plot, and one dropped against an edge stays against it.
    pub at: Option<[f32; 2]>,
    /// Text size, times the plot's ([`SCALE_LIMITS`]).
    pub scale: f32,
}

impl Default for PlatePlace {
    fn default() -> Self {
        Self {
            at: None,
            scale: 1.0,
        }
    }
}

impl PlatePlace {
    /// Moves it to `x`, `y` ([`PlateBox::position_for`]), kept inside the plot.
    pub fn move_to(&mut self, x: f32, y: f32) {
        if x.is_finite() && y.is_finite() {
            self.at = Some([x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)]);
        }
    }

    /// Sets its text scale ([`PlateBox::scale_for`]), within [`SCALE_LIMITS`].
    pub fn set_scale(&mut self, s: f32) {
        if s.is_finite() {
            self.scale = s.clamp(*SCALE_LIMITS.start(), *SCALE_LIMITS.end());
        }
    }

    /// Its scale within bounds, whatever was stored.
    pub fn scale(&self) -> f32 {
        if self.scale.is_finite() {
            self.scale.clamp(*SCALE_LIMITS.start(), *SCALE_LIMITS.end())
        } else {
            1.0
        }
    }

    /// Where it starts: placed by the scene, at the plot's text size.
    pub fn is_auto(&self) -> bool {
        *self == Self::default()
    }

    /// Its top-left in `area` for a plate `w` × `h` placed at `at`.
    pub(crate) fn rect_at(at: [f32; 2], area: Rect, w: f32, h: f32) -> Rect {
        let room_w = (area.w - 2.0 * INSET - w).max(0.0);
        let room_h = (area.h - 2.0 * INSET - h).max(0.0);
        let f = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        Rect::new(
            area.x + INSET + f(at[0]) * room_w,
            area.y + INSET + f(at[1]) * room_h,
            w,
            h,
        )
    }
}

/// The text scale a plate measuring `w` × `h` at scale `s` is drawn at in `area`: a plate
/// enlarged in a big pane keeps its text size until the pane is too small for it, then
/// shrinks with the pane so it stays whole inside the plot (its size is near enough
/// proportional to the scale: text width and rows both are). Never below the theme's
/// size, or the scale set when that is smaller: a plate too large even then is the
/// automatic placement's case of a pane too small, which cuts and scrolls its rows.
pub(crate) fn fitted(s: f32, area: Rect, w: f32, h: f32) -> f32 {
    let room_w = area.w - 2.0 * INSET;
    let room_h = area.h - 2.0 * INSET;
    let k = (room_w / w).min(room_h / h).min(1.0);
    if k.is_finite() && k > 0.0 {
        (s * k).max(s.min(1.0))
    } else {
        s
    }
}

/// A pane's plates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Plates {
    pub legend: PlatePlace,
    pub readout: PlatePlace,
    pub mark: PlatePlace,
}

impl Plates {
    pub fn get(&self, k: PlateKind) -> &PlatePlace {
        match k {
            PlateKind::Legend => &self.legend,
            PlateKind::Readout => &self.readout,
            PlateKind::Mark => &self.mark,
        }
    }

    pub fn get_mut(&mut self, k: PlateKind) -> &mut PlatePlace {
        match k {
            PlateKind::Legend => &mut self.legend,
            PlateKind::Readout => &mut self.readout,
            PlateKind::Mark => &mut self.mark,
        }
    }

    /// Every plate where the scene puts it.
    pub fn is_auto(&self) -> bool {
        PlateKind::ALL.iter().all(|k| self.get(*k).is_auto())
    }
}

/// What the pointer is over: the plate moves it, the grip scales its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlateHover {
    pub kind: PlateKind,
    pub grip: bool,
}

/// Where a plate went, for the mouse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlateBox {
    pub kind: PlateKind,
    /// What it moves within.
    pub area: Rect,
    /// The plate.
    pub rect: Rect,
    /// The scale grip, inside the plate's bottom-right corner.
    pub grip: Rect,
    /// Its text scale as drawn.
    pub scale: f32,
}

impl PlateBox {
    pub(crate) fn new(kind: PlateKind, area: Rect, rect: Rect, scale: f32) -> Self {
        Self {
            kind,
            area,
            rect,
            grip: grip_rect(rect),
            scale,
        }
    }

    /// [`PlatePlace::at`] that puts the plate's top-left at `left`, `top`.
    pub fn position_for(&self, left: f32, top: f32) -> [f32; 2] {
        let f = |p: f32, lo: f32, slack: f32| {
            if slack > 0.0 {
                ((p - lo) / slack).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        [
            f(
                left,
                self.area.x + INSET,
                self.area.w - 2.0 * INSET - self.rect.w,
            ),
            f(
                top,
                self.area.y + INSET,
                self.area.h - 2.0 * INSET - self.rect.h,
            ),
        ]
    }

    /// [`PlatePlace::scale`] that brings the plate's bottom-right corner to `right`,
    /// `bottom`: the plate grows with its text, so the corner follows the pointer along
    /// the plate's diagonal.
    pub fn scale_for(&self, right: f32, bottom: f32) -> f32 {
        let now = self.rect.w + self.rect.h;
        let want = (right - self.rect.x) + (bottom - self.rect.y);
        let s = if now > 0.0 {
            self.scale * want / now
        } else {
            self.scale
        };
        if s.is_finite() {
            s.clamp(*SCALE_LIMITS.start(), *SCALE_LIMITS.end())
        } else {
            self.scale
        }
    }
}

impl PlateBox {
    /// The top-left nearest `left`, `top` at which the plate overlaps none of `others`
    /// (its own kind among them is skipped): dropped on another plate, it slides beside it,
    /// because two plates' text drawn over each other reads as neither. When the plot has
    /// no clear place for it, `left`, `top` as asked.
    pub fn clear_of(&self, left: f32, top: f32, others: &[PlateBox]) -> (f32, f32) {
        let others: Vec<Rect> = others
            .iter()
            .filter(|o| o.kind != self.kind)
            .map(|o| o.rect)
            .collect();
        let (w, h) = (self.rect.w, self.rect.h);
        let x_hi = (self.area.right() - INSET - w).max(self.area.x + INSET);
        let y_hi = (self.area.bottom() - INSET - h).max(self.area.y + INSET);
        let xs = |v: f32| v.clamp(self.area.x + INSET, x_hi);
        let ys = |v: f32| v.clamp(self.area.y + INSET, y_hi);
        let (left, top) = (xs(left), ys(top));
        let mut cx = vec![left];
        let mut cy = vec![top];
        for o in &others {
            cx.extend([xs(o.x - INSET - w), xs(o.right() + INSET)]);
            cy.extend([ys(o.y - INSET - h), ys(o.bottom() + INSET)]);
        }
        let clear = |x: f32, y: f32| {
            let r = Rect::new(x, y, w, h);
            !others.iter().any(|o| overlap(&r, o))
        };
        cx.iter()
            .flat_map(|&x| cy.iter().map(move |&y| (x, y)))
            .filter(|&(x, y)| clear(x, y))
            .min_by(|a, b| {
                let d = |p: &(f32, f32)| (p.0 - left).powi(2) + (p.1 - top).powi(2);
                d(a).total_cmp(&d(b))
            })
            .unwrap_or((left, top))
    }

    /// [`Self::scale_for`], stopped where the growing plate would run into one of `others`
    /// it does not already overlap: the grip enlarges the text up to its neighbour, not
    /// over it. Shrinking is never stopped.
    pub fn scale_clear_of(&self, right: f32, bottom: f32, others: &[PlateBox]) -> f32 {
        let want = self.scale_for(right, bottom);
        if want <= self.scale || self.scale <= 0.0 {
            return want;
        }
        let others: Vec<Rect> = others
            .iter()
            .filter(|o| o.kind != self.kind && !overlap(&self.rect, &o.rect))
            .map(|o| o.rect)
            .collect();
        let clear = |s: f32| {
            let k = s / self.scale;
            let r = Rect::new(self.rect.x, self.rect.y, self.rect.w * k, self.rect.h * k);
            !others.iter().any(|o| overlap(&r, o))
        };
        if clear(want) {
            return want;
        }
        // The plate grows monotonically with its scale: halve the interval to the largest
        // clear scale (a twelfth halving is well under a pixel).
        let (mut lo, mut hi) = (self.scale, want);
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            if clear(mid) { lo = mid } else { hi = mid }
        }
        lo
    }
}

/// Whether `a` and `b` share any area (touching edges do not).
fn overlap(a: &Rect, b: &Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

/// The plate at `x`, `y` in `plates` (drawn in order, so the last is on top), and whether
/// on its grip.
pub fn hover_at(plates: &[PlateBox], x: f32, y: f32) -> Option<PlateHover> {
    let inside = |r: &Rect| (r.x..=r.right()).contains(&x) && (r.y..=r.bottom()).contains(&y);
    plates
        .iter()
        .rev()
        .find(|p| inside(&p.rect))
        .map(|p| PlateHover {
            kind: p.kind,
            grip: inside(&p.grip),
        })
}

/// The grip's square, inside `rect`'s bottom-right corner.
pub(crate) fn grip_rect(rect: Rect) -> Rect {
    Rect::new(rect.right() - GRIP, rect.bottom() - GRIP, GRIP, GRIP)
}

/// Sizes of a plate's text and spacing at scale `s`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Metrics {
    pub font: f32,
    pub row: f32,
    pub pad_x: f32,
    pub pad_y: f32,
}

impl Metrics {
    pub fn new(theme: &Theme, s: f32) -> Self {
        Self::of(theme.small_font_size, s)
    }

    /// At a plot font of `font`.
    pub fn of(font: f32, s: f32) -> Self {
        Self {
            font: font * s,
            row: ROW * s,
            pad_x: PAD_X * s,
            pad_y: PAD_Y * s,
        }
    }

    /// The same plate at `k` times the size.
    pub fn scaled(self, k: f32) -> Self {
        Self {
            font: self.font * k,
            row: self.row * k,
            pad_x: self.pad_x * k,
            pad_y: self.pad_y * k,
        }
    }

    /// A plate's size for `lines`.
    pub fn size<S: AsRef<str>>(&self, lines: &[S]) -> (f32, f32) {
        let w = lines
            .iter()
            .map(|l| text_width(l.as_ref(), self.font))
            .fold(0.0f32, f32::max)
            + 2.0 * self.pad_x;
        (w, lines.len() as f32 * self.row + 2.0 * self.pad_y)
    }
}

/// Draws a plate at `rect` with `lines` (text, colour) a row each, aligned `h`; with
/// `grip` (one the operator moves) its grip, lit while the pointer is on it (`hover`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_lines(
    layer: &mut Layer,
    rect: Rect,
    lines: &[(String, Color)],
    h: HAlign,
    m: Metrics,
    grip_shown: bool,
    hover: Option<PlateHover>,
    clip: Rect,
    theme: &Theme,
) {
    crate::legend::plate(layer, rect, hover.is_some(), clip, theme);
    let x = match h {
        HAlign::Right => rect.right() - m.pad_x,
        _ => rect.x + m.pad_x,
    };
    for (k, (t, color)) in lines.iter().enumerate() {
        let y = rect.y + m.pad_y + m.row * (k as f32 + 0.5);
        let mut l = label(t.clone(), [x, y], anchor(h, VAlign::Center), m.font, *color);
        l.clip = Some(clip);
        layer.labels.push(l);
    }
    if grip_shown {
        grip(layer, rect, hover.is_some_and(|h| h.grip), clip, theme);
    }
}

/// The grip: three dots along the corner's diagonal, brighter while the pointer is on it.
pub(crate) fn grip(layer: &mut Layer, rect: Rect, hot: bool, clip: Rect, theme: &Theme) {
    let dot = if hot { theme.text } else { theme.text_dim };
    let (gx, gy) = (rect.right() - 3.0, rect.bottom() - 3.0);
    for (dx, dy) in [(0.0, 0.0), (-4.0, 0.0), (0.0, -4.0)] {
        layer.rects.push(FillRect {
            rect: Rect::new(gx + dx - 2.0, gy + dy - 2.0, 2.0, 2.0),
            color: dot,
            clip: Some(clip),
        });
    }
}

/// The tooltip while the pointer is on a plate: what a drag there does, and the
/// double-click.
pub fn hint(h: PlateHover) -> String {
    let name = h.kind.name();
    if h.grip {
        format!("drag: text size of the {name} · double-click: back to its place and size")
    } else {
        format!(
            "drag: move the {name} · its corner: text size · double-click: back to its place and size"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(50.0, 30.0, 800.0, 300.0);

    fn plate_box(rect: Rect) -> PlateBox {
        PlateBox::new(PlateKind::Readout, AREA, rect, 1.0)
    }

    /// A place is a fraction of the room beside the plate: the edges are 0 and 1 and the
    /// plate stays inside however the plot changes.
    #[test]
    fn place_is_a_fraction_of_the_free_room() {
        let r = PlatePlace::rect_at([1.0, 1.0], AREA, 100.0, 40.0);
        assert_eq!(r.right(), AREA.right() - INSET);
        assert_eq!(r.bottom(), AREA.bottom() - INSET);
        let b = plate_box(r);
        assert_eq!(b.position_for(r.x, r.y), [1.0, 1.0]);
        assert_eq!(b.position_for(-1e6, 1e6), [0.0, 1.0]);
        let mid = PlatePlace::rect_at([0.5, 0.5], AREA, 100.0, 40.0);
        assert_eq!(b.position_for(mid.x, mid.y), [0.5, 0.5]);
        // A smaller plot: still wholly inside, still in the corner.
        let small = Rect::new(0.0, 0.0, 200.0, 100.0);
        let r = PlatePlace::rect_at([1.0, 1.0], small, 100.0, 40.0);
        assert_eq!(r.right(), small.right() - INSET);
        assert_eq!(r.bottom(), small.bottom() - INSET);
        // Larger than the plot: from its top-left inset.
        let r = PlatePlace::rect_at([0.7, 0.7], small, 400.0, 400.0);
        assert_eq!([r.x, r.y], [INSET, INSET]);
    }

    /// Dragging the grip out along the diagonal scales the text, within bounds.
    #[test]
    fn grip_scales_within_bounds() {
        let b = plate_box(Rect::new(100.0, 100.0, 100.0, 50.0));
        assert_eq!(b.scale_for(300.0, 200.0), 2.0);
        assert_eq!(b.scale_for(100.0, 100.0), 0.75);
        assert_eq!(b.scale_for(1e6, 1e6), 3.0);
        let mut p = PlatePlace::default();
        p.set_scale(9.0);
        assert_eq!(p.scale, 3.0);
        p.set_scale(f32::NAN);
        assert_eq!(p.scale, 3.0);
        p.move_to(2.0, -1.0);
        assert_eq!(p.at, Some([1.0, 0.0]));
        assert!(!p.is_auto());
        assert!(PlatePlace::default().is_auto());
    }

    #[test]
    fn hover_takes_the_top_plate_and_its_grip() {
        let a = plate_box(Rect::new(100.0, 100.0, 100.0, 50.0));
        let b = PlateBox::new(
            PlateKind::Mark,
            AREA,
            Rect::new(150.0, 120.0, 100.0, 50.0),
            1.0,
        );
        let plates = [a, b];
        assert_eq!(
            hover_at(&plates, 160.0, 130.0),
            Some(PlateHover {
                kind: PlateKind::Mark,
                grip: false
            })
        );
        assert_eq!(
            hover_at(&plates, 105.0, 105.0).map(|h| h.kind),
            Some(PlateKind::Readout)
        );
        assert_eq!(hover_at(&plates, 245.0, 165.0).map(|h| h.grip), Some(true));
        assert_eq!(hover_at(&plates, 10.0, 10.0), None);
    }

    /// Dropped on another plate, a plate slides to the nearest clear place beside it;
    /// grown by its grip, it stops at its neighbour.
    #[test]
    fn plates_keep_clear_of_each_other() {
        let a = plate_box(Rect::new(100.0, 100.0, 100.0, 50.0));
        let m = PlateBox::new(
            PlateKind::Mark,
            AREA,
            Rect::new(300.0, 200.0, 100.0, 50.0),
            1.0,
        );
        let plates = [a, m];
        // Dropped on the readout: the nearest clear place is just below it.
        let (x, y) = m.clear_of(110.0, 110.0, &plates);
        assert_eq!((x, y), (110.0, 150.0 + INSET));
        // A clear place is kept as asked.
        assert_eq!(m.clear_of(600.0, 100.0, &plates), (600.0, 100.0));
        // Its own kind does not push it.
        assert_eq!(a.clear_of(120.0, 100.0, &plates), (120.0, 100.0));
        // The grip grows the readout towards the mark only up to it.
        let s = a.scale_clear_of(700.0, 400.0, &plates);
        let k = s / a.scale;
        assert!(s > 1.0 && s < 3.0, "{s}");
        let grown = Rect::new(100.0, 100.0, 100.0 * k, 50.0 * k);
        assert!(!overlap(&grown, &m.rect));
        assert!(k > 1.99, "{k}");
        // Shrinking is never stopped.
        assert_eq!(a.scale_clear_of(100.0, 100.0, &plates), 0.75);
    }
}
