//! Rolling Leq windows (`docs/design/leq.md`): one tile per window, big enough to read from
//! across a room — amber when near its limit, red when over, back to normal when it
//! recovers — and a history strip of each window's value against its limit.
//!
//! Display truth only: the daemon judges (the state comes with the `leq` frame), this
//! module words and colours it. Window names follow IEC 61672 notation (`LAeq 30 min`).

use std::collections::VecDeque;

use ac2_proto::frame::{LeqFlags, LeqFrame, LeqPeak};
use ac2_proto::model::{
    LeqAlarm, LeqAlarmKind, LeqConfig, LeqJudgement, LeqWindow, LevelScale, PeakQuantity,
    PositionCorrection, SplHistory, Weighting,
};

use crate::axis::{self, Axis, Mapping, Range, Scale, Steps};
use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, anchor, label};
use crate::format;
use crate::primitives::{
    Color, Dash, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport,
};
use crate::theme::Theme;
use crate::view::{LeqLayout, LeqStyle, PlotChrome, Style};

mod caption;
mod columns;
mod draw;
mod history;
mod run;
mod tiles;
/// A window length in words (`1 min 30 s`), as the daemon and the CLI say it too.
pub use ac2_proto::model::window_length as length;
use caption::{Caption, caption, caption_left};
pub use columns::{
    ABOVE_LIMIT_DB, BELOW_LIMIT_DB, Behind, FREE_SPAN_DB, LeqColumn, LeqColumns, NAME_RATIO,
    VALUE_RATIO, ValueLabel, column_colors, column_range, shared_range,
};
use draw::{columns, draw_history, draw_tile, tile_below};
pub use history::{
    HISTORY_S, HistoryLine, HistoryPoint, HistoryStrip, LeqHistory, LeqStripView, ReadoutRow,
    StripReadout, StripSpan, history_caption, history_span, history_strip, strip_readout,
};
#[cfg(test)]
use history::{decimate, lay_out_strip};
pub use run::{LeqRunText, NewLogConfirm, new_log_confirm, run_text};
use tiles::TILE_INSTRUCTION_EM;
pub use tiles::{LeqTile, TileKind, TileState, leq_tiles, tile_colors, time_to};

/// `LAeq 30 min`.
pub fn window_name(w: &LeqWindow) -> String {
    ac2_proto::model::window_name(w.duration.0, w.weighting)
}

/// `corrected +5.0 dB`, or with a peak difference of its own `corrected +5.0 dB, peaks
/// +1.0 dB`: the measuring-position correction in the levels shown.
pub fn position_text(p: &PositionCorrection) -> String {
    let db = |v: f64| format!("{v:+.1} dB");
    if p.level == p.peak {
        format!("corrected {}", db(p.level.0))
    } else {
        format!("corrected {}, peaks {}", db(p.level.0), db(p.peak.0))
    }
}

/// The unit a value is shown with when corrected: the plain unit then `corr.`, so a
/// corrected number never reads as a measured one (the caption says by how much).
const CORRECTED_MARK: &str = "corr.";

/// What an alarm says: `FOH SPL: LAeq 30 min over its limit — 101.2 dB SPL > 99.0 dB SPL`,
/// `FOH SPL: LCpeak back within its limit — 133.0 dB SPL (corrected +2.0 dB)`. `true` for
/// an over, which the app shows as an error.
pub fn alarm_text(meter: &str, a: &LeqAlarm) -> (bool, String) {
    let what = a.subject.to_string();
    let corrected = match a.position {
        Some(p) => format!(" (corrected {:+.1} dB)", p.0),
        None => String::new(),
    };
    match a.kind {
        LeqAlarmKind::Over => (
            true,
            format!(
                "{meter}: {what} over its limit — {} > {}{corrected}",
                format::spl(a.level.0),
                format::spl(a.limit.0)
            ),
        ),
        LeqAlarmKind::Recovered => (
            false,
            format!(
                "{meter}: {what} back within its limit — {}{corrected}",
                format::spl(a.level.0)
            ),
        ),
    }
}

/// Elapsed time as a clock: `0:05`, `12:30`, `1:02:03`.
pub fn clock(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return format::NO_VALUE.to_string();
    }
    let s = seconds.floor() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

// ---------------------------------------------------------------------------------------
// Scene

/// What the Leq view shows.
#[derive(Clone, Debug, PartialEq)]
pub struct LeqView<'a> {
    /// `FOH SPL`.
    pub meter: String,
    /// Calibration text (`spl::cal_text`), with the mic.
    pub cal: String,
    pub cfg: &'a LeqConfig,
    pub tiles: Vec<LeqTile>,
    pub history: Option<&'a LeqHistory>,
    /// `STALE 3.2 s`.
    pub stale: Option<String>,
    /// What the values are in (the columns' scale without limits starts from it).
    pub scale: LevelScale,
    pub layout: LeqLayout,
    /// The log as a whole: run clock, start, total, gaps (`None` before its first second).
    pub run: Option<LeqRunText>,
    /// Full screen (the stage view): the windows read across a room, so the caption (meter,
    /// calibration) is drawn only with the history on or the values stale.
    pub stage: bool,
    /// Grid and labels of the history strip.
    pub chrome: crate::view::PlotChrome,
    /// Where the history strip looks and the pointer over it.
    pub strip: LeqStripView<'a>,
    /// The scale the Leq-column panes on screen share ([`shared_range`]); the columns span
    /// it and their own ([`pane_range`]). `None`: their own alone.
    pub shared: Option<Range>,
}

/// The columns' own scale of a pane showing `tiles` in unit `scale` ([`column_range`] on its
/// judged limits and values, kept by its history while the levels allow).
pub fn pane_range(tiles: &[LeqTile], scale: LevelScale, history: Option<&LeqHistory>) -> Range {
    let limits: Vec<f64> = tiles.iter().filter_map(|t| t.limit_db).collect();
    let values: Vec<f64> = tiles.iter().map(|t| t.leq_db).collect();
    column_range(&limits, &values, scale, history.and_then(LeqHistory::range))
}

/// The Leq view as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct LeqScene {
    pub scene: Scene,
    /// Each tile's rectangle, in window order (tiles layout).
    pub tiles: Vec<Rect>,
    /// The columns, shortest window left (columns layout).
    pub columns: Option<LeqColumns>,
    /// The history strip, when there is room for it.
    pub history: Option<HistoryStrip>,
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
    /// The caption above the windows: meter, run, calibration.
    pub caption: Rect,
    /// The run's wording as drawn, if there was room for any.
    pub run: Option<String>,
    /// The strip's caption while it looks back (`history · 14:02–15:10 · Home: live`).
    pub strip_caption: Option<String>,
    /// The cursor readout drawn over the strip.
    pub strip_readout: Option<StripReadout>,
}

/// Lays the view out in `size` below the banner strip: the meter and calibration on one
/// line, the windows as columns or tiles, and the history strip below them when it is on
/// and the pane is tall enough.
pub fn leq_scene(v: &LeqView<'_>, status: &Status, theme: &Theme, size: Viewport) -> LeqScene {
    leq_scene_under(v, status, theme, size, |_, area| (area, ())).0
}

/// [`leq_scene`] with `head` given the area under the caption first: it draws what goes
/// above the windows (the meter of the meter + Leq view) and returns the area it leaves
/// them. The caption stays the one caption of the whole pane.
pub(crate) fn leq_scene_under<T>(
    v: &LeqView<'_>,
    status: &Status,
    theme: &Theme,
    size: Viewport,
    head: impl FnOnce(&mut Canvas, Rect) -> (Rect, T),
) -> (LeqScene, T) {
    let mut c = Canvas::new(size, theme);
    let pad = 10.0;
    let strip = canvas::banner_strip(&mut c, status, pad, size.width - 2.0 * pad, size, theme);
    let top = strip.rect.bottom() + pad;
    let n = v.tiles.len();
    // The columns decide whether their names leave the weighting to the caption once the
    // caption's height is known; the run is placed clear of the longer of the two texts.
    // Peak limits keep their own names (`LCpeak`), so only the windows decide.
    let windows: Vec<&LeqTile> = v
        .tiles
        .iter()
        .filter(|t| t.kind == TileKind::Window)
        .collect();
    let shared = windows
        .first()
        .map(|t| t.weighting)
        .filter(|w| windows.iter().all(|t| t.weighting == *w))
        .map(|w| format!("L{}eq", w.letter()));
    let right = match &v.stale {
        Some(s) => format!("{s} · {}", v.cal),
        None => v.cal.clone(),
    };
    // The meter and the calibration share a row: on a narrow pane the calibration gives
    // way first (down to half the row, its STALE first), then the meter's name.
    let row_w = (size.width - 2.0 * pad - theme.font_size).max(1.0);
    let fs = theme.font_size;
    let left_w = canvas::text_width(&caption_left(v, shared.as_deref()), fs);
    let right = crate::spl::cut(&right, (row_w - left_w).max(row_w * 0.5), fs);
    let left_room = row_w - canvas::text_width(&right, fs);
    // On the stage the caption is the look back's (with the history) or the warning's
    // (stale values): otherwise the windows and the number are the whole picture.
    let captioned = !v.stage || v.layout.history || v.stale.is_some();
    let cap = if captioned {
        caption(
            v,
            &crate::spl::cut(&caption_left(v, shared.as_deref()), left_room, fs),
            &right,
            top,
            size,
            theme,
        )
    } else {
        Caption {
            height: 0.0,
            run: None,
        }
    };
    let area = Rect::new(
        pad,
        top + cap.height,
        (size.width - 2.0 * pad).max(1.0),
        (size.height - top - cap.height - pad).max(1.0),
    );
    let (area, extra) = head(&mut c, area);
    // The strip takes a third of a tall pane; a short one is all windows.
    let show_history =
        v.layout.history && v.history.is_some() && area.h >= 300.0 && area.w >= 300.0;
    let (tiles_area, hist_area) = if show_history {
        let hh = (area.h * 0.34).min(260.0);
        (
            Rect::new(area.x, area.y, area.w, area.h - hh - pad),
            Some(Rect::new(
                area.x + canvas::MARGINS.left - pad,
                area.bottom() - hh + canvas::MARGINS.top,
                area.w - canvas::MARGINS.left + pad,
                hh - canvas::MARGINS.top - canvas::MARGINS.bottom,
            )),
        )
    } else {
        (area, None)
    };
    let mut rects = Vec::new();
    let mut cols = None;
    if n == 0 {
        c.overlay.labels.push(label(
            "no Leq windows: add them in Settings › SPL / Leq",
            [
                tiles_area.x + tiles_area.w / 2.0,
                tiles_area.y + tiles_area.h / 2.0,
            ],
            anchor(HAlign::Center, VAlign::Center),
            theme.font_size,
            theme.text_dim,
        ));
    } else if v.layout.style == LeqStyle::Columns {
        let own = pane_range(&v.tiles, v.scale, v.history);
        let range = v.shared.map_or(own, |s| columns::union(own, s));
        cols = Some(columns::draw_columns(
            &mut c,
            &v.tiles,
            range,
            tiles_area,
            v.stale.is_some(),
            Style {
                chrome: v.chrome,
                theme,
            },
        ));
    } else {
        let gap = 8.0;
        let ncols = columns(n, tiles_area.w, tiles_area.h);
        let rows = n.div_ceil(ncols);
        let tw = (tiles_area.w - gap * (ncols as f32 - 1.0)) / ncols as f32;
        let th = (tiles_area.h - gap * (rows as f32 - 1.0)) / rows as f32;
        // Every tile keeps room for as many small lines as the fullest one, so all bodies
        // are one height and the values one size.
        let lines_below = v
            .tiles
            .iter()
            .map(|t| tile_below(t, v.chrome).len())
            .max()
            .unwrap_or(0);
        for (i, t) in v.tiles.iter().enumerate() {
            let (row, col) = (i / ncols, i % ncols);
            let r = Rect::new(
                tiles_area.x + col as f32 * (tw + gap),
                tiles_area.y + row as f32 * (th + gap),
                tw.max(1.0),
                th.max(1.0),
            );
            draw_tile(
                &mut c,
                t,
                r,
                v.stale.is_some(),
                shared.as_deref(),
                lines_below,
                Style {
                    chrome: v.chrome,
                    theme,
                },
            );
            rects.push(r);
        }
    }
    let left = crate::spl::cut(
        &caption_left(
            v,
            cols.as_ref()
                .map_or(shared.as_deref(), |k| k.weighting.as_deref()),
        ),
        left_room,
        fs,
    );
    // The caption: the meter (with the unit, and the weighting the columns' names leave
    // out), the run, and the calibration.
    if captioned {
        c.overlay.labels.push(label(
            left,
            [pad, top],
            anchor(HAlign::Left, VAlign::Top),
            theme.font_size,
            theme.text_dim,
        ));
    }
    if let Some((text, pos, size)) = &cap.run {
        c.overlay.labels.push(label(
            text.clone(),
            *pos,
            anchor(HAlign::Center, VAlign::Top),
            *size,
            if v.stale.is_some() {
                theme.text_dim
            } else {
                theme.text
            },
        ));
    }
    if captioned {
        c.overlay.labels.push(label(
            right,
            [size.width - pad, top],
            anchor(HAlign::Right, VAlign::Top),
            theme.font_size,
            if v.stale.is_some() {
                theme.banner_warning.background
            } else {
                theme.text_dim
            },
        ));
    }
    let judged = v
        .tiles
        .iter()
        .any(|t| matches!(t.state, TileState::Ok | TileState::Near | TileState::Over));
    let mut strip_caption = None;
    let mut readout = None;
    let history = match (hist_area, v.history) {
        (Some(r), Some(h)) => {
            let pts = v.strip.points.unwrap_or(h);
            let s = history_strip(v.cfg, pts, judged, r, theme, v.strip.span);
            draw_history(&mut c, &s, shared.as_deref(), v.chrome, theme);
            let off = v.strip.span.utc_offset_s;
            if v.strip.span.span.is_some() {
                let text = history_caption(
                    s.from,
                    s.to,
                    off,
                    v.strip.live_key.as_deref(),
                    v.strip.loading,
                );
                draw::history_note(&mut c, &s, &text, theme);
                strip_caption = Some(text);
            }
            if v.chrome.cursor()
                && let Some(t) = v.strip.cursor.filter(|t| *t >= s.from && *t <= s.to)
                && let Some(r) = strip_readout(v.cfg, pts, judged, t, off)
            {
                draw::history_cursor(&mut c, &s, t, &r, theme);
                readout = Some(r);
            }
            Some(s)
        }
        _ => None,
    };
    let scene = LeqScene {
        scene: c.into_scene(size),
        tiles: rects,
        columns: cols,
        history,
        strip: strip.rect,
        banners: strip.rows,
        caption: Rect::new(0.0, top, size.width, cap.height),
        run: cap.run.map(|(t, ..)| t),
        strip_caption,
        strip_readout: readout,
    };
    (scene, extra)
}

#[cfg(test)]
mod tests;
