// Filled trapezoids (rects and band slices): vertical extent between a linear top and bottom
// edge over [x0, x1). Coverage is the exact box-filter overlap of the pixel with the
// interval between the edges, measured perpendicular to each edge, so thin and sloped band
// edges anti-alias like strokes. Outer left/right edges ramp too; inner edges between band
// slices are shared and owned by pixel centre, so seams are drawn exactly once. A rectangle
// whose edges lie on pixel edges (a snapped grid line) has hard edges: no margin, no ramp.

struct VsIn {
    @location(0) x: vec2<f32>,
    @location(1) top: vec2<f32>,
    @location(2) bottom: vec2<f32>,
    @location(3) clip: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) flags: u32,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) x: vec2<f32>,
    @location(1) @interpolate(flat) top: vec2<f32>,
    @location(2) @interpolate(flat) bottom: vec2<f32>,
    @location(3) @interpolate(flat) clip: vec4<f32>,
    @location(4) @interpolate(flat) color: vec4<f32>,
    @location(5) @interpolate(flat) flags: u32,
}

const AA_LEFT: u32 = 1u;
const AA_RIGHT: u32 = 2u;
const CRISP_Y: u32 = 4u;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, f: VsIn) -> VsOut {
    let ml = select(0.0, 1.0, (f.flags & AA_LEFT) != 0u);
    let mr = select(0.0, 1.0, (f.flags & AA_RIGHT) != 0u);
    let x = select(f.x.x - ml, f.x.y + mr, (vi & 2u) != 0u);
    let my = select(1.0, 0.0, (f.flags & CRISP_Y) != 0u);
    let y = select(min(f.top.x, f.top.y) - my, max(f.bottom.x, f.bottom.y) + my, (vi & 1u) != 0u);

    var out: VsOut;
    out.pos = vec4<f32>(to_ndc(vec2<f32>(x, y)), 0.0, 1.0);
    out.x = f.x;
    out.top = f.top;
    out.bottom = f.bottom;
    out.clip = f.clip;
    out.color = f.color;
    out.flags = f.flags;
    return out;
}

// Signed distance (pixels, positive below) from p to the line through (x0,y0)-(x1,y1).
fn below(p: vec2<f32>, x: vec2<f32>, y: vec2<f32>) -> f32 {
    let slope = (y.y - y.x) / (x.y - x.x);
    let at = y.x + slope * (p.x - x.x);
    return (p.y - at) * inverseSqrt(1.0 + slope * slope);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.pos.xy;
    if (!inside_clip(p, in.clip)) {
        discard;
    }
    var cx = 1.0;
    if ((in.flags & AA_LEFT) != 0u) {
        cx = cx * clamp(p.x - in.x.x + 0.5, 0.0, 1.0);
    } else if (p.x < in.x.x) {
        discard;
    }
    if ((in.flags & AA_RIGHT) != 0u) {
        cx = cx * clamp(in.x.y - p.x + 0.5, 0.0, 1.0);
    } else if (p.x >= in.x.y) {
        discard;
    }
    var cy = 1.0;
    if ((in.flags & CRISP_Y) != 0u) {
        if (p.y < in.top.x || p.y >= in.bottom.x) {
            discard;
        }
    } else {
        // Overlap of [y - 0.5, y + 0.5] with [top, bottom] = cover(top) + cover(bottom) - 1.
        let ct = clamp(below(p, in.x, in.top) + 0.5, 0.0, 1.0);
        let cb = clamp(0.5 - below(p, in.x, in.bottom), 0.0, 1.0);
        cy = max(ct + cb - 1.0, 0.0);
    }
    let a = in.color.a * cx * cy;
    if (a <= 0.0) {
        discard;
    }
    return output(in.color.rgb, a);
}
