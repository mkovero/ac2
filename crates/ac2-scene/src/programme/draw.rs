//! The Programme pane as primitives: the Meters view ([`super::meters`]) with its
//! correlation strip and the loudness plate docked beside or above the bars or floating
//! where the operator put it, or the Goniometer view ([`super::gonio_view`]) with the chip
//! under the square. The embers themselves are `ac2-plot`'s (`embers::ember_points`),
//! placed in the square this returns.

use crate::canvas::{Canvas, anchor, cut_to, label, text_width};
use crate::plate::{PlateBox, PlateKind, PlatePlace};
use crate::primitives::{FillRect, HAlign, Rect, Scene, VAlign, Viewport};
use crate::theme::Theme;
use crate::view::{PlotChrome, ProgrammeMode, Style};

use super::layout::ProgrammeLayout;
use super::{Correlation, ProgrammePane, loudness_plate};

/// The pane's picture, where its embers go, and its plate.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgrammeScene {
    pub scene: Scene,
    /// The goniometer's square, when the pane shows one: the embers are drawn here.
    pub gonio: Option<Rect>,
    /// The loudness plate as drawn; `None` when hidden by **T**, not shown, or in the
    /// Goniometer view, so a plate that is not on screen cannot be grabbed.
    pub plate: Option<PlateBox>,
}

impl ProgrammeScene {
    /// The plates the operator moves, in drawing order (the last on top).
    pub fn plates(&self) -> Vec<PlateBox> {
        self.plate.into_iter().collect()
    }
}

/// Builds the pane at `size`. `missing`: what the pane says instead of its meters when the
/// inputs it names are not in the session ([`super::missing_text`]).
pub fn programme_scene(
    p: &ProgrammePane,
    style: Style,
    size: Viewport,
    missing: Option<&str>,
) -> ProgrammeScene {
    let theme = style.theme;
    let mut c = Canvas::new(size, theme);
    let pad = 10.0;
    let area = Rect::new(
        pad,
        pad,
        (size.width - 2.0 * pad).max(1.0),
        (size.height - 2.0 * pad).max(1.0),
    );
    if let Some(m) = missing {
        c.overlay.labels.push(label(
            cut_to(m, area.w, theme.font_size),
            [area.x + area.w / 2.0, area.y + area.h / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            theme.font_size,
            theme.text_dim,
        ));
        return ProgrammeScene {
            scene: c.into_scene(size),
            gonio: None,
            plate: None,
        };
    }
    let l = p.layout(area, theme.font_size, theme.small_font_size, style.chrome);
    let square = l.gonio;
    if let Some(sq) = square {
        super::gonio_view::field(&mut c, sq, style);
    }
    if let (Some(r), Some(pair)) = (l.correlation, &p.pair) {
        chip(&mut c, r, pair.correlation, theme);
    }
    let mut plate_box = None;
    if p.shown_view() == ProgrammeMode::Meters {
        super::meters::bars(&mut c, p, l.bars, style);
        // The plate goes at *traces only* wherever it sits (graphs hide their cursor values
        // at the same step); a docked one's room went to the bars in the layout.
        plate_box = (style.chrome != PlotChrome::Bare)
            .then(|| plate(&mut c, p, &l, area, theme))
            .flatten();
    }
    ProgrammeScene {
        scene: c.into_scene(size),
        gonio: square,
        plate: plate_box,
    }
}

/// The correlation chip in `r`: its word and value while both fit, else the value alone.
/// A reading, so it stays at every **T** step.
fn chip(c: &mut Canvas, r: Rect, correlation: Correlation, theme: &Theme) {
    let chip = correlation.chip();
    let (bg, fg) = if chip.warning {
        (theme.banner_warning.background, theme.banner_warning.text)
    } else {
        (theme.plot_background, theme.text)
    };
    let r = Rect::new(
        r.x + 4.0,
        r.y + 2.0,
        (r.w - 8.0).max(1.0),
        (r.h - 4.0).max(1.0),
    );
    c.base.rects.push(FillRect {
        rect: r,
        color: bg,
        clip: None,
    });
    let fs = theme.font_size;
    let word = "correlation";
    let room = r.w - text_width(&chip.text, fs) - 16.0;
    if room >= text_width(word, theme.small_font_size) {
        c.overlay.labels.push(label(
            word,
            [r.x + 6.0, r.y + r.h / 2.0],
            anchor(HAlign::Left, VAlign::Center),
            theme.small_font_size,
            theme.text_dim,
        ));
        c.overlay.labels.push(label(
            chip.text,
            [r.right() - 6.0, r.y + r.h / 2.0],
            anchor(HAlign::Right, VAlign::Center),
            fs,
            fg,
        ));
    } else {
        c.overlay.labels.push(label(
            cut_to(&chip.text, r.w - 4.0, fs),
            [r.x + r.w / 2.0, r.y + r.h / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            fs,
            fg,
        ));
    }
}

/// The loudness plate, docked where [`super::dock`] put it at the scale its room allows,
/// or floating at the operator's place in `area` (the pane's rect, which a floating
/// plate's place is a fraction of) at the operator's scale.
fn plate(
    c: &mut Canvas,
    p: &ProgrammePane,
    l: &ProgrammeLayout,
    area: Rect,
    theme: &Theme,
) -> Option<PlateBox> {
    let r = p.loudness.as_ref()?;
    let font = theme.small_font_size;
    let (origin, scale, dropped, docked) = match (p.plate.at, l.plate) {
        (None, Some(d)) => ([d.rect.x, d.rect.y], d.scale, d.dropped, true),
        (None, None) => return None,
        (Some(at), _) => {
            // A floating plate too large for the pane shrinks, then drops slots by the
            // dock's order.
            let (w, h) = loudness_plate::size(font, p.plate.scale(), 0);
            let s = crate::plate::fitted(p.plate.scale(), area, w, h);
            let room = area.h - 2.0 * crate::legend::INSET;
            let dropped = (0..=loudness_plate::MAX_DROPPED)
                .find(|&d| loudness_plate::size(font, s, d).1 <= room)
                .unwrap_or(loudness_plate::MAX_DROPPED);
            let (w, h) = loudness_plate::size(font, s, dropped);
            let at = PlatePlace::rect_at(at, area, w, h);
            ([at.x, at.y], s, dropped, false)
        }
    };
    let (rect, cells) = loudness_plate::cells(origin, font, scale, dropped);
    let hover = p.plate_hover.filter(|h| h.kind == PlateKind::Loudness);
    // Its own layer, the last: a layer's labels draw over its fills, so a plate floated
    // over the bars in theirs would show their scale and readouts through its fill.
    loudness_plate::draw(&mut c.banners, r, rect, &cells, hover, area, theme);
    let mut b = PlateBox::new(PlateKind::Loudness, area, rect, scale);
    b.docked = docked;
    Some(b)
}
