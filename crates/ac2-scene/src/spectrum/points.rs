//! Peak points: the peaks of the curve picked in the legend, each box labelled with its
//! frequency and level where the list does not name them (`docs/design/legend-peak-points.md`).

use super::*;

/// A peak point's label as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct PeakLabel {
    /// `1.00 kHz  −32.0 dBFS`, with the quantity note (`(band)`) on a pane that mixes
    /// quantities.
    pub text: String,
    /// The label's box, its plate.
    pub rect: Rect,
}

/// Gap between a box and its label.
const GAP: f32 = 2.0;
/// Least room between two labels side by side.
const APART: f32 = 4.0;

/// What a peak point says: frequency and displayed level in the cursor readout's forms,
/// with the axis unit; the quantity note only when the pane's curves mix quantities, as the
/// cursor readout then needs it to be read right.
pub(super) fn point_text(row: &PeakRow, t: &SpectrumTrace<'_>, mixed: bool) -> String {
    let note = if mixed {
        format!(" ({})", quantity_note(t.quantity))
    } else {
        String::new()
    };
    format!(
        "{}  {} {}{note}",
        format::freq_readout(row.freq_hz),
        format::level(row.level),
        value_unit(t.scale, t.quantity)
    )
}

/// Labels the boxes `marks` (loudest peak first, each its box and text) inside `plot`:
/// above its box, below when the plot's top would cut it. A label overlapping one placed
/// before it is dropped and its box stays: half a label reads as another number, and the
/// louder peak is the one kept.
pub(super) fn label_marks(
    c: &mut Canvas,
    marks: &[(Rect, String)],
    plot: Rect,
    theme: &Theme,
) -> Vec<PeakLabel> {
    let font = theme.small_font_size;
    let h = font * 1.25;
    let mut kept: Vec<PeakLabel> = Vec::new();
    for (m, text) in marks {
        let w = text_width(text, font);
        let x = (m.x + m.w / 2.0 - w / 2.0)
            .min(plot.right() - w)
            .max(plot.x);
        let above = m.y - GAP - h;
        let y = if above < plot.y {
            m.bottom() + GAP
        } else {
            above
        };
        let rect = Rect::new(x, y, w, h);
        let clear = kept.iter().all(|k| {
            rect.right() + APART <= k.rect.x
                || k.rect.right() + APART <= rect.x
                || rect.bottom() <= k.rect.y
                || k.rect.bottom() <= rect.y
        });
        if clear && rect.bottom() <= plot.bottom() {
            kept.push(PeakLabel {
                text: text.clone(),
                rect,
            });
        }
    }
    for l in &kept {
        c.overlay.polygons.push(crate::primitives::FillPolygon {
            points: crate::spectrograph_3d::plate(l.rect),
            color: theme
                .plot_background
                .with_alpha(crate::spectrograph_3d::PLATE_ALPHA),
            clip: Some(plot),
        });
        let mut lb = label(
            l.text.clone(),
            [l.rect.x + l.rect.w / 2.0, l.rect.y + l.rect.h / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            font,
            theme.text,
        );
        lb.clip = Some(plot);
        c.overlay.labels.push(lb);
    }
    kept
}
