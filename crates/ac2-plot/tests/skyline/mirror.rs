//! The lake mirror: a second channel's spikes hang from the floor, drawn in the same target
//! as the first's.

use super::*;
use ac2_plot::SkylineMirror;

fn lake_view(cam: SpectrumCamera) -> ViewProj {
    spectrograph_3d::view_proj(Pose::from(cam), f64::from(W / H), Stage::Lake)
}

/// `s` framed for the lake, with `mirror` (same size ring) hanging below the floor.
fn lake_scene(s: &Sky, mirror: Option<Vec<Option<HeatmapColumn>>>, field: bool) -> ac2_plot::Scene {
    let vp = lake_view(s.cam);
    let mut sk = s.primitive();
    sk.view_proj = vp.gpu();
    sk.eye = vp.eye.map(|v| v as f32);
    sk.mirror = mirror.map(|data| SkylineMirror {
        cols: s.cols,
        rows: s.rows,
        newest: 0,
        newest_id: s.newest_id,
        phase: s.phase,
        range: s.range,
        data,
    });
    let rect = Rect::new(0.0, 0.0, W, H);
    let mut l = Layer::default();
    if field {
        l.rects.push(FillRect::new(rect, FIELD));
    }
    l.skylines.push(sk);
    if s.glow && s.tier.bloom_taps().is_some() {
        l.glows.push(SkylineGlow {
            id: SkylineId(1),
            clip: Some(rect),
            gain: glow_gain(s.colormap),
        });
    }
    scene(W, H, vec![l])
}

fn lake_draw(s: &Sky, mirror: Option<Vec<Option<HeatmapColumn>>>) -> Image {
    let gpu = gpu("skyline mirror").expect("checked by the caller");
    render_on(
        gpu,
        &mut renderer(gpu),
        &lake_scene(s, mirror, false),
        1.0,
        Color::BLACK,
    )
}

#[test]
fn a_mirrored_spike_hangs_below_the_floor_where_its_channel_has_level() {
    let Some(_) = gpu("a_mirrored_spike_hangs_below_the_floor_where_its_channel_has_level") else {
        return;
    };
    let (cols, rows) = (8, 4);
    let (c, j) = (5u32, 1u32);
    // The first channel empty: everything lit is the mirror's.
    let s = Sky::new(cols, rows, vec![None; rows as usize]);
    let vp = lake_view(s.cam);
    let foot = spectrograph_3d::spike_foot(
        cols as usize,
        rows as usize,
        (c as usize, j as usize),
        1.0,
        0,
    );
    let tip = pixel(&vp, [foot[0], -MAX_HEIGHT, foot[2]]);
    let foot_px = pixel(&vp, foot);
    assert!(
        tip[1] > foot_px[1] + 10.0,
        "hangs down: {foot_px:?} → {tip:?}"
    );
    let near_tip = [
        foot_px[0] + 0.97 * (tip[0] - foot_px[0]),
        foot_px[1] + 0.97 * (tip[1] - foot_px[1]),
    ];
    let lit_at = |img: &Image| lit(img.pixel(near_tip[0] as u32, near_tip[1] as u32));
    let with = lake_draw(&s, Some(lone(cols, rows, c, j, 0.0)));
    assert!(lit_at(&with), "nothing at {near_tip:?}");
    // Nothing above the spike's foot: the mirror never stands on the floor.
    for py in 0..(foot_px[1] - 2.0) as u32 {
        for px in 0..W as u32 {
            assert!(
                !lit(with.pixel(px, py)),
                "lit above the floor at ({px},{py})"
            );
        }
    }
    // Without the mirror, or with a mirror below its range, the lake stays dark.
    let without = lake_draw(&s, None);
    assert!(!lit_at(&without));
    let quiet = lake_draw(&s, Some(lone(cols, rows, c, j, -200.0)));
    assert!((0..H as u32).all(|py| (0..W as u32).all(|px| !lit(quiet.pixel(px, py)))));
}

#[test]
fn both_halves_meet_at_the_floor_as_reflections() {
    let Some(_) = gpu("both_halves_meet_at_the_floor_as_reflections") else {
        return;
    };
    let (cols, rows) = (8, 4);
    let data = lone(cols, rows, 3, 2, 0.0);
    let s = Sky::new(cols, rows, data.clone());
    // The same level in both: the picture is symmetric about the floor's spike foot.
    let img = lake_draw(&s, Some(data));
    let vp = lake_view(s.cam);
    let foot = spectrograph_3d::spike_foot(cols as usize, rows as usize, (3, 2), 1.0, 0);
    let up = pixel(&vp, [foot[0], 0.9 * MAX_HEIGHT, foot[2]]);
    let down = pixel(&vp, [foot[0], -0.9 * MAX_HEIGHT, foot[2]]);
    assert!(
        lit(img.pixel(up[0] as u32, up[1] as u32)),
        "upper at {up:?}"
    );
    assert!(
        lit(img.pixel(down[0] as u32, down[1] as u32)),
        "lower at {down:?}"
    );
}

/// Two different channels on the lake, bloom on: the stereo look.
#[test]
fn skyline_lake_golden() {
    let name = "skyline_lake_tonal_over_pink_room";
    let Some(gpu) = gpu(name) else {
        return;
    };
    let s = sky_for(SkylineTier::Full, tonal);
    let lower = pink_room(s.cols, s.rows);
    let img = render_on(
        gpu,
        &mut renderer(gpu),
        &lake_scene(&s, Some(lower), true),
        1.0,
        Color::BLACK,
    );
    golden(name, &img, LINES);
}
