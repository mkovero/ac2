// 3D spectrograph: one vertical spike per cell of a ring of level rows, drawn max-blended
// into an Rg8 target (R = normalised level, G = sqrt(brightness)), then composited over the
// plot as LUT(R) · G².
//
// Level and brightness stay apart until the composite: a max of RGB colours would mix hues
// no level has, while the max of R is always a level some spike under the pixel has. G holds
// the square root so the dim far rows keep more of the 8 bits; the composite filters it
// (half-resolution tiers) and squares it back.

struct Sky {
    view_proj: mat4x4<f32>,
    // Plot rect and clip in the host target's physical pixels.
    rect: vec4<f32>,
    clip: vec4<f32>,
    // The eye; w: the floor's nearest depth along the view (its nearest corner's clip w).
    eye: vec4<f32>,
    // Floor half width, depth, full height.
    world: vec4<f32>,
    // Levels at height 0 and full height.
    range: vec2<f32>,
    phase: f32,
    cols: u32,
    rows: u32,
    newest: u32,
    // The spike target's size: the rect's size times `down`.
    target_size: vec2<f32>,
    foot: f32,
    body_pow: f32,
    tip_px: f32,
    tip_gain: f32,
    level_floor: f32,
    age_fade: f32,
    eye_fade: f32,
    width_share: f32,
    width_min: f32,
    width_max: f32,
    // Spike target pixels per host pixel: 1 or 0.5.
    down: f32,
    _pad: u32,
}

@group(1) @binding(0) var<uniform> sky: Sky;

fn is_nan_bits(v: f32) -> bool {
    return (bitcast<u32>(v) & 0x7f800000u) == 0x7f800000u;
}

// ---- spike pass -------------------------------------------------------------------------

@group(1) @binding(1) var levels: texture_2d<f32>;

struct SpikeOut {
    @builtin(position) pos: vec4<f32>,
    // Distance across the spike's axis and along it from the foot, spike target pixels.
    @location(0) across: f32,
    @location(1) along: f32,
    @location(2) @interpolate(flat) len: f32,
    @location(3) @interpolate(flat) width: f32,
    @location(4) @interpolate(flat) level: f32,
    @location(5) @interpolate(flat) light: f32,
}

// World point to spike target pixels; w ≤ 0 never reaches here (the eye is outside the box).
fn to_target(p: vec3<f32>) -> vec3<f32> {
    let c = sky.view_proj * vec4<f32>(p, 1.0);
    let n = c.xy / c.w;
    return vec3<f32>(
        (n.x * 0.5 + 0.5) * sky.target_size.x,
        (0.5 - n.y * 0.5) * sky.target_size.y,
        c.w,
    );
}

@vertex
fn vs_spike(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> SpikeOut {
    var out: SpikeOut;
    // Outside the target: a cell without a spike makes a degenerate quad there.
    out.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    out.across = 0.0;
    out.along = 0.0;
    out.len = 0.0;
    out.width = 0.0;
    out.level = 0.0;
    out.light = 0.0;
    let col = ii % sky.cols;
    let j = ii / sky.cols;
    let ring = (sky.newest + sky.rows - j) % sky.rows;
    let v = textureLoad(levels, vec2<u32>(col, ring), 0).r;
    if (is_nan_bits(v)) {
        return out;
    }
    let n = clamp((v - sky.range.x) / (sky.range.y - sky.range.x), 0.0, 1.0);
    if (n <= 0.0) {
        return out;
    }
    let hw = sky.world.x;
    let depth = sky.world.y;
    let pitch = 2.0 * hw / f32(sky.cols);
    let x = -hw + (f32(col) + 0.5) * pitch;
    let z = (f32(j) + sky.phase) * depth / f32(sky.rows);
    let foot = vec3<f32>(x, 0.0, z);
    let f = to_target(foot);
    let t = to_target(vec3<f32>(x, sky.world.z * n, z));
    let nb = to_target(vec3<f32>(x + pitch, 0.0, z));
    if (f.z <= 1e-3 || t.z <= 1e-3 || nb.z <= 1e-3) {
        return out;
    }
    let width = clamp(sky.width_share * length(nb.xy - f.xy), sky.width_min, sky.width_max);
    let axis = t.xy - f.xy;
    let len = length(axis);
    let dir = select(vec2<f32>(0.0, -1.0), axis / len, len > 1e-4);
    let perp = vec2<f32>(-dir.y, dir.x);
    // Half a pixel beyond the spike's edge each side, so a fractional width keeps its
    // coverage wherever the axis falls: no snapping, no moiré against the pixel grid.
    let half = width * 0.5 + 1.0;
    let s = f32(vi & 1u);
    let side = select(-1.0, 1.0, (vi & 2u) != 0u);
    let p = mix(f.xy, t.xy, s) + perp * side * half;
    out.pos = vec4<f32>(
        p.x / sky.target_size.x * 2.0 - 1.0,
        1.0 - p.y / sky.target_size.y * 2.0,
        0.0,
        1.0,
    );
    out.across = side * half;
    out.along = s * len;
    out.len = len;
    out.width = width;
    out.level = n;
    // Depth fade from the floor's nearest point, not from the eye: the front row is at full
    // light whatever the dolly, and only the depth the floor spans dims the back.
    out.light = (sky.level_floor + (1.0 - sky.level_floor) * n)
        * exp(-sky.age_fade * z / depth)
        * exp(-sky.eye_fade * max(f.z - sky.eye.w, 0.0));
    return out;
}

@fragment
fn fs_spike(in: SpikeOut) -> @location(0) vec4<f32> {
    // The pixel's share of the spike's width, a box filter across the axis, with the spike a
    // quarter pixel wider each side: a rasterizer lighting every pixel a spike touches
    // (what the look was tuned on) lights about width + ½ pixels a row, and this keeps that
    // light while staying smooth as the spike moves across pixels.
    let w = in.width * 0.5 + 0.25;
    let cover = clamp(min(in.across + 0.5, w) - max(in.across - 0.5, -w), 0.0, 1.0);
    if (cover <= 0.0) {
        discard;
    }
    let t = clamp(in.along / max(in.len, 1e-4), 0.0, 1.0);
    var body = sky.foot + (1.0 - sky.foot) * pow(t, sky.body_pow);
    // The tip cap is brighter than the body's top, and clips only after the light: a dim far
    // or low spike still gets its bright tip.
    if (in.len - in.along <= sky.tip_px * sky.down) {
        body = sky.tip_gain;
    }
    return vec4<f32>(in.level, sqrt(min(body * in.light, 1.0) * cover), 0.0, 0.0);
}

// ---- composite --------------------------------------------------------------------------

@group(1) @binding(2) var spikes: texture_2d<f32>;
@group(1) @binding(3) var spikes_linear: sampler;
@group(1) @binding(4) var lut: texture_2d<f32>;

struct CompOut {
    @builtin(position) pos: vec4<f32>,
}

@vertex
fn vs_comp(@builtin(vertex_index) vi: u32) -> CompOut {
    let r = vec4<f32>(max(sky.rect.xy, sky.clip.xy), min(sky.rect.zw, sky.clip.zw));
    let x = select(r.x, r.z, (vi & 2u) != 0u);
    let y = select(r.y, r.w, (vi & 1u) != 0u);
    var out: CompOut;
    out.pos = vec4<f32>(to_ndc(vec2<f32>(x, y)), 0.0, 1.0);
    return out;
}

@fragment
fn fs_comp(in: CompOut) -> @location(0) vec4<f32> {
    let local = (in.pos.xy - sky.rect.xy) * sky.down;
    let dims = vec2<f32>(textureDimensions(spikes));
    // Brightness filtered (the half-resolution tiers upscale smoothly), level never: a
    // filtered level would be a level no spike has.
    let g = textureSample(spikes, spikes_linear, local / dims).g;
    if (!inside_clip(in.pos.xy, sky.clip) || !inside_clip(in.pos.xy, sky.rect)) {
        discard;
    }
    let texel = vec2<u32>(clamp(local, vec2<f32>(0.0), dims - 1.0));
    let n = textureLoad(spikes, texel, 0).r;
    if (n <= 0.0) {
        discard;
    }
    let size = textureDimensions(lut).x;
    let idx = min(u32(round(n * f32(size - 1u))), size - 1u);
    let c = textureLoad(lut, vec2<u32>(idx, 0u), 0).rgb * (g * g);
    // Alpha is masked off: the host target's alpha stays as it was.
    return output(c, 1.0);
}
