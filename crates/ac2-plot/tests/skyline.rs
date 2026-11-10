//! The 3D spectrograph's renderer: spike placement, max blending, coverage, upload by
//! identity and the goldens.
//!
//! A test binary of its own so it runs alone against a chosen backend: CI runs it again with
//! `WGPU_BACKEND=gl` (llvmpipe, the WebGL2-class limits a Pi's GLES gets), and the CM4 gate
//! runs it on the Pi with `WGPU_BACKEND=gl` and `WGPU_BACKEND=vulkan`:
//! `cargo nextest run -p ac2-plot --test skyline`.

#[path = "it/common/mod.rs"]
mod common;

use std::sync::Arc;

use ac2_plot::{
    Color, Colormap, FillRect, HeatmapColumn, Layer, Rect, Skyline3d, SkylineId, SkylineLook,
    SkylineTier,
};
use ac2_proto::model::layout::SpectrumCamera;
use ac2_scene::spectrograph_3d::{
    self, FIELD, FLOOR_DEPTH, FLOOR_HALF_W, LITE_WIDTH_PX, LOOK, MAX_HEIGHT, Pose, ViewProj,
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
            range: [-80.0, 0.0],
            phase: 1.0,
            look: LOOK,
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
    assert!(
        (0.25..0.6).contains(&ratio),
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
    let s = Sky::new(16, 8, pink_room(16, 8));
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

/// Three steady tones over a low noise floor, the middle one stopping half way back.
fn tonal(cols: u32, rows: u32) -> Vec<Option<HeatmapColumn>> {
    ring(
        (0..rows)
            .map(|j| {
                (0..cols)
                    .map(|c| {
                        let f = freq(c, cols);
                        let mut l = -72.0 + 3.0 * jitter(c, j, 3);
                        let tone =
                            |at: f32, level: f32| level + 60.0 * (gauss_oct(f, at, 0.02) - 1.0);
                        l = l.max(tone(100.0, -12.0)).max(tone(5000.0, -30.0));
                        if j < rows / 2 {
                            l = l.max(tone(1000.0, -20.0));
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
    // A level range narrowed around the noise turns the slab into terrain.
    let mut narrow = sky_for(SkylineTier::Full, pink_room);
    narrow.range = [-45.0, 0.0];
    golden_of("skyline_pink_room_narrow", &narrow);
    let mut orbit = sky_for(SkylineTier::Full, pink_room);
    orbit.cam = camera(-35, 30);
    golden_of("skyline_pink_room_orbit", &orbit);
}
