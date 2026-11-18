//! Reducer tests of the generator's signal: asked in a dialog at the first arm, set on
//! the Settings › Audio I/O rows, and a sine resolved onto an FFT bin of the running
//! spectrum measurements whenever it is sent.

use super::sweep::{next_hint, sweep_once};
use super::*;
use crate::forms::FormKind;
use ac2_proto::model::SpecResolution;
use ac2_scene::stimulus::SignalRow;

/// The test daemon's one spectrum (Sub) has 8192 points at 48 kHz: bins 5.859375 Hz
/// apart, the 171st nearest 1 kHz.
const BIN_1K: f64 = 171.0 * 48_000.0 / 8192.0;

fn prompt(t: &T) -> (PromptKind, &str, Option<&str>) {
    match &t.st.overlay {
        Overlay::Prompt(p) => (p.kind, p.text.as_str(), p.error.as_deref()),
        o => panic!("no prompt: {o:?}"),
    }
}

/// The signal dialog's rows as shown (title, value, what stands beside it) and its error.
fn dialog(t: &T) -> (Vec<(&str, String, &str)>, Option<&str>) {
    match &t.st.overlay {
        Overlay::Form(f) if f.kind == FormKind::StimulusSignal => (
            f.fields
                .iter()
                .map(|x| (x.label.as_str(), x.display(), x.hint.as_str()))
                .collect(),
            f.error.as_deref(),
        ),
        o => panic!("no signal dialog: {o:?}"),
    }
}

fn dialog_open(t: &T) -> bool {
    matches!(&t.st.overlay, Overlay::Form(f) if f.kind == FormKind::StimulusSignal)
}

/// The first Space of a fresh app and the keys that take the dialog from pink noise to
/// `kind`; the dialog stays open.
fn ask(t: &mut T, kind: SignalKind) -> Vec<Request> {
    let mut r = t.type_key("Space", " ");
    assert_eq!(dialog(t).0, [("Signal", "pink noise".to_owned(), "")]);
    match kind {
        SignalKind::Pink => {}
        SignalKind::White => r.extend(t.key("ArrowRight")),
        SignalKind::Sine => r.extend(t.key("ArrowLeft")),
    }
    r
}

/// [`ask`], then Enter.
fn first_arm(t: &mut T, kind: SignalKind) -> Vec<Request> {
    let mut r = ask(t, kind);
    r.extend(t.key("Enter"));
    r
}

/// [`ask`] for a sine with `freq` typed over the frequency held; the dialog stays open on
/// the frequency row.
fn ask_sine(t: &mut T, freq: &str) -> Vec<Request> {
    let mut r = ask(t, SignalKind::Sine);
    r.extend(t.key("ArrowDown"));
    r.extend(t.key("Backspace"));
    t.text(freq);
    r
}

fn signals(r: &[Request]) -> Vec<(Signal, bool, bool)> {
    r.iter()
        .filter_map(|x| match x {
            Request::StimSet(d) => Some((d.settings.signal, d.armed, d.firing)),
            _ => None,
        })
        .collect()
}

fn sine(hz: f64) -> Signal {
    Signal::Sine { freq: Hz(hz) }
}

/// Every spectrum measurement of the mirrored state given `resolution`.
fn set_resolution(t: &mut T, resolution: SpecResolution) {
    t.remirror(|s| {
        for m in &mut s.measurements {
            if let MeasKind::Spectrum { config } = &mut m.config.kind {
                config.resolution = resolution;
            }
        }
    });
}

/// A second spectrum measurement of `fft_len` points beside Sub.
fn add_spectrum(t: &mut T, id: u32, fft_len: u32) {
    t.remirror(|s| {
        let mut m = s
            .measurements
            .iter()
            .find(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }))
            .cloned()
            .expect("a spectrum");
        m.id = MeasId(id);
        m.config.name = format!("Spectrum {id}");
        if let MeasKind::Spectrum { config } = &mut m.config.kind {
            config.resolution = SpecResolution::Fixed { fft_len };
        }
        s.measurements.push(m);
    });
}

/// Settings on the Audio I/O page, at the signal row, as the palette's command opens it.
fn open_rows(t: &mut T) {
    t.st.apply(Msg::Command(CommandId::StimulusSignal), &t.keys);
    let s = t.st.overlay.settings().expect("Settings open");
    assert_eq!(s.page, Page::AudioIo);
    assert_eq!(s.signal.focus, Some(SignalRow::Signal));
    assert!(!s.on_ceiling);
}

fn rows(t: &T) -> &crate::settings::SignalRows {
    &t.st.overlay.settings().expect("Settings open").signal
}

/// The first Space asks what to play in a dialog that opens on pink noise, then the level;
/// the level typed, nothing is armed yet, as after the level prompt alone: the next Space
/// arms. Neither is asked again.
#[test]
fn the_first_arm_asks_the_signal_then_the_level() {
    let mut t = T::new();
    assert_eq!(t.st.rig.stimulus.generator, None);
    assert_eq!(next_hint(&t).as_deref(), Some("L types a level"));
    let r = t.type_key("Space", " ");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(
        dialog(&t),
        (vec![("Signal", "pink noise".to_owned(), "")], None)
    );
    let Overlay::Form(f) = &t.st.overlay else {
        panic!()
    };
    assert_eq!(f.kind.title(), "What should the generator play?");
    assert_eq!(
        f.keys_hint(),
        "Enter chooses it; nothing is armed · ←→ choose · Esc closes"
    );
    assert_eq!(
        t.last_toast(),
        "say what to play first; Enter takes pink noise"
    );
    let r = t.key("Enter");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.rig.stimulus.generator, Some(SignalKind::Pink));
    assert_eq!(prompt(&t), (PromptKind::StimulusLevel, "", None));
    assert_eq!(
        t.last_toast(),
        "pink noise · type a level; nothing is armed without one"
    );
    t.text("-20");
    let r = t.key("Enter");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
    assert_eq!(t.last_toast(), "level −20.0 dBFS · Space arms");
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Space arms: pink −20 dBFS → out 1")
    );
    let r = t.key("Space");
    match r.as_slice() {
        [Request::StimArm { settings, .. }] => assert_eq!(settings.signal, Signal::Pink),
        other => panic!("{other:?}"),
    }
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.key("Escape");
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
    let r = t.key("Space");
    assert!(matches!(r.as_slice(), [Request::StimArm { .. }]), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
}

/// Esc on the signal dialog chooses nothing, whatever it showed: the next Space asks
/// again, from pink noise. Esc on the level prompt after it keeps the signal chosen and
/// no level, so the next Space asks only for the level. Neither arms or sends anything.
#[test]
fn escape_on_either_question_arms_nothing() {
    let mut t = T::new();
    let r = ask_sine(&mut t, "5k");
    assert!(r.is_empty(), "{r:?}");
    let r = t.key("Escape");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.rig.stimulus.generator, None);
    assert_eq!(t.st.rig.stimulus.sine, ac2_scene::stimulus::DEFAULT_SINE);
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
    let r = first_arm(&mut t, SignalKind::White);
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(prompt(&t).0, PromptKind::StimulusLevel);
    let r = t.key("Escape");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.rig.stimulus.generator, Some(SignalKind::White));
    assert_eq!(t.st.rig.stimulus.level, None);
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
    let r = t.key("Space");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(prompt(&t), (PromptKind::StimulusLevel, "", None));
}

/// With the level typed before (L), the first Space asks only for the signal and the
/// answer arms nothing: the next Space does.
#[test]
fn a_level_typed_before_leaves_only_the_signal_to_ask() {
    let mut t = T::new();
    t.st.rig.stimulus.level = Some(Dbfs(-20.0));
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Space asks what to play: pink, white or a sine")
    );
    let r = first_arm(&mut t, SignalKind::White);
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.last_toast(), "white noise · Space arms");
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Space arms: white −20 dBFS → out 1")
    );
    let r = t.key("Space");
    match r.as_slice() {
        [Request::StimArm { settings, .. }] => assert_eq!(settings.signal, Signal::White),
        other => panic!("{other:?}"),
    }
}

/// ←/→ on the signal row go through pink noise, white noise and sine; only the sine has
/// the frequency and bin rows, holding 1 kHz on a bin of the running spectrum and saying
/// what that plays. ↑/↓ move between the rows there are.
#[test]
fn the_sine_rows_are_there_only_with_a_sine() {
    let mut t = T::new();
    t.type_key("Space", " ");
    let pink = vec![("Signal", "pink noise".to_owned(), "")];
    let sine_rows = vec![
        ("Signal", "sine".to_owned(), ""),
        ("Sine frequency", "1000 Hz".to_owned(), "997, 1k, 1.5 kHz"),
        (
            "On an FFT bin",
            "on · bins of the 8192-point FFT".to_owned(),
            "plays 1000 Hz → 1001.95 Hz",
        ),
    ];
    assert_eq!(dialog(&t).0, pink);
    // One row: ↑/↓ have nowhere to go.
    t.key("ArrowDown");
    t.key("ArrowRight");
    assert_eq!(dialog(&t).0, [("Signal", "white noise".to_owned(), "")]);
    t.key("ArrowRight");
    assert_eq!(dialog(&t).0, sine_rows);
    let Overlay::Form(f) = &t.st.overlay else {
        panic!()
    };
    assert_eq!(
        f.keys_hint(),
        "Enter chooses it; nothing is armed · ↑↓ field · ←→ choose · Esc closes"
    );
    assert_eq!(f.focus, 0);
    t.key("ArrowRight");
    assert_eq!(dialog(&t).0, pink);
    t.key("ArrowLeft");
    assert_eq!(dialog(&t).0, sine_rows);
    for (key, at) in [
        ("ArrowDown", 1),
        ("ArrowDown", 2),
        ("ArrowDown", 0),
        ("ArrowUp", 2),
    ] {
        t.key(key);
        let Overlay::Form(f) = &t.st.overlay else {
            panic!()
        };
        assert_eq!(f.focus, at, "{key}");
    }
    // Nothing is chosen, sent or armed by looking.
    assert_eq!(t.st.rig.stimulus.generator, None);
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
}

/// Enter takes what the dialog shows: a noise, or the sine at the frequency typed, on a
/// bin of the running spectrum unless the bin row is off. The toast names it as played,
/// and the level is asked next.
#[test]
fn enter_takes_the_signal_shown() {
    let level = " · type a level; nothing is armed without one";
    // (the frequency typed, the bin row turned off) for a sine.
    for (kind, typed, sine_set, played, toast) in [
        (
            SignalKind::Pink,
            None,
            (1000.0, true),
            Signal::Pink,
            "pink noise",
        ),
        (
            SignalKind::White,
            None,
            (1000.0, true),
            Signal::White,
            "white noise",
        ),
        (
            SignalKind::Sine,
            None,
            (1000.0, true),
            sine(BIN_1K),
            "sine 1001.95 Hz · on an FFT bin of the spectrum",
        ),
        (
            SignalKind::Sine,
            Some(("1.5 kHz", false)),
            (1500.0, true),
            sine(256.0 * 48_000.0 / 8192.0),
            "sine 1500 Hz · on an FFT bin of the spectrum",
        ),
        (
            SignalKind::Sine,
            Some(("999,76", true)),
            (999.76, false),
            sine(999.76),
            "sine 999.76 Hz · as typed",
        ),
    ] {
        let mut t = T::new();
        let what = format!("{kind:?} {typed:?}");
        let mut r = match typed {
            None => ask(&mut t, kind),
            Some((freq, off)) => {
                let mut r = ask_sine(&mut t, freq);
                if off {
                    r.extend(t.key("ArrowDown"));
                    r.extend(t.key("ArrowRight"));
                    assert_eq!(
                        dialog(&t).0[2],
                        (
                            "On an FFT bin",
                            "off · played as typed".to_owned(),
                            "plays 999.76 Hz"
                        )
                    );
                }
                r
            }
        };
        r.extend(t.key("Enter"));
        assert!(r.is_empty(), "{what}: {r:?}");
        let stim = &t.st.rig.stimulus;
        assert_eq!(stim.generator, Some(kind), "{what}");
        assert_eq!((stim.sine.freq.0, stim.sine.on_bin), sine_set, "{what}");
        assert_eq!(t.st.generator_signal(), Some(played), "{what}");
        assert_eq!(t.last_toast(), format!("{toast}{level}"), "{what}");
        assert_eq!(prompt(&t).0, PromptKind::StimulusLevel, "{what}");
    }
    // The rows hold what Settings holds, not the defaults; a noise chosen leaves it be.
    let mut t = T::new();
    t.st.rig.stimulus.sine = SineSetting {
        freq: Hz(440.0),
        on_bin: false,
    };
    ask(&mut t, SignalKind::Sine);
    assert_eq!(
        dialog(&t).0[1..],
        [
            ("Sine frequency", "440 Hz".to_owned(), "997, 1k, 1.5 kHz"),
            (
                "On an FFT bin",
                "off · played as typed".to_owned(),
                "plays 440 Hz"
            ),
        ]
    );
    t.key("Enter");
    assert_eq!(t.st.generator_signal(), Some(sine(440.0)));
    let mut t = T::new();
    ask_sine(&mut t, "5k");
    t.key("ArrowUp");
    t.key("ArrowRight");
    t.key("Enter");
    assert_eq!(t.st.rig.stimulus.generator, Some(SignalKind::Pink));
    assert_eq!(t.st.rig.stimulus.sine, ac2_scene::stimulus::DEFAULT_SINE);
}

/// A frequency the generator cannot play is refused in the dialog on Enter, in the words
/// of the settings row, and chooses nothing; typing on clears the refusal.
#[test]
fn the_dialog_refuses_a_frequency_the_generator_cannot_play() {
    let mut t = T::new();
    for (text, error) in [
        ("", "type the sine's frequency (e.g. 1k), then Enter"),
        ("brown", "\"brown\": expected a number followed by a unit"),
        ("0", "\"0\": frequency must be positive"),
        ("-40", "\"-40\": frequency must be positive"),
        ("1 v", "\"1 v\": unknown unit \"v\" (expected hz or khz)"),
        // The test session runs at 48 kHz.
        ("24k", "a sine must be below 24000 Hz, half the sample rate"),
    ] {
        let mut r = ask_sine(&mut t, text);
        r.extend(t.key("Enter"));
        assert!(r.is_empty(), "{text}: {r:?}");
        let (rows, e) = dialog(&t);
        assert_eq!(rows[1].1, text, "{text}");
        assert_eq!(e, Some(error), "{text}");
        t.text("1");
        assert_eq!(dialog(&t).1, None, "{text}");
        t.key("Escape");
        assert_eq!(t.st.rig.stimulus.generator, None, "{text}");
    }
    // With no audio I/O open the rate is not known: the daemon judges it on arming.
    t.remirror(|s| s.session.open = None);
    ask_sine(&mut t, "30k");
    t.key("Enter");
    assert_eq!(t.st.generator_signal(), Some(sine(30_000.0)));
}

/// A sweep is armed and run from the sweep view without the signal ever being asked; the
/// first arm of the generator after it, armed re-sweep or not, still asks, and leaves an
/// armed sweep alone until answered.
#[test]
fn a_sweep_arm_never_asks_the_signal() {
    let mut t = T::new();
    sweep_once(&mut t);
    assert_eq!(t.st.rig.stimulus.generator, None);
    assert!(t.st.sweep_view());
    let r = t.key("Space");
    assert!(
        matches!(r.as_slice(), [Request::StimArm { settings, .. }]
            if matches!(settings.signal, Signal::Ess { .. })),
        "{r:?}"
    );
    assert_eq!(t.st.overlay, Overlay::None);
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    // Armed with the sweep, Space on a live view would turn it into the generator's
    // signal: not before that is chosen.
    t.go(PaneKind::Transfer);
    let r = t.key("Space");
    assert!(r.is_empty(), "{r:?}");
    assert!(dialog_open(&t));
    assert!(matches!(t.st.rig.stimulus.signal, Signal::Ess { .. }));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Armed);
    // Esc closes the dialog and nothing else: the sweep stays armed.
    let r = t.key("Escape");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(matches!(t.st.rig.stimulus.signal, Signal::Ess { .. }));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Armed);
    t.key("Space");
    assert!(dialog_open(&t));
    let r = t.key("Enter");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(matches!(t.st.rig.stimulus.signal, Signal::Ess { .. }));
    let r = t.key("Space");
    assert_eq!(signals(&r), [(Signal::Pink, true, false)]);
}

/// A chosen sine is what Space arms on the live views before and after a sweep ran, and
/// what an armed re-sweep turns back into there; the sweep itself plays its own signal,
/// and a choice made while a sweep is set up waits for the next live arm.
#[test]
fn a_chosen_sine_outlives_a_sweep() {
    let mut t = T::new();
    t.st.rig.stimulus.generator = Some(SignalKind::Sine);
    t.st.rig.stimulus.sine.on_bin = false;
    sweep_once(&mut t);
    assert_eq!(t.st.rig.stimulus.generator, Some(SignalKind::Sine));
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
        [Request::StimArm { settings, .. }] => assert_eq!(settings.signal, sine(1000.0)),
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
    open_rows(&mut t);
    let r = t.key("ArrowRight");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.rig.stimulus.generator, Some(SignalKind::Pink));
    assert!(matches!(t.st.rig.stimulus.signal, Signal::Ess { .. }));
    t.key("Escape");
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Enter fires: sweep Sweep 1 · 3 s −50 dBFS")
    );
    t.go(PaneKind::Transfer);
    let r = t.key("Space");
    assert_eq!(signals(&r), [(Signal::Pink, true, false)]);
    assert_eq!(t.st.rig.stimulus.signal, Signal::Pink);
}

/// The command has no key in any scope: the palette's "Stimulus signal…" opens Settings
/// on the Audio I/O page at the signal row.
#[test]
fn the_palette_command_opens_settings_at_the_signal_row() {
    let mut t = T::new();
    assert!(
        crate::keys::defaults()
            .iter()
            .all(|b| b.command != CommandId::StimulusSignal)
    );
    assert_eq!(CommandId::StimulusSignal.title(), "Stimulus signal…");
    open_rows(&mut t);
    assert_eq!(t.st.rig.stimulus.generator, None);
    // On the open view, from another row, it comes back to the row.
    t.key("ArrowDown");
    assert!(t.st.overlay.settings().is_some_and(|s| s.on_ceiling));
    open_rows(&mut t);
}

/// ←/→ on the signal row choose among pink, white and sine, from nothing chosen too; the
/// sine is complete at once at 1 kHz, on a bin of the running spectrum, and is what the
/// next Space arms without asking.
#[test]
fn the_signal_row_chooses_and_a_sine_starts_at_1_khz() {
    let mut t = T::new();
    t.st.rig.stimulus.level = Some(Dbfs(-20.0));
    open_rows(&mut t);
    assert_eq!(SignalRow::shown(false), [SignalRow::Signal]);
    for (key, kind) in [
        ("ArrowRight", SignalKind::Pink),
        ("ArrowRight", SignalKind::White),
        ("Space", SignalKind::Sine),
        ("Enter", SignalKind::Pink),
        ("ArrowLeft", SignalKind::Sine),
    ] {
        let r = t.key(key);
        assert!(r.is_empty(), "{key}: {r:?}");
        assert_eq!(t.st.rig.stimulus.generator, Some(kind), "{key}");
    }
    assert_eq!(t.st.rig.stimulus.sine, ac2_scene::stimulus::DEFAULT_SINE);
    assert_eq!(
        t.last_toast(),
        "sine 1001.95 Hz · on an FFT bin of the spectrum · Space arms"
    );
    assert_eq!(
        ac2_scene::stimulus::sine_freq_value(t.st.rig.stimulus.sine, t.st.sine_bins()),
        "1000 Hz → 1001.95 Hz"
    );
    t.key("Escape");
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Space arms: sine 1001.95 Hz −20 dBFS → out 1")
    );
    let r = t.key("Space");
    match r.as_slice() {
        [Request::StimArm { settings, .. }] => assert_eq!(settings.signal, sine(BIN_1K)),
        other => panic!("{other:?}"),
    }
    // From nothing chosen, ← starts at the other end.
    let mut t = T::new();
    open_rows(&mut t);
    t.key("ArrowLeft");
    assert_eq!(t.st.rig.stimulus.generator, Some(SignalKind::Sine));
    assert_eq!(
        t.last_toast(),
        "sine 1001.95 Hz · on an FFT bin of the spectrum · type a level; nothing is armed \
         without one"
    );
}

/// The frequency row is typed in place and applied on Enter, read and refused as the
/// dialog does; the bin row toggles with ←/→; both are sent at once while armed and while
/// playing.
#[test]
fn the_sine_rows_are_typed_toggled_and_sent_while_live() {
    let mut t = T::new();
    t.pink_at(-20.0);
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    open_rows(&mut t);
    let r = t.key("ArrowLeft");
    assert_eq!(signals(&r), [(sine(BIN_1K), true, false)]);
    assert_eq!(
        t.last_toast(),
        "sine 1001.95 Hz · on an FFT bin of the spectrum"
    );
    t.key("ArrowDown");
    assert_eq!(rows(&t).focus, Some(SignalRow::Frequency));
    assert!(t.st.overlay.settings().is_some_and(Settings::typing));
    for (text, error) in [
        ("", "type the sine's frequency (e.g. 1k), then Enter"),
        ("0", "\"0\": frequency must be positive"),
        ("1 v", "\"1 v\": unknown unit \"v\" (expected hz or khz)"),
        ("x", "\"x\": expected a number followed by a unit"),
        ("24k", "a sine must be below 24000 Hz, half the sample rate"),
    ] {
        t.text(text);
        let r = t.key("Enter");
        assert!(r.is_empty(), "{text}: {r:?}");
        assert_eq!(rows(&t).error.as_deref(), Some(error), "{text}");
        assert_eq!(rows(&t).text, text, "{text}");
        for _ in text.chars() {
            t.key("Backspace");
        }
        assert_eq!(rows(&t).text, "", "{text}");
        assert_eq!(t.st.rig.stimulus.sine.freq, Hz(1000.0), "{text}");
    }
    // Typing clears the refusal; letters are text here, not keys of the page.
    t.type_key("2", "2");
    assert_eq!(rows(&t).error, None);
    t.type_key("K", "k");
    assert_eq!(rows(&t).text, "2k");
    let r = t.key("Enter");
    let bin_2k = 341.0 * 48_000.0 / 8192.0;
    assert_eq!(signals(&r), [(sine(bin_2k), true, false)]);
    assert_eq!(rows(&t).text, "");
    assert_eq!(t.st.rig.stimulus.sine.freq, Hz(2000.0));
    assert_eq!(
        ac2_scene::stimulus::sine_freq_value(t.st.rig.stimulus.sine, t.st.sine_bins()),
        "2000 Hz → 1998.05 Hz"
    );

    // Playing: the bin row off sends the typed frequency, on again the bin.
    t.key("Escape");
    t.key("Enter");
    t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Firing);
    open_rows(&mut t);
    t.key("ArrowDown");
    t.key("ArrowDown");
    assert_eq!(rows(&t).focus, Some(SignalRow::OnBin));
    let r = t.key("ArrowRight");
    assert_eq!(signals(&r), [(sine(2000.0), true, true)]);
    assert!(!t.st.rig.stimulus.sine.on_bin);
    assert_eq!(t.last_toast(), "sine 2000 Hz · as typed");
    let r = t.key("ArrowLeft");
    assert_eq!(signals(&r), [(sine(bin_2k), true, true)]);
    assert!(t.st.rig.stimulus.sine.on_bin);
    assert_eq!(set_level(&r[0]), Some((-20.0, true, true)));
    // The mouse: ‹/› act on a choice row, a click on the frequency row focuses it.
    let r = t.st.apply(
        Msg::Settings(SettingsMsg::Signal(SignalRow::Frequency, 0)),
        &t.keys,
    );
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(rows(&t).focus, Some(SignalRow::Frequency));
    let r = t.st.apply(
        Msg::Settings(SettingsMsg::Signal(SignalRow::Signal, 1)),
        &t.keys,
    );
    assert_eq!(signals(&r), [(Signal::Pink, true, true)]);
    assert_eq!(rows(&t).focus, Some(SignalRow::Signal));
}

/// The bin a sine goes onto is the shortest FFT's among the running spectrum
/// measurements, whatever pane is focused: none running, the frequency typed; one FFT,
/// its length; multi-resolution, its longest window; two, the shorter.
#[test]
fn a_sine_goes_onto_a_bin_of_the_shortest_running_spectrum_fft() {
    let mut t = T::new();
    t.st.rig.stimulus.generator = Some(SignalKind::Sine);
    assert_eq!(t.focus_kind(), PaneKind::Transfer);
    assert_eq!(t.st.sine_bins(), Some((8192, 48_000.0)));
    assert_eq!(t.st.generator_signal(), Some(sine(BIN_1K)));
    // Multi-resolution: the longest window at 48 kHz has 32 768 points.
    set_resolution(&mut t, SpecResolution::MULTI);
    assert_eq!(t.st.sine_bins(), Some((32_768, 48_000.0)));
    assert_eq!(
        t.st.generator_signal(),
        Some(sine(683.0 * 48_000.0 / 32_768.0))
    );
    // A second, shorter one decides; a longer one changes nothing.
    add_spectrum(&mut t, 7, 1024);
    assert_eq!(t.st.sine_bins(), Some((1024, 48_000.0)));
    assert_eq!(
        t.st.generator_signal(),
        Some(sine(21.0 * 48_000.0 / 1024.0))
    );
    add_spectrum(&mut t, 8, 65_536);
    assert_eq!(t.st.sine_bins(), Some((1024, 48_000.0)));
    // A stopped one does not count.
    t.remirror(|s| {
        for m in &mut s.measurements {
            m.running = m.id != MeasId(7);
        }
    });
    assert_eq!(t.st.sine_bins(), Some((32_768, 48_000.0)));
    // The bin row off: as typed, with every spectrum running.
    t.st.rig.stimulus.sine.on_bin = false;
    assert_eq!(t.st.generator_signal(), Some(sine(1000.0)));
    t.st.rig.stimulus.sine.on_bin = true;
    // An RTA has no FFT bins to offer; nor has a rig without audio I/O a rate.
    t.remirror(|s| {
        s.measurements
            .retain(|m| !matches!(m.config.kind, MeasKind::Spectrum { .. }));
        s.measurements.push(meas(9, "RTA", rta()));
    });
    assert_eq!(t.st.sine_bins(), None);
    assert_eq!(t.st.generator_signal(), Some(sine(1000.0)));
    let mut t = T::new();
    t.st.rig.stimulus.generator = Some(SignalKind::Sine);
    t.remirror(|s| s.session.open = None);
    assert_eq!(t.st.sine_bins(), None);
    assert_eq!(t.st.generator_signal(), Some(sine(1000.0)));
}

/// While the sine plays (or is armed), a spectrum added, removed or given another
/// resolution re-sends it only when the frequency it resolves to changed.
#[test]
fn a_playing_sine_follows_the_spectrum_measurements() {
    let mut t = T::new();
    t.pink_at(-20.0);
    t.st.rig.stimulus.generator = Some(SignalKind::Sine);
    let r = t.key("Space");
    match r.as_slice() {
        [Request::StimArm { settings, .. }] => assert_eq!(settings.signal, sine(BIN_1K)),
        other => panic!("{other:?}"),
    }
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    let sent = |t: &mut T, f: &dyn Fn(&mut State)| {
        let mut s = t.st.daemon().cloned().expect("a daemon state");
        f(&mut s);
        signals(&t.conn(mirror(s)))
    };
    // A longer FFT beside it: the bin of the 8192-point one is its bin too.
    fn by_id(s: &mut State, id: u32) -> &mut Measurement {
        s.measurements
            .iter_mut()
            .find(|m| m.id == MeasId(id))
            .expect("the measurement")
    }
    let longer = |s: &mut State| {
        let mut m = by_id(s, 2).clone();
        assert!(matches!(m.config.kind, MeasKind::Spectrum { .. }));
        m.id = MeasId(7);
        if let MeasKind::Spectrum { config } = &mut m.config.kind {
            config.resolution = SpecResolution::Fixed { fft_len: 65_536 };
        }
        s.measurements.push(m);
    };
    assert_eq!(sent(&mut t, &longer), []);
    // Armed: a shorter one moves the sine.
    let shorter = |s: &mut State| {
        if let MeasKind::Spectrum { config } = &mut by_id(s, 7).config.kind {
            config.resolution = SpecResolution::Fixed { fft_len: 1024 };
        }
    };
    let bin_1024 = 21.0 * 48_000.0 / 1024.0;
    assert_eq!(sent(&mut t, &shorter), [(sine(bin_1024), true, false)]);
    t.key("Enter");
    t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Firing);
    // Playing: the same state again sends nothing; the short one stopped, back to 8192.
    assert_eq!(sent(&mut t, &|_| {}), []);
    let stop = |s: &mut State| by_id(s, 7).running = false;
    assert_eq!(sent(&mut t, &stop), [(sine(BIN_1K), true, true)]);
    // Removed altogether: nothing changes for the sine.
    let remove = |s: &mut State| s.measurements.retain(|m| m.id != MeasId(7));
    assert_eq!(sent(&mut t, &remove), []);
    // Its resolution changed to multi-resolution: 32 768 points, another bin.
    let multi = |s: &mut State| {
        if let MeasKind::Spectrum { config } = &mut by_id(s, 2).config.kind {
            config.resolution = SpecResolution::MULTI;
        }
    };
    assert_eq!(
        sent(&mut t, &multi),
        [(sine(683.0 * 48_000.0 / 32_768.0), true, true)]
    );
    // The last spectrum gone: the sine as typed.
    let none = |s: &mut State| s.measurements.retain(|m| m.id != MeasId(2));
    assert_eq!(sent(&mut t, &none), [(sine(1000.0), true, true)]);
    // A noise never follows them.
    let rta_added = |s: &mut State| s.measurements.push(meas(8, "RTA", rta()));
    t.st.rig.stimulus.generator = Some(SignalKind::Pink);
    let r = t.key("ArrowUp");
    assert_eq!(signals(&r), [(Signal::Pink, true, true)]);
    assert_eq!(sent(&mut t, &rta_added), []);
}
