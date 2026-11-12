//! The 3D spectrograph scene: the skyline primitive, floor, glow, labels, legend and caption.

use super::*;

/// What the 3D spectrograph shows: the history of the pane's measurement, its skyline and
/// how to see it.
#[derive(Clone, Debug)]
pub struct Spectrograph3dInput<'a> {
    /// The measurement's live curve: its legend row's, and the one whose tracked peaks (P)
    /// stand on the newest row.
    pub key: TraceKey,
    pub history: &'a SpectrographHistory,
    /// Updated for this history, the pane's frequency span and `tier`.
    pub skyline: &'a Skyline,
    pub name: String,
    /// The level range the heights span (the pane's level axis for the history's scale).
    pub range: Range,
    /// The measurement's display offset, dB.
    pub offset_db: f64,
    pub freshness: Option<Freshness>,
    pub colours: Colormap,
    pub camera: Pose,
    pub tier: SkylineTier,
    /// The tier was set in Settings, not chosen by the adapter.
    pub tier_forced: bool,
    /// How much the axis decorations show, 0 … 1 ([`AxesReveal::alpha`]); what the tier
    /// in `view.chrome` would show, faded. An exported picture shows them whole (1).
    pub axes: f32,
    /// How many history slots past the end of the newest slot the time axis stands
    /// ([`Skyline::drawn`], [`SpectrographHistory::lead_slots`]); 0 draws the rows at
    /// their places (scroll off, an exported picture).
    pub lead_slots: f64,
}

/// Which axis a label belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelAxis {
    Frequency,
    Time,
    Level,
    /// The level unit at the top of the height scale.
    Unit,
}

/// A label placed by projecting its anchor through the camera.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisLabel {
    pub axis: LabelAxis,
    pub text: String,
    pub pos: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spectrograph3dScene {
    pub scene: Scene,
    pub banners: Vec<crate::banner::BannerRow>,
    pub plot: Rect,
    pub caption: String,
    /// Shown in the plot instead of a picture.
    pub message: Option<String>,
    pub labels: Vec<AxisLabel>,
    /// The time axis's side: `true` right.
    pub time_right: bool,
    pub view_proj: ViewProj,
    /// The measurement's tracked peaks (P) on the newest row (loudest first).
    pub points: Vec<super::PeakPoint3d>,
    /// The legend's rows as drawn: the measurement's curve, named and tagged as in the
    /// spectrum's legend. Empty without a measurement or while hidden.
    pub legend_shown: Vec<String>,
    /// The legend's plate, moved and scaled as on every view.
    pub legend_box: Option<crate::legend::LegendBox>,
    /// Where its row is: a click there selects the measurement.
    pub legend_hits: Vec<crate::legend::LegendHit>,
    /// What a click needs to pick a spike ([`super::pick()`]); `None` without a picture.
    pub pick: Option<super::PickFrame>,
    /// The cursor's cell, marked on its spike; `None` off the picture, without a cursor, or
    /// at traces only (T).
    pub cursor: Option<Cursor3d>,
    /// The spike under the pointer ([`ViewState::pointer_3d`]), marked: what a click puts
    /// the cursor on.
    pub hover: Option<super::Picked>,
    /// The cursor values' plate, as every view's.
    pub readout_box: Option<crate::plate::PlateBox>,
}

/// The cursor in the 3D view: the cell it is on, read as the 2D view reads its cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct Cursor3d {
    pub freq_hz: f64,
    pub before_s: f64,
    /// The cell's column and how many rows back from the newest.
    pub cell: (usize, usize),
    /// Its lines on the plate: the frequency, then `4.2 s ago  −23.5 dBFS`.
    pub lines: Vec<String>,
    /// The spike's tip on screen; `None` when it has no spike or is behind the eye.
    pub tip: Option<[f32; 2]>,
}

impl Spectrograph3dScene {
    /// The plates the operator moves: the legend and the cursor values.
    pub fn plates(&self) -> Vec<crate::plate::PlateBox> {
        self.legend_box
            .as_ref()
            .map(|b| b.plate)
            .into_iter()
            .chain(self.readout_box)
            .collect()
    }
}

/// Floor x of frequency `hz` over `lo … hi`.
pub(super) fn x_of(hz: f64, lo: f64, hi: f64) -> f64 {
    -FLOOR_HALF_W + 2.0 * FLOOR_HALF_W * (hz / lo).ln() / (hi / lo).ln()
}

/// Maps world points to the plot's pixels.
pub(super) struct Projector {
    vp: ViewProj,
    plot: Rect,
}

impl Projector {
    pub(super) fn new(vp: ViewProj, plot: Rect) -> Self {
        Self { vp, plot }
    }

    fn px(&self, ndc: [f64; 2]) -> [f32; 2] {
        [
            self.plot.x + ((ndc[0] + 1.0) / 2.0) as f32 * self.plot.w,
            self.plot.y + ((1.0 - ndc[1]) / 2.0) as f32 * self.plot.h,
        ]
    }

    pub(super) fn point(&self, p: V3) -> Option<[f32; 2]> {
        self.vp.ndc(p).map(|n| self.px(n))
    }

    pub(super) fn inside(&self, p: [f32; 2]) -> bool {
        p[0] >= self.plot.x
            && p[0] <= self.plot.right()
            && p[1] >= self.plot.y
            && p[1] <= self.plot.bottom()
    }

    /// The segment `a`–`b` cut at the near plane; `None` when wholly behind it.
    fn segment(&self, a: V3, b: V3) -> Option<[[f32; 2]; 2]> {
        let (ca, cb) = (self.vp.clip(a), self.vp.clip(b));
        let (wa, wb) = (ca[3] - NEAR, cb[3] - NEAR);
        if wa < 0.0 && wb < 0.0 {
            return None;
        }
        let cut = |p: V3, q: V3, wp: f64, wq: f64| add(p, scale(sub(q, p), wp / (wp - wq)));
        let a = if wa < 0.0 { cut(a, b, wa, wb) } else { a };
        let b = if wb < 0.0 { cut(b, a, wb, wa) } else { b };
        Some([self.point(a)?, self.point(b)?])
    }

    /// The polygon cut at the near plane, in pixels.
    fn polygon(&self, pts: &[V3]) -> Vec<[f32; 2]> {
        let w = |p: V3| self.vp.clip(p)[3] - NEAR;
        let mut out = Vec::new();
        for (i, &p) in pts.iter().enumerate() {
            let q = pts[(i + 1) % pts.len()];
            let (wp, wq) = (w(p), w(q));
            if wp >= 0.0 {
                out.extend(self.point(p));
            }
            if (wp >= 0.0) != (wq >= 0.0) {
                out.extend(self.point(add(p, scale(sub(q, p), wp / (wp - wq)))));
            }
        }
        out
    }

    /// Projected length of `a`–`b`, pixels (0 when behind the eye).
    fn length(&self, a: V3, b: V3) -> f32 {
        self.segment(a, b).map_or(0.0, |[p, q]| {
            ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt()
        })
    }
}

pub(super) fn mix(field: Color, light: Color, k: f32) -> Color {
    Color::rgb(
        (field.r + light.r * k).min(1.0),
        (field.g + light.g * k).min(1.0),
        (field.b + light.b * k).min(1.0),
    )
}

/// Generous box around a one-line label (no font metrics here): 0.62 em a character.
pub(super) fn label_box(l: &Label) -> Rect {
    let w = canvas::text_width(&l.text, l.size);
    let h = l.size * 1.25;
    let x = match l.anchor.h {
        HAlign::Left => l.pos[0],
        HAlign::Center => l.pos[0] - w / 2.0,
        HAlign::Right => l.pos[0] - w,
    };
    let y = match l.anchor.v {
        VAlign::Top => l.pos[1],
        VAlign::Center => l.pos[1] - h / 2.0,
        _ => l.pos[1] - h,
    };
    Rect::new(x, y, w, h)
}

/// How much of the field a label's plate hides: enough to dim a spike behind the text, not
/// so much that the plate reads as a box.
pub const PLATE_ALPHA: f32 = 0.55;

/// A rounded plate around `b`, tightened to the text: [`label_box`] is generous across.
pub(crate) fn plate(b: Rect) -> Vec<[f32; 2]> {
    const R: f32 = 3.0;
    const SEGS: usize = 4;
    let inset = (b.w * 0.06).min(4.0);
    let (x0, x1) = (b.x + inset - 2.0, b.right() - inset + 2.0);
    let (y0, y1) = (b.y, b.bottom());
    let r = R.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    let corners = [
        (x1 - r, y0 + r, -90.0f32),
        (x1 - r, y1 - r, 0.0),
        (x0 + r, y1 - r, 90.0),
        (x0 + r, y0 + r, 180.0),
    ];
    let mut out = Vec::with_capacity(4 * (SEGS + 1));
    for (cx, cy, a0) in corners {
        for i in 0..=SEGS {
            let a = (a0 + 90.0 * i as f32 / SEGS as f32).to_radians();
            out.push([cx + r * a.cos(), cy + r * a.sin()]);
        }
    }
    out
}

/// Labels kept where they do not overlap one kept before them, in the order given.
pub(super) fn thin<T>(candidates: Vec<(T, Label)>, plot: Rect, out: &mut Vec<(T, Label)>) {
    const GAP: f32 = 6.0;
    for c in candidates {
        let b = label_box(&c.1);
        // A label the field would cut is not drawn: half a number reads as another one.
        let whole = b.x >= plot.x
            && b.right() <= plot.right()
            && b.y >= plot.y
            && b.bottom() <= plot.bottom();
        let clear = whole
            && out.iter().all(|(_, k)| {
                let o = label_box(k);
                b.right() + GAP <= o.x
                    || o.right() + GAP <= b.x
                    || b.bottom() + 1.0 <= o.y
                    || o.bottom() + 1.0 <= b.y
            });
        if clear {
            out.push(c);
        }
    }
}

/// The spectrum pane's 3D spectrograph: the banners, a caption, and the skyline of `sg`'s
/// history on a dark field with its labels, height scale and ceiling line as `view.chrome`
/// says. `None` (no spectrum or RTA measurement) draws an empty field that says so.
pub fn spectrograph_3d_scene(
    traces: &[SpectrumTrace<'_>],
    status: &Status,
    sg: Option<&Spectrograph3dInput<'_>>,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> Spectrograph3dScene {
    // The axes come on demand: at rest the view is the skyline alone, as T's traces-only
    // tier draws it; while shown they fade as a whole, each part what the tier would show.
    let axes = sg.map_or(1.0, |s| s.axes.clamp(0.0, 1.0));
    let chrome = if axes > 0.0 {
        view.chrome
    } else {
        PlotChrome::Bare
    };
    let right = crate::freq_column::right(RIGHT_MARGIN, view);
    let mut c0 = Canvas::new(size, theme);
    let plot_w = (size.width - MARGINS.left - right).max(1.0);
    let strip = canvas::banner_strip(&mut c0, status, MARGINS.left, plot_w, size, theme);
    let top = strip.rect.bottom() + MARGINS.top;
    let mut layers = c0.into_scene(size).layers;
    let plot = Rect::new(
        MARGINS.left,
        top + CAPTION_H,
        plot_w,
        (size.height - top - CAPTION_H - MARGINS.bottom).max(1.0),
    );
    let aspect = f64::from(plot.w / plot.h);
    let pose = sg.map_or(Pose::from(SpectrumCamera::DEFAULT), |s| s.camera);
    let vp = view_proj(pose, aspect);
    let pj = Projector::new(vp, plot);
    // The field and the labels in it keep the dark theme's colours in every theme.
    let dark = Theme::dark();
    let mut c = Canvas::default();
    c.base.rects.push(FillRect::new(plot, FIELD));
    let colours = sg.map_or_else(Colormap::default, |s| s.colours);
    let mid = Color::from_rgba8(colormap::lut(colours)[colormap::LUT_SIZE / 2]);
    let depth_fade = |z: f64| (-f64::from(LOOK.age_fade) * z / FLOOR_DEPTH).exp() as f32;
    // Floor band: strips front to back, each lit by the depth fade at its front, under
    // the spikes (which are max-composited over it, so the brighter of the two shows).
    for i in 0..FLOOR_STRIPS {
        let z0 = FLOOR_DEPTH * i as f64 / FLOOR_STRIPS as f64;
        let z1 = FLOOR_DEPTH * (i + 1) as f64 / FLOOR_STRIPS as f64;
        let w = FLOOR_HALF_W;
        let points = pj.polygon(&[[-w, 0.0, z0], [w, 0.0, z0], [w, 0.0, z1], [-w, 0.0, z1]]);
        if points.len() >= 3 {
            c.base.polygons.push(FillPolygon {
                points,
                color: mix(FIELD, mid, FLOOR_BAND * depth_fade(z0)),
                clip: Some(plot),
            });
        }
    }
    if let Some(s) = pj.segment([-FLOOR_HALF_W, 0.0, 0.0], [FLOOR_HALF_W, 0.0, 0.0]) {
        c.base.polylines.push(Polyline::clipped(
            s.to_vec(),
            Stroke::solid(mix(FIELD, mid, NOW_LINE), 1.0),
            plot,
        ));
    }

    let skyline = sg.map(|s| s.skyline);
    let span_hz = skyline.and_then(Skyline::span_hz);
    let span_s = sg.map_or(ac2_proto::model::SpectrumDisplay::DEFAULT_SPAN_S, |s| {
        s.history.span_s()
    });
    let range = sg.map_or(view.spectrum.level, |s| s.range);
    let time_right = pose.yaw_deg >= 0.0;
    let side_x = if time_right {
        FLOOR_HALF_W
    } else {
        -FLOOR_HALF_W
    };
    // Age (seconds) to floor depth, as the renderer places rows: a row of `k` slots
    // stands at its oldest edge, `R + 1` rows over the floor's depth.
    let z_of_age = |t: f64| {
        skyline
            .filter(|s| s.rows() > 0)
            .map_or(t / f64::from(span_s) * FLOOR_DEPTH, |s| {
                FLOOR_DEPTH * t / (s.row_s() * s.rows() as f64)
            })
    };

    let grid_stroke = Stroke::solid(dark.grid_major.color.with_alpha(0.5 * axes), 1.0);
    let mut cands = Vec::new();
    let small = theme.small_font_size;
    if let Some((lo, hi)) = span_hz {
        let front = pj.length([-FLOOR_HALF_W, 0.0, 0.0], [FLOOR_HALF_W, 0.0, 0.0]);
        let fax = axis::freq_axis(Range::new(lo, hi), 0.0, front.max(1.0));
        let decade = |v: f64| (v.log10() - v.log10().round()).abs() < 1e-9;
        let mut ticks: Vec<_> = fax
            .ticks
            .iter()
            .filter(|t| t.label.is_some() && t.value >= lo && t.value <= hi)
            .collect();
        // Decades first: thinning keeps them over the 2 and 5 between.
        ticks.sort_by_key(|t| !decade(t.value));
        if chrome.grid() {
            for t in fax.ticks.iter().filter(|t| decade(t.value)) {
                let x = x_of(t.value, lo, hi);
                if let Some(s) = pj.segment([x, 0.0, 0.0], [x, 0.0, FLOOR_DEPTH]) {
                    c.base
                        .polylines
                        .push(Polyline::clipped(s.to_vec(), grid_stroke, plot));
                }
            }
        }
        for t in ticks {
            let Some(p) = pj.point([x_of(t.value, lo, hi), 0.0, 0.0]) else {
                continue;
            };
            let text = t.label.clone().unwrap_or_default();
            let l = label(
                text.clone(),
                [p[0], p[1] + 4.0],
                anchor(HAlign::Center, VAlign::Top),
                small,
                dark.axis_text,
            );
            if pj.inside(p) {
                cands.push((
                    AxisLabel {
                        axis: LabelAxis::Frequency,
                        text,
                        pos: l.pos,
                    },
                    l,
                ));
            }
        }
    }
    // Time along the side nearer the eye.
    let side = pj.length([side_x, 0.0, 0.0], [side_x, 0.0, FLOOR_DEPTH]);
    let tax = axis::linear_axis(Range::new(0.0, f64::from(span_s)), 0.0, side.max(1.0), "s");
    let mut times: Vec<(f64, String)> = vec![(0.0, "now".to_string())];
    times.extend(
        tax.ticks
            .iter()
            .filter(|t| t.label.is_some() && t.value > 0.0 && t.value <= f64::from(span_s))
            .map(|t| {
                (
                    t.value,
                    format!("−{} s", t.label.clone().unwrap_or_default()),
                )
            }),
    );
    for (t, text) in times {
        let z = z_of_age(t);
        if z > FLOOR_DEPTH + 1e-9 {
            continue;
        }
        if chrome.grid()
            && t > 0.0
            && let Some(s) = pj.segment([-FLOOR_HALF_W, 0.0, z], [FLOOR_HALF_W, 0.0, z])
        {
            c.base
                .polylines
                .push(Polyline::clipped(s.to_vec(), grid_stroke, plot));
        }
        let Some(p) = pj.point([side_x, 0.0, z]) else {
            continue;
        };
        let (h, dx) = if time_right {
            (HAlign::Left, 6.0)
        } else {
            (HAlign::Right, -6.0)
        };
        let l = label(
            text.clone(),
            [p[0] + dx, p[1]],
            anchor(h, VAlign::Center),
            small,
            dark.axis_text,
        );
        if pj.inside(p) {
            cands.push((
                AxisLabel {
                    axis: LabelAxis::Time,
                    text,
                    pos: l.pos,
                },
                l,
            ));
        }
    }
    // Time before frequency: "now" shares the front corner with the top decade's label, and
    // the corner says where the newest row is.
    cands.sort_by_key(|(a, _)| a.axis != LabelAxis::Time);
    // The height scale: a post at the front corner nearer the eye, floor to range top.
    let ceiling_stroke = Stroke {
        color: dark.text_dim.with_alpha(0.45 * axes),
        width: 1.0,
        dash: Some(Dash {
            on: 4.0,
            off: 4.0,
            offset: 0.0,
        }),
    };
    let post_stroke = Stroke::solid(dark.axis_text.with_alpha(0.7 * axes), 1.0);
    let unit = sg
        .and_then(|s| s.history.unit())
        .map_or(String::new(), str::to_string);
    if chrome.labels() && range.is_valid() {
        if let Some(s) = pj.segment([side_x, 0.0, 0.0], [side_x, MAX_HEIGHT, 0.0]) {
            c.data
                .polylines
                .push(Polyline::clipped(s.to_vec(), post_stroke, plot));
        }
        if let Some(s) = pj.segment(
            [-FLOOR_HALF_W, MAX_HEIGHT, FLOOR_DEPTH],
            [FLOOR_HALF_W, MAX_HEIGHT, FLOOR_DEPTH],
        ) {
            c.data
                .polylines
                .push(Polyline::clipped(s.to_vec(), ceiling_stroke, plot));
        }
        let post = pj.length([side_x, 0.0, 0.0], [side_x, MAX_HEIGHT, 0.0]);
        let lax = axis::linear_axis(range, 0.0, post.max(1.0), &unit);
        let mut ticks = Vec::new();
        for t in lax.ticks.iter().filter(|t| t.label.is_some()) {
            let y = MAX_HEIGHT * (t.value - range.lo) / range.span();
            // A tick on the floor would sit on the time axis's "now"; the floor reads the
            // range's bottom without it.
            if !(MAX_HEIGHT * 0.05..=MAX_HEIGHT + 1e-9).contains(&y) {
                continue;
            }
            let (Some(p), Some(q)) = (
                pj.point([side_x, y, 0.0]),
                pj.point([side_x * 1.03, y, 0.0]),
            ) else {
                continue;
            };
            c.data
                .polylines
                .push(Polyline::clipped(vec![p, q], post_stroke, plot));
            ticks.push((p, q, t.label.clone().unwrap_or_default()));
        }
        // The numbers stand outside the post, past its ticks; where the field's edge would
        // cut one, all of them stand inside it instead, so the scale reads on one side.
        let place = |inward: bool, p: [f32; 2], q: [f32; 2], text: &str| {
            let (h, x) = match (time_right, inward) {
                (true, false) => (HAlign::Left, q[0] + 5.0),
                (false, false) => (HAlign::Right, q[0] - 5.0),
                (true, true) => (HAlign::Right, p[0] - 5.0),
                (false, true) => (HAlign::Left, p[0] + 5.0),
            };
            label(
                text.to_string(),
                [x, q[1]],
                anchor(h, VAlign::Center),
                small,
                dark.axis_text,
            )
        };
        let inward = ticks.iter().any(|(p, q, text)| {
            let b = label_box(&place(false, *p, *q, text));
            b.x < plot.x || b.right() > plot.right()
        });
        let mut level = Vec::new();
        for (p, q, text) in ticks {
            let l = place(inward, p, q, &text);
            if pj.inside(p) {
                level.push((
                    AxisLabel {
                        axis: LabelAxis::Level,
                        text,
                        pos: l.pos,
                    },
                    l,
                ));
            }
        }
        if let Some(p) = pj.point([side_x, MAX_HEIGHT, 0.0])
            && pj.inside(p)
            && !unit.is_empty()
        {
            // High enough that the top tick's number, centred on the post's top, clears it.
            let l = label(
                unit.clone(),
                [p[0], p[1] - 9.0],
                anchor(HAlign::Center, VAlign::Bottom),
                small,
                dark.text_dim,
            );
            level.insert(
                0,
                (
                    AxisLabel {
                        axis: LabelAxis::Unit,
                        text: unit.clone(),
                        pos: l.pos,
                    },
                    l,
                ),
            );
        }
        // The level scale's own labels first: its unit and ticks read the heights.
        let mut kept = Vec::new();
        thin(level, plot, &mut kept);
        thin(cands, plot, &mut kept);
        cands = kept;
    } else if !chrome.labels() {
        cands.clear();
    } else {
        let mut kept = Vec::new();
        thin(cands, plot, &mut kept);
        cands = kept;
    }

    let mut message = None;
    let mut pick = None;
    let mut caption = String::new();
    match sg {
        None => message = Some("no spectrum or RTA measurement".to_string()),
        Some(s) => {
            let name = if s.offset_db != 0.0 {
                offset_note(&s.name, s.offset_db)
            } else {
                s.name.clone()
            };
            caption = format!("{name} · last {span_s} s");
            if let Some(unit) = s.history.unit_noted() {
                caption.push_str(&format!(" · {unit}"));
            }
            // The legend tags the curve; without one (hidden), the caption does.
            if !crate::spectrograph::legend_shows(traces, s.key, view)
                && let Some(tag) = s.freshness.and_then(|f| f.tag())
            {
                caption.push_str(" · ");
                caption.push_str(&tag);
            }
            let sk = s.skyline;
            if s.history.is_empty() || sk.span_hz().is_none() || !range.is_valid() {
                message = Some(format!("{}: no frames yet", s.name));
            } else {
                let off = s.offset_db;
                let drawn = sk.drawn(s.lead_slots);
                pick = Some(super::PickFrame {
                    view_proj: vp,
                    plot,
                    phase: drawn.phase,
                    newest_id: drawn.newest_id,
                    range: [range.lo - off, range.hi - off],
                    tier: s.tier,
                });
                let data = sk.ring().to_vec();
                c.data.skylines.push(Skyline3d {
                    id: SKYLINE_ID,
                    rect: plot,
                    clip: Some(plot),
                    cols: sk.cols() as u32,
                    rows: sk.rows() as u32,
                    newest: drawn.newest as u32,
                    newest_id: drawn.newest_id,
                    phase: drawn.phase,
                    view_proj: vp.gpu(),
                    eye: vp.eye.map(|v| v as f32),
                    world: [FLOOR_HALF_W as f32, FLOOR_DEPTH as f32, MAX_HEIGHT as f32],
                    // Heights are of the displayed (offset) level.
                    range: [(range.lo - off) as f32, (range.hi - off) as f32],
                    colormap: s.colours,
                    tier: s.tier,
                    look: SkylineLook {
                        width_px: match s.tier {
                            SkylineTier::Full => LOOK.width_px,
                            SkylineTier::Lite | SkylineTier::Bare => LITE_WIDTH_PX,
                        },
                        ..LOOK
                    },
                    data,
                });
                if s.tier.bloom_taps().is_some() {
                    c.data.glows.push(SkylineGlow {
                        id: SKYLINE_ID,
                        clip: Some(plot),
                        gain: glow_gain(s.colours),
                    });
                }
            }
        }
    }
    // The tracked peaks (P) of the measurement drawn, on its newest row. Few, and asked for:
    // they show at rest too, reading the pane's own tier, not the bare one the hidden axes
    // force; traces only hides them.
    let points = match sg {
        Some(s) if message.is_none() && view.chrome.peak_marks() => traces
            .iter()
            .find(|t| t.key == s.key)
            .and_then(|t| Some((t, t.tracked?)))
            .map_or_else(Vec::new, |(t, peaks)| {
                super::points::peak_points(
                    &mut c,
                    &pj,
                    s.skyline,
                    s.skyline.drawn(s.lead_slots),
                    peaks,
                    t.offset_db,
                    crate::spectrum::value_unit(t.scale, t.quantity),
                    range,
                    plot,
                    small,
                )
            }),
        _ => Vec::new(),
    };
    // The spike under the pointer first, so the cursor's mark is drawn over it.
    let hover = match (sg, pick.as_ref(), view.pointer_3d) {
        (Some(s), Some(f), Some(at)) if view.chrome.cursor() => super::pick(f, s.skyline, at),
        _ => None,
    };
    if let (Some(s), Some(f), Some(h)) = (sg, pick.as_ref(), hover.as_ref()) {
        let cursor = super::cursor::cursor_at(s.skyline, view).map(|c| c.2);
        super::cursor::hover_cell(&mut c, f, s.skyline, h, cursor);
    }
    let cursor = match (sg, pick.as_ref()) {
        (Some(s), Some(f)) if view.chrome.cursor() => {
            super::cursor::cursor_cell(&mut c, f, s, view)
        }
        _ => None,
    };
    let cell = sg.and_then(|s| cell_text(s.skyline));
    let tier = sg
        .filter(|s| s.tier_forced)
        .map(|s| tier_name(s.tier).to_string());
    let tail: Vec<String> = cell.into_iter().chain(tier).collect();
    let with_tail = |base: &str| {
        std::iter::once(base.to_string())
            .chain(tail.iter().cloned())
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let mut forms = Vec::new();
    if !caption.is_empty()
        && let Some(cal) = traces.first().map(|t| t.caption.as_str())
        && let cal = cal.strip_suffix(" · stopped").unwrap_or(cal)
        && !cal.is_empty()
    {
        for f in crate::spectrum::caption_forms(cal) {
            if !f.is_empty() {
                forms.push(with_tail(&format!("{caption} · {f}")));
            }
        }
    }
    let own = with_tail(&caption);
    forms.extend([own.clone(), caption.clone(), String::new()]);
    let shown = forms
        .into_iter()
        .find(|c| canvas::text_width(c, small) <= plot.w)
        .unwrap_or_default();
    let caption = if caption.is_empty() {
        caption
    } else {
        shown.clone()
    };
    c.overlay.labels.push(label(
        shown,
        [plot.x, plot.y - 3.0],
        anchor(HAlign::Left, VAlign::Bottom),
        small,
        theme.text_dim,
    ));
    if let Some(m) = &message {
        c.overlay.labels.push(label(
            m.clone(),
            [plot.x + plot.w / 2.0, plot.y + plot.h / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            small,
            dark.text_dim,
        ));
    }
    let labels = if chrome == PlotChrome::Bare {
        Vec::new()
    } else {
        cands
    };
    let axis_labels = labels.iter().map(|(a, _)| a.clone()).collect();
    // Over the spikes and their bloom (the overlay draws after both): each label on a
    // faint dark plate, so a number over a bright spike still reads.
    for (_, l) in &labels {
        c.overlay.polygons.push(FillPolygon {
            points: plate(label_box(l)),
            color: FIELD.with_alpha(PLATE_ALPHA * axes),
            clip: Some(plot),
        });
    }
    c.overlay
        .labels
        .extend(labels.into_iter().map(|(_, mut l)| {
            l.clip = Some(plot);
            l.color = l.color.with_alpha(axes);
            l
        }));
    // The legend is no axis: its plate stays at rest, as the points do, over
    // everything else.
    let legend = crate::spectrograph::legend_over(
        &mut c.overlay,
        traces,
        sg.map(|s| s.key),
        view,
        plot,
        theme,
    );
    // The cursor values' plate as every view's, at the plot's top right; over the legend's
    // place only when dragged there.
    let readout_box = cursor.as_ref().map(|cur| {
        let top = plot.y + crate::legend::INSET + crate::legend::PAD_Y;
        crate::spectrum::readout_plate(&mut c.overlay, cur.lines.clone(), plot, top, view, theme).0
    });
    layers.extend(c.into_scene(size).layers.into_iter().take(3));
    Spectrograph3dScene {
        scene: Scene {
            viewport: size,
            layers,
        },
        banners: strip.rows,
        plot,
        caption,
        message,
        labels: axis_labels,
        time_right,
        view_proj: vp,
        points,
        legend_shown: legend.as_ref().map_or_else(Vec::new, |l| l.shown.clone()),
        legend_box: legend.as_ref().and_then(|l| l.b.clone()),
        legend_hits: legend.as_ref().map_or_else(Vec::new, |l| l.hits.clone()),
        pick,
        cursor,
        hover,
        readout_box,
    }
}

/// `1/19 oct · 0.30 s`: the cell the picture is made of.
pub fn cell_text(s: &Skyline) -> Option<String> {
    let (lo, hi) = s.span_hz()?;
    let per_oct = s.cols() as f64 / (hi / lo).log2();
    let freq = if per_oct >= 1.5 {
        format!("1/{} oct", per_oct.round())
    } else {
        format!("{:.1} oct", 1.0 / per_oct)
    };
    Some(format!("{freq} · {:.2} s", s.row_s()))
}

/// What the caption calls a forced tier.
pub fn tier_name(t: SkylineTier) -> &'static str {
    match t {
        SkylineTier::Full => "full",
        SkylineTier::Lite => "lite",
        SkylineTier::Bare => "bare",
    }
}
