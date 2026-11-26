//! Reducer tests of the Rig monitor page's sACN rows: the target typed over three rows and
//! sent together on the rev the page shows, a conflict keeping what was typed, and the test
//! frame going out with only sACN set.

use super::*;
use crate::settings::RigRow;
use ac2_proto::model::{SacnDest, SacnPriority, SacnTarget, SacnUniverse};

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

fn erase(t: &mut T, n: usize) {
    for _ in 0..n {
        t.st.apply(Msg::Backspace, &t.keys);
    }
}

#[test]
fn the_rig_page_sends_the_sacn_target_from_its_three_rows() {
    let mut t = T::new();
    t.conn(mirror(with_rig(7)));
    t.st.apply(Msg::OpenRig, &t.keys);
    t.st.apply(
        Msg::Settings(SettingsMsg::RigFocus(RigRow::Universe)),
        &t.keys,
    );
    t.key("ArrowDown");
    assert_eq!(rig_page(&t).focus, RigRow::SacnTarget);
    assert_eq!(RigRow::SacnTarget.title(), "sACN target");
    t.text("multicast");
    assert!(call(&t.key("Enter")).is_none());
    assert!(
        rig_page(&t)
            .error
            .as_deref()
            .expect("a refusal")
            .starts_with("type the interface too")
    );
    t.text(" via 10.0.0.4");
    assert!(call(&t.key("Enter")).is_none());
    assert_eq!(
        rig_page(&t).error.as_deref(),
        Some("type the sACN universe too, a number 1–63999")
    );
    t.key("ArrowDown");
    t.text("258");
    t.key("ArrowDown");
    assert_eq!(rig_page(&t).focus, RigRow::SacnPriority);
    // Priority left empty: E1.31's 100.
    let r = t.key("Enter");
    let target = SacnTarget {
        dest: SacnDest::Multicast {
            interface: "10.0.0.4".parse().expect("test value"),
        },
        universe: SacnUniverse::new(258).expect("test value"),
        priority: SacnPriority::DEFAULT,
    };
    assert_eq!(
        call(&r),
        Some((
            &Command::SacnTarget {
                base: Rev(7),
                target: Some(target),
            },
            "sACN to 239.255.1.2 (multicast via 10.0.0.4) universe 258 priority 100"
        ))
    );
    // The mirror brings it: the rows show the rig's target, the priority filled in.
    let mut s = with_rig(8);
    s.rig.sacn_target = Some(target);
    t.conn(mirror(s));
    let p = rig_page(&t);
    assert_eq!(
        (
            p.sacn.as_str(),
            p.sacn_universe.as_str(),
            p.sacn_priority.as_str(),
            p.rev
        ),
        ("multicast via 10.0.0.4", "258", "100", Rev(8))
    );
    // Emptied: off.
    t.st.apply(
        Msg::Settings(SettingsMsg::RigFocus(RigRow::SacnTarget)),
        &t.keys,
    );
    erase(&mut t, "multicast via 10.0.0.4".len());
    assert_eq!(
        call(&t.key("Enter")),
        Some((
            &Command::SacnTarget {
                base: Rev(8),
                target: None,
            },
            "sACN off"
        ))
    );
}

#[test]
fn a_conflict_keeps_the_typed_sacn_target_and_a_test_needs_one_output() {
    let mut t = T::new();
    t.conn(mirror(with_rig(3)));
    t.st.apply(Msg::OpenRig, &t.keys);
    t.st.apply(
        Msg::Settings(SettingsMsg::RigFocus(RigRow::SacnTarget)),
        &t.keys,
    );
    t.text("192.168.1.40");
    t.key("ArrowDown");
    t.text("9");
    let r = t.key("Enter");
    let what = call(&r).expect("sacn.target").1.to_owned();
    assert_eq!(what, "sACN to 192.168.1.40 universe 9 priority 100");
    let mut s = with_rig(4);
    s.rig.sacn_target = ac2_scene::sacn::parse_target("10.0.0.9", "2", "").expect("test value");
    t.conn(mirror(s.clone()));
    t.conn(ConnEvent::Conflict {
        what,
        error: "conflict: stale base".into(),
    });
    let p = rig_page(&t);
    assert_eq!(
        (p.sacn.as_str(), p.sacn_universe.as_str(), p.rev),
        ("192.168.1.40", "9", Rev(4))
    );
    // With only sACN set, the test frame goes.
    t.st.apply(
        Msg::Settings(SettingsMsg::RigFocus(RigRow::DmxTest)),
        &t.keys,
    );
    let Some((Command::ArtnetTest { .. }, what)) =
        call(&t.key("Enter")).map(|(c, w)| (c.clone(), w.to_owned()))
    else {
        panic!("artnet.test")
    };
    assert_eq!(what, "DMX test 1=255 sent");
}
