//! The Dynamics pane's scene: the caption, the Steps grid and the Table (the two views that
//! compare A with B), the readout and legend plates; Envelopes and Release are in `plots`.

use super::*;
use crate::banner::Status;
use crate::canvas::{self, Canvas, MARGINS, anchor, label};
use crate::distortion::{LegendEntry, LegendMark};
use crate::plate::PlateBox;
use crate::primitives::{
    FillPolygon, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport,
};
use crate::theme::Theme;
use crate::trace::TraceKey;
use crate::view::ViewState;

/// What the pane holds and where the pointer is.
#[derive(Clone, Debug)]
pub struct DynamicsFrame<'a> {
    pub curves: &'a [DynCurve<'a>],
    pub modes: DynamicsModes,
    /// The pointer over the pane, pane pixels.
    pub pointer: Option<[f32; 2]>,
    /// What the pane says when it holds no curve.
    pub missing: &'a str,
}

/// The pane as drawn, with everything the UI and the tests read back.
#[derive(Clone, Debug)]
pub struct DynamicsScene {
    pub scene: Scene,
    /// Above the plot: `VCA L · Run 1 − VCA R · Run 1 · GR end · −8 dBFS · 1 s`.
    pub caption: String,
    /// `programmes differ`, `rests differ`, or why nothing is drawn.
    pub note: Option<String>,
    /// Curves A and B (indices into the frame's curves).
    pub a: Option<usize>,
    pub b: Option<usize>,
    /// The pick in force.
    pub pick: Option<DynamicsStepPick>,
    /// Steps: each cell and the step it is, for a click.
    pub cells: Vec<(Rect, DynamicsStepPick)>,
    /// Steps: each cell's text, rows from the loudest level, as drawn.
    pub cell_texts: Vec<Vec<String>>,
    /// Table: its rows as drawn, the heading first.
    pub table: Vec<Vec<String>>,
    /// The readout plate's lines.
    pub readout: Vec<String>,
    pub readout_box: Option<PlateBox>,
    pub legend: Vec<LegendEntry>,
    pub legend_box: Option<crate::legend::LegendBox>,
    pub legend_hits: Vec<crate::legend::LegendHit>,
    /// The plots of Envelopes and Release (top first).
    pub plots: Vec<Rect>,
}

/// The step each curve shows of the pick (Envelopes fetches these), `None` where it has no
/// such step.
pub fn curve_steps(frame: &DynamicsFrame<'_>) -> Vec<Option<u32>> {
    let (a, _) = resolve(&frame.modes, &ids(frame.curves));
    let Some(p) = a.and_then(|a| pick(&frame.modes, frame.curves[a].info())) else {
        return vec![None; frame.curves.len()];
    };
    frame
        .curves
        .iter()
        .map(|c| step_of(c.info(), p).map(|s| s as u32))
        .collect()
}

pub(super) fn ids(curves: &[DynCurve<'_>]) -> Vec<CurveId> {
    curves.iter().map(|c| c.id).collect()
}

const CAPTION_H: f32 = 20.0;

pub fn dynamics_scene(
    frame: &DynamicsFrame<'_>,
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> DynamicsScene {
    let mut c = Canvas::new(size, theme);
    let right = MARGINS.right;
    let plot_w = (size.width - MARGINS.left - right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, MARGINS.left, plot_w, size, theme);
    let area = canvas::plot_area(size, strip.rect.bottom() + CAPTION_H, right);
    let style = crate::view::Style {
        chrome: view.chrome,
        theme,
    };
    let curves = frame.curves;
    let (a, b) = resolve(&frame.modes, &ids(curves));
    let pick = a.and_then(|a| pick(&frame.modes, curves[a].info()));
    let mut out = DynamicsScene {
        scene: Scene {
            viewport: size,
            layers: Vec::new(),
        },
        caption: String::new(),
        note: None,
        a,
        b,
        pick,
        cells: Vec::new(),
        cell_texts: Vec::new(),
        table: Vec::new(),
        readout: Vec::new(),
        readout_box: None,
        legend: Vec::new(),
        legend_box: None,
        legend_hits: Vec::new(),
        plots: Vec::new(),
    };
    let (Some(ai), Some(p)) = (a, pick) else {
        out.note = Some(frame.missing.to_string());
        c.base.labels.push(label(
            frame.missing,
            [area.x + area.w / 2.0, area.y + area.h / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            theme.font_size,
            theme.text_dim,
        ));
        out.scene = c.into_scene(size);
        return out;
    };
    // B compares only where the programmes match; Envelopes and Release draw every curve.
    let m = b.map(|b| programmes(curves[ai].info(), curves[b].info()));
    let compared = b.filter(|_| m != Some(Match::Differ));
    out.note = match m {
        Some(Match::Differ) => Some(PROGRAMMES_DIFFER.to_string()),
        Some(Match::RestsDiffer) => Some(RESTS_DIFFER.to_string()),
        _ => None,
    };
    let lines: Vec<(String, Color)> = match frame.modes.view {
        DynamicsView::Steps => {
            let scale = view.plates.readout.scale();
            steps(
                &mut c,
                &mut out,
                frame,
                (ai, compared),
                p,
                area,
                scale,
                theme,
            )
        }
        DynamicsView::Table => table(&mut c, &mut out, frame, ai, compared, area, theme),
        DynamicsView::Envelopes => super::plots::envelopes(&mut c, &mut out, frame, p, area, style),
        DynamicsView::Release => super::plots::release(&mut c, &mut out, frame, ai, p, area, style),
    };
    let sides = match compared {
        Some(b) => format!("{} − {}", curves[ai].name, curves[b].name),
        None => curves[ai].name.clone(),
    };
    let what = match frame.modes.view {
        DynamicsView::Steps => format!("{sides} · {}", metric_name(frame.modes.metric)),
        DynamicsView::Table => sides,
        DynamicsView::Envelopes if frame.modes.thd => "THD envelopes".to_string(),
        DynamicsView::Envelopes => "gain-reduction envelopes".to_string(),
        DynamicsView::Release if frame.modes.all_levels => {
            format!("{} · every level", curves[ai].name)
        }
        DynamicsView::Release => "release against burst length".to_string(),
    };
    out.caption = match &out.note {
        Some(n) => format!("{what} · {} · {n}", pick_text(p)),
        None => format!("{what} · {}", pick_text(p)),
    };
    c.base.labels.push(label(
        canvas::cut_to(&out.caption, area.w, theme.small_font_size),
        [area.x, area.y - CAPTION_H / 2.0 - 2.0],
        anchor(HAlign::Left, VAlign::Center),
        theme.small_font_size,
        theme.text,
    ));
    if !out.legend.is_empty() {
        let lg =
            crate::distortion::plates::legend(&mut c.overlay, &out.legend, view, area, area, theme);
        out.legend_box = lg.b;
        out.legend_hits = lg.hits;
    }
    if !lines.is_empty() {
        out.readout = lines.iter().map(|l| l.0.clone()).collect();
        // Steps: under the grid when the readout fits there, so it hides no cell.
        let below = out
            .cells
            .iter()
            .map(|(r, _)| r.bottom() + READOUT_GAP)
            .reduce(f32::max)
            .filter(|y| {
                let texts: Vec<&str> = lines.iter().map(|l| l.0.as_str()).collect();
                let m = crate::plate::Metrics::new(theme, view.plates.readout.scale());
                area.bottom() - y >= m.size(&texts).1 + m.pad_y
            });
        out.readout_box = Some(crate::distortion::plates::readout(
            &mut c.overlay,
            lines,
            area,
            below.unwrap_or(area.y + 4.0),
            view,
            theme,
        ));
    }
    out.scene = c.into_scene(size);
    out
}

/// A curve's legend row.
pub(super) fn legend_row(c: &DynCurve<'_>, name: String, color: Color) -> LegendEntry {
    LegendEntry {
        name,
        color,
        mark: LegendMark::Line,
        key: TraceKey::Stored(c.id.trace),
        selected: false,
    }
}

/// A − B (or A) of metric `m` at step `k`, as a cell shows it, and whether it is beyond the
/// tolerance (never where either curve's baseline is flagged).
fn cell(
    a: &DynCurve<'_>,
    b: Option<(&DynCurve<'_>, &[bool])>,
    a_flags: &[bool],
    k: usize,
    m: DynamicsMetric,
) -> (String, bool) {
    let Some(ra) = a.rows().get(k) else {
        return (NO_SUCH_STEP.to_string(), false);
    };
    let va = value(ra, m);
    let Some((b, b_flags)) = b else {
        return (value_text(va, m), false);
    };
    let Some(rb) = b.rows().get(k) else {
        return (NO_SUCH_STEP.to_string(), false);
    };
    if a_flags.get(k) == Some(&true) || b_flags.get(k) == Some(&true) {
        return (CANT_COMPARE.to_string(), false);
    }
    let period = f64::from(a.info().period) / a.info().sample_rate_hz.0;
    let d = diff(va, value(rb, m), period);
    (diff_text(d), matches!(d, Diff::Num(_, _, true)))
}

/// The readout of step `k`: the pick, A and B, every metric of both and their difference,
/// and each curve's baseline.
fn step_readout(
    a: &DynCurve<'_>,
    b: Option<&DynCurve<'_>>,
    k: usize,
    theme: &Theme,
) -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    let Some(s) = a.info().steps.get(k) else {
        return lines;
    };
    lines.push((
        format!(
            "{} · step {} of {}",
            pick_text(DynamicsStepPick {
                level: s.level,
                burst: s.burst
            }),
            k + 1,
            a.info().steps.len()
        ),
        theme.text,
    ));
    let ba = baselines(a.info(), a.rows());
    let fa = flagged(&ba);
    lines.push((format!("A {}", a.name), a.color));
    let bb = b.map(|b| baselines(b.info(), b.rows()));
    if let Some(b) = b {
        lines.push((format!("B {}", b.name), b.color));
    }
    for m in DynamicsMetric::ALL {
        let Some(ra) = a.rows().get(k) else { break };
        let va = value_text(value(ra, m), m);
        let line = match b.and_then(|b| Some((b, b.rows().get(k)?))) {
            Some((b, rb)) => {
                let fb = flagged(bb.as_deref().unwrap_or(&[]));
                let (d, _) = cell(a, Some((b, &fb)), &fa, k, m);
                format!(
                    "{}  {va} · {} · Δ {d}",
                    metric_name(m),
                    value_text(value(rb, m), m)
                )
            }
            None => format!("{}  {va}", metric_name(m)),
        };
        lines.push((line, theme.text));
    }
    if let Some(x) = ba.get(k) {
        lines.push((format!("A {}", baseline_text(*x)), theme.text_dim));
    }
    if let Some(x) = bb.as_ref().and_then(|v| v.get(k)) {
        lines.push((format!("B {}", baseline_text(*x)), theme.text_dim));
    }
    lines
}

const HEAD_W: f32 = 76.0;
const HEAD_H: f32 = 20.0;
const MIN_CELL_H: f32 = 24.0;
const MAX_CELL_H: f32 = 48.0;
const READOUT_GAP: f32 = 8.0;

#[allow(clippy::too_many_arguments)]
fn steps(
    c: &mut Canvas,
    out: &mut DynamicsScene,
    frame: &DynamicsFrame<'_>,
    (ai, bi): (usize, Option<usize>),
    p: DynamicsStepPick,
    area: Rect,
    readout_scale: f32,
    theme: &Theme,
) -> Vec<(String, Color)> {
    let a = &frame.curves[ai];
    let b = bi.map(|b| &frame.curves[b]);
    let info = a.info();
    let lv: Vec<f64> = levels(info).into_iter().rev().collect();
    let ln = lengths(info);
    if lv.is_empty() || ln.is_empty() {
        return Vec::new();
    }
    let fa = flagged(&baselines(info, a.rows()));
    let fb = b.map(|b| flagged(&baselines(b.info(), b.rows())));
    let grid = Rect::new(
        area.x + HEAD_W,
        area.y + HEAD_H,
        (area.w - HEAD_W).max(1.0),
        (area.h - HEAD_H).max(1.0),
    );
    let cw = grid.w / ln.len() as f32;
    // The cells leave room under them for the picked step's readout when rows of a
    // readable height still fit; otherwise the readout floats over them.
    let picked = step_of(info, p);
    let reserve = picked.map_or(0.0, |k| {
        let lines = step_readout(a, b, k, theme);
        let texts: Vec<&str> = lines.iter().map(|l| l.0.as_str()).collect();
        let m = crate::plate::Metrics::new(theme, readout_scale);
        m.size(&texts).1 + m.pad_y + READOUT_GAP
    });
    let n = lv.len() as f32;
    let ch = if (grid.h - reserve) / n >= MIN_CELL_H {
        ((grid.h - reserve) / n).min(MAX_CELL_H)
    } else {
        (grid.h / n).min(MAX_CELL_H)
    };
    c.base
        .rects
        .push(FillRect::new(grid, theme.plot_background));
    for (j, l) in ln.iter().enumerate() {
        c.base.labels.push(label(
            length_text(*l),
            [grid.x + (j as f32 + 0.5) * cw, area.y + HEAD_H / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            theme.small_font_size,
            theme.axis_text,
        ));
    }
    let mut hovered = None;
    for (i, l) in lv.iter().enumerate() {
        let y = grid.y + i as f32 * ch;
        c.base.labels.push(label(
            level_text(*l),
            [grid.x - 6.0, y + ch / 2.0],
            anchor(HAlign::Right, VAlign::Center),
            theme.small_font_size,
            theme.axis_text,
        ));
        let mut texts = Vec::new();
        for (j, d) in ln.iter().enumerate() {
            let r = Rect::new(grid.x + j as f32 * cw, y, cw, ch);
            let sp = DynamicsStepPick {
                level: ac2_proto::units::Dbfs(*l),
                burst: ac2_proto::units::Seconds(*d),
            };
            let k = step_of(info, sp);
            let (text, beyond) = match k {
                Some(k) => cell(a, b.zip(fb.as_deref()), &fa, k, frame.modes.metric),
                None => (NO_SUCH_STEP.to_string(), false),
            };
            if beyond {
                c.data
                    .rects
                    .push(FillRect::clipped(r, theme.unresolved_shade, area));
                c.data.polylines.push(Polyline::clipped(
                    outline(r, 2.0),
                    theme.unresolved_edge,
                    area,
                ));
            }
            if let Some(k) = k
                && (fa.get(k) == Some(&true) || fb.as_ref().and_then(|f| f.get(k)) == Some(&true))
            {
                hatch(c, r, theme);
            }
            if k.is_some() && k == picked {
                c.overlay
                    .polylines
                    .push(Polyline::clipped(outline(r, 1.0), theme.cursor, area));
            }
            if frame.pointer.is_some_and(|q| contains(r, q)) {
                hovered = k;
            }
            c.data.labels.push(
                label(
                    canvas::cut_to(&text, cw - 4.0, theme.small_font_size),
                    [r.x + r.w / 2.0, r.y + r.h / 2.0],
                    anchor(HAlign::Center, VAlign::Center),
                    theme.small_font_size,
                    theme.text,
                )
                .tabular(),
            );
            out.cells.push((r, sp));
            texts.push(text);
        }
        out.cell_texts.push(texts);
    }
    // Cell lines.
    for j in 0..=ln.len() {
        let x = grid.x + j as f32 * cw;
        c.base.polylines.push(Polyline::new(
            vec![[x, grid.y], [x, grid.y + ch * lv.len() as f32]],
            theme.grid_minor,
        ));
    }
    for i in 0..=lv.len() {
        let y = grid.y + i as f32 * ch;
        c.base.polylines.push(Polyline::new(
            vec![[grid.x, y], [grid.right(), y]],
            theme.grid_minor,
        ));
    }
    match hovered.or(picked) {
        Some(k) => step_readout(a, b, k, theme),
        None => vec![(NO_SUCH_STEP.to_string(), theme.text_dim)],
    }
}

pub(super) fn contains(r: Rect, p: [f32; 2]) -> bool {
    p[0] >= r.x && p[0] < r.right() && p[1] >= r.y && p[1] < r.bottom()
}

pub(super) fn outline(r: Rect, inset: f32) -> Vec<[f32; 2]> {
    let (x0, y0, x1, y1) = (
        r.x + inset,
        r.y + inset,
        r.right() - inset,
        r.bottom() - inset,
    );
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]
}

/// The hatched corner of a step whose baseline is flagged: a small triangle at the top right
/// with diagonal strokes, readable without colour.
fn hatch(c: &mut Canvas, r: Rect, theme: &Theme) {
    let s = (r.h * 0.45).min(14.0);
    let (x1, y0) = (r.right(), r.y);
    c.data.polygons.push(FillPolygon {
        points: vec![[x1 - s, y0], [x1, y0], [x1, y0 + s]],
        color: theme.unresolved_shade,
        clip: Some(r),
    });
    for k in 1..=3 {
        let d = s * k as f32 / 4.0;
        c.data.polylines.push(Polyline::clipped(
            vec![[x1 - d, y0], [x1, y0 + d]],
            Stroke::solid(theme.text_dim, 1.0),
            r,
        ));
    }
}

const ROW_H: f32 = 18.0;
/// `−20.0 dBFS · 100 ms` at the small font.
const STEP_COL_W: f32 = 120.0;
/// `A − B`.
const SIDE_COL_W: f32 = 44.0;

fn table(
    c: &mut Canvas,
    out: &mut DynamicsScene,
    frame: &DynamicsFrame<'_>,
    ai: usize,
    bi: Option<usize>,
    area: Rect,
    theme: &Theme,
) -> Vec<(String, Color)> {
    let a = &frame.curves[ai];
    let b = bi.map(|b| &frame.curves[b]);
    let info = a.info();
    let fa = flagged(&baselines(info, a.rows()));
    let fb = b.map(|b| flagged(&baselines(b.info(), b.rows())));
    let mut heading = vec!["step".to_string(), String::new()];
    heading.extend(
        DynamicsMetric::ALL
            .iter()
            .map(|m| metric_heading(*m).to_string()),
    );
    let mut rows = vec![(heading, theme.axis_text, None)];
    for (k, s) in info.steps.iter().enumerate() {
        let step = pick_text(DynamicsStepPick {
            level: s.level,
            burst: s.burst,
        });
        let values = |cv: &DynCurve<'_>| -> Vec<String> {
            DynamicsMetric::ALL
                .iter()
                .map(|m| {
                    cv.rows()
                        .get(k)
                        .map_or(NO_SUCH_STEP.to_string(), |r| value_text(value(r, *m), *m))
                })
                .collect()
        };
        let flag = fa.get(k) == Some(&true);
        let mut ra = vec![step.clone(), "A".to_string()];
        ra.extend(values(a));
        rows.push((ra, a.color, Some((k, flag))));
        if let Some(b) = b {
            let mut rb = vec![String::new(), "B".to_string()];
            rb.extend(values(b));
            rows.push((rb, b.color, None));
            let mut rd = vec![String::new(), "A − B".to_string()];
            rd.extend(
                DynamicsMetric::ALL
                    .iter()
                    .map(|m| cell(a, Some((b, fb.as_deref().unwrap_or(&[]))), &fa, k, *m).0),
            );
            rows.push((rd, theme.text, None));
        }
    }
    // The picked step's rows stay in view: the table starts so they fit.
    let fit = ((area.h / ROW_H) as usize).max(2);
    let per = if b.is_some() { 3 } else { 1 };
    let picked = out.pick.and_then(|p| step_of(info, p)).unwrap_or(0);
    let first = (picked * per + 1 + per).saturating_sub(fit - 1);
    // The step and side columns are as wide as their texts; the metrics share the rest.
    let cols = rows[0].0.len();
    let step_w = STEP_COL_W.min(area.w / 4.0);
    let side_w = SIDE_COL_W;
    let col_w = ((area.w - step_w - side_w) / (cols - 2) as f32).max(40.0);
    let x_of = |j: usize| match j {
        0 => (area.x, step_w),
        1 => (area.x + step_w, side_w),
        _ => (area.x + step_w + side_w + (j - 2) as f32 * col_w, col_w),
    };
    c.base
        .rects
        .push(FillRect::new(area, theme.plot_background));
    let shown = std::iter::once(&rows[0]).chain(rows.iter().skip(1 + first).take(fit - 1));
    for (n, (r, color, step)) in shown.enumerate() {
        let y = area.y + n as f32 * ROW_H;
        if let Some((k, flag)) = step {
            if *flag {
                hatch(c, Rect::new(area.x, y, step_w + side_w, ROW_H), theme);
            }
            if Some(*k) == out.pick.and_then(|p| step_of(info, p)) {
                c.base.rects.push(FillRect::clipped(
                    Rect::new(area.x, y, area.w, ROW_H * per as f32),
                    theme.unresolved_shade,
                    area,
                ));
            }
        }
        for (j, t) in r.iter().enumerate() {
            let (x, w) = x_of(j);
            let mut l = label(
                canvas::cut_to(t, w - 4.0, theme.small_font_size),
                [x + w - 4.0, y + ROW_H / 2.0],
                anchor(HAlign::Right, VAlign::Center),
                theme.small_font_size,
                if j < 2 { *color } else { theme.text },
            )
            .tabular();
            l.clip = Some(area);
            c.data.labels.push(l);
        }
    }
    out.table = rows.into_iter().map(|r| r.0).collect();
    Vec::new()
}
