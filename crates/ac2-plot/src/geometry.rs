//! CPU side of the instanced pipelines: scene primitives → GPU instances in physical
//! framebuffer pixels. Pure, allocation-free once the output vectors have grown.

use bytemuck::{Pod, Zeroable};

use crate::scene::{Band, Color, FillRect, Grid, GridAxis, GridKind, Polyline, Rect, Stroke};

/// Logical → physical framebuffer pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Xform {
    pub scale: f32,
    pub origin: [f32; 2],
}

impl Xform {
    pub fn point(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.origin[0] + p[0] * self.scale,
            self.origin[1] + p[1] * self.scale,
        ]
    }

    pub fn x(&self, x: f32) -> f32 {
        self.origin[0] + x * self.scale
    }

    pub fn y(&self, y: f32) -> f32 {
        self.origin[1] + y * self.scale
    }

    /// Rectangle as `[x0, y0, x1, y1]` in physical pixels.
    pub fn rect(&self, r: &Rect) -> [f32; 4] {
        [
            self.x(r.x),
            self.y(r.y),
            self.x(r.right()),
            self.y(r.bottom()),
        ]
    }
}

/// Physical clip rectangle `[x0, y0, x1, y1]`; empty when `x1 <= x0` or `y1 <= y0`.
pub(crate) type ClipPx = [f32; 4];

pub(crate) fn intersect(a: ClipPx, b: ClipPx) -> ClipPx {
    [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ]
}

pub(crate) fn is_empty(c: &ClipPx) -> bool {
    // Written so NaN edges count as empty.
    !(c[2] > c[0] && c[3] > c[1])
}

/// Integer scissor covering every pixel whose centre lies inside `clip`; `None` if none.
pub(crate) fn scissor_of(clip: &ClipPx, target: [u32; 2]) -> Option<[u32; 4]> {
    // A pixel [i, i+1) has its centre inside [x0, x1) iff x0 <= i + 0.5 < x1, which is
    // what the shaders test; the scissor must contain all those pixels. The first pixel
    // index whose centre is >= v is ceil(v - 0.5), for both edges.
    let edge = |v: f32, max: u32| ((v - 0.5).ceil().max(0.0) as u32).min(max);
    let x0 = edge(clip[0], target[0]);
    let y0 = edge(clip[1], target[1]);
    let x1 = edge(clip[2], target[0]);
    let y1 = edge(clip[3], target[1]);
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

/// `v > 0`, false for NaN: zero, negative and undefined widths/opacities draw nothing.
pub(crate) fn positive(v: f32) -> bool {
    v > 0.0
}

pub(crate) fn pack_color(c: Color) -> u32 {
    u32::from_le_bytes(c.to_rgba8())
}

/// One polyline segment, drawn as a capsule. Layout mirrors `VsIn` in `lines.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct SegmentInstance {
    pub prev: [f32; 2],
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub next: [f32; 2],
    pub clip: [f32; 4],
    /// Opacity at `p0` and `p1`.
    pub alpha: [f32; 2],
    /// Arc length at `p0`, dash on-length, dash period (0 = solid); physical pixels.
    pub dash: [f32; 3],
    pub half_width: f32,
    /// RGBA8, straight alpha.
    pub color: u32,
    pub flags: u32,
}

pub(crate) const HAS_PREV: u32 = 1;
pub(crate) const HAS_NEXT: u32 = 2;

/// Points closer than this (physical px) are merged: a zero-length segment between two
/// neighbours would tie with both in the join-ownership test and leave a hole.
const MERGE_EPS: f32 = 1e-3;

/// Stroke parameters resolved to physical pixels.
#[derive(Clone, Copy, Debug)]
struct PxStroke {
    half_width: f32,
    /// Opacity factor for sub-pixel strokes.
    alpha: f32,
    color: u32,
    dash_on: f32,
    dash_period: f32,
    dash_offset: f32,
}

fn px_stroke(s: &Stroke, scale: f32, snap_width: bool) -> Option<PxStroke> {
    let w = s.width * scale;
    if !positive(w) || !positive(s.color.a) {
        return None;
    }
    // Below one pixel the AA ramp would be wider than the stroke and the line would break
    // into beads; a 1 px line at reduced opacity carries the same ink per unit length.
    let (half_width, alpha) = if w < 1.0 {
        (0.5, w)
    } else if snap_width {
        (w.round() * 0.5, 1.0)
    } else {
        (w * 0.5, 1.0)
    };
    let (dash_on, dash_period, dash_offset) = match s.dash {
        Some(d) if d.on > 0.0 && d.off > 0.0 => {
            (d.on * scale, (d.on + d.off) * scale, d.offset * scale)
        }
        _ => (0.0, 0.0, 0.0),
    };
    Some(PxStroke {
        half_width,
        alpha,
        color: pack_color(s.color),
        dash_on,
        dash_period,
        dash_offset,
    })
}

fn finite(p: [f32; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

/// Reusable scratch for polyline runs.
#[derive(Debug, Default)]
pub(crate) struct LineScratch {
    /// Current run: physical point and opacity.
    run: Vec<([f32; 2], f32)>,
}

fn emit_run(out: &mut Vec<SegmentInstance>, run: &[([f32; 2], f32)], s: &PxStroke, clip: ClipPx) {
    let base = SegmentInstance {
        prev: [0.0; 2],
        p0: [0.0; 2],
        p1: [0.0; 2],
        next: [0.0; 2],
        clip,
        alpha: [0.0; 2],
        dash: [s.dash_offset, s.dash_on, s.dash_period],
        half_width: s.half_width,
        color: s.color,
        flags: 0,
    };
    match run {
        [] => {}
        // A lone point is a dot: a zero-length capsule.
        [(p, a)] => out.push(SegmentInstance {
            prev: *p,
            p0: *p,
            p1: *p,
            next: *p,
            alpha: [a * s.alpha; 2],
            ..base
        }),
        _ => {
            let n = run.len();
            let mut arc = s.dash_offset;
            for i in 0..n - 1 {
                let (p0, a0) = run[i];
                let (p1, a1) = run[i + 1];
                let mut flags = 0;
                if i > 0 {
                    flags |= HAS_PREV;
                }
                if i + 2 < n {
                    flags |= HAS_NEXT;
                }
                out.push(SegmentInstance {
                    prev: run[i.saturating_sub(1)].0,
                    p0,
                    p1,
                    next: run[(i + 2).min(n - 1)].0,
                    alpha: [a0 * s.alpha, a1 * s.alpha],
                    dash: [arc, s.dash_on, s.dash_period],
                    flags,
                    ..base
                });
                arc += (p1[0] - p0[0]).hypot(p1[1] - p0[1]);
            }
        }
    }
}

/// Appends the segments of `line`; non-finite points split it into runs.
pub(crate) fn push_polyline(
    out: &mut Vec<SegmentInstance>,
    scratch: &mut LineScratch,
    line: &Polyline,
    xf: &Xform,
    clip: ClipPx,
) {
    let Some(s) = px_stroke(&line.stroke, xf.scale, false) else {
        return;
    };
    let alpha_at = |i: usize| -> f32 {
        if line.alpha.is_empty() {
            1.0
        } else {
            line.alpha[i].clamp(0.0, 1.0)
        }
    };
    scratch.run.clear();
    for (i, &p) in line.points.iter().enumerate() {
        if !finite(p) {
            emit_run(out, &scratch.run, &s, clip);
            scratch.run.clear();
            continue;
        }
        let q = xf.point(p);
        if let Some((last, _)) = scratch.run.last()
            && (q[0] - last[0]).abs() < MERGE_EPS
            && (q[1] - last[1]).abs() < MERGE_EPS
        {
            continue;
        }
        scratch.run.push((q, alpha_at(i)));
    }
    emit_run(out, &scratch.run, &s, clip);
}

/// Physical coordinate of a grid line so that a stroke of `width_px` whole pixels covers
/// whole pixel columns: odd widths centre on a pixel centre, even widths on a pixel edge.
pub(crate) fn snap(pos: f32, width_px: f32) -> f32 {
    let w = width_px.round().max(1.0) as u32;
    if w % 2 == 1 {
        pos.floor() + 0.5
    } else {
        pos.round()
    }
}

/// Appends grid lines, snapped to the pixel grid, and returns the clip they use: `grid.rect`
/// grown by half the widest stroke, so a line on the rect's edge (the plot frame) is drawn
/// whole however the edge rounds, then intersected with `clip`.
///
/// A snapped solid line covers whole pixel columns (rows), so across it coverage is exactly
/// 0 or 1 and it is a filled rectangle with a hard edge there: one fragment per covered
/// pixel instead of a capsule's quad with an anti-aliasing margin and distance tests. Along
/// it the ends ramp over a pixel as a capsule's ends do on the line's axis. Dashed lines
/// stay strokes (`segments`).
pub(crate) fn push_grid(
    fills: &mut Vec<FillInstance>,
    segments: &mut Vec<SegmentInstance>,
    grid: &Grid,
    xf: &Xform,
    clip: ClipPx,
) -> ClipPx {
    let r = xf.rect(&grid.rect);
    let major = px_stroke(&grid.major, xf.scale, true);
    let minor = px_stroke(&grid.minor, xf.scale, true);
    let hw = [major, minor]
        .iter()
        .flatten()
        .map(|s| s.half_width)
        .fold(0.0, f32::max);
    let clip = intersect(clip, [r[0] - hw, r[1] - hw, r[2] + hw, r[3] + hw]);
    if is_empty(&clip) {
        return clip;
    }
    for l in &grid.lines {
        let s = match l.kind {
            GridKind::Major => major,
            GridKind::Minor => minor,
        };
        let Some(s) = s else { continue };
        if !l.pos.is_finite() {
            continue;
        }
        let hw = s.half_width;
        let at = match l.axis {
            GridAxis::X => snap(xf.x(l.pos), hw * 2.0),
            GridAxis::Y => snap(xf.y(l.pos), hw * 2.0),
        };
        if s.dash_period > 0.0 {
            let (p0, p1) = match l.axis {
                GridAxis::X => ([at, r[1]], [at, r[3]]),
                GridAxis::Y => ([r[0], at], [r[2], at]),
            };
            emit_run(segments, &[(p0, 1.0), (p1, 1.0)], &s, clip);
            continue;
        }
        let color = scale_alpha(s.color, s.alpha);
        fills.push(match l.axis {
            GridAxis::X => FillInstance {
                x: [at - hw, at + hw],
                top: [r[1] - hw; 2],
                bottom: [r[3] + hw; 2],
                clip,
                color,
                flags: 0,
            },
            GridAxis::Y => FillInstance {
                x: [r[0] - hw, r[2] + hw],
                top: [at - hw; 2],
                bottom: [at + hw; 2],
                clip,
                color,
                flags: AA_LEFT | AA_RIGHT | CRISP_Y,
            },
        });
    }
    clip
}

/// A packed straight-alpha colour with its opacity multiplied by `a`.
fn scale_alpha(c: u32, a: f32) -> u32 {
    let [r, g, b, al] = c.to_le_bytes();
    let al = (f32::from(al) * a.clamp(0.0, 1.0)).round() as u8;
    u32::from_le_bytes([r, g, b, al])
}

/// Filled trapezoid between `x[0]` and `x[1]` with linear top and bottom edges. Layout
/// mirrors `VsIn` in `fill.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct FillInstance {
    pub x: [f32; 2],
    /// Top edge y at `x[0]`, `x[1]` (smaller y).
    pub top: [f32; 2],
    /// Bottom edge y at `x[0]`, `x[1]`.
    pub bottom: [f32; 2],
    pub clip: [f32; 4],
    pub color: u32,
    pub flags: u32,
}

/// The left/right edge is an outer edge and gets an anti-aliasing ramp. Inner edges between
/// adjacent band trapezoids are shared exactly, so each pixel column belongs to one of them
/// (pixel centre in `[x0, x1)`) and translucent bands do not double-blend at the seams.
pub(crate) const AA_LEFT: u32 = 1;
pub(crate) const AA_RIGHT: u32 = 2;
/// Top and bottom lie on pixel edges and are hard: rows whose centre is inside are fully
/// covered (snapped grid lines).
pub(crate) const CRISP_Y: u32 = 4;

pub(crate) fn push_rect(out: &mut Vec<FillInstance>, r: &FillRect, xf: &Xform, clip: ClipPx) {
    if !positive(r.color.a) {
        return;
    }
    let p = xf.rect(&r.rect);
    let (x0, x1) = (p[0].min(p[2]), p[0].max(p[2]));
    let (y0, y1) = (p[1].min(p[3]), p[1].max(p[3]));
    if !(x1 > x0 && y1 > y0) {
        return;
    }
    out.push(FillInstance {
        x: [x0, x1],
        top: [y0, y0],
        bottom: [y1, y1],
        clip,
        color: pack_color(r.color),
        flags: AA_LEFT | AA_RIGHT,
    });
}

pub(crate) fn push_band(out: &mut Vec<FillInstance>, band: &Band, xf: &Xform, clip: ClipPx) {
    if !positive(band.color.a) {
        return;
    }
    let color = pack_color(band.color);
    let ok = |a: &crate::scene::BandPoint| a.x.is_finite() && a.y0.is_finite() && a.y1.is_finite();
    let mut prev_emitted = false;
    for (i, w) in band.points.windows(2).enumerate() {
        let (a, b) = (&w[0], &w[1]);
        if !(ok(a) && ok(b) && b.x > a.x) {
            if let Some(last) = out.last_mut().filter(|_| prev_emitted) {
                last.flags |= AA_RIGHT;
            }
            prev_emitted = false;
            continue;
        }
        let (ya0, ya1) = (xf.y(a.y0), xf.y(a.y1));
        let (yb0, yb1) = (xf.y(b.y0), xf.y(b.y1));
        let mut flags = 0;
        if !prev_emitted {
            flags |= AA_LEFT;
        }
        if i + 2 == band.points.len() {
            flags |= AA_RIGHT;
        }
        out.push(FillInstance {
            x: [xf.x(a.x), xf.x(b.x)],
            top: [ya0.min(ya1), yb0.min(yb1)],
            bottom: [ya0.max(ya1), yb0.max(yb1)],
            clip,
            color,
            flags,
        });
        prev_emitted = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{BandPoint, Dash, GridLine};

    const XF: Xform = Xform {
        scale: 1.0,
        origin: [0.0, 0.0],
    };
    const CLIP: ClipPx = [0.0, 0.0, 100.0, 100.0];

    fn line(points: Vec<[f32; 2]>, width: f32) -> Polyline {
        Polyline {
            points,
            alpha: vec![],
            stroke: Stroke::solid(Color::WHITE, width),
            clip: None,
        }
    }

    #[test]
    fn layouts_match_shaders() {
        assert_eq!(std::mem::size_of::<SegmentInstance>(), 80);
        assert_eq!(std::mem::size_of::<FillInstance>(), 48);
    }

    #[test]
    fn joins_are_flagged_only_inside_a_run() {
        let mut out = vec![];
        let l = line(vec![[0.0, 0.0], [1.0, 0.0], [2.0, 1.0], [3.0, 0.0]], 2.0);
        push_polyline(&mut out, &mut LineScratch::default(), &l, &XF, CLIP);
        let flags: Vec<u32> = out.iter().map(|s| s.flags).collect();
        assert_eq!(flags, [HAS_NEXT, HAS_PREV | HAS_NEXT, HAS_PREV]);
    }

    #[test]
    fn non_finite_points_split_runs_and_duplicates_merge() {
        let mut out = vec![];
        let l = line(
            vec![
                [0.0, 0.0],
                [1.0, 0.0],
                [1.0, 0.0],
                [2.0, 0.0],
                [f32::NAN, 0.0],
                [5.0, 5.0],
                [f32::INFINITY, 0.0],
                [7.0, 7.0],
                [8.0, 7.0],
            ],
            2.0,
        );
        push_polyline(&mut out, &mut LineScratch::default(), &l, &XF, CLIP);
        let segs: Vec<([f32; 2], [f32; 2], u32)> =
            out.iter().map(|s| (s.p0, s.p1, s.flags)).collect();
        assert_eq!(
            segs,
            [
                ([0.0, 0.0], [1.0, 0.0], HAS_NEXT),
                ([1.0, 0.0], [2.0, 0.0], HAS_PREV),
                ([5.0, 5.0], [5.0, 5.0], 0),
                ([7.0, 7.0], [8.0, 7.0], 0),
            ]
        );
    }

    #[test]
    fn sub_pixel_stroke_becomes_faded_hairline() {
        let mut out = vec![];
        let mut l = line(vec![[0.0, 0.0], [10.0, 0.0]], 0.25);
        l.alpha = vec![1.0, 0.5];
        push_polyline(&mut out, &mut LineScratch::default(), &l, &XF, CLIP);
        assert_eq!(out[0].half_width, 0.5);
        assert_eq!(out[0].alpha, [0.25, 0.125]);
        // Same stroke at 4x pixel density is a full 1 px line.
        out.clear();
        let xf = Xform {
            scale: 4.0,
            origin: [0.0, 0.0],
        };
        push_polyline(&mut out, &mut LineScratch::default(), &l, &xf, CLIP);
        assert_eq!(out[0].half_width, 0.5);
        assert_eq!(out[0].alpha, [1.0, 0.5]);
    }

    #[test]
    fn dash_arc_length_accumulates() {
        let mut out = vec![];
        let mut l = line(vec![[0.0, 0.0], [3.0, 4.0], [3.0, 10.0]], 1.0);
        l.stroke.dash = Some(Dash {
            on: 2.0,
            off: 1.0,
            offset: 0.5,
        });
        let xf = Xform {
            scale: 2.0,
            origin: [0.0, 0.0],
        };
        push_polyline(&mut out, &mut LineScratch::default(), &l, &xf, CLIP);
        assert_eq!(out[0].dash, [1.0, 4.0, 6.0]);
        assert_eq!(out[1].dash, [11.0, 4.0, 6.0]);
    }

    #[test]
    fn grid_snaps_to_pixel_grid() {
        assert_eq!(snap(10.3, 1.0), 10.5);
        assert_eq!(snap(10.7, 1.0), 10.5);
        assert_eq!(snap(10.7, 2.0), 11.0);
        assert_eq!(snap(10.7, 3.0), 10.5);
        let g = Grid {
            rect: Rect::new(10.0, 10.0, 50.0, 40.0),
            lines: vec![
                GridLine {
                    axis: GridAxis::X,
                    pos: 20.2,
                    kind: GridKind::Major,
                },
                GridLine {
                    axis: GridAxis::Y,
                    pos: 30.9,
                    kind: GridKind::Minor,
                },
            ],
            major: Stroke::solid(Color::WHITE, 1.0),
            minor: Stroke::solid(Color::WHITE, 0.5),
        };
        let (mut fills, mut segs) = (vec![], vec![]);
        let xf = Xform {
            scale: 1.5,
            origin: [0.0, 0.0],
        };
        let clip = push_grid(&mut fills, &mut segs, &g, &xf, CLIP);
        assert!(segs.is_empty());
        // 1.5 px major rounds to 2 px: on a pixel edge, two whole columns, hard across and
        // ramped at its ends.
        assert_eq!(fills[0].x, [29.0, 31.0]);
        assert_eq!(fills[0].top, [14.0, 14.0]);
        assert_eq!(fills[0].bottom, [76.0, 76.0]);
        assert_eq!(fills[0].flags, 0);
        // 0.75 px minor is a faded hairline: one whole row at 3/4 opacity.
        assert_eq!(fills[1].top, [46.0, 46.0]);
        assert_eq!(fills[1].bottom, [47.0, 47.0]);
        assert_eq!(fills[1].x, [14.5, 90.5]);
        assert_eq!(fills[1].flags, AA_LEFT | AA_RIGHT | CRISP_Y);
        assert_eq!(fills[1].color.to_le_bytes()[3], 191);
        assert_eq!(clip, [14.0, 14.0, 91.0, 76.0]);
        assert_eq!(fills[0].clip, clip);
        // A dashed grid stays strokes.
        let dashed = Grid {
            major: Stroke {
                dash: Some(Dash {
                    on: 2.0,
                    off: 2.0,
                    offset: 0.0,
                }),
                ..g.major
            },
            ..g
        };
        let (mut fills, mut segs) = (vec![], vec![]);
        push_grid(&mut fills, &mut segs, &dashed, &xf, CLIP);
        assert_eq!((fills.len(), segs.len()), (1, 1));
        assert_eq!(segs[0].p0, [30.0, 15.0]);
        assert_eq!(segs[0].half_width, 1.0);
    }

    #[test]
    fn band_marks_outer_edges_and_gaps() {
        let p = |x: f32| BandPoint {
            x,
            y0: 5.0,
            y1: 1.0,
        };
        let band = Band {
            points: vec![p(0.0), p(1.0), p(2.0), p(f32::NAN), p(4.0), p(5.0)],
            color: Color::WHITE,
            clip: None,
        };
        let mut out = vec![];
        push_band(&mut out, &band, &XF, CLIP);
        let flags: Vec<u32> = out.iter().map(|f| f.flags).collect();
        assert_eq!(flags, [AA_LEFT, AA_RIGHT, AA_LEFT | AA_RIGHT]);
        assert_eq!(out[0].top, [1.0, 1.0]);
        assert_eq!(out[0].bottom, [5.0, 5.0]);
    }

    #[test]
    fn scissor_covers_pixels_whose_centres_are_inside() {
        assert_eq!(
            scissor_of(&[0.0, 0.0, 10.0, 10.0], [100, 100]),
            Some([0, 0, 10, 10])
        );
        // Centre 10.5 is inside [10.4, 20.6): pixels 10..=20.
        assert_eq!(
            scissor_of(&[10.4, 10.6, 20.6, 20.4], [100, 100]),
            Some([10, 11, 11, 9])
        );
        assert_eq!(scissor_of(&[10.6, 0.0, 11.4, 5.0], [100, 100]), None);
        assert_eq!(
            scissor_of(&[-5.0, -5.0, 500.0, 50.0], [100, 40]),
            Some([0, 0, 100, 40])
        );
    }
}
