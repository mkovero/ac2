//! Reducer tests of the Settings view while the mirror changes every second, as it does
//! while the rig meters (SPL logs, rig cues, OSC counts): no page loses its focus, its
//! typed text or its selection to an update, and only a key (or a page being shown) asks
//! the view to bring the focused row into view, so the wheel's scroll holds.

use super::*;
use crate::leq_dialog::Focus;
use crate::settings::{Page, RigRow};
use ac2_proto::model::{
    CueState, CueSubject, LeqJudgement, LeqPeakState, PeakStates, RigCue, SplLog,
};

/// The mirror one tick later: the meter's log, the rig's cue state and its sent count move.
fn tick(s: &mut State, n: u64) {
    let still = LeqPeakState {
        judgement: LeqJudgement::NoLimit,
        since: WallNs(0),
    };
    s.spl_logs = vec![SplLog {
        meas: MeasId(4),
        started_at: Some(WallNs(n)),
        windows: vec![],
        alarms: vec![],
        peaks: PeakStates {
            lcpeak: still,
            lafmax: still,
        },
    }];
    s.rig.osc.sent = n;
    s.rig.artnet.sent = n;
    s.rig.dmx = if n.is_multiple_of(2) {
        ac2_proto::model::parse_dmx_list("12=255 14=40").expect("test value")
    } else {
        vec![]
    };
    s.rig.cues = vec![RigCue {
        meas: MeasId(4),
        subject: CueSubject::Meter,
        state: if n.is_multiple_of(2) {
            CueState::Near
        } else {
            CueState::Ok
        },
    }];
}

fn settings(t: &T) -> &Settings {
    t.st.overlay.settings().expect("Settings open")
}

/// One mirror update with the view having taken its reveal: the page must come out as it
/// went in, with nothing to bring into view.
fn update_holds(t: &mut T, s: &mut State, n: u64) {
    t.st.settings_reveal = false;
    let before = settings(t).clone();
    tick(s, n);
    t.conn(mirror(s.clone()));
    assert_eq!(settings(t), &before, "update {n} changed the page");
    assert!(!t.st.settings_reveal, "update {n} asked to scroll the page");
}

/// The meter of the SPL / Leq page has its windows and the band meter's 11; ↓ walks every
/// row to the rig's, each key asking the view to bring the focus into view, and a mirror
/// update between keys changes nothing: neither the focus nor the scroll's anchor.
#[test]
fn the_leq_page_holds_its_focus_and_scroll_through_mirror_updates() {
    let mut t = T::new();
    let mut s = with_spl();
    tick(&mut s, 0);
    t.conn(mirror(s.clone()));
    t.type_key("Shift+L", "L");
    {
        let d = t.st.overlay.leq_mut().expect("the SPL / Leq page");
        // The band meter's row, then its 545 preset: 11 band windows.
        d.move_cell(-3);
        d.cycle(2);
        assert_eq!(d.bands.rows.len(), 11);
        d.focus = Focus::Preset;
    }
    assert!(
        t.st.settings_reveal,
        "the page shown brings its focus into view"
    );
    update_holds(&mut t, &mut s, 1);
    let mut n = 2;
    while t.st.overlay.leq().map(|d| d.focus) != Some(Focus::Scope) {
        assert!(n < 80, "↓ never reached the rig rows");
        t.key("ArrowDown");
        assert!(t.st.settings_reveal, "↓ brings the focused row into view");
        update_holds(&mut t, &mut s, n);
        n += 1;
    }
    // A limit typed low on the page stays through updates too.
    t.key("ArrowUp");
    t.text("9");
    update_holds(&mut t, &mut s, n);
    t.key("ArrowDown");
    t.key("ArrowDown");
    assert_eq!(t.st.overlay.leq().map(|d| d.focus), Some(Focus::CueFold));
    update_holds(&mut t, &mut s, n + 1);
}

/// The Rig monitor page while the rig meters: the test message types as keys come, mirror
/// updates between them, and Enter sends what was typed; the target the same.
#[test]
fn the_rig_page_types_through_mirror_updates() {
    let mut t = T::new();
    let mut s = with_spl();
    s.rig.osc_target = Some("10.0.0.9:9000".parse().expect("test value"));
    tick(&mut s, 0);
    t.conn(mirror(s.clone()));
    t.st.apply(Msg::OpenRig, &t.keys);
    t.key("ArrowDown");
    t.key("ArrowDown");
    assert_eq!(settings(&t).rig.focus, RigRow::Test);
    // The caret at the end of the default message: Backspace empties it, keys append.
    for _ in 0..settings(&t).rig.test.chars().count() {
        t.st.apply(Msg::Backspace, &t.keys);
    }
    for (n, c) in "/fx/go 3".chars().enumerate() {
        t.text(&c.to_string());
        update_holds(&mut t, &mut s, n as u64 + 1);
    }
    assert_eq!(settings(&t).rig.test, "/fx/go 3");
    t.st.apply(Msg::Backspace, &t.keys);
    update_holds(&mut t, &mut s, 20);
    t.text("4");
    let r = t.key("Enter");
    let Some((msg, what)) = r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::OscTest { msg },
            what,
        } => Some((msg, what)),
        _ => None,
    }) else {
        panic!("no osc.test: {r:?}")
    };
    assert_eq!(msg.path.as_str(), "/fx/go");
    assert!(what.contains("/fx/go 4"), "{what}");
    // The target: the rig's, edited in place key by key through updates (Home, then
    // Delete the old one away).
    t.key("ArrowUp");
    t.key("ArrowUp");
    assert_eq!(settings(&t).rig.focus, RigRow::Target);
    assert_eq!(settings(&t).rig.target, "10.0.0.9:9000");
    t.key("Home");
    for _ in 0.."10.0.0.9:9000".len() {
        t.key("Delete");
    }
    for (n, c) in "10.0.0.5:9001".chars().enumerate() {
        t.text(&c.to_string());
        update_holds(&mut t, &mut s, n as u64 + 30);
    }
    let r = t.key("Enter");
    assert!(
        r.iter().any(|r| matches!(
            r,
            Request::Call { cmd: Command::OscTarget { addr: Some(a), .. }, .. }
                if a.to_string() == "10.0.0.5:9001"
        )),
        "{r:?}"
    );
}

/// The Art-Net rows while the rig meters and its frame count and channels move: the
/// address, the universe and the test levels type as keys come, and Enter sends them.
#[test]
fn the_rig_page_types_artnet_through_mirror_updates() {
    let mut t = T::new();
    let mut s = with_spl();
    s.rig.artnet_target = ac2_scene::artnet::parse_target("10.0.0.9", "3").expect("test value");
    tick(&mut s, 0);
    t.conn(mirror(s.clone()));
    t.st.apply(Msg::OpenRig, &t.keys);
    let mut n = 1;
    for (row, typed) in [
        (RigRow::ArtnetTarget, "2.255.255.255"),
        (RigRow::Universe, "0.0.1"),
        (RigRow::Fade, "2.5"),
        (RigRow::DmxTest, "12=255 13=40"),
    ] {
        t.st.apply(Msg::Settings(SettingsMsg::RigFocus(row)), &t.keys);
        // The rig's text with the caret at its end: emptied, then typed key by key.
        for _ in 0..64 {
            t.st.apply(Msg::Backspace, &t.keys);
        }
        for c in typed.chars() {
            t.text(&c.to_string());
            update_holds(&mut t, &mut s, n);
            n += 1;
        }
    }
    let p = &settings(&t).rig;
    assert_eq!(
        (
            p.artnet.as_str(),
            p.universe.as_str(),
            p.fade.as_str(),
            p.dmx_test.as_str()
        ),
        ("2.255.255.255", "0.0.1", "2.5", "12=255 13=40")
    );
    let r = t.key("Enter");
    assert!(
        r.iter().any(|r| matches!(
            r,
            Request::Call { cmd: Command::ArtnetTest { channels }, .. }
                if ac2_proto::model::dmx_list_text(channels) == "12=255 13=40"
        )),
        "{r:?}"
    );
    t.key("ArrowUp");
    let r = t.key("Enter");
    assert!(
        r.iter().any(|r| matches!(
            r,
            Request::Call { cmd: Command::ArtnetFade { seconds, .. }, .. } if seconds.0 == 2.5
        )),
        "{r:?}"
    );
    t.key("ArrowUp");
    let r = t.key("Enter");
    assert!(
        r.iter().any(|r| matches!(
            r,
            Request::Call { cmd: Command::ArtnetTarget { target: Some(a), .. }, .. }
                if a.addr.to_string() == "2.255.255.255:6454" && a.universe.get() == 1
        )),
        "{r:?}"
    );
}

/// Every other page, its focus moved off the first row and (where it types) something
/// typed: a run of mirror updates leaves it as it was.
#[test]
fn no_settings_page_resets_on_mirror_updates() {
    let mut t = T::new();
    let mut s = with_spl();
    tick(&mut s, 0);
    t.conn(mirror(s.clone()));
    t.type_key("Shift+L", "L");
    let mut n = 1;
    for page in Page::ALL {
        t.st.apply(Msg::Settings(SettingsMsg::Page(page)), &t.keys);
        assert_eq!(settings(&t).page, page);
        t.key("ArrowDown");
        t.text("7");
        for _ in 0..3 {
            update_holds(&mut t, &mut s, n);
            n += 1;
        }
    }
    // The Rig monitor page's Art-Net rows, each with something typed; and the Art-Net
    // target changed elsewhere while the universe is typed leaves the typed rows alone.
    t.st.apply(Msg::Settings(SettingsMsg::Page(Page::Rig)), &t.keys);
    for row in [
        RigRow::ArtnetTarget,
        RigRow::Universe,
        RigRow::Fade,
        RigRow::DmxTest,
    ] {
        t.st.apply(Msg::Settings(SettingsMsg::RigFocus(row)), &t.keys);
        t.text("7");
        for _ in 0..3 {
            update_holds(&mut t, &mut s, n);
            n += 1;
        }
    }
    s.rig.artnet_target = ac2_scene::artnet::parse_target("10.0.0.9", "3").expect("test value");
    s.rig.artnet_fade = ac2_proto::model::ArtnetFade::new(4.0).expect("test value");
    update_holds(&mut t, &mut s, n);
    let p = &settings(&t).rig;
    assert_eq!(
        (p.artnet.as_str(), p.universe.as_str(), p.fade.as_str()),
        ("7", "7", "17")
    );
}
