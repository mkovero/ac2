//! The Programme pane as primitives: bars on the right with their readouts under them, the
//! goniometer's dark field and grid on the left with the correlation chip below it, the
//! loudness column at the right edge. The
//! embers themselves are `ac2-plot`'s (`embers::ember_points`), placed in the square this
//! returns.

use crate::canvas::{Canvas, anchor, cut_to, label, text_width};
use crate::gonio;
use crate::primitives::{Color, FillRect, HAlign, Rect, Scene, Stroke, VAlign, Viewport};
use crate::theme::Theme;
use crate::view::{PlotChrome, Style};

use super::loudness::LoudnessColumn;
use super::{Ballistic, ProgrammeInput, ProgrammePane, bar_fill};

/// The pane's picture and where its embers go.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgrammeScene {
    pub scene: Scene,
    /// The goniometer's square, when the pane has room for one: the embers are drawn here.
    pub gonio: Option<Rect>,
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
        };
    }
    let l = p.layout(area);
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
    if let (Some(r), Some(col)) = (l.loudness, &p.loudness) {
        loudness(&mut c, col, r, style);
    }
    ProgrammeScene {
        scene: c.into_scene(size),
        gonio: square,
    }
}

/// The bars side by side, each with its label over it and its readouts under it, and the
/// dBFS scale left of the first.
fn bars(c: &mut Canvas, p: &ProgrammePane, area: Rect, style: Style) {
    let theme = style.theme;
    let n = p.inputs.len().max(1);
    let fs = theme.font_size;
    let small = theme.small_font_size;
    let line = 1.3 * small;
    let scale_w = if style.chrome == PlotChrome::Bare {
        0.0
    } else {
        3.0 * small
    };
    let col_w = (area.w - scale_w) / n as f32;
    // Under each bar: level, true peak, max, overs / CLIP, and the VU note when it applies.
    let notes = p.inputs.iter().any(|i| i.note(p.ballistic).is_some());
    let under = (if notes { 5.0 } else { 4.0 }) * line + 4.0;
    let top = area.y + 1.4 * fs;
    let bar_h = (area.bottom() - under - top).max(8.0);
    let y_of = |fill: f32| top + bar_h * (1.0 - fill);
    if scale_w > 0.0 {
        for db in SCALE_DBFS {
            let y = y_of(bar_fill(Some(db as f32)));
            c.overlay.labels.push(label(
                crate::format::fixed(db, 0),
                [area.x + scale_w - 4.0, y],
                anchor(HAlign::Right, VAlign::Center),
                small,
                theme.text_dim,
            ));
        }
    }
    for (k, input) in p.inputs.iter().enumerate() {
        let x0 = area.x + scale_w + k as f32 * col_w;
        let bw = (col_w * 0.45).clamp(6.0, 40.0);
        let bx = x0 + (col_w - bw) / 2.0;
        let cx = x0 + col_w / 2.0;
        c.overlay.labels.push(label(
            cut_to(&input.label, col_w - 4.0, fs),
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
        if style.chrome == PlotChrome::Full {
            for db in SCALE_DBFS {
                let y = y_of(bar_fill(Some(db as f32)));
                c.base.polylines.push(crate::primitives::Polyline {
                    points: vec![[bx, y], [bx + bw, y]],
                    alpha: vec![],
                    stroke: theme.grid_minor,
                    clip: None,
                });
            }
        }
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
        // Every readout sits at a place fixed by the column, never by its own text: the
        // values right-aligned at one edge (the unit stays put, the digits grow left), `max`
        // and the overs count from the other, CLIP at the right. A centred readout would
        // shift with every digit the value gains or loses.
        let unit = input.unit(p.ballistic);
        let block = text_width(&format!("max −120.0 {unit}"), small).min(col_w - 4.0);
        let (xl, xr) = (cx - block / 2.0, cx + block / 2.0);
        let value_w = block - text_width("max ", small);
        let mut y = top + bar_h + 4.0;
        let mut put = |t: String, x: f32, h: HAlign, color: Color, y: f32| {
            c.overlay
                .labels
                .push(label(t, [x, y], anchor(h, VAlign::Top), small, color));
        };
        put(
            cut_to(&input.ppm_text(p.ballistic), value_w, small),
            xr,
            HAlign::Right,
            theme.text,
            y,
        );
        y += line;
        // A true-peak bar already reads the dBTP number; a second line would repeat it.
        if input.reads(p.ballistic) != Ballistic::TruePeak {
            put(
                cut_to(&input.true_peak_text(), value_w, small),
                xr,
                HAlign::Right,
                theme.text_dim,
                y,
            );
            y += line;
        }
        put("max".to_owned(), xl, HAlign::Left, theme.text_dim, y);
        let max = input.max_text(p.ballistic);
        let max = max.strip_prefix("max ").unwrap_or(&max);
        put(
            cut_to(max, value_w, small),
            xr,
            HAlign::Right,
            theme.text_dim,
            y,
        );
        y += line;
        let flag_color = if input.clipped {
            theme.banner_fault.background
        } else {
            theme.banner_warning.background
        };
        let overs = input.overs_text();
        if !overs.is_empty() {
            let room = block - text_width("CLIP ", small);
            put(cut_to(&overs, room, small), xl, HAlign::Left, flag_color, y);
        }
        if input.clipped {
            put(
                input.clip_text().to_owned(),
                xr,
                HAlign::Right,
                flag_color,
                y,
            );
        }
        y += line;
        if let Some(n) = input.note(p.ballistic) {
            put(
                cut_to(n, col_w - 4.0, small),
                cx,
                HAlign::Center,
                theme.text_dim,
                y,
            );
        }
    }
}

/// The loudness column: its source's name level with the bar labels, the readouts from the
/// bars' top down, the since line at the foot (chrome, so **T** hides it with the rest); or
/// why there is none. Lines too long for the column wrap.
fn loudness(c: &mut Canvas, col: &LoudnessColumn, r: Rect, style: Style) {
    let theme = style.theme;
    let (fs, small) = (theme.font_size, theme.small_font_size);
    let x = r.x + 4.0;
    let w = (r.w - 6.0).max(1.0);
    let put = |c: &mut Canvas, text: &str, y: &mut f32, size: f32, color: Color| {
        for line in crate::toast::wrap(text, w, &|s| crate::canvas::text_width(s, size)) {
            c.overlay.labels.push(label(
                line,
                [x, *y],
                anchor(HAlign::Left, VAlign::Top),
                size,
                color,
            ));
            *y += 1.3 * size;
        }
    };
    let mut y = r.y + 1.4 * fs;
    match col {
        LoudnessColumn::Message(m) => put(c, m, &mut y, small, theme.text_dim),
        LoudnessColumn::Readout(l) => {
            let mut top = r.y;
            put(c, &l.title, &mut top, fs, theme.text);
            y = y.max(top);
            let [mom, short, int, lra, tp] = l.lines();
            for t in [mom, short, int] {
                put(c, &t, &mut y, fs, theme.text);
            }
            for t in [lra, tp] {
                put(c, &t, &mut y, small, theme.text_dim);
            }
            if style.chrome != PlotChrome::Bare {
                let mut foot = (r.bottom() - 1.3 * small).max(y);
                put(c, &l.since, &mut foot, small, theme.text_dim);
            }
        }
    }
}

fn bar_color(input: &ProgrammeInput, theme: &Theme) -> Color {
    match input.ppm {
        _ if input.clipped => theme.banner_fault.background,
        Some(v) if v >= HOT_DBFS => theme.banner_warning.background,
        _ => theme.level_ok,
    }
}
