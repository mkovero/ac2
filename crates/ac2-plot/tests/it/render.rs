//! Structural rendering tests: properties that fail with a clear message instead of a diff
//! image (join blending, clipping, hairline ink, heatmap ring semantics, anchors, egui-style
//! targets).

use crate::common;

use ac2_plot::{
    Anchor, Band, BandPoint, Color, Colormap, FrameTarget, HAlign, Heatmap, HeatmapAxes,
    HeatmapColumn, HeatmapId, Label, Layer, Polyline, PrepareError, Rect, Stroke, VAlign,
    offscreen::Offscreen,
};
use ac2_testkit::image::Image;
use common::{gpu, ink, render, render_on, renderer, scene};

fn white_line(points: Vec<[f32; 2]>, width: f32, alpha: f32) -> Polyline {
    Polyline {
        alpha: vec![alpha; points.len()],
        points,
        stroke: Stroke::solid(Color::WHITE, width),
        clip: None,
    }
}

fn one_layer(w: f32, h: f32, f: impl FnOnce(&mut Layer)) -> ac2_plot::Scene {
    let mut l = Layer::default();
    f(&mut l);
    scene(w, h, vec![l])
}

#[test]
fn rendering_is_repeatable() {
    let Some(gpu) = gpu("rendering_is_repeatable") else {
        return;
    };
    let mut r = renderer(gpu);
    let s = one_layer(64.0, 40.0, |l| {
        l.polylines.push(white_line(
            vec![[4.0, 4.0], [30.0, 35.0], [60.0, 10.0]],
            3.0,
            0.7,
        ));
    });
    let a = render(gpu, &mut r, &s, 1.0);
    let b = render(gpu, &mut r, &s, 1.0);
    assert_eq!(a, b, "same adapter, same scene must be bit-identical");
}

/// Labels kept from earlier frames (by text and size) land exactly where freshly shaped
/// ones do when labels come and go around them, repeat, or change size; and renderers
/// sharing one set of font caches do not disturb each other.
#[test]
fn kept_labels_match_freshly_shaped_ones() {
    let Some(gpu) = gpu("kept_labels_match_freshly_shaped_ones") else {
        return;
    };
    let labels = |texts: &[(&str, f32)]| {
        one_layer(220.0, 120.0, |l| {
            for (i, (t, size)) in texts.iter().enumerate() {
                l.labels.push(Label {
                    text: (*t).into(),
                    pos: [6.0 + 30.0 * (i % 2) as f32, 6.0 + 18.0 * i as f32],
                    anchor: Anchor::TOP_LEFT,
                    size: *size,
                    color: Color::WHITE,
                    clip: None,
                });
            }
        })
    };
    let frames = [
        labels(&[("−12 dB", 12.0), ("1 kHz", 12.0), ("γ²", 13.0)]),
        labels(&[
            ("new first", 12.0),
            ("−12 dB", 12.0),
            ("1 kHz", 12.0),
            ("γ²", 13.0),
        ]),
        labels(&[("1 kHz", 12.0), ("1 kHz", 12.0), ("1 kHz", 16.0)]),
        labels(&[("−12 dB", 12.0), ("γ²", 13.0)]),
    ];
    let mut kept = renderer(gpu);
    let mut other = renderer(gpu);
    for (i, s) in frames.iter().enumerate() {
        let got = render(gpu, &mut kept, s, 1.0);
        render(gpu, &mut other, &frames[(i + 1) % frames.len()], 1.0);
        let want = render(gpu, &mut renderer(gpu), s, 1.0);
        assert_eq!(got, want, "frame {i}");
    }
}

/// The join pixels of a straight half-alpha polyline must look like mid-segment pixels.
#[test]
fn translucent_joins_do_not_double_blend() {
    let Some(gpu) = gpu("translucent_joins_do_not_double_blend") else {
        return;
    };
    let mut r = renderer(gpu);
    let pts: Vec<[f32; 2]> = (0..7).map(|i| [4.0 + 8.0 * i as f32, 8.5]).collect();
    let s = one_layer(64.0, 16.0, |l| l.polylines.push(white_line(pts, 4.0, 0.5)));
    let img = render_on(gpu, &mut r, &s, 1.0, Color::BLACK);
    let mid = img.pixel(8, 8)[0];
    let join = img.pixel(12, 8)[0];
    assert!(
        (100..150).contains(&mid),
        "half-alpha white should be ~128, got {mid}"
    );
    assert!(
        mid.abs_diff(join) <= 2,
        "join {join} vs segment {mid}: double blending"
    );
}

/// Band slices share their inner edges; a translucent band must be uniform across seams.
#[test]
fn band_seams_do_not_double_blend() {
    let Some(gpu) = gpu("band_seams_do_not_double_blend") else {
        return;
    };
    let mut r = renderer(gpu);
    let s = one_layer(64.0, 20.0, |l| {
        l.bands.push(Band {
            points: (0..9)
                .map(|i| BandPoint {
                    // Seams at fractional x on purpose.
                    x: 2.3 + 7.1 * i as f32,
                    y0: 4.0,
                    y1: 16.0,
                })
                .collect(),
            color: Color::WHITE.with_alpha(0.5),
            clip: None,
        });
    });
    let img = render_on(gpu, &mut r, &s, 1.0, Color::BLACK);
    let row: Vec<u8> = (3..59).map(|x| img.pixel(x, 10)[0]).collect();
    let (lo, hi) = (row.iter().min().copied(), row.iter().max().copied());
    assert!(
        hi.zip(lo).is_some_and(|(h, l)| h - l <= 1),
        "band interior not uniform: {row:?}"
    );
}

#[test]
fn nothing_is_drawn_outside_the_clip() {
    let Some(gpu) = gpu("nothing_is_drawn_outside_the_clip") else {
        return;
    };
    let mut r = renderer(gpu);
    let clip = Rect::new(10.5, 8.25, 40.0, 20.6);
    let s = one_layer(64.0, 40.0, |l| {
        let mut p = white_line(
            vec![[0.0, 0.0], [64.0, 40.0], [0.0, 40.0], [64.0, 0.0]],
            8.0,
            1.0,
        );
        p.clip = Some(clip);
        l.polylines.push(p);
        l.bands.push(Band {
            points: vec![
                BandPoint {
                    x: 0.0,
                    y0: 0.0,
                    y1: 40.0,
                },
                BandPoint {
                    x: 64.0,
                    y0: 0.0,
                    y1: 40.0,
                },
            ],
            color: Color::WHITE.with_alpha(0.2),
            clip: Some(clip),
        });
    });
    let img = render_on(gpu, &mut r, &s, 1.0, Color::BLACK);
    // Pixel centres inside [10.5, 50.5) x [8.25, 28.85): columns 10..=49, rows 8..=28.
    for y in 0..40 {
        for x in 0..64 {
            let inside = (10..=49).contains(&x) && (8..=28).contains(&y);
            let v = img.pixel(x, y)[0];
            if inside {
                assert!(v > 0, "({x},{y}) inside the clip has no ink");
            } else {
                assert_eq!(v, 0, "({x},{y}) outside the clip has ink {v}");
            }
        }
    }
}

/// A sub-pixel stroke carries ink proportional to its width.
#[test]
fn hairline_ink_scales_with_width() {
    let Some(gpu) = gpu("hairline_ink_scales_with_width") else {
        return;
    };
    let mut r = renderer(gpu);
    let ink_of = |r: &mut ac2_plot::Renderer, w: f32| {
        let s = one_layer(80.0, 20.0, |l| {
            l.polylines
                .push(white_line(vec![[10.0, 6.0], [70.0, 13.0]], w, 1.0));
        });
        let img = render_on(gpu, r, &s, 1.0, Color::BLACK);
        ink(&img, 20, 0, 60, 20) as f64
    };
    let full = ink_of(&mut r, 1.0);
    for w in [0.25, 0.5, 0.75] {
        let ratio = ink_of(&mut r, w) / full;
        assert!(
            (ratio - w as f64).abs() < 0.05,
            "width {w}: ink ratio {ratio:.3}"
        );
    }
}

fn heatmap_with(
    rect: Rect,
    columns: u32,
    rows: u32,
    axes: HeatmapAxes,
    scroll: u32,
    data: Vec<Option<HeatmapColumn>>,
) -> Heatmap {
    Heatmap {
        id: HeatmapId(1),
        rect,
        clip: None,
        columns,
        rows,
        axes,
        scroll,
        range: [0.0, 31.0],
        colormap: Colormap::Viridis,
        opacity: 1.0,
        data,
    }
}

fn heatmap_scene(scroll: u32, data: Vec<Option<HeatmapColumn>>) -> ac2_plot::Scene {
    one_layer(40.0, 24.0, |l| {
        l.heatmaps.push(heatmap_with(
            Rect::new(4.0, 4.0, 32.0, 16.0),
            8,
            4,
            HeatmapAxes::TimeAcross,
            scroll,
            data,
        ));
    })
}

/// Logical column `c` of the test ring: rows `c * 4 + row`.
fn column(c: u32) -> HeatmapColumn {
    (0..4).map(|row| (c * 4 + row) as f32).collect()
}

/// Scrolling by k over a ring equals drawing the rotated ring unscrolled.
#[test]
fn heatmap_scroll_is_a_rotation() {
    let Some(gpu) = gpu("heatmap_scroll_is_a_rotation") else {
        return;
    };
    let mut a = renderer(gpu);
    let mut b = renderer(gpu);
    let cols: Vec<HeatmapColumn> = (0..8).map(column).collect();
    let scrolled = heatmap_scene(3, cols.iter().cloned().map(Some).collect());
    // Ring column i holds logical column (3 + i) % 8.
    let rotated = heatmap_scene(0, (0..8).map(|i| Some(cols[(3 + i) % 8].clone())).collect());
    let ia = render(gpu, &mut a, &scrolled, 1.0);
    let ib = render(gpu, &mut b, &rotated, 1.0);
    let unscrolled = heatmap_scene(0, cols.iter().cloned().map(Some).collect());
    let ia0 = render(gpu, &mut a, &unscrolled, 1.0);
    assert_ne!(ia, ia0, "scroll offset has no effect");
    assert_eq!(ia, ib, "scrolled ring differs from rotated ring");
}

/// A column is uploaded when the scene holds another one at its ring position: replacing a
/// column (or emptying it) shows at once, and a ring updated column by column draws as a
/// fresh renderer given the same columns does.
#[test]
fn heatmap_columns_follow_identity() {
    let Some(gpu) = gpu("heatmap_columns_follow_identity") else {
        return;
    };
    let mut r = renderer(gpu);
    let cols: Vec<HeatmapColumn> = (0..8).map(column).collect();
    let all = |cols: &[HeatmapColumn]| cols.iter().cloned().map(Some).collect::<Vec<_>>();
    render_on(
        gpu,
        &mut r,
        &heatmap_scene(0, all(&cols)),
        1.0,
        Color::BLACK,
    );
    let mut changed = cols.clone();
    changed[2] = column(7);
    let mut emptied = all(&changed);
    emptied[5] = None;
    let img = render_on(
        gpu,
        &mut r,
        &heatmap_scene(0, emptied.clone()),
        1.0,
        Color::BLACK,
    );
    let fresh = render_on(
        gpu,
        &mut renderer(gpu),
        &heatmap_scene(0, emptied),
        1.0,
        Color::BLACK,
    );
    assert_eq!(
        img, fresh,
        "a ring updated column by column differs from a fresh one"
    );
    // Ring column 5 (x 24..28) is empty now: background.
    assert_eq!(img.pixel(25, 12), [0, 0, 0, 255]);
    // Ring column 2 (x 12..16) shows column 7's top row, as ring column 7 does.
    assert_eq!(img.pixel(13, 5), img.pixel(33, 5));
}

#[test]
fn heatmap_nan_is_transparent_and_rings_are_released() {
    let Some(gpu) = gpu("heatmap_nan_is_transparent_and_rings_are_released") else {
        return;
    };
    let mut r = renderer(gpu);
    let mut data: Vec<Option<HeatmapColumn>> = (0..8).map(|c| Some(column(c))).collect();
    let mut first: Vec<f32> = column(0).to_vec();
    first[0] = f32::NAN; // ring column 0, row 0: bottom-left cell (4x4 px).
    data[0] = Some(first.into());
    let img = render_on(gpu, &mut r, &heatmap_scene(0, data), 1.0, Color::BLACK);
    assert_eq!(
        img.pixel(5, 18),
        [0, 0, 0, 255],
        "NaN cell must show the background"
    );
    assert_ne!(img.pixel(9, 18), [0, 0, 0, 255]);
    // A scene without the heatmap releases it; it comes back empty.
    render(gpu, &mut r, &one_layer(40.0, 24.0, |_| {}), 1.0);
    let img = render_on(
        gpu,
        &mut r,
        &heatmap_scene(0, vec![None; 8]),
        1.0,
        Color::BLACK,
    );
    assert_eq!(
        img.pixel(20, 12),
        [0, 0, 0, 255],
        "released ring must restart empty"
    );
}

/// Time running up is the transpose of time running across: on a square rect, the pixel
/// at (x, y) of one is the pixel at (15 − y, 15 − x) of the other.
#[test]
fn heatmap_time_up_is_the_transpose() {
    let Some(gpu) = gpu("heatmap_time_up_is_the_transpose") else {
        return;
    };
    let cols: Vec<Option<HeatmapColumn>> = (0..4).map(|c| Some(column(c))).collect();
    let draw = |axes| {
        let s = one_layer(16.0, 16.0, |l| {
            l.heatmaps.push(heatmap_with(
                Rect::new(0.0, 0.0, 16.0, 16.0),
                4,
                4,
                axes,
                0,
                cols.clone(),
            ));
        });
        render(gpu, &mut renderer(gpu), &s, 1.0)
    };
    let across = draw(HeatmapAxes::TimeAcross);
    let up = draw(HeatmapAxes::TimeUp);
    assert_ne!(across, up);
    for y in 0..16 {
        for x in 0..16 {
            assert_eq!(
                up.pixel(x, y),
                across.pixel(15 - y, 15 - x),
                "pixel ({x}, {y})"
            );
        }
    }
}

/// A pixel over several cells shows the highest of them: one hot cell among cold ones
/// draws its pixel as hot as a pixel of hot cells only, along both axes.
#[test]
fn heatmap_pixel_shows_the_highest_cell() {
    let Some(gpu) = gpu("heatmap_pixel_shows_the_highest_cell") else {
        return;
    };
    // 16 columns x 16 rows on 4 x 4 pixels: 4 x 4 cells per pixel.
    let draw = |hot: &dyn Fn(u32, u32) -> bool| {
        let data = (0..16)
            .map(|c| {
                Some(
                    (0..16)
                        .map(|row| if hot(c, row) { 31.0 } else { 0.0 })
                        .collect::<HeatmapColumn>(),
                )
            })
            .collect();
        let s = one_layer(4.0, 4.0, |l| {
            l.heatmaps.push(heatmap_with(
                Rect::new(0.0, 0.0, 4.0, 4.0),
                16,
                16,
                HeatmapAxes::TimeAcross,
                0,
                data,
            ));
        });
        render(gpu, &mut renderer(gpu), &s, 1.0)
    };
    let one = draw(&|c, row| c == 5 && row == 9);
    let all = draw(&|_, _| true);
    let none = draw(&|_, _| false);
    // Column 5 is in pixel column 1, row 9 in pixel row 2 from the bottom (y = 1).
    assert_eq!(one.pixel(1, 1), all.pixel(1, 1));
    assert_eq!(one.pixel(0, 0), none.pixel(0, 0));
    assert_ne!(all.pixel(0, 0), none.pixel(0, 0));
}

/// Ink bounding box of channel 0 above `threshold`.
fn ink_box(img: &Image, threshold: u8) -> Option<[u32; 4]> {
    let mut b: Option<[u32; 4]> = None;
    for y in 0..img.height() {
        for x in 0..img.width() {
            if img.pixel(x, y)[0] > threshold {
                let e = b.get_or_insert([x, y, x, y]);
                e[0] = e[0].min(x);
                e[1] = e[1].min(y);
                e[2] = e[2].max(x);
                e[3] = e[3].max(y);
            }
        }
    }
    b
}

#[test]
fn label_anchors_place_text() {
    let Some(gpu) = gpu("label_anchors_place_text") else {
        return;
    };
    let mut r = renderer(gpu);
    let at = |r: &mut ac2_plot::Renderer, anchor: Anchor| {
        let s = one_layer(200.0, 80.0, |l| {
            l.labels.push(Label {
                // Cap-height glyphs only: no descenders, so ink bottom = baseline.
                text: "HEH".into(),
                pos: [100.0, 40.0],
                anchor,
                size: 20.0,
                color: Color::WHITE,
                clip: None,
            });
        });
        ink_box(&render_on(gpu, r, &s, 1.0, Color::BLACK), 100).expect("label has ink")
    };
    let c = at(
        &mut r,
        Anchor {
            h: HAlign::Center,
            v: VAlign::Baseline,
        },
    );
    let mid = (c[0] + c[2]) as f32 / 2.0;
    assert!((mid - 100.0).abs() <= 1.5, "centred label spans {c:?}");
    assert!(
        c[3].abs_diff(39) <= 1,
        "baseline label bottom at {}, want ~39",
        c[3]
    );
    let l = at(&mut r, Anchor::TOP_LEFT);
    assert!(
        l[0].abs_diff(100) <= 3,
        "left-anchored ink starts at {}",
        l[0]
    );
    assert!(
        l[1] > 40 && l[1] < 52,
        "top-anchored ink starts at {}",
        l[1]
    );
    let rt = at(
        &mut r,
        Anchor {
            h: HAlign::Right,
            v: VAlign::Bottom,
        },
    );
    assert!(
        rt[2].abs_diff(99) <= 3,
        "right-anchored ink ends at {}",
        rt[2]
    );
    assert!(
        rt[3] < 40 && rt[3] > 30,
        "bottom-anchored ink ends at {}",
        rt[3]
    );
}

/// The egui path: the scene drawn at an integer offset inside a larger target, under an
/// outer clip, equals the stand-alone render shifted and cropped.
#[test]
fn offset_target_matches_standalone_render() {
    let Some(gpu) = gpu("offset_target_matches_standalone_render") else {
        return;
    };
    let mut r = renderer(gpu);
    let s = one_layer(60.0, 40.0, |l| {
        l.polylines.push(white_line(
            vec![[0.0, 30.0], [20.0, 5.0], [40.0, 35.0], [60.0, 10.0]],
            3.0,
            0.8,
        ));
        l.labels.push(Label {
            text: "Aa".into(),
            pos: [30.0, 20.0],
            anchor: Anchor::CENTER,
            size: 12.0,
            color: Color::WHITE,
            clip: None,
        });
    });
    let alone = render_on(gpu, &mut r, &s, 1.0, Color::BLACK);

    let big = Offscreen::new(&gpu.device, [120, 90]);
    let target = FrameTarget {
        size_px: [120, 90],
        origin_px: [37.0, 21.0],
        scale: 1.0,
        // Outer clip cuts the scene's right 10 px.
        clip_px: [0.0, 0.0, 87.0, 90.0],
    };
    r.prepare(&gpu.device, &gpu.queue, &s, &target)
        .expect("prepare");
    let (px, _) = big.paint_prepared(gpu, &r, Color::BLACK).expect("render");
    let img = Image::new(px.width, px.height, px.rgba).expect("size");
    for y in 0..90 {
        for x in 0..120 {
            let (sx, sy) = (x as i32 - 37, y as i32 - 21);
            let want = if (0..50).contains(&sx) && (0..40).contains(&sy) {
                alone.pixel(sx as u32, sy as u32)
            } else {
                [0, 0, 0, 255]
            };
            assert_eq!(img.pixel(x, y), want, "pixel ({x},{y})");
        }
    }
}

#[test]
fn invalid_scenes_are_rejected() {
    let Some(gpu) = gpu("invalid_scenes_are_rejected") else {
        return;
    };
    let mut r = renderer(gpu);
    let t = FrameTarget::full([10, 10], 1.0);
    let s = one_layer(10.0, 10.0, |l| {
        let mut p = white_line(vec![[0.0, 0.0], [5.0, 5.0]], 1.0, 1.0);
        p.alpha.pop();
        l.polylines.push(p);
    });
    assert!(matches!(
        r.prepare(&gpu.device, &gpu.queue, &s, &t),
        Err(PrepareError::AlphaLength { .. })
    ));
    let mut short: Vec<Option<HeatmapColumn>> = vec![None; 8];
    short[3] = Some(vec![0.0; 5].into());
    let bad_upload = heatmap_scene(0, short);
    assert!(matches!(
        r.prepare(&gpu.device, &gpu.queue, &bad_upload, &t),
        Err(PrepareError::Heatmap { .. })
    ));
    assert!(matches!(
        r.prepare(
            &gpu.device,
            &gpu.queue,
            &heatmap_scene(0, vec![None; 7]),
            &t
        ),
        Err(PrepareError::Heatmap { .. })
    ));
    let mut twice = heatmap_scene(0, vec![None; 8]);
    let h = twice.layers[0].heatmaps[0].clone();
    twice.layers[0].heatmaps.push(h);
    assert!(matches!(
        r.prepare(&gpu.device, &gpu.queue, &twice, &t),
        Err(PrepareError::Heatmap { .. })
    ));
}
