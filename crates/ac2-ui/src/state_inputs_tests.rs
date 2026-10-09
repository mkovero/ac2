//! Reducer tests of the link, the delay finder, inputs and mics, stimulus outputs and the calibration views.

use super::*;

#[test]
fn link_failure_clears_live_state() {
    let mut t = T::new();
    t.st.stimulus.phase = StimPhase::Armed;
    t.conn(ConnEvent::Failed {
        target: "local daemon".into(),
        error: "not responding".into(),
        retry_in: std::time::Duration::from_secs(2),
    });
    assert!(t.st.mirror.is_none());
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(!t.st.connected());
}

#[test]
fn parsers() {
    assert_eq!(parse_number(" −12,5 dBFS", &["dbfs", "db"]), Ok(-12.5));
    assert_eq!(parse_number("3ms", &["ms"]), Ok(3.0));
    assert!(parse_number("loud", &[]).is_err());
    assert!(parse_number("inf", &[]).is_err());
    assert_eq!(parse_outputs("1, 2 2"), Ok(vec![0, 1]));
    assert!(parse_outputs("0").is_err());
    assert!(parse_outputs("").is_err());
}

#[test]
fn level_change_during_arming_is_sent_once_armed() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    // ↓ while the arm is in flight: nothing to send yet.
    assert!(t.key("Down").is_empty());
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert_eq!(
        r.iter().filter_map(set_level).collect::<Vec<_>>(),
        [(-21.0, true, false)]
    );
    // Unchanged settings: the confirmation sends nothing.
    t.key("Esc");
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    t.key("Space");
    assert!(t.conn(ConnEvent::Stimulus(StimEvent::Armed)).is_empty());
}

#[test]
fn accepted_finding_inserts_what_was_asked() {
    let mut t = T::new();
    let f = || {
        finding(DelayOutcome::Accepted {
            first: arrival(12.5, -3.0),
            strongest: arrival(12.7, 0.0),
        })
    };
    let r = found(&mut t, DelayPick::FirstArrival, f());
    assert_eq!(inserted(&r), Some(DelayPick::FirstArrival));
    let r = found(&mut t, DelayPick::Strongest, f());
    assert_eq!(inserted(&r), Some(DelayPick::Strongest));
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn ambiguous_first_arrival_opens_the_candidate_list() {
    let mut t = T::new();
    t.key("X");
    let r = found(&mut t, DelayPick::FirstArrival, ambiguous());
    assert!(r.is_empty(), "{r:?}");
    let Overlay::DelayPick(c) = &t.st.overlay else {
        panic!("no candidate list: {:?}", t.st.overlay);
    };
    let rows = c.rows();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].text, "12.50 ms  −11.5 dB · rule pick");
    // Keys 1–3 pick (and do not focus panes while the list is up).
    let r = t.key("2");
    assert_eq!(inserted(&r), Some(DelayPick::Ranked { index: 1 }));
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.focus_kind(), PaneKind::Transfer);
    // 1 is the rule's pre-selection.
    found(&mut t, DelayPick::FirstArrival, ambiguous());
    let r = t.key("1");
    assert_eq!(inserted(&r), Some(DelayPick::Ranked { index: 0 }));
    // Other keys keep working with the list up; Esc closes it.
    found(&mut t, DelayPick::FirstArrival, ambiguous());
    t.key("Alt+4");
    assert_eq!(t.focus_kind(), PaneKind::Spl);
    assert!(matches!(t.st.overlay, Overlay::DelayPick(_)));
    t.key("Esc");
    assert_eq!(t.st.overlay, Overlay::None);
    // Shift+X on an ambiguous finding: the strongest is well defined, insert it.
    let r = found(&mut t, DelayPick::Strongest, ambiguous());
    assert_eq!(inserted(&r), Some(DelayPick::Strongest));
}

#[test]
fn refusal_inserts_nothing_and_says_why() {
    let mut t = T::new();
    let r = found(
        &mut t,
        DelayPick::FirstArrival,
        finding(DelayOutcome::NoEstimate {
            reasons: vec![NoEstimateReason::LowPsr],
        }),
    );
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(
        t.st.toasts
            .last()
            .is_some_and(|x| x.severity == Severity::Warning)
    );
    assert_eq!(t.last_toast(), "Main L: no delay estimate (no clear peak)");
    // The banner over the transfer pane carries the reason too.
    let mut s = daemon_state();
    if let Some(d) = s.measurements[1].delay.as_mut() {
        d.last_finding = Some(*finding(DelayOutcome::NoEstimate {
            reasons: vec![NoEstimateReason::LowPsr, NoEstimateReason::LowBandSnr],
        }));
    }
    t.conn(mirror(s));
    let m = t.st.meas(MeasId(1)).cloned().expect("meas");
    let why = ac2_scene::banner::no_delay_estimate(&m);
    let b = ac2_scene::banner::banners(&ac2_scene::banner::Status {
        no_delay_estimate: why,
        ..Default::default()
    });
    assert_eq!(
        b[0].detail.as_deref(),
        Some("finder: no clear peak, too noisy in band")
    );
}

#[test]
fn own_client_id_follows_the_daemon_incarnation() {
    let mut t = T::new();
    assert_eq!(t.st.my_client_id(), Some(&ClientId("c1".into())));
    let mut s = daemon_state();
    s.generator.owner = Some(ClientId("c1".into()));
    s.generator.armed = true;
    // The daemon restarted: the mirror shows the new incarnation, and until its welcome
    // arrives this client has no identity there; the old id is never "me".
    t.conn(mirror_of(s.clone(), 2, None));
    assert_eq!(t.st.my_client_id(), None);
    // The new welcome binds another id; an owner "c1" (another client now) is not us.
    t.conn(mirror_of(s, 2, Some("c9")));
    assert_eq!(t.st.my_client_id(), Some(&ClientId("c9".into())));
}

fn inputs_call(r: &[Request]) -> Option<Vec<InputSetup>> {
    r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::SessionInputs { inputs },
            ..
        } => Some(inputs.clone()),
        _ => None,
    })
}

#[test]
fn input_mics_prompt_sets_the_input_setup() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.inputs = vec![InputSetup {
        channel: 1,
        mic: Some("M30".into()),
        curve: CurveChoice::Off,
    }];
    t.conn(mirror(s));
    let r = t.st.update(Msg::Command(CommandId::InputMics), &t.keys);
    assert!(r.is_empty());
    match &t.st.overlay {
        Overlay::Prompt(p) => {
            assert_eq!(p.kind, PromptKind::InputMics);
            assert_eq!(p.text, "2=M30");
        }
        o => panic!("{o:?}"),
    }
    for _ in 0..3 {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text(", 3= ECM 8000 ");
    let r = t.key("Enter");
    assert_eq!(
        inputs_call(&r),
        Some(vec![
            // Another mic (here none): the old choice is dropped.
            InputSetup {
                channel: 1,
                mic: None,
                curve: CurveChoice::NotChosen,
            },
            InputSetup {
                channel: 2,
                mic: Some("ECM 8000".into()),
                curve: CurveChoice::NotChosen,
            },
        ]),
        "{r:?}"
    );
    // Bad text keeps the prompt open with the reason.
    t.st.update(Msg::Command(CommandId::InputMics), &t.keys);
    for _ in 0..5 {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("M30");
    let r = t.key("Enter");
    assert!(inputs_call(&r).is_none());
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
}

#[test]
fn mic_curve_steps_through_the_selected_measurements_input() {
    let mut t = T::new();
    // Main L (transfer): its measurement input is 1 (shown as 2). No mic name: nothing to
    // choose from.
    let r = t.st.update(Msg::Command(CommandId::MicCurve), &t.keys);
    assert_eq!(inputs_call(&r), None);
    assert!(t.last_toast().contains("no mic name"), "{}", t.last_toast());
    let mut s = daemon_state();
    s.mics = vec![mm1()];
    let row = |curve: CurveChoice| InputSetup {
        channel: 1,
        mic: Some("MM1 34804".into()),
        curve,
    };
    let label = |l: &str| CurveChoice::Curve { label: l.into() };
    // off → 0° → 90° → off, each from what the daemon holds.
    for (from, to) in [
        (CurveChoice::Off, label("0°")),
        (label("0°"), label("90°")),
        (label("90°"), CurveChoice::Off),
        (CurveChoice::NotChosen, label("0°")),
    ] {
        s.inputs = vec![row(from.clone())];
        t.conn(mirror(s.clone()));
        let r = t.st.update(Msg::Command(CommandId::MicCurve), &t.keys);
        assert_eq!(inputs_call(&r), Some(vec![row(to.clone())]), "{from:?}");
    }
    // The palette's "Mic curve on input N…": a label of the mic, or off.
    let r = prompt_text(&mut t, CommandId::MicCurveInput, "2=90°");
    assert_eq!(inputs_call(&r), Some(vec![row(label("90°"))]));
    let r = prompt_text(&mut t, CommandId::MicCurveInput, "2=off");
    assert_eq!(inputs_call(&r), Some(vec![row(CurveChoice::Off)]));
    let r = prompt_text(&mut t, CommandId::MicCurveInput, "2=45°");
    assert_eq!(inputs_call(&r), None);
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.error.as_deref().is_some_and(|e| e.contains("stored: 0°, 90°"))
    ));
}

#[test]
fn mic_text_round_trips() {
    let rows = vec![
        InputSetup {
            channel: 0,
            mic: Some("M30 #1".into()),
            curve: CurveChoice::NotChosen,
        },
        InputSetup {
            channel: 3,
            mic: Some("ECM".into()),
            curve: CurveChoice::Off,
        },
    ];
    assert_eq!(mics_text(&rows), "1=M30 #1, 4=ECM");
    assert_eq!(
        parse_mics(&mics_text(&rows)),
        Ok(vec![(0, Some("M30 #1".into())), (3, Some("ECM".into()))])
    );
    for bad in ["", "M30", "0=M30", "x=M30", "1=a, 1=b"] {
        assert!(parse_mics(bad).is_err(), "{bad:?}");
    }
}

fn with_output_device(dev: &str) -> State {
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.output_device = DeviceId(dev.into());
    }
    s
}

#[test]
fn stimulus_outputs_are_remembered_per_device() {
    // First run: output 1, no outputs to remember (the layout is, with the measurements
    // the panes show).
    let mut t = T::new();
    assert_eq!(t.st.stimulus.outputs, vec![0]);
    assert!(t.st.stimulus.describe().ends_with("→ out 1"));
    assert!(t.st.prefs.outputs.is_empty());
    t.st.prefs_dirty = false;
    // Choosing outputs (S on an output in Settings › Inputs & outputs, opened on the
    // stimulus output) remembers them for the session's output device and applies them
    // now: the open session (on the simulated rig the dialog lists) has them.
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.backend = BackendKind::Fake;
    }
    t.conn(mirror(s));
    t.st.update(Msg::Command(CommandId::StimulusOutputs), &t.keys);
    t.conn(ConnEvent::Devices(Ok(backends(true))));
    // No output ticked yet: the first one is focused; output 2 takes the stimulus.
    assert_eq!(dialog(&t).focus, Row::Output(0));
    t.key("Down");
    t.key("S");
    assert_eq!(t.st.stimulus.outputs, vec![1]);
    assert!(t.st.prefs_dirty);
    assert_eq!(t.st.prefs.outputs_for("fake:loop"), Some(&[1u16][..]));
    assert!(
        dialog(&t)
            .notice
            .as_deref()
            .is_some_and(|n| n.starts_with("the stimulus plays on 2 · ")),
        "{:?}",
        dialog(&t).notice
    );
    t.key("Escape");

    // Another device: never used, so output 1; back on the first, its outputs return.
    t.conn(mirror(with_output_device("hw:UMC1820")));
    assert_eq!(t.st.stimulus.outputs, vec![0]);
    t.conn(mirror(with_output_device("fake:loop")));
    assert_eq!(t.st.stimulus.outputs, vec![1]);

    // A later run starts from the saved preferences.
    let mut t2 = T::disconnected();
    t2.st.prefs = t.st.prefs.clone();
    t2.conn(ConnEvent::Connected {
        target: "local daemon".into(),
        server: "ac2d test".into(),
        client_id: ClientId("c1".into()),
    });
    t2.conn(mirror(daemon_state()));
    assert_eq!(t2.st.stimulus.outputs, vec![1]);
}

#[test]
fn outputs_never_change_under_a_held_stimulus() {
    let mut t = T::new();
    t.st.prefs.outputs.insert("hw:UMC1820".into(), vec![3]);
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.conn(mirror(with_output_device("hw:UMC1820")));
    assert_eq!(t.st.stimulus.outputs, vec![0]);
    // Once stopped, the device's remembered outputs apply.
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    t.conn(mirror(with_output_device("hw:UMC1820")));
    assert_eq!(t.st.stimulus.outputs, vec![3]);
}

fn cal_delete(r: &[Request]) -> Option<CalKey> {
    r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::CalDelete { key },
            ..
        } => Some(key.clone()),
        _ => None,
    })
}

#[test]
fn calibrations_are_deleted_from_the_palette() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.inputs = vec![InputSetup {
        channel: 1,
        mic: Some("M30".into()),
        curve: CurveChoice::NotChosen,
    }];
    t.conn(mirror(s));
    // Prefilled with the selected measurement's input and its mic.
    t.st.update(Msg::Command(CommandId::CalDelete), &t.keys);
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::CalDelete && p.text == "2=M30"
    ));
    let r = t.key("Enter");
    let key = CalKey {
        device: DeviceId("fake:loop".into()),
        channel: 1,
        mic: "M30".into(),
    };
    assert_eq!(cal_delete(&r), Some(key.clone()));
    let r = prompt_text(&mut t, CommandId::CalDelete, "4=ECM 8000");
    assert_eq!(
        cal_delete(&r),
        Some(CalKey {
            channel: 3,
            mic: "ECM 8000".into(),
            ..key
        })
    );
    // A mic name is required; the prompt stays with the reason.
    let r = prompt_text(&mut t, CommandId::CalDelete, "2=");
    assert!(cal_delete(&r).is_none());
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
    // Without a session there is no device to name.
    let mut s = daemon_state();
    s.session.open = None;
    t.conn(mirror(s));
    let r = prompt_text(&mut t, CommandId::CalDelete, "2=M30");
    assert!(cal_delete(&r).is_none());
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.error.as_deref().is_some_and(|e| e.contains("cal rm"))
    ));
}

fn find_request(r: &[Request]) -> Option<(FinderBand, Option<Seconds>)> {
    r.iter().find_map(|r| match r {
        Request::FindDelay {
            band, observation, ..
        } => Some((*band, *observation)),
        _ => None,
    })
}

#[test]
fn finder_band_and_observation_are_the_operators_choice() {
    let mut t = T::new();
    t.key("Alt+1");
    assert_eq!(find_request(&t.key("X")), Some((FinderBand::Auto, None)));
    t.st.update(Msg::Command(CommandId::FinderSub), &t.keys);
    assert!(t.last_toast().contains("sub band · auto observation"));
    assert_eq!(find_request(&t.key("X")), Some((FinderBand::Sub, None)));
    // The sub band observes 2, 4 or 8 s.
    prompt_text(&mut t, CommandId::FinderObservation, "3");
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
    t.key("Escape");
    prompt_text(&mut t, CommandId::FinderObservation, "8 s");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(
        t.last_toast().contains("sub band · 8 s"),
        "{}",
        t.last_toast()
    );
    assert_eq!(
        find_request(&t.key("Shift+X")),
        Some((FinderBand::Sub, Some(Seconds(8.0))))
    );
    // Another band starts from its automatic observation.
    t.st.update(Msg::Command(CommandId::FinderMid), &t.keys);
    assert_eq!(find_request(&t.key("X")), Some((FinderBand::Mid, None)));
    prompt_text(&mut t, CommandId::FinderObservation, "0,5");
    assert_eq!(
        find_request(&t.key("X")),
        Some((FinderBand::Mid, Some(Seconds(0.5))))
    );
    // Empty: automatic again; above 8 s is refused.
    prompt_text(&mut t, CommandId::FinderObservation, "");
    assert_eq!(t.st.finder.observation, None);
    prompt_text(&mut t, CommandId::FinderObservation, "9");
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
    t.key("Escape");
    // Custom edges.
    prompt_text(&mut t, CommandId::FinderCustom, "80 – 800 Hz");
    assert_eq!(
        find_request(&t.key("X")),
        Some((
            FinderBand::Custom {
                lo_hz: Hz(80.0),
                hi_hz: Hz(800.0)
            },
            None
        ))
    );
    prompt_text(&mut t, CommandId::FinderCustom, "800-80");
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
    t.key("Escape");
    // A custom band reaching below 150 Hz is a sub band: 2, 4 or 8 s.
    prompt_text(&mut t, CommandId::FinderObservation, "1");
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
    t.key("Escape");
    t.st.update(Msg::Command(CommandId::FinderAuto), &t.keys);
    assert_eq!(t.st.finder, FinderChoice::default());
}

// ----- audio session and measurement dialogs ----------------------------------------------

fn calls(r: &[Request]) -> Vec<&Command> {
    r.iter()
        .filter_map(|r| match r {
            Request::Call { cmd, .. } => Some(cmd),
            _ => None,
        })
        .collect()
}

#[test]
fn input_setup_view_steps_curves_and_manages_the_library() {
    use crate::cal_view::{CalLine, lines};
    let label = |l: &str| CurveChoice::Curve { label: l.into() };
    let mut t = T::new();
    t.conn(mirror(mm1_state(label("0°"))));
    // Main L (transfer, measuring input 2) is selected: Input setup opens on its input.
    t.st.update(Msg::Command(CommandId::Calibrations), &t.keys);
    let st = t.st.daemon().cloned().expect("state");
    assert_eq!(
        cal_view(&t).focused(&st),
        Some(CalLine::Input(1)),
        "{:?}",
        lines(&st)
    );
    // The lines: inputs 1 and 2, the two curves, the calibration.
    assert_eq!(lines(&st).len(), 5);
    let rows = crate::cal_view::line_texts(&st, WallNs(3 * 3_600_000_000_000), Default::default());
    assert_eq!(rows[1].title, "in 2 · MM1 34804");
    assert_eq!(rows[1].detail, "curve 0°");
    assert_eq!(
        rows[1].extra,
        "verified · 94.0 dB SPL at 1.00 kHz · 3 h ago"
    );
    assert_eq!(rows[2].title, "MM1 34804 0°");
    assert_eq!(rows[2].extra, "in use on in 2");
    assert_eq!(rows[3].extra, "not in use");
    assert_eq!(rows[4].title, "MM1 34804 on in 2 of fake:loop");

    // → / ←: one request each, the next / previous curve (off after the last).
    let row = |curve: CurveChoice| InputSetup {
        channel: 1,
        mic: Some("MM1 34804".into()),
        curve,
    };
    let r = t.key("ArrowRight");
    assert_eq!(inputs_call(&r), Some(vec![row(label("90°"))]));
    t.conn(mirror(mm1_state(label("90°"))));
    let r = t.key("ArrowRight");
    assert_eq!(inputs_call(&r), Some(vec![row(CurveChoice::Off)]));
    let r = t.key("ArrowLeft");
    assert_eq!(inputs_call(&r), Some(vec![row(label("0°"))]));

    // I: a curve file for this input's mic, by path.
    let r = t.type_key("I", "i");
    assert!(r.is_empty());
    t.text("/curves/449350_34804_90Grad.txt");
    let r = t.key("Enter");
    assert!(
        r.iter().any(|x| matches!(x,
            Request::ImportCurve { path, mic, input: Some(1) }
                if mic == "MM1 34804" && path.ends_with("449350_34804_90Grad.txt"))),
        "{r:?}"
    );
    assert!(cal_view(&t).edit.is_none());

    // N: another mic name on the input drops the curve choice.
    t.type_key("N", "n");
    for _ in 0.."MM1 34804".len() {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("ECM 8000");
    let r = t.key("Enter");
    assert_eq!(
        inputs_call(&r),
        Some(vec![InputSetup {
            channel: 1,
            mic: Some("ECM 8000".into()),
            curve: CurveChoice::NotChosen,
        }])
    );

    // On the 90° curve: R renames, Delete twice deletes.
    t.key("ArrowDown");
    t.key("ArrowDown");
    assert_eq!(
        cal_view(&t).focused(&st),
        Some(CalLine::Curve(MicCurveId {
            mic: "MM1 34804".into(),
            label: "90°".into()
        }))
    );
    t.type_key("R", "r");
    for _ in 0..3 {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("grazing");
    let r = t.key("Enter");
    assert!(
        matches!(
            calls(&r).as_slice(),
            [Command::CalCurveRename { curve, label }] if curve.label == "90°" && label == "grazing"
        ),
        "{r:?}"
    );
    let r = t.key("Delete");
    assert!(calls(&r).is_empty());
    assert!(
        cal_view(&t)
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("Delete again"))
    );
    let r = t.key("Delete");
    assert!(matches!(
        calls(&r).as_slice(),
        [Command::CalCurveDelete { curve }] if curve.label == "90°"
    ));
    // The sensitivity calibration; Backspace deletes as Delete does.
    t.key("ArrowDown");
    t.st.update(Msg::Backspace, &t.keys);
    let r = t.st.update(Msg::Backspace, &t.keys);
    assert!(matches!(
        calls(&r).as_slice(),
        [Command::CalDelete { key }] if key.mic == "MM1 34804" && key.channel == 1
    ));
    // ←/→ off an input line explain themselves instead of acting.
    let r = t.key("ArrowRight");
    assert!(inputs_call(&r).is_none());
    assert!(cal_view(&t).notice.is_some());
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
}

/// E on an input of the calibrations view: the electrical calibration dialog, prefilled
/// with the data sheet's sensitivity; Enter sends `cal.spl_electrical`, a refusal stays in
/// the dialog, a success closes it with what to do next.
#[test]
fn electrical_calibration_from_the_input_setup_view() {
    use ac2_proto::model::ElectricalConnection;
    use ac2_proto::units::Volts;
    let mut t = T::new();
    let mut s = mm1_state(CurveChoice::Curve {
        label: "0°".into()
    });
    s.calibrations.clear();
    for c in &mut s.mics[0].curves {
        c.stated_sensitivity = Some(15.0);
    }
    t.conn(mirror(s));
    t.st.update(Msg::Command(CommandId::Calibrations), &t.keys);
    let r = t.type_key("E", "e");
    assert!(r.is_empty());
    let d = cal_view(&t).electrical.clone().expect("the dialog");
    assert_eq!(d.title(), "Electrical calibration · in 2 · MM1 34804");
    assert_eq!(d.sensitivity, "15.0 mV/Pa");
    assert_eq!(d.sensitivity_source(), "data sheet (MM1 34804 0°)");
    assert!(d.safety().contains("pins 2 and 3"));
    // The typed letter that opened it is not in the voltage.
    assert_eq!(d.volts, "");
    t.text("15.03 mV");
    let r = t.key("Enter");
    let what = match r.as_slice() {
        [
            Request::Call {
                cmd:
                    Command::CalSplElectrical {
                        input: 1,
                        volts,
                        mic_sensitivity: None,
                        connection: ElectricalConnection::InLine,
                        replace_acoustic: false,
                        ..
                    },
                what,
            },
        ] if *volts == Volts(0.01503) => what.clone(),
        other => panic!("{other:?}"),
    };
    // Backspace edits the field, it does not delete a calibration.
    let r = t.st.update(Msg::Backspace, &t.keys);
    assert!(calls(&r).is_empty());
    t.text("V");
    // A refusal stays in the dialog; Enter again retries.
    t.conn(ConnEvent::Reply {
        what: what.clone(),
        result: Err("the tone level on input 2 is not steady yet".into()),
    });
    let d = cal_view(&t).electrical.clone().expect("still open");
    assert!(d.error.as_deref().is_some_and(|e| e.contains("not steady")));
    let r = t.key("Enter");
    assert_eq!(calls(&r).len(), 1);
    t.conn(ConnEvent::Reply {
        what,
        result: Ok(()),
    });
    assert!(cal_view(&t).electrical.is_none());
    assert!(
        cal_view(&t)
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("stored") && n.contains("unplug the meter")),
        "{:?}",
        cal_view(&t).notice
    );
    // Off an input line E explains itself.
    t.key("ArrowDown");
    t.type_key("E", "e");
    assert!(cal_view(&t).electrical.is_none());
    assert!(cal_view(&t).notice.is_some());
}

#[test]
fn input_setup_names_a_missing_curve_and_the_meter_label_follows() {
    let mut t = T::new();
    let mut s = mm1_state(CurveChoice::Curve {
        label: "45°".into(),
    });
    if let Some(o) = &mut s.session.open {
        o.config.input_channels = vec![0, 1];
    }
    t.conn(mirror(s));
    let label = |t: &T| t.st.session_inputs()[1].label.clone();
    assert_eq!(label(&t), "MM1 34804 · 45° not stored · mic (in 2)");
    assert_eq!(
        t.st.curve_note(1, false).as_deref(),
        Some("mic curve 45° not stored for MM1 34804")
    );
    t.conn(mirror(mm1_state(CurveChoice::NotChosen)));
    assert_eq!(label(&t), "MM1 34804 · curve not chosen · mic (in 2)");
    t.conn(mirror(mm1_state(CurveChoice::Curve {
        label: "90°".into(),
    })));
    assert_eq!(label(&t), "MM1 34804 · 90° · mic (in 2)");
    assert_eq!(
        t.st.curve_note(1, true).as_deref(),
        Some("mic curve: MM1 34804 90°")
    );
}

#[test]
fn session_dialog_steps_the_mic_curve_of_a_named_mic() {
    let mut t = T::new();
    // The session runs on the rig with MM1 34804 on input 2, 0° chosen.
    t.conn(mirror(mm1_state(CurveChoice::Curve {
        label: "0°".into()
    })));
    t.type_key("Shift+O", "O");
    t.conn(ConnEvent::Devices(Ok(backends(true))));
    assert!(dialog(&t).is_open_device());
    focus(&mut t, Row::Input(1));
    let st = t.st.daemon().cloned().expect("state");
    let text = dialog(&t)
        .row_cal_text(1, &st, WallNs(3 * 3_600_000_000_000), Default::default())
        .expect("named mic");
    assert_eq!(
        text,
        (
            "curve 0° · verified · 94.0 dB SPL at 1.00 kHz · 3 h ago".to_owned(),
            false
        )
    );
    // → applies at once: the session captures this mic already.
    let r = t.key("ArrowRight");
    assert_eq!(
        inputs_call(&r),
        Some(vec![InputSetup {
            channel: 1,
            mic: Some("MM1 34804".into()),
            curve: CurveChoice::Curve {
                label: "90°".into()
            },
        }])
    );
    assert_eq!(
        dialog(&t).inputs[1].curve,
        CurveChoice::Curve {
            label: "90°".into()
        }
    );
    // Another name typed: not live yet, the choice starts over and waits for Enter.
    t.type_key("N", "n");
    t.text(" B");
    t.key("Enter");
    assert_eq!(dialog(&t).inputs[1].curve, CurveChoice::NotChosen);
    let r = t.key("ArrowRight");
    assert_eq!(inputs_call(&r), None);
    assert!(
        dialog(&t)
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("when the session opens"))
    );
    // A row without a mic: nothing to choose.
    focus(&mut t, Row::Input(2));
    let r = t.key("ArrowRight");
    assert_eq!(inputs_call(&r), None);
}

/// S and R act on the measurement the focused pane shows: an SPL meter picked in the list
/// stays as it is when S is pressed on the transfer pane, which says what is missing.
#[test]
fn pane_keys_act_on_the_panes_own_measurement() {
    use ac2_proto::Command;
    let mut t = T::new();
    let mut s = with_spl();
    s.measurements
        .retain(|m| matches!(m.config.kind, MeasKind::Spl { .. }));
    s.measurements[0].running = true;
    t.conn(mirror(s));
    t.st.update(Msg::SelectMeas(MeasId(4)), &t.keys);
    t.key("Alt+1");
    let r = t.key("S");
    assert!(
        !r.iter().any(|r| matches!(
            r,
            Request::Call {
                cmd: Command::MeasStop { .. } | Command::MeasStart { .. },
                ..
            }
        )),
        "{r:?}"
    );
    assert!(
        t.last_toast().contains("no transfer measurement"),
        "{}",
        t.last_toast()
    );
    // With a transfer measurement, S on the transfer pane stops that one, not the meter.
    let mut s = with_spl();
    for m in &mut s.measurements {
        m.running = true;
    }
    t.conn(mirror(s));
    t.st.update(Msg::SelectMeas(MeasId(4)), &t.keys);
    t.key("Alt+1");
    let r = t.key("S");
    assert!(
        r.iter().any(|r| matches!(
            r,
            Request::Call { cmd: Command::MeasStop { meas }, .. } if *meas == MeasId(1)
        )),
        "{r:?}"
    );
}
