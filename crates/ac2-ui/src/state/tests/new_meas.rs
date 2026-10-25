//! Reducer tests of the New measurement chooser (Ctrl+M): its rows, the kind it guesses
//! from the focused pane, the kinds it refuses and why, the pane list's way in, and the
//! pane a measurement made through it opens in.

use super::*;
use crate::forms::FormKind;

const ALIGN_REFUSAL: &str = "an alignment needs two transfer curves, live or stored: make a \
                             transfer measurement or capture one";

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
            "Alignment — delay and polarity of two systems in their overlap (sub + main, delay \
             tower)  ·  Alt+M",
            "Programme meter — meters inputs live in a new pane (levels, correlation); adds no \
             measurement",
        ]
    );
    assert_eq!(c.index, 0, "the transfer pane's kind");
    assert_eq!(
        c.hint,
        "↑/↓ choose · Enter opens its settings · Esc cancels"
    );
    // One transfer function: Alt+M would refuse, and the row says so in its words.
    assert!(
        c.choices[6]
            .blocked
            .as_deref()
            .is_some_and(|w| w.starts_with(ALIGN_REFUSAL)),
        "{:?}",
        c.choices[6]
    );
    assert_eq!(
        c.choices[7].blocked, None,
        "the session has inputs to meter"
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
    t.go(PaneKind::Programme);
    assert_eq!(at(&mut t), 7, "the Programme meter");
}

/// The Alignment row opens what Alt+M opens: the math dialog, its operator on align.
#[test]
fn the_alignment_row_opens_the_aligned_math_dialog() {
    use ac2_scene::new_meas::NewKind;
    let mut t = T::new();
    t.conn(mirror(four()));
    t.put(PaneKind::Transfer);
    let at = NewKind::ALL
        .iter()
        .position(|k| *k == NewKind::Alignment)
        .expect("alignment row");
    t.key("Ctrl+M");
    assert_eq!(chooser(&t).choices[at].blocked, None);
    t.st.update(Msg::Choose(Some(at)), &t.keys);
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(f.kind, FormKind::Math);
    assert!(
        f.fields[1].display().starts_with("↔  aligned"),
        "{}",
        f.fields[1].display()
    );
    let from_row = f.fields.iter().map(|x| x.display()).collect::<Vec<_>>();
    t.key("Esc");
    t.key("Alt+M");
    let Overlay::Form(g) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    let by_key = g.fields.iter().map(|x| x.display()).collect::<Vec<_>>();
    assert_eq!(from_row, by_key);
}

/// A session without inputs has nothing to meter: the row says so.
#[test]
fn the_meter_row_is_blocked_without_inputs() {
    let mut t = T::new();
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.config.input_channels.clear();
    }
    t.conn(mirror(s));
    t.key("Ctrl+M");
    assert_eq!(
        chooser(&t).choices[7].blocked.as_deref(),
        Some("no inputs in the session to meter")
    );
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

/// From an empty daemon: every command that makes a measurement opens the Sessions dialog
/// rather than a chooser that could only refuse; its New opens the audio setup, headed as a
/// new session, and the opened session asks what to measure.
#[test]
fn without_a_session_new_measurement_goes_through_the_sessions_dialog() {
    let mut t = T::new();
    t.conn(mirror(no_session_state()));
    for c in [
        CommandId::NewMeasurement,
        CommandId::NewTransfer,
        CommandId::NewSpectrum,
        CommandId::NewRta,
        CommandId::NewSpl,
        CommandId::SweepNew,
    ] {
        let r = t.st.update(Msg::Command(c), &t.keys);
        assert!(
            matches!(r.as_slice(), [Request::SessionList]),
            "{c:?}: {r:?}"
        );
        assert!(
            t.st.sessions_dialog().is_some(),
            "{c:?}: {:?}",
            t.st.overlay
        );
        t.key("Escape");
        assert_eq!(t.st.overlay, Overlay::None);
    }
    t.key("Ctrl+M");
    let i =
        t.st.sessions_rows(WallNs(10))
            .iter()
            .position(|r| r.title == "New session")
            .expect("New session row");
    let r = t.st.update(Msg::Sessions(SessionsMsg::Row(i)), &t.keys);
    assert!(
        matches!(r.as_slice(), [Request::Sessions(j)]
            if j.steps == [crate::sessions::SessionStep::New { discard: false }]),
        "{r:?}"
    );
    t.conn(ConnEvent::Sessions(Box::new(
        crate::sessions::SessionOutcome {
            done: vec![crate::sessions::SessionDone::New],
            failed: None,
        },
    )));
    let s = t.st.overlay.settings().expect("the audio setup");
    assert_eq!(s.page, crate::settings::Page::AudioIo);
    assert_eq!(s.heading(), crate::settings::NEW_SESSION_AUDIO);
    assert_eq!(s.heading(), "New session — Audio I/O");
    t.conn(ConnEvent::Devices(Ok(backends(true))));
    let r = t.key("Enter");
    assert!(opened(&r).is_some(), "{r:?}");
    t.conn(ConnEvent::SessionOpened);
    let mut s = daemon_state();
    s.measurements.clear();
    t.conn(mirror(s));
    assert_eq!(chooser(&t).purpose, ChoicePurpose::NewMeasurement);
    // With the session open, the chooser opens directly again.
    t.key("Escape");
    t.key("Ctrl+M");
    assert_eq!(chooser(&t).purpose, ChoicePurpose::NewMeasurement);
}

/// Shift+O opens the audio page under its own name: only the Sessions dialog's New heads
/// it as a new session.
#[test]
fn the_audio_page_is_a_new_session_only_after_new() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    let s = t.st.overlay.settings().expect("the audio setup");
    assert_eq!(s.heading(), crate::settings::AUDIO_IO);
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
             sweep, math channel, alignment)"
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

/// The list as text: each row's name and first detail, child rows under their header.
fn list_text(t: &T) -> Vec<String> {
    t.st.tree_rows()
        .iter()
        .map(|row| {
            let lead = match (row.depth, row.last) {
                (0, _) => "",
                (_, false) => "  ├ ",
                (_, true) => "  └ ",
            };
            format!("{lead}{} · {}", row.name, row.details[0])
        })
        .collect()
}

/// Kind `i` of the chooser made on the daemon as id `id`: its form taken as it opens (the
/// sweep's, which asks for a reference and a level, as the test sweep fills it), the
/// daemon listing it and answering the create.
fn create(t: &mut T, i: usize, id: u32) -> MeasConfig {
    t.key("Ctrl+M");
    t.st.update(Msg::Choose(Some(i)), &t.keys);
    let r = t.key("Enter");
    let c = match created(&r) {
        Some(c) => c.clone(),
        None if ac2_scene::new_meas::NewKind::ALL[i] == ac2_scene::new_meas::NewKind::Sweep => {
            t.key("Escape");
            sweep_measurement().config
        }
        None => panic!("kind {i}: {r:?} {:?}", t.st.overlay),
    };
    let m = meas(id, &c.name, c.kind.clone());
    t.remirror(|s| s.measurements.push(m.clone()));
    t.conn(ConnEvent::MeasCreated(Box::new(m)));
    c
}

/// A measurement made from an empty daemon is selected with its live curve listed under
/// it, and the list scrolls to it.
#[test]
fn a_new_spectrum_is_selected_and_unfolded() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.clear();
    t.conn(mirror(s));
    let c = create(&mut t, 1, 1);
    assert_eq!(t.st.selected, Some(MeasId(1)));
    assert!(t.st.tree_reveal, "the list scrolls to the new row");
    let rows = t.st.tree_rows();
    assert_eq!(rows[0].mark, ac2_scene::meas_list::Mark::Active);
    assert_eq!(
        list_text(&t),
        [
            format!("FFT  {} · running", c.name),
            format!("  └ {} (live) · live", c.name),
        ]
    );
}

/// A fold kept from a measurement that is gone (another session, a restarted daemon) does
/// not fold the new one that takes its id: every kind opens unfolded, its live curve (when
/// it has one) listed under it.
#[test]
fn a_new_measurement_is_never_folded() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.clear();
    t.conn(mirror(s));
    for (i, id) in [(0, 2), (1, 3), (2, 4), (3, 5), (4, 6)] {
        t.st.collapsed.insert(TraceOwner::Meas { meas: MeasId(id) });
        let c = create(&mut t, i, id);
        assert_eq!(t.st.selected, Some(MeasId(id)), "{c:?}");
        let row =
            t.st.tree_rows()
                .into_iter()
                .find(|r| r.key == ac2_scene::meas_list::TreeKey::Meas(MeasId(id)))
                .expect("its header");
        assert_eq!(row.collapsed, Some(false), "{}", row.name);
        if ac2_scene::meas_list::has_live_curve(&c.kind) {
            assert!(
                list_text(&t).contains(&format!("  └ {} (live) · live", c.name)),
                "{:?}",
                list_text(&t)
            );
        }
    }
}

/// A fold goes with its measurement: a later one listed under the same id (a session
/// loaded, a restarted daemon numbering afresh) starts unfolded.
#[test]
fn a_gone_measurements_fold_is_forgotten() {
    let mut t = T::new();
    let sub = TraceOwner::Meas { meas: MeasId(2) };
    t.st.update(Msg::ToggleGroup(sub), &t.keys);
    t.st.update(Msg::ToggleGroup(TraceOwner::Imported), &t.keys);
    t.remirror(|s| s.measurements.retain(|m| m.id != MeasId(2)));
    assert!(!t.st.collapsed.contains(&sub));
    assert!(t.st.collapsed.contains(&TraceOwner::Imported));
}

/// Tab steps the chooser's rows down, Shift+Tab up.
#[test]
fn shift_tab_steps_the_chooser_back() {
    let mut t = T::new();
    t.put(PaneKind::Transfer);
    t.key("Ctrl+M");
    let first = chooser(&t).index;
    t.key("Tab");
    assert_eq!(chooser(&t).index, first + 1);
    t.key("Shift+Tab");
    assert_eq!(chooser(&t).index, first);
}
