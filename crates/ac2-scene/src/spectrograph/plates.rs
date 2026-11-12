//! The plates and marks a spectrograph shown alone draws over its history: the tracked
//! peaks on its newest edge, the cursor's reading and the legend.

use super::*;

/// The tracked peaks (P) of curve `key` on a spectrograph shown alone: no level axis to
/// put them on, so each is a box at its frequency on the newest row (the plot's top edge)
/// with its rank under it, listed on the peak list's plate as the spectrum lists them;
/// traces only (T) hides marks and list. Its peaks, the list and its plate.
#[allow(clippy::too_many_arguments)]
pub(super) fn tracked_marks(
    c: &mut Canvas,
    traces: &[SpectrumTrace<'_>],
    key: crate::trace::TraceKey,
    view: &ViewState,
    plot: Rect,
    xm: &crate::axis::Mapping,
    top: f32,
    readout: Option<crate::plate::PlateBox>,
    theme: &Theme,
) -> (
    Vec<crate::spectrum::SpectrumPeaks>,
    Option<String>,
    Option<crate::plate::PlateBox>,
) {
    let Some(t) = traces.iter().find(|t| t.key == key && t.tracked.is_some()) else {
        return (Vec::new(), None, None);
    };
    let peaks = crate::spectrum::tracked_peaks(std::slice::from_ref(t), xm, None, plot, false);
    let Some(p) = peaks.first().filter(|p| !p.rows.is_empty()) else {
        return (peaks, None, None);
    };
    if !view.chrome.peak_marks() {
        return (peaks, None, None);
    }
    let half = crate::spectrum::MARK_HALF;
    let stroke = crate::primitives::Stroke::solid(theme.text, 1.5);
    for (k, r) in p.rows.iter().enumerate() {
        let x = xm.to_px(r.freq_hz);
        if !(x.is_finite() && x >= plot.x && x <= plot.right()) {
            continue;
        }
        let (y0, y1) = (plot.y + 1.0, plot.y + 1.0 + 2.0 * half);
        c.overlay.polylines.push(Polyline::clipped(
            vec![
                [x - half, y0],
                [x + half, y0],
                [x + half, y1],
                [x - half, y1],
                [x - half, y0],
            ],
            stroke,
            plot,
        ));
        // The rank ties a mark to its row in the list; under the box, as the plot's top
        // is the newest row's edge.
        let mut l = label(
            (k + 1).to_string(),
            [x, y1 + 2.0],
            anchor(HAlign::Center, VAlign::Top),
            theme.small_font_size,
            theme.text,
        );
        l.clip = Some(plot);
        c.overlay.labels.push(l);
    }
    let list = crate::spectrum::list_text(&[(t, p)]);
    let b = crate::spectrum::peak_plate(c, &list, plot, top, readout, view, theme);
    (peaks, Some(list), Some(b))
}

/// The cursor on the history of `s`: put (`cursor_hz`, the view's `cursor_at_s`) within
/// the `span` shown, it reads the history there.
pub(super) fn history_cursor(
    s: &SpectrographInput<'_>,
    cursor_hz: Option<f64>,
    view: &ViewState,
    span: u32,
) -> Option<SpectrographCursor> {
    let hz = cursor_hz?;
    let before = (s.history.newest_end_s()? - view.spectrum.spectrograph.cursor_at_s?).max(0.0);
    if before > f64::from(span) {
        return None;
    }
    let v = s
        .history
        .value_at(hz, before)
        .map_or(f64::NAN, |v| f64::from(v) + s.offset_db);
    let level = cursor_level(v, s.history.unit_noted().as_deref());
    let ago = format!("{} ago", format::age(before));
    Some(SpectrographCursor {
        freq_hz: hz,
        before_s: before,
        text: format!("{} · {ago} · {level}", format::freq_readout(hz)),
        line: format!("{ago}  {level}"),
    })
}

/// A history cursor's level as the 2D and 3D views read it: `−23.5 dBFS (tone)` for
/// displayed level `v` in `unit`, a dash for none.
pub(crate) fn cursor_level(v: f64, unit: Option<&str>) -> String {
    match unit {
        Some(unit) if v.is_finite() => format!("{} {unit}", format::level(v)),
        _ => format::level(f64::NAN),
    }
}

/// Whether a spectrograph's legend plate names curve `key`: not hidden, and the curve is
/// among `traces`.
pub(crate) fn legend_shows(
    traces: &[SpectrumTrace<'_>],
    key: crate::trace::TraceKey,
    view: &ViewState,
) -> bool {
    !view.legend.hidden && traces.iter().any(|t| t.key == key)
}

/// The legend plate of a spectrograph shown alone, in `plot`, over the one curve it draws
/// (`key`'s, found in `traces`): its name and tags as the spectrum's legend has them, moved,
/// scaled and hidden as every legend is. `None` without that curve.
pub(crate) fn legend_over(
    layer: &mut crate::primitives::Layer,
    traces: &[SpectrumTrace<'_>],
    key: Option<crate::trace::TraceKey>,
    view: &ViewState,
    plot: Rect,
    theme: &Theme,
) -> Option<crate::spectrum::SpectrumLegend> {
    let drawn: Vec<SpectrumTrace<'_>> = traces
        .iter()
        .filter(|t| Some(t.key) == key)
        .cloned()
        .collect();
    if drawn.is_empty() {
        return None;
    }
    Some(crate::spectrum::legend_of(
        layer, &drawn, view, plot, plot, theme,
    ))
}
