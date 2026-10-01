// Scrolling value texture through a colormap LUT (spectrograph).
//
// The ring is stored transposed: texel (row, column), so one time column is one texture row
// and a column upload is a contiguous write. Cells are drawn nearest-neighbour: each pixel
// shows the value of the cell its centre falls in, never an interpolated value.

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
    _pad: vec2<u32>,
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
    let u = (p.x - hm.rect.x) / size.x;
    let v = (hm.rect.w - p.y) / size.y;
    let col = min(u32(u * f32(hm.columns)), hm.columns - 1u);
    let row = min(u32(v * f32(hm.rows)), hm.rows - 1u);
    let ring = (col + hm.scroll) % hm.columns;
    let value = textureLoad(values, vec2<u32>(row, ring), 0).r;
    // "No data" is NaN; tested on the bit pattern because float NaN comparisons may be
    // folded away by shader compilers.
    if ((bitcast<u32>(value) & 0x7f800000u) == 0x7f800000u) {
        discard;
    }
    let t = clamp((value - hm.range.x) / (hm.range.y - hm.range.x), 0.0, 1.0);
    let n = textureDimensions(lut).x;
    let idx = min(u32(round(t * f32(n - 1u))), n - 1u);
    let c = textureLoad(lut, vec2<u32>(idx, 0u), 0);
    return output(c.rgb, c.a * hm.opacity);
}
