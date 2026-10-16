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
/// refuses the stale edit, a toast says so and the page shows the rig's again.
#[test]
fn a_conflict_reloads_the_page_and_says_so() {
    let mut t = T::new();
    t.conn(mirror(with_rig(3)));
    t.st.update(Msg::OpenRig, &t.keys);
    t.text("10.0.0.5:9000");
    // Typed: a mirror change leaves the edit (and its base) alone.
    let mut s = with_rig(4);
    s.rig.osc_target = Some("10.0.0.9:9000".parse().expect("test value"));
    t.conn(mirror(s));
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
        "OSC target 10.0.0.5:9000: changed elsewhere — reloaded"
    );
    assert_eq!(rig_page(&t).target, "10.0.0.9:9000");
    assert_eq!(rig_page(&t).rev, Rev(4));
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
    assert_eq!(
        cue.over.as_ref().expect("test value").path.as_str(),
        "/light/red"
    );
    assert_eq!(
        cue.over.as_ref().expect("test value").args,
        vec![OscArg::Int(1)]
    );
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
