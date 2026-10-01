// Shared by every ac2-plot pipeline; prepended to each shader source.

struct Globals {
    // Render target size in physical pixels.
    target_size: vec2<f32>,
    // 1 when the target format is sRGB: the hardware then encodes on write, so the
    // display-encoded scene colours must be decoded first to come out unchanged.
    srgb_target: u32,
    _pad: u32,
}

@group(0) @binding(0) var<uniform> globals: Globals;

fn to_ndc(px: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        px.x / globals.target_size.x * 2.0 - 1.0,
        1.0 - px.y / globals.target_size.y * 2.0,
    );
}

// Pixel centre test against [x0, x1) × [y0, y1); matches the scissor rounding on the CPU.
fn inside_clip(p: vec2<f32>, clip: vec4<f32>) -> bool {
    return p.x >= clip.x && p.y >= clip.y && p.x < clip.z && p.y < clip.w;
}

fn srgb_decode(c: f32) -> f32 {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

// Premultiplied output of a display-encoded colour with opacity a.
fn output(rgb: vec3<f32>, a: f32) -> vec4<f32> {
    var c = rgb;
    if (globals.srgb_target != 0u) {
        c = vec3<f32>(srgb_decode(c.r), srgb_decode(c.g), srgb_decode(c.b));
    }
    return vec4<f32>(c * a, a);
}
