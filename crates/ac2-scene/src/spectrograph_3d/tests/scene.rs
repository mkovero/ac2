//! The scene: labels, chrome, glow, axes on demand and the fit through an orbit.

use super::*;

struct Built {
    h: SpectrographHistory,
    s: Skyline,
}

fn pink_room(span: u32, tier: SkylineTier) -> Built {
    let mut h = SpectrographHistory::new(span);
    let mut f = Feed::new(&log_bins());
    for slot in 0..h.slots() as u64 {
        let l: Vec<f32> = f
            .edges
            .iter()
            .map(|e| (-30.0 - 3.0 * (e.0.max(20.0) / 1000.0).log2()) as f32)
            .collect();
        f.push_with(&mut h, slot, &l, None);
    }
    let mut s = Skyline::default();
    s.update(&h, full_range(), tier);
    Built { h, s }
}

fn scene_3d(
    b: &Built,
    view: &ViewState,
    camera: SpectrumCamera,
    forced: bool,
) -> Spectrograph3dScene {
    let tier = if forced {
        SkylineTier::Lite
    } else {
        SkylineTier::Full
    };
    scene_3d_as(b, view, camera, tier, forced, Colormap::Ice, 1.0)
}

fn scene_3d_as(
    b: &Built,
    view: &ViewState,
    camera: SpectrumCamera,
    tier: SkylineTier,
    forced: bool,
    colours: Colormap,
    axes: f32,
) -> Spectrograph3dScene {
    let input = Spectrograph3dInput {
        key: crate::trace::TraceKey::Live(ac2_proto::units::MeasId(1)),
        history: &b.h,
        skyline: &b.s,
        name: "Main L".into(),
        range: Range::new(-80.0, 0.0),
        offset_db: 0.0,
        freshness: None,
        colours,
        camera: Pose::from(camera),
        tier,
        tier_forced: forced,
        axes,
    };
    spectrograph_3d_scene(
        &[],
        &Status::default(),
        Some(&input),
        view,
        &Theme::dark(),
        Viewport {
            width: 1200.0,
            height: 600.0,
        },
    )
}

fn texts(s: &Spectrograph3dScene, axis: LabelAxis) -> Vec<&str> {
    s.labels
        .iter()
        .filter(|l| l.axis == axis)
        .map(|l| l.text.as_str())
        .collect()
}

fn skylines(s: &Scene) -> Vec<&Skyline3d> {
    s.layers.iter().flat_map(|l| &l.skylines).collect()
}

#[test]
fn labels_read_frequency_time_and_level() {
    let b = pink_room(30, SkylineTier::Full);
    let view = ViewState::default();
    let s = scene_3d(&b, &view, SpectrumCamera::DEFAULT, false);
    let f = texts(&s, LabelAxis::Frequency);
    for want in ["100", "1k", "10k"] {
        assert!(f.contains(&want), "{want} in {f:?}");
    }
    let t = texts(&s, LabelAxis::Time);
    assert!(t.contains(&"now"), "{t:?}");
    assert!(
        t.iter().any(|l| l.starts_with('−') && l.ends_with(" s")),
        "{t:?}"
    );
    let l = texts(&s, LabelAxis::Level);
    assert!(l.len() >= 3, "{l:?}");
    assert_eq!(texts(&s, LabelAxis::Unit), ["dBFS"]);
    // Frequency reads left to right.
    let x = |t: &str| {
        s.labels
            .iter()
            .find(|l| l.axis == LabelAxis::Frequency && l.text == t)
            .map(|l| l.pos[0])
            .expect("label")
    };
    assert!(x("100") < x("1k") && x("1k") < x("10k"));
    assert!(s.time_right);
}

/// No label is cut by the field's edge, at any pane size or camera, and the height scale's
/// numbers stand on one side of its post.
#[test]
fn labels_stay_whole_inside_the_field() {
    let b = pink_room(30, SkylineTier::Full);
    let view = ViewState::default();
    for (w, h) in [
        (1570.0, 950.0),
        (1200.0, 600.0),
        (730.0, 430.0),
        (420.0, 300.0),
    ] {
        for (yaw, pitch) in [(0, 8), (-35, 30), (80, 2), (-80, 75)] {
            let input = Spectrograph3dInput {
                key: crate::trace::TraceKey::Live(ac2_proto::units::MeasId(1)),
                history: &b.h,
                skyline: &b.s,
                name: "Main L".into(),
                range: Range::new(-100.0, 0.0),
                offset_db: 0.0,
                freshness: None,
                colours: Colormap::Ice,
                camera: Pose::from(SpectrumCamera {
                    yaw_deg: yaw,
                    pitch_deg: pitch,
                    zoom_step: 0,
                }),
                tier: SkylineTier::Full,
                tier_forced: false,
                axes: 1.0,
            };
            let s = spectrograph_3d_scene(
                &[],
                &Status::default(),
                Some(&input),
                &view,
                &Theme::dark(),
                Viewport {
                    width: w,
                    height: h,
                },
            );
            let drawn: Vec<&Label> = s
                .scene
                .layers
                .iter()
                .flat_map(|l| &l.labels)
                .filter(|l| s.labels.iter().any(|a| a.pos == l.pos && a.text == l.text))
                .collect();
            assert_eq!(drawn.len(), s.labels.len(), "{w}×{h} {yaw}/{pitch}");
            for l in &drawn {
                let r = label_box(l);
                assert!(
                    r.x >= s.plot.x
                        && r.right() <= s.plot.right()
                        && r.y >= s.plot.y
                        && r.bottom() <= s.plot.bottom(),
                    "{:?} at {w}×{h} {yaw}/{pitch}: {r:?} outside {:?}",
                    l.text,
                    s.plot
                );
            }
            let sides: Vec<HAlign> = s
                .labels
                .iter()
                .filter(|a| a.axis == LabelAxis::Level)
                .filter_map(|a| drawn.iter().find(|l| l.pos == a.pos).map(|l| l.anchor.h))
                .collect();
            assert!(
                sides.windows(2).all(|p| p[0] == p[1]),
                "{w}×{h} {yaw}/{pitch}: {sides:?}"
            );
            // A 1080p pane at the default camera: the numbers outside the post (right of
            // its unit), the scale whole up to the range's top.
            if w > 1500.0 && yaw == 0 {
                let unit_x = s
                    .labels
                    .iter()
                    .find(|a| a.axis == LabelAxis::Unit)
                    .map(|a| a.pos[0])
                    .expect("unit");
                assert_eq!(sides.first(), Some(&HAlign::Left));
                assert!(
                    s.labels
                        .iter()
                        .filter(|a| a.axis == LabelAxis::Level)
                        .all(|a| a.pos[0] > unit_x),
                    "{:?}",
                    s.labels
                );
                assert!(texts(&s, LabelAxis::Level).contains(&"0"));
            }
        }
    }
}

#[test]
fn the_time_axis_follows_the_eye_and_decades_survive_a_steep_yaw() {
    let b = pink_room(30, SkylineTier::Full);
    let view = ViewState::default();
    let at = |yaw| {
        scene_3d(
            &b,
            &view,
            SpectrumCamera {
                yaw_deg: yaw,
                ..SpectrumCamera::DEFAULT
            },
            false,
        )
    };
    let now = |s: &Spectrograph3dScene| {
        s.labels
            .iter()
            .find(|l| l.text == "now")
            .map(|l| l.pos[0])
            .expect("now")
    };
    let (left, right) = (at(-40), at(40));
    assert!(!left.time_right && right.time_right);
    assert!(now(&left) < left.plot.x + left.plot.w / 2.0);
    assert!(now(&right) > right.plot.x + right.plot.w / 2.0);
    let steep = at(80);
    let f = texts(&steep, LabelAxis::Frequency);
    assert!(!f.is_empty());
    assert!(f.len() < texts(&at(0), LabelAxis::Frequency).len());
    // Thinned: no two labels share a place.
    for (i, a) in steep.labels.iter().enumerate() {
        for b in &steep.labels[i + 1..] {
            assert!(
                (a.pos[0] - b.pos[0]).abs() > 1.0 || (a.pos[1] - b.pos[1]).abs() > 1.0,
                "{a:?} {b:?}"
            );
        }
    }
}

#[test]
fn chrome_levels_and_draw_order() {
    let b = pink_room(30, SkylineTier::Full);
    let mut view = ViewState::default();
    let full = scene_3d(&b, &view, SpectrumCamera::DEFAULT, false);
    let lines = |s: &Spectrograph3dScene| -> usize {
        s.scene.layers.iter().map(|l| l.polylines.len()).sum()
    };
    let dashed = |s: &Spectrograph3dScene| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.polylines)
            .filter(|p| p.stroke.dash.is_some())
            .count()
    };
    assert_eq!(dashed(&full), 1, "the ceiling line");
    view.chrome = PlotChrome::NoGrid;
    let no_grid = scene_3d(&b, &view, SpectrumCamera::DEFAULT, false);
    assert!(lines(&no_grid) < lines(&full));
    assert_eq!(dashed(&no_grid), 1);
    assert!(!no_grid.labels.is_empty());
    view.chrome = PlotChrome::Bare;
    let bare = scene_3d(&b, &view, SpectrumCamera::DEFAULT, false);
    assert!(bare.labels.is_empty());
    assert_eq!(dashed(&bare), 0);
    assert_eq!(skylines(&bare.scene).len(), 1, "spikes stay");
    // Field and floor band under the spikes; scale, ceiling and labels over them.
    let idx = |pred: &dyn Fn(&Layer) -> bool| full.scene.layers.iter().position(pred);
    let field = idx(&|l| l.rects.iter().any(|r| r.color == FIELD)).expect("field");
    let band = idx(&|l| !l.polygons.is_empty()).expect("floor band");
    let sky = idx(&|l| !l.skylines.is_empty()).expect("skyline");
    let ceiling = idx(&|l| l.polylines.iter().any(|p| p.stroke.dash.is_some())).expect("ceiling");
    let labels = idx(&|l| l.labels.iter().any(|t| t.text == "1k")).expect("labels");
    assert!(field <= band && band < sky && sky <= ceiling && sky <= labels);
}

#[test]
fn the_glow_follows_the_ceiling_and_precedes_the_labels() {
    let b = pink_room(30, SkylineTier::Full);
    let s = scene_3d(&b, &ViewState::default(), SpectrumCamera::DEFAULT, false);
    let glows: Vec<_> = s.scene.layers.iter().flat_map(|l| &l.glows).collect();
    assert_eq!(glows.len(), 1);
    assert_eq!(glows[0].id, skylines(&s.scene)[0].id);
    assert_eq!(glows[0].gain, GLOW_ICE);
    // Glows draw after the ceiling in its layer; the labels and their plates in a later
    // layer, never bloomed.
    let l = s
        .scene
        .layers
        .iter()
        .find(|l| !l.glows.is_empty())
        .expect("glow layer");
    assert!(
        l.polylines.iter().any(|p| p.stroke.dash.is_some()),
        "ceiling"
    );
    let sky = s.scene.layers.iter().position(|l| !l.skylines.is_empty());
    let glow = s.scene.layers.iter().position(|l| !l.glows.is_empty());
    let labels = s
        .scene
        .layers
        .iter()
        .position(|l| l.labels.iter().any(|t| t.text == "1k"));
    assert!(sky <= glow && glow < labels);
}

#[test]
fn the_glow_by_tier_and_colour_map() {
    let view = ViewState::default();
    let glows = |tier, colours| {
        let b = pink_room(30, tier);
        let s = scene_3d_as(&b, &view, SpectrumCamera::DEFAULT, tier, true, colours, 1.0);
        s.scene
            .layers
            .iter()
            .flat_map(|l| l.glows.clone())
            .map(|g| g.gain)
            .collect::<Vec<_>>()
    };
    assert_eq!(glows(SkylineTier::Lite, Colormap::Ice), [GLOW_ICE]);
    assert_eq!(glows(SkylineTier::Full, Colormap::Viridis), [GLOW_MAP]);
    assert!(glows(SkylineTier::Bare, Colormap::Ice).is_empty());
    assert_eq!(SkylineTier::Full.bloom_taps(), Some(9));
    assert_eq!(SkylineTier::Lite.bloom_taps(), Some(5));
}

#[test]
fn the_skyline_primitive_carries_the_grid_and_the_camera() {
    let b = pink_room(30, SkylineTier::Full);
    let s = scene_3d(&b, &ViewState::default(), SpectrumCamera::DEFAULT, false);
    let sk = skylines(&s.scene);
    assert_eq!(sk.len(), 1);
    let k = sk[0];
    assert_eq!((k.cols, k.rows), (192, 101));
    assert_eq!(k.data.len(), 101);
    assert_eq!(k.range, [-80.0, 0.0]);
    assert_eq!(k.colormap, Colormap::Ice);
    assert_eq!(k.rect, s.plot);
    assert_eq!(k.look, LOOK);
    assert_eq!(k.view_proj, s.view_proj.gpu());
    assert!(k.phase > 0.0 && k.phase <= 1.0);
}

#[test]
fn caption_names_the_cell_and_a_forced_tier() {
    let b = pink_room(30, SkylineTier::Full);
    let view = ViewState::default();
    let s = scene_3d(&b, &view, SpectrumCamera::DEFAULT, false);
    assert_eq!(
        s.caption,
        "Main L · last 30 s · dBFS (tone) · 1/19 oct · 0.30 s"
    );
    let lite = pink_room(30, SkylineTier::Lite);
    let s = scene_3d(&lite, &view, SpectrumCamera::DEFAULT, true);
    assert_eq!(
        s.caption,
        "Main L · last 30 s · dBFS (tone) · 1/13 oct · 0.63 s · lite"
    );
}

#[test]
fn empty_and_missing_say_so() {
    let h = SpectrographHistory::new(30);
    let mut s = Skyline::default();
    s.update(&h, full_range(), SkylineTier::Full);
    let b = Built { h, s };
    let sc = scene_3d(&b, &ViewState::default(), SpectrumCamera::DEFAULT, false);
    assert_eq!(sc.message.as_deref(), Some("Main L: no frames yet"));
    assert!(skylines(&sc.scene).is_empty());
    let none = spectrograph_3d_scene(
        &[],
        &Status::default(),
        None,
        &ViewState::default(),
        &Theme::light(),
        Viewport {
            width: 800.0,
            height: 400.0,
        },
    );
    assert_eq!(
        none.message.as_deref(),
        Some("no spectrum or RTA measurement")
    );
    // The field is dark in the light theme too.
    assert!(
        none.scene
            .layers
            .iter()
            .any(|l| l.rects.iter().any(|r| r.color == FIELD))
    );
}

#[test]
fn the_floor_band_fades_with_depth() {
    let b = pink_room(30, SkylineTier::Full);
    let s = scene_3d(&b, &ViewState::default(), SpectrumCamera::DEFAULT, false);
    let polys: Vec<_> = s.scene.layers.iter().flat_map(|l| &l.polygons).collect();
    assert!(polys.len() >= 10);
    let lum = |c: Color| c.r + c.g + c.b;
    assert!(lum(polys[0].color) > lum(polys[polys.len() - 1].color));
    assert!(lum(polys[polys.len() - 1].color) >= lum(FIELD));
}

#[test]
fn heights_follow_the_range_and_offset() {
    let b = pink_room(30, SkylineTier::Full);
    let input = Spectrograph3dInput {
        key: crate::trace::TraceKey::Live(ac2_proto::units::MeasId(1)),
        history: &b.h,
        skyline: &b.s,
        name: "Main L".into(),
        range: Range::new(-60.0, -10.0),
        offset_db: 6.0,
        freshness: None,
        colours: Colormap::Viridis,
        camera: Pose::from(SpectrumCamera::DEFAULT),
        tier: SkylineTier::Full,
        tier_forced: false,
        axes: 1.0,
    };
    let s = spectrograph_3d_scene(
        &[],
        &Status::default(),
        Some(&input),
        &ViewState {
            freq: FreqRange::default(),
            ..ViewState::default()
        },
        &Theme::dark(),
        Viewport {
            width: 900.0,
            height: 500.0,
        },
    );
    // The offset moves the levels up, so the range in stored levels moves down.
    assert_eq!(skylines(&s.scene)[0].range, [-66.0, -16.0]);
    assert!(s.caption.starts_with("Main L · offset"), "{}", s.caption);
}

#[test]
fn the_axes_fade_with_hover_and_hold_after_a_touch() {
    let mut r = AxesReveal::default();
    // Hidden at rest.
    assert_eq!(r.alpha(10.0), 0.0);
    assert_eq!(r.next_change(10.0), None);
    // Shown on hover, after a short fade in.
    r.hover(true, 10.0);
    assert_eq!(r.next_change(10.0), Some(0.0));
    assert!(r.alpha(10.0 + AXES_FADE_IN_S / 2.0) > 0.0);
    assert!(r.alpha(10.0 + AXES_FADE_IN_S / 2.0) < 1.0);
    assert_eq!(r.alpha(10.0 + AXES_FADE_IN_S), 1.0);
    assert_eq!(r.alpha(100.0), 1.0, "for as long as the pointer stays");
    assert_eq!(r.next_change(100.0), None);
    // Gone a fade after the pointer leaves.
    r.hover(false, 100.0);
    assert!(r.alpha(100.0 + AXES_FADE_OUT_S / 2.0) > 0.0);
    assert_eq!(r.alpha(100.0 + AXES_FADE_OUT_S), 0.0);
    assert_eq!(r.next_change(100.0 + AXES_FADE_OUT_S), None);

    // A touch shows them for the hold, then they fade.
    let mut r = AxesReveal::default();
    r.poke(20.0);
    assert_eq!(r.alpha(20.0 + AXES_FADE_IN_S), 1.0);
    let held = 20.0 + AXES_HOLD_S - 0.5;
    assert_eq!(r.alpha(held), 1.0);
    let wait = r.next_change(held).expect("fades later");
    assert!((wait - 0.5).abs() < 1e-9, "{wait}");
    let fading = 20.0 + AXES_HOLD_S + AXES_FADE_OUT_S / 2.0;
    assert!(r.alpha(fading) > 0.0 && r.alpha(fading) < 1.0);
    assert_eq!(r.next_change(fading), Some(0.0));
    assert_eq!(r.alpha(20.0 + AXES_HOLD_S + AXES_FADE_OUT_S), 0.0);
    assert_eq!(r.next_change(20.0 + AXES_HOLD_S + AXES_FADE_OUT_S), None);

    // A second touch while fading out carries on from where the axes are: no jump.
    let mut r = AxesReveal::default();
    r.poke(0.0);
    let t = AXES_HOLD_S + AXES_FADE_OUT_S / 2.0;
    let before = r.alpha(t);
    r.poke(t);
    assert!((r.alpha(t) - before).abs() < 1e-6);
    assert_eq!(r.alpha(t + AXES_FADE_IN_S), 1.0);
    // A touch while already shown keeps them shown, and restarts the hold.
    r.poke(t + 1.0);
    assert_eq!(r.alpha(t + 1.0), 1.0);
    assert_eq!(r.alpha(t + 1.0 + AXES_HOLD_S - 0.01), 1.0);
}

#[test]
fn axes_at_rest_leave_the_skyline_band_and_caption() {
    let b = pink_room(30, SkylineTier::Full);
    let view = ViewState::default();
    let cam = SpectrumCamera::DEFAULT;
    let shown = scene_3d_as(&b, &view, cam, SkylineTier::Full, false, Colormap::Ice, 1.0);
    let rest = scene_3d_as(&b, &view, cam, SkylineTier::Full, false, Colormap::Ice, 0.0);
    let half = scene_3d_as(&b, &view, cam, SkylineTier::Full, false, Colormap::Ice, 0.5);
    let lines = |s: &Spectrograph3dScene| -> usize {
        s.scene.layers.iter().map(|l| l.polylines.len()).sum()
    };
    let texts = |s: &Spectrograph3dScene| -> Vec<String> {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .map(|l| l.text.clone())
            .collect()
    };
    // At rest: no labels, grid, post or ceiling; the now line, spikes and caption stay.
    assert!(rest.labels.is_empty());
    assert_eq!(lines(&rest), 1, "the now line alone");
    assert_eq!(skylines(&rest.scene).len(), 1);
    assert!(!rest.caption.is_empty());
    assert_eq!(texts(&rest), vec![rest.caption.clone()]);
    assert!(!shown.labels.is_empty());
    assert!(lines(&shown) > 1);
    // Half shown: the same labels in the same places, fainter.
    assert_eq!(half.labels, shown.labels);
    let alpha_of = |s: &Spectrograph3dScene, t: &str| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .find(|l| l.text == t)
            .map(|l| l.color.a)
    };
    let a1 = alpha_of(&shown, "1k").expect("1k shown");
    let a2 = alpha_of(&half, "1k").expect("1k half");
    assert!((a2 - a1 * 0.5).abs() < 1e-6);
    // T traces-only never shows them, even hovered.
    let bare_view = ViewState {
        chrome: PlotChrome::Bare,
        ..ViewState::default()
    };
    let bare = scene_3d_as(
        &b,
        &bare_view,
        cam,
        SkylineTier::Full,
        false,
        Colormap::Ice,
        1.0,
    );
    assert!(bare.labels.is_empty());
    assert_eq!(lines(&bare), 1);
}

#[test]
fn labels_stand_on_faint_plates() {
    let b = pink_room(30, SkylineTier::Full);
    let s = scene_3d(&b, &ViewState::default(), SpectrumCamera::DEFAULT, false);
    let layer = s
        .scene
        .layers
        .iter()
        .find(|l| l.labels.iter().any(|t| t.text == "1k"))
        .expect("labels");
    let plates: Vec<_> = layer
        .polygons
        .iter()
        .filter(|p| p.color.a > 0.0 && p.color.a < 1.0)
        .collect();
    assert_eq!(plates.len(), s.labels.len(), "one plate a label");
    for p in &plates {
        assert!(p.color.a <= PLATE_ALPHA);
        assert!(p.points.len() >= 8, "rounded");
    }
}

#[test]
fn the_box_fits_at_zoom_zero_from_every_angle() {
    let lim = 1.0 - FIT_MARGIN + 1e-6;
    for aspect in [0.8, 1.0, 1.78, 3.0] {
        for yaw in (-80..=80).step_by(5) {
            for pitch in (2..=75).step_by(4) {
                let pose = Pose {
                    yaw_deg: f64::from(yaw),
                    pitch_deg: f64::from(pitch),
                    zoom_step: 0.0,
                };
                let vp = view_proj(pose, aspect);
                for p in box_corners() {
                    let n = vp.ndc(p).expect("in front of the eye");
                    assert!(
                        n[0].abs() <= lim && n[1].abs() <= lim,
                        "{p:?} at yaw {yaw} pitch {pitch} aspect {aspect}: {n:?}"
                    );
                }
            }
        }
    }
    // The orbit the app review shot: the front floor corner is in the picture.
    let vp = view_proj(
        Pose {
            yaw_deg: -35.0,
            pitch_deg: 30.0,
            zoom_step: 0.0,
        },
        1.78,
    );
    for x in [-FLOOR_HALF_W, FLOOR_HALF_W] {
        let n = vp.ndc([x, 0.0, 0.0]).expect("front corner");
        assert!(n[1] >= -1.0 + FIT_MARGIN - 1e-6, "{n:?}");
    }
}

#[test]
fn the_fit_moves_smoothly_through_an_orbit() {
    // A camera easing through yaw and pitch dollies with the fit: no step between angles
    // a frame apart.
    let d = |yaw: f64, pitch: f64| {
        eye_distance(
            Pose {
                yaw_deg: yaw,
                pitch_deg: pitch,
                zoom_step: 0.0,
            },
            1.78,
        )
    };
    for pitch in [2.0, 8.0, 30.0, 75.0] {
        let mut prev = d(-80.0, pitch);
        let mut yaw = -80.0;
        while yaw < 80.0 {
            yaw += 0.1;
            let now = d(yaw, pitch);
            assert!((now - prev).abs() < 0.01 * prev, "yaw {yaw} pitch {pitch}");
            prev = now;
        }
    }
    let mut prev = d(-35.0, 2.0);
    let mut pitch = 2.0;
    while pitch < 75.0 {
        pitch += 0.1;
        let now = d(-35.0, pitch);
        assert!((now - prev).abs() < 0.01 * prev, "pitch {pitch}");
        prev = now;
    }
    // Zoom steps scale from the angle's own fit.
    let pose = Pose {
        yaw_deg: -35.0,
        pitch_deg: 30.0,
        zoom_step: 0.0,
    };
    let near = Pose {
        zoom_step: 2.0,
        ..pose
    };
    let r = eye_distance(near, 1.78) / fitted_distance(pose, 1.78);
    assert!((r - 2f64.powf(-0.25)).abs() < 1e-9);
}
