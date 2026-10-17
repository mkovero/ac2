//! Reducer tests of the level axis: zoom, pan, fit and reset per pane, and the fit a
//! started spectrum gets on its first frame.

use super::*;

#[test]
fn level_axis_zooms_pans_fits_and_resets_per_pane() {
    let mut t = T::new();
    let d = ViewState::default();
    let r = |t: &T, p| t.level(p);
    // Transfer: Ctrl+I zooms in about the middle, Ctrl+↑ pans by a round step.
    t.key("Ctrl+I");
    assert_eq!(r(&t, PaneKind::Transfer), Range::new(-20.0, 20.0));
    t.key("Ctrl+Up");
    assert_eq!(r(&t, PaneKind::Transfer), Range::new(-15.0, 25.0));
    t.key("Ctrl+O");
    assert!((r(&t, PaneKind::Transfer).span() - 60.0).abs() < 1e-9);
    // ↑ alone is still the stimulus level: no axis change.
    let before = r(&t, PaneKind::Transfer);
    t.key("Up");
    assert_eq!(r(&t, PaneKind::Transfer), before);
    // Each pane keeps its own range.
    assert_eq!(r(&t, PaneKind::Spectrum), d.spectrum.level);
    t.key("Alt+2");
    t.key("Ctrl+Down");
    t.key("Ctrl+Down");
    t.key("Ctrl+Down");
    assert_eq!(r(&t, PaneKind::Spectrum), Range::new(-130.0, -30.0));
    assert_eq!(r(&t, PaneKind::Transfer), before);
    // The mouse: Ctrl+wheel about the pointer, Shift+wheel pans.
    t.st.update(
        Msg::LevelZoom {
            pane: t.st.layout.focus,
            about_db: Some(-80.0),
            factor: 2.0,
        },
        &t.keys,
    );
    assert_eq!(r(&t, PaneKind::Spectrum), Range::new(-105.0, -55.0));
    t.st.update(
        Msg::LevelPan {
            pane: t.st.layout.focus,
            db: 5.0,
        },
        &t.keys,
    );
    assert_eq!(r(&t, PaneKind::Spectrum), Range::new(-100.0, -50.0));
    // Ctrl+Home: the pane's default again.
    t.key("Ctrl+Home");
    assert_eq!(r(&t, PaneKind::Spectrum), d.spectrum.level);
    t.key("Alt+1");
    t.key("Ctrl+Home");
    assert_eq!(r(&t, PaneKind::Transfer), d.tf.magnitude_db);
    // The sweep pane's distortion axis.
    t.go(PaneKind::Distortion);
    t.key("Ctrl+I");
    assert!((r(&t, PaneKind::Distortion).span() - 100.0 / 1.5).abs() < 1e-9);
    // The IR pane without an IR: said, nothing changes.
    t.key("Alt+3");
    let view = t.st.view;
    let levels = t.st.pane_levels(t.st.layout.focus);
    t.key("Ctrl+I");
    assert_eq!(t.st.view, view);
    assert_eq!(t.st.pane_levels(t.st.layout.focus), levels);
    assert!(
        t.last_toast().contains("no impulse response shown"),
        "{}",
        t.last_toast()
    );
}

/// Very low signals: Shift+Home frames what the spectrum pane shows, live and stored, as
/// drawn (offsets included), in the shown frequency range.
#[test]
fn fit_frames_low_signals_in_the_spectrum() {
    let mut t = T::new();
    t.key("Alt+2");
    // Nothing shown yet.
    t.key("Shift+Home");
    assert!(
        t.last_toast().contains("no curve shown"),
        "{}",
        t.last_toast()
    );
    assert_eq!(
        t.level(PaneKind::Spectrum),
        ViewState::default().spectrum.level
    );
    // Bins at 0, 6, 12, 18 and 24 kHz: DC and 24 kHz lie outside 20 Hz – 20 kHz.
    spec_frame(&mut t, 2, vec![0.0, -138.0, -120.0, -95.0, 0.0]);
    t.key("Shift+Home");
    let r = t.level(PaneKind::Spectrum);
    assert!(r.lo <= -138.0 && r.lo >= -150.0, "{r:?}");
    assert!(r.hi >= -95.0 && r.hi <= -85.0, "{r:?}");
    assert!(
        t.last_toast().starts_with("Spectrum / RTA: level "),
        "{}",
        t.last_toast()
    );
    // A stored trace with an offset counts where it is drawn.
    let sp = with_offset(spec_meta(16), 30.0);
    t.conn(with_traces(vec![sp.clone()]));
    t.conn(data(&sp, vec![-100.0; 8]));
    t.key("Shift+Home");
    let r = t.level(PaneKind::Spectrum);
    assert!(r.hi >= -70.0 && r.hi <= -60.0, "{r:?}");
    // The transfer pane frames its own curves.
    t.key("Alt+1");
    t.key("Shift+Home");
    assert!(
        t.last_toast().contains("no curve shown"),
        "{}",
        t.last_toast()
    );
}

/// What Shift+Home frames for a spectrum frame of `level` on bins at 0, 6, 12, 18 and
/// 24 kHz: only 6–18 kHz is in 20 Hz – 20 kHz.
fn fit_of(level: &[f32]) -> Range {
    ac2_scene::view::level::fit(level[1..4].iter().map(|x| f64::from(*x))).expect("finite levels")
}

/// A spectrum pane frames its first frame on its level axis, exactly as Shift+Home does;
/// later frames leave the range alone.
#[test]
fn a_spectrum_pane_fits_its_first_frame_once() {
    let mut t = T::new();
    assert_eq!(
        t.level(PaneKind::Spectrum),
        ViewState::default().spectrum.level
    );
    let first = vec![0.0, -138.0, -120.0, -95.0, 0.0];
    spec_frame(&mut t, 2, first.clone());
    let fit = t.level(PaneKind::Spectrum);
    assert_eq!(fit, fit_of(&first));
    spec_frame(&mut t, 2, vec![0.0, -40.0, -30.0, -20.0, 0.0]);
    assert_eq!(
        t.level(PaneKind::Spectrum),
        fit,
        "a later frame is no first show"
    );

    // The key on that first frame gives the same range.
    let mut u = T::new();
    spec_frame(&mut u, 2, first);
    u.key("Alt+2");
    u.key("Ctrl+Home");
    u.key("Shift+Home");
    assert_eq!(u.level(PaneKind::Spectrum), fit);
}

/// A range the operator set (zoom, pan, Ctrl+Home) stays through later frames, a stop and
/// a start, and a reconnect: only another measurement on the pane frames again.
#[test]
fn an_operator_level_range_is_not_refitted() {
    let mut t = T::new();
    spec_frame(&mut t, 2, vec![0.0, -138.0, -120.0, -95.0, 0.0]);
    t.key("Alt+2");
    t.key("Ctrl+I");
    let zoomed = t.level(PaneKind::Spectrum);
    spec_frame(&mut t, 2, vec![0.0, -40.0, -30.0, -20.0, 0.0]);
    assert_eq!(t.level(PaneKind::Spectrum), zoomed);

    t.conn(with_running(2, false));
    t.conn(with_running(2, true));
    spec_frame(&mut t, 2, vec![0.0, -30.0, -35.0, -40.0, 0.0]);
    assert_eq!(t.level(PaneKind::Spectrum), zoomed, "a new run keeps it");

    t.conn(ConnEvent::Failed {
        target: "local daemon".into(),
        error: "gone".into(),
        retry_in: std::time::Duration::from_secs(1),
    });
    t.connect(daemon_state());
    spec_frame(&mut t, 2, vec![0.0, -10.0, -10.0, -10.0, 0.0]);
    assert_eq!(t.level(PaneKind::Spectrum), zoomed, "a reconnect keeps it");

    // Ctrl+Home is the operator's range too.
    t.key("Ctrl+Home");
    spec_frame(&mut t, 2, vec![0.0, -12.0, -12.0, -12.0, 0.0]);
    assert_eq!(
        t.level(PaneKind::Spectrum),
        ViewState::default().spectrum.level
    );
}

/// A pane framed automatically frames a measurement's new run again on its first new
/// frame (a new run is often a new signal); a stop alone, or the old frame, does not.
#[test]
fn a_started_spectrum_refits_an_automatically_framed_pane() {
    let mut t = T::new();
    spec_frame(&mut t, 2, vec![0.0, -138.0, -120.0, -95.0, 0.0]);
    let fit = t.level(PaneKind::Spectrum);
    t.conn(with_running(2, false));
    spec_frame(&mut t, 2, vec![0.0, -40.0, -40.0, -40.0, 0.0]);
    assert_eq!(t.level(PaneKind::Spectrum), fit);
    t.conn(with_running(2, true));
    assert_eq!(
        t.level(PaneKind::Spectrum),
        fit,
        "the old frame is no start"
    );
    let new = vec![0.0, -30.0, -35.0, -40.0, 0.0];
    spec_frame(&mut t, 2, new.clone());
    assert_eq!(t.level(PaneKind::Spectrum), fit_of(&new));
}

/// Another measurement picked on a pane is a first show: its first frame frames the pane,
/// even after the operator zoomed it on the measurement before.
#[test]
fn a_measurement_picked_on_a_pane_fits_its_first_frame() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.push(meas(4, "Sub 2", spectrum()));
    t.conn(mirror(s));
    spec_frame(&mut t, 2, vec![0.0, -138.0, -120.0, -95.0, 0.0]);
    t.key("Alt+2");
    t.key("Ctrl+I");
    let sp = t.pane(PaneKind::Spectrum);
    t.st.update(Msg::PanePick(sp, PaneMenuRow::Meas(MeasId(4))), &t.keys);
    let zoomed = t.st.level_range(sp);
    // Its first frame is the measurement's first show here, not the other's frame.
    spec_frame(&mut t, 2, vec![0.0, -10.0, -10.0, -10.0, 0.0]);
    assert_eq!(t.st.level_range(sp), zoomed);
    let other = vec![0.0, -60.0, -50.0, -45.0, 0.0];
    spec_frame(&mut t, 4, other.clone());
    assert_eq!(t.st.level_range(sp), Some(fit_of(&other)));
    // And back: the first one frames again.
    t.st.update(Msg::PanePick(sp, PaneMenuRow::Meas(MeasId(2))), &t.keys);
    let back = vec![0.0, -90.0, -80.0, -70.0, 0.0];
    spec_frame(&mut t, 2, back.clone());
    assert_eq!(t.st.level_range(sp), Some(fit_of(&back)));
}

/// Each pane has its own level axis: zooming one of two transfer panes split from one
/// leaves the other's range; the frequency axis stays shared. The split half starts on
/// the range of the pane it came from.
#[test]
fn zooming_one_split_transfer_pane_leaves_the_other() {
    let mut t = T::new();
    t.st.pane_area = (1000.0, 600.0);
    let first = t.pane(PaneKind::Transfer);
    t.st.update(Msg::FocusPane(first), &t.keys);
    let before = t.st.level_range(first).expect("level axis");
    t.key("N");
    let second = t.st.layout.focus;
    assert_ne!(second, first);
    assert_eq!(t.st.level_range(second), Some(before));
    t.key("Ctrl+I");
    assert_eq!(t.st.level_range(second), Some(Range::new(-20.0, 20.0)));
    assert_eq!(
        t.st.level_range(first),
        Some(before),
        "the first pane's axis moved"
    );
    // The wheel on the first pane is that pane's alone.
    t.st.update(
        Msg::LevelZoom {
            pane: first,
            about_db: None,
            factor: 0.5,
        },
        &t.keys,
    );
    assert_eq!(t.st.level_range(first), Some(Range::new(-60.0, 60.0)));
    assert_eq!(t.st.level_range(second), Some(Range::new(-20.0, 20.0)));
    assert_eq!(
        t.st.view_for(first).tf.magnitude_db,
        Range::new(-60.0, 60.0)
    );
    assert_eq!(t.st.view_for(second).freq, t.st.view_for(first).freq);
    // A split of the zoomed pane starts on its range.
    t.key("N");
    assert_eq!(
        t.st.level_range(t.st.layout.focus),
        Some(Range::new(-20.0, 20.0))
    );
}

/// Each pane's level axis comes back with its pane on the next start.
#[test]
fn split_panes_level_axes_are_remembered_each_with_its_pane() {
    let mut t = T::new();
    t.st.pane_area = (1000.0, 600.0);
    let first = t.pane(PaneKind::Transfer);
    t.st.update(Msg::FocusPane(first), &t.keys);
    t.key("N");
    let second = t.st.layout.focus;
    t.key("Ctrl+I");
    t.st.update(Msg::FocusPane(first), &t.keys);
    t.key("Ctrl+Up");
    let (a, b) = (t.st.level_range(first), t.st.level_range(second));
    assert_ne!(a, b);
    let text = t.st.prefs.to_toml();
    let mut u = T::new();
    u.st.set_prefs(crate::prefs::UiPrefs::from_toml(&text).expect("parse"));
    assert_eq!(u.st.level_range(first), a);
    assert_eq!(u.st.level_range(second), b);
}

/// A level axis moved on the app's own first pane (an automatic fit writes it this way) is
/// no layout choice: no panes go to `ui.toml`, and the next start's pane still takes the
/// kind of the first measurement.
#[test]
fn an_automatic_fit_on_the_auto_pane_writes_no_layout() {
    let mut t = T::fresh();
    let mut s = daemon_state();
    s.measurements.clear();
    t.connect(s.clone());
    let pane = t.st.layout.focus;
    t.st.change_level(pane, |_| Range::new(-12.0, 6.0));
    t.conn(mirror(s));
    assert_eq!(t.st.level_range(pane), Some(Range::new(-12.0, 6.0)));
    assert!(t.st.prefs.layout.panes.is_none());
    let text = t.st.prefs.to_toml();
    assert!(!text.contains("[[layout.panes]]"), "{text}");

    let mut u = T::fresh();
    u.st.set_prefs(crate::prefs::UiPrefs::from_toml(&text).expect("parse"));
    let mut sub = daemon_state();
    sub.measurements.retain(|m| m.config.name == "Sub");
    u.connect(sub);
    assert_eq!(u.st.layout.focus_kind(), PaneKind::Spectrum);
}
