//! Drawing the spectrum scene: the axes, each curve as bars or a line, peak hold, the
//! valid-resolution marker, the legend and the cursor.

use super::*;

pub fn spectrum_scene(
    traces: &[SpectrumTrace<'_>],
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> SpectrumScene {
    let right = crate::freq_column::right(MARGINS.right, view);
    spectrum_scene_in(traces, status, view, theme, size, right)
}

/// [`spectrum_scene`] with `right` logical pixels right of the plot: room for what a
/// composed view draws beside a plot under it, so the two frequency axes line up.
pub(crate) fn spectrum_scene_in(
    traces: &[SpectrumTrace<'_>],
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
    right: f32,
) -> SpectrumScene {
    let mut c = Canvas::new(size, theme);
    let plot_w = (size.width - MARGINS.left - right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, MARGINS.left, plot_w, size, theme);
    let font = theme.small_font_size;
    // The legend takes rows of its own between the banner strip and the plot, so it never
    // covers a curve; a short pane gives it fewer rows.
    let legend: Vec<LegendEntry> = traces.iter().map(legend_entry).collect();
    let free = size.height - strip.rect.bottom() - MARGINS.top - MARGINS.bottom;
    let max_rows =
        ((free * LEGEND_SHARE / LEGEND_ROW).floor().max(0.0) as usize).min(LEGEND_MAX_ROWS);
    let placed = legend_layout(&legend, plot_w, max_rows, font);
    let legend_rows = placed.iter().map(|p| p.1 + 1).max().unwrap_or(0);
    let legend_rect = Rect::new(
        MARGINS.left,
        strip.rect.bottom() + if legend_rows > 0 { 4.0 } else { 0.0 },
        plot_w,
        legend_rows as f32 * LEGEND_ROW,
    );
    let plot = canvas::plot_area(
        size,
        legend_rect.bottom() - if legend_rows > 0 { 4.0 } else { 0.0 },
        right,
    );
    let mut legend_shown = Vec::with_capacity(placed.len());
    let mut legend_hits = Vec::with_capacity(placed.len());
    for (i, row, x, text) in placed {
        let y = legend_rect.y + (row as f32 + 0.5) * LEGEND_ROW;
        let x = legend_rect.x + x;
        if let Some(e) = i.and_then(|i| legend.get(i)) {
            legend_hits.push(crate::legend::LegendHit {
                key: e.key,
                rect: Rect::new(
                    x,
                    y - LEGEND_ROW / 2.0,
                    entry_width(&text, font),
                    LEGEND_ROW,
                ),
            });
        }
        let (tx, color) = match i.and_then(|i| traces.get(i).zip(legend.get(i))) {
            Some((t, e)) => {
                let a = if e.stale { theme.stale_alpha } else { 1.0 };
                let color = t.color.with_alpha(a);
                crate::tf::legend_swatch(
                    &mut c.overlay,
                    x,
                    y,
                    SWATCH_W,
                    color,
                    e.selected,
                    None,
                    theme,
                );
                (
                    x + SWATCH_W + SWATCH_GAP,
                    if e.stale { theme.text_dim } else { theme.text },
                )
            }
            None => (x, theme.text_dim),
        };
        c.overlay.labels.push(label(
            text.clone(),
            [tx, y],
            anchor(HAlign::Left, VAlign::Center),
            font,
            color,
        ));
        legend_shown.push(text);
    }
    let full_caption = traces.first().map_or(String::new(), |t| t.caption.clone());
    // The unit (left) and the caption (right) share the plot's top line. The caption takes
    // its longest form that leaves the unit its compact form room (`dB SPL/bin`: a bin level
    // is no band level, which the bare scale would no longer say), then the unit its longest
    // form beside it: the two never overlap, and what the axis means goes last.
    let (forms, unit_help) = unit_forms(traces);
    let compact = forms
        .len()
        .checked_sub(2)
        .and_then(|i| forms.get(i))
        .or(forms.last())
        .map_or(0.0, |f| text_width(f, font));
    let shortest = forms.last().map_or(0.0, |f| text_width(f, font));
    let room_beside = |caption: &str| {
        plot.w
            - 12.0
            - if caption.is_empty() {
                0.0
            } else {
                text_width(caption, font) + 12.0
            }
    };
    let caption_forms = caption_forms(&full_caption);
    let caption = caption_forms
        .iter()
        .find(|c| room_beside(c) >= compact)
        .or_else(|| caption_forms.iter().find(|c| room_beside(c) >= shortest))
        .cloned()
        .unwrap_or_default();
    let room = room_beside(&caption);
    let unit = forms
        .iter()
        .find(|f| text_width(f, font) <= room)
        .or(forms.last())
        .cloned()
        .unwrap_or_default();
    let x_axis = axis::freq_axis(view.freq.range(), plot.x, plot.right());
    let y_axis = axis::linear_axis(view.spectrum.level, plot.bottom(), plot.y, &unit);
    canvas::pane_frame(
        &mut c,
        plot,
        &x_axis,
        &y_axis,
        true,
        &unit,
        view.chrome,
        theme,
    );
    c.base.labels.push(label(
        caption.clone(),
        [plot.right() - 6.0, plot.y + 4.0],
        anchor(HAlign::Right, VAlign::Top),
        theme.small_font_size,
        theme.text_dim,
    ));
    let (xm, ym) = (x_axis.mapping, y_axis.mapping);
    let marked = marked_bands(traces, view);
    let stitches = marked.map_or_else(Vec::new, |(b, _)| crate::stitch::stitches(b));
    let unresolved = marked.map_or_else(Vec::new, |(b, first)| {
        crate::unresolved::marks(
            crate::unresolved::Source {
                unresolved: &b.unresolved,
                fill: crate::unresolved::Fill::Steps,
            },
            first,
            None,
        )
    });
    crate::unresolved::draw(&mut c, plot, &xm, &unresolved, theme);
    crate::stitch::draw(&mut c, plot, &xm, &stitches, true, theme);

    // Later shapes draw over earlier ones, and `traces` lists the pane's own group first,
    // then the compared ones in compare order: drawn last to first, the pane's own curves
    // come out on top and the first compared above the second.
    for t in traces.iter().rev() {
        let dim = if t.is_stale() { theme.stale_alpha } else { 1.0 };
        let color = t.color.with_alpha(dim);
        let back = super::layer_alpha(t.layer);
        let n = t.freqs.len().min(t.level.len());
        let cols = visible_columns(&t.freqs[..n], xm.range.lo, xm.range.hi);
        // Bars are band power (RTA). A narrowband bin is a tone level, and thousands of bins
        // share a pixel at the top of a log axis: drawn as bars, a single-bin tone becomes a
        // sub-pixel sliver. A tone trace asked for bars gets the max-per-pixel line, which
        // keeps a tone a spike.
        let style = match t.style {
            SpectrumStyle::Bars if t.quantity.is_narrow() => SpectrumStyle::Line,
            s => s,
        };
        match style {
            SpectrumStyle::Bars => {
                for i in cols.clone() {
                    let v = t.drawn_value(i);
                    let Some(&(e0, e1)) = t.edges.get(i) else {
                        continue;
                    };
                    let y = ym.to_px(v).max(plot.y);
                    if !y.is_finite() || y >= plot.bottom() {
                        continue;
                    }
                    let (x0, x1) = (xm.to_px(e0.max(xm.range.lo * 1e-3)), xm.to_px(e1));
                    // A one-pixel gap between bars when they are wide enough to show it.
                    let gap = if x1 - x0 > 4.0 { 1.0 } else { 0.0 };
                    c.data.rects.push(FillRect {
                        rect: Rect::new(
                            x0 + gap / 2.0,
                            y,
                            (x1 - x0 - gap).max(0.5),
                            plot.bottom() - y,
                        ),
                        color: color.with_alpha(theme.bar_alpha * back),
                        clip: Some(plot),
                    });
                }
            }
            SpectrumStyle::Line | SpectrumStyle::Fill => {
                let xs: Vec<f32> = t.freqs[cols.clone()].iter().map(|f| xm.to_px(*f)).collect();
                let ys: Vec<f32> = cols.clone().map(|i| ym.to_px(t.drawn_value(i))).collect();
                let (xs, ys) = max_per_pixel(&xs, &ys);
                let (points, _) = gapped(&xs, &ys, None, |_, _| false);
                if style == SpectrumStyle::Fill && !points.is_empty() {
                    // The area under the same thinned points as the line, so a tone stays a
                    // spike and a gap in the line (a NaN break) is a gap in the fill.
                    c.data.bands.push(Band {
                        points: points
                            .iter()
                            .map(|&[x, y]| BandPoint {
                                x,
                                y0: y,
                                y1: plot.bottom(),
                            })
                            .collect(),
                        color: color.with_alpha(theme.fill_alpha * back),
                        clip: Some(plot),
                    });
                }
                if !points.is_empty() {
                    c.data.polylines.push(Polyline {
                        points,
                        alpha: vec![],
                        stroke: Stroke::solid(
                            color,
                            theme.trace_width
                                * if t.selected {
                                    crate::tf::SELECTED_WIDTH
                                } else {
                                    1.0
                                },
                        ),
                        clip: Some(plot),
                    });
                }
            }
        }
        if t.peak.is_some() {
            let pc = color.with_alpha(theme.peak_alpha);
            let points = match style {
                // A cap across each band.
                SpectrumStyle::Bars => {
                    let mut pts = Vec::new();
                    for i in cols.clone() {
                        let (Some(p), Some(&(e0, e1))) = (t.peak_value(i), t.edges.get(i)) else {
                            continue;
                        };
                        let y = ym.to_px(p);
                        if !y.is_finite() {
                            continue;
                        }
                        if !pts.is_empty() {
                            pts.push([f32::NAN, f32::NAN]);
                        }
                        pts.push([xm.to_px(e0.max(xm.range.lo * 1e-3)), y]);
                        pts.push([xm.to_px(e1), y]);
                    }
                    pts
                }
                SpectrumStyle::Line | SpectrumStyle::Fill => {
                    let xs: Vec<f32> = t.freqs[cols.clone()].iter().map(|f| xm.to_px(*f)).collect();
                    let ys: Vec<f32> = cols
                        .clone()
                        .map(|i| t.peak_value(i).map_or(f32::NAN, |p| ym.to_px(p)))
                        .collect();
                    let (xs, ys) = max_per_pixel(&xs, &ys);
                    gapped(&xs, &ys, None, |_, _| false).0
                }
            };
            if !points.is_empty() {
                c.data.polylines.push(Polyline {
                    points,
                    alpha: vec![],
                    stroke: Stroke {
                        color: pc,
                        width: (theme.trace_width * 0.75).max(1.0),
                        dash: (style != SpectrumStyle::Bars).then_some(Dash {
                            on: 4.0,
                            off: 3.0,
                            offset: 0.0,
                        }),
                    },
                    clip: Some(plot),
                });
            }
        }
    }

    let cursor_hz = view.cursor_hz.filter(|_| view.chrome.cursor());
    let cursor = cursor_hz.and_then(|hz| {
        // The line and the frequency stay where the cursor was put, as on the spectrograph
        // beside it; each curve reads the column whose band holds that frequency.
        traces.first()?;
        let f = hz;
        let rows = traces
            .iter()
            .filter_map(|t| {
                let j = column_at(t.freqs, t.edges, hz)?;
                let v = t.value(j);
                let s = format::level(v);
                let s = if v.is_finite() {
                    format!(
                        "{s} {} ({})",
                        value_unit(t.scale, t.quantity),
                        quantity_note(t.quantity)
                    )
                } else {
                    s
                };
                // An offset trace reads its displayed (offset) level, and says so.
                let name = if t.offset_db != 0.0 {
                    offset_note(&t.name, t.offset_db)
                } else {
                    t.name.clone()
                };
                Some((name, s))
            })
            .collect();
        let mut freq = format::freq_readout(f);
        if let Some((b, _)) = marked {
            if let Some(w) = crate::stitch::windows_at(b, f) {
                freq = format!("{freq} · {w}");
            }
            if let Some(m) = crate::unresolved::at(&unresolved, f) {
                freq = format!("{freq} · {}", m.note);
            }
        }
        Some(SpectrumCursor {
            freq_hz: f,
            freq,
            rows,
        })
    });
    if let Some(cur) = &cursor {
        canvas::vline(&mut c.overlay, plot, xm.to_px(cur.freq_hz), theme.cursor);
        let mut lines = vec![cur.freq.clone()];
        lines.extend(cur.rows.iter().map(|(n, v)| format!("{n}  {v}")));
        c.overlay.labels.push(label(
            lines.join("\n"),
            [plot.right() - 8.0, plot.y + 22.0],
            anchor(HAlign::Right, VAlign::Top),
            theme.small_font_size,
            theme.text,
        ));
    }

    SpectrumScene {
        scene: c.into_scene(size),
        plot,
        x_axis,
        y_axis,
        unit_rect: Rect::new(
            plot.x + 6.0,
            plot.y + 4.0,
            text_width(&unit, font),
            font * 1.25,
        ),
        unit,
        unit_help,
        caption,
        cursor,
        legend,
        legend_shown,
        legend_rect,
        legend_hits,
        strip: strip.rect,
        banners: strip.rows,
        stitches,
        unresolved,
    }
}
