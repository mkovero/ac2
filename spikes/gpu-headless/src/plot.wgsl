// Thick anti-aliased polylines as instanced quads, one instance per segment.
//
// Each segment is a capsule (round caps). Adjacent capsules overlap at a join; with
// translucent strokes that overlap would blend twice and leave dark beads. Each fragment
// therefore also measures its distance to the previous and next segment and keeps itself
// only if this segment is the nearest one. The union of the three capsules is then drawn
// exactly once, which gives round joins without double blending.

struct Globals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;

struct SegIn {
    @location(0) prev: vec2<f32>,
    @location(1) p0: vec2<f32>,
    @location(2) p1: vec2<f32>,
    @location(3) next: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) alpha: vec2<f32>,
    @location(6) half_width: f32,
    @location(7) flags: u32,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) prev: vec2<f32>,
    @location(1) @interpolate(flat) p0: vec2<f32>,
    @location(2) @interpolate(flat) p1: vec2<f32>,
    @location(3) @interpolate(flat) next: vec2<f32>,
    @location(4) @interpolate(flat) color: vec4<f32>,
    @location(5) @interpolate(flat) alpha: vec2<f32>,
    @location(6) @interpolate(flat) half_width: f32,
    @location(7) @interpolate(flat) flags: u32,
}

const HAS_PREV: u32 = 1u;
const HAS_NEXT: u32 = 2u;
// Coverage ramps over one pixel centred on the stroke edge, so the quad must reach half a
// pixel beyond the edge; one full pixel leaves room for rounding.
const AA_MARGIN: f32 = 1.0;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, seg: SegIn) -> VsOut {
    let d = seg.p1 - seg.p0;
    let len = length(d);
    var dir = vec2<f32>(1.0, 0.0);
    if (len > 1e-6) {
        dir = d / len;
    }
    let n = vec2<f32>(-dir.y, dir.x);
    let e = seg.half_width + AA_MARGIN;
    // Triangle strip corners: (start,-n) (start,+n) (end,-n) (end,+n).
    let along = select(-e, len + e, (vi & 2u) != 0u);
    let side = select(-e, e, (vi & 1u) != 0u);
    let px = seg.p0 + dir * along + n * side;
    let ndc = vec2<f32>(px.x / globals.viewport.x * 2.0 - 1.0, 1.0 - px.y / globals.viewport.y * 2.0);

    var out: VsOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.prev = seg.prev;
    out.p0 = seg.p0;
    out.p1 = seg.p1;
    out.next = seg.next;
    out.color = seg.color;
    out.alpha = seg.alpha;
    out.half_width = seg.half_width;
    out.flags = seg.flags;
    return out;
}

// Distance from p to segment ab, and the projection parameter t in [0,1].
fn seg_dist(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let pa = p - a;
    let ba = b - a;
    let l2 = dot(ba, ba);
    var t = 0.0;
    if (l2 > 1e-12) {
        t = clamp(dot(pa, ba) / l2, 0.0, 1.0);
    }
    return vec2<f32>(length(pa - ba * t), t);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The framebuffer pixel centre is identical for every instance touching this pixel,
    // so the ownership test below is evaluated on bit-identical inputs by both neighbours.
    let p = in.pos.xy;
    let self_dt = seg_dist(p, in.p0, in.p1);
    let d = self_dt.x;
    // Ties go to the later segment: this one yields when next is <=, the next one keeps
    // itself when its prev (this) is merely ==.
    if ((in.flags & HAS_PREV) != 0u && seg_dist(p, in.prev, in.p0).x < d) {
        discard;
    }
    if ((in.flags & HAS_NEXT) != 0u && seg_dist(p, in.p1, in.next).x <= d) {
        discard;
    }
    let coverage = clamp(in.half_width + 0.5 - d, 0.0, 1.0);
    let a = in.color.a * mix(in.alpha.x, in.alpha.y, self_dt.y) * coverage;
    if (a <= 0.0) {
        discard;
    }
    // Premultiplied output.
    return vec4<f32>(in.color.rgb * a, a);
}
