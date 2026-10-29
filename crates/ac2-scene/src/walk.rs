//! The spatial walk as the operator reads it (`docs/design/spatial-walk.md` §3–§5): the walk
//! pane's plan with every point and its state, where the mic goes next read from FOH, whether
//! the measurement is ready there, and how far the average has come; the *Spatial walk…*
//! window's rows; the setup window's and the plan editor's rows ([`setup`]).

pub mod conditions;
pub mod plan_view;
pub mod setup;
#[cfg(test)]
mod tests;

use ac2_proto::model::{PlanSpec, PointState, WalkState};

use crate::canvas::{Canvas, anchor, cut_to, label};
use crate::primitives::{FillRect, HAlign, Rect, Scene, VAlign, Viewport};
use crate::view::Style;

pub use conditions::{Changed, conditions_text};
pub use plan_view::{
    DotLook, EMPTY_PLAN_M, PlanDot, PlanMap, PlanPicture, SNAP_M, plan_scene, scale_bar, snap,
};

/// The pane's title word.
pub const PANE_TITLE: &str = "Spatial walk";

/// The window Shift+W opens.
pub const WINDOW_TITLE: &str = "Spatial walk";

/// The window's first row.
pub const NEW_WALK: &str = "New walk…";

/// Whether the walk's measurement may be captured now (§5 *Ready*). Capture is allowed in
/// every state; what is captured before ready records it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ready {
    /// No frame of the measurement yet (not running, or the stream has not started).
    Waiting,
    /// Its averages are this full (0…1) since they last restarted.
    Settling(f32),
    /// The delay finder holds two near-equal arrivals and waits for the operator's pick.
    Pick,
    /// Full, settled, nothing to pick.
    Ready,
}

impl Ready {
    /// From the newest frame's `filled` (`None`: no frame) and the delay finder's
    /// `awaiting_pick`. A pick waits for the operator whatever the averages say.
    pub fn of(filled: Option<f32>, awaiting_pick: bool) -> Self {
        match filled {
            _ if awaiting_pick => Self::Pick,
            None => Self::Waiting,
            Some(f) if f < 1.0 => Self::Settling(f.max(0.0)),
            Some(_) => Self::Ready,
        }
    }

    /// *settling*, *arrival: pick one*, *ready*. How far settling has come is the bar
    /// beside the word ([`Ready::bar`]), not a number: `filled` counts blocks, so a number
    /// jumps in the stages' coarse steps.
    pub fn text(self) -> String {
        match self {
            Self::Waiting => "waiting for the measurement".to_owned(),
            Self::Settling(_) => "settling".to_owned(),
            Self::Pick => "arrival: pick one".to_owned(),
            Self::Ready => "ready".to_owned(),
        }
    }

    /// The readiness bar's fill (0…1) at the frame's value: full when ready, none while
    /// there is nothing to fill (no frame, or an arrival to pick).
    pub fn bar(self) -> Option<f32> {
        match self {
            Self::Settling(f) => Some(f.clamp(0.0, 1.0)),
            Self::Ready => Some(1.0),
            Self::Waiting | Self::Pick => None,
        }
    }
}

/// The point the walk goes to after the current one: the next pending point, wrapping
/// round, as the daemon advances on a capture.
pub fn next_point(w: &WalkState) -> Option<usize> {
    let n = w.points.len();
    let cur = w.current.map(usize::from);
    let start = cur.map_or(0, |c| c + 1);
    (0..n)
        .map(|k| (start + k) % n)
        .find(|&i| matches!(w.points[i].state, PointState::Pending) && Some(i) != cur)
}

/// *Move mic to P3 · 2.1 m left, 4.0 m back from FOH*; when every point is captured or
/// skipped, that.
pub fn step_text(w: &WalkState, plan: Option<&PlanSpec>) -> String {
    let Some(p) = w.current.and_then(|i| w.points.get(usize::from(i))) else {
        return "every point is captured or skipped".to_owned();
    };
    match plan {
        Some(plan) => format!(
            "Move mic to {} · {}",
            p.label,
            ac2_room::plan::readout(plan, [p.at[0], p.at[1]])
        ),
        None => format!("Move mic to {}", p.label),
    }
}

/// How many of the walk's points are captured, and how many are not left out.
pub fn counts(w: &WalkState) -> (usize, usize) {
    let captured = w
        .points
        .iter()
        .filter(|p| matches!(p.state, PointState::Captured { .. }))
        .count();
    let skipped = w
        .points
        .iter()
        .filter(|p| matches!(p.state, PointState::Skipped))
        .count();
    (captured, w.points.len() - skipped)
}

/// `8 positions · 2 unsettled`: the captures in the average and how many of them were
/// taken before the measurement was ready (none: no second part).
pub fn positions_text(positions: usize, unsettled: usize) -> String {
    let p = format!(
        "{positions} position{}",
        if positions == 1 { "" } else { "s" }
    );
    if unsettled == 0 {
        p
    } else {
        format!("{p} · {unsettled} unsettled")
    }
}

/// The average line: *NO AVERAGE · 1 OF 8 POSITIONS* until the second capture (the daemon
/// makes the average then), afterwards the average's name and [`positions_text`].
pub fn average_text(w: &WalkState, average_name: Option<&str>, unsettled: usize) -> String {
    let (captured, total) = counts(w);
    match (w.average, average_name) {
        (Some(_), Some(name)) => format!("{name} · {}", positions_text(captured, unsettled)),
        (Some(_), None) => format!("average · {}", positions_text(captured, unsettled)),
        (None, _) => format!("NO AVERAGE · {captured} OF {total} POSITIONS"),
    }
}

/// A row of the *Spatial walk…* window: `Mains · 3 of 8 captured · Hall`.
pub fn walk_row(w: &WalkState) -> String {
    let (captured, total) = counts(w);
    format!("{} · {captured} of {total} captured · {}", w.name, w.plan)
}

/// The dots of a walk on its plan.
pub fn walk_dots(w: &WalkState) -> Vec<PlanDot> {
    let next = next_point(w);
    w.points
        .iter()
        .enumerate()
        .map(|(i, p)| PlanDot {
            label: p.label.clone(),
            at: [p.at[0], p.at[1]],
            look: match p.state {
                PointState::Pending => DotLook::Pending,
                PointState::Captured { .. } => DotLook::Captured,
                PointState::Skipped => DotLook::Skipped,
            },
            current: w.current.map(usize::from) == Some(i),
            next: next == Some(i) && w.current.map(usize::from) != Some(i),
            broken: false,
        })
        .collect()
}

/// What the walk pane draws.
#[derive(Clone, Debug, PartialEq)]
pub struct WalkPane<'a> {
    pub walk: &'a WalkState,
    /// The session's copy of its plan; `None` when the session lacks it.
    pub plan: Option<&'a PlanSpec>,
    pub ready: Ready,
    /// The average measurement's name, once there is one.
    pub average: Option<&'a str>,
    /// Captures in the average taken before ready.
    pub unsettled: usize,
    /// The readiness bar as drawn now, gliding to [`Ready::bar`] as the level meters'
    /// bars do (`crate::bar_ease`); `None`: drawn at the frame's value.
    pub bar_drawn: Option<f32>,
    /// [`conditions_text`] of the walk: what differs between its captures.
    pub conditions: String,
}

/// The walk pane's lines, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct WalkLines {
    pub step: String,
    pub ready: String,
    pub average: String,
    /// What differs between the captures; empty when nothing recorded does.
    pub conditions: String,
}

impl WalkPane<'_> {
    pub fn lines(&self) -> WalkLines {
        WalkLines {
            step: step_text(self.walk, self.plan),
            ready: if self.walk.current.is_some() {
                self.ready.text()
            } else {
                String::new()
            },
            average: average_text(self.walk, self.average, self.unsettled),
            conditions: self.conditions.clone(),
        }
    }
}

/// The walk pane's picture and the plan mapping (for the mouse).
#[derive(Clone, Debug, PartialEq)]
pub struct WalkScene {
    pub scene: Scene,
    pub map: Option<PlanMap>,
    pub lines: WalkLines,
}

/// Length of the readiness bar beside its word, px: a meter read at a glance, not a scale.
const BAR_W: f32 = 160.0;

/// Builds the walk pane at `size`: its lines at the top (the step, readiness with its bar,
/// the average, what differs between the captures), the plan under them.
pub fn walk_scene(p: &WalkPane<'_>, style: Style, size: Viewport) -> WalkScene {
    let theme = style.theme;
    let mut c = Canvas::new(size, theme);
    let lines = p.lines();
    let pad = 10.0;
    let width = (size.width - 2.0 * pad).max(1.0);
    // The step is read from across the room: as large as the width allows, up to twice the
    // text size, never below it.
    let chars = lines.step.chars().count().max(1) as f32;
    let big = (width / (0.62 * chars)).clamp(theme.font_size, 2.0 * theme.font_size);
    let mut y = pad;
    c.overlay.labels.push(label(
        cut_to(&lines.step, width, big),
        [pad, y],
        anchor(HAlign::Left, VAlign::Top),
        big,
        theme.text,
    ));
    y += big * 1.4;
    let ready_colour = match p.ready {
        Ready::Ready => theme.level_ok,
        Ready::Pick => theme.banner_warning.background,
        Ready::Waiting | Ready::Settling(_) => theme.text_dim,
    };
    let ready_w = 0.62 * theme.font_size * lines.ready.chars().count() as f32;
    c.overlay.labels.push(label(
        cut_to(&lines.ready, width, theme.font_size),
        [pad, y],
        anchor(HAlign::Left, VAlign::Top),
        theme.font_size,
        ready_colour,
    ));
    if let Some(fill) = p.ready.bar().filter(|_| !lines.ready.is_empty()) {
        let fill = p.bar_drawn.unwrap_or(fill).clamp(0.0, 1.0);
        let x = pad + ready_w + theme.font_size;
        let w = BAR_W.min(pad + width - x);
        if w > 4.0 {
            let h = theme.font_size * 0.6;
            let top = y + (theme.font_size - h) / 2.0;
            c.base.rects.push(FillRect {
                rect: Rect::new(x, top, w, h),
                color: theme.plot_background,
                clip: None,
            });
            c.data.rects.push(FillRect {
                rect: Rect::new(x, top, w * fill, h),
                color: if matches!(p.ready, Ready::Ready) {
                    theme.level_ok
                } else {
                    theme.text_dim
                },
                clip: None,
            });
        }
    }
    y += theme.font_size * 1.5;
    c.overlay.labels.push(label(
        cut_to(&lines.average, width, theme.font_size),
        [pad, y],
        anchor(HAlign::Left, VAlign::Top),
        theme.font_size,
        if p.walk.average.is_some() {
            theme.text
        } else {
            theme.text_dim
        },
    ));
    y += theme.font_size * 1.6;
    if !lines.conditions.is_empty() {
        c.overlay.labels.push(label(
            cut_to(&lines.conditions, width, theme.font_size),
            [pad, y],
            anchor(HAlign::Left, VAlign::Top),
            theme.font_size,
            theme.banner_warning.background,
        ));
        y += theme.font_size * 1.6;
    }
    let rect = Rect::new(pad, y, width, (size.height - y - pad).max(1.0));
    let map = match p.plan {
        Some(plan) => {
            let pic = PlanPicture {
                plan,
                dots: walk_dots(p.walk),
                editing: None,
            };
            Some(plan_view::draw_plan(&mut c, &pic, rect, theme))
        }
        None => {
            c.overlay.labels.push(label(
                cut_to(
                    &format!("the session holds no plan named {}", p.walk.plan),
                    width,
                    theme.font_size,
                ),
                [pad + width / 2.0, rect.y + rect.h / 2.0],
                anchor(HAlign::Center, VAlign::Center),
                theme.font_size,
                theme.text_dim,
            ));
            None
        }
    };
    WalkScene {
        scene: c.into_scene(size),
        map,
        lines,
    }
}
