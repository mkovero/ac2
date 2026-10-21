//! Drawing the tiles and the history strip into the scene.

use super::*;
use crate::view::Style;

/// Columns of a grid of `n` tiles in `w × h` that makes the tiles largest at about 2:1.
pub(super) fn columns(n: usize, w: f32, h: f32) -> usize {
    (1..=n.max(1))
        .max_by(|&a, &b| {
            let score = |c: usize| {
                let rows = n.div_ceil(c) as f32;
                let (tw, th) = (w / c as f32, h / rows);
                (tw / 2.0).min(th)
            };
            score(a).total_cmp(&score(b))
        })
        .unwrap_or(1)
}

/// A window's name as shown: `30 min` when every window shares the weighting the caption
/// names once (`LAeq`), else the full `LAeq 30 min`.
fn shown_name<'a>(name: &'a str, shared: Option<&str>) -> &'a str {
    shared
        .and_then(|w| name.strip_prefix(w))
        .and_then(|rest| rest.strip_prefix(' '))
        .unwrap_or(name)
}

/// The small lines at the bottom of a tile: on course first, then the limit (not past
/// [`PlotChrome::Full`]), the filling and the gaps (a peak limit: what its value is).
pub(super) fn tile_below(t: &LeqTile, chrome: PlotChrome) -> Vec<String> {
    let mut below: Vec<String> = t.course.iter().cloned().collect();
    below.extend(t.limit.iter().filter(|_| chrome.limit_text()).cloned());
    below.extend(t.held.iter().cloned());
    match (&t.filling, &t.incomplete) {
        (Some(f), Some(i)) => below.push(format!("{f} · {i}")),
        (Some(f), None) => below.push(f.clone()),
        (None, Some(i)) => below.push(i.clone()),
        (None, None) => {}
    }
    below
}

/// One tile: the name and state on top; in the middle, large, what to do (`stay ≤ 101.5 dB SPL`,
/// or how long it cools down); under it, at a fixed place low in the body and smaller than
/// the name, the window's value — on stage the instruction is acted on, the value only
/// explains it, and a meter's number shown above the tiles must stay the big one; below,
/// the course, limit and progress, in room for `lines_below` of them.
pub(super) fn draw_tile(
    c: &mut Canvas,
    t: &LeqTile,
    r: Rect,
    stale: bool,
    shared: Option<&str>,
    lines_below: usize,
    style: Style,
) {
    let Style { chrome, theme } = style;
    let (bg, fg) = tile_colors(t.state, theme);
    c.base.rects.push(FillRect {
        rect: r,
        color: bg,
        clip: None,
    });
    let fg = if stale && t.state != TileState::Over && t.state != TileState::Near {
        theme.text_dim
    } else {
        fg
    };
    let pad = (r.h * 0.06).clamp(4.0, 14.0);
    let head = (r.h * 0.11).clamp(11.0, 30.0);
    let small = (r.h * 0.075).clamp(9.0, 20.0);
    let width = (r.w - 2.0 * pad).max(1.0);
    let clip = Some(r);
    let mut push = |text: String, pos: [f32; 2], h: HAlign, v: VAlign, size: f32| {
        let mut l = label(text, pos, anchor(h, v), size, fg);
        l.clip = clip;
        c.overlay.labels.push(l);
    };
    push(
        shown_name(&t.name, shared).to_string(),
        [r.x + pad, r.y + pad],
        HAlign::Left,
        VAlign::Top,
        head,
    );
    if let Some(s) = t.state_word(chrome) {
        push(
            s.to_owned(),
            [r.right() - pad, r.y + pad],
            HAlign::Right,
            VAlign::Top,
            head,
        );
    }
    let below = tile_below(t, chrome);
    let lines = below.len() as f32;
    for (k, text) in below.into_iter().enumerate().rev() {
        let row = lines - 1.0 - k as f32;
        push(
            text,
            [r.x + r.w / 2.0, r.bottom() - pad - row * small * 1.3],
            HAlign::Center,
            VAlign::Bottom,
            small,
        );
    }
    let body_top = r.y + pad + head * 1.3;
    let body_bottom = r.bottom() - pad - lines_below as f32 * small * 1.3;
    let [_, instruction, _] = columns::detail_lines(t, chrome);
    // Every tile shares the shares of its body, so tiles of one grid show their values at
    // one size and place whether or not they have an instruction above them. The
    // instruction stays within [`TILE_INSTRUCTION_EM`] of the header's size and the value
    // under the name's, so a meter's number above the tiles keeps the eye.
    let avail = (body_bottom - body_top).max(1.0);
    let instr = (avail * 0.5 / 1.25)
        .min(head * TILE_INSTRUCTION_EM)
        .max(1.0);
    let value_h = (avail * 0.35 / 1.25)
        .min(instr * columns::VALUE_RATIO)
        .min(head * columns::NAME_RATIO);
    let unit_ratio = 0.55;
    let value_w =
        canvas::text_width(&t.value, 1.0) + 0.1 + canvas::text_width(&t.weighted_unit, unit_ratio);
    let value_size = value_h.min(width / value_w.max(1e-3)).max(8.0);
    let unit_size = value_size * unit_ratio;
    let cx = r.x + r.w / 2.0;
    let value_top = (body_bottom - value_size * 1.25).max(body_top);
    if let Some((text, size)) = columns::fitting_shrunk(&instruction, width, instr, small) {
        let y = body_top + ((value_top - body_top) - size * 1.25).max(0.0) / 2.0;
        push(text, [cx, y], HAlign::Center, VAlign::Top, size);
    }
    let base = value_top + 0.95 * value_size;
    let vw = canvas::text_width(&t.value, value_size);
    let uw = canvas::text_width(&t.weighted_unit, unit_size);
    let gap = value_size * 0.1;
    let x0 = cx - (vw + gap + uw) / 2.0 + vw;
    push(
        t.value.clone(),
        [x0, base],
        HAlign::Right,
        VAlign::Baseline,
        value_size,
    );
    push(
        t.weighted_unit.clone(),
        [x0 + gap, base],
        HAlign::Left,
        VAlign::Baseline,
        unit_size,
    );
}

pub(super) fn draw_history(
    c: &mut Canvas,
    s: &HistoryStrip,
    shared: Option<&str>,
    chrome: crate::view::PlotChrome,
    theme: &Theme,
) {
    canvas::pane_frame(c, s.plot, &s.x, &s.y, true, "dB", Style { chrome, theme });
    for l in &s.lines {
        if let Some(y) = l.limit_y {
            canvas::hline(
                c,
                s.plot,
                y,
                Stroke {
                    color: l.color,
                    width: 1.2,
                    dash: Some(Dash {
                        on: 6.0,
                        off: 4.0,
                        offset: 0.0,
                    }),
                },
            );
        }
        if l.points.len() > 1 {
            c.data.polylines.push(Polyline {
                points: l.points.clone(),
                alpha: vec![],
                stroke: Stroke::solid(l.color, theme.trace_width),
                clip: Some(s.plot),
            });
        }
        for run in &l.over {
            c.data.polylines.push(Polyline {
                points: run.clone(),
                alpha: vec![],
                stroke: Stroke::solid(theme.banner_fault.background, theme.trace_width * 2.5),
                clip: Some(s.plot),
            });
        }
    }
    // Legend: each window's name in its colour, along the top of the strip.
    let mut x = s.plot.x + 30.0;
    for l in &s.lines {
        let name = shown_name(&l.name, shared);
        c.overlay.labels.push(label(
            name.to_string(),
            [x, s.plot.y + 4.0],
            anchor(HAlign::Left, VAlign::Top),
            theme.small_font_size,
            l.color,
        ));
        x += canvas::text_width(name, theme.small_font_size) + 12.0;
    }
}

/// The caption of a strip looking back, along its top right in the dim text.
pub(super) fn history_note(c: &mut Canvas, s: &HistoryStrip, text: &str, theme: &Theme) {
    c.overlay.labels.push(label(
        text.to_string(),
        [s.plot.right() - 6.0, s.plot.y + 4.0],
        anchor(HAlign::Right, VAlign::Top),
        theme.small_font_size,
        theme.text_dim,
    ));
}

/// The cursor at daemon wall time `t` over the strip: a line across it and the readout
/// beside it (on the side with room), a line per window in its colour, red where over.
pub(super) fn history_cursor(
    c: &mut Canvas,
    s: &HistoryStrip,
    t: f64,
    r: &StripReadout,
    theme: &Theme,
) {
    let span = (s.to - s.from).max(1e-9);
    let x = s.plot.x + ((t - s.from) / span) as f32 * s.plot.w;
    canvas::vline(
        &mut c.overlay,
        s.plot,
        x,
        Stroke::solid(theme.text_dim, 1.0),
    );
    let fs = theme.small_font_size;
    let line_h = fs * 1.35;
    let lines: Vec<(&str, Color)> = std::iter::once((r.time.as_str(), theme.text))
        .chain(r.rows.iter().map(|row| {
            let color = if row.over {
                theme.banner_fault.background
            } else {
                theme.trace_color(row.window)
            };
            (row.text.as_str(), color)
        }))
        .collect();
    let w = lines
        .iter()
        .map(|(t, _)| canvas::text_width(t, fs))
        .fold(0.0, f32::max)
        + 12.0;
    let h = line_h * lines.len() as f32 + 8.0;
    let left = if x + 8.0 + w <= s.plot.right() {
        x + 8.0
    } else {
        (x - 8.0 - w).max(s.plot.x)
    };
    let top = s.plot.y + 20.0;
    c.overlay.rects.push(FillRect {
        rect: Rect::new(left, top, w, h),
        color: theme.plot_background.with_alpha(0.88),
        clip: None,
    });
    for (i, (text, color)) in lines.into_iter().enumerate() {
        c.overlay.labels.push(label(
            text.to_string(),
            [left + 6.0, top + 4.0 + i as f32 * line_h],
            anchor(HAlign::Left, VAlign::Top),
            fs,
            color,
        ));
    }
}
