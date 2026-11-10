//! Reducer tests of the 3D spectrograph's camera: G reaches the view, the frequency axis's
//! keys orbit, tilt and dolly the camera instead of moving the axis, and a drag stores one
//! camera on release.

use super::*;
use ac2_proto::model::layout::SpectrumCamera;
use ac2_scene::view::SpectrumMode;

/// The spectrum pane focused, on the 3D spectrograph.
fn on_3d() -> T {
    let mut t = T::new();
    t.key("Alt+2");
    t.st.rig.spectrographs.insert(
        MeasId(2),
        ac2_scene::spectrograph::SpectrographHistory::new(30),
    );
    for _ in 0..3 {
        t.key("G");
    }
    assert_eq!(
        t.st.kind_modes(PaneKind::Spectrum).spectrum,
        SpectrumMode::Spectrograph3d
    );
    t
}

fn camera(t: &T) -> SpectrumCamera {
    t.st.layout.focused().modes.spectrum_camera
}

fn tick(t: &mut T, dt_s: f64) {
    t.st.apply(Msg::Tick { now_s: 0.0, dt_s }, &t.keys);
}

#[test]
fn g_walks_four_views_and_back() {
    let mut t = on_3d();
    assert_eq!(t.st.cam3d_target(), Some(t.st.layout.focus));
    t.key("G");
    assert_eq!(
        t.st.kind_modes(PaneKind::Spectrum).spectrum,
        SpectrumMode::Spectrum
    );
    assert_eq!(t.st.cam3d_target(), None);
    t.key("Shift+G");
    assert_eq!(
        t.st.kind_modes(PaneKind::Spectrum).spectrum,
        SpectrumMode::Spectrograph3d
    );
}

/// A GPU without the 3D view (a Pi's V3D): G walks three views, and a layout that names
/// the 3D one has the spectrograph's keys, not the camera's.
#[test]
fn without_the_3d_view_g_skips_it() {
    let mut t = on_3d();
    t.st.auto_tier = None;
    assert_eq!(t.st.cam3d_target(), None);
    assert_eq!(t.st.scope(), crate::keys::Scope::Spectrum);
    let cam = camera(&t);
    t.key("ArrowLeft");
    assert_eq!(camera(&t), cam, "no camera to move");
    // From the flat drawing of the layout's 3D view, as from the spectrograph.
    t.key("G");
    let mode = |t: &T| t.st.kind_modes(PaneKind::Spectrum).spectrum;
    assert_eq!(mode(&t), SpectrumMode::Spectrum);
    for want in [
        SpectrumMode::Split,
        SpectrumMode::Spectrograph,
        SpectrumMode::Spectrum,
    ] {
        t.key("G");
        assert_eq!(mode(&t), want);
    }
    for want in [
        SpectrumMode::Spectrograph,
        SpectrumMode::Split,
        SpectrumMode::Spectrum,
    ] {
        t.key("Shift+G");
        assert_eq!(mode(&t), want);
    }
    // Settings cannot force it: the quality stays, the toast says why.
    let q = t.st.prefs.quality_3d;
    t.st.step_quality_3d(1);
    assert_eq!(t.st.prefs.quality_3d, q);
    assert_eq!(t.last_toast(), "3D quality: unavailable on this GPU");
}

#[test]
fn keys_move_the_camera_not_the_frequency_axis() {
    let mut t = on_3d();
    let freq = t.st.view.freq;
    let d = SpectrumCamera::DEFAULT;
    t.key("ArrowLeft");
    assert_eq!(camera(&t).yaw_deg, d.yaw_deg - 15);
    t.key("ArrowRight");
    t.key("ArrowRight");
    assert_eq!(camera(&t).yaw_deg, d.yaw_deg + 15);
    t.key("PageUp");
    assert_eq!(camera(&t).pitch_deg, d.pitch_deg + 5);
    t.key("PageDown");
    t.key("PageDown");
    assert_eq!(camera(&t).pitch_deg, d.pitch_deg - 5);
    t.key("I");
    assert_eq!(camera(&t).zoom_step, 2);
    t.key("O");
    t.key("O");
    assert_eq!(camera(&t).zoom_step, -2);
    for _ in 0..40 {
        tick(&mut t, 0.1);
    }
    assert_eq!(t.st.view.freq, freq, "the frequency axis stays put");
    // The drawn camera settled on the stored one.
    let drawn = t.st.drawn_camera(t.st.layout.focus);
    assert_eq!(drawn.snapped(), camera(&t));
    t.key("Home");
    assert_eq!(camera(&t), d);
}

#[test]
fn camera_limits_hold() {
    let mut t = on_3d();
    for _ in 0..20 {
        t.key("ArrowRight");
        t.key("PageUp");
        t.key("I");
    }
    let c = camera(&t);
    assert_eq!(c.yaw_deg, SpectrumCamera::YAW_MAX_DEG);
    assert_eq!(c.pitch_deg, *SpectrumCamera::PITCH_DEG.end());
    assert_eq!(c.zoom_step, *SpectrumCamera::ZOOM_STEPS.end());
    assert!(c.check().is_ok());
}

#[test]
fn cursor_and_compare_keys_are_inert() {
    let mut t = on_3d();
    let freq = t.st.view.freq;
    for k in ["Shift+T", "C"] {
        t.key(k);
    }
    assert_eq!(camera(&t), SpectrumCamera::DEFAULT);
    assert_eq!(t.st.view.freq, freq);
    assert_eq!(
        t.st.kind_modes(PaneKind::Spectrum).spectrum,
        SpectrumMode::Spectrograph3d
    );
}

#[test]
fn a_drag_stores_one_camera_on_release() {
    let mut t = on_3d();
    let pane = t.st.layout.focus;
    t.st.apply(
        Msg::Cam3d(pane, Cam3dMsg::Drag { dx: 50.0, dy: 0.0 }),
        &t.keys,
    );
    t.st.apply(
        Msg::Cam3d(pane, Cam3dMsg::Drag { dx: 0.0, dy: -50.0 }),
        &t.keys,
    );
    assert_eq!(
        camera(&t),
        SpectrumCamera::DEFAULT,
        "stored only on release"
    );
    let drawn = t.st.drawn_camera(pane);
    // The eye orbits with the pointer: a drag right turns the floor towards the right.
    assert!(drawn.yaw_deg < 0.0 && drawn.pitch_deg > 8.0, "{drawn:?}");
    t.st.apply(Msg::Cam3d(pane, Cam3dMsg::Release), &t.keys);
    assert_eq!(camera(&t), drawn.snapped());
    assert_ne!(camera(&t), SpectrumCamera::DEFAULT);
    t.st.apply(Msg::Cam3d(pane, Cam3dMsg::Dolly { steps: 3 }), &t.keys);
    assert_eq!(camera(&t).zoom_step, 3);
    // Esc closes things; it never resets the camera.
    let before = camera(&t);
    t.key("Escape");
    assert_eq!(camera(&t), before);
}

#[test]
fn mouse_on_a_2d_pane_is_ignored() {
    let mut t = T::new();
    t.key("Alt+2");
    let pane = t.st.layout.focus;
    t.st.apply(
        Msg::Cam3d(pane, Cam3dMsg::Drag { dx: 50.0, dy: 0.0 }),
        &t.keys,
    );
    t.st.apply(Msg::Cam3d(pane, Cam3dMsg::Release), &t.keys);
    assert_eq!(camera(&t), SpectrumCamera::DEFAULT);
}

fn at(t: &mut T, now_s: f64) {
    t.st.apply(Msg::Tick { now_s, dt_s: 0.0 }, &t.keys);
}

/// The axes come on demand: hidden at rest, shown while hovered, held a few seconds after a
/// touch, a camera key or a level-range change, and the UI keeps drawing only while they
/// fade.
#[test]
fn the_axes_show_on_hover_and_after_an_interaction() {
    use ac2_scene::spectrograph_3d::{AXES_FADE_IN_S, AXES_FADE_OUT_S, AXES_HOLD_S};
    let mut t = on_3d();
    let p = t.st.layout.focus;
    at(&mut t, 10.0);
    assert_eq!(t.st.axes3d_alpha(p), 0.0);
    assert!(!t.st.animating());
    assert_eq!(t.st.axes3d_wait(), None);

    t.st.apply(Msg::Cam3d(p, Cam3dMsg::Hover(true)), &t.keys);
    assert!(t.st.animating(), "fading in");
    at(&mut t, 10.0 + AXES_FADE_IN_S);
    assert_eq!(t.st.axes3d_alpha(p), 1.0);
    assert!(!t.st.animating(), "shown and still: no redraws");
    t.st.apply(Msg::Cam3d(p, Cam3dMsg::Hover(false)), &t.keys);
    at(&mut t, 11.0 + AXES_FADE_OUT_S);
    assert_eq!(t.st.axes3d_alpha(p), 0.0);

    // A touch holds them, waking once for the fade.
    at(&mut t, 20.0);
    t.st.apply(Msg::Cam3d(p, Cam3dMsg::Touch), &t.keys);
    at(&mut t, 21.0);
    assert_eq!(t.st.axes3d_alpha(p), 1.0);
    assert!(!t.st.animating());
    let wait = t.st.axes3d_wait().expect("fades later");
    assert!((wait - (AXES_HOLD_S - 1.0)).abs() < 1e-9);
    at(&mut t, 20.0 + AXES_HOLD_S + AXES_FADE_OUT_S / 2.0);
    assert!(t.st.animating(), "fading out");
    at(&mut t, 20.0 + AXES_HOLD_S + AXES_FADE_OUT_S);
    assert_eq!(t.st.axes3d_alpha(p), 0.0);
    assert_eq!(t.st.axes3d_wait(), None);

    // A camera key and a level-range change show them too.
    let mut now = 100.0;
    for key in ["ArrowLeft", "PageUp", "I", "Home", "Ctrl+ArrowUp"] {
        at(&mut t, now);
        assert_eq!(t.st.axes3d_alpha(p), 0.0, "{key}");
        t.key(key);
        at(&mut t, now + 1.0);
        assert_eq!(t.st.axes3d_alpha(p), 1.0, "{key}");
        now += 10.0;
    }
    // So does the mouse's dolly.
    at(&mut t, now);
    t.st.apply(Msg::Cam3d(p, Cam3dMsg::Dolly { steps: -1 }), &t.keys);
    at(&mut t, now + 1.0);
    assert_eq!(t.st.axes3d_alpha(p), 1.0);
}
