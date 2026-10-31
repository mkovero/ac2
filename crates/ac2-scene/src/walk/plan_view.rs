//! A room plan in plan view: floor, zones, markers and measurement positions placed in a
//! rectangle at one scale on both axes (a room keeps its shape), plan y up, with a scale bar.
//! The mapping it returns is what the mouse reads metres back from.

use ac2_proto::model::{MarkerKind, PlanSpec, SimMap};

use crate::canvas::{Canvas, anchor, label};
use crate::primitives::{
    Color, FillPolygon, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport,
};
use crate::theme::Theme;

/// Plan metres to pixels and back: one scale on both axes, plan y up, the plan's middle at
/// the rectangle's middle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanMap {
    /// The rectangle the plan is placed in.
    pub rect: Rect,
    px_per_m: f64,
    centre: [f64; 2],
}

impl PlanMap {
    /// The plan area from `lo` to `hi` (metres) fitted into `rect`; an area under a metre
    /// across reads as a metre, so a single point still has a scale.
    pub fn fit(lo: [f64; 2], hi: [f64; 2], rect: Rect) -> Self {
        let span = [(hi[0] - lo[0]).max(1.0), (hi[1] - lo[1]).max(1.0)];
        let k = (f64::from(rect.w) / span[0]).min(f64::from(rect.h) / span[1]);
        Self {
            rect,
            px_per_m: k.max(1e-6),
            centre: [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])],
        }
    }

    /// Pixels per metre.
    pub fn px_per_m(&self) -> f64 {
        self.px_per_m
    }

    /// Where plan point `p` (metres) is drawn.
    pub fn to_px(&self, p: [f64; 2]) -> [f32; 2] {
        let cx = f64::from(self.rect.x) + 0.5 * f64::from(self.rect.w);
        let cy = f64::from(self.rect.y) + 0.5 * f64::from(self.rect.h);
        [
            (cx + (p[0] - self.centre[0]) * self.px_per_m) as f32,
            (cy - (p[1] - self.centre[1]) * self.px_per_m) as f32,
        ]
    }

    /// The plan point (metres) under pixel `p`.
    pub fn to_m(&self, p: [f32; 2]) -> [f64; 2] {
        let cx = f64::from(self.rect.x) + 0.5 * f64::from(self.rect.w);
        let cy = f64::from(self.rect.y) + 0.5 * f64::from(self.rect.h);
        [
            self.centre[0] + (f64::from(p[0]) - cx) / self.px_per_m,
            self.centre[1] - (f64::from(p[1]) - cy) / self.px_per_m,
        ]
    }
}

/// What a click places is snapped to this grid, metres: a tenth of a metre is finer than a
/// tape measure is read in a hall and coarse enough that typed and clicked values agree.
pub const SNAP_M: f64 = 0.1;

/// `p` on the [`SNAP_M`] grid, without a minus sign on a zero.
pub fn snap(p: [f64; 2]) -> [f64; 2] {
    let s = |v: f64| {
        let r = (v / SNAP_M).round() * SNAP_M;
        let r = (r * 10.0).round() / 10.0;
        if r == 0.0 { 0.0 } else { r }
    };
    [s(p[0]), s(p[1])]
}

/// The scale bar for `px_per_m`: the longest of 0.5, 1, 2, 5, 10, 20, 50, 100 m that fits
/// in `max_px`, and its label (`5 m`).
pub fn scale_bar(px_per_m: f64, max_px: f32) -> (f64, String) {
    let steps = [0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0];
    let m = steps
        .iter()
        .copied()
        .rev()
        .find(|m| m * px_per_m <= f64::from(max_px))
        .unwrap_or(steps[0]);
    let text = if m < 1.0 {
        format!("{m:.1} m")
    } else {
        format!("{m:.0} m")
    };
    (m, text)
}

/// How a point on the plan looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DotLook {
    /// A walk point not captured yet (or a suggested position in the setup).
    Pending,
    /// A captured walk point.
    Captured,
    /// A point the operator skipped.
    Skipped,
    /// A vertex of the polygon being edited.
    Vertex,
}

/// One point drawn on the plan.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanDot {
    /// Its label beside it (`P3`, a vertex number).
    pub label: String,
    /// Plan x, y; metres.
    pub at: [f64; 2],
    pub look: DotLook,
    /// The mic is here now (a walk), or the row in focus (an editor).
    pub current: bool,
    /// The point the walk goes to next: drawn faint.
    pub next: bool,
    /// It breaks a rule of the walk's preset: marked, not refused.
    pub broken: bool,
    /// The palette place of the ring when current or next: the walk mic standing or going
    /// there (0 outside a walk).
    pub mic: usize,
}

/// A plan and what is drawn on it.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanPicture<'a> {
    pub plan: &'a PlanSpec,
    pub dots: Vec<PlanDot>,
    /// The polygon being edited, drawn as a line through its vertices in order (closed
    /// from three), so an outline under construction reads before it validates.
    pub editing: Option<Vec<[f64; 2]>>,
    /// A simulated coverage map drawn faintly over the zones, under everything else.
    pub coverage: Option<&'a SimMap>,
}

/// Opacity of the coverage cells: the zones, markers and points stay readable through it.
pub const COVERAGE_ALPHA: f32 = 0.5;

/// The coverage colour of a cell at `db` re the loudest (0 … [`SimMap::FLOOR_DB`]): a dark
/// violet through red and orange to pale yellow at the loudest, a ramp whose lightness rises
/// steadily, so it reads in either theme and in grey. Not a measurement colour.
pub fn heat(db: f32) -> Color {
    const STOPS: [[f32; 3]; 4] = [
        [0.18, 0.05, 0.33],
        [0.55, 0.13, 0.42],
        [0.90, 0.36, 0.17],
        [0.99, 0.85, 0.40],
    ];
    let t = (1.0 - db / SimMap::FLOOR_DB).clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let i = (t.floor() as usize).min(STOPS.len() - 2);
    let f = t - i as f32;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    Color::rgba(
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
        COVERAGE_ALPHA,
    )
}

impl PlanPicture<'_> {
    /// The plan area to show: every vertex, marker and dot, a metre round them; `None` when
    /// there is nothing yet.
    pub fn bounds(&self) -> Option<([f64; 2], [f64; 2])> {
        let p = self.plan;
        let all = p
            .outline
            .iter()
            .copied()
            .chain(p.zones.iter().flat_map(|z| z.outline.iter().copied()))
            .chain(p.markers.iter().map(|m| [m.at[0], m.at[1]]))
            .chain(self.dots.iter().map(|d| d.at))
            .chain(self.editing.iter().flatten().copied());
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for v in all.filter(|v| v[0].is_finite() && v[1].is_finite()) {
            for k in 0..2 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        lo[0]
            .is_finite()
            .then(|| ([lo[0] - 1.0, lo[1] - 1.0], [hi[0] + 1.0, hi[1] + 1.0]))
    }
}

/// With nothing on the plan yet, the view shows this much floor from the origin (metres),
/// so the first click has a scale to land on.
pub const EMPTY_PLAN_M: f64 = 20.0;

/// The plan view as a scene of its own (the walk setup, the plan editor): the plan in
/// `size` and the mapping the mouse reads.
pub fn plan_scene(pic: &PlanPicture<'_>, size: Viewport, theme: &Theme) -> (Scene, PlanMap) {
    let mut c = Canvas::new(size, theme);
    let rect = Rect::new(
        8.0,
        8.0,
        (size.width - 16.0).max(1.0),
        (size.height - 16.0).max(1.0),
    );
    let map = draw_plan(&mut c, pic, rect, theme);
    (c.into_scene(size), map)
}

fn ring(centre: [f32; 2], r: f32, sides: usize) -> Vec<[f32; 2]> {
    (0..sides)
        .map(|k| {
            let a = std::f32::consts::TAU * k as f32 / sides as f32;
            [centre[0] + r * a.cos(), centre[1] + r * a.sin()]
        })
        .collect()
}

fn closed(mut v: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    if let Some(first) = v.first().copied() {
        v.push(first);
    }
    v
}

fn line(points: Vec<[f32; 2]>, color: Color, width: f32, clip: Rect) -> Polyline {
    Polyline {
        points,
        alpha: Vec::new(),
        stroke: Stroke::solid(color, width),
        clip: Some(clip),
    }
}

fn disc(at: [f32; 2], r: f32, color: Color, clip: Rect) -> FillPolygon {
    FillPolygon {
        points: ring(at, r, 20),
        color,
        clip: Some(clip),
    }
}

/// Draws `pic` into `rect` of `c` and returns the mapping it used.
pub(crate) fn draw_plan(
    c: &mut Canvas,
    pic: &PlanPicture<'_>,
    rect: Rect,
    theme: &Theme,
) -> PlanMap {
    let (lo, hi) = pic
        .bounds()
        .unwrap_or(([-1.0, -1.0], [EMPTY_PLAN_M + 1.0, EMPTY_PLAN_M + 1.0]));
    let map = PlanMap::fit(lo, hi, rect);
    let plan = pic.plan;
    let px = |v: &[[f64; 2]]| v.iter().map(|p| map.to_px(*p)).collect::<Vec<_>>();
    let fs = theme.small_font_size;

    // The floor, then the zones over it.
    if plan.outline.len() >= 3 {
        c.base.polygons.push(FillPolygon {
            points: px(&plan.outline),
            color: theme.plot_background,
            clip: Some(rect),
        });
        c.base
            .polylines
            .push(line(closed(px(&plan.outline)), theme.text_dim, 2.0, rect));
    }
    for (k, z) in plan.zones.iter().enumerate() {
        let colour = theme.families[k % theme.families.len()];
        if z.outline.len() >= 3 {
            c.base.polygons.push(FillPolygon {
                points: px(&z.outline),
                color: colour.with_alpha(0.16),
                clip: Some(rect),
            });
        }
        c.base.polylines.push(line(
            closed(px(&z.outline)),
            colour.with_alpha(0.6),
            1.0,
            rect,
        ));
        if !z.outline.is_empty() {
            let n = z.outline.len() as f64;
            let mid = z
                .outline
                .iter()
                .fold([0.0, 0.0], |a, v| [a[0] + v[0] / n, a[1] + v[1] / n]);
            let mut l = label(
                z.name.clone(),
                map.to_px(mid),
                anchor(HAlign::Center, VAlign::Center),
                fs,
                colour.with_alpha(0.8),
            );
            l.clip = Some(rect);
            c.base.labels.push(l);
        }
    }
    if let Some(m) = pic.coverage {
        let w = (m.spacing * map.px_per_m()) as f32;
        for cell in &m.cells {
            let centre = [
                m.origin[0] + f64::from(cell.col) * m.spacing,
                m.origin[1] + f64::from(cell.row) * m.spacing,
            ];
            let tl = map.to_px([centre[0] - 0.5 * m.spacing, centre[1] + 0.5 * m.spacing]);
            // The data layer: its rects go under its markers and dots, over the floor.
            c.data.rects.push(FillRect {
                rect: Rect::new(tl[0], tl[1], w, w),
                color: heat(cell.db),
                clip: Some(rect),
            });
        }
    }
    if let Some(v) = &pic.editing {
        let pts = px(v);
        let pts = if v.len() >= 3 { closed(pts) } else { pts };
        c.data.polylines.push(line(pts, theme.text, 1.5, rect));
    }

    // Markers: a speaker a filled triangle, FOH and points a ring, a heading a tick.
    for m in &plan.markers {
        let at = map.to_px([m.at[0], m.at[1]]);
        let colour = match m.kind {
            MarkerKind::Speaker => theme.text,
            MarkerKind::Foh => theme.traces[1 % theme.traces.len()],
            MarkerKind::Point => theme.text_dim,
        };
        match m.kind {
            MarkerKind::Speaker => c.data.polygons.push(FillPolygon {
                points: ring(at, 7.0, 3),
                color: colour,
                clip: Some(rect),
            }),
            MarkerKind::Foh | MarkerKind::Point => {
                c.data
                    .polylines
                    .push(line(closed(ring(at, 6.0, 16)), colour, 2.0, rect))
            }
        }
        if let Some(h) = m.heading_deg {
            let a = h.to_radians();
            let tip = map.to_px([m.at[0] + a.cos(), m.at[1] + a.sin()]);
            c.data
                .polylines
                .push(line(vec![at, tip], colour, 2.0, rect));
        }
        let mut l = label(
            m.name.clone(),
            [at[0] + 9.0, at[1] + 9.0],
            anchor(HAlign::Left, VAlign::Top),
            fs,
            colour,
        );
        l.clip = Some(rect);
        c.data.labels.push(l);
    }

    // The positions: captured filled, pending hollow, skipped small and grey; the current
    // ones large with a ring in their mic's colour, the next ones with a faint ring; a broken
    // rule a warning ring.
    for d in &pic.dots {
        let at = map.to_px(d.at);
        let (r, colour) = match d.look {
            DotLook::Captured => (6.0, theme.level_ok),
            DotLook::Pending => (5.0, theme.text_dim),
            DotLook::Skipped => (3.5, theme.neutral.with_alpha(0.6)),
            DotLook::Vertex => (4.0, theme.text),
        };
        let r = if d.current { r + 2.0 } else { r };
        match d.look {
            DotLook::Pending => {
                c.data
                    .polylines
                    .push(line(closed(ring(at, r, 16)), colour, 1.5, rect))
            }
            _ => c.data.polygons.push(disc(at, r, colour, rect)),
        }
        if d.current {
            c.overlay.polylines.push(line(
                closed(ring(at, r + 6.0, 24)),
                theme.trace_color(d.mic),
                2.5,
                rect,
            ));
        } else if d.next {
            c.overlay.polylines.push(line(
                closed(ring(at, r + 5.0, 24)),
                theme.trace_color(d.mic).with_alpha(0.35),
                1.5,
                rect,
            ));
        }
        if d.broken {
            c.overlay.polylines.push(line(
                closed(ring(at, r + 3.0, 20)),
                theme.banner_warning.background,
                2.0,
                rect,
            ));
        }
        let text_colour = if d.current {
            theme.text
        } else if d.next {
            theme.text.with_alpha(0.55)
        } else {
            theme.text_dim
        };
        let text = if d.broken {
            format!("{} !", d.label)
        } else {
            d.label.clone()
        };
        let mut l = label(
            text,
            [at[0] + r + 4.0, at[1] - r - 2.0],
            anchor(HAlign::Left, VAlign::Bottom),
            fs,
            text_colour,
        );
        l.clip = Some(rect);
        c.overlay.labels.push(l);
    }

    // The scale bar, bottom left, a quarter of the width at most.
    let (m, text) = scale_bar(map.px_per_m(), rect.w * 0.25);
    let len = (m * map.px_per_m()) as f32;
    let y = rect.y + rect.h - 6.0;
    let x0 = rect.x + 6.0;
    c.overlay.polylines.push(line(
        vec![[x0, y - 4.0], [x0, y], [x0 + len, y], [x0 + len, y - 4.0]],
        theme.text_dim,
        1.5,
        rect,
    ));
    c.overlay.labels.push(label(
        text,
        [x0 + len + 6.0, y],
        anchor(HAlign::Left, VAlign::Center),
        fs,
        theme.text_dim,
    ));
    map
}
