//! The 3D spectrograph's renderer: spike placement, max blending, coverage, upload by
//! identity, the bloom and the goldens; with `AC2_SKYLINE_BENCH=1`, the frame cost per tier
//! (`bench_frame_cost`).
//!
//! A test binary of its own so it runs alone against a chosen backend: CI runs it again with
//! `WGPU_BACKEND=gl` (llvmpipe, the WebGL2-class limits a GLES adapter gets):
//! `cargo nextest run -p ac2-plot --test skyline`.

#[path = "it/common/mod.rs"]
mod common;

use std::sync::Arc;

use ac2_plot::{
    Color, Colormap, FillRect, HeatmapColumn, Layer, PrepareError, Rect, Skyline3d, SkylineGlow,
    SkylineId, SkylineLook, SkylineTier,
};
use ac2_proto::model::layout::SpectrumCamera;
use ac2_scene::spectrograph_3d::{
    self, FIELD, FLOOR_DEPTH, FLOOR_HALF_W, LITE_WIDTH_PX, LOOK, MAX_HEIGHT, Pose, ViewProj,
    glow_gain,
};
use ac2_testkit::image::{Image, ImageTolerance};
use common::{golden, gpu, render_on, renderer, scene};

const W: f32 = 320.0;
const H: f32 = 200.0;

/// As the other line-like goldens: anti-aliased edges and float math differ between
/// rasterizers by a few LSB; a spike a pixel off moves a run of pixels by far more.
const LINES: ImageTolerance = ImageTolerance {
    channel: 24,
    max_bad_fraction: 0.002,
};

fn camera(yaw: i8, pitch: u8) -> SpectrumCamera {
    SpectrumCamera {
        yaw_deg: yaw,
        pitch_deg: pitch,
        zoom_step: 0,
    }
}

fn view(cam: SpectrumCamera) -> ViewProj {
    spectrograph_3d::view_proj(Pose::from(cam), f64::from(W / H))
}

struct Sky {
    cols: u32,
    rows: u32,
    data: Vec<Option<HeatmapColumn>>,
    tier: SkylineTier,
    colormap: Colormap,
    cam: SpectrumCamera,
    range: [f32; 2],
    phase: f32,
    look: SkylineLook,
    /// Adds the bloom, as the scene does for a tier that has it.
    glow: bool,
}

impl Sky {
    fn new(cols: u32, rows: u32, data: Vec<Option<HeatmapColumn>>) -> Self {
        Self {
            cols,
            rows,
            data,
            tier: SkylineTier::Full,
            colormap: Colormap::Ice,
            cam: SpectrumCamera::DEFAULT,
            range: [-100.0, 0.0],
            phase: 1.0,
            look: LOOK,
            glow: false,
        }
    }

    fn primitive(&self) -> Skyline3d {
        let vp = view(self.cam);
        let rect = Rect::new(0.0, 0.0, W, H);
        Skyline3d {
            id: SkylineId(1),
            rect,
            clip: Some(rect),
            cols: self.cols,
            rows: self.rows,
            newest: 0,
            phase: self.phase,
            view_proj: vp.gpu(),
            eye: vp.eye.map(|v| v as f32),
            world: [FLOOR_HALF_W as f32, FLOOR_DEPTH as f32, MAX_HEIGHT as f32],
            range: self.range,
            colormap: self.colormap,
            tier: self.tier,
            look: self.look,
            data: self.data.clone(),
        }
    }

    /// On the dark field when `field`, else on black.
    fn scene(&self, field: bool) -> ac2_plot::Scene {
        let mut l = Layer::default();
        if field {
            l.rects.push(FillRect {
                rect: Rect::new(0.0, 0.0, W, H),
                color: FIELD,
                clip: None,
            });
        }
        l.skylines.push(self.primitive());
        if self.glow && self.tier.bloom_taps().is_some() {
            l.glows.push(SkylineGlow {
                id: SkylineId(1),
                clip: Some(Rect::new(0.0, 0.0, W, H)),
                gain: glow_gain(self.colormap),
            });
        }
        scene(W, H, vec![l])
    }
}

fn draw(s: &Sky) -> Image {
    let gpu = gpu("skyline").expect("checked by the caller");
    render_on(gpu, &mut renderer(gpu), &s.scene(false), 1.0, Color::BLACK)
}

/// Ring with one row per entry of `rows` (ring position = entry), `cols` levels each.
fn ring(rows: Vec<Vec<f32>>) -> Vec<Option<HeatmapColumn>> {
    rows.into_iter().map(|r| Some(Arc::from(r))).collect()
}

/// One spike at column `c` of `cols`, row `j` back of `rows`, at `level`; the rest NaN.
fn lone(cols: u32, rows: u32, c: u32, j: u32, level: f32) -> Vec<Option<HeatmapColumn>> {
    let mut data = vec![None; rows as usize];
    let mut r = vec![f32::NAN; cols as usize];
    r[c as usize] = level;
    // newest is ring position 0, so row j back is at (rows − j) % rows.
    data[((rows - j) % rows) as usize] = Some(Arc::from(r));
    data
}

/// Device pixel of a world point in the 320×200 frame.
fn pixel(vp: &ViewProj, p: [f64; 3]) -> [f64; 2] {
    let n = vp.ndc(p).expect("in front of the eye");
    [
        (n[0] + 1.0) / 2.0 * f64::from(W),
        (1.0 - n[1]) / 2.0 * f64::from(H),
    ]
}

fn lit(p: [u8; 4]) -> bool {
    p[0] > 2 || p[1] > 2 || p[2] > 2
}

#[test]
fn a_spike_stands_where_the_scene_projects_it() {
    let Some(_) = gpu("a_spike_stands_where_the_scene_projects_it") else {
        return;
    };
    let (cols, rows) = (8, 4);
    let s = Sky::new(cols, rows, lone(cols, rows, 5, 0, 0.0));
    let img = draw(&s);
    let vp = view(s.cam);
    let x = -FLOOR_HALF_W + 5.5 * 2.0 * FLOOR_HALF_W / f64::from(cols);
    // Row 0 at phase 1 stands one row back.
    let z = FLOOR_DEPTH / f64::from(rows);
    let foot = pixel(&vp, [x, 0.0, z]);
    let tip = pixel(&vp, [x, MAX_HEIGHT, z]);
    let near_tip = [
        foot[0] + 0.97 * (tip[0] - foot[0]),
        foot[1] + 0.97 * (tip[1] - foot[1]),
    ];
    assert!(
        lit(img.pixel(near_tip[0] as u32, near_tip[1] as u32)),
        "nothing at {near_tip:?} (foot {foot:?}, tip {tip:?})"
    );
    // Everything lit lies on the segment, within its width and a pixel of coverage.
    let (dx, dy) = (tip[0] - foot[0], tip[1] - foot[1]);
    let len = dx.hypot(dy);
    let mut n = 0;
    for py in 0..H as u32 {
        for px in 0..W as u32 {
            if !lit(img.pixel(px, py)) {
                continue;
            }
            n += 1;
            let (cx, cy) = (f64::from(px) + 0.5 - foot[0], f64::from(py) + 0.5 - foot[1]);
            let along = (cx * dx + cy * dy) / len;
            let across = (cx * dy - cy * dx).abs() / len;
            assert!(
                (-1.0..=len + 1.0).contains(&along) && across <= 2.5,
                "lit pixel ({px},{py}) off the spike: along {along:.1}, across {across:.1}"
            );
        }
    }
    assert!(
        f64::from(n) > len * 0.8,
        "{n} lit pixels for a {len:.0} px spike"
    );
}

#[test]
fn overlapping_spikes_keep_the_brighter_never_the_sum() {
    let Some(_) = gpu("overlapping_spikes_keep_the_brighter_never_the_sum") else {
        return;
    };
    let (cols, rows) = (4, 64);
    let a = draw(&Sky::new(cols, rows, lone(cols, rows, 2, 10, -10.0)));
    let b = draw(&Sky::new(cols, rows, lone(cols, rows, 2, 11, -10.0)));
    let mut both_rows = vec![None; rows as usize];
    let r = |v: f32| -> Vec<f32> {
        (0..cols)
            .map(|c| if c == 2 { v } else { f32::NAN })
            .collect()
    };
    both_rows[(rows - 10) as usize] = Some(Arc::from(r(-10.0)));
    both_rows[(rows - 11) as usize] = Some(Arc::from(r(-10.0)));
    let both = draw(&Sky::new(cols, rows, both_rows));
    let mut overlap = 0;
    let mut summed_would_differ = 0;
    for y in 0..H as u32 {
        for x in 0..W as u32 {
            let (pa, pb, pc) = (a.pixel(x, y), b.pixel(x, y), both.pixel(x, y));
            for ch in 0..3 {
                let max = pa[ch].max(pb[ch]);
                assert!(
                    pc[ch].abs_diff(max) <= 2,
                    "({x},{y}) ch {ch}: {pc:?}, alone {pa:?} / {pb:?}"
                );
            }
            if lit(pa) && lit(pb) {
                overlap += 1;
                if u16::from(pa[2]) + u16::from(pb[2]) > u16::from(pc[2]) + 8 {
                    summed_would_differ += 1;
                }
            }
        }
    }
    assert!(overlap > 10, "the two spikes barely overlap ({overlap} px)");
    assert!(summed_would_differ > 0);
}

#[test]
fn a_spike_narrower_than_a_pixel_dims_and_keeps_its_hue() {
    let Some(_) = gpu("a_spike_narrower_than_a_pixel_dims_and_keeps_its_hue") else {
        return;
    };
    let (cols, rows) = (8, 4);
    let mut s = Sky::new(cols, rows, lone(cols, rows, 5, 0, -20.0));
    s.colormap = Colormap::Viridis;
    let at = |w: f32| {
        let mut s2 = Sky::new(s.cols, s.rows, s.data.clone());
        s2.colormap = s.colormap;
        s2.look.width_px = [w, w];
        draw(&s2)
    };
    let (one, thin) = (at(1.0), at(0.4));
    let total = |img: &Image| -> [u64; 3] {
        let mut t = [0u64; 3];
        for y in 0..H as u32 {
            for x in 0..W as u32 {
                let p = img.pixel(x, y);
                for c in 0..3 {
                    t[c] += u64::from(p[c]);
                }
            }
        }
        t
    };
    let (t1, t4) = (total(&one), total(&thin));
    let ratio = t4[1] as f64 / t1[1] as f64;
    // Light goes as the width plus the filter's half pixel: (0.4 + ½) / (1 + ½) = 0.6.
    assert!(
        (0.45..0.75).contains(&ratio),
        "a 0.4 px spike gives {ratio:.2} of a 1 px one's light"
    );
    // Same level, same hue: the channels keep their proportions.
    let hue = |t: [u64; 3]| [t[0] as f64 / t[1] as f64, t[2] as f64 / t[1] as f64];
    let (h1, h4) = (hue(t1), hue(t4));
    assert!(
        (h1[0] - h4[0]).abs() < 0.15 && (h1[1] - h4[1]).abs() < 0.15,
        "{h1:?} vs {h4:?}"
    );
}

#[test]
fn a_row_of_spikes_at_a_near_integer_pitch_leaves_no_dark_columns() {
    let Some(_) = gpu("a_row_of_spikes_at_a_near_integer_pitch_leaves_no_dark_columns") else {
        return;
    };
    // Seen from above, 300 columns over ~290 px: a pitch a little under 1 px wide spikes.
    // Snapped to whole pixels, some columns would be skipped and some doubled.
    let (cols, rows) = (300, 4);
    let mut data = vec![None; rows as usize];
    data[(rows - 1) as usize] = Some(Arc::from(vec![-10.0f32; cols as usize]));
    let mut s = Sky::new(cols, rows, data);
    s.cam = camera(0, 75);
    s.look.width_px = [1.0, 1.0];
    let img = draw(&s);
    let lit_cols: Vec<u64> = (0..W as u32)
        .map(|x| (0..H as u32).map(|y| u64::from(img.pixel(x, y)[1])).sum())
        .collect();
    let first = lit_cols
        .iter()
        .position(|&v| v > 0)
        .expect("something drawn");
    let last = lit_cols
        .iter()
        .rposition(|&v| v > 0)
        .expect("something drawn");
    let inner = &lit_cols[first + 3..last - 2];
    let max = *inner.iter().max().expect("columns") as f64;
    let min = *inner.iter().min().expect("columns") as f64;
    assert!(min / max > 0.35, "darkest column {min} of brightest {max}");
}

#[test]
fn the_host_alpha_is_left_as_it_was() {
    let Some(gpu) = gpu("the_host_alpha_is_left_as_it_was") else {
        return;
    };
    let mut s = Sky::new(16, 8, pink_room(16, 8));
    s.glow = true;
    let bg = Color::rgba(0.0, 0.0, 0.0, 0.5);
    let img = render_on(gpu, &mut renderer(gpu), &s.scene(false), 1.0, bg);
    let a = img.pixel(0, 0)[3];
    assert!((126..=129).contains(&a));
    let mut any = false;
    for y in 0..H as u32 {
        for x in 0..W as u32 {
            let p = img.pixel(x, y);
            assert_eq!(p[3], a, "({x},{y})");
            any |= lit(p);
        }
    }
    assert!(any);
}

#[test]
fn ring_rows_follow_identity() {
    let Some(gpu) = gpu("ring_rows_follow_identity") else {
        return;
    };
    let mut r = renderer(gpu);
    let (cols, rows) = (32, 16);
    let base = pink_room(cols, rows);
    let mut s = Sky::new(cols, rows, base.clone());
    s.glow = true;
    render_on(gpu, &mut r, &s.scene(false), 1.0, Color::BLACK);
    let mut changed = base.clone();
    changed[3] = base[9].clone();
    changed[5] = None;
    s.data = changed;
    let img = render_on(gpu, &mut r, &s.scene(false), 1.0, Color::BLACK);
    let fresh = render_on(gpu, &mut renderer(gpu), &s.scene(false), 1.0, Color::BLACK);
    assert_eq!(
        img, fresh,
        "a ring updated row by row differs from a fresh one"
    );
}

#[test]
fn an_empty_ring_draws_nothing() {
    let Some(gpu) = gpu("an_empty_ring_draws_nothing") else {
        return;
    };
    let mut s = Sky::new(8, 4, vec![None; 4]);
    s.range = [-80.0, 0.0];
    s.glow = true;
    let blank = render_on(gpu, &mut renderer(gpu), &s.scene(true), 1.0, Color::BLACK);
    // Every level at or below the range's bottom: no spike either.
    s.data = ring(vec![
        vec![-80.0; 8],
        vec![-90.0; 8],
        vec![f32::NAN; 8],
        vec![-100.0; 8],
    ]);
    let floor = render_on(gpu, &mut renderer(gpu), &s.scene(true), 1.0, Color::BLACK);
    let field = [
        (FIELD.r * 255.0).round() as u8,
        (FIELD.g * 255.0).round() as u8,
        (FIELD.b * 255.0).round() as u8,
        255,
    ];
    for img in [&blank, &floor] {
        for y in 0..H as u32 {
            for x in 0..W as u32 {
                let p = img.pixel(x, y);
                assert!(
                    (0..3).all(|c| p[c].abs_diff(field[c]) <= 1),
                    "({x},{y}) {p:?}"
                );
            }
        }
    }
}

#[test]
fn the_glow_spreads_the_tips_light_and_only_adds() {
    let Some(_) = gpu("the_glow_spreads_the_tips_light_and_only_adds") else {
        return;
    };
    // One spike alone is under one LSB of bloom: the glow is of rows standing together.
    let mut s = sky_for(SkylineTier::Full, tonal);
    s.glow = false;
    let sharp = draw(&s);
    s.glow = true;
    let glow = draw(&s);
    let mut beside = 0;
    for y in 0..H as u32 {
        for x in 0..W as u32 {
            let (a, b) = (sharp.pixel(x, y), glow.pixel(x, y));
            assert!(
                (0..3).all(|c| b[c] >= a[c].saturating_sub(1)),
                "({x},{y}) darkened"
            );
            if !lit(a) && lit(b) {
                beside += 1;
            }
        }
    }
    assert!(
        beside > 50,
        "the glow lights {beside} pixels the spikes leave dark"
    );
}

#[test]
fn no_glow_on_bare_and_a_glow_needs_its_skyline() {
    let Some(gpu) = gpu("no_glow_on_bare_and_a_glow_needs_its_skyline") else {
        return;
    };
    let mut s = sky_for(SkylineTier::Bare, pink_room);
    s.glow = false;
    let mut sc = s.scene(true);
    sc.layers[0].glows.push(SkylineGlow {
        id: SkylineId(1),
        clip: None,
        gain: 0.6,
    });
    let target = ac2_plot::FrameTarget::full([W as u32, H as u32], 1.0);
    let mut r = renderer(gpu);
    let mut enc = common::encoder(gpu);
    let e = r.prepare(&gpu.device, &gpu.queue, &mut enc, &sc, &target);
    assert!(matches!(e, Err(PrepareError::Skyline { .. })), "{e:?}");
    sc.layers[0].skylines.clear();
    let e = r.prepare(&gpu.device, &gpu.queue, &mut enc, &sc, &target);
    assert!(matches!(e, Err(PrepareError::Skyline { .. })), "{e:?}");
}

/// Synthetic data for a ring of `cols` × `rows`.
type Ring = fn(u32, u32) -> Vec<Option<HeatmapColumn>>;

// ---- goldens ----------------------------------------------------------------------------

/// Small deterministic jitter in ±1 for cell (c, j).
fn jitter(c: u32, j: u32, seed: u32) -> f32 {
    let mut h = c
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add(j.wrapping_mul(0x85EB_CA77))
        .wrapping_add(seed.wrapping_mul(0xC2B2_AE3D));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 32767.5 - 1.0
}

/// Frequency of column `c` of `cols`, log-spaced 20 Hz – 20 kHz.
fn freq(c: u32, cols: u32) -> f32 {
    20.0 * 1000f32.powf((c as f32 + 0.5) / cols as f32)
}

fn gauss_oct(f: f32, at: f32, width_oct: f32) -> f32 {
    let d = (f / at).log2() / width_oct;
    (-0.5 * d * d).exp()
}

/// A room on per-bin pink noise: −3 dB/octave, three low modes, a comb from one reflection
/// above 300 Hz, a crossover dip, a dB of frame-to-frame movement.
fn pink_room(cols: u32, rows: u32) -> Vec<Option<HeatmapColumn>> {
    ring(
        (0..rows)
            .map(|j| {
                (0..cols)
                    .map(|c| {
                        let f = freq(c, cols);
                        let modes = 7.0 * gauss_oct(f, 45.0, 0.08)
                            + 5.0 * gauss_oct(f, 71.0, 0.08)
                            + 4.0 * gauss_oct(f, 112.0, 0.1);
                        let tau = 0.0021;
                        let ph = std::f32::consts::TAU * f * tau;
                        let comb = if f > 300.0 {
                            20.0 * ((1.0 + 0.45 * ph.cos()).hypot(0.45 * ph.sin())).log10()
                        } else {
                            0.0
                        };
                        -24.0 - 3.0 * (f / 1000.0).log2() + modes + comb
                            - 8.0 * gauss_oct(f, 1200.0, 0.3)
                            + 1.2 * jitter(c, j, 1)
                    })
                    .collect()
            })
            .collect(),
    )
}

/// An RTA of pink noise: 31 flat third-octave plateaus with a few dB of movement each.
fn rta_pink(cols: u32, rows: u32) -> Vec<Option<HeatmapColumn>> {
    ring(
        (0..rows)
            .map(|j| {
                (0..cols)
                    .map(|c| {
                        let band = ((freq(c, cols) / 20.0).log2() * 3.0).floor() as u32;
                        -28.0 + 2.5 * jitter(band, j, 2)
                    })
                    .collect()
            })
            .collect(),
    )
}

/// Six tones over a low noise floor, each a few columns wide and swelling by ±6 dB over
/// time, as the look emulator's tonal case: ridges, not walls.
fn tonal(cols: u32, rows: u32) -> Vec<Option<HeatmapColumn>> {
    ring(
        (0..rows)
            .map(|j| {
                (0..cols)
                    .map(|c| {
                        let f = freq(c, cols);
                        // Noise smoothed over four rows, about 2 dB rms.
                        let noise: f32 = (0..4).map(|k| jitter(c, j + k, 3)).sum::<f32>() * 0.6;
                        let mut l = -88.0 + noise;
                        for (at, level) in [
                            (250.0f32, -12.0f32),
                            (630.0, -6.0),
                            (1000.0, -3.0),
                            (1600.0, -9.0),
                            (3150.0, -18.0),
                            (125.0, -24.0),
                        ] {
                            let k = (f / at).log2() * 19.0;
                            let swell = 6.0 * (j as f32 / 9.0 + at).sin();
                            l = l.max(level + swell - 3.0 * k * k);
                        }
                        l
                    })
                    .collect()
            })
            .collect(),
    )
}

fn sky_for(tier: SkylineTier, data: Ring) -> Sky {
    let (cols, rows) = spectrograph_3d::grid(tier);
    let (cols, rows) = (cols as u32, rows as u32 + 1);
    let mut s = Sky::new(cols, rows, data(cols, rows));
    s.tier = tier;
    s.phase = 0.4;
    s.glow = true;
    if tier != SkylineTier::Full {
        s.look.width_px = LITE_WIDTH_PX;
    }
    s
}

fn golden_of(name: &str, s: &Sky) {
    let Some(gpu) = gpu(name) else {
        return;
    };
    let img = render_on(gpu, &mut renderer(gpu), &s.scene(true), 1.0, Color::BLACK);
    golden(name, &img, LINES);
}

#[test]
fn skyline_goldens() {
    let cases: [(&str, Ring); 3] = [
        ("pink_room", pink_room),
        ("rta_pink", rta_pink),
        ("tonal", tonal),
    ];
    for (case, data) in cases {
        for (tier, tname) in [
            (SkylineTier::Full, "full"),
            (SkylineTier::Lite, "lite"),
            (SkylineTier::Bare, "bare"),
        ] {
            golden_of(&format!("skyline_{case}_ice_{tname}"), &sky_for(tier, data));
        }
        let mut v = sky_for(SkylineTier::Full, data);
        v.colormap = Colormap::Viridis;
        golden_of(&format!("skyline_{case}_viridis_full"), &v);
    }
    // A level range narrowed with its top kept at 0 turns the slab into terrain.
    let mut narrow = sky_for(SkylineTier::Full, pink_room);
    narrow.range = [-60.0, 0.0];
    golden_of("skyline_pink_room_narrow", &narrow);
    let mut orbit = sky_for(SkylineTier::Full, pink_room);
    orbit.cam = camera(-35, 30);
    golden_of("skyline_pink_room_orbit", &orbit);
}

/// Renders histories exported from the look emulator (`AC2_SKYLINE_PARITY=<dir>` holding
/// `<case>.f32`: rows × 192 levels, newest first) at 960×540 from the emulator's cameras
/// (default and tilted, eye 2.6 from the target), Full tier with its bloom (without it when
/// `AC2_SKYLINE_PARITY_NO_GLOW` is set), to set beside its pictures. Skipped
/// without the variable: a tool for look work, not a check.
#[test]
fn parity_renders() {
    let Some(dir) = std::env::var_os("AC2_SKYLINE_PARITY") else {
        return;
    };
    let Some(gpu) = gpu("parity_renders") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let glow = std::env::var_os("AC2_SKYLINE_PARITY_NO_GLOW").is_none();
    let (w, h) = (960.0f32, 540.0f32);
    let aspect = f64::from(w / h);
    let cols = 192u32;
    for (case, full, narrow) in [
        ("pink-room", [-100.0, 0.0], [-60.0, 0.0]),
        ("rta-pink", [-100.0, 0.0], [-45.0, -5.0]),
        ("tonal", [-100.0, 0.0], [-100.0, 0.0]),
    ] {
        let bytes = std::fs::read(dir.join(format!("{case}.f32"))).expect("exported history");
        let levels: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let rows = (levels.len() / cols as usize) as u32 + 1;
        let mut data: Vec<Option<HeatmapColumn>> = levels
            .chunks_exact(cols as usize)
            .map(|r| Some(Arc::from(r)))
            .collect();
        data.push(None);
        for (cname, yaw, pitch) in [("default", 0.0, 8.0), ("tilted", 25.0, 30.0)] {
            let at = Pose {
                yaw_deg: yaw,
                pitch_deg: pitch,
                zoom_step: 0.0,
            };
            let d0 = spectrograph_3d::fitted_distance(at, aspect);
            let pose = Pose {
                zoom_step: -spectrograph_3d::ZOOM_STEPS_PER_OCTAVE * (2.6 / d0).log2(),
                ..at
            };
            let vp = spectrograph_3d::view_proj(pose, aspect);
            for (vname, colormap, range) in [
                ("ice", Colormap::Ice, full),
                ("viridis", Colormap::Viridis, full),
                ("ice_narrow", Colormap::Ice, narrow),
            ] {
                let rect = Rect::new(0.0, 0.0, w, h);
                let mut l = Layer::default();
                l.rects.push(FillRect {
                    rect,
                    color: FIELD,
                    clip: None,
                });
                l.skylines.push(Skyline3d {
                    id: SkylineId(1),
                    rect,
                    clip: Some(rect),
                    cols,
                    rows,
                    newest: 0,
                    phase: 1e-3,
                    view_proj: vp.gpu(),
                    eye: vp.eye.map(|v| v as f32),
                    world: [FLOOR_HALF_W as f32, FLOOR_DEPTH as f32, MAX_HEIGHT as f32],
                    range,
                    colormap,
                    tier: SkylineTier::Full,
                    look: LOOK,
                    data: data.clone(),
                });
                if glow {
                    l.glows.push(SkylineGlow {
                        id: SkylineId(1),
                        clip: Some(rect),
                        gain: glow_gain(colormap),
                    });
                }
                let img = render_on(
                    gpu,
                    &mut renderer(gpu),
                    &scene(w, h, vec![l]),
                    1.0,
                    Color::BLACK,
                );
                img.save_png(&dir.join(format!("real_{case}_{cname}_{vname}.png")))
                    .expect("written");
            }
        }
    }
}

// ---- frame cost -------------------------------------------------------------------------

/// Frame cost per tier on this machine's adapter, printed as a table: a 1920×1080 pane of
/// the whole 3D view (fold, scene, labels, floor, spikes, bloom) over a 30 s history with
/// every spike near full height (the fill worst case), at the default camera and at the
/// clamp corners at the closest zoom (the worst of them reported). CPU is the frame's work
/// up to the submit; GPU is timestamp queries around the frame's passes where the adapter
/// has them, else the wall time from submit to idle. Run with `AC2_SKYLINE_BENCH=1` (and
/// `--nocapture` to see the table); optimised builds give the meaningful CPU numbers.
#[test]
fn bench_frame_cost() {
    use ac2_plot::{FrameTarget, Gpu, RenderShared, Renderer, offscreen::OFFSCREEN_FORMAT, wgpu};
    use ac2_proto::GridDef;
    use ac2_proto::frame::ValidityMask;
    use ac2_proto::model::LevelScale;
    use ac2_proto::units::{Hz, WallNs};
    use ac2_scene::Viewport;
    use ac2_scene::axis::Range;
    use ac2_scene::banner::Status;
    use ac2_scene::spectrograph::{SpectrographFrame, SpectrographHistory};
    use ac2_scene::spectrograph_3d::{Skyline, Spectrograph3dInput, spectrograph_3d_scene};
    use ac2_scene::theme::Theme;
    use ac2_scene::view::ViewState;
    use std::time::{Duration, Instant};

    if std::env::var_os("AC2_SKYLINE_BENCH").is_none() {
        return;
    }
    let gpu = Gpu::with_features(wgpu::Features::TIMESTAMP_QUERY).expect("adapter");
    let stamps = gpu
        .device
        .features()
        .contains(wgpu::Features::TIMESTAMP_QUERY);
    println!(
        "adapter: {}; GPU time by {}",
        gpu.describe(),
        if stamps {
            "timestamp queries"
        } else {
            "wall time, submit to idle"
        }
    );
    let (w, h) = (1920u32, 1080u32);
    let shared = RenderShared::new(
        &gpu.device,
        OFFSCREEN_FORMAT,
        wgpu::MultisampleState::default(),
    );
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let queries = stamps.then(|| {
        let set = gpu.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("bench"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let buf = |usage| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bench"),
                size: 16,
                usage,
                mapped_at_creation: false,
            })
        };
        (
            set,
            buf(wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
            buf(wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST),
        )
    });
    let period = f64::from(gpu.queue.get_timestamp_period());

    let grid = GridDef::LogBins {
        fs: Hz(48_000.0),
        n: 65_536,
        ppo: 96,
    };
    let (edges, grid_id) = (grid.edges(), grid.id());
    const SLOT_NS: u64 = 1_000_000_000 / 30;
    const T0: u64 = 1_000 * 1_000_000_000;
    let mut seq = 0u64;
    let mut push = |hist: &mut SpectrographHistory, slot: u64| {
        seq += 1;
        let level: Vec<f32> = (0..edges.len() as u32)
            .map(|i| -1.0 + 0.5 * jitter(i, slot as u32, 7))
            .collect();
        hist.push(&SpectrographFrame {
            seq,
            at: WallNs(T0 + slot * SLOT_NS + SLOT_NS / 2),
            grid: grid_id,
            edges: &edges,
            scale: LevelScale::Dbfs,
            spec_level: None,
            level: &level,
            validity: None::<&[ValidityMask]>,
        });
    };
    let freq = Range::new(20.0, 20_000.0);
    let size = Viewport {
        width: w as f32,
        height: h as f32,
    };
    let median = |mut v: Vec<Duration>| {
        v.sort();
        v[v.len() / 2]
    };
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let warm = 5;
    let frames: usize = std::env::var("AC2_SKYLINE_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let mut cams = vec![("default", SpectrumCamera::DEFAULT)];
    let z = *SpectrumCamera::ZOOM_STEPS.end();
    for yaw in [-SpectrumCamera::YAW_MAX_DEG, 0, SpectrumCamera::YAW_MAX_DEG] {
        for pitch in [
            *SpectrumCamera::PITCH_DEG.start(),
            *SpectrumCamera::PITCH_DEG.end(),
        ] {
            cams.push((
                "corner",
                SpectrumCamera {
                    yaw_deg: yaw,
                    pitch_deg: pitch,
                    zoom_step: z,
                },
            ));
        }
    }
    println!("tier  camera                 CPU ms  GPU ms   (median of {frames} frames)");
    for tier in [SkylineTier::Full, SkylineTier::Lite, SkylineTier::Bare] {
        let mut hist = SpectrographHistory::new(30);
        let mut slot = 0u64;
        for _ in 0..hist.slots() {
            push(&mut hist, slot);
            slot += 1;
        }
        let mut sky = Skyline::default();
        sky.update(&hist, freq, tier);
        let mut r = Renderer::new(&gpu.device, &gpu.queue, &shared);
        let mut worst: Option<(String, f64, f64)> = None;
        for (name, cam) in &cams {
            let (mut cpu, mut gpu_t) = (Vec::new(), Vec::new());
            for i in 0..warm + frames {
                let t0 = Instant::now();
                push(&mut hist, slot);
                slot += 1;
                sky.update(&hist, freq, tier);
                let input = Spectrograph3dInput {
                    history: &hist,
                    skyline: &sky,
                    name: "Main L".into(),
                    range: Range::new(-100.0, 0.0),
                    offset_db: 0.0,
                    freshness: None,
                    colours: Colormap::Ice,
                    camera: Pose::from(*cam),
                    tier,
                    tier_forced: false,
                    axes: 1.0,
                };
                let sc = spectrograph_3d_scene(
                    &[],
                    &Status::default(),
                    Some(&input),
                    &ViewState::default(),
                    &Theme::dark(),
                    size,
                )
                .scene;
                let mut enc = gpu
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                let pass =
                    |enc: &mut wgpu::CommandEncoder, load, index: (Option<u32>, Option<u32>)| {
                        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("bench"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: queries.as_ref().map(|(set, _, _)| {
                                wgpu::RenderPassTimestampWrites {
                                    query_set: set,
                                    beginning_of_pass_write_index: index.0,
                                    end_of_pass_write_index: index.1,
                                }
                            }),
                            occlusion_query_set: None,
                            multiview_mask: None,
                        })
                        .forget_lifetime()
                    };
                drop(pass(
                    &mut enc,
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    (Some(0), None),
                ));
                r.prepare(
                    &gpu.device,
                    &gpu.queue,
                    &mut enc,
                    &sc,
                    &FrameTarget::full([w, h], 1.0),
                )
                .expect("prepare");
                {
                    let mut p = pass(&mut enc, wgpu::LoadOp::Load, (None, Some(1)));
                    r.paint(&mut p).expect("paint");
                }
                if let Some((set, resolve, read)) = &queries {
                    enc.resolve_query_set(set, 0..2, resolve, 0);
                    enc.copy_buffer_to_buffer(resolve, 0, read, 0, 16);
                }
                gpu.queue.submit([enc.finish()]);
                let t1 = Instant::now();
                let wall = if let Some((_, _, read)) = &queries {
                    read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                    gpu.device
                        .poll(wgpu::PollType::wait_indefinitely())
                        .expect("poll");
                    let t = {
                        let v = read.slice(..).get_mapped_range().expect("mapped");
                        let t = stamps_of(&v);
                        Duration::from_nanos((t[1].saturating_sub(t[0]) as f64 * period) as u64)
                    };
                    read.unmap();
                    t
                } else {
                    gpu.device
                        .poll(wgpu::PollType::wait_indefinitely())
                        .expect("poll");
                    t1.elapsed()
                };
                if i >= warm {
                    cpu.push(t1 - t0);
                    gpu_t.push(wall);
                }
            }
            let (c, g) = (ms(median(cpu)), ms(median(gpu_t)));
            let label = format!("{name} {}/{}/{}", cam.yaw_deg, cam.pitch_deg, cam.zoom_step);
            if *name == "default" {
                println!("{:5} {label:22} {c:6.2}  {g:6.2}", tier_name(tier));
            } else if worst.as_ref().is_none_or(|w| g > w.2) {
                worst = Some((label, c, g));
            }
        }
        if let Some((label, c, g)) = worst {
            println!(
                "{:5} {:22} {c:6.2}  {g:6.2}",
                tier_name(tier),
                format!("worst {label}")
            );
        }
    }
}

fn tier_name(t: SkylineTier) -> &'static str {
    match t {
        SkylineTier::Full => "Full",
        SkylineTier::Lite => "Lite",
        SkylineTier::Bare => "Bare",
    }
}

/// The two timestamps of a mapped query result.
fn stamps_of(b: &[u8]) -> [u64; 2] {
    let at = |i: usize| u64::from_ne_bytes(b[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
    [at(0), at(1)]
}
