//! The Meters view's bars: the true-peak block and the LUFS block, each after its own scale
//! column, with their readouts under them (docs/design/programme-meter.md §6.2, §6.3).

use crate::canvas::{Canvas, anchor, cut_to, label, text_width};
use crate::meter::HOT_PEAK_DBFS;
use crate::primitives::{
    Band, BandPoint, Color, Dash, FillRect, HAlign, Polyline, Rect, Stroke, VAlign,
};
use crate::theme::Theme;
use crate::view::{PlotChrome, Style};

use super::bars::{LUFS_LOW, LUFS_SPAN, on_lufs_span};
use super::{
    Ballistic, ProgrammeBar, ProgrammeInput, ProgrammePane, Scale, ScaleKind, SourceBar, bar_fill,
    floor_text, lufs_fill, scale_w,
};

/// Dashes of the meter scale, dBFS.
const SCALE_DBFS: [f64; 7] = [0.0, -10.0, -20.0, -30.0, -40.0, -50.0, -60.0];

/// Text rows under a bar: its reading, the true peak where the reading is not already it,
/// the max, the flags, the note; a source bar's reading, max and floor row.
fn rows(bar: &ProgrammeBar, ballistic: Ballistic) -> usize {
    match bar {
        ProgrammeBar::Peak { input, note } => {
            3 + usize::from(input.reads(ballistic) != Ballistic::TruePeak)
                + usize::from(note.is_some())
        }
        // Its reading, its max, and the floor row, kept whatever the reading so no bar
        // moves when the mark comes and goes.
        ProgrammeBar::Source(_) => 3,
    }
}

/// The LUFS scale's ticks for a bar `bar_h` high: every 3 LU when the bar is at least ten
/// label rows high, else every 9 LU; the target's always. The rule follows the bar's
/// height only, so labels never come and go with values.
fn lufs_ticks(target: f64, bar_h: f32, small: f32) -> Vec<f64> {
    let step = if bar_h >= 10.0 * 1.3 * small {
        3.0
    } else {
        9.0
    };
    let n = (LUFS_SPAN / step).round() as usize;
    (0..=n)
        .map(|k| target + LUFS_LOW + k as f64 * step)
        .collect()
}

/// The peak scale's ticks for a bar `bar_h` high: every 10 dB when the bar has a label row
/// per tick, else every 30 dB, so a short pane's labels never overprint; by height only.
fn peak_ticks(bar_h: f32, small: f32) -> impl Iterator<Item = &'static f64> {
    let gaps = (SCALE_DBFS.len() - 1) as f32;
    let every = if bar_h >= gaps * 1.3 * small { 1 } else { 3 };
    SCALE_DBFS.iter().step_by(every)
}

/// Halfway from the level green to the text colour: lighter on a dark theme, darker on a
/// light one, so the part of a bar above the target reads as a tip of the same bar.
pub(crate) fn above_target(theme: &Theme) -> Color {
    let (a, b) = (theme.level_ok, theme.text);
    let m = |x: f32, y: f32| 0.5 * (x + y);
    Color::rgba(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b), a.a)
}

/// The bars side by side, each with its label over it and its readouts under it: the
/// true-peak block after its dBFS scale column, then the LUFS block after its LUFS column
/// (one block, dBFS, under a peak ballistic). Under a loudness ballistic each column has
/// its unit at its head: `dBTP` while every bar of the block reads a digital true peak
/// (a calibrated bar's own text carries its unit), `LUFS`.
pub(super) fn bars(c: &mut Canvas, p: &ProgrammePane, area: Rect, style: Style) {
    let theme = style.theme;
    let n = p.bars.len().max(1);
    let fs = theme.font_size;
    let small = theme.small_font_size;
    let line = 1.3 * small;
    // The columns keep their width at every T step, their numbers gone at *traces only*,
    // so the bars never widen.
    let peak_col = if p.has_peak_bar() || !p.has_source_bar() {
        scale_w(ScaleKind::Dbfs, small)
    } else {
        0.0
    };
    let lufs_col = if p.has_source_bar() {
        scale_w(ScaleKind::Lufs, small)
    } else {
        0.0
    };
    let col_w = (area.w - peak_col - lufs_col) / n as f32;
    let n_peak = p
        .bars
        .iter()
        .filter(|b| matches!(b, ProgrammeBar::Peak { .. }))
        .count();
    let lufs_x = area.x + peak_col + n_peak as f32 * col_w;
    let rows = p
        .bars
        .iter()
        .map(|b| rows(b, p.ballistic))
        .fold(4, usize::max);
    let under = rows as f32 * line + 4.0;
    let top = area.y + 1.4 * fs;
    let bar_h = (area.bottom() - under - top).max(8.0);
    let y_of = |fill: f32| top + bar_h * (1.0 - fill);
    let target = p.loudness_target;
    let lufs = lufs_ticks(target, bar_h, small);
    if style.chrome != PlotChrome::Bare {
        let mut ticks: Vec<(String, f32, f32)> = Vec::new();
        if peak_col > 0.0 {
            let x = area.x + peak_col - 4.0;
            ticks.extend(peak_ticks(bar_h, small).map(|&db| {
                (
                    crate::format::fixed(db, 0),
                    x,
                    y_of(bar_fill(Some(db as f32))),
                )
            }));
        }
        if lufs_col > 0.0 {
            let x = lufs_x + lufs_col - 4.0;
            ticks.extend(lufs.iter().map(|&v| {
                let n = crate::format::fixed(v, 0);
                let t = if v == target && style.chrome == PlotChrome::Full {
                    format!("target {n}")
                } else {
                    n
                };
                (t, x, y_of(lufs_fill(Some(v), target)))
            }));
        }
        if p.ballistic.is_loudness() {
            let digital = p.bars.iter().all(|b| match b {
                ProgrammeBar::Peak { input, .. } => input.scale == Scale::Dbfs,
                ProgrammeBar::Source(_) => true,
            });
            if peak_col > 0.0 && digital {
                ticks.push(("dBTP".into(), area.x + peak_col - 4.0, area.y));
            }
            if lufs_col > 0.0 {
                ticks.push(("LUFS".into(), lufs_x + lufs_col - 4.0, area.y));
            }
        }
        for (t, x, y) in ticks {
            let v = if y == area.y {
                VAlign::Top
            } else {
                VAlign::Center
            };
            c.overlay.labels.push(label(
                t,
                [x, y],
                anchor(HAlign::Right, v),
                small,
                theme.text_dim,
            ));
        }
    }
    for (k, bar) in p.bars.iter().enumerate() {
        let x0 = if k < n_peak {
            area.x + peak_col + k as f32 * col_w
        } else {
            lufs_x + lufs_col + (k - n_peak) as f32 * col_w
        };
        let bw = (col_w * 0.45).clamp(6.0, 40.0);
        let bx = x0 + (col_w - bw) / 2.0;
        let cx = x0 + col_w / 2.0;
        let name = match bar {
            ProgrammeBar::Peak { input, .. } => &input.label,
            ProgrammeBar::Source(s) => &s.name,
        };
        c.overlay.labels.push(label(
            cut_to(name, col_w - 4.0, fs),
            [cx, area.y],
            anchor(HAlign::Center, VAlign::Top),
            fs,
            theme.text,
        ));
        c.base.rects.push(FillRect {
            rect: Rect::new(bx, top, bw, bar_h),
            color: theme.plot_background,
            clip: None,
        });
        let grid: Vec<f32> = match bar {
            ProgrammeBar::Peak { .. } => SCALE_DBFS
                .iter()
                .map(|&db| y_of(bar_fill(Some(db as f32))))
                .collect(),
            ProgrammeBar::Source(_) => lufs
                .iter()
                .map(|&v| y_of(lufs_fill(Some(v), target)))
                .collect(),
        };
        if style.chrome == PlotChrome::Full {
            for y in grid {
                c.base.polylines.push(Polyline {
                    points: vec![[bx, y], [bx + bw, y]],
                    alpha: vec![],
                    stroke: theme.grid_minor,
                    clip: None,
                });
            }
        }
        // Every readout sits at a place fixed by the column, never by its own text: the
        // values right-aligned at one edge (the unit stays put, the digits grow left), the
        // words and the flags from the other, CLIP at the right. A centred readout would
        // shift with every digit the value gains or loses.
        //
        // Neighbouring columns face each other with a right-aligned value and a left-aligned
        // word; squeezed together they read as one line ("−11.8 dBTP max"). A squeezed block
        // writes its row words once, in its first column, as a table writes a row heading.
        let full = text_width("max −120.0 dBTP", small);
        let squeezed = full + text_width(GUTTER, small) > col_w;
        let word = !squeezed || k == 0 || k == n_peak;
        let block = full.min(col_w - 4.0);
        let cols = Columns {
            xl: cx - block / 2.0,
            xr: cx + block / 2.0,
            block,
            value_w: if word {
                block - text_width("max ", small)
            } else {
                block
            },
            word,
            cx,
            col_w,
            line,
            small,
        };
        let y = top + bar_h + 4.0;
        match bar {
            ProgrammeBar::Peak { input, note } => {
                peak_bar(c, input, bx, bw, &y_of, theme);
                peak_text(c, input, note.as_deref(), p.ballistic, &cols, y, theme);
            }
            ProgrammeBar::Source(s) => {
                source_bar(c, s, target, bx, bw, &y_of, theme);
                source_text(c, s, target, &cols, y, theme);
            }
        }
    }
}

/// The least room between two neighbouring columns' readouts.
const GUTTER: &str = "m";

/// A bar column's text places.
struct Columns {
    xl: f32,
    xr: f32,
    block: f32,
    value_w: f32,
    /// This column writes the row words ("max"); a squeezed block's later columns leave
    /// them to its first.
    word: bool,
    cx: f32,
    col_w: f32,
    line: f32,
    small: f32,
}

impl Columns {
    #[allow(clippy::too_many_arguments)]
    fn put(&self, c: &mut Canvas, t: String, x: f32, h: HAlign, color: Color, y: f32, num: bool) {
        let l = label(t, [x, y], anchor(h, VAlign::Top), self.small, color);
        c.overlay.labels.push(if num { l.tabular() } else { l });
    }

    fn flag_color(clipped: bool, theme: &Theme) -> Color {
        if clipped {
            theme.banner_fault.background
        } else {
            theme.banner_warning.background
        }
    }
}

fn peak_bar(
    c: &mut Canvas,
    input: &ProgrammeInput,
    bx: f32,
    bw: f32,
    y_of: &dyn Fn(f32) -> f32,
    theme: &Theme,
) {
    let bar_h = y_of(0.0) - y_of(1.0);
    let fill = input.bar_fill();
    if fill > 0.0 {
        c.data.rects.push(FillRect {
            rect: Rect::new(bx, y_of(fill), bw, bar_h * fill),
            color: bar_color(input, theme),
            clip: None,
        });
    }
    if input.flying_peak.is_some() {
        let y = y_of(input.flying_peak_fill());
        c.data.rects.push(FillRect {
            rect: Rect::new(bx, y - 1.0, bw, 2.0),
            color: theme.text,
            clip: None,
        });
    }
}

fn peak_text(
    c: &mut Canvas,
    input: &ProgrammeInput,
    note: Option<&str>,
    ballistic: Ballistic,
    k: &Columns,
    mut y: f32,
    theme: &Theme,
) {
    let small = k.small;
    k.put(
        c,
        cut_to(&input.ppm_text(ballistic), k.value_w, small),
        k.xr,
        HAlign::Right,
        theme.text,
        y,
        true,
    );
    y += k.line;
    // A true-peak bar already reads the dBTP number; a second line would repeat it.
    if input.reads(ballistic) != Ballistic::TruePeak {
        k.put(
            c,
            cut_to(&input.true_peak_text(), k.value_w, small),
            k.xr,
            HAlign::Right,
            theme.text_dim,
            y,
            true,
        );
        y += k.line;
    }
    if k.word {
        k.put(
            c,
            "max".to_owned(),
            k.xl,
            HAlign::Left,
            theme.text_dim,
            y,
            false,
        );
    }
    let max = input.max_text(ballistic);
    let max = max.strip_prefix("max ").unwrap_or(&max);
    k.put(
        c,
        cut_to(max, k.value_w, small),
        k.xr,
        HAlign::Right,
        theme.text_dim,
        y,
        true,
    );
    y += k.line;
    let flag_color = Columns::flag_color(input.clipped, theme);
    let overs = input.overs_text();
    if !overs.is_empty() {
        let room = k.block - text_width("CLIP ", small);
        k.put(
            c,
            cut_to(&overs, room, small),
            k.xl,
            HAlign::Left,
            flag_color,
            y,
            false,
        );
    }
    if input.clipped {
        k.put(
            c,
            input.clip_text().to_owned(),
            k.xr,
            HAlign::Right,
            flag_color,
            y,
            false,
        );
    }
    y += k.line;
    if let Some(n) = note {
        k.put(
            c,
            cut_to(n, k.col_w - 4.0, small),
            k.cx,
            HAlign::Center,
            theme.text_dim,
            y,
            false,
        );
    }
}

/// A source's bar: green below the target, the lighter green above it (zones by position,
/// never yellow or red: a programme on target is above it half the time, which is no
/// fault), the dashed target line (a setting; the dash tells it from a reading), and the
/// max since the reset as a pair of ticks outside both edges. No flying peak: Tech 3341
/// defines a maximum for M and S and no falling hold.
#[allow(clippy::too_many_arguments)]
fn source_bar(
    c: &mut Canvas,
    s: &SourceBar,
    target: f64,
    bx: f32,
    bw: f32,
    y_of: &dyn Fn(f32) -> f32,
    theme: &Theme,
) {
    let bar_h = y_of(0.0) - y_of(1.0);
    let fill = lufs_fill(s.reading, target);
    let at_target = lufs_fill(Some(target), target);
    let low = fill.min(at_target);
    if low > 0.0 {
        c.data.rects.push(FillRect {
            rect: Rect::new(bx, y_of(low), bw, bar_h * low),
            color: theme.level_ok,
            clip: None,
        });
    }
    if fill > at_target {
        c.data.rects.push(FillRect {
            rect: Rect::new(bx, y_of(fill), bw, bar_h * (fill - at_target)),
            color: above_target(theme),
            clip: None,
        });
    }
    let ty = y_of(at_target);
    c.data.polylines.push(Polyline {
        points: vec![[bx - TICK_W - 1.0, ty], [bx + bw + TICK_W + 1.0, ty]],
        alpha: vec![],
        stroke: Stroke {
            color: theme.text,
            width: 1.0,
            dash: Some(TARGET_DASH),
        },
        clip: None,
    });
    if s.below_floor(target) {
        // Pointing down at the foot of the empty bar: the reading lies under it.
        let (cx, foot) = (bx + bw / 2.0, y_of(0.0) - 2.0);
        let half = (FLOOR_MARK_W / 2.0).min(bw / 2.0);
        let top = foot - 0.75 * FLOOR_MARK_W;
        c.data.bands.push(Band {
            points: vec![
                BandPoint {
                    x: cx - half,
                    y0: top,
                    y1: top,
                },
                BandPoint {
                    x: cx,
                    y0: top,
                    y1: foot,
                },
                BandPoint {
                    x: cx + half,
                    y0: top,
                    y1: top,
                },
            ],
            color: theme.text_dim,
            clip: None,
        });
    }
    if let Some(m) = s.max.filter(|&m| on_lufs_span(m, target)) {
        let y = y_of(lufs_fill(Some(m), target));
        for x in [bx - TICK_W - 1.0, bx + bw + 1.0] {
            c.data.rects.push(FillRect {
                rect: Rect::new(x, y - 1.0, TICK_W, 2.0),
                color: theme.text_dim,
                clip: None,
            });
        }
    }
}

/// The below-the-floor mark's width, logical pixels.
pub(crate) const FLOOR_MARK_W: f32 = 8.0;
/// The max-since-reset ticks' width, logical pixels.
pub(crate) const TICK_W: f32 = 4.0;
/// The target line's dash.
pub(crate) const TARGET_DASH: Dash = Dash {
    on: 4.0,
    off: 3.0,
    offset: 0.0,
};

/// A source's reading, its max, and under the floor the floor's word.
fn source_text(c: &mut Canvas, s: &SourceBar, target: f64, k: &Columns, mut y: f32, theme: &Theme) {
    let small = k.small;
    k.put(
        c,
        cut_to(&s.reading_text(), k.value_w, small),
        k.xr,
        HAlign::Right,
        theme.text,
        y,
        true,
    );
    y += k.line;
    if k.word {
        k.put(
            c,
            "max".to_owned(),
            k.xl,
            HAlign::Left,
            theme.text_dim,
            y,
            false,
        );
    }
    k.put(
        c,
        cut_to(&s.max_value_text(), k.value_w, small),
        k.xr,
        HAlign::Right,
        theme.text_dim,
        y,
        true,
    );
    y += k.line;
    if s.below_floor(target) {
        k.put(
            c,
            cut_to(&floor_text(target), k.col_w - 4.0, small),
            k.xr,
            HAlign::Right,
            theme.text_dim,
            y,
            true,
        );
    }
}

fn bar_color(input: &ProgrammeInput, theme: &Theme) -> Color {
    match input.ppm {
        _ if input.clipped => theme.banner_fault.background,
        Some(v) if f64::from(v) >= HOT_PEAK_DBFS => theme.banner_warning.background,
        _ => theme.level_ok,
    }
}

#[cfg(test)]
mod tick_tests {
    use super::peak_ticks;

    #[test]
    fn a_short_bar_thins_its_peak_labels() {
        let tall: Vec<f64> = peak_ticks(200.0, 10.0).copied().collect();
        assert_eq!(tall.len(), 7);
        let short: Vec<f64> = peak_ticks(40.0, 10.0).copied().collect();
        assert_eq!(short, [0.0, -30.0, -60.0]);
    }
}
