//! The Programme pane as primitives: bars on the right with their readouts under them, the
//! goniometer's dark field and grid on the left with the correlation chip below it, the
//! loudness plate docked beside or above them or floating where the operator put it. The
//! embers themselves are `ac2-plot`'s (`embers::ember_points`), placed in the square this
//! returns.

use crate::canvas::{Canvas, anchor, cut_to, label, text_width};
use crate::gonio;
use crate::plate::{PlateBox, PlateKind, PlatePlace};
use crate::primitives::{
    Color, Dash, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport,
};
use crate::theme::Theme;
use crate::view::{PlotChrome, Style};

use super::bars::{LUFS_LOW, LUFS_SPAN, on_lufs_span};
use super::layout::ProgrammeLayout;
use super::{
    Ballistic, ProgrammeBar, ProgrammeInput, ProgrammePane, ScaleKind, SourceBar, bar_fill,
    loudness_plate, lufs_fill, scale_w,
};

/// The pane's picture, where its embers go, and its plate.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgrammeScene {
    pub scene: Scene,
    /// The goniometer's square, when the pane has room for one: the embers are drawn here.
    pub gonio: Option<Rect>,
    /// The loudness plate as drawn; `None` when hidden by **T** or not shown, so a plate
    /// that is not on screen cannot be grabbed.
    pub plate: Option<PlateBox>,
}

impl ProgrammeScene {
    /// The plates the operator moves, in drawing order (the last on top).
    pub fn plates(&self) -> Vec<PlateBox> {
        self.plate.into_iter().collect()
    }
}

/// The goniometer's field: dark in every theme, since embers add light and only read on
/// black.
const FIELD: Color = Color::rgb(0.02, 0.02, 0.025);
/// Dashes of the meter scale, dBFS.
const SCALE_DBFS: [f64; 7] = [0.0, -10.0, -20.0, -30.0, -40.0, -50.0, -60.0];
/// A bar this close to full scale reads in the warning colour.
const HOT_DBFS: f32 = -6.0;

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
    let l = p.layout(area, theme.small_font_size);
    let mut square = None;
    if let Some(g) = l.gonio {
        let sq = gonio::square(Rect::new(g.x + 4.0, g.y + 4.0, g.w - 8.0, g.h - 8.0));
        c.base.rects.push(FillRect {
            rect: sq,
            color: FIELD,
            clip: None,
        });
        if style.chrome == PlotChrome::Full {
            let stroke = Stroke::solid(theme.grid_major.color.with_alpha(0.6), 1.0);
            c.base.polylines.extend(gonio::grid_polylines(sq, stroke));
        }
        if style.chrome != PlotChrome::Bare {
            let s = theme.small_font_size;
            for (t, x, h) in [
                ("L", sq.x + 3.0, HAlign::Left),
                ("R", sq.right() - 3.0, HAlign::Right),
            ] {
                c.overlay.labels.push(label(
                    t,
                    [x, sq.y + 3.0],
                    anchor(h, VAlign::Top),
                    s,
                    theme.text_dim,
                ));
            }
        }
        square = Some(sq);
    }
    if let (Some(r), Some(pair)) = (l.correlation, &p.pair) {
        let chip = pair.correlation.chip();
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
        let room = r.w - crate::canvas::text_width(&chip.text, fs) - 16.0;
        if room >= crate::canvas::text_width(word, theme.small_font_size) {
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
    bars(&mut c, p, l.bars, style);
    // The plate goes at *traces only* wherever it sits (graphs hide their cursor values at
    // the same step); a docked one's room stays reserved, so nothing moves.
    let plate = (style.chrome != PlotChrome::Bare)
        .then(|| plate(&mut c, p, &l, area, theme))
        .flatten();
    ProgrammeScene {
        scene: c.into_scene(size),
        gonio: square,
        plate,
    }
}

/// The loudness plate, docked where [`super::dock`] put it or floating at the operator's
/// place in `area` (the pane's rect, which a floating plate's place is a fraction of).
fn plate(
    c: &mut Canvas,
    p: &ProgrammePane,
    l: &ProgrammeLayout,
    area: Rect,
    theme: &Theme,
) -> Option<PlateBox> {
    let r = p.loudness.as_ref()?;
    let font = theme.small_font_size;
    let (origin, scale, dropped) = match (p.plate.at, l.plate) {
        (None, Some(d)) => ([d.rect.x, d.rect.y], d.scale, d.dropped),
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
            ([at.x, at.y], s, dropped)
        }
    };
    let (rect, cells) = loudness_plate::cells(origin, font, scale, dropped);
    let hover = p.plate_hover.filter(|h| h.kind == PlateKind::Loudness);
    // Its own layer, the last: a layer's labels draw over its fills, so a plate floated
    // over the bars in theirs would show their scale and readouts through its fill.
    loudness_plate::draw(&mut c.banners, r, rect, &cells, hover, area, theme);
    Some(PlateBox::new(PlateKind::Loudness, area, rect, scale))
}

/// Dashes of the dBFS scale a peak bar carries beside LUFS bars: four numbers, narrow.
const OWN_SCALE_DBFS: [f64; 4] = [0.0, -20.0, -40.0, -60.0];

/// Text rows under a bar: its reading, the true peak where the reading is not already it,
/// the max, the flags, the note; a source bar's reading, max, true peak and flags.
fn rows(bar: &ProgrammeBar, ballistic: Ballistic) -> usize {
    match bar {
        ProgrammeBar::Peak { input, note } => {
            3 + usize::from(input.reads(ballistic) != Ballistic::TruePeak)
                + usize::from(note.is_some())
        }
        ProgrammeBar::Source(_) => 4,
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

/// Halfway from the level green to the text colour: lighter on a dark theme, darker on a
/// light one, so the part of a bar above the target reads as a tip of the same bar.
pub(crate) fn above_target(theme: &Theme) -> Color {
    let (a, b) = (theme.level_ok, theme.text);
    let m = |x: f32, y: f32| 0.5 * (x + y);
    Color::rgba(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b), a.a)
}

/// The bars side by side, each with its label over it and its readouts under it, and the
/// pane's scale left of the first: LUFS when a bar reads a source, dBFS otherwise.
fn bars(c: &mut Canvas, p: &ProgrammePane, area: Rect, style: Style) {
    let theme = style.theme;
    let n = p.bars.len().max(1);
    let fs = theme.font_size;
    let small = theme.small_font_size;
    let line = 1.3 * small;
    let kind = p.scale_kind();
    // The column keeps its width at every T step, its numbers gone at *traces only*, so
    // the bars never widen.
    let scale_w = scale_w(kind, small);
    let col_w = (area.w - scale_w) / n as f32;
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
        let ticks: Vec<(String, f32)> = match kind {
            ScaleKind::Dbfs => SCALE_DBFS
                .iter()
                .map(|&db| (crate::format::fixed(db, 0), y_of(bar_fill(Some(db as f32)))))
                .collect(),
            ScaleKind::Lufs => lufs
                .iter()
                .map(|&v| {
                    let n = crate::format::fixed(v, 0);
                    let t = if v == target && style.chrome == PlotChrome::Full {
                        format!("target {n}")
                    } else {
                        n
                    };
                    (t, y_of(lufs_fill(Some(v), target)))
                })
                .collect(),
        };
        for (t, y) in ticks {
            c.overlay.labels.push(label(
                t,
                [area.x + scale_w - 4.0, y],
                anchor(HAlign::Right, VAlign::Center),
                small,
                theme.text_dim,
            ));
        }
    }
    for (k, bar) in p.bars.iter().enumerate() {
        let x0 = area.x + scale_w + k as f32 * col_w;
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
        let block = text_width("max −120.0 dBTP", small).min(col_w - 4.0);
        let cols = Columns {
            xl: cx - block / 2.0,
            xr: cx + block / 2.0,
            block,
            value_w: block - text_width("max ", small),
            cx,
            col_right: x0 + col_w - 2.0,
            col_w,
            line,
            small,
        };
        let y = top + bar_h + 4.0;
        match bar {
            ProgrammeBar::Peak { input, note } => {
                if kind == ScaleKind::Lufs && style.chrome != PlotChrome::Bare {
                    // Its span is not the pane's: it carries its own numbers.
                    for db in OWN_SCALE_DBFS {
                        c.overlay.labels.push(label(
                            crate::format::fixed(db, 0),
                            [bx - 3.0, y_of(bar_fill(Some(db as f32)))],
                            anchor(HAlign::Right, VAlign::Center),
                            small,
                            theme.text_dim,
                        ));
                    }
                }
                peak_bar(c, input, bx, bw, &y_of, theme);
                peak_text(c, input, note.as_deref(), p.ballistic, &cols, y, theme);
            }
            ProgrammeBar::Source(s) => {
                source_bar(c, s, target, bx, bw, &y_of, theme);
                source_text(c, s, &cols, y, theme);
            }
        }
    }
}

/// A bar column's text places.
struct Columns {
    xl: f32,
    xr: f32,
    block: f32,
    value_w: f32,
    cx: f32,
    col_right: f32,
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
    k.put(
        c,
        "max".to_owned(),
        k.xl,
        HAlign::Left,
        theme.text_dim,
        y,
        false,
    );
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

/// The max-since-reset ticks' width, logical pixels.
pub(crate) const TICK_W: f32 = 4.0;
/// The target line's dash.
pub(crate) const TARGET_DASH: Dash = Dash {
    on: 4.0,
    off: 3.0,
    offset: 0.0,
};

fn source_text(c: &mut Canvas, s: &SourceBar, k: &Columns, mut y: f32, theme: &Theme) {
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
    for (word, value) in [
        ("max", s.max_value_text()),
        ("TP", s.true_peak_value_text()),
    ] {
        k.put(
            c,
            word.to_owned(),
            k.xl,
            HAlign::Left,
            theme.text_dim,
            y,
            false,
        );
        k.put(
            c,
            cut_to(&value, k.value_w, small),
            k.xr,
            HAlign::Right,
            theme.text_dim,
            y,
            true,
        );
        y += k.line;
    }
    let flag_color = Columns::flag_color(s.clipped(), theme);
    let flags = s.flags_text();
    if s.sides.len() == 1 {
        if !flags.is_empty() {
            let room = k.block - text_width("CLIP ", small);
            k.put(
                c,
                cut_to(&flags, room, small),
                k.xl,
                HAlign::Left,
                flag_color,
                y,
                false,
            );
        }
        if s.clipped() {
            k.put(
                c,
                "CLIP".to_owned(),
                k.xr,
                HAlign::Right,
                flag_color,
                y,
                false,
            );
        }
    } else if !flags.is_empty() {
        // Left-aligned at the readouts' left edge, so the row grows only to the right as
        // flags come.
        k.put(
            c,
            cut_to(&flags, k.col_right - k.xl, small),
            k.xl,
            HAlign::Left,
            flag_color,
            y,
            false,
        );
    }
}

fn bar_color(input: &ProgrammeInput, theme: &Theme) -> Color {
    match input.ppm {
        _ if input.clipped => theme.banner_fault.background,
        Some(v) if v >= HOT_DBFS => theme.banner_warning.background,
        _ => theme.level_ok,
    }
}
