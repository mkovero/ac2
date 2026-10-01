//! Spike driver: logs adapters, benchmarks offscreen render + readback, and `--bless`
//! regenerates the reference PNG used by the golden test.
//!
//! ```text
//! cargo run -p spike-gpu-headless --release                   # bench 1920x1080, 16 curves
//! cargo run -p spike-gpu-headless --release -- --bless        # rewrite tests/reference/*.png
//! cargo run -p spike-gpu-headless --release -- --out x.png    # also save the bench frame
//! AC2_GPU_FALLBACK=1 WGPU_BACKEND=vulkan cargo run ...         # force software adapter
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use spike_gpu_headless::{Gpu, Renderer, TextLayer, demo_scene, describe};

const GOLDEN_W: u32 = 800;
const GOLDEN_H: u32 = 450;

fn reference_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/reference/plot_800x450.png")
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bless = args.iter().any(|a| a == "--bless");
    let out = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    for a in &gpu.seen {
        println!("seen:   {}", describe(a));
    }
    println!("picked: {}", gpu.describe());
    println!("device create: {:.1} ms", ms(gpu.create_time));

    let mut renderer = Renderer::new(&gpu);

    if bless {
        let scene = demo_scene(GOLDEN_W, GOLDEN_H, 2, 480);
        let img = match renderer.render(&gpu, &scene, None) {
            Ok((img, _)) => img,
            Err(e) => {
                eprintln!("render failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        let p = reference_path();
        if let Err(e) = img.save_png(&p) {
            eprintln!("save failed: {e}");
            return ExitCode::FAILURE;
        }
        println!("blessed {} on {}", p.display(), gpu.describe());
        return ExitCode::SUCCESS;
    }

    let mut text = TextLayer::new(&gpu);
    if text.is_none() {
        println!("no system fonts: text label skipped");
    }
    let scene = demo_scene(1920, 1080, 16, 480);
    let segs: usize = scene.lines.iter().map(|l| l.points.len() - 1).sum();
    println!(
        "scene: {}x{}, {} polylines, {} segments",
        scene.width,
        scene.height,
        scene.lines.len(),
        segs
    );
    for with_text in [false, true] {
        if with_text && text.is_none() {
            continue;
        }
        let mut frames = Vec::new();
        let mut last = None;
        for i in 0..23 {
            let t = if with_text { text.as_mut() } else { None };
            match renderer.render(&gpu, &scene, t) {
                Ok((img, timing)) => {
                    if i == 0 {
                        println!(
                            "  first frame (text={with_text}): {:.1} ms (incl. pipeline/atlas warmup)",
                            ms(timing.total)
                        );
                    }
                    if i >= 3 {
                        frames.push(timing);
                    }
                    last = Some(img);
                }
                Err(e) => {
                    eprintln!("render failed: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        frames.sort_by_key(|t| t.total);
        let med = frames[frames.len() / 2];
        println!(
            "  text={with_text}: render+readback median {:.2} ms (min {:.2}, max {:.2}); \
             median split encode {:.2} / gpu+map {:.2} / unpack {:.2} ms",
            ms(med.total),
            ms(frames[0].total),
            ms(frames[frames.len() - 1].total),
            ms(med.encode),
            ms(med.gpu_and_map),
            ms(med.unpack),
        );
        if with_text == text.is_some()
            && let (Some(p), Some(img)) = (&out, &last)
        {
            if let Err(e) = img.save_png(p) {
                eprintln!("save failed: {e}");
                return ExitCode::FAILURE;
            }
            println!("  wrote {}", p.display());
        }
    }
    ExitCode::SUCCESS
}
