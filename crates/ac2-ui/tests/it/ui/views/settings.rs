//! The rig monitor on a live rig.

use super::*;

/// The rig monitor's look: an SPL meter kept by the rig leads the tree under the Rig header
/// with its cue chip; the Rig monitor page shows the targets, monitor at start, the test
/// message, how sending fares and the rig's meters.
#[test]
fn rig_monitor_views() {
    if !have_gpu("rig_monitor_views") {
        return;
    }
    let _rig = Rig::scene("monitor");
    let mut h = harness(options());
    step_until(&mut h, "live frames", |a| live_with(a, 5));
    step_until(&mut h, "the Rig header", |a| {
        a.state.tree_rows().first().map(|r| r.name.as_str()) == Some("Rig")
    });
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "tree_rig_group");
    h.state_mut().dispatch(ac2_ui::state::Msg::OpenRig);
    step_until(&mut h, "the Rig monitor page", |a| {
        a.state
            .overlay
            .settings()
            .is_some_and(|s| s.page == ac2_ui::settings::Page::Rig)
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "settings_rig");
}

/// The stimulus signal rows of the Audio I/O page as the palette's "Stimulus signal…"
/// opens them, a sine chosen: the signal, its frequency with the one played on a bin of
/// the rig's spectrum, the bin row, each with its hint, between the channels and the max
/// level.
#[test]
fn audio_io_stimulus_signal_rows() {
    use ac2_scene::stimulus::SignalRow;
    if !have_gpu("audio_io_stimulus_signal_rows") {
        return;
    }
    let _rig = Rig::start();
    let mut h = harness(options());
    step_until(&mut h, "live frames", live);
    h.state_mut().dispatch(ac2_ui::state::Msg::Command(
        ac2_ui::keys::CommandId::StimulusSignal,
    ));
    step_until(&mut h, "the signal row", |a| {
        a.state
            .overlay
            .settings()
            .is_some_and(|s| s.signal.focus == Some(SignalRow::Signal))
    });
    h.key_press(Key::ArrowLeft);
    step_until(&mut h, "a sine", |a| {
        a.state.rig.stimulus.generator == Some(ac2_scene::stimulus::SignalKind::Sine)
    });
    h.key_press(Key::ArrowDown);
    step_until(&mut h, "the frequency row", |a| {
        a.state
            .overlay
            .settings()
            .is_some_and(|s| s.signal.focus == Some(SignalRow::Frequency))
    });
    let st = &h.state().state;
    assert!(st.sine_bins().is_some(), "the rig's spectrum is running");
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "settings_stimulus_signal");
}

/// The dialog the first Space opens, a sine picked: the three rows of the settings
/// section under the question, the frequency held, what the bin row makes of it on the
/// rig's spectrum, and the footer with the keys the dialog has.
#[test]
fn stimulus_signal_dialog() {
    use ac2_ui::forms::FormKind;
    use ac2_ui::state::Overlay;
    if !have_gpu("stimulus_signal_dialog") {
        return;
    }
    let _rig = Rig::start();
    let mut h = harness(options());
    step_until(&mut h, "live frames", live);
    h.key_press(Key::Space);
    step_until(
        &mut h,
        "the signal dialog",
        |a| matches!(&a.state.overlay, Overlay::Form(f) if f.kind == FormKind::StimulusSignal),
    );
    h.key_press(Key::ArrowLeft);
    step_until(
        &mut h,
        "the sine's rows",
        |a| matches!(&a.state.overlay, Overlay::Form(f) if f.fields.len() == 3),
    );
    {
        let st = &h.state().state;
        assert!(st.sine_bins().is_some(), "the rig's spectrum is running");
        let Overlay::Form(f) = &st.overlay else {
            panic!("no dialog");
        };
        let rows: Vec<String> = f
            .fields
            .iter()
            .map(|x| format!("{}: {} · {}", x.label, x.display(), x.hint))
            .collect();
        assert_eq!(rows[0], "Signal: sine · ");
        assert_eq!(rows[1], "Sine frequency: 1000 Hz · 997, 1k, 1.5 kHz");
        assert!(
            rows[2].starts_with("On an FFT bin: on · bins of the ")
                && rows[2].contains(" · plays 1000 Hz → "),
            "{rows:?}"
        );
        assert_eq!(
            f.keys_hint(),
            "Enter chooses it; nothing is armed · ↑↓ field · ←→ choose · Esc closes"
        );
        // Nothing is chosen by looking.
        assert_eq!(st.rig.stimulus.generator, None);
    }
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "stimulus_signal_dialog");
}
