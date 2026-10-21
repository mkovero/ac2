//! The cursor values' plate on the transfer view.

use super::trace_stroke;
use crate::canvas::{Canvas, text_width};
use crate::legend::{INSET, LegendBox};
use crate::plate::{self, Metrics, PlateBox, PlateHover, PlateKind, PlatePlace};
use crate::primitives::{Color, HAlign, Rect};
use crate::readout::{self, CursorReadout};
use crate::theme::Theme;
use crate::trace::DisplayTrace;

/// The cursor values on a plate of their own. Placed by the scene: each on its trace's
/// legend row, on the side of the plot away from the legend, the frequency above them
/// (below when the pane title or the plot's top leaves no room); when that plate would run
/// into the legend (a narrow pane, long names) the values go under the legend, or over it
/// when it sits low, in the same order. A value names its trace unless it sits on that
/// trace's legend row: with the legend hidden, moved apart from it or at another text size
/// its row says nothing. Dragged ([`PlatePlace::at`]): the frequency first, the values
/// under it. Returns the plate.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    c: &mut Canvas,
    cr: &CursorReadout,
    shown: &[DisplayTrace],
    legend: Option<&LegendBox>,
    place: &PlatePlace,
    hover: Option<PlateHover>,
    area: Rect,
    block: f32,
    clip: Rect,
    theme: &Theme,
) -> Option<PlateBox> {
    let hover = hover.filter(|h| h.kind == PlateKind::Readout);
    // Sized as the plate at its widest (every value named), then shrunk when that does not
    // fit the plot.
    let s = {
        let set = place.scale();
        let named = cr.rows.iter().filter_map(|r| {
            let t = shown.iter().find(|t| t.key == r.key)?;
            Some(format!(
                "{}  {}  {}  {}",
                t.name, r.magnitude, r.phase, r.coherence
            ))
        });
        let texts: Vec<String> = std::iter::once(cr.freq.clone()).chain(named).collect();
        let (w, h) = Metrics::new(theme, set).size(&texts);
        plate::fitted(set, area, w, h)
    };
    let m = Metrics::new(theme, s);
    let (font, row, pad_x, pad_y) = (m.font, m.row, m.pad_x, m.pad_y);
    // On the legend's rows only while both are where the scene puts them, at one size.
    let rows_of = legend.filter(|b| place.at.is_none() && b.plate.scale == s);
    let values = |r: &readout::CursorRow| format!("{}  {}  {}", r.magnitude, r.phase, r.coherence);
    let lines = cr.rows.iter().filter_map(|r| {
        let i = shown.iter().position(|t| t.key == r.key)?;
        let text = match rows_of {
            Some(_) => values(r),
            None => format!("{}  {}", shown[i].name, values(r)),
        };
        Some((text, trace_stroke(&shown[i], false, theme).color, i))
    });
    let first_y = area.y + INSET + pad_y + row * 1.5;
    let right_side = legend.is_none_or(|b| b.rect.x + b.rect.w / 2.0 <= area.x + area.w / 2.0);
    let mut rows: Vec<(String, Color, f32)> = match rows_of {
        Some(b) => lines
            .filter_map(|(t, col, i)| b.row_y(i).map(|y| (t, col, y)))
            .collect(),
        None => lines
            .enumerate()
            .map(|(k, (t, col, _))| (t, col, first_y + k as f32 * row))
            .take_while(|l| l.2 + row / 2.0 + pad_y <= area.bottom() - INSET)
            .collect(),
    };
    let w = rows
        .iter()
        .map(|l| text_width(&l.0, font))
        .chain([text_width(&cr.freq, font)])
        .fold(0.0f32, f32::max)
        + 2.0 * pad_x;
    if let Some(at) = place.at {
        let mut lines = vec![(cr.freq.clone(), theme.text)];
        lines.extend(rows.into_iter().map(|(t, col, _)| (t, col)));
        let h = lines.len() as f32 * row + 2.0 * pad_y;
        let rect = PlatePlace::rect_at(at, area, w, h);
        plate::draw_lines(
            &mut c.overlay,
            rect,
            &lines,
            HAlign::Left,
            m,
            true,
            hover,
            clip,
            theme,
        );
        return Some(PlateBox::new(PlateKind::Readout, area, rect, s));
    }
    let x = if right_side {
        area.right() - INSET - w
    } else {
        area.x + INSET
    };
    if let Some(b) = legend
        && !rows.is_empty()
        && x < b.rect.right()
        && x + w > b.rect.x
    {
        let need = (rows.len() + 1) as f32 * row + 2.0 * pad_y;
        let top = if b.rect.bottom() + 2.0 + need <= area.bottom() {
            b.rect.bottom() + 2.0
        } else {
            b.rect.y - 2.0 - need
        };
        for (k, l) in rows.iter_mut().enumerate() {
            l.2 = top + pad_y + row * (k as f32 + 1.5);
        }
    }
    let lo = rows.iter().map(|l| l.2).fold(f32::INFINITY, f32::min);
    let hi = rows.iter().map(|l| l.2).fold(f32::NEG_INFINITY, f32::max);
    // The pane title is at the left: on the right the frequency may share its line.
    let ceiling = if right_side { block } else { area.y };
    let freq_y = if rows.is_empty() {
        area.y + INSET + pad_y + row / 2.0
    } else if lo - row * 1.5 - pad_y >= ceiling {
        lo - row
    } else {
        hi + row
    };
    let (top, bottom) = (lo.min(freq_y), hi.max(freq_y));
    let rect = Rect::new(
        x,
        top - row / 2.0 - pad_y,
        w,
        bottom - top + row + 2.0 * pad_y,
    );
    // The values keep their rows' heights (a legend row's), so they are drawn one by one
    // rather than as an evenly spaced list.
    crate::legend::plate(&mut c.overlay, rect, hover.is_some(), clip, theme);
    let h = if right_side {
        HAlign::Right
    } else {
        HAlign::Left
    };
    let tx = if right_side {
        rect.right() - pad_x
    } else {
        rect.x + pad_x
    };
    for (text, color, y) in std::iter::once((cr.freq.clone(), theme.text, freq_y)).chain(rows) {
        let mut l = crate::canvas::label(
            text,
            [tx, y],
            crate::canvas::anchor(h, crate::primitives::VAlign::Center),
            font,
            color,
        );
        l.clip = Some(clip);
        c.overlay.labels.push(l);
    }
    plate::grip(
        &mut c.overlay,
        rect,
        hover.is_some_and(|h| h.grip),
        clip,
        theme,
    );
    Some(PlateBox::new(PlateKind::Readout, area, rect, s))
}
