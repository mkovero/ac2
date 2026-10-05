// Scrolling value texture through a colormap LUT (spectrograph).
//
// The ring is stored transposed: texel (row, column), so one time column is one texture row
// and a column upload is a contiguous write. Values are never interpolated: a pixel larger
// than a cell shows the cell its centre falls in; a pixel covering several cells shows the
// highest of them (up to MAX_TAPS per axis, spread evenly beyond that), so a short event or
// a narrow tone is not lost between pixels.

struct Heatmap {
    // Destination rect [x0, y0, x1, y1] in physical pixels.
    rect: vec4<f32>,
    clip: vec4<f32>,
    // Values mapped to LUT entry 0 and the last entry.
    range: vec2<f32>,
    opacity: f32,
    columns: u32,
    rows: u32,
    scroll: u32,
    // 1: columns run bottom to top and rows left to right.
    time_up: u32,
    _pad: u32,
}

const MAX_TAPS: u32 = 4u;

// Cells `[first, first + count)` of `n` covered by the span `[a0, a1)` of the unit
// interval: the one under `centre` when a cell is at least the span's size.
fn cells(a0: f32, a1: f32, centre: f32, n: u32) -> vec2<u32> {
    let nf = f32(n);
    if ((a1 - a0) * nf <= 1.0) {
        return vec2<u32>(min(u32(max(centre, 0.0) * nf), n - 1u), 1u);
    }
    let first = min(u32(max(a0, 0.0) * nf), n - 1u);
    let end = clamp(u32(ceil(max(a1, 0.0) * nf)), first + 1u, n);
    return vec2<u32>(first, end - first);
}

fn is_nan_bits(v: f32) -> bool {
    // Tested on the bit pattern because float NaN comparisons may be folded away by shader
    // compilers.
    return (bitcast<u32>(v) & 0x7f800000u) == 0x7f800000u;
}

@group(1) @binding(0) var<uniform> hm: Heatmap;
@group(1) @binding(1) var values: texture_2d<f32>;
@group(1) @binding(2) var lut: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let r = vec4<f32>(max(hm.rect.xy, hm.clip.xy), min(hm.rect.zw, hm.clip.zw));
    let x = select(r.x, r.z, (vi & 2u) != 0u);
    let y = select(r.y, r.w, (vi & 1u) != 0u);
    var out: VsOut;
    out.pos = vec4<f32>(to_ndc(vec2<f32>(x, y)), 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.pos.xy;
    if (!inside_clip(p, hm.clip) || !inside_clip(p, hm.rect)) {
        discard;
    }
    let size = hm.rect.zw - hm.rect.xy;
    // The pixel's extent along each axis as fractions of the rect; v runs bottom to top.
    let u0 = (p.x - 0.5 - hm.rect.x) / size.x;
    let u1 = (p.x + 0.5 - hm.rect.x) / size.x;
    let v0 = (hm.rect.w - p.y - 0.5) / size.y;
    let v1 = (hm.rect.w - p.y + 0.5) / size.y;
    let uc = (p.x - hm.rect.x) / size.x;
    let vc = (hm.rect.w - p.y) / size.y;
    var cs: vec2<u32>;
    var rs: vec2<u32>;
    if (hm.time_up != 0u) {
        cs = cells(v0, v1, vc, hm.columns);
        rs = cells(u0, u1, uc, hm.rows);
    } else {
        cs = cells(u0, u1, uc, hm.columns);
        rs = cells(v0, v1, vc, hm.rows);
    }
    let ct = min(cs.y, MAX_TAPS);
    let rt = min(rs.y, MAX_TAPS);
    var value = 0.0;
    var found = false;
    for (var i = 0u; i < ct; i++) {
        let col = cs.x + (i * cs.y) / ct;
        let ring = (col + hm.scroll) % hm.columns;
        for (var j = 0u; j < rt; j++) {
            let row = rs.x + (j * rs.y) / rt;
            let x = textureLoad(values, vec2<u32>(row, ring), 0).r;
            if (!is_nan_bits(x) && (!found || x > value)) {
                value = x;
                found = true;
            }
        }
    }
    // "No data" is NaN: transparent.
    if (!found) {
        discard;
    }
    let t = clamp((value - hm.range.x) / (hm.range.y - hm.range.x), 0.0, 1.0);
    let n = textureDimensions(lut).x;
    let idx = min(u32(round(t * f32(n - 1u))), n - 1u);
    let c = textureLoad(lut, vec2<u32>(idx, 0u), 0);
    return output(c.rgb, c.a * hm.opacity);
}
