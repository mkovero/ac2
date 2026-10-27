//! Reducer tests of the Sessions dialog: unsaved changes asked about before New, Open and
//! quitting (Save / Discard / Cancel), Save as onto another session, an old session's
//! traces salvaged, the recents and the audio settings after New.

use super::*;
use crate::sessions::{
    Page as SPage, SessionDone, SessionFailure, SessionJob, SessionOutcome, SessionStep, Source,
};
use crate::state::display::ChoicePurpose;
use ac2_proto::{ErrorCode, ErrorDetail};

fn with_working(dirty: bool, origin: Option<SessionOrigin>) -> T {
    let mut t = T::new();
    let mut s = daemon_state();
    s.working = WorkingSession {
        origin,
        dirty,
        saved_at: Some(WallNs(1)),
        recover: None,
    };
    t.conn(mirror(s));
    t
}

fn named(n: &str) -> Option<SessionOrigin> {
    Some(SessionOrigin::Name { name: n.into() })
}

fn job(r: &[Request]) -> Vec<SessionStep> {
    match r {
        [Request::Sessions(SessionJob { steps })] => steps.clone(),
        other => panic!("{other:?}"),
    }
}

fn open_friday() -> SessionStep {
    SessionStep::Open {
        from: Source::Daemon(SessionRef::Name {
            name: "friday".into(),
        }),
        discard: false,
    }
}

fn question(t: &T) -> (String, Vec<String>) {
    match &t.st.overlay {
        Overlay::Choose(c) => (
            c.title.clone(),
            c.choices.iter().map(|c| c.label.clone()).collect(),
        ),
        other => panic!("{other:?}"),
    }
}

fn dialog(t: &mut T) {
    let r = t.key("Alt+S");
    assert!(matches!(r.as_slice(), [Request::SessionList]), "{r:?}");
    t.conn(ConnEvent::SessionList(Ok(vec![SessionFile {
        name: "friday".into(),
        path: "/srv/sessions/friday".into(),
        saved_at: WallNs(5),
        measurements: 1,
        traces: 2,
    }])));
}

/// The row of the first page titled `title`.
fn row(t: &T, title: &str) -> usize {
    t.st.sessions_rows(WallNs(10))
        .iter()
        .position(|r| r.title == title)
        .unwrap_or_else(|| panic!("no row {title}"))
}

fn click(t: &mut T, m: SessionsMsg) -> Vec<Request> {
    t.st.apply(Msg::Sessions(m), &t.keys)
}

#[test]
fn a_clean_state_opens_a_recent_session_at_once() {
    let mut t = with_working(false, named("sat"));
    dialog(&mut t);
    let i = row(&t, "friday");
    let r = click(&mut t, SessionsMsg::Row(i));
    assert_eq!(job(&r), [open_friday()]);
    // Done: the dialog closes, the toast says what opened.
    t.conn(ConnEvent::Sessions(Box::new(SessionOutcome {
        done: vec![SessionDone::Opened {
            file: SessionFile {
                name: "friday".into(),
                path: "/srv/sessions/friday".into(),
                saved_at: WallNs(5),
                measurements: 1,
                traces: 2,
            },
            here: None,
        }],
        failed: None,
    })));
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(
        t.last_toast(),
        "opened friday (1 measurement · 2 traces), disarmed"
    );
}

#[test]
fn unsaved_changes_ask_save_discard_or_cancel() {
    let mut t = with_working(true, named("sat"));
    dialog(&mut t);
    let i = row(&t, "friday");
    assert!(click(&mut t, SessionsMsg::Row(i)).is_empty());
    let (title, answers) = question(&t);
    assert_eq!(title, "Save changes to sat?");
    assert_eq!(answers, ["Save", "Discard", "Cancel"]);
    // Cancel: nothing sent.
    assert!(t.key("Escape").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);

    // Save: saved where it belongs, then opened (not discarding: a failed save stops it).
    dialog(&mut t);
    click(&mut t, SessionsMsg::Row(i));
    let r = t.key("Enter");
    assert_eq!(
        job(&r),
        [
            SessionStep::Save {
                session: SessionRef::Name { name: "sat".into() },
                overwrite: true,
            },
            open_friday(),
        ]
    );

    // Discard: opened, discarding.
    dialog(&mut t);
    click(&mut t, SessionsMsg::Row(i));
    t.key("Right");
    let r = t.key("Enter");
    assert_eq!(
        job(&r),
        [SessionStep::Open {
            from: Source::Daemon(SessionRef::Name {
                name: "friday".into()
            }),
            discard: true,
        }]
    );
}

#[test]
fn untitled_save_names_it_first_then_goes_on() {
    let mut t = with_working(true, None);
    dialog(&mut t);
    let i = row(&t, "New session");
    click(&mut t, SessionsMsg::Row(i));
    let (title, answers) = question(&t);
    assert_eq!(title, "Save the untitled measurements and traces?");
    assert_eq!(answers[0], "Save as…");
    assert!(t.key("Enter").is_empty());
    assert!(matches!(
        t.st.sessions_dialog().map(|d| &d.page),
        Some(SPage::Name(_))
    ));
    t.text("fri/day");
    t.key("Enter");
    let Some(SPage::Name(n)) = t.st.sessions_dialog().map(|d| &d.page) else {
        panic!()
    };
    assert!(n.error.is_some(), "a path is not a name");
    for _ in 0..7 {
        t.key("Backspace");
    }
    t.text("friday");
    let r = t.key("Enter");
    assert_eq!(
        job(&r),
        [
            SessionStep::Save {
                session: SessionRef::Name {
                    name: "friday".into()
                },
                overwrite: false,
            },
            SessionStep::New { discard: false },
        ]
    );
    // The name is taken: asked before replacing it, then the same steps go on.
    let r = t.conn(ConnEvent::Sessions(Box::new(SessionOutcome {
        done: vec![],
        failed: Some(SessionFailure {
            step: SessionStep::Save {
                session: SessionRef::Name {
                    name: "friday".into(),
                },
                overwrite: false,
            },
            rest: vec![SessionStep::New { discard: false }],
            error: "conflict".into(),
            code: Some(ErrorCode::Conflict),
            detail: Some(ErrorDetail::SessionExists {
                name: "friday".into(),
                saved_at: WallNs(0),
            }),
        }),
    })));
    assert!(r.is_empty());
    let (title, answers) = question(&t);
    assert_eq!(title, "Replace session friday?");
    assert_eq!(answers, ["Replace", "Cancel"]);
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!()
    };
    assert_eq!(c.index, 1, "Cancel is the default");
    t.key("Left");
    let r = t.key("Enter");
    assert_eq!(
        job(&r),
        [
            SessionStep::Save {
                session: SessionRef::Name {
                    name: "friday".into()
                },
                overwrite: true,
            },
            SessionStep::New { discard: false },
        ]
    );
    // New done: the audio settings open.
    t.conn(ConnEvent::Sessions(Box::new(SessionOutcome {
        done: vec![
            SessionDone::Saved(SessionFile {
                name: "friday".into(),
                path: "/x".into(),
                saved_at: WallNs(0),
                measurements: 0,
                traces: 1,
            }),
            SessionDone::New,
        ],
        failed: None,
    })));
    assert!(
        matches!(t.st.overlay, Overlay::Settings(_)),
        "{:?}",
        t.st.overlay
    );
}

#[test]
fn an_old_session_offers_its_traces() {
    let mut t = with_working(false, None);
    dialog(&mut t);
    t.conn(ConnEvent::Sessions(Box::new(SessionOutcome {
        done: vec![],
        failed: Some(SessionFailure {
            step: open_friday(),
            rest: vec![],
            error: "unsupported".into(),
            code: Some(ErrorCode::Unsupported),
            detail: None,
        }),
    })));
    let (title, answers) = question(&t);
    assert_eq!(title, "friday cannot be opened");
    assert_eq!(answers[0], "Import its traces");
    t.key("Left");
    let r = t.key("Enter");
    assert_eq!(
        job(&r),
        [SessionStep::Salvage {
            from: Source::Daemon(SessionRef::Name {
                name: "friday".into()
            })
        }]
    );
}

#[test]
fn quitting_an_embedded_daemon_with_unsaved_changes_asks() {
    let mut t = with_working(true, named("sat"));
    t.st.rig.embedded_autosave = Some(true);
    t.key("Ctrl+Q");
    assert!(!t.st.quit);
    let (_, answers) = question(&t);
    assert_eq!(answers, ["Save", "Quit without saving", "Cancel"]);
    assert!(!t.st.close_requested(), "the window stays while asked");
    t.key("Right");
    t.key("Enter");
    assert!(t.st.quit);

    // Save, then quit once saved.
    let mut t = with_working(true, named("sat"));
    t.st.rig.embedded_autosave = Some(false);
    assert!(!t.st.close_requested());
    let r = t.key("Enter");
    assert_eq!(
        job(&r),
        [
            SessionStep::Save {
                session: SessionRef::Name { name: "sat".into() },
                overwrite: true,
            },
            SessionStep::Quit,
        ]
    );
    assert!(!t.st.quit);
    t.conn(ConnEvent::Sessions(Box::new(SessionOutcome {
        done: vec![SessionDone::Quit],
        failed: None,
    })));
    assert!(t.st.quit);

    // A daemon elsewhere keeps its state: no question.
    let mut t = with_working(true, named("sat"));
    t.key("Ctrl+Q");
    assert!(t.st.quit);
}

#[test]
fn files_opened_here_join_the_recents() {
    let mut t = with_working(false, None);
    dialog(&mut t);
    t.conn(ConnEvent::Sessions(Box::new(SessionOutcome {
        done: vec![SessionDone::Opened {
            file: SessionFile {
                name: "fri.ac2session".into(),
                path: "/media/S/fri.ac2session".into(),
                saved_at: WallNs(0),
                measurements: 0,
                traces: 0,
            },
            here: Some("/media/S/fri.ac2session".into()),
        }],
        failed: None,
    })));
    assert_eq!(
        t.st.prefs.recent_sessions[0].path,
        "/media/S/fri.ac2session"
    );
    assert!(t.st.prefs_dirty);
    dialog(&mut t);
    let i = row(&t, "fri.ac2session");
    let r = click(&mut t, SessionsMsg::Row(i));
    assert_eq!(
        job(&r),
        [SessionStep::Open {
            from: Source::Here("/media/S/fri.ac2session".into()),
            discard: false,
        }]
    );
}

#[test]
fn the_top_bar_label_and_the_palette() {
    let t = with_working(true, named("sat"));
    let l = t.st.working_label(WallNs(0));
    assert_eq!(l.text, "sat · unsaved changes");
    let mut t = with_working(false, named("sat"));
    let r = t.st.apply(Msg::Command(CommandId::SessionSave), &t.keys);
    assert_eq!(
        job(&r),
        [SessionStep::Save {
            session: SessionRef::Name { name: "sat".into() },
            overwrite: true,
        }]
    );
    let _ = ChoicePurpose::Salvage {
        from: Source::Here("/x".into()),
    };
}

/// Save as with a name full of letters that are shortcuts elsewhere (A, F, W, Q, G, N, Tab):
/// every key press types, none runs its command, and Enter saves under that name.
#[test]
fn save_as_types_shortcut_letters_into_the_name() {
    let mut t = with_working(true, named("sat"));
    dialog(&mut t);
    let i = row(&t, "Save as…");
    assert!(click(&mut t, SessionsMsg::Row(i)).is_empty());
    assert!(t.st.overlay.takes_text(), "the name page takes typed text");
    // The suggested name goes; the typed one comes in key by key, as egui delivers it.
    for _ in 0..8 {
        t.key("Backspace");
    }
    let mut r = Vec::new();
    for (k, c) in [
        ("Shift+A", "A"),
        ("F", "f"),
        ("W", "w"),
        ("Q", "q"),
        ("G", "g"),
        ("N", "n"),
        ("Space", " "),
        ("S", "s"),
        ("H", "h"),
    ] {
        r.extend(t.type_key(k, c));
    }
    r.extend(t.key("Tab"));
    assert!(r.is_empty(), "no shortcut ran: {r:?}");
    let Some(SPage::Name(n)) = t.st.sessions_dialog().map(|d| &d.page) else {
        panic!("{:?}", t.st.overlay)
    };
    assert_eq!(n.text, "Afwqgn sh");
    let r = t.key("Enter");
    assert_eq!(
        job(&r),
        [SessionStep::Save {
            session: SessionRef::Name {
                name: "Afwqgn sh".into()
            },
            overwrite: false,
        }]
    );
}

/// Esc on the name page goes back to the first page (nothing saved); a second Esc closes.
#[test]
fn save_as_esc_backs_out_without_saving() {
    let mut t = with_working(true, named("sat"));
    dialog(&mut t);
    let i = row(&t, "Save as…");
    click(&mut t, SessionsMsg::Row(i));
    t.type_key("X", "x");
    assert!(t.key("Escape").is_empty());
    assert_eq!(t.st.sessions_dialog().map(|d| &d.page), Some(&SPage::Main));
    assert!(!t.st.overlay.takes_text(), "the lists take keys, not text");
    assert!(t.key("Escape").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
}
