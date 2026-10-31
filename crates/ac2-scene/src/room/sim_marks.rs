//! Simulated reflections over a measured impulse response (`docs/design/spatial-walk.md`
//! §7): where a simulation predicts each specular arrival at the point a sweep was captured,
//! drawn as faint dashed lines named by the surfaces the sound bounced off, in plan terms
//! (*back wall*, *wall 3 → ceiling*). The simulated direct sound is put on the measured one:
//! the measured time base carries the system's latency and the speaker's own delay, the
//! simulation neither, so only the times after the direct sound compare.

use ac2_proto::model::{PlanSpec, SimState, SimStatus, Surface, TracePosition};
use ac2_room::plan::{WallSide, wall_side};

use super::sim;
use crate::canvas::{anchor, label, text_width};
use crate::ir::IrScene;
use crate::primitives::{Dash, FillRect, HAlign, Polyline, Rect, Stroke, VAlign};
use crate::theme::Theme;

/// Reflections weaker than this, dB re the direct sound, are not marked: they are lost
/// under the measured decay and would only crowd the plot.
pub const MARK_FLOOR_DB: f64 = -30.0;
/// Reflections at least this strong, dB re the direct sound, are named on the plot.
pub const LABEL_FLOOR_DB: f64 = -20.0;
/// Rows the reflection names are stacked in, so neighbours a few ms apart both read.
const LABEL_ROWS: usize = 3;

/// The plan's name for a surface, as someone facing the stage names it: *floor*, *ceiling*,
/// *back wall*; a wall whose side another wall shares, or a plan without a FOH or speaker
/// heading, by its number (*wall 3*, 1-based as the vertex table counts edges), the side
/// after it when known (*wall 3, left*).
pub fn surface_name(plan: Option<&PlanSpec>, s: Surface) -> String {
    let edge = match s {
        Surface::Floor => return "floor".to_owned(),
        Surface::Ceiling => return "ceiling".to_owned(),
        Surface::Wall { edge } => edge as usize,
    };
    let numbered = format!("wall {}", edge + 1);
    let Some(plan) = plan else {
        return numbered;
    };
    let Some(side) = wall_side(plan, edge) else {
        return numbered;
    };
    let word = match side {
        WallSide::Stage => "stage",
        WallSide::Back => "back",
        WallSide::Left => "left",
        WallSide::Right => "right",
    };
    let shared = (0..plan.outline.len())
        .filter(|&e| e != edge && wall_side(plan, e) == Some(side))
        .count()
        > 0;
    if shared {
        format!("{numbered}, {word}")
    } else {
        format!("{word} wall")
    }
}

/// A path's surfaces in order, *wall 3 → ceiling*; *direct sound* for none.
pub fn path_name(plan: Option<&PlanSpec>, path: &[Surface]) -> String {
    if path.is_empty() {
        return "direct sound".to_owned();
    }
    path.iter()
        .map(|&s| surface_name(plan, s))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// One simulated reflection on the measured time axis.
#[derive(Clone, Debug, PartialEq)]
pub struct SimMark {
    /// Milliseconds after the measured direct sound.
    pub t_ms: f64,
    /// Broadband energy, dB re the simulated direct sound.
    pub level_db: f64,
    /// [`path_name`].
    pub name: String,
}

/// What the overlay draws for one trace.
#[derive(Clone, Debug, PartialEq)]
pub struct SimMarks {
    /// The reflections at or above [`MARK_FLOOR_DB`], by time.
    pub marks: Vec<SimMark>,
    /// *simulated · aligned on direct sound · absorption assumed · Main L*.
    pub legend: String,
}

/// The receiver of `s` a trace captured at `pos` is heard at: a finished simulation of the
/// trace's plan whose receivers are its walk's points, or a receiver of that label within
/// 5 cm of it. The first source (a fit's only one).
fn receiver<'a>(
    s: &'a SimState,
    pos: &TracePosition,
) -> Option<(&'a str, &'a ac2_proto::model::SimReceiver)> {
    if s.status != SimStatus::Done || s.plan != pos.plan || s.map.is_some() {
        return None;
    }
    let src = s.sources.first()?;
    let near = |a: [f64; 3]| (0..3).map(|k| (a[k] - pos.at[k]).powi(2)).sum::<f64>() < 0.0025;
    let r = src
        .receivers
        .iter()
        .find(|r| r.label == pos.label && (s.walk == Some(pos.walk) || near(r.at)))?;
    Some((src.name.as_str(), r))
}

/// The reflections `s` predicts at the point a trace was captured at (`pos`), aligned so
/// the simulated direct sound sits at time zero, the measured arrival; `None` when `s` is
/// not a finished simulation heard there.
pub fn reflection_marks(
    s: &SimState,
    plan: Option<&PlanSpec>,
    pos: &TracePosition,
) -> Option<SimMarks> {
    let (source, r) = receiver(s, pos)?;
    let broadband = |e: &[f32]| e.iter().map(|&v| f64::from(v)).sum::<f64>();
    // A direct sound the room hides (round a corner) leaves the first arrival as the one
    // the measurement's time zero found.
    let direct = r
        .arrivals
        .iter()
        .find(|a| a.path.is_empty())
        .or_else(|| r.arrivals.first())?;
    let (t0, e0) = (direct.time.0, broadband(&direct.energy));
    if e0 <= 0.0 {
        return None;
    }
    let marks = r
        .arrivals
        .iter()
        .filter(|a| !std::ptr::eq(*a, direct))
        .filter_map(|a| {
            let e = broadband(&a.energy);
            let level_db = 10.0 * (e / e0).log10();
            (e > 0.0 && level_db >= MARK_FLOOR_DB).then(|| SimMark {
                t_ms: (a.time.0 - t0) * 1000.0,
                level_db,
                name: path_name(plan, &a.path),
            })
        })
        .collect();
    Some(SimMarks {
        marks,
        legend: overlay_legend(s, source),
    })
}

/// *simulated · aligned on direct sound · absorption fitted to 4 positions · Main L*.
pub fn overlay_legend(s: &SimState, source: &str) -> String {
    format!(
        "{} · aligned on direct sound · {} · {source}",
        sim::SIMULATED,
        sim::absorption_label(s)
    )
}

/// Gap between a mark's line and its name, and between neighbouring names, px.
const LABEL_GAP: f32 = 4.0;

/// Where each name goes: the strongest first, right of its line in the first row where it
/// covers no name already placed, else left of it; `None` where neither fits.
fn place_names(
    marks: &[SimMark],
    xs: &[Option<f32>],
    plot: Rect,
    top: f32,
    fs: f32,
) -> Vec<Option<Rect>> {
    let mut order: Vec<usize> = (0..marks.len()).collect();
    order.sort_by(|&a, &b| marks[b].level_db.total_cmp(&marks[a].level_db));
    let row_h = 1.3 * fs;
    let mut placed: Vec<Option<Rect>> = vec![None; marks.len()];
    let mut taken: Vec<Rect> = Vec::new();
    for i in order {
        let (Some(x), m) = (xs[i], &marks[i]) else {
            continue;
        };
        if m.level_db < LABEL_FLOOR_DB {
            continue;
        }
        let w = text_width(&m.name, fs);
        let spot = (0..LABEL_ROWS)
            .flat_map(|row| [(row, x + LABEL_GAP), (row, x - LABEL_GAP - w)])
            .map(|(row, x0)| Rect::new(x0, top + row as f32 * row_h, w, fs * 1.15))
            .find(|r| {
                r.x >= plot.x
                    && r.right() <= plot.right()
                    && taken.iter().all(|t| {
                        r.right() + LABEL_GAP <= t.x
                            || r.x >= t.right() + LABEL_GAP
                            || r.bottom() <= t.y
                            || r.y >= t.bottom()
                    })
            });
        if let Some(r) = spot {
            taken.push(r);
            placed[i] = Some(r);
        }
    }
    placed
}

/// The parts of a vertical line at `x` from `y0` to `y1` that pass no box of `boxes`: a name
/// crossed by another reflection's line would read as cut.
fn around(x: f32, y0: f32, y1: f32, boxes: &[Rect]) -> Vec<(f32, f32)> {
    let mut cuts: Vec<(f32, f32)> = boxes
        .iter()
        .filter(|b| x >= b.x - 2.0 && x <= b.right() + 2.0)
        .map(|b| (b.y - 2.0, b.bottom() + 2.0))
        .collect();
    cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    let mut y = y0;
    for (a, b) in cuts {
        if a > y {
            out.push((y, a.min(y1)));
        }
        y = y.max(b);
    }
    if y < y1 {
        out.push((y, y1));
    }
    out.retain(|(a, b)| b > a);
    out
}

/// Draws `marks` on the IR scene `sc`: a faint dashed line per reflection in the plot, the
/// strongest names (at least [`LABEL_FLOOR_DB`]) beside their lines in rows where they cover
/// no other name, on a backing that hides the response under them, every line broken where it would cross a name or the legend at the plot's
/// bottom right. Stored in `sc.sim`.
pub fn draw_marks(sc: &mut IrScene, marks: SimMarks, theme: &Theme) {
    let plot = sc.plot;
    let xm = sc.x_axis.mapping;
    let fs = theme.small_font_size;
    let colour = theme.text_dim.with_alpha(0.55);
    let Some(layer) = sc.scene.layers.get_mut(2) else {
        return;
    };
    let xs: Vec<Option<f32>> = marks
        .marks
        .iter()
        .map(|m| {
            let x = xm.to_px(m.t_ms);
            (x > plot.x && x <= plot.right()).then_some(x)
        })
        .collect();
    let names = place_names(&marks.marks, &xs, plot, plot.y + 0.3 * plot.h, fs);
    let legend_w = text_width(&marks.legend, fs);
    let legend = Rect::new(
        plot.right() - 6.0 - legend_w,
        plot.bottom() - 4.0 - 1.15 * fs,
        legend_w,
        1.15 * fs,
    );
    let mut boxes: Vec<Rect> = names.iter().flatten().copied().collect();
    boxes.push(legend);
    for x in xs.iter().flatten() {
        for (y0, y1) in around(*x, plot.y, plot.bottom(), &boxes) {
            layer.polylines.push(Polyline {
                points: vec![[*x, y0], [*x, y1]],
                alpha: vec![],
                stroke: Stroke {
                    color: colour,
                    width: 1.0,
                    dash: Some(Dash {
                        on: 2.0,
                        off: 4.0,
                        offset: 0.0,
                    }),
                },
                clip: Some(plot),
            });
        }
    }
    for (m, r) in marks.marks.iter().zip(&names) {
        if let Some(r) = r {
            // A backing in the plot's colour: the measured response under a name would
            // otherwise run through its letters.
            layer.rects.push(FillRect {
                rect: Rect::new(r.x - 1.0, r.y - 1.0, r.w + 2.0, r.h + 2.0),
                color: theme.background.with_alpha(0.85),
                clip: Some(plot),
            });
            layer.labels.push(label(
                m.name.clone(),
                [r.x, r.y],
                anchor(HAlign::Left, VAlign::Top),
                fs,
                colour,
            ));
        }
    }
    layer.labels.push(label(
        marks.legend.clone(),
        [plot.right() - 6.0, plot.bottom() - 4.0],
        anchor(HAlign::Right, VAlign::Bottom),
        fs,
        theme.text_dim,
    ));
    sc.sim = Some(marks);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
