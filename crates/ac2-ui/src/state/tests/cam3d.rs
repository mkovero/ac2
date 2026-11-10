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
