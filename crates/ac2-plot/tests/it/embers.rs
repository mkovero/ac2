//! The goniometer's embers: the five figures of the design (left only, right only, mono,
//! reversed, uncorrelated noise) from synthetic rings built as the core would (45° rotation,
//! auto gain, `i8`), at two cooling settings. Blessed on lavapipe (`AC2_BLESS=1`).

use crate::common;

use ac2_plot::{Blend, Color, Layer, Rect, Stroke, ember_points};
use ac2_scene::gonio::{self, EmberRing, WirePoint};
use ac2_testkit::image::{Image, ImageTolerance};
use common::{golden, gpu, render, renderer, scene};

/// Thousands of soft additive discs: rasterizers differ at every rim, by a few LSB each,
/// and the sums differ by rounding; a point in the wrong place is a run of pixels off by
/// far more.
const EMBERS: ImageTolerance = ImageTolerance {
    channel: 32,
    max_bad_fraction: 0.01,
};

const PANEL: f32 = 120.0;
const GAP: f32 = 8.0;
const FRAMES: usize = 32;
const POINTS: usize = 256;
const FRAME_S: f64 = 1.0 / 30.0;
const NOW: f64 = 100.0;

#[derive(Clone, Copy, Debug)]
enum Figure {
    LeftOnly,
    RightOnly,
    Mono,
    Reversed,
    Noise,
}

const FIGURES: [Figure; 5] = [
    Figure::LeftOnly,
    Figure::RightOnly,
    Figure::Mono,
    Figure::Reversed,
    Figure::Noise,
];

/// Deterministic uniform in `0 … 1`.
fn lcg(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*state >> 8) as f32 / (1u32 << 24) as f32
}

fn sample(fig: Figure, t: f64, rng: &mut u32) -> (f32, f32) {
    let s = (2.0 * std::f64::consts::PI * 1000.0 * t).sin() as f32;
    match fig {
        Figure::LeftOnly => (s, 0.0),
        Figure::RightOnly => (0.0, s),
        Figure::Mono => (s, s),
        Figure::Reversed => (s, -s),
        Figure::Noise => (lcg(rng) * 2.0 - 1.0, lcg(rng) * 2.0 - 1.0),
    }
}

fn q(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}

/// One frame of wire points: 256 bins, one sample pair from a random position in each,
/// rotated 45°, normalised by the window peak (1 here), intensity the pair's level.
fn frame(fig: Figure, start_s: f64, rng: &mut u32) -> Vec<WirePoint> {
    (0..POINTS)
        .map(|i| {
            let t = start_s + (i as f64 + f64::from(lcg(rng))) / POINTS as f64 * FRAME_S;
            let (l, r) = sample(fig, t, rng);
            let x = (r - l) / 2f32.sqrt();
            let y = (l + r) / 2f32.sqrt();
            [q(x), q(y), q(l.abs().max(r.abs()))]
        })
        .collect()
}

/// A ring holding the last second of `fig`: 32 frames whose ages at [`NOW`] run 0 … 1 s.
fn ring(fig: Figure) -> EmberRing {
    let mut rng = 0x5eed_1234 ^ fig as u32;
    let mut ring = EmberRing::new(POINTS * FRAMES);
    for f in 0..FRAMES {
        let landed = NOW - 1.0 + f as f64 / (FRAMES - 1) as f64;
        ring.push_frame(landed, &frame(fig, landed, &mut rng));
    }
    ring
}

const GRID: Stroke = Stroke::solid(Color::rgba(1.0, 1.0, 1.0, 0.12), 1.0);

/// The goniometer of `ring` in `rect`: the grid when `chrome` (hidden by **T**), then the
/// embers.
fn gonio_layer(ring: &EmberRing, decay_s: f32, rect: Rect, chrome: bool) -> Layer {
    let square = gonio::square(rect);
    let mut layer = Layer::default();
    if chrome {
        layer.polylines = gonio::grid_polylines(square, GRID);
    }
    layer.points.push(ember_points(ring, NOW, decay_s, square));
    layer
}

fn panel_rect(i: usize) -> Rect {
    Rect::new(GAP + (PANEL + GAP) * i as f32, GAP, PANEL, PANEL)
}

fn five_figures(decay_s: f32) -> ac2_plot::Scene {
    let layers = FIGURES
        .iter()
        .enumerate()
        .map(|(i, &fig)| gonio_layer(&ring(fig), decay_s, panel_rect(i), true))
        .collect();
    scene(
        GAP + (PANEL + GAP) * FIGURES.len() as f32,
        PANEL + 2.0 * GAP,
        layers,
    )
}

/// Red light above the background over a region (embers are red-heavy at every stage).
fn glow(img: &Image, r: [u32; 4]) -> u64 {
    let bg = common::BG.to_rgba8()[0];
    let mut s = 0;
    for y in r[1]..r[3] {
        for x in r[0]..r[2] {
            s += u64::from(img.pixel(x, y)[0].saturating_sub(bg));
        }
    }
    s
}

/// `[x0, y0, x1, y1]` of a fraction of panel `i`.
fn region(i: usize, fx0: f32, fy0: f32, fx1: f32, fy1: f32) -> [u32; 4] {
    let p = panel_rect(i);
    let r = |v: f32| v.round() as u32;
    [
        r(p.x + fx0 * PANEL),
        r(p.y + fy0 * PANEL),
        r(p.x + fx1 * PANEL),
        r(p.y + fy1 * PANEL),
    ]
}

/// The shapes the design names, read off the pixels, not the PNG.
fn assert_figures(img: &Image) {
    let g = |i, a, b, c, d| glow(img, region(i, a, b, c, d));
    // Left only: a line leaning left, its top in the left half.
    let (tl, tr) = (g(0, 0.0, 0.0, 0.5, 0.45), g(0, 0.5, 0.0, 1.0, 0.45));
    let (bl, br) = (g(0, 0.0, 0.55, 0.5, 1.0), g(0, 0.5, 0.55, 1.0, 1.0));
    assert!(
        tl > 3 * tr && br > 3 * bl,
        "left only leans left: {tl} {tr} / {bl} {br}"
    );
    // Right only: mirrored.
    let (tl, tr) = (g(1, 0.0, 0.0, 0.5, 0.45), g(1, 0.5, 0.0, 1.0, 0.45));
    let (bl, br) = (g(1, 0.0, 0.55, 0.5, 1.0), g(1, 0.5, 0.55, 1.0, 1.0));
    assert!(
        tr > 3 * tl && bl > 3 * br,
        "right only leans right: {tl} {tr} / {bl} {br}"
    );
    // Mono: a vertical line in the middle fifth.
    let (l, c, r) = (
        g(2, 0.0, 0.0, 0.4, 1.0),
        g(2, 0.4, 0.0, 0.6, 1.0),
        g(2, 0.6, 0.0, 1.0, 1.0),
    );
    assert!(c > 3 * l && c > 3 * r, "mono stands upright: {l} {c} {r}");
    // Reversed: a horizontal line.
    let (t, m, b) = (
        g(3, 0.0, 0.0, 1.0, 0.4),
        g(3, 0.0, 0.4, 1.0, 0.6),
        g(3, 0.0, 0.6, 1.0, 1.0),
    );
    assert!(m > 3 * t && m > 3 * b, "reversed lies flat: {t} {m} {b}");
    // Noise: a cloud, every quadrant lit alike.
    let quads = [
        g(4, 0.0, 0.0, 0.5, 0.5),
        g(4, 0.5, 0.0, 1.0, 0.5),
        g(4, 0.0, 0.5, 0.5, 1.0),
        g(4, 0.5, 0.5, 1.0, 1.0),
    ];
    let (lo, hi) = (
        *quads.iter().min().expect("four quadrants"),
        *quads.iter().max().expect("four quadrants"),
    );
    assert!(lo > 0 && hi < 2 * lo, "noise is a cloud: {quads:?}");
}

#[test]
fn gonio_embers_decay_0p4() {
    let Some(gpu) = gpu("gonio_embers_decay_0p4") else {
        return;
    };
    let mut r = renderer(gpu);
    let img = render(gpu, &mut r, &five_figures(0.4), 1.0);
    assert_figures(&img);
    golden("gonio_embers_decay_0p4", &img, EMBERS);
}

#[test]
fn gonio_embers_decay_2s() {
    let Some(gpu) = gpu("gonio_embers_decay_2s") else {
        return;
    };
    let mut r = renderer(gpu);
    let img = render(gpu, &mut r, &five_figures(2.0), 1.0);
    assert_figures(&img);
    golden("gonio_embers_decay_2s", &img, EMBERS);
}

/// The slow setting keeps old points lit: the second-old half of the ring still glows.
#[test]
fn long_decay_keeps_more_light() {
    let Some(gpu) = gpu("long_decay_keeps_more_light") else {
        return;
    };
    let mut r = renderer(gpu);
    let fast = render(gpu, &mut r, &five_figures(0.4), 1.0);
    let slow = render(gpu, &mut r, &five_figures(2.0), 1.0);
    let whole = |img: &Image| glow(img, [0, 0, img.width(), img.height()]);
    assert!(
        whole(&slow) > whole(&fast) * 3 / 2,
        "{} vs {}",
        whole(&slow),
        whole(&fast)
    );
}

#[test]
fn chrome_flag_hides_the_grid() {
    let ring = ring(Figure::Mono);
    let rect = panel_rect(0);
    let shown = gonio_layer(&ring, 0.4, rect, true);
    let hidden = gonio_layer(&ring, 0.4, rect, false);
    assert_eq!(shown.polylines.len(), 3);
    assert!(hidden.polylines.is_empty());
    assert_eq!(shown.points, hidden.points);
    assert_eq!(shown.points[0].blend, Blend::Additive);
    assert_eq!(shown.points[0].points.len(), POINTS * FRAMES);
}
