// Point clouds: one instanced quad per point, a soft disc in the fragment shader. Coverage
// falls linearly from 1 at the centre to 0 at the rim, so a 2–3 px point reads as a glowing
// grain rather than a hard square; the pipeline's blend state (additive or source-over) is
// chosen per `Points` primitive, the shader is the same.

struct VsIn {
    @location(0) centre: vec2<f32>,
    @location(1) clip: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) radius: f32,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) centre: vec2<f32>,
    @location(1) @interpolate(flat) clip: vec4<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) radius: f32,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, f: VsIn) -> VsOut {
    // One pixel of margin so the rim's partial pixels are rasterized.
    let m = f.radius + 1.0;
    let d = vec2<f32>(select(-m, m, (vi & 2u) != 0u), select(-m, m, (vi & 1u) != 0u));
    var out: VsOut;
    out.pos = vec4<f32>(to_ndc(f.centre + d), 0.0, 1.0);
    out.centre = f.centre;
    out.clip = f.clip;
    out.color = f.color;
    out.radius = f.radius;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.pos.xy;
    if (!inside_clip(p, in.clip)) {
        discard;
    }
    let cov = clamp(1.0 - distance(p, in.centre) / in.radius, 0.0, 1.0);
    let a = in.color.a * cov;
    if (a <= 0.0) {
        discard;
    }
    return output(in.color.rgb, a);
}
