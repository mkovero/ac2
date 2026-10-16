//! Reducer tests of the rig monitor: the Rig monitor settings page (target, monitor at
//! start, a test message) and the revs its edits name, the SPL limit editor's scope toggle
//! and cue rows, the Rig group in the tree, and the toast when another client changed the
//! settings first.

use super::*;
use crate::leq_dialog::{CueCol, CueRow, Focus};
use crate::settings::{Page, RigRow};
use crate::state::LeqMsg;
use ac2_proto::model::{CueState, CueSubject, MeasScope, OscArg, RigCue};
use ac2_scene::meas_list::TreeKey;

fn with_rig(rev: u64) -> State {
    let mut s = with_spl();
    s.rig.rev = Rev(rev);
    s
}

fn call(r: &[Request]) -> Option<(&Command, &str)> {
    r.iter().find_map(|r| match r {
        Request::Call { cmd, what } => Some((cmd, what.as_str())),
        _ => None,
    })
}

fn rig_page(t: &T) -> &crate::settings::RigPage {
    &t.st.overlay.settings().expect("Settings open").rig
}

#[test]
fn the_rig_page_sends_its_edits_on_the_rev_it_shows() {
    let mut t = T::new();
    t.conn(mirror(with_rig(7)));
    t.st.update(Msg::OpenRig, &t.keys);
    assert_eq!(t.st.overlay.settings().map(|s| s.page), Some(Page::Rig));
    assert_eq!(rig_page(&t).focus, RigRow::Target);
    // A bad target says why and sends nothing.
    t.text("lights.local:9000");
    assert!(call(&t.key("Enter")).is_none());
    assert!(
        rig_page(&t)
            .error
            .as_deref()
            .expect("test value")
            .contains("IP address and port")
    );
    // Ctrl+A is not needed: Backspace clears what was typed.
    for _ in 0.."lights.local:9000".len() {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("10.0.0.5:9000");
    let r = t.key("Enter");
    let (cmd, what) = call(&r).expect("osc.target");
    assert_eq!(
        cmd,
        &Command::OscTarget {
            base: Rev(7),
            addr: Some("10.0.0.5:9000".parse().expect("test value")),
        }
    );
    assert_eq!(what, "OSC target 10.0.0.5:9000");
    // Monitor at start: → turns it on, on the same rev.
    t.key("ArrowDown");
    let r = t.key("ArrowRight");
    assert_eq!(
        call(&r).map(|c| c.0),
        Some(&Command::RigMonitorStart {
            base: Rev(7),
            on: true,
        })
    );
    // ← on an off setting sends nothing.
    assert!(call(&t.key("ArrowLeft")).is_none());
    // The test message: the default, sent as typed.
    t.key("ArrowDown");
    let r = t.key("Enter");
    let Some((Command::OscTest { msg }, what)) = call(&r) else {
        panic!("{r:?}")
    };
    assert_eq!(msg.path.as_str(), "/ac2/test");
    assert_eq!(msg.args, vec![OscArg::Int(1)]);
    assert_eq!(what, "OSC test /ac2/test 1 sent");
    // The mirror moves on: the page follows the rig's rev and target.
    let mut s = with_rig(8);
    s.rig.osc_target = Some("10.0.0.6:9000".parse().expect("test value"));
    t.conn(mirror(s));
    assert_eq!(rig_page(&t).rev, Rev(8));
    assert_eq!(rig_page(&t).target, "10.0.0.6:9000");
}

/// Another client changed the rig settings while this one typed a target: the daemon
/// refuses the stale edit, the page keeps the typed target on the rig's newer rev with a
/// line saying Enter again overwrites, and a toast says so.
#[test]
fn a_conflict_keeps_the_typed_target_on_the_newer_rev() {
    let mut t = T::new();
    t.conn(mirror(with_rig(3)));
    t.st.update(Msg::OpenRig, &t.keys);
    t.text("10.0.0.5:9000");
    // Typed: a mirror change leaves the edit (and its base) alone.
    let mut s = with_rig(4);
    s.rig.osc_target = Some("10.0.0.9:9000".parse().expect("test value"));
    t.conn(mirror(s.clone()));
    assert_eq!(rig_page(&t).target, "10.0.0.5:9000");
    let r = t.key("Enter");
    assert!(matches!(
        call(&r),
        Some((Command::OscTarget { base: Rev(3), .. }, _))
    ));
    t.conn(ConnEvent::Conflict {
        what: "OSC target 10.0.0.5:9000".into(),
        error: "conflict: stale base".into(),
    });
    assert_eq!(
        t.last_toast(),
        "OSC target 10.0.0.5:9000: changed elsewhere — not applied, your edits kept"
    );
    assert_eq!(rig_page(&t).target, "10.0.0.5:9000");
    assert_eq!(rig_page(&t).rev, Rev(4));
    assert_eq!(
        rig_page(&t).notice.as_deref(),
        Some("changed elsewhere: Enter again overwrites with your edit")
    );
    // Another mirror change while the kept target waits: still the operator's text, on
    // the newest rev.
    s.rig.rev = Rev(5);
    t.conn(mirror(s));
    assert_eq!(rig_page(&t).target, "10.0.0.5:9000");
    let r = t.key("Enter");
    assert!(matches!(
        call(&r),
        Some((Command::OscTarget { base: Rev(5), .. }, _))
    ));
    assert_eq!(rig_page(&t).notice, None);
}

/// A conflict on a toggle the page holds nothing typed for: the page shows the rig's
/// settings again.
#[test]
fn a_conflict_on_monitor_at_start_reloads_the_page() {
    let mut t = T::new();
    t.conn(mirror(with_rig(3)));
    t.st.update(Msg::OpenRig, &t.keys);
    t.key("ArrowDown");
    assert!(call(&t.key("ArrowRight")).is_some());
    t.conn(ConnEvent::Conflict {
        what: "monitor at start on".into(),
        error: "conflict: stale base".into(),
    });
    assert_eq!(
        t.last_toast(),
        "monitor at start on: changed elsewhere — reloaded"
    );
    assert_eq!(rig_page(&t).notice, None);
}

/// Another client changed the meter while the SPL / Leq page was applied: the page comes
/// back with the operator's edits on the meter's newer rev, a line says Apply again
/// overwrites, and the second Apply goes through on that rev.
#[test]
fn a_conflict_on_apply_keeps_the_leq_page_and_its_edits() {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    t.type_key("Shift+L", "L");
    let opened = t.st.overlay.leq().expect("test value").base_rev;
    t.st.update(Msg::Leq(LeqMsg::Focus(Focus::Scope)), &t.keys);
    t.key("ArrowRight");
    assert!(t.st.overlay.leq().expect("test value").on_rig);
    // Elsewhere: the meter's configuration moves on.
    let mut s = with_spl();
    s.measurements[2].config_rev = Rev(opened.0 + 5);
    t.conn(mirror(s.clone()));
    let r = t.key("Enter");
    let Some((Command::MeasUpdate { base, .. }, what)) = call(&r) else {
        panic!("{r:?}")
    };
    assert_eq!(*base, opened);
    let what = what.to_owned();
    assert!(t.st.overlay.leq().is_none(), "applied: the page closes");
    t.conn(ConnEvent::Conflict {
        what: what.clone(),
        error: "conflict: stale base".into(),
    });
    assert_eq!(
        t.last_toast(),
        format!("{what}: changed elsewhere — not applied, your edits kept")
    );
    let d = t.st.overlay.leq().expect("the page is back");
    assert!(d.on_rig, "the edit is kept");
    assert_eq!(d.base_rev, Rev(opened.0 + 5));
    assert_eq!(
        d.notice.as_deref(),
        Some("changed elsewhere: Apply again overwrites with your edits")
    );
    // A later change elsewhere: the kept page follows its rev.
    s.measurements[2].config_rev = Rev(opened.0 + 6);
    t.conn(mirror(s));
    let r = t.key("Enter");
    let Some((Command::MeasUpdate { base, config, .. }, _)) = call(&r) else {
        panic!("{r:?}")
    };
    assert_eq!(*base, Rev(opened.0 + 6));
    assert_eq!(config.scope, MeasScope::Rig);
    assert!(t.st.overlay.leq().is_none());
}

/// A reply's warnings toast with the rig meters' names, even with the warning toasts off.
#[test]
fn reply_warnings_toast_with_meter_names() {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    t.st.prefs.warning_toasts = false;
    let name =
        t.st.meas(MeasId(4))
            .expect("test value")
            .config
            .name
            .clone();
    t.conn(ConnEvent::Warnings(vec![
        ac2_proto::Warning::RigMonitorBlind {
            meas: vec![MeasId(4), MeasId(99)],
        },
    ]));
    assert_eq!(
        t.last_toast(),
        format!(
            "rig SPL meter(s) {name}, 99 stop metering until a session opens again; their \
             OSC cues report fault"
        )
    );
    t.conn(ConnEvent::Warnings(vec![
        ac2_proto::Warning::RigMonitorMoved {
            meas: vec![MeasId(4)],
        },
    ]));
    assert_eq!(
        t.last_toast(),
        format!("rig SPL meter(s) {name} now meter this device's inputs")
    );
}

/// The limit editor moves the meter onto the rig and sets its meter-wide cue; Enter names
/// the configuration rev the editor opened on, not one that arrived meanwhile.
#[test]
fn the_limit_editor_sets_scope_and_cues_on_its_opening_rev() {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    t.type_key("Shift+L", "L");
    let opened = t.st.overlay.leq().expect("test value").base_rev;
    let leq = |t: &mut T, m: LeqMsg| {
        t.st.update(Msg::Leq(m), &t.keys);
    };
    leq(&mut t, LeqMsg::Focus(Focus::Scope));
    t.key("ArrowRight");
    assert!(t.st.overlay.leq().expect("test value").on_rig);
    t.key("ArrowDown");
    assert_eq!(
        t.st.overlay.leq().expect("test value").focus,
        Focus::CueFold
    );
    t.key("ArrowRight");
    t.key("ArrowDown");
    assert_eq!(
        t.st.overlay.leq().expect("test value").focus,
        Focus::Cue {
            row: CueRow::Meter,
            col: CueCol::State(CueState::Over),
        }
    );
    t.text("/light/red 1");
    // A bad line refuses Enter and names the subject.
    t.key("Tab");
    t.text("amber");
    t.key("Enter");
    let d = t.st.overlay.leq().expect("still open");
    assert_eq!(
        d.error.as_deref(),
        Some("OSC cue of the meter: an OSC address starts with '/'")
    );
    for _ in 0.."amber".len() {
        t.st.update(Msg::Backspace, &t.keys);
    }
    // The meter's config moves on elsewhere meanwhile.
    let mut s = with_spl();
    s.measurements[2].config_rev = Rev(opened.0 + 5);
    t.conn(mirror(s));
    let r = t.key("Enter");
    let Some((Command::MeasUpdate { meas, base, config }, _)) = call(&r) else {
        panic!("{r:?}")
    };
    assert_eq!(*meas, MeasId(4));
    assert_eq!(*base, opened);
    assert_eq!(config.scope, MeasScope::Rig);
    let MeasKind::Spl { config } = &config.kind else {
        panic!()
    };
    let cue = config.cue.as_ref().expect("meter cue");
    let over = cue.osc(CueState::Over).expect("test value");
    assert_eq!(over.path.as_str(), "/light/red");
    assert_eq!(over.args, vec![OscArg::Int(1)]);
    assert!(cue.near.is_none());
}

/// A rig meter leads the tree under the Rig header with its cue chip; the header opens
/// the Rig monitor page.
#[test]
fn the_tree_lists_rig_meters_first_with_their_chip() {
    let mut t = T::new();
    let mut s = with_spl();
    s.measurements[2].config.scope = MeasScope::Rig;
    s.rig.cues.push(RigCue {
        meas: MeasId(4),
        subject: CueSubject::Meter,
        state: CueState::Near,
    });
    t.conn(mirror(s));
    let rows = t.st.tree_rows();
    let heads: Vec<&str> = rows
        .iter()
        .filter(|r| r.depth == 0)
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(heads[..3], ["Rig", "SPL  FOH SPL", "Session"]);
    let foh = rows
        .iter()
        .find(|r| r.key == TreeKey::Meas(MeasId(4)))
        .expect("test value");
    assert_eq!(foh.chip, Some(CueState::Near));
    t.st.update(Msg::OpenRig, &t.keys);
    assert_eq!(t.st.overlay.settings().map(|s| s.page), Some(Page::Rig));
}
