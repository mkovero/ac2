//! Reducer tests of the stimulus signal prompt: the forms it takes, what it refuses, the
//! FFT bin a sine is moved onto, and the choice outliving a sweep.

use super::sweep::{next_hint, sweep_once};
use super::*;
use ac2_proto::model::SpecResolution;
use ac2_scene::stimulus::SignalChoice;

/// Shift+J, `text` typed over the prompt's prefill, Enter.
fn choose(t: &mut T, text: &str) -> Vec<Request> {
    t.key("Shift+J");
    let Overlay::Prompt(p) = &mut t.st.overlay else {
        panic!("no prompt");
    };
    assert_eq!(p.kind, PromptKind::StimulusSignal);
    p.text = text.into();
    t.key("Enter")
}

/// What the prompt opens with.
fn prefill(t: &mut T) -> String {
    t.key("Shift+J");
    let Overlay::Prompt(p) = &t.st.overlay else {
        panic!("no prompt");
    };
    let text = p.text.clone();
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
    text
}

fn sine(hz: f64, exact: bool) -> SignalChoice {
    SignalChoice::Sine {
        freq: Hz(hz),
        exact,
    }
}

fn signals(r: &[Request]) -> Vec<(Signal, bool, bool)> {
    r.iter()
        .filter_map(|x| match x {
            Request::StimSet(d) => Some((d.settings.signal, d.armed, d.firing)),
            _ => None,
        })
        .collect()
}

/// Shift+J opens the prompt with the current choice; each form it takes is applied as
/// typed on a pane that is no spectrum, sends nothing while idle and leaves the level and
/// the phase alone.
#[test]
fn the_prompt_takes_a_noise_or_a_sines_frequency() {
    let mut t = T::new();
    assert_eq!(t.focus_kind(), PaneKind::Transfer);
    assert_eq!(prefill(&mut t), "pink");
    for (text, want, toast) in [
        ("white", SignalChoice::White, "white noise"),
        (" Pink ", SignalChoice::Pink, "pink noise"),
        (
            "997",
            sine(997.0, false),
            "sine 997 Hz · as typed, not moved onto an FFT bin",
        ),
        (
            "1k",
            sine(1000.0, false),
            "sine 1000 Hz · as typed, not moved onto an FFT bin",
        ),
        (
            "997 Hz",
            sine(997.0, false),
            "sine 997 Hz · as typed, not moved onto an FFT bin",
        ),
        (
            "1.5 kHz",
            sine(1500.0, false),
            "sine 1500 Hz · as typed, not moved onto an FFT bin",
        ),
        (
            "999,76 exact",
            sine(999.76, true),
            "sine 999.76 Hz · as typed, not moved onto an FFT bin",
        ),
    ] {
        let r = choose(&mut t, text);
        assert!(r.is_empty(), "{text}: {r:?}");
        assert_eq!(t.st.overlay, Overlay::None, "{text}");
        assert_eq!(t.st.rig.stimulus.generator, want, "{text}");
        assert_eq!(t.st.rig.stimulus.signal, want.signal(), "{text}");
        assert_eq!(t.last_toast(), toast, "{text}");
    }
    assert_eq!(t.st.rig.stimulus.level, None);
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
    assert_eq!(prefill(&mut t), "999.76 exact");
}

/// What the generator cannot play is refused in the prompt, in words, and changes nothing.
#[test]
fn the_prompt_refuses_what_the_generator_cannot_play() {
    let mut t = T::new();
    choose(&mut t, "white");
    for (text, error) in [
        ("", "type pink, white, or a sine's frequency"),
        (
            "brown",
            "\"brown\" is no signal: type pink, white, or a sine's frequency",
        ),
        ("0", "\"0\": frequency must be positive"),
        ("-40", "\"-40\": frequency must be positive"),
        ("1 v", "\"1 v\": unknown unit \"v\" (expected hz or khz)"),
        // The test session runs at 48 kHz.
        ("24k", "a sine must be below 24000 Hz, half the sample rate"),
        (
            "30 khz exact",
            "a sine must be below 24000 Hz, half the sample rate",
        ),
    ] {
        let r = choose(&mut t, text);
        assert!(r.is_empty(), "{text}: {r:?}");
        match &t.st.overlay {
            Overlay::Prompt(p) => assert_eq!(p.error.as_deref(), Some(error), "{text}"),
            o => panic!("{text}: {o:?}"),
        }
        t.key("Escape");
        assert_eq!(t.st.rig.stimulus.generator, SignalChoice::White, "{text}");
    }
    // With no audio I/O open the rate is not known: the daemon judges it on arming.
    t.remirror(|s| s.session.open = None);
    choose(&mut t, "30k");
    assert_eq!(t.st.rig.stimulus.generator, sine(30_000.0, false));
}

/// On a pane showing a spectrum measurement a sine goes onto the nearest bin of its FFT
/// (one FFT: its length; multi-resolution: its longest window's), and the toast names the
/// frequency really set; `exact`, an RTA and any other pane leave it as typed.
#[test]
fn a_sine_is_moved_onto_a_bin_of_the_focused_spectrum() {
    let mut t = T::new();
    t.go(PaneKind::Spectrum);
    assert_eq!(
        t.st.pane_meas(t.st.layout.focus).map(|m| m.id),
        Some(MeasId(2))
    );
    // 8192 points at 48 kHz: bins 5.859375 Hz apart, the 171st nearest 1 kHz.
    let bin = 171.0 * 48_000.0 / 8192.0;
    choose(&mut t, "1k");
    assert_eq!(t.st.rig.stimulus.generator, sine(bin, false));
    assert_eq!(t.st.rig.stimulus.signal, Signal::Sine { freq: Hz(bin) });
    assert_eq!(t.last_toast(), "sine 1001.95 Hz · on a bin of Sub");
    // The prefill keeps every digit, so Enter on it anywhere leaves the sine on its bin.
    assert_eq!(prefill(&mut t), "1001.953125");
    choose(&mut t, "1001.953125");
    assert_eq!(t.st.rig.stimulus.generator, sine(bin, false));

    choose(&mut t, "1k exact");
    assert_eq!(t.st.rig.stimulus.generator, sine(1000.0, true));
    assert_eq!(
        t.last_toast(),
        "sine 1000 Hz · as typed, not moved onto an FFT bin"
    );
    assert_eq!(prefill(&mut t), "1000 exact");

    // Multi-resolution: the longest window at 48 kHz has 32 768 points.
    t.remirror(|s| {
        for m in &mut s.measurements {
            if let MeasKind::Spectrum { config } = &mut m.config.kind {
                config.resolution = SpecResolution::MULTI;
            }
        }
    });
    let bin = 683.0 * 48_000.0 / 32_768.0;
    choose(&mut t, "1k");
    assert_eq!(t.st.rig.stimulus.generator, sine(bin, false));
    assert_eq!(t.last_toast(), "sine 1000.49 Hz · on a bin of Sub");

    // The spectrograph of the same measurement: the same bins.
    t.key("G");
    t.key("G");
    choose(&mut t, "2k");
    assert_eq!(
        t.st.rig.stimulus.generator,
        sine(1365.0 * 48_000.0 / 32_768.0, false)
    );

    // An RTA in the pane has no FFT bins to offer.
    t.remirror(|s| {
        for m in &mut s.measurements {
            if matches!(m.config.kind, MeasKind::Spectrum { .. }) {
                m.config.kind = rta();
            }
        }
    });
    choose(&mut t, "1k");
    assert_eq!(t.st.rig.stimulus.generator, sine(1000.0, false));
    assert_eq!(
        t.last_toast(),
        "sine 1000 Hz · as typed, not moved onto an FFT bin"
    );
}

/// A signal chosen while armed or playing goes to the daemon at once, as a level does.
#[test]
fn a_choice_while_armed_or_firing_is_sent() {
    let mut t = T::new();
    t.st.rig.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    let r = choose(&mut t, "1k");
    assert_eq!(
        signals(&r),
        [(Signal::Sine { freq: Hz(1000.0) }, true, false)]
    );
    t.key("Enter");
    t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Firing);
    let r = choose(&mut t, "white");
    assert_eq!(signals(&r), [(Signal::White, true, true)]);
    assert_eq!(set_level(&r[0]), Some((-20.0, true, true)));
}

/// A chosen sine is what Space arms on the live views before and after a sweep ran, and
/// what an armed re-sweep turns back into there; the sweep itself plays its own signal,
/// and a choice made while a sweep is set up waits for the next live arm.
#[test]
fn a_chosen_sine_outlives_a_sweep() {
    let mut t = T::new();
    choose(&mut t, "1k exact");
    let tone = Signal::Sine { freq: Hz(1000.0) };
    sweep_once(&mut t);
    assert_eq!(t.st.rig.stimulus.generator, sine(1000.0, true));
    // On the sweep view the keys still name the sweep, never the sine.
    assert!(t.st.sweep_view());
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Space arms: sweep Sweep 1 · 3 s −50 dBFS")
    );
    t.go(PaneKind::Transfer);
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Space arms: sine 1000 Hz −50 dBFS → out 1")
    );
    let r = t.key("Space");
    match r.as_slice() {
        [Request::StimArm { settings, .. }] => assert_eq!(settings.signal, tone),
        other => panic!("{other:?}"),
    }
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));

    // Armed and silent, over to the sweep view: Space there arms the sweep's own signal.
    t.go(PaneKind::Distortion);
    let r = t.key("Space");
    assert!(
        matches!(signals(&r).as_slice(), [(Signal::Ess { .. }, true, false)]),
        "{r:?}"
    );
    // A choice made now is kept for the live views and leaves the armed sweep alone.
    let r = choose(&mut t, "white");
    assert!(r.is_empty(), "{r:?}");
    assert!(matches!(t.st.rig.stimulus.signal, Signal::Ess { .. }));
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Enter fires: sweep Sweep 1 · 3 s −50 dBFS")
    );
    t.go(PaneKind::Transfer);
    let r = t.key("Space");
    assert_eq!(signals(&r), [(Signal::White, true, false)]);
    assert_eq!(t.st.rig.stimulus.signal, Signal::White);
}
