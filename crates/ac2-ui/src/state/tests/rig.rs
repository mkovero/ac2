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

/// The Art-Net rows: the address and the universe go out together as one `artnet.target`
/// on the rev the page shows, Enter on either row; a bad address or universe says why under
/// its row and sends nothing; an empty address turns sending off.
#[test]
fn the_rig_page_sends_the_artnet_target_on_the_rev_it_shows() {
    let mut t = T::new();
    t.conn(mirror(with_rig(7)));
    t.st.update(Msg::OpenRig, &t.keys);
    let focus = |t: &mut T, row| {
        t.st.update(Msg::Settings(SettingsMsg::RigFocus(row)), &t.keys);
    };
    focus(&mut t, RigRow::ArtnetTarget);
    t.text("node.local");
    assert!(call(&t.key("Enter")).is_none());
    assert!(
        rig_page(&t)
            .error
            .as_deref()
            .expect("a refusal")
            .contains("IPv4 address")
    );
    for _ in 0.."node.local".len() {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("2.255.255.255");
    // No universe yet: Enter says to type one.
    assert!(call(&t.key("Enter")).is_none());
    assert_eq!(
        rig_page(&t).error.as_deref(),
        Some("type the universe too, like 0.0.1 or 1")
    );
    t.key("ArrowDown");
    assert_eq!(rig_page(&t).focus, RigRow::Universe);
    assert_eq!(rig_page(&t).error, None, "the refusal stays with its row");
    t.text("0.16.0");
    assert!(call(&t.key("Enter")).is_none());
    assert_eq!(
        rig_page(&t).error.as_deref(),
        Some("\"0.16.0\": net is 0–127, sub-net 0–15, universe 0–15")
    );
    // The caret stays where it was: edited in place, the "16" becomes "0" and the last 0
    // a 1.
    t.key("ArrowLeft");
    t.key("ArrowLeft");
    t.st.update(Msg::Backspace, &t.keys);
    t.st.update(Msg::Backspace, &t.keys);
    t.text("0");
    t.key("End");
    t.st.update(Msg::Backspace, &t.keys);
    t.text("1");
    assert_eq!(rig_page(&t).universe, "0.0.1");
    let r = t.key("Enter");
    let (cmd, what) = call(&r).expect("artnet.target");
    assert_eq!(
        cmd,
        &Command::ArtnetTarget {
            base: Rev(7),
            target: Some(ac2_proto::model::ArtnetTarget {
                addr: "2.255.255.255:6454".parse().expect("test value"),
                universe: ac2_proto::model::PortAddress::new(1).expect("test value"),
            }),
        }
    );
    assert_eq!(what, "Art-Net target 2.255.255.255:6454 universe 0.0.1 (1)");
    // The mirror brings it: the rows show the rig's target, the universe as net.sub.uni.
    let mut s = with_rig(8);
    s.rig.artnet_target =
        ac2_scene::artnet::parse_target("2.255.255.255", "1").expect("test value");
    t.conn(mirror(s));
    let p = rig_page(&t);
    assert_eq!(
        (p.artnet.as_str(), p.universe.as_str(), p.rev),
        ("2.255.255.255:6454", "0.0.1", Rev(8))
    );
    // Emptied: off, whatever the universe says.
    focus(&mut t, RigRow::ArtnetTarget);
    for _ in 0.."2.255.255.255:6454".len() {
        t.st.update(Msg::Backspace, &t.keys);
    }
    let r = t.key("Enter");
    assert_eq!(
        call(&r),
        Some((
            &Command::ArtnetTarget {
                base: Rev(8),
                target: None,
            },
            "Art-Net target off"
        ))
    );
}

/// Send DMX test: the typed levels go out as `artnet.test`; with no target, or levels that
/// do not read, Enter says why and sends nothing.
#[test]
fn the_rig_page_sends_a_dmx_test() {
    let mut t = T::new();
    t.conn(mirror(with_rig(7)));
    t.st.update(Msg::OpenRig, &t.keys);
    t.st.update(
        Msg::Settings(SettingsMsg::RigFocus(RigRow::DmxTest)),
        &t.keys,
    );
    assert_eq!(rig_page(&t).dmx_test, "1=255");
    assert!(call(&t.key("Enter")).is_none());
    assert_eq!(
        rig_page(&t).error.as_deref(),
        Some("no Art-Net target: set one above first")
    );
    let mut s = with_rig(7);
    s.rig.artnet_target = ac2_scene::artnet::parse_target("10.0.0.40", "0").expect("test value");
    t.conn(mirror(s));
    let r = t.key("Enter");
    let Some((Command::ArtnetTest { channels }, what)) = call(&r) else {
        panic!("{r:?}")
    };
    assert_eq!(ac2_proto::model::dmx_list_text(channels), "1=255");
    assert_eq!(what, "DMX test 1=255 sent");
    // The caret at the end of the levels: typing adds to them.
    t.text(" 2=300");
    assert_eq!(rig_page(&t).dmx_test, "1=255 2=300");
    assert!(call(&t.key("Enter")).is_none());
    assert_eq!(
        rig_page(&t).error.as_deref(),
        Some("\"2=300\": the value is a number 0–255")
    );
}

/// Another client changed the rig settings while this one typed an Art-Net target: the
/// stale edit is refused, both typed rows stay on the rig's newer rev with the line saying
/// Enter again overwrites.
#[test]
fn a_conflict_keeps_the_typed_artnet_target() {
    let mut t = T::new();
    t.conn(mirror(with_rig(3)));
    t.st.update(Msg::OpenRig, &t.keys);
    t.st.update(
        Msg::Settings(SettingsMsg::RigFocus(RigRow::ArtnetTarget)),
        &t.keys,
    );
    t.text("10.0.0.40:6455");
    t.key("ArrowDown");
    t.text("17");
    let mut s = with_rig(4);
    s.rig.artnet_target = ac2_scene::artnet::parse_target("10.0.0.9", "2").expect("test value");
    t.conn(mirror(s.clone()));
    assert_eq!(rig_page(&t).universe, "17", "typed: the mirror leaves it");
    let r = t.key("Enter");
    let Some((Command::ArtnetTarget { base, .. }, what)) = call(&r) else {
        panic!("{r:?}")
    };
    assert_eq!(*base, Rev(3));
    let what = what.to_owned();
    assert_eq!(what, "Art-Net target 10.0.0.40:6455 universe 0.1.1 (17)");
    t.conn(ConnEvent::Conflict {
        what: what.clone(),
        error: "conflict: stale base".into(),
    });
    assert_eq!(
        t.last_toast(),
        format!("{what}: changed elsewhere — not applied, your edits kept")
    );
    let p = rig_page(&t);
    assert_eq!(
        (p.artnet.as_str(), p.universe.as_str(), p.rev),
        ("10.0.0.40:6455", "17", Rev(4))
    );
    assert_eq!(
        p.notice.as_deref(),
        Some("changed elsewhere: Enter again overwrites with your edit")
    );
    s.rig.rev = Rev(5);
    t.conn(mirror(s));
    assert_eq!(rig_page(&t).artnet, "10.0.0.40:6455");
    let r = t.key("Enter");
    assert!(matches!(
        call(&r),
        Some((
            Command::ArtnetTarget {
                base: Rev(5),
                target: Some(_)
            },
            _
        ))
    ));
}

/// The fade row: the rig's fade shown in seconds, typed over and sent as `artnet.fade` on
/// the rev the page shows; a value out of range says why; a conflict keeps the typed one;
/// emptied, the fade is off (0 s).
#[test]
fn the_rig_page_sets_the_artnet_fade() {
    let mut t = T::new();
    t.conn(mirror(with_rig(7)));
    t.st.update(Msg::OpenRig, &t.keys);
    assert_eq!(rig_page(&t).fade, "1", "the default a rig starts with");
    t.st.update(
        Msg::Settings(SettingsMsg::RigFocus(RigRow::Universe)),
        &t.keys,
    );
    t.key("ArrowDown");
    assert_eq!(rig_page(&t).focus, RigRow::Fade);
    // The caret at the end of the rig's text: typing adds to it.
    t.text("5");
    assert_eq!(rig_page(&t).fade, "15");
    t.text("0");
    assert!(call(&t.key("Enter")).is_none());
    assert_eq!(
        rig_page(&t).error.as_deref(),
        Some("Art-Net fade 150 s is outside 0–60 s")
    );
    for _ in 0..3 {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("0.5");
    let r = t.key("Enter");
    assert_eq!(
        call(&r),
        Some((
            &Command::ArtnetFade {
                base: Rev(7),
                seconds: Seconds(0.5),
            },
            "Art-Net fade 0.5 s"
        ))
    );
    let mut s = with_rig(8);
    s.rig.artnet_fade = ac2_proto::model::ArtnetFade::new(0.5).expect("test value");
    t.conn(mirror(s.clone()));
    assert_eq!(
        (rig_page(&t).fade.as_str(), rig_page(&t).rev),
        ("0.5", Rev(8))
    );
    // Emptied: off. Refused as stale, the empty row stays on the newer rev.
    for _ in 0..3 {
        t.st.update(Msg::Backspace, &t.keys);
    }
    let r = t.key("Enter");
    let Some((cmd, what)) = call(&r) else {
        panic!("{r:?}")
    };
    assert_eq!(
        cmd,
        &Command::ArtnetFade {
            base: Rev(8),
            seconds: Seconds(0.0),
        }
    );
    let what = what.to_owned();
    assert_eq!(what, "Art-Net fade off: cue changes jump");
    s.rig.rev = Rev(9);
    t.conn(mirror(s));
    t.conn(ConnEvent::Conflict {
        what: what.clone(),
        error: "conflict: stale base".into(),
    });
    let p = rig_page(&t);
    assert_eq!((p.fade.as_str(), p.rev), ("", Rev(9)));
    assert!(p.notice.is_some());
}

/// A cue row's DMX line: ↓ from a state's message reaches its levels; levels that do not
/// read refuse Enter naming the subject; typed levels go out in `meas.update` on the rev
/// the editor opened on, beside the message of the same state.
#[test]
fn the_limit_editor_sends_dmx_cue_levels() {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    t.type_key("Shift+L", "L");
    let opened = t.st.overlay.leq().expect("test value").base_rev;
    t.st.update(Msg::Leq(LeqMsg::Focus(Focus::CueFold)), &t.keys);
    t.key("ArrowRight");
    t.key("ArrowDown");
    t.text("/light/red 1");
    t.key("ArrowDown");
    assert_eq!(
        t.st.overlay.leq().expect("test value").focus,
        Focus::Cue {
            row: CueRow::Meter,
            col: CueCol::Dmx(CueState::Over),
        }
    );
    assert_eq!(
        t.st.overlay.leq().expect("test value").cue_fold_text(),
        "shown · 1 message set"
    );
    t.text("12=255 13=300");
    t.key("Enter");
    assert_eq!(
        t.st.overlay.leq().expect("still open").error.as_deref(),
        Some("DMX cue of the meter: \"13=300\": the value is a number 0–255")
    );
    for _ in 0.."300".len() {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("0");
    // Tab: the next state's levels.
    t.key("Tab");
    assert_eq!(
        t.st.overlay.leq().expect("test value").focus,
        Focus::Cue {
            row: CueRow::Meter,
            col: CueCol::Dmx(CueState::Near),
        }
    );
    t.text("14=40");
    assert_eq!(
        t.st.overlay.leq().expect("test value").cue_fold_text(),
        "shown · 1 message set · DMX in 2 states"
    );
    let r = t.key("Enter");
    let Some((Command::MeasUpdate { base, config, .. }, _)) = call(&r) else {
        panic!("{r:?}")
    };
    assert_eq!(*base, opened);
    let MeasKind::Spl { config } = &config.kind else {
        panic!()
    };
    let cue = config.cue.as_ref().expect("meter cue");
    let over = cue.over.as_ref().expect("over");
    assert_eq!(
        over.osc.as_ref().map(|m| m.path.as_str()),
        Some("/light/red")
    );
    assert_eq!(ac2_proto::model::dmx_list_text(&over.dmx), "12=255 13=0");
    let near = cue.near.as_ref().expect("near");
    assert_eq!(near.osc, None);
    assert_eq!(ac2_proto::model::dmx_list_text(&near.dmx), "14=40");
    assert!(cue.ok.is_none() && cue.fault.is_none());
}

/// The cue fold is the operator's for the app's run: unfolded, it stays shown after Apply
/// and when the page opens again; folded again, a meter without a cue opens folded.
#[test]
fn the_cue_fold_survives_apply_and_reopening() {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    t.type_key("Shift+L", "L");
    let shown = |t: &T| t.st.overlay.leq().expect("open").cues_shown;
    assert!(!shown(&t), "no cue set: folded");
    t.st.update(Msg::Leq(LeqMsg::Focus(Focus::CueFold)), &t.keys);
    t.key("ArrowRight");
    assert!(shown(&t));
    let r = t.key("Enter");
    assert!(matches!(call(&r), Some((Command::MeasUpdate { .. }, _))));
    assert!(t.st.overlay.leq().is_none(), "applied: the page closes");
    t.type_key("Shift+L", "L");
    assert!(shown(&t), "the unfold is kept across Apply");
    t.st.update(Msg::Leq(LeqMsg::Focus(Focus::CueFold)), &t.keys);
    t.key("ArrowLeft");
    t.key("Escape");
    assert!(t.st.overlay.leq().is_none(), "closed");
    t.type_key("Shift+L", "L");
    assert!(!shown(&t), "folded by the operator: stays folded");
}

/// A meter with any cue set opens its cues shown, whatever the fold last chosen.
#[test]
fn a_meter_with_a_cue_opens_its_cues_shown() {
    use ac2_proto::model::{Cue, CueAction, DmxChannel, DmxSet};
    let mut s = with_spl();
    let MeasKind::Spl { config } = &mut s.measurements[2].config.kind else {
        panic!()
    };
    config.cue = Some(Box::new(Cue {
        over: Some(CueAction {
            osc: None,
            dmx: vec![DmxSet {
                channel: DmxChannel::new(12).expect("test value"),
                value: 255,
            }],
        }),
        ..Cue::default()
    }));
    let mut t = T::new();
    t.conn(mirror(s));
    t.type_key("Shift+L", "L");
    let d = t.st.overlay.leq().expect("open");
    assert!(d.cues_shown);
    assert_eq!(d.cue_fold_text(), "shown · no message set · DMX in 1 state");
}

/// A cue's DMX line is edited in place: ←/→ move the caret, Backspace and Delete remove at
/// it, typing inserts at it, Home/End go to the ends; coming back to the line puts the
/// caret at its end and the first key adds to it. The edited line goes out in
/// `meas.update`.
#[test]
fn a_dmx_cue_line_is_edited_at_its_caret() {
    use crate::caret::Shown;
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    t.type_key("Shift+L", "L");
    t.st.update(Msg::Leq(LeqMsg::Focus(Focus::CueFold)), &t.keys);
    t.key("ArrowRight");
    t.key("ArrowDown");
    t.key("ArrowDown");
    let over = Focus::Cue {
        row: CueRow::Meter,
        col: CueCol::Dmx(CueState::Over),
    };
    assert_eq!(t.st.overlay.leq().expect("open").focus, over);
    t.text("12=255 13=0 14=40");
    let shown = |t: &T| {
        let d = t.st.overlay.leq().expect("open");
        Shown::new(d.caret_text().expect("a cue line"), d.caret, 40).text
    };
    assert_eq!(shown(&t), "12=255 13=0 14=40▏");
    for _ in 0.." 14=40".len() {
        t.key("ArrowLeft");
    }
    assert_eq!(shown(&t), "12=255 13=0▏ 14=40");
    t.st.update(Msg::Backspace, &t.keys);
    t.text("128");
    assert_eq!(shown(&t), "12=255 13=128▏ 14=40");
    t.key("Home");
    t.key("Delete");
    t.text("1");
    assert_eq!(shown(&t), "1▏2=255 13=128 14=40");
    // Away and back: the caret at the end, the line kept, the first key adds to it.
    t.key("Tab");
    t.key("Shift+Tab");
    assert_eq!(t.st.overlay.leq().expect("open").focus, over);
    assert_eq!(shown(&t), "12=255 13=128 14=40▏");
    t.text(" 15=9");
    let r = t.key("Enter");
    let Some((Command::MeasUpdate { config, .. }, _)) = call(&r) else {
        panic!("{r:?}")
    };
    let MeasKind::Spl { config } = &config.kind else {
        panic!()
    };
    let cue = config.cue.as_ref().expect("meter cue");
    assert_eq!(
        ac2_proto::model::dmx_list_text(&cue.over.as_ref().expect("over").dmx),
        "12=255 13=128 14=40 15=9"
    );
}

/// Tab walks the Rig page's rows, Shift+Tab back.
#[test]
fn shift_tab_steps_back_on_the_rig_page() {
    let mut t = T::new();
    t.conn(mirror(with_rig(7)));
    t.st.update(Msg::OpenRig, &t.keys);
    assert_eq!(rig_page(&t).focus, RigRow::Target);
    t.key("Tab");
    assert_ne!(rig_page(&t).focus, RigRow::Target);
    t.key("Shift+Tab");
    assert_eq!(rig_page(&t).focus, RigRow::Target);
}

/// The programme-meter rows step the rig's value with ←/→ and send each step as a
/// `levels.set` of that value alone, on the rev the page shows; a range's end sends nothing.
#[test]
fn the_rig_page_steps_the_programme_meters() {
    use ac2_proto::model::{Ballistic, CorrelationWindow};
    use ac2_proto::units::{Dbfs, Dbtp};
    let mut t = T::new();
    t.conn(mirror(with_rig(7)));
    t.st.update(Msg::OpenRig, &t.keys);
    t.st.update(
        Msg::Settings(SettingsMsg::RigFocus(RigRow::Ballistic)),
        &t.keys,
    );
    let set =
        |ballistic, correlation_window, over_threshold, correlation_gate| Command::LevelsSet {
            base: Rev(7),
            ballistic,
            correlation_window,
            over_threshold,
            correlation_gate,
            loudness_target: None,
        };
    let r = t.key("ArrowRight");
    assert_eq!(
        call(&r),
        Some((
            &set(Some(Ballistic::Type1), None, None, None),
            "programme meters read Type I"
        ))
    );
    assert!(
        call(&t.key("ArrowLeft")).is_none(),
        "true peak is the first"
    );
    // Past the loudness target (its own test) to the correlation window.
    t.key("ArrowDown");
    assert_eq!(rig_page(&t).focus, RigRow::LoudnessTarget);
    t.key("ArrowDown");
    let r = t.key("ArrowLeft");
    assert_eq!(
        call(&r),
        Some((
            &set(None, Some(CorrelationWindow::Short), None, None),
            "correlation window 0.3 s"
        ))
    );
    t.key("ArrowDown");
    let r = t.key("ArrowLeft");
    assert_eq!(
        call(&r),
        Some((
            &set(None, None, Some(Dbtp(-0.5)), None),
            "an over is above −0.5 dBTP"
        ))
    );
    t.key("ArrowDown");
    let r = t.key("ArrowRight");
    assert_eq!(
        call(&r),
        Some((
            &set(None, None, None, Some(Dbfs(-55.0))),
            "correlation gate −55 dBFS"
        ))
    );
}

/// The loudness target steps 1 LU with ←/→ and sends each step as a `levels.set` of the
/// target alone; past either end of the rig's range it sends nothing and the row says why.
#[test]
fn the_rig_page_steps_the_loudness_target() {
    use ac2_proto::units::Lufs;
    let mut t = T::new();
    let mut s = with_rig(7);
    s.rig.levels.loudness_target = Lufs(-36.0);
    t.conn(mirror(s));
    t.st.update(Msg::OpenRig, &t.keys);
    t.st.update(
        Msg::Settings(SettingsMsg::RigFocus(RigRow::LoudnessTarget)),
        &t.keys,
    );
    assert_eq!(RigRow::LoudnessTarget.title(), "Loudness target");
    let rig = t.st.daemon().expect("state").rig.clone();
    assert_eq!(
        crate::settings::RigPage::levels_text(&rig, RigRow::LoudnessTarget),
        "\u{2212}36 LUFS"
    );
    let r = t.key("ArrowRight");
    assert_eq!(
        call(&r),
        Some((
            &Command::LevelsSet {
                base: Rev(7),
                ballistic: None,
                correlation_window: None,
                over_threshold: None,
                correlation_gate: None,
                loudness_target: Some(Lufs(-35.0)),
            },
            "loudness target \u{2212}35 LUFS"
        ))
    );
    assert_eq!(rig_page(&t).error, None);
    let r = t.key("ArrowLeft");
    assert!(call(&r).is_none(), "−37 is past the range: {r:?}");
    assert_eq!(
        rig_page(&t).error.as_deref(),
        Some("loudness target -37 LUFS is outside -36 … -5 LUFS")
    );
}
