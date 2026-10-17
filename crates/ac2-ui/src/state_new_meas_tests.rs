//! Reducer tests of the New measurement chooser (Ctrl+M): its rows, the kind it guesses
//! from the focused pane, the kinds it refuses and why, the pane list's way in, and the
//! pane a measurement made through it opens in.

use super::*;
use crate::forms::FormKind;

const MATH_REFUSAL: &str = "a math channel needs two transfer functions, spectra or RTAs of one \
                            kind, live or stored: make a measurement or capture one";

/// The open chooser.
fn chooser(t: &T) -> &ChoicePrompt {
    match &t.st.overlay {
        Overlay::Choose(c) if c.purpose == ChoicePurpose::NewMeasurement => c,
        other => panic!("no chooser: {other:?}"),
    }
}

fn labels(t: &T) -> Vec<String> {
    chooser(t).choices.iter().map(|c| c.label.clone()).collect()
}

#[test]
fn ctrl_m_lists_every_kind_with_its_own_key() {
    let mut t = T::new();
    t.put(PaneKind::Transfer);
    t.key("Ctrl+M");
    let c = chooser(&t);
    assert_eq!(c.title, "New measurement");
    assert_eq!(
        labels(&t),
        [
            "Transfer — reference vs measurement: magnitude, phase, coherence, IR",
            "Spectrum — one input's level at every frequency (FFT)",
            "RTA — one input's level in fractional-octave bands",
            "SPL meter — sound level: A / C / Z weighting, Leq windows and limits",
            "Sweep — a played sweep: response and harmonic distortion  ·  Shift+S",
            "Math channel — A ÷ × + − B of two curves, or the average of several  ·  Shift+M",
        ]
    );
    assert_eq!(c.index, 0, "the transfer pane's kind");
    assert_eq!(
        c.hint,
        "↑/↓ choose · Enter opens its settings · Esc cancels"
    );
    // One transfer function and one spectrum: Shift+M would refuse, and the row says so.
    assert!(
        c.choices[5]
            .blocked
            .as_deref()
            .is_some_and(|w| w.starts_with(MATH_REFUSAL)),
        "{:?}",
        c.choices[5]
    );
    assert!(c.choices[..5].iter().all(|x| x.blocked.is_none()));
    t.key("Shift+M");
    // The window owns the keys: Shift+M did nothing behind it.
    assert_eq!(chooser(&t).index, 0);
    t.key("Esc");
    t.st.update(Msg::Command(CommandId::NewMath), &t.keys);
    assert!(
        t.last_toast().starts_with(MATH_REFUSAL),
        "{}",
        t.last_toast()
    );

    // A key bound to a kind shows; unbound, the row has none.
    let keys =
        Keymap::from_toml("[global]\nsweep_new = []\nmeas_new_rta = [\"Shift+F9\"]").expect("keys");
    t.st.update(Msg::Command(CommandId::NewMeasurement), &keys);
    let l = labels(&t);
    assert!(l[2].ends_with("  ·  Shift+F9"), "{}", l[2]);
    assert!(!l[4].contains("Shift+S"), "{}", l[4]);
}

#[test]
fn the_focused_pane_picks_the_kind_highlighted() {
    let mut t = T::new();
    let at = |t: &mut T| {
        t.key("Ctrl+M");
        let i = chooser(t).index;
        t.key("Esc");
        assert_eq!(t.st.overlay, Overlay::None);
        i
    };
    let sp = t.pane(PaneKind::Spectrum);
    t.st.update(Msg::PanePick(sp, PaneMenuRow::Meas(MeasId(2))), &t.keys);
    assert_eq!(at(&mut t), 1, "spectrum");
    t.conn(mirror(four()));
    t.st.update(Msg::PanePick(sp, PaneMenuRow::Meas(MeasId(4))), &t.keys);
    assert_eq!(at(&mut t), 2, "an RTA on the spectrum pane");
    t.go(PaneKind::Spl);
    assert_eq!(at(&mut t), 3, "SPL meter");
    t.go(PaneKind::Distortion);
    assert_eq!(at(&mut t), 4, "sweep");
    t.go(PaneKind::Transfer);
    assert_eq!(at(&mut t), 0, "transfer");
}

#[test]
fn enter_opens_the_kinds_form_and_a_refused_kind_stays_listed() {
    let mut t = T::new();
    t.put(PaneKind::Transfer);
    t.key("Ctrl+M");
    t.key("ArrowDown");
    assert_eq!(chooser(&t).index, 1);
    t.key("Enter");
    assert!(matches!(&t.st.overlay, Overlay::Form(f) if f.kind == FormKind::Spectrum));
    t.key("Esc");
    // A click on a row takes it as Enter does.
    t.key("Ctrl+M");
    t.st.update(Msg::Choose(Some(4)), &t.keys);
    assert!(matches!(&t.st.overlay, Overlay::Form(f) if f.kind == FormKind::Sweep));
    t.key("Esc");
    // The math row refused: the chooser stays and says why.
    t.key("Ctrl+M");
    for _ in 0..5 {
        t.key("ArrowDown");
    }
    t.key("Enter");
    assert_eq!(chooser(&t).index, 5);
    assert!(
        t.last_toast()
            .starts_with(&format!("not possible: {MATH_REFUSAL}")),
        "{}",
        t.last_toast()
    );
    t.key("Esc");
    // Two transfer functions: the math channel's form.
    t.conn(mirror(four()));
    t.key("Ctrl+M");
    assert_eq!(chooser(&t).choices[5].blocked, None);
    t.st.update(Msg::Choose(Some(5)), &t.keys);
    assert!(matches!(&t.st.overlay, Overlay::Form(f) if f.kind == FormKind::Math));
}

#[test]
fn without_a_session_the_kinds_say_how_to_open_one() {
    let mut t = T::new();
    t.conn(mirror(no_session_state()));
    t.key("Ctrl+M");
    let c = chooser(&t).clone();
    let hint = open_session_hint(&t.keys);
    assert_eq!(
        c.choices[1].blocked.as_deref(),
        Some(format!("no audio session to measure: {hint}").as_str())
    );
    assert_eq!(
        c.choices[4].blocked.as_deref(),
        Some(format!("no audio session to sweep: {hint}").as_str())
    );
    t.key("Enter");
    assert!(matches!(t.st.overlay, Overlay::Choose(_)));
}

#[test]
fn the_pane_list_ends_in_new_measurement() {
    let mut t = T::new();
    let spl = t.pane(PaneKind::Spl);
    t.st.update(Msg::PaneMenu(spl), &t.keys);
    let rows = t.st.pane_menu_rows(spl);
    let (last, text) = rows.last().expect("rows").clone();
    assert_eq!(last, PaneMenuRow::NewMeasurement);
    assert_eq!(text, "New measurement…  Ctrl+M");
    t.key("End");
    t.key("Enter");
    // The list's pane is the one the chooser guesses from.
    assert_eq!(chooser(&t).index, 3);
    assert_eq!(t.st.layout.focus, spl);
}

#[test]
fn empty_texts_name_the_chooser() {
    let mut t = T::new();
    assert_eq!(t.st.empty_list_text(), "none — Ctrl+M: New measurement…");
    let mut s = daemon_state();
    s.measurements.clear();
    t.conn(mirror(s));
    assert_eq!(
        t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys)
            .map(|h| h.text)
            .as_deref(),
        Some(
            "No measurements — Ctrl+M → New measurement… (transfer, spectrum, RTA, SPL meter, \
             sweep, math channel)"
        )
    );
}

/// A kind with no pane on screen gets one split off the focused pane; the focused pane
/// keeps what it shows.
#[test]
fn a_new_meter_opens_in_its_own_pane() {
    let mut t = T::new();
    t.go(PaneKind::Spl);
    t.key("Q");
    assert!(!t.shown(PaneKind::Spl));
    t.put(PaneKind::Transfer);
    let tf = t.st.layout.focus;
    let before = t.visible();
    t.key("Ctrl+M");
    t.st.update(Msg::Choose(Some(3)), &t.keys);
    let r = t.key("Enter");
    let c = r
        .iter()
        .find_map(|x| match x {
            Request::CreateMeas { config } => Some(config.clone()),
            _ => None,
        })
        .expect("meas.create");
    let m = meas(9, &c.name, c.kind.clone());
    t.conn(ConnEvent::MeasCreated(Box::new(m)));
    assert_eq!(t.st.layout.kind(tf), PaneKind::Transfer);
    assert_eq!(t.visible().len(), before.len() + 1);
    let spl = t.pane(PaneKind::Spl);
    assert_ne!(spl, tf);
    assert_eq!(t.st.layout.view(spl).and_then(|v| v.meas), Some(MeasId(9)));
}
