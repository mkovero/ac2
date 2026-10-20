//! Tests of the Settings view.

use super::*;
use ac2_proto::model::GenAudit;

fn generator(ceiling: f64, bound: f64) -> Generator {
    Generator {
        owner: None,
        armed: false,
        firing: false,
        settings: None,
        ceiling: Dbfs(ceiling),
        ceiling_bound: Dbfs(bound),
        last_action: None::<GenAudit>,
    }
}

#[test]
fn pages_step_and_wrap() {
    assert_eq!(Page::AudioIo.step(1), Page::Calibration);
    assert_eq!(Page::AudioIo.step(-1), Page::Rig);
    assert_eq!(Page::Connection.step(1), Page::Rig);
    assert_eq!(Page::Rig.step(1), Page::AudioIo);
    for p in Page::ALL {
        assert!(!p.title().is_empty() && !p.keys().is_empty());
        assert!(p.whose().contains(THIS_APP) || p.whose().contains(THE_RIG));
    }
}

#[test]
fn lowering_goes_out_at_once_a_raise_asks_for_the_word() {
    let g = generator(-40.0, -10.0);
    let mut e = CeilingEdit {
        text: "-50".into(),
        ..CeilingEdit::default()
    };
    assert_eq!(e.enter(Some(&g), true), Some((Dbfs(-50.0), false)));
    assert!(e.text.is_empty());

    // Above the bound: refused here, saying where the bound comes from.
    e.type_text("-6");
    assert_eq!(e.enter(Some(&g), false), None);
    assert!(
        e.error
            .as_deref()
            .expect("set")
            .contains("ac2d --max-level")
    );

    // A raise: refused while live, else a confirmation that needs the word.
    e.text = "-20 dBFS".into();
    assert_eq!(e.enter(Some(&g), true), None);
    assert_eq!(e.error.as_deref(), Some(RAISE_WHILE_LIVE));
    assert_eq!(e.enter(Some(&g), false), None);
    let c = e.confirm.clone().expect("set");
    assert_eq!((c.from, c.to), (Dbfs(-40.0), Dbfs(-20.0)));
    e.type_text("yes");
    assert_eq!(e.enter(Some(&g), false), None);
    assert!(e.error.as_deref().expect("set").starts_with("type raise"));
    e.confirm.as_mut().expect("set").typed.clear();
    e.type_text("Raise");
    assert_eq!(e.enter(Some(&g), false), Some((Dbfs(-20.0), true)));
    assert!(e.confirm.is_none());

    e.type_text("loud");
    assert_eq!(e.enter(Some(&g), false), None);
    assert!(
        e.error
            .as_deref()
            .expect("set")
            .contains("expected a number followed by a unit")
    );
}

#[test]
fn display_and_recording_texts() {
    assert_eq!(step_theme(ThemeName::Dark, 1), ThemeName::Light);
    assert_eq!(step_theme(ThemeName::Dark, -1), ThemeName::HighContrast);
    assert_eq!(step_hold(None, 1), Some(250));
    assert_eq!(step_hold(None, -1), None);
    let views = PaneViews {
        spectrum: SpectrumMode::Split,
        sweep: SweepMode::Room,
        leq: LeqLayout {
            style: LeqStyle::Tiles,
            history: false,
        },
        distortion_unit: DistortionUnit::Percent,
        coherence: CoherencePlacement::OverlayOnMagnitude,
    };
    let switches = DisplaySwitches {
        key_hints: false,
        warning_toasts: false,
        resolution_marker: false,
        bar_ease: false,
        spec_ease: false,
    };
    let rows = display_rows(
        ThemeName::Light,
        switches,
        Some(500),
        views,
        ac2_scene::advice::AdviceSettings::default(),
    );
    let texts: Vec<&str> = rows.iter().map(|(_, v)| v.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "light",
            "hidden",
            "off: warnings and Leq limit alarms go only to the notification log",
            "0.50 s",
            "tiles",
            "hidden",
            "jump to each new reading",
            "the spectrum over its spectrograph",
            "jumps to each new frame",
            "the room parameters",
            "% of fundamental",
            "over the magnitude",
            "off",
            "each pane's as last left · Enter resets them to the defaults",
            "on: a notice suggests lowering or raising the stimulus (never changes it)",
            "reference peak within 3 dB of full scale",
            "coherence under γ² 0.70",
            "measurement input under −70.0 dBFS RMS",
        ]
    );
    let rows = recording_rows(60, None);
    assert_eq!(rows[0].1, "the record toggle stops by itself after 60 min");
    assert_eq!(rows[1].1, "asking the daemon…");
    let s = ac2_proto::samples::server_info();
    assert_eq!(
        recording_rows(60, Some(&s))[1].1,
        "/home/fohtech/.local/share/ac2/recordings (ac2d --recordings)"
    );
    let mut r = RecordingPage {
        text: "90 min".into(),
        ..RecordingPage::default()
    };
    assert_eq!(r.enter(), Some(90));
    r.text = "0".into();
    assert_eq!(r.enter(), None);
    assert_eq!(r.error.as_deref(), Some("type whole minutes, 1 … 480"));
}

#[test]
fn connection_lines_authorize_and_revoke() {
    let s = ac2_proto::samples::server_info();
    let mut p = ConnectionPage {
        server: Some(Ok(s.clone())),
        ..ConnectionPage::default()
    };
    let lines = p.lines();
    assert_eq!(lines[0], ConnLine::Reconnect);
    assert_eq!(lines[1], ConnLine::ConnectOther);
    assert_eq!(lines[2], ConnLine::Authorized("laptop".into()));
    assert!(matches!(&lines[3], ConnLine::Refused(Some(_), a) if a == "192.168.1.40"));
    assert!(matches!(&lines[4], ConnLine::Refused(None, _)));
    assert_eq!(lines[5], ConnLine::AddKey);
    assert_eq!(p.enter(), Some(ConnAction::Reconnect));

    // Revoking takes two presses.
    p.focus = 2;
    assert_eq!(p.delete(), None);
    assert!(
        p.notice
            .as_deref()
            .expect("set")
            .starts_with("Delete again revokes laptop")
    );
    assert_eq!(p.delete(), Some(ConnAction::Revoke("laptop".into())));

    // A refused key: a name, then Enter authorizes it.
    p.focus = 3;
    assert!(p.start_authorize());
    p.type_text("tablet");
    let key = "D:)Q[IlAW!ahhC2ac:9*A}h:p?([4%wOTJ%JR%cs".to_owned();
    assert_eq!(
        p.enter(),
        Some(ConnAction::Authorize {
            name: "tablet".into(),
            key
        })
    );
    // A peer without CURVE has nothing to authorize.
    p.focus = 4;
    assert!(!p.start_authorize());

    // By hand: name and key.
    p.focus = 5;
    assert_eq!(p.enter(), None);
    p.type_text("phone");
    assert_eq!(p.enter(), None);
    assert!(p.error.is_some(), "a name alone is not enough");
    p.type_text(" KEY40");
    assert_eq!(
        p.enter(),
        Some(ConnAction::Authorize {
            name: "phone".into(),
            key: "KEY40".into()
        })
    );
    assert_eq!(
        conn_line_text(&ConnLine::Authorized("laptop".into()), Some(&s)),
        "laptop · fingerprint SHA256:b2aa 0c3d 9e41 7f60"
    );
}

#[test]
fn link_lines_say_who_and_which_key() {
    let k = ClientKey {
        fingerprint: "1a2b-3c4d".into(),
        key: "Z85KEY".into(),
    };
    assert_eq!(
        link_lines(
            "0.1.0+0123456789ab",
            "foh-rig",
            "connected",
            Some("ac2d 0.1"),
            Some("laptop"),
            Some(&k)
        ),
        vec![
            "Version: ac2-ui 0.1.0+0123456789ab",
            "Daemon: foh-rig · connected",
            "Server software: ac2d 0.1",
            "This client is laptop on the daemon",
            "This client's key: fingerprint 1a2b-3c4d · Z85KEY (what a rig's operator \
             authorizes)",
        ]
    );
}

/// The Leq page's range key is written in the label style.
#[test]
fn page_keys_follow_the_label_style() {
    use crate::keys::LabelStyle;
    assert!(
        Page::Leq
            .keys_in(LabelStyle::Pc)
            .contains("Shift+Insert or + range…")
    );
    assert!(
        Page::Leq
            .keys_in(LabelStyle::Mac)
            .contains("⇧Insert or + range…")
    );
}
