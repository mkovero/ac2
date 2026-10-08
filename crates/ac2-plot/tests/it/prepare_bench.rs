//! Bench-style test: prepare time for a dense 1080p plot (16 traces × 480 points, grid,
//! band, labels). Prints timings and never fails on them; the budget (< 1 ms prepare) is
//! for optimized builds:
//!
//! ```text
//! cargo test -p ac2-plot --release --test it prepare_bench:: -- --nocapture
//! ```

use crate::common;

use std::time::{Duration, Instant};

use ac2_plot::{
    Anchor, Band, BandPoint, Color, FrameTarget, Grid, GridAxis, GridKind, GridLine, Label, Layer,
    Polyline, Rect, Stroke, offscreen::Offscreen,
};
use common::{BG, gpu, renderer, scene};

const W: f32 = 1920.0;
const H: f32 = 1080.0;
const TRACES: usize = 16;
const POINTS: usize = 480;

fn dense_scene(phase: f32) -> ac2_plot::Scene {
    let plot = Rect::new(80.0, 40.0, W - 120.0, H - 100.0);
    let mut layer = Layer::default();
    let mut lines = Vec::new();
    for i in 0..=30 {
        lines.push(GridLine {
            axis: GridAxis::X,
            pos: plot.x + plot.w * i as f32 / 30.0,
            kind: if i % 10 == 0 {
                GridKind::Major
            } else {
                GridKind::Minor
            },
        });
    }
    for i in 0..=16 {
        lines.push(GridLine {
            axis: GridAxis::Y,
            pos: plot.y + plot.h * i as f32 / 16.0,
            kind: if i % 4 == 0 {
                GridKind::Major
            } else {
                GridKind::Minor
            },
        });
    }
    layer.grids.push(Grid {
        rect: plot,
        lines,
        major: Stroke::solid(Color::rgb(0.5, 0.5, 0.55).with_alpha(0.6), 1.0),
        minor: Stroke::solid(Color::rgb(0.4, 0.4, 0.45).with_alpha(0.3), 1.0),
    });
    let y = |t: f32, k: usize| {
        plot.y + plot.h * (0.5 + 0.3 * ((t * (3.0 + k as f32) + phase).sin()) - 0.02 * k as f32)
    };
    layer.bands.push(Band {
        points: (0..POINTS)
            .map(|i| {
                let t = i as f32 / (POINTS - 1) as f32;
                BandPoint {
                    x: plot.x + plot.w * t,
                    y0: y(t, 0) - 20.0,
                    y1: y(t, 0) + 20.0,
                }
            })
            .collect(),
        color: Color::rgb(0.0, 0.6, 0.9).with_alpha(0.2),
        clip: Some(plot),
    });
    for k in 0..TRACES {
        let mut points = Vec::with_capacity(POINTS);
        let mut alpha = Vec::with_capacity(POINTS);
        for i in 0..POINTS {
            let t = i as f32 / (POINTS - 1) as f32;
            points.push([plot.x + plot.w * t, y(t, k)]);
            alpha.push(0.2 + 0.8 * t);
        }
        layer.polylines.push(Polyline {
            points,
            alpha,
            stroke: Stroke::solid(Color::rgb(0.2 + 0.05 * k as f32, 0.6, 0.9), 2.5),
            clip: Some(plot),
        });
    }
    for i in 0..=10 {
        layer.labels.push(Label {
            text: format!("{} Hz", 20 * (1 << i)),
            pos: [plot.x + plot.w * i as f32 / 10.0, plot.bottom() + 6.0],
            anchor: Anchor {
                h: ac2_plot::HAlign::Center,
                v: ac2_plot::VAlign::Top,
            },
            size: 12.0,
            color: Color::rgb(0.8, 0.8, 0.85),
            clip: None,
        });
    }
    scene(W, H, vec![layer])
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

#[test]
fn prepare_time_dense_1080p() {
    let Some(gpu) = gpu("prepare_time_dense_1080p") else {
        return;
    };
    let mut r = renderer(gpu);
    let target = FrameTarget::full([W as u32, H as u32], 1.0);
    // Scenes are rebuilt per frame by the scene layer; vary the data so nothing is cached
    // but the label text (which is what happens in practice).
    let scenes: Vec<_> = (0..8).map(|i| dense_scene(i as f32 * 0.1)).collect();
    for s in scenes.iter().take(3) {
        r.prepare(&gpu.device, &gpu.queue, s, &target)
            .expect("prepare");
    }
    let mut times = Vec::new();
    for i in 0..40 {
        let t0 = Instant::now();
        r.prepare(&gpu.device, &gpu.queue, &scenes[i % scenes.len()], &target)
            .expect("prepare");
        times.push(t0.elapsed());
    }
    let min = times.iter().min().copied().unwrap_or_default();
    eprintln!(
        "prepare {TRACES}x{POINTS} @1080p: median {:?}, min {:?} ({} build)",
        median(times),
        min,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );

    let off = Offscreen::new(&gpu.device, [W as u32, H as u32]);
    let mut frames = Vec::new();
    for i in 0..10 {
        let (_, t) = off
            .render(gpu, &mut r, &scenes[i % scenes.len()], 1.0, BG)
            .expect("render");
        frames.push(t);
    }
    eprintln!(
        "offscreen frame: prepare {:?}, encode {:?}, gpu+readback {:?} (median; adapter {})",
        median(frames.iter().map(|t| t.prepare).collect()),
        median(frames.iter().map(|t| t.encode).collect()),
        median(frames.iter().map(|t| t.gpu).collect()),
        gpu.describe()
    );
}
