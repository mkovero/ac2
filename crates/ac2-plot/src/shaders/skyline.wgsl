// 3D spectrograph: one vertical spike per cell of a ring of level rows, drawn max-blended
// into an Rg8 target (R = normalised level, G = sqrt(brightness)), then composited over the
// plot as LUT(R) · G².
//
// Level and brightness stay apart until the composite: a max of RGB colours would mix hues
// no level has, while the max of R is always a level some spike under the pixel has. G holds
// the square root so the dim far rows keep more of the 8 bits; the composite filters it
// (half-resolution tiers) and squares it back.
//
// Bloom: the spikes again into a quarter-resolution Rgba8 target, summed (One/One add),
// each its colour times its light above a threshold; blurred by a separable Gaussian and
// added over the plot. Summing is what makes many rows standing together glow denser than
// one; the threshold keeps the dim bodies of stacked rows from summing into a haze.

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
    // The tallest spike in full heights (`HEIGHT_CAP`).
    height_cap: f32,
    // The bloom target's size (a quarter of the rect's, rounded up).
    bloom_size: vec2<f32>,
    bloom_threshold: f32,
    bloom_gain: f32,
    // The bloom's gain over the plot.
    glow_gain: f32,
    // Blur taps: 9 (point taps) or 5 (bilinear fetches folding the same kernel).
    taps: u32,
    _pad2: vec2<u32>,
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
    // The bloom pass's colour, LUT(level); unused by the spike pass.
    @location(6) @interpolate(flat) colour: vec3<f32>,
}

// World point to spike target pixels; w ≤ 0 never reaches here (the eye is outside the box).
fn to_target(p: vec3<f32>, size: vec2<f32>) -> vec3<f32> {
    let c = sky.view_proj * vec4<f32>(p, 1.0);
    let n = c.xy / c.w;
    return vec3<f32>((n.x * 0.5 + 0.5) * size.x, (0.5 - n.y * 0.5) * size.y, c.w);
}

// One spike as a quad in a target of `size` pixels, `scale` of them per host pixel. Its
// width is set in the spike target's pixels (`width_min`/`width_max` are those) and carried
// over to this target, so the bloom's spikes are the spikes' own width.
fn spike(vi: u32, ii: u32, size: vec2<f32>, scale: f32) -> SpikeOut {
    var out: SpikeOut;
    // Outside the target: a cell without a spike makes a degenerate quad there.
    out.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    out.across = 0.0;
    out.along = 0.0;
    out.len = 0.0;
    out.width = 0.0;
    out.level = 0.0;
    out.light = 0.0;
    out.colour = vec3<f32>(0.0);
    let col = ii % sky.cols;
    let j = ii / sky.cols;
    let ring = (sky.newest + sky.rows - j) % sky.rows;
    let v = textureLoad(levels, vec2<u32>(col, ring), 0).r;
    if (is_nan_bits(v)) {
        return out;
    }
    // Height in full heights: in proportion past the range top, up to the cap. The level
    // the target keeps (R, 8 bits) saturates at the top instead: the colour stops at the
    // colormap's top and the height carries the rest, so the range keeps all 256 steps.
    let h = clamp((v - sky.range.x) / (sky.range.y - sky.range.x), 0.0, sky.height_cap);
    if (h <= 0.0) {
        return out;
    }
    let n = min(h, 1.0);
    let hw = sky.world.x;
    let depth = sky.world.y;
    let pitch = 2.0 * hw / f32(sky.cols);
    let x = -hw + (f32(col) + 0.5) * pitch;
    let z = (f32(j) + sky.phase) * depth / f32(sky.rows);
    let foot = vec3<f32>(x, 0.0, z);
    let f = to_target(foot, size);
    let t = to_target(vec3<f32>(x, sky.world.z * h, z), size);
    let nb = to_target(vec3<f32>(x + pitch, 0.0, z), size);
    if (f.z <= 1e-3 || t.z <= 1e-3 || nb.z <= 1e-3) {
        return out;
    }
    // Pitch and width in spike target pixels, then in this target's.
    let k = scale / sky.down;
    let width = clamp(sky.width_share * length(nb.xy - f.xy) / k, sky.width_min, sky.width_max)
        * k;
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
    out.pos = vec4<f32>(p.x / size.x * 2.0 - 1.0, 1.0 - p.y / size.y * 2.0, 0.0, 1.0);
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

@vertex
fn vs_spike(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> SpikeOut {
    return spike(vi, ii, sky.target_size, sky.down);
}

// The light at `along` host pixels from the foot of a spike `len` host pixels long: a steep
// body and a brighter cap at the tip, clipped at 1 after the light so a dim far or low
// spike still gets its bright tip.
fn spike_light(along: f32, len: f32, light: f32) -> f32 {
    let t = clamp(along / max(len, 1e-4), 0.0, 1.0);
    var body = sky.foot + (1.0 - sky.foot) * pow(t, sky.body_pow);
    if (len - along <= sky.tip_px) {
        body = sky.tip_gain;
    }
    return min(body * light, 1.0);
}

// The pixel's share of a spike `width` wide centred `across` from the pixel centre, a box
// filter across the axis with the spike widened by `widen` each side.
fn coverage(across: f32, width: f32, widen: f32) -> f32 {
    let w = width * 0.5 + widen;
    return clamp(min(across + 0.5, w) - max(across - 0.5, -w), 0.0, 1.0);
}

@fragment
fn fs_spike(in: SpikeOut) -> @location(0) vec4<f32> {
    // A rasterizer lighting every pixel a spike touches (what the look was tuned on) lights
    // about width + ½ pixels a row; a quarter pixel wider each side keeps that light while
    // staying smooth as the spike moves across pixels.
    let cover = coverage(in.across, in.width, 0.25);
    if (cover <= 0.0) {
        discard;
    }
    let along = in.along / sky.down;
    let b = spike_light(along, in.len / sky.down, in.light);
    return vec4<f32>(in.level, sqrt(b * cover), 0.0, 0.0);
}

// ---- bloom ------------------------------------------------------------------------------

@group(1) @binding(4) var lut: texture_2d<f32>;

fn lut_colour(n: f32) -> vec3<f32> {
    let size = textureDimensions(lut).x;
    let idx = min(u32(round(n * f32(size - 1u))), size - 1u);
    return textureLoad(lut, vec2<u32>(idx, 0u), 0).rgb;
}

@vertex
fn vs_bloom(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> SpikeOut {
    var out = spike(vi, ii, sky.bloom_size, 0.25);
    if (out.level > 0.0) {
        out.colour = lut_colour(out.level);
    }
    return out;
}

@fragment
fn fs_bloom(in: SpikeOut) -> @location(0) vec4<f32> {
    // The spike's own quarter pixel of light widening, in host pixels.
    let cover = coverage(in.across, in.width, 0.0625);
    if (cover <= 0.0) {
        discard;
    }
    // A quarter-res pixel spans four host pixels along the spike: average their light, so
    // the tip cap (shorter than one such pixel) feeds the bloom wherever it falls.
    let len = in.len * 4.0;
    let c = in.along * 4.0;
    var sum = 0.0;
    for (var i = 0; i < 4; i++) {
        let a = c + f32(i) - 1.5;
        if (a >= 0.0 && a <= len) {
            sum += max(spike_light(a, len, in.light) - sky.bloom_threshold, 0.0);
        }
    }
    let lit = sum * 0.25 / (1.0 - sky.bloom_threshold);
    return vec4<f32>(in.colour * (lit * cover * sky.bloom_gain), 0.0);
}

@group(1) @binding(5) var bloom: texture_2d<f32>;
@group(1) @binding(3) var linear: sampler;

struct FullOut {
    @builtin(position) pos: vec4<f32>,
}

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FullOut {
    var out: FullOut;
    out.pos = vec4<f32>(
        select(-1.0, 1.0, (vi & 2u) != 0u),
        select(-1.0, 1.0, (vi & 1u) != 0u),
        0.0,
        1.0,
    );
    return out;
}

// A Gaussian of σ = 2 bloom pixels (≈ 8 host pixels), cut at ±4 and normalised.
const W0: f32 = 0.2042;
const W1: f32 = 0.1802;
const W2: f32 = 0.1238;
const W3: f32 = 0.0663;
const W4: f32 = 0.0276;

fn blur(pos: vec2<f32>, dir: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(bloom));
    if (sky.taps == 9u) {
        let hi = vec2<i32>(dims) - 1;
        let p = vec2<i32>(pos);
        let d = vec2<i32>(dir);
        var acc = textureLoad(bloom, p, 0) * W0;
        acc += (textureLoad(bloom, clamp(p + d, vec2<i32>(0), hi), 0)
            + textureLoad(bloom, clamp(p - d, vec2<i32>(0), hi), 0)) * W1;
        acc += (textureLoad(bloom, clamp(p + 2 * d, vec2<i32>(0), hi), 0)
            + textureLoad(bloom, clamp(p - 2 * d, vec2<i32>(0), hi), 0)) * W2;
        acc += (textureLoad(bloom, clamp(p + 3 * d, vec2<i32>(0), hi), 0)
            + textureLoad(bloom, clamp(p - 3 * d, vec2<i32>(0), hi), 0)) * W3;
        acc += (textureLoad(bloom, clamp(p + 4 * d, vec2<i32>(0), hi), 0)
            + textureLoad(bloom, clamp(p - 4 * d, vec2<i32>(0), hi), 0)) * W4;
        return acc;
    }
    // The same kernel in five fetches: each pair of taps read at the point between them
    // where the bilinear filter weighs them as the kernel does.
    let o1 = (W1 + 2.0 * W2) / (W1 + W2);
    let o2 = (3.0 * W3 + 4.0 * W4) / (W3 + W4);
    let uv = pos / dims;
    let du = dir / dims;
    var acc = textureSampleLevel(bloom, linear, uv, 0.0) * W0;
    acc += (textureSampleLevel(bloom, linear, uv + du * o1, 0.0)
        + textureSampleLevel(bloom, linear, uv - du * o1, 0.0)) * (W1 + W2);
    acc += (textureSampleLevel(bloom, linear, uv + du * o2, 0.0)
        + textureSampleLevel(bloom, linear, uv - du * o2, 0.0)) * (W3 + W4);
    return acc;
}

@fragment
fn fs_blur_h(in: FullOut) -> @location(0) vec4<f32> {
    return blur(in.pos.xy, vec2<f32>(1.0, 0.0));
}

@fragment
fn fs_blur_v(in: FullOut) -> @location(0) vec4<f32> {
    return blur(in.pos.xy, vec2<f32>(0.0, 1.0));
}

// ---- composite --------------------------------------------------------------------------

@group(1) @binding(2) var spikes: texture_2d<f32>;

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
    let g = textureSample(spikes, linear, local / dims).g;
    if (!inside_clip(in.pos.xy, sky.clip) || !inside_clip(in.pos.xy, sky.rect)) {
        discard;
    }
    let texel = vec2<u32>(clamp(local, vec2<f32>(0.0), dims - 1.0));
    let n = textureLoad(spikes, texel, 0).r;
    if (n <= 0.0) {
        discard;
    }
    let c = lut_colour(n) * (g * g);
    // Alpha is masked off: the host target's alpha stays as it was.
    return output(c, 1.0);
}

// The blurred bloom over the plot, added; bilinear, so its quarter-res pixels upscale
// smoothly.
@fragment
fn fs_glow(in: CompOut) -> @location(0) vec4<f32> {
    let uv = (in.pos.xy - sky.rect.xy) * 0.25 / vec2<f32>(textureDimensions(bloom));
    let c = textureSample(bloom, linear, uv).rgb;
    if (!inside_clip(in.pos.xy, sky.clip) || !inside_clip(in.pos.xy, sky.rect)) {
        discard;
    }
    return output(c * sky.glow_gain, 1.0);
}
