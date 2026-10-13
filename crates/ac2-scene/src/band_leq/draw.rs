//! Laying the band view out: the headline, the predicted line and a row of bars per
//! window.

use super::*;

// ---------------------------------------------------------------------------------------
// Scene

/// What the band view shows.
#[derive(Clone, Debug, PartialEq)]
pub struct BandLeqView {
    /// `FOH SPL`.
    pub meter: String,
    /// Calibration text, with the mic.
    pub cal: String,
    pub text: BandLeqText,
    /// `STALE 3.2 s`.
    pub stale: Option<String>,
}

/// The band view as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct BandLeqScene {
    pub scene: Scene,
    /// Per row, each bar's column, low band to high.
    pub columns: Vec<Vec<Rect>>,
    /// The headline's box.
    pub headline: Rect,
    /// Per row, the bar scale, dB (bottom, top).
    pub ranges: Vec<(f64, f64)>,
    pub banners: Vec<BannerRow>,
}

/// Below the lowest limit…
const BELOW_LIMIT_DB: f64 = 20.0;
/// …and above the highest: room to see how far over a band is.
const ABOVE_LIMIT_DB: f64 = 6.0;

/// The bar scale: 10 dB steps around the limits and the values.
pub fn bar_range(bars: &[BandBar]) -> (f64, f64) {
    let limits: Vec<f64> = bars.iter().filter_map(|b| b.limit_db).collect();
    let values: Vec<f64> = bars
        .iter()
        .map(|b| b.leq_db)
        .filter(|v| v.is_finite())
        .collect();
    let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (lo, hi) = if limits.is_empty() {
        if values.is_empty() {
            return (0.0, 100.0);
        }
        (min(&values) - 10.0, max(&values) + ABOVE_LIMIT_DB)
    } else {
        (
            min(&limits) - BELOW_LIMIT_DB,
            max(&limits).max(max(&values)) + ABOVE_LIMIT_DB,
        )
    };
    let lo = (lo / 10.0).floor() * 10.0;
    let hi = ((hi / 10.0).ceil() * 10.0).max(lo + 10.0);
    (lo, hi)
}

/// Lays the band view out in `size`: the meter and calibration on one line, the headline
/// large, the limits' place and the prediction under it, then one row of columns per
/// length and weighting, stacked in the order of their first windows, each under its
/// caption and key.
pub fn band_leq_scene(
    v: &BandLeqView,
    status: &Status,
    theme: &Theme,
    size: Viewport,
) -> BandLeqScene {
    let mut c = Canvas::new(size, theme);
    let pad = 10.0;
    let strip = canvas::banner_strip(&mut c, status, pad, size.width - 2.0 * pad, size, theme);
    let t = &v.text;
    let fs = theme.font_size;
    let w = (size.width - 2.0 * pad).max(1.0);
    let mut y = strip.rect.bottom() + pad;

    // Caption: the meter and the bands, the calibration (and STALE) right.
    let left = format!("{} · band Leq {}, {}", v.meter, t.bands, t.unit);
    let right = match &v.stale {
        Some(s) => format!("{s} · {}", v.cal),
        None => v.cal.clone(),
    };
    let right = crate::spl::cut(&right, w * 0.5, fs);
    let left = crate::spl::cut(&left, w - canvas::text_width(&right, fs) - fs, fs);
    c.overlay.labels.push(label(
        left,
        [pad, y],
        anchor(HAlign::Left, VAlign::Top),
        fs,
        theme.text,
    ));
    c.overlay.labels.push(label(
        right,
        [size.width - pad, y],
        anchor(HAlign::Right, VAlign::Top),
        fs,
        if v.stale.is_some() {
            theme.banner_warning.background
        } else {
            theme.text_dim
        },
    ));
    y += fs * 1.6;

    // The headline: what to do, large; on the judgement's colour.
    let big = (size.height * 0.06).clamp(fs, theme.big_font_size.max(fs));
    let big = fit(&t.headline, w - pad * 2.0, big, fs * 0.9);
    let head_h = big * 1.6;
    let headline = Rect::new(pad, y, w, head_h);
    let (bg, ink) = tile_colors(t.headline_state, theme);
    c.base.rects.push(FillRect {
        rect: headline,
        color: bg,
        clip: None,
    });
    c.overlay.labels.push(label(
        t.headline.clone(),
        [pad * 2.0, y + head_h / 2.0],
        anchor(HAlign::Left, VAlign::Center),
        big,
        ink,
    ));
    y += head_h + pad * 0.6;

    // The meter's small lines: the limits' place and the correction; then the prediction.
    let sfs = theme.small_font_size.max(fs * 0.85);
    let info: Vec<String> = t
        .limits_from
        .iter()
        .chain(t.correction.iter())
        .cloned()
        .collect();
    if !info.is_empty() {
        c.overlay.labels.push(label(
            crate::spl::cut(&info.join(" · "), w, sfs),
            [pad, y],
            anchor(HAlign::Left, VAlign::Top),
            sfs,
            theme.text_dim,
        ));
        y += sfs * 1.5;
    }
    if let Some(p) = &t.predicted {
        let pfs = (big * 0.7).max(fs);
        let pfs = fit(&p.line, w, pfs, sfs);
        let ink = match p.state {
            TileState::Over => theme.banner_fault.background,
            TileState::Near => theme.banner_warning.background,
            _ => theme.text,
        };
        c.overlay.labels.push(label(
            p.line.clone(),
            [pad, y],
            anchor(HAlign::Left, VAlign::Top),
            pfs,
            ink,
        ));
        y += pfs * 1.5;
    }

    // The rows, stacked: each its caption and key, then its columns.
    let n_win = t.rows.len().max(1);
    let section_h = ((size.height - y - pad) / n_win as f32).max(1.0);
    let mut columns = Vec::with_capacity(t.rows.len());
    let mut ranges = Vec::with_capacity(t.rows.len());
    for (wi, win) in t.rows.iter().enumerate() {
        let top = y + wi as f32 * section_h;
        let range = bar_range(&win.bars);
        let worst = t
            .worst
            .filter(|&(w, _)| w == wi)
            .map(|(_, i)| i)
            .or(win.worst);
        let cols = draw_window(&mut c, win, worst, range, (top, section_h), pad, w, theme);
        columns.push(cols);
        ranges.push(range);
    }
    BandLeqScene {
        scene: c.into_scene(size),
        columns,
        headline,
        ranges,
        banners: strip.rows,
    }
}

/// One row's caption, key and columns in the section `(top, height)`; its columns.
#[allow(clippy::too_many_arguments)]
fn draw_window(
    c: &mut Canvas,
    t: &BandRowText,
    worst: Option<usize>,
    range: (f64, f64),
    (top, height): (f32, f32),
    pad: f32,
    w: f32,
    theme: &Theme,
) -> Vec<Rect> {
    let fs = theme.font_size;
    let sfs = theme.small_font_size.max(fs * 0.85);
    let mut y = top;
    // The key to the two marks across the columns, right of the caption: a mark is keyed
    // only where one is drawn, so an allowed level off every column's scale has none.
    let limit_stroke = Stroke::solid(theme.text, 3.0);
    let allowed_stroke = Stroke::solid(theme.text_dim, 2.0);
    let allowed_drawn = |b: &BandBar| b.allowed_db.filter(|a| (range.0..=range.1).contains(a));
    let mut keys: Vec<(&str, Stroke)> = Vec::new();
    if let Some(k) = &t.limit_key {
        keys.push((k, limit_stroke));
    }
    if let Some(k) = t
        .allowed_key
        .as_ref()
        .filter(|_| t.bars.iter().any(|b| allowed_drawn(b).is_some()))
    {
        keys.push((k, allowed_stroke));
    }
    // Each mark left of its word, the word placed from the mark: an estimated text width
    // then widens only the gap to the next pair, never the one inside a pair.
    let mark_w = sfs * 1.6;
    let pair_w = |text: &str| mark_w + sfs * 0.4 + canvas::text_width(text, sfs);
    let keys_w: f32 = keys.iter().map(|(t, _)| pair_w(t) + sfs * 1.5).sum();
    let right = pad + w;
    let key_x = right - keys_w + sfs * 1.5;
    let mut x = key_x;
    for (text, stroke) in &keys {
        let mark_y = y + sfs * 0.6;
        c.overlay.polylines.push(Polyline {
            points: vec![[x, mark_y], [x + mark_w, mark_y]],
            alpha: Vec::new(),
            stroke: *stroke,
            clip: None,
        });
        c.overlay.labels.push(label(
            *text,
            [x + mark_w + sfs * 0.4, y],
            anchor(HAlign::Left, VAlign::Top),
            sfs,
            theme.text_dim,
        ));
        x += pair_w(text) + sfs * 1.5;
    }
    c.overlay.labels.push(label(
        crate::spl::cut(&t.caption(), (key_x - sfs - pad).max(1.0), sfs),
        [pad, y],
        anchor(HAlign::Left, VAlign::Top),
        sfs,
        theme.text,
    ));
    y += sfs * 1.5;

    // The columns: a y scale left, one column per band, the band labels under them; a
    // column no wider than one of the 11 low-frequency bands, so a lone band reads as a
    // bar, not a slab.
    let scale_w = fs * 3.0;
    let label_fs = fs.max(((w - scale_w) / WIDEST_OF as f32 * 0.22).min(fs * 1.6));
    let below_h = label_fs * 1.4 + sfs * 1.4;
    let area = Rect::new(
        pad + scale_w,
        y + pad * 0.5,
        (w - scale_w).max(1.0),
        (top + height - y - pad * 1.5 - below_h).max(1.0),
    );
    let to_y = |db: f64| -> f32 {
        let k = ((db - range.0) / (range.1 - range.0)).clamp(0.0, 1.0);
        area.bottom() - (k as f32) * area.h
    };
    // Grid lines every 10 dB, fewer when the section is short.
    let step = if area.h < sfs * 8.0 { 20.0 } else { 10.0 };
    let mut db = range.0;
    while db <= range.1 + 1e-9 {
        let gy = to_y(db);
        c.base.polylines.push(Polyline {
            points: vec![[area.x, gy], [area.right(), gy]],
            alpha: Vec::new(),
            stroke: theme.grid_major,
            clip: None,
        });
        c.overlay.labels.push(label(
            format::fixed(db, 0),
            [area.x - 4.0, gy],
            anchor(HAlign::Right, VAlign::Center),
            sfs,
            theme.axis_text,
        ));
        db += step;
    }
    let col_w = area.w / t.bars.len().max(WIDEST_OF) as f32;
    let gap = (col_w * 0.12).max(2.0);
    let mut columns = Vec::with_capacity(t.bars.len());
    for (i, b) in t.bars.iter().enumerate() {
        let x = area.x + i as f32 * col_w + gap / 2.0;
        let cw = (col_w - gap).max(1.0);
        let col = Rect::new(x, area.y, cw, area.h);
        columns.push(col);
        let (track, bar) = column_colors(b.state, theme);
        c.base.rects.push(FillRect {
            rect: col,
            color: track,
            clip: None,
        });
        let bar_db = b.drawn_db.unwrap_or(b.leq_db);
        if bar_db.is_finite() {
            let top = to_y(bar_db);
            c.data.rects.push(FillRect {
                rect: Rect::new(x, top, cw, (area.bottom() - top).max(0.0)),
                color: bar,
                clip: None,
            });
        }
        // The headroom: how loud the band may go, a thin line under the limit.
        // Off the scale it would sit on the top or bottom edge, reading as a level it is not.
        if let Some(a) = allowed_drawn(b) {
            let ay = to_y(a);
            c.overlay.polylines.push(Polyline {
                points: vec![[x + cw * 0.2, ay], [x + cw * 0.8, ay]],
                alpha: Vec::new(),
                stroke: allowed_stroke,
                clip: None,
            });
        }
        if let Some(l) = b.limit_db {
            let ly = to_y(l);
            c.overlay.polylines.push(Polyline {
                points: vec![[x - gap * 0.3, ly], [x + cw + gap * 0.3, ly]],
                alpha: Vec::new(),
                stroke: limit_stroke,
                clip: None,
            });
        }
        // The value, held low on the column: read up close, the bars read from afar.
        let vfs = (cw * 0.24).clamp(sfs * 0.8, fs * 1.4);
        let value_ink = if bar_db.is_finite() && to_y(bar_db) < area.bottom() - vfs * 1.6 {
            readable_on(bar, theme)
        } else {
            theme.text
        };
        c.overlay.labels.push(label(
            b.value.clone(),
            [x + cw / 2.0, area.bottom() - 4.0],
            anchor(HAlign::Center, VAlign::Bottom),
            vfs,
            value_ink,
        ));
        let worst = worst == Some(i);
        c.overlay.labels.push(label(
            b.label.clone(),
            [x + cw / 2.0, area.bottom() + 4.0],
            anchor(HAlign::Center, VAlign::Top),
            label_fs,
            if worst { theme.text } else { theme.axis_text },
        ));
        if let Some(u) = &b.under {
            c.overlay.labels.push(label(
                u.clone(),
                [x + cw / 2.0, area.bottom() + 4.0 + label_fs * 1.3],
                anchor(HAlign::Center, VAlign::Top),
                sfs.min(cw * 0.22).max(READABLE),
                if b.state == TileState::Over {
                    theme.banner_fault.background
                } else {
                    theme.text_dim
                },
            ));
        }
        if worst {
            c.overlay.polylines.push(Polyline {
                points: vec![
                    [x, area.y],
                    [x + cw, area.y],
                    [x + cw, area.bottom()],
                    [x, area.bottom()],
                    [x, area.y],
                ],
                alpha: Vec::new(),
                stroke: Stroke::solid(theme.text, 2.0),
                clip: None,
            });
        }
    }
    c.overlay.labels.push(label(
        "Hz",
        [area.x - 4.0, area.bottom() + 4.0],
        anchor(HAlign::Right, VAlign::Top),
        sfs,
        theme.axis_text,
    ));
    columns
}

/// A column is at most this many to a row's width.
const WIDEST_OF: usize = 11;

/// Text smaller than this is left out of a narrow column.
const READABLE: f32 = 8.0;

/// `size`, shrunk down to `min` until `text` fits `w`.
fn fit(text: &str, w: f32, size: f32, min: f32) -> f32 {
    let need = canvas::text_width(text, size);
    if need <= w {
        size
    } else {
        (size * w / need).max(min)
    }
}

/// Text ink on a coloured bar.
fn readable_on(bar: Color, theme: &Theme) -> Color {
    if crate::theme::contrast_ratio(theme.text, bar) >= 3.0 {
        theme.text
    } else {
        theme.background
    }
}
