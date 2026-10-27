//! The Goniometer view's field: the dark square, its grid and its L / R labels. The
//! embers are `ac2-plot`'s, drawn in the square.

use crate::canvas::{Canvas, anchor, label};
use crate::gonio;
use crate::primitives::{Color, FillRect, HAlign, Rect, Stroke, VAlign};
use crate::view::{PlotChrome, Style};

/// The goniometer's field: dark in every theme, since embers add light and only read on
/// black.
const FIELD: Color = Color::rgb(0.02, 0.02, 0.025);

/// The field in `sq`: the grid at *full* only, the L / R labels at *full* and *no grid*.
pub(super) fn field(c: &mut Canvas, sq: Rect, style: Style) {
    let theme = style.theme;
    c.base.rects.push(FillRect {
        rect: sq,
        color: FIELD,
        clip: None,
    });
    if style.chrome == PlotChrome::Full {
        let stroke = Stroke::solid(theme.grid_major.color.with_alpha(0.6), 1.0);
        c.base.polylines.extend(gonio::grid_polylines(sq, stroke));
    }
    if style.chrome != PlotChrome::Bare {
        let s = theme.small_font_size;
        for (t, x, h) in [
            ("L", sq.x + 3.0, HAlign::Left),
            ("R", sq.right() - 3.0, HAlign::Right),
        ] {
            c.overlay.labels.push(label(
                t,
                [x, sq.y + 3.0],
                anchor(h, VAlign::Top),
                s,
                theme.text_dim,
            ));
        }
    }
}
