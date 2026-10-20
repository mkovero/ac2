//! The Settings pages, the rig monitor and the SPL / Leq page on a live rig.

use super::*;
use ac2_proto::model::PeakMode;

/// The Settings view's own pages on a live rig whose outputs are named and whose max level
/// is below its bound: Audio I/O with the max level row and a raise waiting for its
/// word, Recording, Display and Connection (an embedded-style daemon: no client keys).
#[test]
fn settings_pages() {
    use ac2_proto::Change;
    use ac2_proto::model::OutputSetup;
    use ac2_proto::units::Dbfs;
    if !have_gpu("settings_pages") {
        return;
    }
    let rig = common::Rig::start();
    {
        let mut f = rig.fake.lock();
        let mut g = f.state.generator.clone();
        g.ceiling = Dbfs(-40.0);
        g.ceiling_bound = Dbfs(-10.0);
        f.commit(Change::Generator(g));
        f.commit(Change::Outputs(vec![
            OutputSetup {
                channel: 0,
                label: Some("Main L".into()),
            },
            OutputSetup {
                channel: 1,
                label: Some("Main R".into()),
            },
        ]));
    }
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::COMMAND, Key::P);
    step_until(&mut h, "Audio I/O with the device", |a| {
        a.state.overlay.settings().is_some_and(|s| {
            s.page == ac2_ui::settings::Page::AudioIo && s.session.device_info().is_some()
        })
    });
    // ↑ from the first row (the backend) wraps to the max level row; -20, Enter: the raise asks for its word.
    h.key_press(Key::ArrowUp);
    h.event(Event::Text("-20".into()));
    h.key_press(Key::Enter);
    step_until(&mut h, "the raise confirmation", |a| {
        a.state
            .overlay
            .settings()
            .is_some_and(|s| s.ceiling.confirm.is_some())
    });
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "settings_max_level_raise");
    h.key_press(Key::Escape);
    for (key, page, name) in [
        (
            Key::Num4,
            ac2_ui::settings::Page::Recording,
            "settings_recording",
        ),
        (
            Key::Num5,
            ac2_ui::settings::Page::Display,
            "settings_display",
        ),
        (
            Key::Num6,
            ac2_ui::settings::Page::Connection,
            "settings_connection",
        ),
    ] {
        h.key_press_modifiers(Modifiers::ALT, key);
        step_until(&mut h, name, |a| {
            a.state.overlay.settings().is_some_and(|s| {
                s.page == page
                    && (page == ac2_ui::settings::Page::Display || s.connection.server.is_some())
            })
        });
        h.state_mut().state.toasts.clear();
        // The connection's id is the daemon's per-connection one: pinned for the picture.
        let pin = |a: &mut App| {
            if let Some(m) = &a.state.mirror {
                let mut m = (**m).clone();
                m.client_id = Some(ac2_proto::units::ClientId("ac2-ui test".into()));
                a.state.mirror = Some(std::sync::Arc::new(m));
            }
        };
        snapshot_when(&mut h, name, pin, |a| {
            a.state.my_client_id().map(|c| c.0.as_str()) == Some("ac2-ui test")
        });
    }
}

/// The rig monitor's look: an SPL meter kept by the rig leads the tree under the Rig header
/// with its cue chip; the Rig monitor page shows the target, monitor at start, the test
/// message, how sending fares and the rig's meters.
#[test]
fn rig_monitor_views() {
    use ac2_proto::Change;
    use ac2_proto::Patch;
    use ac2_proto::model::{
        CueState, CueSubject, MeasConfig, MeasKind, MeasScope, Measurement, OutputStatus, Rig,
        RigCue, SplConfig, TimeWeighting, Weighting,
    };
    use ac2_proto::units::{MeasId, Rev};
    if !have_gpu("rig_monitor_views") {
        return;
    }
    let rig = common::Rig::start();
    {
        let mut f = rig.fake.lock();
        f.commit(Change::Measurement(Patch::Set(Measurement {
            id: MeasId(40),
            config: MeasConfig {
                scope: MeasScope::Rig,
                name: "FOH limit".into(),
                kind: MeasKind::Spl {
                    config: SplConfig::on_input(0, Weighting::A, TimeWeighting::Fast),
                },
            },
            config_rev: Rev(1),
            running: true,
            delay: None,
            grid_id: None,
            colour: None,
            layout: None,
            compared: Vec::new(),
            offset: ac2_proto::units::Db(0.0),
        })));
        f.commit(Change::Rig(Rig {
            rev: Rev(5),
            osc_target: Some("192.168.1.20:9000".parse().expect("addr")),
            artnet_fade: ac2_proto::model::ArtnetFade::new(1.5).expect("fade"),
            monitor_at_start: true,
            damaged_files: Vec::new(),
            osc: OutputStatus {
                sent: 128,
                last_error: None,
            },
            artnet_target: Some(ac2_proto::model::ArtnetTarget {
                addr: "192.168.1.50:6454".parse().expect("addr"),
                universe: "0.0.1".parse().expect("universe"),
            }),
            artnet: OutputStatus {
                sent: 64,
                last_error: None,
            },
            dmx: ac2_proto::model::parse_dmx_list("12=255 14=40").expect("levels"),
            cues: vec![RigCue {
                meas: MeasId(40),
                subject: CueSubject::Meter,
                state: CueState::Near,
            }],
        }));
    }
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", |a| live_with(a, 5));
    step_until(&mut h, "the Rig header", |a| {
        a.state.tree_rows().first().map(|r| r.name.as_str()) == Some("Rig")
    });
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "tree_rig_group");
    h.state_mut().dispatch(ac2_ui::state::Msg::OpenRig);
    step_until(&mut h, "the Rig monitor page", |a| {
        a.state
            .overlay
            .settings()
            .is_some_and(|s| s.page == ac2_ui::settings::Page::Rig)
    });
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "settings_rig");
}

/// The SPL / Leq page taller than the window (the band meter's 11 windows) holds where the
/// wheel scrolled it while the rig's updates arrive: the Rig section at its bottom stays in
/// view. A key moving the focus brings the focused row back into view.
#[test]
fn leq_page_holds_its_scroll_while_the_rig_meters() {
    use ac2_proto::Change;
    use ac2_proto::Patch;
    use ac2_proto::model::{
        BandLeqPreset, CueState, CueSubject, MeasKind, MeasScope, OutputStatus, Rig, RigCue,
    };
    use ac2_proto::units::{MeasId, Rev};
    if !have_gpu("leq_page_holds_its_scroll_while_the_rig_meters") {
        return;
    }
    let rig_state = |meas: MeasId, n: u64| Rig {
        rev: Rev(5),
        osc_target: Some("192.168.1.20:9000".parse().expect("addr")),
        artnet_fade: ac2_proto::model::ArtnetFade::DEFAULT,
        monitor_at_start: true,
        damaged_files: Vec::new(),
        osc: OutputStatus {
            sent: n,
            last_error: None,
        },
        artnet: OutputStatus {
            sent: n,
            last_error: None,
        },
        dmx: ac2_proto::model::parse_dmx_list(if n.is_multiple_of(2) { "12=80" } else { "" })
            .expect("levels"),
        artnet_target: None,
        cues: vec![RigCue {
            meas,
            subject: CueSubject::Meter,
            state: if n.is_multiple_of(2) {
                CueState::Near
            } else {
                CueState::Ok
            },
        }],
    };
    let rig = common::Rig::start();
    let meas;
    {
        // The rig's SPL meter, the page's, with the band meter's 545 windows.
        let mut f = rig.fake.lock();
        let mut m = f
            .state
            .measurements
            .iter()
            .find(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
            .cloned()
            .expect("the rig's SPL meter");
        if let MeasKind::Spl { config } = &mut m.config.kind {
            config.bands = Some(Box::new(BandLeqPreset::Finland545Lf.apply(None)));
        }
        m.config.scope = MeasScope::Rig;
        meas = m.id;
        f.commit(Change::Measurement(Patch::Set(m)));
        f.commit(Change::Rig(rig_state(meas, 0)));
    }
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.state_mut().dispatch(ac2_ui::state::Msg::OpenRig);
    h.state_mut().dispatch(ac2_ui::state::Msg::Settings(
        ac2_ui::state::SettingsMsg::Page(ac2_ui::settings::Page::Leq),
    ));
    step_until(&mut h, "the SPL / Leq page with 11 band windows", |a| {
        a.state
            .overlay
            .leq()
            .is_some_and(|d| d.bands.rows.len() == 11)
    });
    h.step();
    let keep = |h: &Harness<'_, App>| {
        h.query_by_label(ac2_scene::osc::KEEP_ON_RIG)
            .map(|n| n.rect())
            .expect("the Keep on rig row")
    };
    let below = keep(&h);
    assert!(
        below.min.y > SIZE.y,
        "the Rig section starts below the window: {below:?}"
    );
    h.event(Event::PointerMoved(egui::pos2(700.0, 400.0)));
    for _ in 0..20 {
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -400.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    settle(&mut h, 10);
    let scrolled = keep(&h);
    assert!(
        scrolled.max.y < SIZE.y,
        "the wheel brought the Rig section up: {scrolled:?}"
    );
    // The rig's updates, one a frame: the page stays where the wheel left it.
    for n in 1..=30 {
        rig.fake.lock().commit(Change::Rig(rig_state(meas, n)));
        h.step();
    }
    step_until(&mut h, "the rig's updates", |a| {
        a.state.daemon().is_some_and(|s| s.rig.osc.sent == 30)
    });
    settle(&mut h, 5);
    assert_eq!(keep(&h), scrolled, "a mirror update moved the page");
    // ↑ from the preset row wraps to the last row; ↓ back to the preset brings it into view.
    h.key_press(Key::ArrowUp);
    h.key_press(Key::ArrowDown);
    settle(&mut h, 10);
    assert!(
        keep(&h).min.y > SIZE.y,
        "the focused preset row came back into view"
    );
}

/// P twice on the spectrum pane: peak hold, then peak tracking, which drops the hold line.
/// The rig's tone near 1 kHz gets a box on the curve and a row in the list under the cursor
/// readout; T to traces only takes the list (and the cursor) away and leaves the box.
#[test]
fn spectrum_peak_tracking() {
    if !have_gpu("spectrum_peak_tracking") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::ALT, Key::Num2);
    h.key_press(Key::W);
    let peak = |a: &App| a.state.meas(MeasId(3)).map(|m| a.state.display_of(m).peak);
    h.key_press(Key::P);
    // The second P steps from what the daemon took, not from what was asked.
    step_until(&mut h, "peak hold", |a| peak(a) == Some(PeakMode::Hold));
    h.key_press(Key::P);
    h.state_mut().state.view.cursor_hz = Some(1000.0);
    step_until(&mut h, "peaks tracked", |a| {
        peak(a) == Some(PeakMode::Track)
            && a.state
                .peak_tracks
                .get(&MeasId(3))
                .is_some_and(|t| !t.1.peaks().is_empty())
    });
    assert!(
        !h.state().state.peaks.contains_key(&MeasId(3)),
        "tracking keeps no hold"
    );
    let scene = |h: &Harness<'_, App>| {
        let st = &h.state().state;
        ac2_ui::scenes::spectrum(
            st,
            crate::common::pane(st, PaneKind::Spectrum),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1000.0,
                height: 450.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        )
    };
    let s = scene(&h);
    let list = s.peak_list.clone().expect("the list is up");
    // The tone is in the 996 Hz bin (11.7 Hz bins), its neighbours equal: no shift.
    assert!(list.starts_with("1  996 Hz  −32.0 dBFS (tone)"), "{list}");
    assert!(s.peaks[0].rows[0].mark.is_some());
    h.state_mut().state.toasts.clear();
    snapshot(&mut h, "spectrum_peak_tracking");
    h.key_press(Key::T);
    h.key_press(Key::T);
    step_until(&mut h, "traces only", |a| {
        let p = crate::common::pane(&a.state, PaneKind::Spectrum);
        a.state.view_for(p).chrome == ac2_scene::view::PlotChrome::Bare
    });
    let s = scene(&h);
    assert_eq!(s.peak_list, None);
    assert!(s.cursor.is_none());
    assert!(s.peaks[0].rows[0].mark.is_some(), "the box stays");
    h.state_mut().state.toasts.clear();
    snapshot(&mut h, "spectrum_peak_tracking_bare");
}
