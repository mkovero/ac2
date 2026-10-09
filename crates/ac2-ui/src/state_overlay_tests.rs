//! Reducer tests of the windows' keyboard: an open window owns ↑/↓, ←/→, the page keys,
//! Enter and Esc, and none of them reaches the stimulus; the stop chord stops from anywhere.

use super::*;
use crate::keys::STOP_ANYWHERE;

/// Requests that touch the generator.
fn stimulus_requests(r: &[Request]) -> Vec<&Request> {
    r.iter()
        .filter(|x| {
            matches!(
                x,
                Request::StimArm { .. }
                    | Request::StimSet(_)
                    | Request::StimStop
                    | Request::Sweep { .. }
            )
        })
        .collect()
}

/// Pink noise at −20 dBFS, playing.
fn firing(t: &mut T) {
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.key("Enter");
    t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
    assert_eq!(t.st.stimulus.phase, StimPhase::Firing);
}

/// The windows, each with how it opens on a daemon that has what it needs.
#[derive(Clone, Copy, Debug)]
enum Window {
    Help,
    Palette,
    Prompt,
    DelayPick,
    Form,
    Sweep,
    Session,
    PaneMenu,
    Offer,
    Calibrations,
    Electrical,
    Acoustic,
    Leq,
    NewLog,
    DeleteTrace,
    DeleteMeas,
}

const WINDOWS: [Window; 16] = [
    Window::Help,
    Window::Palette,
    Window::Prompt,
    Window::DelayPick,
    Window::Form,
    Window::Sweep,
    Window::Session,
    Window::PaneMenu,
    Window::Offer,
    Window::Calibrations,
    Window::Electrical,
    Window::Acoustic,
    Window::Leq,
    Window::NewLog,
    Window::DeleteTrace,
    Window::DeleteMeas,
];

/// A connected daemon with a calibrated mic, an SPL meter with Leq data and a stored trace,
/// playing pink noise; then `w` opened.
fn with_window(w: Window) -> T {
    let mut t = T::new();
    let mut s = mm1_state(CurveChoice::Curve {
        label: "0°".into()
    });
    s.measurements.push(meas(4, "FOH SPL", spl_meter()));
    for c in &mut s.mics[0].curves {
        c.stated_sensitivity = Some(15.0);
    }
    s.traces = vec![stored(10, Some(1), 2)];
    t.conn(mirror(s));
    t.conn(leq_data(1, 100, 80.0, ac2_proto::frame::LeqFlags::NONE));
    firing(&mut t);
    let cmd = |t: &mut T, c: CommandId| {
        t.st.update(Msg::Command(c), &t.keys);
    };
    match w {
        Window::Help => {
            t.key("H");
        }
        Window::Palette => {
            t.key("Ctrl+K");
        }
        Window::Prompt => {
            t.type_key("L", "l");
        }
        Window::DelayPick => {
            t.key("X");
            found(&mut t, DelayPick::FirstArrival, ambiguous());
        }
        Window::Form => cmd(&mut t, CommandId::NewTransfer),
        Window::Sweep => {
            t.type_key("Shift+S", "S");
        }
        Window::Session => {
            open_dialog(&mut t, backends(true));
        }
        Window::PaneMenu => {
            t.st.update(Msg::PaneMenu(t.pane(PaneKind::Transfer)), &t.keys);
        }
        Window::Offer => {
            t.st.overlay = Overlay::Offer(Box::new(Offer {
                transfers: Vec::new(),
            }));
        }
        Window::Calibrations => cmd(&mut t, CommandId::Calibrations),
        Window::Electrical => {
            cmd(&mut t, CommandId::Calibrations);
            t.type_key("E", "e");
            assert!(cal_view(&t).electrical.is_some());
        }
        Window::Acoustic => {
            cmd(&mut t, CommandId::Calibrations);
            t.type_key("C", "c");
            assert!(cal_view(&t).acoustic.is_some());
        }
        Window::Leq => {
            t.type_key("Shift+L", "L");
        }
        Window::NewLog => {
            t.key("Alt+4");
            t.key("Shift+R");
        }
        Window::DeleteTrace => {
            t.key("V");
            t.key("Delete");
        }
        Window::DeleteMeas => {
            t.key("Backspace");
        }
    }
    let open = matches!(
        (w, &t.st.overlay),
        (Window::Help, Overlay::Help)
            | (Window::Palette, Overlay::Palette(_))
            | (Window::Prompt, Overlay::Prompt(_))
            | (Window::DelayPick, Overlay::DelayPick(_))
            | (Window::Form | Window::Sweep, Overlay::Form(_))
            | (Window::PaneMenu, Overlay::PaneMenu(_))
            | (Window::Offer, Overlay::Offer(_))
            | (
                Window::Session
                    | Window::Calibrations
                    | Window::Electrical
                    | Window::Acoustic
                    | Window::Leq,
                Overlay::Settings(_)
            )
            | (Window::NewLog, Overlay::NewLog(_))
    ) || match (w, &t.st.overlay) {
        (Window::DeleteTrace, Overlay::Delete(p)) => {
            matches!(p.target, crate::state::DeleteTarget::Trace(_))
        }
        (Window::DeleteMeas, Overlay::Delete(p)) => {
            matches!(p.target, crate::state::DeleteTarget::Meas(_))
        }
        _ => false,
    };
    assert!(open, "{w:?} did not open: {:?}", t.st.overlay);
    t
}

#[test]
fn a_window_owns_the_arrows_and_esc_closes_it_without_touching_the_stimulus() {
    for w in WINDOWS {
        let mut t = with_window(w);
        let level = t.st.stimulus.level;
        for k in [
            "Up",
            "Down",
            "Shift+Up",
            "Shift+Down",
            "Left",
            "Right",
            "PageUp",
            "PageDown",
            "Home",
            "End",
        ] {
            let r = t.key(k);
            assert!(stimulus_requests(&r).is_empty(), "{w:?} {k}: {r:?}");
            assert_eq!(t.st.stimulus.level, level, "{w:?} {k}");
            assert_eq!(t.st.stimulus.phase, StimPhase::Firing, "{w:?} {k}");
        }
        assert_ne!(t.st.overlay, Overlay::None, "{w:?}: the keys closed it");
        let r = t.key("Esc");
        assert!(stimulus_requests(&r).is_empty(), "{w:?} Esc: {r:?}");
        assert_eq!(t.st.stimulus.phase, StimPhase::Firing, "{w:?}");
        // A dialog over a view closes back to the view; the next Esc closes the view.
        if matches!(w, Window::Electrical | Window::Acoustic) {
            assert!(cal_view(&t).electrical.is_none() && cal_view(&t).acoustic.is_none());
            let r = t.key("Esc");
            assert!(stimulus_requests(&r).is_empty(), "{r:?}");
        }
        assert_eq!(t.st.overlay, Overlay::None, "{w:?}: Esc did not close it");
        // With nothing open Esc stops, as before.
        let r = t.key("Esc");
        assert!(matches!(r.as_slice(), [Request::StimStop]), "{w:?}: {r:?}");
    }
}

#[test]
fn the_stop_chord_stops_from_inside_every_window() {
    assert_eq!(Chord::parse("Shift+Esc"), Ok(STOP_ANYWHERE));
    for w in WINDOWS {
        let mut t = with_window(w);
        let before = std::mem::discriminant(&t.st.overlay);
        let r = t.key("Shift+Esc");
        assert!(matches!(r.as_slice(), [Request::StimStop]), "{w:?}: {r:?}");
        assert_eq!(t.st.stimulus.phase, StimPhase::Stopping, "{w:?}");
        // It stops; it does not close what is open.
        assert_eq!(std::mem::discriminant(&t.st.overlay), before, "{w:?}");
    }
    // With nothing open too, and for another client's generator.
    let mut t = T::new();
    let mut s = daemon_state();
    s.generator.armed = true;
    s.generator.firing = true;
    s.generator.owner = Some(ClientId("other".into()));
    t.conn(mirror(s));
    let r = t.key("Shift+Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
}

#[test]
fn space_and_enter_in_a_window_never_arm_or_fire() {
    // Armed, not firing: Enter in a window that does not use it must not fire, nor Space
    // arm. The help and the delay candidates let other keys through; never these.
    for w in [Window::Help, Window::DelayPick] {
        let mut t = with_window(w);
        t.key("Shift+Esc");
        t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
        let r = t.key("Space");
        assert!(stimulus_requests(&r).is_empty(), "{w:?}: {r:?}");
        assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    }
    let mut t = with_window(Window::Help);
    t.key("Shift+Esc");
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    t.key("Esc");
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.key("H");
    // Enter closes the help; it does not fire.
    let r = t.key("Enter");
    assert!(stimulus_requests(&r).is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.stimulus.phase, StimPhase::Armed);
}

#[test]
fn help_scrolls_with_the_arrows_and_page_keys() {
    let mut t = with_window(Window::Help);
    assert_eq!(t.st.help_scroll, 0.0);
    t.key("Down");
    t.key("Down");
    assert_eq!(t.st.help_scroll, 2.0 * HELP_LINE);
    t.key("Up");
    assert_eq!(t.st.help_scroll, HELP_LINE);
    t.key("Up");
    t.key("Up");
    assert_eq!(t.st.help_scroll, 0.0, "never above the top");
    t.st.help_page = 300.0;
    t.key("PageDown");
    assert_eq!(t.st.help_scroll, 300.0);
    t.key("PageUp");
    assert_eq!(t.st.help_scroll, 0.0);
    t.key("End");
    assert!(t.st.help_scroll > 1e6, "the view clamps it to the end");
    t.key("Home");
    assert_eq!(t.st.help_scroll, 0.0);
    // Other keys keep working with the keys shown: T steps the focused plot's grid.
    let chrome = t.st.modes().chrome;
    t.key("T");
    assert_ne!(t.st.modes().chrome, chrome);
    assert_eq!(t.st.overlay, Overlay::Help);
    // Opened again, it starts at the top.
    t.key("Down");
    t.key("H");
    t.key("H");
    assert_eq!(t.st.help_scroll, 0.0);
}

#[test]
fn delay_candidates_are_chosen_with_the_arrows() {
    let mut t = with_window(Window::DelayPick);
    t.key("Down");
    t.key("Down");
    t.key("Up");
    let Overlay::DelayPick(c) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(c.selected, 1);
    let r = t.key("Enter");
    assert_eq!(inserted(&r), Some(DelayPick::Ranked { index: 1 }));
    assert!(stimulus_requests(&r).is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    // ↑ from the first wraps to the last.
    found(&mut t, DelayPick::FirstArrival, ambiguous());
    t.key("Up");
    let r = t.key("Enter");
    assert_eq!(inserted(&r), Some(DelayPick::Ranked { index: 2 }));
}

#[test]
fn palette_and_lists_page_and_wheel() {
    let mut t = with_window(Window::Palette);
    let n = match &t.st.overlay {
        Overlay::Palette(p) => p.entries(&t.keys, t.st.scope()).len(),
        o => panic!("{o:?}"),
    };
    let sel = |t: &T| match &t.st.overlay {
        Overlay::Palette(p) => p.selected,
        o => panic!("{o:?}"),
    };
    t.key("PageDown");
    assert_eq!(sel(&t), crate::palette::PALETTE_ROWS);
    t.key("End");
    assert_eq!(sel(&t), n - 1);
    t.key("Home");
    assert_eq!(sel(&t), 0);
    t.st.update(Msg::Wheel { rows: 3 }, &t.keys);
    assert_eq!(sel(&t), 3);
    t.st.update(Msg::Wheel { rows: -10 }, &t.keys);
    assert_eq!(sel(&t), 0);

    // The calibrations view: PageDown / End / Home move the focus without wrapping.
    let mut t = with_window(Window::Calibrations);
    let st = t.st.daemon().cloned().expect("state");
    let lines = crate::cal_view::lines(&st).len();
    assert!(lines > 1);
    t.key("End");
    assert_eq!(cal_view(&t).focus, lines - 1);
    t.key("PageDown");
    assert_eq!(cal_view(&t).focus, lines - 1);
    t.key("Home");
    assert_eq!(cal_view(&t).focus, 0);
    t.key("PageUp");
    assert_eq!(cal_view(&t).focus, 0);

    // A pane's measurement list follows the wheel.
    let mut t = with_window(Window::PaneMenu);
    let index = |t: &T| match t.st.overlay {
        Overlay::PaneMenu(m) => m.index,
        ref o => panic!("{o:?}"),
    };
    t.key("Home");
    assert_eq!(index(&t), 0);
    let pane = match t.st.overlay {
        Overlay::PaneMenu(m) => m.pane,
        ref o => panic!("{o:?}"),
    };
    let n = t.st.pane_menu_rows(pane).len();
    t.st.update(Msg::Wheel { rows: 5 }, &t.keys);
    assert_eq!(index(&t), n - 1);
}

#[test]
fn closing_the_sweep_dialog_leaves_the_stimulus_alone() {
    // The dialog makes a sweep measurement; it never arms. Armed (noise) behind it: Esc
    // closes it and the noise stays armed.
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-30.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.type_key("Shift+S", "S");
    assert!(matches!(&t.st.overlay, Overlay::Form(f) if f.kind == FormKind::Sweep));
    assert_eq!(FormKind::Sweep.close_note(), None);
    assert!(t.key("Esc").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.stimulus.phase, StimPhase::Armed);
    // The Cancel button does the same.
    t.type_key("Shift+S", "S");
    assert!(t.st.update(Msg::Form(FormMsg::Cancel), &t.keys).is_empty());
    assert_eq!(t.st.overlay, Overlay::None);

    // Playing: closing the dialog leaves it playing; the stop chord stops it.
    let mut t = with_window(Window::Sweep);
    let r = t.key("Esc");
    assert!(stimulus_requests(&r).is_empty(), "{r:?}");
    assert_eq!(t.st.stimulus.phase, StimPhase::Firing);
    t.type_key("Shift+S", "S");
    let r = t.key("Shift+Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
}

#[test]
fn a_window_over_the_panes_owns_the_mouse() {
    let mut t = T::new();
    assert!(!t.st.window_over_panes());
    t.key("H");
    assert!(t.st.window_over_panes());
    t.key("Esc");
    t.key("X");
    found(&mut t, DelayPick::FirstArrival, ambiguous());
    assert!(
        !t.st.window_over_panes(),
        "the candidates leave the plots to the mouse"
    );
}
