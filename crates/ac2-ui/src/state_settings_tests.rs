//! Reducer tests of the Settings view: opening it at a page from the old dialogs' keys,
//! paging, each page's keys and the requests they make, and that the view owns the
//! keyboard (the stimulus is never touched by its keys; Shift+Esc still stops).

use super::*;
use crate::settings::{ConnLine, DisplayRow, Page};

fn settings(t: &T) -> &Settings {
    t.st.overlay.settings().expect("Settings open")
}

fn page(t: &T) -> Page {
    settings(t).page
}

fn ceiling_call(r: &[Request]) -> Option<(f64, bool)> {
    r.iter().find_map(|r| match r {
        Request::Call {
            cmd:
                Command::GenCeiling {
                    ceiling,
                    confirm_raise,
                },
            ..
        } => Some((ceiling.0, *confirm_raise)),
        _ => None,
    })
}

/// The daemon state with the generator's ceiling and bound.
fn with_ceiling(ceiling: f64, bound: f64, armed: bool) -> State {
    let mut s = daemon_state();
    s.generator.ceiling = Dbfs(ceiling);
    s.generator.ceiling_bound = Dbfs(bound);
    s.generator.armed = armed;
    s
}

#[test]
fn the_old_dialog_keys_open_their_pages_and_ctrl_p_the_last_one() {
    let mut t = T::new();
    t.type_key("Shift+O", "O");
    assert_eq!(page(&t), Page::Audio);
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
    t.st.update(Msg::Command(CommandId::InputSetup), &t.keys);
    assert_eq!(page(&t), Page::Io);
    t.key("Escape");
    t.st.update(Msg::Command(CommandId::Calibrations), &t.keys);
    assert_eq!(page(&t), Page::Calibration);
    t.key("Escape");
    // Ctrl+P opens the page last shown; Ctrl+PgDn / PgUp and Alt+digit move between pages.
    t.key("Ctrl+P");
    assert_eq!(page(&t), Page::Calibration);
    t.key("Ctrl+PageDown");
    assert_eq!(page(&t), Page::Leq);
    t.key("Ctrl+PageUp");
    t.key("Ctrl+PageUp");
    assert_eq!(page(&t), Page::Audio);
    t.key("Alt+7");
    assert_eq!(page(&t), Page::Connection);
    t.key("Ctrl+Tab");
    assert_eq!(page(&t), Page::Io, "wraps");
    // The mouse picks a page too; Ctrl+P again closes the view.
    t.st.update(Msg::Settings(SettingsMsg::Page(Page::Display)), &t.keys);
    assert_eq!(page(&t), Page::Display);
    t.key("Ctrl+P");
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn the_view_owns_the_keys_and_shift_esc_still_stops() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.key("Ctrl+P");
    for k in ["Space", "Enter", "Up", "Down", "Shift+Up", "Left", "Right"] {
        let r = t.key(k);
        assert!(
            !r.iter()
                .any(|x| matches!(x, Request::StimSet(_) | Request::StimStop)),
            "{k}: {r:?}"
        );
        assert_eq!(t.st.stimulus.level, Some(Dbfs(-20.0)), "{k}");
    }
    assert!(t.st.overlay.settings().is_some());
    let r = t.key("Shift+Escape");
    assert!(r.iter().any(|x| matches!(x, Request::StimStop)), "{r:?}");
    assert!(
        t.st.overlay.settings().is_some(),
        "the stop leaves the view open"
    );
}

#[test]
fn lowering_the_max_level_goes_out_at_once_a_raise_needs_the_word() {
    let mut t = T::new();
    t.conn(mirror(with_ceiling(-30.0, -10.0, false)));
    t.st.update(Msg::Command(CommandId::InputSetup), &t.keys);
    // The max level row is below the channels: ↑ from the first channel wraps to it.
    t.st.update(Msg::Settings(SettingsMsg::Ceiling), &t.keys);
    assert!(settings(&t).on_ceiling);
    // Typed text goes to the row, R / M / S do not act.
    t.type_key("Minus", "-");
    t.text("40");
    let r = t.key("Enter");
    assert_eq!(ceiling_call(&r), Some((-40.0, false)));
    // Above the bound: refused here with where the bound comes from.
    t.text("-6");
    assert_eq!(ceiling_call(&t.key("Enter")), None);
    assert!(
        settings(&t)
            .ceiling
            .error
            .as_deref()
            .is_some_and(|e| e.contains("--max-level"))
    );
    // A refused value stays for correcting; Backspace takes it back.
    assert_eq!(settings(&t).ceiling.text, "-6");
    t.st.update(Msg::Backspace, &t.keys);
    t.st.update(Msg::Backspace, &t.keys);
    // A raise: the confirmation first; Esc keeps the level and the view.
    t.text("-20");
    assert_eq!(ceiling_call(&t.key("Enter")), None);
    assert!(settings(&t).ceiling.confirm.is_some());
    t.key("Escape");
    assert!(
        t.st.overlay.settings().is_some(),
        "Esc closed the confirmation only"
    );
    assert!(settings(&t).ceiling.confirm.is_none());
    t.text("-20");
    t.key("Enter");
    t.text("raise");
    let r = t.key("Enter");
    assert_eq!(ceiling_call(&r), Some((-20.0, true)));
    // Never while anything is armed.
    t.conn(mirror(with_ceiling(-30.0, -10.0, true)));
    t.text("-20");
    assert_eq!(ceiling_call(&t.key("Enter")), None);
    assert_eq!(
        settings(&t).ceiling.error.as_deref(),
        Some(ac2_scene::rig::RAISE_WHILE_LIVE)
    );
    // Lowering is never refused for that.
    for _ in 0..3 {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("-35");
    assert_eq!(ceiling_call(&t.key("Enter")), Some((-35.0, false)));
}

#[test]
fn an_output_is_named_for_every_client_on_enter() {
    let mut t = T::new();
    t.st.update(Msg::Command(CommandId::StimulusOutputs), &t.keys);
    t.conn(ConnEvent::Devices(Ok(backends(true))));
    assert_eq!(dialog(&t).focus, Row::Output(0));
    t.type_key("N", "n");
    t.text("Main L");
    let r = t.key("Enter");
    let sent = r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::SessionOutputs { outputs },
            what,
        } => Some((outputs.clone(), what.clone())),
        _ => None,
    });
    assert_eq!(
        sent,
        Some((
            vec![OutputSetup {
                channel: 0,
                label: Some("Main L".into())
            }],
            "output 1: named Main L (every client)".into()
        ))
    );
    // The mirror brings the label back: the row and the top bar's outputs say it.
    let mut s = daemon_state();
    s.outputs = vec![OutputSetup {
        channel: 0,
        label: Some("Main L".into()),
    }];
    t.conn(mirror(s));
    assert_eq!(dialog(&t).outputs[0].label(), "Main L");
    // ↓ drops an edit unsent; an empty label clears.
    t.type_key("N", "n");
    t.text("x");
    t.key("Down");
    assert_eq!(dialog(&t).edit, None);
    t.key("Up");
    t.type_key("N", "n");
    for _ in 0.."Main L".len() {
        t.st.update(Msg::Backspace, &t.keys);
    }
    let r = t.key("Enter");
    assert!(r.iter().any(|r| matches!(r,
        Request::Call { cmd: Command::SessionOutputs { outputs }, .. }
            if outputs == &[OutputSetup { channel: 0, label: None }])));
}

#[test]
fn display_page_changes_apply_at_once_and_are_remembered() {
    let mut t = T::new();
    t.key("Ctrl+P");
    t.st.update(Msg::Settings(SettingsMsg::Page(Page::Display)), &t.keys);
    assert_eq!(page(&t), Page::Display);
    t.st.prefs_dirty = false;
    t.key("Right");
    assert_eq!(t.st.theme, ThemeName::Light);
    assert_eq!(t.st.prefs.theme, Some(ThemeName::Light));
    t.key("Down");
    t.key("Space");
    assert!(!t.st.prefs.key_hints);
    t.key("Down");
    t.key("Right");
    assert_eq!(t.st.prefs.spl_hold_ms, Some(250));
    t.key("Down");
    t.key("Right");
    assert_eq!(t.st.view.spectrum.spectrograph.span_s, 60);
    assert_eq!(t.st.prefs.spectrograph_span_s, Some(60));
    t.key("Down");
    assert_eq!(settings(&t).display, DisplayRow::LevelAxes);
    t.st.view.tf.magnitude_db = ac2_scene::axis::Range { lo: -3.0, hi: 3.0 };
    t.key("Enter");
    assert_eq!(
        crate::prefs::LevelPrefs::of(&t.st.view),
        crate::prefs::LevelPrefs::default()
    );
    assert!(t.st.prefs_dirty);
}

#[test]
fn recording_limit_is_this_apps_and_the_record_key_uses_it() {
    let mut t = T::new();
    t.key("Ctrl+P");
    t.key("Alt+5");
    assert_eq!(page(&t), Page::Recording);
    // Opening it asks the daemon where it records.
    t.text("90");
    t.key("Enter");
    assert_eq!(t.st.prefs.record_limit_min, Some(90));
    t.key("Escape");
    let r = t.st.update(Msg::Command(CommandId::Record), &t.keys);
    let max = r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::RecStart { request },
            ..
        } => Some(request.max_duration.0),
        _ => None,
    });
    assert_eq!(max, Some(5400.0));
}

#[test]
fn connection_page_asks_the_daemon_authorizes_and_revokes() {
    let mut t = T::new();
    t.key("Ctrl+P");
    let r = t.key("Alt+7");
    assert!(r.iter().any(|r| matches!(r, Request::ServerInfo)), "{r:?}");
    // Asked again while the page is shown.
    t.st.update(
        Msg::Tick {
            now_s: 1.0,
            dt_s: 1.0,
        },
        &t.keys,
    );
    let r = t.st.update(
        Msg::Tick {
            now_s: 2.5,
            dt_s: 1.5,
        },
        &t.keys,
    );
    assert!(r.iter().any(|r| matches!(r, Request::ServerInfo)), "{r:?}");
    let info = ac2_proto::samples::server_info();
    t.conn(ConnEvent::Server {
        what: None,
        result: Ok(info.clone()),
    });
    let lines = settings(&t).connection.lines();
    assert_eq!(lines[2], ConnLine::Authorized("laptop".into()));
    // Enter on Reconnect; Enter on Connect… asks the app for the connect dialog.
    assert!(
        t.key("Enter")
            .iter()
            .any(|r| matches!(r, Request::Reconnect))
    );
    t.key("Down");
    t.key("Enter");
    assert!(t.st.want_connect_dialog);
    // Delete twice revokes the focused client.
    t.key("Down");
    assert!(t.key("Delete").is_empty());
    let r = t.key("Delete");
    assert!(r.iter().any(|r| matches!(r,
        Request::ServerCall { cmd: Command::ServerRevoke { name }, .. } if name == "laptop")));
    // A on a refused key, a name, Enter authorizes it.
    t.key("Down");
    t.type_key("A", "a");
    t.text("tablet");
    let r = t.key("Enter");
    assert!(r.iter().any(|r| matches!(r,
        Request::ServerCall { cmd: Command::ServerAuthorize { name, .. }, .. } if name == "tablet")));
    // The answer to a change replaces the info; a failure leaves it and says why.
    t.conn(ConnEvent::Server {
        what: Some("client tablet authorized".into()),
        result: Err("invalid: client name \"tablet\" is already authorized".into()),
    });
    assert!(t.last_toast().contains("already authorized"));
    assert_eq!(settings(&t).connection.server, Some(Ok(info)));
}
