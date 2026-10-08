//! Golden images: small scenes, one feature each, blessed on lavapipe (`AC2_BLESS=1`).
//!
//! No adapter: skip with a message, unless `AC2_REQUIRE_GPU=1` (CI). On mismatch the
//! actual/expected/diff PNGs go to `target/golden-images/`.

use crate::common;

use ac2_plot::{
    Anchor, Band, BandPoint, Color, Colormap, Dash, FillRect, Grid, GridAxis, GridKind, GridLine,
    HAlign, Heatmap, HeatmapAxes, HeatmapColumn, HeatmapId, Label, Layer, Polyline, Rect, Stroke,
    VAlign,
};
use ac2_testkit::image::ImageTolerance;
use common::{golden, gpu, render, renderer, scene};

/// Strokes and fills: anti-aliased edges are where rasterizers disagree (f32 coverage,
/// blend rounding), by a few LSB. A stroke one pixel off or a double-blended join moves a
/// run of pixels by far more than 24 and exceeds 0.2 % of these small frames.
const LINES: ImageTolerance = ImageTolerance {
    channel: 24,
    max_bad_fraction: 0.002,
};
/// Glyphs are rasterized on the CPU (swash) and identical everywhere; only the atlas
/// sampling and blend differ, on glyph edges, which are a larger share of a text frame.
const TEXT: ImageTolerance = ImageTolerance {
    channel: 32,
    max_bad_fraction: 0.005,
};
/// Heatmap cells are not anti-aliased: every pixel is a LUT entry or background, so any
/// difference beyond blend rounding is a wrong cell.
const HEATMAP: ImageTolerance = ImageTolerance {
    channel: 4,
    max_bad_fraction: 0.001,
};

const BLUE: Color = Color::rgb(0.0, 0.62, 0.95);
const ORANGE: Color = Color::rgb(1.0, 0.55, 0.10);
const GREEN: Color = Color::rgb(0.30, 0.80, 0.35);
const PINK: Color = Color::rgb(0.90, 0.30, 0.45);
const GREY: Color = Color::rgb(0.55, 0.58, 0.62);

fn line(points: Vec<[f32; 2]>, stroke: Stroke) -> Polyline {
    Polyline {
        points,
        alpha: vec![],
        stroke,
        clip: None,
    }
}

fn zigzag(x0: f32, y0: f32, n: usize, dx: f32, dy: f32) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| [x0 + dx * i as f32, y0 + if i % 2 == 0 { 0.0 } else { dy }])
        .collect()
}

#[test]
fn lines_and_joins() {
    let Some(gpu) = gpu("lines_and_joins") else {
        return;
    };
    let mut r = renderer(gpu);
    let mut layer = Layer::default();
    // Sharp opaque zigzag: round joins and caps.
    layer.polylines.push(line(
        zigzag(14.0, 16.0, 8, 13.0, 30.0),
        Stroke::solid(BLUE, 6.0),
    ));
    // Translucent wide zigzag crossing it: joins must not darken.
    layer.polylines.push(line(
        zigzag(10.0, 48.0, 9, 13.0, -22.0),
        Stroke::solid(ORANGE.with_alpha(0.5), 8.0),
    ));
    // Per-vertex alpha fade along a sine.
    let n = 60;
    let sine: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            [130.0 + 100.0 * t, 40.0 + 22.0 * (t * 9.0).sin()]
        })
        .collect();
    layer.polylines.push(Polyline {
        alpha: (0..n)
            .map(|i| 0.1 + 0.9 * i as f32 / (n - 1) as f32)
            .collect(),
        points: sine,
        stroke: Stroke::solid(GREEN, 3.0),
        clip: None,
    });
    // Dashed curve; the dash pattern runs continuously through the joins.
    let arc: Vec<[f32; 2]> = (0..40)
        .map(|i| {
            let a = i as f32 / 39.0 * std::f32::consts::PI;
            [60.0 + 45.0 * a.cos(), 140.0 - 45.0 * a.sin()]
        })
        .collect();
    layer.polylines.push(line(
        arc,
        Stroke {
            color: PINK,
            width: 2.5,
            dash: Some(Dash {
                on: 8.0,
                off: 4.0,
                offset: 0.0,
            }),
        },
    ));
    // A gap (NaN) splits a line; a lone point is a dot.
    layer.polylines.push(line(
        vec![
            [130.0, 100.0],
            [160.0, 120.0],
            [f32::NAN, 0.0],
            [180.0, 120.0],
            [220.0, 100.0],
        ],
        Stroke::solid(GREY, 4.0),
    ));
    layer
        .polylines
        .push(line(vec![[200.0, 140.0]], Stroke::solid(ORANGE, 10.0)));
    let img = render(gpu, &mut r, &scene(240.0, 160.0, vec![layer]), 1.0);
    golden("lines_and_joins", &img, LINES);
}

/// Grid, band, trace and label clipped to a plot rect with fractional edges.
fn clipped_plot() -> ac2_plot::Scene {
    let plot = Rect::new(20.5, 18.25, 150.0, 90.6);
    let mut layer = Layer::default();
    layer.rects.push(FillRect {
        rect: Rect::new(20.0, 18.0, 151.0, 91.0),
        color: Color::rgb(0.11, 0.12, 0.15),
        clip: None,
    });
    let mut lines = vec![];
    for i in 0..=10 {
        lines.push(GridLine {
            axis: GridAxis::X,
            pos: plot.x + plot.w * i as f32 / 10.0,
            kind: if i % 5 == 0 {
                GridKind::Major
            } else {
                GridKind::Minor
            },
        });
    }
    for i in 0..=6 {
        lines.push(GridLine {
            axis: GridAxis::Y,
            pos: plot.y + plot.h * i as f32 / 6.0,
            kind: if i % 3 == 0 {
                GridKind::Major
            } else {
                GridKind::Minor
            },
        });
    }
    layer.grids.push(Grid {
        rect: plot,
        lines,
        major: Stroke::solid(GREY.with_alpha(0.6), 1.0),
        minor: Stroke {
            color: GREY.with_alpha(0.35),
            width: 1.0,
            dash: Some(Dash {
                on: 2.0,
                off: 2.0,
                offset: 0.0,
            }),
        },
    });
    let n = 50;
    let f = |i: usize| -> (f32, f32) {
        let t = i as f32 / (n - 1) as f32;
        (
            5.0 + 190.0 * t,
            60.0 - 60.0 * (t * 5.0).sin() * (1.0 - 0.5 * t),
        )
    };
    layer.bands.push(Band {
        points: (0..n)
            .map(|i| {
                let (x, y) = f(i);
                BandPoint {
                    x,
                    y0: y - 9.0,
                    y1: y + 9.0,
                }
            })
            .collect(),
        color: BLUE.with_alpha(0.3),
        clip: Some(plot),
    });
    layer.polylines.push(Polyline {
        points: (0..n)
            .map(|i| {
                let (x, y) = f(i);
                [x, y]
            })
            .collect(),
        alpha: vec![],
        stroke: Stroke::solid(BLUE, 3.0),
        clip: Some(plot),
    });
    layer.labels.push(Label {
        text: "clipped label".into(),
        pos: [140.0, 100.0],
        anchor: Anchor::TOP_LEFT,
        size: 13.0,
        color: Color::WHITE,
        clip: Some(plot),
    });
    scene(200.0, 130.0, vec![layer])
}

#[test]
fn clipping() {
    let Some(gpu) = gpu("clipping") else {
        return;
    };
    let mut r = renderer(gpu);
    let img = render(gpu, &mut r, &clipped_plot(), 1.0);
    golden("clipping", &img, TEXT);
}

fn hairline_scene() -> ac2_plot::Scene {
    let mut layer = Layer::default();
    for (i, w) in [0.25f32, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0].iter().enumerate() {
        let y = 12.0 + 14.0 * i as f32;
        layer.polylines.push(line(
            vec![[8.0, y], [70.0, y + 6.0], [120.0, y + 6.0]],
            Stroke::solid(Color::WHITE, *w),
        ));
    }
    let rect = Rect::new(130.0, 8.0, 62.0, 100.0);
    let mut lines = vec![];
    for i in 0..=12 {
        lines.push(GridLine {
            axis: GridAxis::X,
            pos: rect.x + 5.13 * i as f32,
            kind: if i % 4 == 0 {
                GridKind::Major
            } else {
                GridKind::Minor
            },
        });
        lines.push(GridLine {
            axis: GridAxis::Y,
            pos: rect.y + 8.3 * i as f32,
            kind: if i % 4 == 0 {
                GridKind::Major
            } else {
                GridKind::Minor
            },
        });
    }
    layer.grids.push(Grid {
        rect,
        lines,
        major: Stroke::solid(GREY, 1.0),
        minor: Stroke::solid(GREY, 0.5),
    });
    scene(200.0, 116.0, vec![layer])
}

#[test]
fn hairlines_and_grid() {
    let Some(gpu) = gpu("hairlines_and_grid") else {
        return;
    };
    let mut r = renderer(gpu);
    let img = render(gpu, &mut r, &hairline_scene(), 1.0);
    golden("hairlines_and_grid", &img, LINES);
}

/// Same scene at 1.5 physical pixels per logical pixel: snapping and the sub-pixel rule
/// work in physical pixels.
#[test]
fn hairlines_and_grid_hidpi() {
    let Some(gpu) = gpu("hairlines_and_grid_hidpi") else {
        return;
    };
    let mut r = renderer(gpu);
    let img = render(gpu, &mut r, &hairline_scene(), 1.5);
    golden("hairlines_and_grid_hidpi", &img, LINES);
}

#[test]
fn text_anchors_and_banner() {
    let Some(gpu) = gpu("text_anchors_and_banner") else {
        return;
    };
    let mut r = renderer(gpu);
    let mut base = Layer::default();
    let anchors = [
        (HAlign::Left, VAlign::Top),
        (HAlign::Center, VAlign::Center),
        (HAlign::Right, VAlign::Baseline),
        (HAlign::Left, VAlign::Bottom),
    ];
    for (i, (h, v)) in anchors.into_iter().enumerate() {
        let pos = [40.0 + 80.0 * i as f32, 30.0];
        // Cross marking the anchor point.
        base.polylines.push(line(
            vec![[pos[0] - 4.0, pos[1]], [pos[0] + 4.0, pos[1]]],
            Stroke::solid(PINK, 1.0),
        ));
        base.polylines.push(line(
            vec![[pos[0], pos[1] - 4.0], [pos[0], pos[1] + 4.0]],
            Stroke::solid(PINK, 1.0),
        ));
        base.labels.push(Label {
            text: "Hz 1k".into(),
            pos,
            anchor: ac2_plot::Anchor { h, v },
            size: 14.0,
            color: Color::WHITE,
            clip: None,
        });
    }
    for (i, size) in [9.0, 11.0, 16.0, 22.0].into_iter().enumerate() {
        base.labels.push(Label {
            text: "−12.5 dB  coh 0.98".into(),
            pos: [8.0, 56.0 + 22.0 * i as f32],
            anchor: Anchor::TOP_LEFT,
            size,
            color: Color::rgb(0.85, 0.87, 0.9),
            clip: None,
        });
    }
    base.polylines.push(line(
        vec![[200.0, 60.0], [310.0, 140.0]],
        Stroke::solid(BLUE, 4.0),
    ));
    // Banner in a later layer covers the line and the base layer's text.
    let mut overlay = Layer::default();
    overlay.rects.push(FillRect {
        rect: Rect::new(190.0, 88.0, 124.0, 26.0),
        color: Color::rgb(0.75, 0.15, 0.1).with_alpha(0.9),
        clip: None,
    });
    overlay.labels.push(Label {
        text: "STALE 2.4 s".into(),
        pos: [252.0, 101.0],
        anchor: Anchor::CENTER,
        size: 15.0,
        color: Color::WHITE,
        clip: None,
    });
    let img = render(gpu, &mut r, &scene(320.0, 150.0, vec![base, overlay]), 1.0);
    golden("text_anchors_and_banner", &img, TEXT);
}

const HM_COLS: u32 = 32;
const HM_ROWS: u32 = 24;

/// Analytic spectrogram cell value: a sweeping ridge over a sloped floor.
fn hm_value(col: u32, row: u32) -> f32 {
    let ridge = (col as f32 * 0.6) % HM_ROWS as f32;
    let d = row as f32 - ridge;
    -60.0 + 40.0 * (-d * d / 6.0).exp() + row as f32 * 0.5
}

fn hm_columns(first: u32, n: u32) -> Vec<f32> {
    let mut v = Vec::new();
    for c in first..first + n {
        for row in 0..HM_ROWS {
            // Hole with no data: transparent.
            if (10..13).contains(&c) && (4..9).contains(&row) {
                v.push(f32::NAN);
            } else {
                v.push(hm_value(c, row));
            }
        }
    }
    v
}

fn heatmap(scroll: u32, data: Vec<Option<HeatmapColumn>>) -> Heatmap {
    Heatmap {
        id: HeatmapId(7),
        rect: Rect::new(10.0, 10.0, 128.0, 72.0),
        clip: None,
        columns: HM_COLS,
        rows: HM_ROWS,
        axes: HeatmapAxes::TimeAcross,
        scroll,
        range: [-60.0, -10.0],
        colormap: Colormap::Viridis,
        opacity: 1.0,
        data,
    }
}

#[test]
fn heatmap_scroll() {
    let Some(gpu) = gpu("heatmap_scroll") else {
        return;
    };
    let mut r = renderer(gpu);
    let frame = |h: Heatmap| {
        let mut layer = Layer::default();
        layer.heatmaps.push(h);
        scene(148.0, 92.0, vec![layer])
    };
    let cols: Vec<HeatmapColumn> = (0..36).map(|c| hm_columns(c, 1).into()).collect();
    // Frame 1: data columns 0..30 in ring columns 0..30; ring columns 30 and 31 are still
    // empty and draw transparent.
    let s1 = frame(heatmap(
        0,
        (0..HM_COLS as usize)
            .map(|i| (i < 30).then(|| cols[i].clone()))
            .collect(),
    ));
    render(gpu, &mut r, &s1, 1.0);
    // Frame 2: data columns 30..36 land in ring columns 30, 31, 0..4 (the ring wraps), the
    // rest are the columns frame 1 uploaded; the view scrolls so the oldest remaining
    // column (data 4, ring 4) is at the left.
    let s2 = frame(heatmap(
        4,
        (0..HM_COLS as usize)
            .map(|i| Some(cols[if i < 4 { 32 + i } else { i }].clone()))
            .collect(),
    ));
    let img = render(gpu, &mut r, &s2, 1.0);
    golden("heatmap_scroll", &img, HEATMAP);
}
