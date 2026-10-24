//! The Programme pane as primitives: bars on the right with their readouts under them, the
//! goniometer's dark field and grid on the left with the correlation chip below it. The
//! embers themselves are `ac2-plot`'s (`embers::ember_points`), placed in the square this
//! returns.

use crate::canvas::{Canvas, anchor, cut_to, label};
use crate::gonio;
use crate::primitives::{Color, FillRect, HAlign, Rect, Scene, Stroke, VAlign, Viewport};
use crate::theme::Theme;
use crate::view::{PlotChrome, Style};

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
        let mut rows: Vec<(String, Color)> = vec![(input.ppm_text(p.ballistic), theme.text)];
        // A true-peak bar already reads the dBTP number; a second line would repeat it.
        if input.reads(p.ballistic) != Ballistic::TruePeak {
            rows.push((input.true_peak_text(), theme.text_dim));
        }
        rows.push((input.max_text(p.ballistic), theme.text_dim));
        let flags = [input.overs_text(), input.clip_text().to_owned()]
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let flag_color = if input.clipped {
            theme.banner_fault.background
        } else {
            theme.banner_warning.background
        };
        rows.push((flags, flag_color));
        if let Some(n) = input.note(p.ballistic) {
            rows.push((n.to_owned(), theme.text_dim));
        }
        let mut y = top + bar_h + 4.0;
        for (t, color) in rows {
            if !t.is_empty() {
                c.overlay.labels.push(label(
                    cut_to(&t, col_w - 4.0, small),
                    [cx, y],
                    anchor(HAlign::Center, VAlign::Top),
                    small,
                    color,
                ));
            }
            y += line;
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
