//! Reducer tests of the stimulus (arm, fire, level, stop) and the transfer commands.

use super::*;

#[test]
fn selects_the_first_transfer_measurement() {
    let t = T::new();
    assert_eq!(t.st.selected, Some(MeasId(1)));
    let names: Vec<&str> =
        t.st.measurements()
            .iter()
            .map(|m| m.config.name.as_str())
            .collect();
    assert_eq!(names, ["Main L", "Sub"]);
}

#[test]
fn stimulus_needs_a_typed_level() {
    let mut t = T::new();
    // Space without a level opens the level prompt and arms nothing.
    let r = t.type_key("Space", " ");
    assert!(r.is_empty(), "{r:?}");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::StimulusLevel && p.text.is_empty()
    ));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    // Enter in the prompt applies it; it does not fire.
    t.text("-20");
    let r = t.key("Enter");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.stimulus.level, Some(Dbfs(-20.0)));
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
}

#[test]
fn arm_fire_level_stop() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-20.0));
    // Enter before arming does nothing but say so.
    assert!(t.key("Enter").is_empty());
    assert!(t.last_toast().contains("not armed"));

    let r = t.key("Space");
    match r.as_slice() {
        [Request::StimArm { settings, force }] => {
            assert_eq!(settings.level, Dbfs(-20.0));
            assert_eq!(settings.signal, Signal::Pink);
            assert_eq!(settings.outputs, vec![0]);
            assert!(!force);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(t.st.stimulus.phase, StimPhase::Arming);
    // A second Space while arming sends nothing.
    assert!(t.key("Space").is_empty());
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert_eq!(t.st.stimulus.phase, StimPhase::Armed);

    let r = t.key("Enter");
    assert_eq!(
        r.iter().filter_map(set_level).collect::<Vec<_>>(),
        [(-20.0, true, true)]
    );
    assert_eq!(t.st.stimulus.phase, StimPhase::FireRequested);
    t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
    assert_eq!(t.st.stimulus.phase, StimPhase::Firing);

    // ↑/↓ change the level of the running stimulus; Shift steps 3 dB.
    let r = t.key("Up");
    assert_eq!(
        r.iter().filter_map(set_level).collect::<Vec<_>>(),
        [(-19.0, true, true)]
    );
    let r = t.key("Shift+Down");
    assert_eq!(
        r.iter().filter_map(set_level).collect::<Vec<_>>(),
        [(-22.0, true, true)]
    );

    let r = t.key("Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
    assert_eq!(t.st.stimulus.phase, StimPhase::Stopping);
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
}

/// Space right after Esc, while the stop is still on its way: nothing goes out then (the
/// lease is being released), and the stimulus arms once the stop has landed. A failed
/// stop, a lost lease, another stop, a closed window or a lost connection drop the request:
/// nothing arms behind them.
#[test]
fn space_during_a_stop_arms_once_the_stop_lands() {
    let stopping = |t: &mut T| {
        t.st.stimulus.level = Some(Dbfs(-20.0));
        t.st.stimulus.phase = StimPhase::Firing;
        let r = t.key("Esc");
        assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
        assert_eq!(t.st.stimulus.phase, StimPhase::Stopping);
        assert!(
            t.key("Space").is_empty(),
            "nothing while the stop is in flight"
        );
        assert!(t.last_toast().contains("arms once the stop is done"));
        assert!(t.key("Space").is_empty());
    };
    let is_arm = |r: &[Request]| matches!(r, [Request::StimArm { force: false, .. }]);

    let mut t = T::new();
    stopping(&mut t);
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert!(is_arm(&r), "{r:?}");
    assert_eq!(t.st.stimulus.phase, StimPhase::Arming);
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert_eq!(t.st.stimulus.phase, StimPhase::Armed);

    // The stop failed: the lease is in doubt, nothing arms.
    let mut t = T::new();
    stopping(&mut t);
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Failed("timeout".into())));
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(t.conn(ConnEvent::Stimulus(StimEvent::Stopped)).is_empty());

    // The lease is gone.
    let mut t = T::new();
    stopping(&mut t);
    t.conn(ConnEvent::Stimulus(StimEvent::Lost("expired".into())));
    assert!(t.conn(ConnEvent::Stimulus(StimEvent::Stopped)).is_empty());

    // Esc, or the stop chord, again: the operator wants it stopped after all.
    for chord in ["Esc", "Shift+Esc"] {
        let mut t = T::new();
        stopping(&mut t);
        t.key(chord);
        let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
        assert!(r.is_empty(), "{chord}: {r:?}");
        assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    }

    // The connection dropped: a new one holds no lease and arms nothing.
    let mut t = T::new();
    stopping(&mut t);
    t.conn(ConnEvent::Failed {
        target: "local daemon".into(),
        error: "not responding".into(),
        retry_in: std::time::Duration::from_secs(2),
    });
    assert!(!t.st.stimulus.arm_after_stop);
    assert!(t.conn(ConnEvent::Stimulus(StimEvent::Stopped)).is_empty());

    // The audio stopped meanwhile: the daemon disarmed it, and nothing arms after the stop.
    let mut t = T::new();
    stopping(&mut t);
    let mut s = daemon_state();
    s.session.stopped = Some(ac2_proto::model::AudioStopped {
        since: WallNs(0),
        cause: ac2_proto::model::StopCause::HostEnded,
        recovery: ac2_proto::model::Recovery::Opening {
            attempt: 1,
            started: WallNs(0),
        },
    });
    t.conn(mirror(s));
    assert!(!t.st.stimulus.arm_after_stop);
    assert!(t.last_toast().contains("the audio stopped"));
    assert!(t.conn(ConnEvent::Stimulus(StimEvent::Stopped)).is_empty());
}

#[test]
fn level_never_exceeds_the_ceiling() {
    let mut t = T::new();
    // empty_state's ceiling is −6 dBFS.
    assert_eq!(t.st.ceiling(), Some(Dbfs(-6.0)));
    t.key("L");
    t.text("-3");
    assert!(t.key("Enter").is_empty());
    match &t.st.overlay {
        Overlay::Prompt(p) => assert!(p.error.as_deref().unwrap_or("").contains("ceiling")),
        o => panic!("{o:?}"),
    }
    assert_eq!(t.st.stimulus.level, None);
    t.key("Esc");
    t.st.stimulus.level = Some(Dbfs(-7.0));
    t.st.stimulus.phase = StimPhase::Armed;
    let r = t.key("Shift+Up");
    assert_eq!(
        r.iter().filter_map(set_level).collect::<Vec<_>>(),
        [(-6.0, true, false)]
    );
    assert!(t.key("Up").is_empty());
    assert!(t.last_toast().contains("ceiling"));
}

#[test]
fn arrows_without_level_send_nothing() {
    let mut t = T::new();
    assert!(t.key("Up").is_empty());
    assert!(
        t.st.toasts
            .last()
            .is_some_and(|x| x.severity == Severity::Warning)
    );
    assert_eq!(t.st.stimulus.level, None);
}

#[test]
fn escape_closes_a_window_or_stops() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    // Esc from inside the palette only closes it; the next Esc, with nothing open, stops.
    t.key("Ctrl+K");
    assert!(matches!(t.st.overlay, Overlay::Palette(_)));
    assert!(t.key("Esc").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.stimulus.phase, StimPhase::Armed);
    let r = t.key("Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]));
    // Another client's running generator: Esc stops it too (gen.stop is universal).
    let mut t = T::new();
    let mut s = daemon_state();
    s.generator.firing = true;
    s.generator.armed = true;
    s.generator.owner = Some(ClientId("other".into()));
    t.conn(mirror(s));
    let r = t.key("Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]));
    // Nothing live: Esc only closes overlays.
    let mut t = T::new();
    t.key("H");
    assert_eq!(t.st.overlay, Overlay::Help);
    assert!(t.key("Esc").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn disconnected_never_arms() {
    let mut t = T::disconnected();
    t.st.stimulus.level = Some(Dbfs(-20.0));
    assert!(t.key("Space").is_empty());
    assert!(t.last_toast().contains("not connected"));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
}

#[test]
fn lost_lease_returns_to_idle() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.conn(ConnEvent::Stimulus(StimEvent::Lost("taken over".into())));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(t.last_toast().contains("taken over"));
    assert!(t.key("Enter").is_empty());
    // A failed arm also returns to idle.
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Failed("lease held".into())));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
}

#[test]
fn opening_key_is_not_typed_into_the_prompt() {
    let mut t = T::new();
    t.type_key("L", "l");
    t.text("-1");
    t.text("8");
    match &t.st.overlay {
        Overlay::Prompt(p) => assert_eq!(p.text, "-18"),
        o => panic!("{o:?}"),
    }
    t.st.update(Msg::Backspace, &t.keys.clone());
    t.text("9");
    t.key("Enter");
    assert_eq!(t.st.stimulus.level, Some(Dbfs(-19.0)));
}

#[test]
fn keys_follow_the_focused_pane() {
    let mut t = T::new();
    // Spectrum pane: P is peak hold.
    t.key("Alt+2");
    assert_eq!(t.st.scope(), Scope::Spectrum);
    t.key("P");
    assert!(t.st.view.spectrum.peak_hold);
    // X means nothing there.
    assert!(t.key("X").is_empty());
    // A transfer pane's IR view has the IR's keys; G back to its response, the transfer's.
    t.key("Alt+3");
    let pane = t.st.layout.focus;
    assert_eq!(t.st.scope(), Scope::Ir);
    t.key("G");
    assert_eq!(
        (t.st.layout.focus, t.focus_kind()),
        (pane, PaneKind::Transfer)
    );
    assert_eq!(t.st.scope(), Scope::Transfer);
    // W maximizes.
    t.key("Alt+2");
    t.key("W");
    assert_eq!(t.visible(), vec![PaneKind::Spectrum]);
}

#[test]
fn transfer_commands() {
    let mut t = T::new();
    let r = t.key("X");
    assert!(matches!(
        r.as_slice(),
        [Request::FindDelay {
            meas: MeasId(1),
            pick: DelayPick::FirstArrival,
            band: FinderBand::Auto,
            observation: None,
        }]
    ));
    let r = t.key("Shift+X");
    assert!(matches!(
        r.as_slice(),
        [Request::FindDelay {
            meas: MeasId(1),
            pick: DelayPick::Strongest,
            ..
        }]
    ));
    let r = t.key("Y");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::DelayTrack { enabled: true, .. },
            ..
        }]
    ));
    // U / J are display edits, not commands.
    assert!(t.key("U").is_empty());
    assert!(t.st.edit(MeasId(1)).inverted);
    t.type_key("J", "j");
    t.text("+3,5 dB");
    t.key("Enter");
    assert_eq!(t.st.edit(MeasId(1)).offset_db, 3.5);
    // A measurement has one delay, the daemon's: plain , . step it by 0.1 ms, Ctrl / Alt on
    // the same keys by a whole sample or a tenth. The mirror is at the arrival (12.50 ms)
    // and gets no reply here, so each toast is one step from there.
    for (key, by, what) in [
        (".", 0.000_1, "+0.1 ms → 12.60 ms (+0.10 ms from arrival)"),
        (",", -0.000_1, "−0.1 ms → 12.40 ms (−0.10 ms from arrival)"),
        (
            "Ctrl+.",
            1.0 / 48_000.0,
            "+1 sample → 12.521 ms (+0.021 ms from arrival)",
        ),
        (
            "Ctrl+,",
            -1.0 / 48_000.0,
            "−1 sample → 12.479 ms (−0.021 ms from arrival)",
        ),
        (
            "Alt+.",
            0.1 / 48_000.0,
            "+0.1 sample → 12.502 ms (+0.002 ms from arrival)",
        ),
        (
            "Alt+,",
            -0.1 / 48_000.0,
            "−0.1 sample → 12.498 ms (−0.002 ms from arrival)",
        ),
    ] {
        let r = t.key(key);
        match r.as_slice() {
            [
                Request::Call {
                    cmd: Command::DelayNudge { meas, by: b },
                    what: w,
                },
            ] => {
                assert_eq!(*meas, MeasId(1));
                assert!((b.0 - by).abs() < 1e-15, "{key}");
                assert_eq!(*w, format!("Main L: delay {what}"), "{key}");
            }
            r => panic!("{key}: {r:?}"),
        }
    }
    // Typed delay.
    t.type_key("D", "d");
    match &t.st.overlay {
        Overlay::Prompt(p) => assert_eq!(p.text, "12.5"),
        o => panic!("{o:?}"),
    }
    let r = t.key("Enter");
    assert!(
        matches!(
            r.as_slice(),
            [Request::Call { cmd: Command::DelaySet { delay: Seconds(d), .. }, what }]
                if (*d - 0.0125).abs() < 1e-12 && what == "Main L: delay 12.50 ms"
        ),
        "{r:?}"
    );
    // A negative typed delay: the measurement leads the reference.
    t.type_key("D", "d");
    if let Overlay::Prompt(p) = &mut t.st.overlay {
        p.text = "\u{2212}2.5".into();
    }
    let r = t.key("Enter");
    assert!(
        matches!(
            r.as_slice(),
            [Request::Call { cmd: Command::DelaySet { delay: Seconds(d), .. }, .. }]
                if (*d + 0.0025).abs() < 1e-12
        ),
        "{r:?}"
    );
    // View toggles.
    t.key("B");
    assert_eq!(t.st.view.tf.coherence.blank_below, Some(0.3));
    t.key("Shift+P");
    assert!(matches!(t.st.view.tf.phase, PhaseView::GroupDelay { .. }));
    // P leaves group delay for wrapped phase, then toggles wrapped / unwrapped.
    t.key("P");
    assert_eq!(t.st.view.tf.phase, PhaseView::Wrapped);
    t.key("P");
    assert!(matches!(t.st.view.tf.phase, PhaseView::Unwrapped { .. }));
    t.st.update(Msg::Command(CommandId::CoherencePlacement), &t.keys);
    assert_eq!(
        t.st.view.tf.coherence_placement,
        ac2_scene::view::CoherencePlacement::OverlayOnMagnitude
    );
    t.key("E");
    assert_eq!(
        t.st.view.tf.phase_reference,
        Some(TraceKey::Live(MeasId(1)))
    );
    // Z asks for a target file; M with no stored traces says what it needs.
    assert!(t.key("Z").is_empty());
    assert!(
        matches!(&t.st.overlay, Overlay::Prompt(p) if p.kind == PromptKind::ImportFile(ImportRole::Target))
    );
    t.key("Esc");
    assert!(t.key("M").is_empty());
    assert!(t.last_toast().contains("at least two"));
}

/// The text of the one call in `r`.
fn call_text(r: &[Request]) -> &str {
    match r {
        [Request::Call { what, .. }] => what,
        r => panic!("{r:?}"),
    }
}

#[test]
fn delay_toasts_name_the_step_the_delay_and_its_offset_from_the_arrival() {
    let mut t = T::new();
    // At the arrival: a typed delay is just the delay.
    t.type_key("D", "d");
    assert_eq!(call_text(&t.key("Enter")), "Main L: delay 12.50 ms");
    // 0.05 ms (2.4 samples) of delay steps on the arrival.
    let mut s = daemon_state();
    for m in &mut s.measurements {
        if let Some(d) = &mut m.delay {
            d.applied = Seconds(602.4 / 48_000.0);
            d.applied_samples = 602.4;
            d.nudged = Seconds(2.4 / 48_000.0);
            d.nudged_samples = 2.4;
        }
    }
    t.conn(mirror(s));
    assert_eq!(
        call_text(&t.key("Ctrl+.")),
        "Main L: delay +1 sample → 12.571 ms (+0.071 ms from arrival)"
    );
    assert_eq!(
        call_text(&t.key("Alt+,")),
        "Main L: delay −0.1 sample → 12.548 ms (+0.048 ms from arrival)"
    );
    assert_eq!(
        call_text(&t.key(".")),
        "Main L: delay +0.1 ms → 12.65 ms (+0.15 ms from arrival)"
    );
    // A typed delay keeps the arrival (12.50 ms): the offset is the rest.
    t.type_key("D", "d");
    for _ in 0..10 {
        t.key("Backspace");
    }
    t.text("12");
    assert_eq!(
        call_text(&t.key("Enter")),
        "Main L: delay 12.00 ms (−0.50 ms from arrival)"
    );
    t.type_key("D", "d");
    for _ in 0..10 {
        t.key("Backspace");
    }
    t.text("12.5");
    assert_eq!(call_text(&t.key("Enter")), "Main L: delay 12.50 ms");
}

#[test]
fn transfer_commands_need_a_transfer_measurement() {
    let mut t = T::new();
    // The spectrum picked in the list: the spectrum pane takes the keys, and a transfer
    // command from the palette says what it needs.
    t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(2)));
    assert_eq!(t.focus_kind(), PaneKind::Spectrum);
    assert!(t.key("X").is_empty());
    assert!(
        t.st.update(Msg::Command(CommandId::InsertDelay), &t.keys)
            .is_empty()
    );
    assert!(t.last_toast().contains("transfer-function"));
}

/// Arms the generator at −20 dBFS and, with `fire`, plays it.
fn play_noise(t: &mut T, fire: bool) {
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    if fire {
        t.key("Enter");
        t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
        assert_eq!(t.st.stimulus.phase, StimPhase::Firing);
    }
}

/// What a measurement stop asked for: its toast text, and whether the stimulus stop went
/// with it.
fn meas_stop(r: &[Request]) -> (String, bool) {
    let what = r
        .iter()
        .find_map(|x| match x {
            Request::Call {
                cmd: Command::MeasStop { .. },
                what,
            } => Some(what.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no measurement stop: {r:?}"));
    (what, r.iter().any(|x| matches!(x, Request::StimStop)))
}

const STIM_TOO: &str = "Main L stopped · stimulus stopped (no transfer measurement left running)";

#[test]
fn stopping_the_last_transfer_stops_the_stimulus() {
    let mut t = T::new();
    t.put(PaneKind::Transfer);
    play_noise(&mut t, true);
    let r = t.key("S");
    assert_eq!(meas_stop(&r), (STIM_TOO.into(), true));
    assert_eq!(t.st.stimulus.phase, StimPhase::Stopping);
    t.conn(ConnEvent::Reply {
        what: STIM_TOO.into(),
        result: Ok(()),
    });
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    // One toast says both; the stop's own "stimulus stopped" would repeat it.
    assert_eq!(t.last_toast(), STIM_TOO);
    // The next operator stop says so again.
    play_noise(&mut t, true);
    t.key("Escape");
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.last_toast(), "stimulus stopped");
}

#[test]
fn stopping_the_last_transfer_disarms_an_armed_stimulus() {
    let mut t = T::new();
    t.put(PaneKind::Transfer);
    play_noise(&mut t, false);
    assert_eq!(meas_stop(&t.key("S")), (STIM_TOO.into(), true));
}

#[test]
fn every_stop_path_of_a_transfer_stops_the_stimulus() {
    // S on the transfer pane, S on its IR view (`None`), the palette's command (any pane),
    // and the command on the sweep view with the transfer measurement selected.
    enum Via {
        Key,
        Palette,
    }
    for (focus, via) in [
        (Some(PaneKind::Transfer), Via::Key),
        (None, Via::Key),
        (Some(PaneKind::Transfer), Via::Palette),
        (Some(PaneKind::Distortion), Via::Palette),
    ] {
        let mut t = T::new();
        t.put(PaneKind::Transfer);
        play_noise(&mut t, true);
        match focus {
            Some(p) => t.put(p),
            None => t.put_ir(),
        }
        t.st.selected = Some(MeasId(1));
        let r = match via {
            Via::Key => t.key("S"),
            Via::Palette => t.st.update(Msg::Command(CommandId::StartStop), &t.keys),
        };
        assert_eq!(meas_stop(&r), (STIM_TOO.into(), true), "{focus:?}");
    }
}

#[test]
fn another_running_transfer_keeps_the_stimulus() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.push(meas(3, "Main R", transfer()));
    t.conn(mirror(s.clone()));
    t.put(PaneKind::Transfer);
    t.st.selected = Some(MeasId(1));
    play_noise(&mut t, true);
    assert_eq!(meas_stop(&t.key("S")), ("Main L stopped".into(), false));
    assert_eq!(t.st.stimulus.phase, StimPhase::Firing);
    // A stopped one does not count: with Main R stopped, Main L is the last running.
    s.measurements[2].running = false;
    t.conn(mirror(s));
    assert_eq!(meas_stop(&t.key("S")), (STIM_TOO.into(), true));
}

#[test]
fn stopping_a_spectrum_keeps_the_stimulus() {
    let mut t = T::new();
    t.put(PaneKind::Transfer);
    play_noise(&mut t, true);
    t.put(PaneKind::Spectrum);
    assert_eq!(meas_stop(&t.key("S")), ("Sub stopped".into(), false));
    assert_eq!(t.st.stimulus.phase, StimPhase::Firing);
}

#[test]
fn a_stimulus_this_app_does_not_hold_keeps_playing() {
    let mut t = T::new();
    t.put(PaneKind::Transfer);
    // Another client's noise: its lease is not this app's to end.
    let mut s = daemon_state();
    s.generator.owner = Some(ClientId("other".into()));
    s.generator.armed = true;
    s.generator.firing = true;
    t.conn(mirror(s.clone()));
    assert_eq!(meas_stop(&t.key("S")), ("Main L stopped".into(), false));
    // Nothing playing at all.
    let mut t = T::new();
    t.put(PaneKind::Transfer);
    assert_eq!(meas_stop(&t.key("S")), ("Main L stopped".into(), false));
    // This client's lease as only the mirror shows it.
    s.generator.owner = Some(ClientId("c1".into()));
    t.conn(mirror(s));
    assert_eq!(meas_stop(&t.key("S")), (STIM_TOO.into(), true));
}

#[test]
fn a_sweep_is_never_stopped_by_a_transfer_stop() {
    let mut t = T::new();
    t.put(PaneKind::Transfer);
    play_noise(&mut t, true);
    t.st.sweep.run = Some(sweep_run(SweepStatus::Playing { repeat: 1 }).id);
    assert_eq!(meas_stop(&t.key("S")), ("Main L stopped".into(), false));
    assert_eq!(t.st.stimulus.phase, StimPhase::Firing);
}

/// Arming, firing, stopping and an arm queued behind a stop never change the layout, the
/// full-screen state or what the view draws around the panes (the pane rects follow from
/// those alone), in every layout: split, one pane, F11 over the split, and the stage view
/// (where a running sweep changes nothing either).
#[test]
fn the_stimulus_never_changes_the_layout() {
    for keys in [&[][..], &["W"], &["F11"], &["W", "W"]] {
        let mut t = T::new();
        t.put(PaneKind::Transfer);
        for k in keys {
            t.key(k);
        }
        let seen = |t: &T| {
            (
                t.st.layout.clone(),
                t.st.fullscreen,
                t.st.stage_view(),
                t.st.key_hints_shown(),
            )
        };
        let before = seen(&t);
        let check = |t: &mut T, what: &str| assert_eq!(seen(t), before, "{keys:?}: {what}");
        t.st.stimulus.level = Some(Dbfs(-20.0));
        t.key("Space");
        check(&mut t, "arming");
        t.conn(ConnEvent::Stimulus(StimEvent::Armed));
        check(&mut t, "armed");
        t.key("Enter");
        check(&mut t, "fire requested");
        t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
        let mut s = daemon_state();
        s.generator.owner = Some(ClientId("c1".into()));
        s.generator.armed = true;
        s.generator.firing = true;
        t.conn(mirror(s.clone()));
        check(&mut t, "firing");
        t.key("Escape");
        check(&mut t, "stopping");
        t.key("Space");
        assert!(t.st.stimulus.arm_after_stop);
        check(&mut t, "an arm queued behind the stop");
        t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
        check(&mut t, "stopped, arming again");
        t.key("Escape");
        t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
        // A sweep running on the daemon: the progress strip is an operation outside full
        // screen; in it, nothing.
        s.sweep = Some(sweep_run(SweepStatus::Playing { repeat: 1 }));
        t.conn(mirror(s));
        assert!(t.st.operation().is_some());
        check(&mut t, "a sweep running");
    }
}

/// What a measurement delete asked for: its toast text, and whether the stimulus stop went
/// with it.
fn meas_delete(r: &[Request]) -> (String, bool) {
    let what = r
        .iter()
        .find_map(|x| match x {
            Request::Call {
                cmd: Command::MeasDelete { .. },
                what,
            } => Some(what.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no measurement delete: {r:?}"));
    (what, r.iter().any(|x| matches!(x, Request::StimStop)))
}

/// Deleting the last running transfer measurement ends its measuring as a stop does: the
/// stimulus this app holds stops with it, through both delete questions; one that owns
/// nothing (the plain confirmation) and one that owns traces (keep / delete / cancel).
#[test]
fn deleting_the_last_transfer_stops_the_stimulus() {
    // The plain confirmation: Main L owns nothing.
    let mut t = T::new();
    t.put(PaneKind::Transfer);
    play_noise(&mut t, true);
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("Delete");
    assert!(
        matches!(t.st.overlay, Overlay::Delete(_)),
        "{:?}",
        t.st.overlay
    );
    assert_eq!(
        meas_delete(&t.key("Enter")),
        (
            "Main L deleted · stimulus stopped (no transfer measurement left running)".into(),
            true
        )
    );
    assert_eq!(t.st.stimulus.phase, StimPhase::Stopping);
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_ne!(t.last_toast(), "stimulus stopped");

    // The three-answer question: Main L owns traces.
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.put(PaneKind::Transfer);
    play_noise(&mut t, false);
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("Delete");
    assert!(
        matches!(t.st.overlay, Overlay::Choose(_)),
        "{:?}",
        t.st.overlay
    );
    let (what, stopped) = meas_delete(&t.key("Enter"));
    assert!(stopped, "{what}");
    assert!(
        what.ends_with(" · stimulus stopped (no transfer measurement left running)"),
        "{what}"
    );
}

/// A delete keeps the stimulus under the same rule as a stop: another transfer
/// measurement running, a spectrum, a stopped transfer measurement, another client's lease.
#[test]
fn deleting_keeps_the_stimulus_when_a_stop_would() {
    let delete = |t: &mut T, meas: u32| {
        t.st.update(Msg::SelectMeas(MeasId(meas)), &t.keys);
        t.key("Delete");
        meas_delete(&t.key("Enter"))
    };
    // Another transfer measurement still running.
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.push(meas(3, "Main R", transfer()));
    t.conn(mirror(s));
    play_noise(&mut t, true);
    assert_eq!(delete(&mut t, 1), ("Main L deleted".into(), false));
    // A spectrum.
    let mut t = T::new();
    play_noise(&mut t, true);
    assert_eq!(delete(&mut t, 2), ("Sub deleted".into(), false));
    // A stopped transfer measurement: it was not measuring.
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements[1].running = false;
    t.conn(mirror(s));
    play_noise(&mut t, true);
    assert_eq!(delete(&mut t, 1), ("Main L deleted".into(), false));
    // Another client's noise.
    let mut t = T::new();
    let mut s = daemon_state();
    s.generator.owner = Some(ClientId("other".into()));
    s.generator.armed = true;
    s.generator.firing = true;
    t.conn(mirror(s));
    assert_eq!(delete(&mut t, 1), ("Main L deleted".into(), false));
    // A sweep running.
    let mut t = T::new();
    play_noise(&mut t, true);
    t.st.sweep.run = Some(sweep_run(SweepStatus::Playing { repeat: 1 }).id);
    assert_eq!(delete(&mut t, 1), ("Main L deleted".into(), false));
}

/// A measurement's delay step is something to see on its live curve: stopped or hidden,
/// the keys send nothing and say why.
#[test]
fn delay_steps_need_the_live_curve() {
    let mut s = daemon_state();
    if let Some(m) = s.measurements.iter_mut().find(|m| m.id == MeasId(1)) {
        m.running = false;
    }
    let mut t = T::new();
    t.conn(mirror(s));
    for k in ["Ctrl+.", "Ctrl+,", "Alt+.", "Alt+,"] {
        assert!(t.key(k).is_empty(), "{k}");
        assert_eq!(
            t.last_toast(),
            "Main L is stopped \u{2014} its delay applies to the live curve; S starts it"
        );
    }
    let mut t = T::new();
    t.st.hidden_meas.insert("Main L".into());
    assert!(t.key("Ctrl+.").is_empty());
    assert!(
        t.last_toast().starts_with("Main L is hidden"),
        "{}",
        t.last_toast()
    );
}
