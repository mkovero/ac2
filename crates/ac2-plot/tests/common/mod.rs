//! Shared helpers for the GPU tests: adapter-or-skip, rendering to a testkit image, goldens.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::OnceLock;

use ac2_plot::{Color, Gpu, Layer, Renderer, Scene, Viewport, offscreen::OFFSCREEN_FORMAT, wgpu};
use ac2_testkit::image::{GoldenOutcome, Image, ImageTolerance, check_golden, gpu_required};

pub const BG: Color = Color::rgb(0.07, 0.08, 0.10);

static GPU: OnceLock<Result<Gpu, String>> = OnceLock::new();

/// The shared headless device, or `None` (after printing why) when there is no adapter and
/// `AC2_REQUIRE_GPU=1` is not set.
pub fn gpu(test: &str) -> Option<&'static Gpu> {
    let g = GPU.get_or_init(|| {
        Gpu::new()
            .inspect(|g| eprintln!("adapter: {}", g.describe()))
            .map_err(|e| e.to_string())
    });
    match g {
        Ok(g) => Some(g),
        Err(e) if gpu_required() => panic!("{test}: {e} (AC2_REQUIRE_GPU=1)"),
        Err(e) => {
            eprintln!("SKIP {test}: {e}");
            None
        }
    }
}

pub fn renderer(gpu: &Gpu) -> Renderer {
    Renderer::new(
        &gpu.device,
        &gpu.queue,
        OFFSCREEN_FORMAT,
        wgpu::MultisampleState::default(),
    )
}

pub fn scene(w: f32, h: f32, layers: Vec<Layer>) -> Scene {
    Scene {
        viewport: Viewport {
            width: w,
            height: h,
        },
        layers,
    }
}

/// Renders at `scale`; the target is the viewport size times `scale`, rounded.
pub fn render(gpu: &Gpu, r: &mut Renderer, scene: &Scene, scale: f32) -> Image {
    render_on(gpu, r, scene, scale, BG)
}

pub fn render_on(gpu: &Gpu, r: &mut Renderer, scene: &Scene, scale: f32, bg: Color) -> Image {
    let size = [
        (scene.viewport.width * scale).round() as u32,
        (scene.viewport.height * scale).round() as u32,
    ];
    let px = ac2_plot::render_to_image(gpu, r, scene, size, scale, bg).expect("render");
    Image::new(px.width, px.height, px.rgba).expect("image size")
}

pub fn reference_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/reference")
}

pub fn golden(name: &str, img: &Image, tol: ImageTolerance) {
    match check_golden(name, img, &reference_dir(), tol) {
        Ok(GoldenOutcome::Blessed(p)) => eprintln!("{name}: blessed {}", p.display()),
        Ok(GoldenOutcome::Matched(d)) => eprintln!("{name}: {}", d.summary(tol)),
        Err(e) => panic!("{e}"),
    }
}

/// Channel 0 summed over a region, as a measure of ink on a black background.
pub fn ink(img: &Image, x0: u32, y0: u32, x1: u32, y1: u32) -> u64 {
    let mut s = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            s += u64::from(img.pixel(x, y)[0]);
        }
    }
    s
}
