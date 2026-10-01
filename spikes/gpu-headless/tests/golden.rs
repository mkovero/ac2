//! Golden-image test: renders the demo plot headless and compares it with
//! `tests/reference/plot_800x450.png` within a tolerance.
//!
//! - No adapter: skips with a message, unless `AC2_REQUIRE_GPU=1` (CI) makes it fail.
//! - `AC2_BLESS=1` rewrites the reference instead of comparing (same as `-- --bless` on the
//!   binary).
//! - On failure, actual and diff images are written under `target/gpu-headless/`.

use std::path::PathBuf;

use spike_gpu_headless::{
    BLESS_ENV, Gpu, Image, REQUIRE_ENV, Renderer, TextLayer, Tolerance, compare, demo_scene,
};

/// Anti-aliased edges are where rasterizers disagree: coverage is computed from pixel-centre
/// distances in f32, and lavapipe, llvmpipe-GL, WARP and hardware differ by a few LSB after
/// blending. A full stroke or grid line that moved by one pixel differs by far more than 24,
/// and would show up as a long run of bad pixels, well above 0.2 % of the frame.
const TOLERANCE: Tolerance = Tolerance {
    channel: 24,
    max_bad_fraction: 0.002,
};

fn gpu_or_skip(test: &str) -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => {
            eprintln!("{test}: adapter {}", g.describe());
            Some(g)
        }
        Err(e) => {
            if std::env::var(REQUIRE_ENV).is_ok_and(|v| v == "1") {
                panic!("{test}: {e} ({REQUIRE_ENV}=1)");
            }
            eprintln!("SKIP {test}: {e}");
            None
        }
    }
}

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn artifacts() -> PathBuf {
    manifest().join("../../target/gpu-headless")
}

#[test]
fn plot_matches_reference() {
    let Some(gpu) = gpu_or_skip("plot_matches_reference") else {
        return;
    };
    let mut r = Renderer::new(&gpu);
    let scene = demo_scene(800, 450, 2, 480);
    let (img, _) = r.render(&gpu, &scene, None).expect("render");
    let reference = manifest().join("tests/reference/plot_800x450.png");

    if std::env::var(BLESS_ENV).is_ok_and(|v| v == "1") {
        img.save_png(&reference).expect("bless");
        eprintln!("blessed {}", reference.display());
        return;
    }

    let expected = Image::load_png(&reference).expect("reference png (bless with AC2_BLESS=1)");
    let diff = compare(&img, &expected, TOLERANCE).expect("comparable");
    eprintln!(
        "diff: {} px differ at all, {} px > {} ({:.4} %), max channel diff {}",
        diff.nonzero_pixels,
        diff.bad_pixels,
        TOLERANCE.channel,
        diff.bad_fraction() * 100.0,
        diff.max_channel_diff
    );
    if !diff.passes(TOLERANCE) {
        let dir = artifacts();
        img.save_png(&dir.join("plot_actual.png")).expect("save");
        diff.image
            .save_png(&dir.join("plot_diff.png"))
            .expect("save");
        panic!(
            "rendering differs from reference beyond tolerance; see {}",
            dir.display()
        );
    }
}

#[test]
fn rendering_is_repeatable_on_one_adapter() {
    let Some(gpu) = gpu_or_skip("rendering_is_repeatable_on_one_adapter") else {
        return;
    };
    let mut r = Renderer::new(&gpu);
    let scene = demo_scene(320, 180, 2, 480);
    let (a, _) = r.render(&gpu, &scene, None).expect("render");
    let (b, _) = r.render(&gpu, &scene, None).expect("render");
    assert_eq!(a, b, "same adapter, same scene must be bit-identical");
}

/// A translucent stroke must not darken at joins: the join pixels of a straight
/// alpha-0.5 polyline must look like the middle of a segment.
#[test]
fn translucent_joins_do_not_double_blend() {
    let Some(gpu) = gpu_or_skip("translucent_joins_do_not_double_blend") else {
        return;
    };
    let mut r = Renderer::new(&gpu);
    let mut scene = demo_scene(64, 16, 0, 2);
    scene.lines.clear();
    scene.labels.clear();
    scene.background = [0.0, 0.0, 0.0, 1.0];
    // Collinear points every 8 px, all at alpha 0.5.
    let pts: Vec<[f32; 2]> = (0..7).map(|i| [4.0 + 8.0 * i as f32, 8.0]).collect();
    scene.lines.push(spike_gpu_headless::Polyline {
        alpha: vec![0.5; pts.len()],
        points: pts,
        color: [1.0, 1.0, 1.0, 1.0],
        width: 4.0,
    });
    let (img, _) = r.render(&gpu, &scene, None).expect("render");
    let px = |x: u32, y: u32| img.rgba[((y * img.width + x) * 4) as usize];
    let mid_segment = px(8, 8);
    let at_join = px(12, 8);
    assert!(
        mid_segment > 100 && mid_segment < 150,
        "half-alpha white should be ~128, got {mid_segment}"
    );
    assert!(
        mid_segment.abs_diff(at_join) <= 2,
        "join {at_join} vs segment {mid_segment}: double blending"
    );
}

/// The text path depends on system fonts, so it is checked structurally (ink inside the
/// label box, none outside), not against a golden.
#[test]
fn label_renders_ink_in_its_box() {
    let Some(gpu) = gpu_or_skip("label_renders_ink_in_its_box") else {
        return;
    };
    let Some(mut text) = TextLayer::new(&gpu) else {
        eprintln!("SKIP label_renders_ink_in_its_box: no system fonts");
        return;
    };
    let mut r = Renderer::new(&gpu);
    let mut scene = demo_scene(400, 100, 0, 2);
    scene.lines.clear();
    scene.background = [0.0, 0.0, 0.0, 1.0];
    let l = &mut scene.labels[0];
    (l.x, l.y, l.size) = (20.0, 30.0, 20.0);
    let (img, _) = r.render(&gpu, &scene, Some(&mut text)).expect("render");
    let mut inside = 0;
    let mut outside = 0;
    for y in 0..img.height {
        for x in 0..img.width {
            let v = img.rgba[((y * img.width + x) * 4) as usize];
            if v > 64 {
                if (15..380).contains(&x) && (25..60).contains(&y) {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
    }
    if std::env::var(BLESS_ENV).is_ok_and(|v| v == "1") {
        img.save_png(&artifacts().join("label.png")).expect("save");
    }
    assert!(inside > 200, "label has too little ink: {inside}");
    assert_eq!(outside, 0, "ink outside the label box");
}
