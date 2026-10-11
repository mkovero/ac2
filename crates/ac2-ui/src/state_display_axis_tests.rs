//! Reducer tests of the level axis: zoom, pan, fit and reset per pane, and the fit a
//! started spectrum gets on its first frame.

use super::*;

#[test]
fn level_axis_zooms_pans_fits_and_resets_per_pane() {
    let mut t = T::new();
    let d = ViewState::default();
    let r = |t: &T, p| {
        level_range(&t.st.view, p, crate::scenes::spectrum_scale(&t.st)).expect("level axis")
    };
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
            pane: PaneKind::Spectrum,
            about_db: Some(-80.0),
            factor: 2.0,
        },
        &t.keys,
    );
    assert_eq!(r(&t, PaneKind::Spectrum), Range::new(-105.0, -55.0));
    t.st.update(
        Msg::LevelPan {
            pane: PaneKind::Spectrum,
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
    t.key("Ctrl+I");
    assert_eq!(t.st.view, view);
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
        t.st.view.spectrum.level,
        ViewState::default().spectrum.level
    );
    // Bins at 0, 6, 12, 18 and 24 kHz: DC and 24 kHz lie outside 20 Hz – 20 kHz.
    spec_frame(&mut t, 2, vec![0.0, -138.0, -120.0, -95.0, 0.0]);
    t.key("Shift+Home");
    let r = t.st.view.spectrum.level;
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
    let r = t.st.view.spectrum.level;
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

/// Each start of a spectrum fits the level axis on its first frame, as Shift+Home; frames
/// after it leave the operator's range alone.
#[test]
fn a_started_spectrum_fits_its_level_axis_once() {
    let mut t = T::new();
    let default = ViewState::default().spectrum.level;
    // Bins at 0, 6, 12, 18 and 24 kHz: only 6–18 kHz is in 20 Hz – 20 kHz.
    spec_frame(&mut t, 2, vec![0.0, -138.0, -120.0, -95.0, 0.0]);
    let r = t.st.view.spectrum.level;
    assert_ne!(r, default);
    assert!(r.lo <= -138.0 && r.lo >= -150.0, "{r:?}");
    assert!(r.hi >= -95.0 && r.hi <= -85.0, "{r:?}");

    // The operator's own range stays while the run goes on.
    t.key("Alt+2");
    t.key("Ctrl+Home");
    spec_frame(&mut t, 2, vec![0.0, -40.0, -40.0, -40.0, 0.0]);
    assert_eq!(t.st.view.spectrum.level, default);

    // Stopped: nothing fits; started again: the first new frame does.
    t.conn(with_running(2, false));
    spec_frame(&mut t, 2, vec![0.0, -40.0, -40.0, -40.0, 0.0]);
    assert_eq!(t.st.view.spectrum.level, default);
    t.conn(with_running(2, true));
    assert_eq!(
        t.st.view.spectrum.level, default,
        "the old frame is no start"
    );
    spec_frame(&mut t, 2, vec![0.0, -30.0, -35.0, -40.0, 0.0]);
    let r = t.st.view.spectrum.level;
    assert!(r.lo <= -40.0 && r.hi >= -30.0 && r.hi <= -20.0, "{r:?}");
}
