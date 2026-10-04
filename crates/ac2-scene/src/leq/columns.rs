//! The Leq windows as columns, for reading from the stage or across a room: one
//! full-height column per window, shortest window left, each a bar filling bottom → top
//! with the window's Leq — like a meter that fills — the window's name at the bottom, and
//! on top, large, what a performer acts on: the state (`OVER`) and how loud the next stretch
//! may be (`next 1 min` / `stay ≤ 101.5 dB`). The window's value is a smaller figure at its
//! bar's top, inked to read on the fill or coloured like the bar above it: the value of
//! each window matters less on stage than whether to back off, and the meter's number above
//! the windows stays the one big number.
//!
//! All columns share one scale so their bars compare: anchored to the limits when the
//! windows are judged, else a range that follows the values in 10 dB steps with hysteresis
//! (it must not move every second).

use ac2_proto::model::{LevelScale, Weighting};

use crate::axis::Range;
use crate::canvas::{self, Canvas, anchor, label};
use crate::format;
use crate::primitives::{Color, FillRect, HAlign, Polyline, Rect, Stroke, VAlign};
use crate::theme::{Theme, contrast_ratio};

use super::{LeqTile, TileState, length, w_letter};

/// A judged scale starts this far below the lowest limit…
pub const BELOW_LIMIT_DB: f64 = 30.0;
/// …and ends this far above the highest: room to see how far over a window is.
pub const ABOVE_LIMIT_DB: f64 = 6.0;
/// Span of the scale without a judged limit.
pub const FREE_SPAN_DB: f64 = 40.0;

/// How strongly an over column's background takes the fault colour, and a near one's the
/// warning colour.
const OVER_TINT: f32 = 0.45;
const NEAR_TINT: f32 = 0.18;
/// A filling window's bar, part of the way from the track to its colour: still a level,
/// visibly not a whole window yet.
const FILLING_STRENGTH: f32 = 0.45;
/// Size of the unit beside a column's value, as a fraction of the value's size.
const UNIT_RATIO: f32 = 0.45;
/// Gap between a column's value and its unit, as a fraction of the value's size.
const UNIT_GAP: f32 = 0.08;
/// Size of the state and the instruction, as a share of the area's height: read from the
/// stage, yet well under the meter's number (a third of the pane) above them.
const LARGE_SHARE: f32 = 0.06;
/// The value's size at most, as a fraction of the state's and instruction's.
pub const VALUE_RATIO: f32 = 0.8;
/// A large wording shrinks down to this fraction of its size before a shorter one is used.
const SHRINK: f32 = 0.7;
/// Text smaller than this is left out.
const READABLE: f32 = 8.0;
/// A value above its bar takes the bar's colour when it contrasts this much with the track
/// (the WCAG minimum for large text).
const INK_CONTRAST: f64 = 3.0;

/// The bar scale of the columns.
///
/// With judged limits (`limits`: those of windows the daemon judges): from
/// [`BELOW_LIMIT_DB`] under the lowest limit to [`ABOVE_LIMIT_DB`] over the highest, the
/// same for every column. Without: [`FREE_SPAN_DB`] whose top is a multiple of 10 dB at
/// least 5 dB above the loudest window; the previous range (`prev`) is kept while the
/// loudest window stays between 20 dB and 2 dB under its top, so the scale moves only when
/// a level nears the top or has fallen well below it.
pub fn column_range(
    limits: &[f64],
    values: &[f64],
    scale: LevelScale,
    prev: Option<Range>,
) -> Range {
    let finite = |v: &&f64| v.is_finite();
    let lo_limit = limits.iter().filter(finite).copied().reduce(f64::min);
    let hi_limit = limits.iter().filter(finite).copied().reduce(f64::max);
    if let (Some(lo), Some(hi)) = (lo_limit, hi_limit) {
        return Range::new(lo - BELOW_LIMIT_DB, hi + ABOVE_LIMIT_DB);
    }
    let max = values.iter().filter(finite).copied().reduce(f64::max);
    let top_for = |m: f64| ((m + 5.0) / 10.0).ceil() * 10.0;
    let top = match (prev, max) {
        (Some(p), None) => p.hi,
        (Some(p), Some(m)) if m <= p.hi - 2.0 && m >= p.hi - 20.0 => p.hi,
        (_, Some(m)) => top_for(m),
        (None, None) => match scale {
            LevelScale::DbSpl => 100.0,
            LevelScale::Dbfs => 0.0,
        },
    };
    Range::new(top - FREE_SPAN_DB, top)
}

/// One column as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct LeqColumn {
    /// The window's index in the configuration (columns are ordered by length).
    pub window: usize,
    pub rect: Rect,
    pub background: Color,
    /// The bar's full scale, and the bar itself from the bottom up to its level (none
    /// before anything was measured or below the scale).
    pub track: Rect,
    pub bar: Option<Rect>,
    /// The bar's level: the Leq, or while filling the Leq the window ends at if the rest
    /// is silent ([`LeqTile::bar_db`]).
    pub bar_db: f64,
    pub bar_color: Color,
    /// The track's colour behind the bar.
    pub track_color: Color,
    /// y of the limit marker, drawn across the whole column.
    pub limit_y: Option<f32>,
    /// The window's value at its bar's top.
    pub value: ValueLabel,
    /// The name under the column: `LAeq 30 min`, `30 min` or `30m`, as the width allows.
    pub name: String,
    pub filling: bool,
}

/// The columns as laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct LeqColumns {
    pub columns: Vec<LeqColumn>,
    pub range: Range,
    /// The dB scale left of the columns, when there is room for it.
    pub gutter: Option<Rect>,
    /// The weighting the shortened names leave out (`LAeq`), for the caption.
    pub weighting: Option<String>,
    /// The type size of the state and the instruction (a shrunk wording is smaller).
    pub large: f32,
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::rgba(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
}

/// Background and bar colour of a column (before the filling dims it).
pub fn column_colors(state: TileState, theme: &Theme) -> (Color, Color) {
    match state {
        TileState::Over => (
            mix(
                theme.plot_background,
                theme.banner_fault.background,
                OVER_TINT,
            ),
            theme.banner_fault.background,
        ),
        TileState::Near => (
            mix(
                theme.plot_background,
                theme.banner_warning.background,
                NEAR_TINT,
            ),
            theme.banner_warning.background,
        ),
        TileState::Ok => (theme.plot_background, theme.level_ok),
        // No judgement, no colour.
        TileState::NoLimit | TileState::NotCalibrated => (theme.plot_background, theme.text_dim),
    }
}

/// `30m`, `1h`, `45s`, `1m30s`.
fn compact(seconds: f64) -> String {
    let s = seconds.round().max(0.0) as u64;
    if s < 60 {
        format!("{s}s")
    } else if s > 7200 && s.is_multiple_of(3600) {
        format!("{}h", s / 3600)
    } else if s.is_multiple_of(60) {
        format!("{}m", s / 60)
    } else {
        format!("{}m{}s", s / 60, s % 60)
    }
}

/// The names of every column at each level of shortening, longest first.
fn name_levels(tiles: &[&LeqTile]) -> Vec<(Vec<String>, bool)> {
    let one = tiles
        .first()
        .map(|t| t.weighting)
        .filter(|w| tiles.iter().all(|t| t.weighting == *w));
    let pre = |w: Weighting| match one {
        Some(_) => String::new(),
        None => w_letter(w).to_string(),
    };
    let sep = |w: Weighting| match one {
        Some(_) => String::new(),
        None => format!("{} ", w_letter(w)),
    };
    // One weighting for every window: the caption names it once (`LAeq`), the columns only
    // their lengths; mixed weightings keep it on every name.
    let full = (tiles.iter().map(|t| t.name.clone()).collect(), false);
    let mut levels = vec![
        full,
        (
            tiles
                .iter()
                .map(|t| format!("{}{}", sep(t.weighting), length(t.duration_s)))
                .collect(),
            one.is_some(),
        ),
        (
            tiles
                .iter()
                .map(|t| format!("{}{}", pre(t.weighting), compact(t.duration_s)))
                .collect(),
            one.is_some(),
        ),
    ];
    if one.is_some() {
        levels.remove(0);
    }
    levels
}

/// The first of `candidates` that fits `width` at `size`.
fn fitting(candidates: &[String], width: f32, size: f32) -> Option<String> {
    candidates
        .iter()
        .find(|c| canvas::text_width(c, size) <= width)
        .cloned()
}

/// The first of `candidates` that fits `width` at no less than [`SHRINK`] of `size`, at the
/// largest size up to `size` it fits; else the first that fits at `floor` or more; else the
/// last (shortest) one if it stays readable. A long wording shrunk a little reads better
/// than a cryptic short one at full size.
pub(super) fn fitting_shrunk(
    candidates: &[String],
    width: f32,
    size: f32,
    floor: f32,
) -> Option<(String, f32)> {
    let at = |c: &String| size.min(width / canvas::text_width(c, 1.0).max(1e-3));
    candidates
        .iter()
        .find(|c| at(c) >= size * SHRINK)
        .or_else(|| candidates.iter().find(|c| at(c) >= floor))
        .or(candidates.last())
        .map(|c| (c.clone(), at(c)))
        .filter(|(_, s)| *s >= READABLE)
}

/// The wording level (index into each column's wordings, a column with fewer using its
/// last) at which every column's wording fits `width` at no less than [`SHRINK`] of `size`;
/// else the first at which each fits at `floor` or more; else the shortest.
fn shared_level(all: &[&Vec<String>], width: f32, size: f32, floor: f32) -> usize {
    let depth = all.iter().map(|c| c.len()).max().unwrap_or(0);
    let worst = |l: usize| {
        all.iter()
            .filter(|c| !c.is_empty())
            .map(|c| width / canvas::text_width(&c[l.min(c.len() - 1)], 1.0).max(1e-3))
            .fold(f32::INFINITY, f32::min)
    };
    (0..depth)
        .find(|&l| worst(l) >= size * SHRINK)
        .or_else(|| (0..depth).find(|&l| worst(l) >= floor))
        .unwrap_or(depth.saturating_sub(1))
}

/// The header rows of a column, top to bottom, each its wordings longest first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    /// `OVER`, `NEAR`, `ON COURSE — over in 12 min`, `OK`: large.
    State,
    /// What the instruction holds for: `next 1 min`, `until full`, `cooling down in`: small.
    Qualifier,
    /// What to do: `stay ≤ 101.5 dB`, or how long until back under: large.
    Instruction,
    /// `limit 99.0 dB`: small and dim, the line on the bar says it too.
    Limit,
}

const ROWS: [Row; 4] = [Row::State, Row::Qualifier, Row::Instruction, Row::Limit];

impl Row {
    fn large(self) -> bool {
        matches!(self, Row::State | Row::Instruction)
    }
}

/// A column's header wordings per [`Row`]: the state and what a performer acts on (stay
/// under this level for the horizon, or how long it is cooling down) read from the stage;
/// the limit is written small, its line across the bar shows where it is.
pub(super) fn detail_lines(t: &LeqTile, horizon: &str) -> [Vec<String>; 4] {
    let state = match t.state {
        TileState::Over => vec!["OVER".to_string()],
        TileState::Near if t.on_course => match t.over_in_s {
            Some(s) => {
                let d = super::time_to(s);
                vec![
                    format!("ON COURSE — over in {d}"),
                    format!("over in {d}"),
                    "ON COURSE".to_string(),
                ]
            }
            None => vec!["ON COURSE".to_string()],
        },
        TileState::Near => vec!["NEAR".to_string()],
        TileState::Ok => vec!["OK".to_string()],
        // Nothing judged: not a call to act on, so not large; said where the horizon goes.
        TileState::NotCalibrated | TileState::NoLimit => vec![],
    };
    let limit = match t.limit_db {
        Some(l) => {
            let l = format::level(l);
            vec![format!("limit {l} dB"), format!("limit {l}"), l]
        }
        None => vec![],
    };
    let stay = |a: f64| {
        let a = format::level(a);
        vec![
            format!("stay ≤ {a} dB"),
            format!("stay ≤ {a}"),
            format!("≤ {a}"),
        ]
    };
    let (qualifier, instruction) = if let Some(a) = t.allowed_db
        && t.allowed_until_full
    {
        (vec!["until full".to_string()], stay(a))
    } else if let Some(a) = t.allowed_db {
        (vec![format!("next {horizon}")], stay(a))
    } else if let Some(r) = t.recover_s {
        // The time back under the limit if the level stays at the limit.
        (
            vec!["cooling down in".to_string(), "cooling".to_string()],
            vec![format::duration(r), super::clock(r)],
        )
    } else if t.recover.is_some() {
        (
            vec![],
            vec!["cooling down".to_string(), "cooling".to_string()],
        )
    } else if t.state == TileState::NotCalibrated {
        (
            vec!["not calibrated".to_string(), "uncal.".to_string()],
            vec![],
        )
    } else {
        (vec![], vec![])
    };
    [state, qualifier, instruction, limit]
}

/// The line above the name: how far a filling window is (its value is the Leq so far), or
/// its gaps.
fn progress_line(t: &LeqTile) -> Vec<String> {
    let filling = t.filling().then(|| {
        (
            format!(
                "{} / {}",
                super::clock(t.elapsed_s),
                super::clock(t.duration_s)
            ),
            super::clock(t.elapsed_s),
        )
    });
    match (filling, &t.incomplete) {
        // "so far" is kept as long as anything is: the value is not a whole window's.
        (Some((f, short)), Some(i)) => vec![
            format!("so far · {f} · {i}"),
            format!("so far · {f}"),
            format!("so far · {short}"),
            f,
            short,
        ],
        (Some((f, short)), None) => vec![
            format!("so far · {f}"),
            format!("so far · {short}"),
            f,
            short,
        ],
        (None, Some(i)) => vec![i.clone(), "offline".to_string()],
        (None, None) => vec![],
    }
}

/// Where a column's value is drawn, and in what.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValueLabel {
    /// The box of the value and its unit (0.62 em per character, 1.25 em high).
    pub rect: Rect,
    /// The value's type size (the unit is smaller).
    pub size: f32,
    pub color: Color,
    /// On the bar's fill (inked for it) rather than above it on the track.
    pub on_bar: bool,
}

/// The colour of `candidates` that reads best on `bg`.
fn best_ink(bg: Color, candidates: &[Color]) -> Color {
    candidates
        .iter()
        .copied()
        .max_by(|a, b| contrast_ratio(*a, bg).total_cmp(&contrast_ratio(*b, bg)))
        .unwrap_or(bg)
}

/// Places a value box of height `bh` in `track` by the bar (`bar`, none before anything was
/// measured) clear of the limit line (`limit`: its y and half its width): on the fill under
/// its top edge when the fill is tall enough, else on the track just above the fill. Either
/// moves past the limit line rather than across it — a level near the limit is the one that
/// matters, and a line through its digits would hide them. Returns the box's top and
/// whether it is on the fill.
fn place_value(track: Rect, bar: Option<Rect>, limit: Option<(f32, f32)>, bh: f32) -> (f32, bool) {
    let gap = (bh * 0.15).max(2.0);
    let fill_top = bar.map_or(track.bottom(), |b| b.y);
    let clear = |top: f32| {
        top >= track.y - 0.01
            && top + bh <= track.bottom() + 0.01
            && limit.is_none_or(|(y, w)| top > y + w || top + bh < y - w)
    };
    let on_fill = |top: f32| top >= fill_top - 0.01 && clear(top);
    let mut tries: Vec<(f32, bool)> = vec![(fill_top + gap, true)];
    if let Some((y, w)) = limit {
        tries.push((y + w + gap, true));
    }
    tries.push((fill_top - gap - bh, false));
    if let Some((y, w)) = limit {
        tries.push((y - w - gap - bh, false));
    }
    tries
        .iter()
        .copied()
        .find(|&(top, fill)| {
            if fill {
                on_fill(top)
            } else {
                clear(top) && top + bh <= fill_top
            }
        })
        .unwrap_or_else(|| {
            // A track too short for the rules: as close above the fill as fits.
            let top = (fill_top - gap - bh).clamp(track.y, (track.bottom() - bh).max(track.y));
            (top, top >= fill_top)
        })
}

/// Lays the columns out in `area` and draws them on the scale `range`. `horizon` is the
/// headroom's horizon in words (`1 min`).
///
/// What a column says large is what a performer acts on: its state and how loud the next
/// stretch may be. The window's value is a figure on its bar — the bar is the level, and
/// the meter's own number above the windows is the one big number of the stage.
pub(super) fn draw_columns(
    c: &mut Canvas,
    tiles: &[LeqTile],
    range: Range,
    horizon: &str,
    area: Rect,
    stale: bool,
    theme: &Theme,
) -> LeqColumns {
    let mut order: Vec<usize> = (0..tiles.len()).collect();
    order.sort_by(|&a, &b| {
        tiles[a]
            .duration_s
            .total_cmp(&tiles[b].duration_s)
            .then_with(|| w_letter(tiles[a].weighting).cmp(w_letter(tiles[b].weighting)))
    });
    let sorted: Vec<&LeqTile> = order.iter().map(|&i| &tiles[i]).collect();
    let n = sorted.len().max(1) as f32;
    let h = area.h;
    let gap = (area.w * 0.012).clamp(3.0, 12.0);
    let small = (h * 0.028).clamp(9.0, 20.0);
    // The dB scale on the left when the columns stay wide enough beside it.
    let step = 10.0;
    let ticks: Vec<f64> = {
        let first = (range.lo / step).ceil() as i64;
        let last = (range.hi / step).floor() as i64;
        (first..=last).map(|k| k as f64 * step).collect()
    };
    let tick_w = ticks
        .iter()
        .map(|v| canvas::text_width(&format::fixed(*v, 0), small))
        .fold(0.0, f32::max)
        + 8.0;
    let with_gutter = (area.w - tick_w - gap * (n - 1.0)) / n >= 64.0;
    let (gutter, cols_area) = if with_gutter {
        (
            Some(Rect::new(area.x, area.y, tick_w, area.h)),
            Rect::new(area.x + tick_w, area.y, area.w - tick_w, area.h),
        )
    } else {
        (None, area)
    };
    let cw = ((cols_area.w - gap * (n - 1.0)) / n).max(1.0);
    let pad = (cw * 0.06).clamp(2.0, 12.0);
    let inner = (cw - 2.0 * pad).max(1.0);

    // Sizes shared by every column, so the bars line up and compare.
    let chars = sorted
        .iter()
        .map(|t| t.value.chars().count())
        .max()
        .unwrap_or(0)
        .max(5) as f32;
    // The value carries its unit and weighting on its baseline (`dB(A)`), so the value's
    // size leaves room for the widest unit; the bar it sits on is narrower than the column.
    let unit_w1 = sorted
        .iter()
        .map(|t| canvas::text_width(&t.weighted_unit, UNIT_RATIO))
        .fold(0.0, f32::max);
    let value_fit = ((inner - 2.0 * pad).max(1.0) / (0.62 * chars + UNIT_GAP + unit_w1)).max(4.0);
    let large = (h * LARGE_SHARE).clamp(10.0, 40.0);
    let small = small.min(large * 0.6);
    let name_size0 = (h * 0.05).clamp(11.0, 32.0);
    let levels = name_levels(&sorted);
    let mut names = levels[levels.len() - 1].clone();
    let mut name_size = 0.0;
    for (k, (lvl, drops)) in levels.iter().enumerate() {
        let longest = lvl
            .iter()
            .map(|s| canvas::text_width(s, 1.0))
            .fold(0.0, f32::max)
            .max(1e-3);
        let fit = name_size0.min(inner / longest);
        if fit >= name_size0.min(11.0) || k == levels.len() - 1 {
            names = (lvl.clone(), *drops);
            name_size = fit;
            break;
        }
    }
    let weighting = names.1.then(|| {
        sorted
            .first()
            .map(|t| format!("L{}eq", w_letter(t.weighting)))
            .unwrap_or_default()
    });

    // Header rows: which exist in any column, then drop the least important while the bar
    // would be too short. Unreadably small text is not drawn at all.
    let lines: Vec<[Vec<String>; 4]> = sorted.iter().map(|t| detail_lines(t, horizon)).collect();
    let progress: Vec<Vec<String>> = sorted.iter().map(|t| progress_line(t)).collect();
    let readable = small >= READABLE;
    let mut rows = [0, 1, 2, 3].map(|r| readable && lines.iter().any(|l| !l[r].is_empty()));
    // The progress line is the least of the figures: a little smaller.
    let progress_size = small * 0.8;
    let mut with_progress = progress_size >= 7.5 && progress.iter().any(|p| !p.is_empty());
    let row_h = |r: Row| {
        if r.large() { large * 1.25 } else { small * 1.3 }
    };
    let top_of_track = |rows: &[bool; 4]| {
        let header: f32 = ROWS
            .iter()
            .zip(rows)
            .filter(|(_, on)| **on)
            .map(|(r, _)| row_h(*r))
            .sum();
        cols_area.y + pad + header + if header > 0.0 { pad } else { 0.0 }
    };
    let name_top = cols_area.bottom() - pad - name_size * 1.25;
    let bottom_of_track = |p: bool| name_top - pad - if p { progress_size * 1.3 } else { 0.0 };
    let min_track = (h * 0.3).max(24.0);
    for shed in [
        Shed::Row(Row::Limit),
        Shed::Row(Row::Qualifier),
        Shed::Progress,
        Shed::Row(Row::State),
        Shed::Row(Row::Instruction),
    ] {
        if bottom_of_track(with_progress) - top_of_track(&rows) >= min_track {
            break;
        }
        match shed {
            Shed::Row(r) => rows[r as usize] = false,
            Shed::Progress => with_progress = false,
        }
    }
    let track_top = top_of_track(&rows);
    let track_bottom = bottom_of_track(with_progress).max(track_top + 1.0);
    let to_y = |v: f64| -> f32 {
        let t = ((v - range.lo) / range.span()).clamp(0.0, 1.0) as f32;
        track_bottom - t * (track_bottom - track_top)
    };
    // One wording level per large row for every column, so neighbours read alike
    // (`stay ≤ 102.0` beside `stay ≤ 85.3`, not `≤ 102.0`).
    let level = ROWS.map(|r| {
        let all: Vec<&Vec<String>> = lines.iter().map(|l| &l[r as usize]).collect();
        if r.large() {
            shared_level(&all, inner, large, small)
        } else {
            0
        }
    });
    // The value: smaller than the state and the instruction, and never more than a share
    // of the track, so it can sit by the bar's top anywhere on the scale.
    let value_size = value_fit
        .min(large * VALUE_RATIO)
        .min((track_bottom - track_top) * 0.4 / 1.25)
        .max(4.0);

    if let Some(g) = gutter {
        // A short track labels every second or fifth line, so the labels stay apart.
        let per_step = (track_bottom - track_top) * step as f32 / range.span().max(1.0) as f32;
        let every = [1.0, 2.0, 5.0, 10.0]
            .into_iter()
            .find(|k| per_step * *k as f32 >= 1.25 * small)
            .unwrap_or(10.0);
        let labelled = |v: f64| ((v / step).round() % every).abs() < 0.5;
        for v in ticks.iter().filter(|v| labelled(**v)) {
            c.overlay.labels.push(label(
                format::fixed(*v, 0),
                [g.right() - 6.0, to_y(*v)],
                anchor(HAlign::Right, VAlign::Center),
                small,
                theme.text_dim,
            ));
        }
    }

    let mut out = Vec::with_capacity(sorted.len());
    for (k, t) in sorted.iter().enumerate() {
        let r = Rect::new(
            cols_area.x + k as f32 * (cw + gap),
            cols_area.y,
            cw,
            cols_area.h,
        );
        let (bg, full) = column_colors(t.state, theme);
        let alarm = matches!(t.state, TileState::Over | TileState::Near);
        let filling = t.filling();
        let track = Rect::new(
            r.x + pad,
            track_top,
            inner,
            (track_bottom - track_top).max(1.0),
        );
        let track_color = mix(bg, theme.grid_major.color, 0.35);
        let bar_color = if filling && !alarm {
            mix(track_color, full, FILLING_STRENGTH)
        } else {
            full
        };
        c.base.rects.push(FillRect {
            rect: r,
            color: bg,
            clip: None,
        });
        c.base.rects.push(FillRect {
            rect: track,
            color: track_color,
            clip: None,
        });
        for v in &ticks {
            let y = to_y(*v);
            c.base.polylines.push(Polyline {
                points: vec![[track.x, y], [track.right(), y]],
                alpha: vec![],
                stroke: Stroke::solid(mix(track_color, theme.grid_major.color, 0.6), 1.0),
                clip: Some(track),
            });
        }
        // While filling the bar is the budget spent: the Leq the window ends at if the
        // rest is silent, reaching the limit line when going over becomes certain.
        let bar_db = t.bar_db();
        let bar = bar_db.is_finite().then(|| {
            let y = to_y(bar_db);
            Rect::new(track.x, y, track.w, track.bottom() - y)
        });
        let bar = bar.filter(|b| b.h > 0.0);
        if let Some(b) = bar {
            c.data.rects.push(FillRect {
                rect: b,
                color: bar_color,
                clip: Some(track),
            });
        }
        let limit_y = t.limit_db.map(&to_y);
        let limit_w = (h * 0.006).clamp(2.0, 5.0);
        if let Some(y) = limit_y {
            c.data.polylines.push(Polyline {
                points: vec![[r.x, y], [r.right(), y]],
                alpha: vec![],
                stroke: Stroke::solid(theme.text, limit_w),
                clip: Some(r),
            });
        }
        let fg = if stale && !alarm {
            theme.text_dim
        } else {
            theme.text
        };
        let cx = r.x + r.w / 2.0;

        // The value and its unit on one baseline, centred together on the bar; the unit
        // never drops, so a column can't be read in the weighting of an SPL meter shown
        // beside it.
        let unit_size = value_size * UNIT_RATIO;
        let value_w = canvas::text_width(&t.value, value_size);
        let unit_w = canvas::text_width(&t.weighted_unit, unit_size);
        let gap_w = value_size * UNIT_GAP;
        let total_w = value_w + gap_w + unit_w;
        let bh = value_size * 1.25;
        let (top, on_bar) = place_value(track, bar, limit_y.map(|y| (y, limit_w / 2.0)), bh);
        let inks = [
            theme.text,
            theme.plot_background,
            theme.banner_fault.text,
            theme.banner_warning.text,
        ];
        // On the fill: the ink that reads best on its colour (white on red, black on amber
        // or green). Above it: the bar's own colour where that reads on the track, so the
        // figure belongs to its bar; dim before anything was measured or while stale.
        let value_color = if on_bar {
            best_ink(bar_color, &inks)
        } else if bar.is_none() || (stale && !alarm) {
            theme.text_dim
        } else if contrast_ratio(bar_color, track_color) >= INK_CONTRAST {
            bar_color
        } else {
            best_ink(track_color, &inks)
        };
        let x0 = cx - total_w / 2.0 + value_w;
        let base = top + 0.95 * value_size;
        for (text, x, ha, size) in [
            (t.value.clone(), x0, HAlign::Right, value_size),
            (t.weighted_unit.clone(), x0 + gap_w, HAlign::Left, unit_size),
        ] {
            let mut l = label(
                text,
                [x, base],
                anchor(ha, VAlign::Baseline),
                size,
                value_color,
            );
            l.clip = Some(track);
            c.overlay.labels.push(l);
        }
        let value = ValueLabel {
            rect: Rect::new(cx - total_w / 2.0, top, total_w, bh),
            size: value_size,
            color: value_color,
            on_bar,
        };

        let mut push = |text: String, y: f32, v: VAlign, size: f32, color: Color| {
            let mut l = label(text, [cx, y], anchor(HAlign::Center, v), size, color);
            l.clip = Some(r);
            c.overlay.labels.push(l);
        };
        let mut y = r.y + pad;
        for (row, cands) in ROWS.iter().zip(&lines[k]) {
            if !rows[*row as usize] {
                continue;
            }
            let color = if *row == Row::Limit && !alarm {
                theme.text_dim
            } else {
                fg
            };
            if row.large() {
                if let Some(s) = cands.get(level[*row as usize].min(cands.len().max(1) - 1)) {
                    let size = large.min(inner / canvas::text_width(s, 1.0).max(1e-3));
                    if size >= READABLE {
                        // Centred in the row, so a shrunk wording keeps its place.
                        push(
                            s.clone(),
                            y + row_h(*row) / 2.0,
                            VAlign::Center,
                            size,
                            color,
                        );
                    }
                }
            } else if let Some(s) = fitting(cands, inner, small) {
                push(s, y, VAlign::Top, small, color);
            }
            y += row_h(*row);
        }
        if with_progress && let Some(s) = fitting(&progress[k], inner, progress_size) {
            push(s, track_bottom + pad, VAlign::Top, progress_size, fg);
        }
        push(
            names.0[k].clone(),
            r.bottom() - pad,
            VAlign::Bottom,
            name_size,
            fg,
        );
        out.push(LeqColumn {
            window: order[k],
            rect: r,
            background: bg,
            track,
            bar,
            bar_db,
            bar_color,
            track_color,
            limit_y,
            value,
            name: names.0[k].clone(),
            filling,
        });
    }
    LeqColumns {
        columns: out,
        range,
        gutter,
        weighting,
        large,
    }
}

/// A line left out to give the bar room.
#[derive(Clone, Copy)]
enum Shed {
    Row(Row),
    Progress,
}
