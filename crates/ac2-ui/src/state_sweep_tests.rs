//! Reducer tests of sweeps: the dialog, the sweep panes and Space on each view.

use super::*;

/// The sweep measurement the dialog makes in these tests, as the mirror lists it.
fn sweep_measurement() -> Measurement {
    meas(
        5,
        "Sweep 1",
        MeasKind::Sweep {
            config: SweepConfig {
                reference_input: 0,
                measurement_input: 1,
                outputs: vec![0],
                level: Dbfs(-50.0),
                sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
                repeats: 1,
                gate: None,
                tail: Some(Seconds(1.0)),
                lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
            },
        },
    )
}

/// The daemon's state with the sweep measurement, the lease this client's.
fn sweep_state() -> State {
    let mut s = daemon_state();
    s.measurements.push(sweep_measurement());
    s.generator.owner = Some(ClientId("c1".into()));
    s
}

/// The dialog makes the sweep measurement (−50 dBFS, 3 s, out 1); the mirror lists it;
/// Space on the sweep pane arms its run.
fn sweep_armed(t: &mut T) {
    t.type_key("Shift+S", "S");
    let Overlay::Form(f) = &mut t.st.overlay else {
        panic!("no dialog");
    };
    f.set_text(crate::forms::FieldId::Level, "-50");
    assert!(f.set_channel(crate::forms::FieldId::Reference, 0));
    let r = t.key("Enter");
    assert!(
        r.iter().any(|x| matches!(x, Request::CreateMeas { .. })),
        "{r:?}"
    );
    t.conn(ConnEvent::MeasCreated(Box::new(sweep_measurement())));
    t.conn(mirror(sweep_state()));
    assert!(t.st.sweep_view());
    let r = t.key("Space");
    assert!(
        r.iter().any(|x| matches!(x, Request::StimArm { .. })),
        "{r:?}"
    );
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
}

/// The sweep from the palette's dialog to the distortion pane, keyboard only: the dialog
/// creates a sweep measurement and arms nothing, Space on the sweep pane arms its run,
/// Enter plays it, the stored result opens the pane.
#[test]
fn sweep_from_the_dialog_to_the_distortion_pane() {
    let mut t = T::new();
    assert!(
        !t.st.layout.is_shown(PaneKind::Distortion),
        "hidden until a sweep"
    );
    t.type_key("Shift+S", "S");
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("no dialog: {:?}", t.st.overlay);
    };
    assert_eq!(f.kind, FormKind::Sweep);
    // Inputs by name, the mic as the measurement, the speaker output. The session declares
    // no loopback: the reference is not guessed.
    assert_eq!(f.channel(crate::forms::FieldId::Reference), None);
    assert_eq!(f.channel(crate::forms::FieldId::Measurement), Some(1));
    assert_eq!(f.channel(crate::forms::FieldId::Output), Some(0));
    assert_eq!(f.text(crate::forms::FieldId::Name), "Sweep 1");
    // No reference, no sweep: the dialog asks for it and stays.
    assert!(t.key("Enter").is_empty());
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("dialog closed");
    };
    assert!(
        f.error
            .as_deref()
            .is_some_and(|e| e.contains("choose the reference")),
        "{:?}",
        f.error
    );
    // The reference field has the focus: → picks the first input.
    t.key("Right");
    // No level, no sweep: the dialog says so and stays.
    assert!(t.key("Enter").is_empty());
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("dialog closed");
    };
    assert!(
        f.error.as_deref().is_some_and(|e| e.contains("level")),
        "{:?}",
        f.error
    );
    let at = f
        .fields
        .iter()
        .position(|x| x.id == crate::forms::FieldId::Level)
        .expect("level field");
    for _ in 0..at {
        t.key("Down");
    }
    t.text("-50");
    let r = t.key("Enter");
    let c = r
        .iter()
        .find_map(|x| match x {
            Request::CreateMeas { config } => Some(config.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no measurement made: {r:?}"));
    let MeasKind::Sweep { config: sc } = &c.kind else {
        panic!("{c:?}");
    };
    assert_eq!(c.name, "Sweep 1");
    assert_eq!(sc.level, Dbfs(-50.0));
    assert_eq!(sc.outputs, vec![0]);
    assert_eq!((sc.reference_input, sc.measurement_input), (0, 1));
    assert_eq!(sc.repeats, 1);
    assert!(
        !r.iter().any(|x| matches!(x, Request::StimArm { .. })),
        "creating it arms nothing: {r:?}"
    );
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.layout.focus, PaneKind::Distortion);
    // Listed, it waits; Space on the sweep pane arms its run.
    assert_eq!(c.kind, sweep_measurement().config.kind);
    t.conn(ConnEvent::MeasCreated(Box::new(sweep_measurement())));
    t.conn(mirror(sweep_state()));
    assert_eq!(t.st.sweep_meas().map(|m| m.id), Some(MeasId(5)));
    let r = t.key("Space");
    let settings = r
        .iter()
        .find_map(|x| match x {
            Request::StimArm { settings, force } => {
                assert!(!force);
                Some(settings.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no arm: {r:?}"));
    assert_eq!(settings.level, Dbfs(-50.0));
    assert_eq!(settings.outputs, vec![0]);
    assert!(matches!(settings.signal, Signal::Ess { sweep } if sweep.start == Hz(20.0)));
    assert_eq!(t.st.overlay, Overlay::None);
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert!(
        t.last_toast().contains("Enter plays the sweep"),
        "{}",
        t.last_toast()
    );

    // Enter plays it: `sweep.run` of the measurement under the held lease, its settings
    // as stored (nothing changed while armed).
    let r = t.key("Enter");
    match r.as_slice() {
        [
            Request::Sweep {
                meas,
                label,
                update,
            },
        ] => {
            assert_eq!(*meas, MeasId(5));
            assert_eq!(label, "Sweep 1");
            assert!(update.is_none());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(t.st.stimulus.phase, StimPhase::FireRequested);
    t.conn(ConnEvent::Stimulus(StimEvent::SweepStarted(Box::new(
        sweep_run(SweepStatus::Playing { repeat: 1 }),
    ))));
    assert_eq!(t.st.stimulus.phase, StimPhase::Firing);

    // Recorded: the daemon has disarmed; the stimulus is off while the analysis runs.
    let mut s = sweep_state();
    s.sweep = Some(sweep_run(SweepStatus::Analysing));
    let r = t.conn(mirror(s.clone()));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(!t.st.stimulus_live(), "STIM OFF while analysing");
    assert!(
        !r.iter().any(|x| matches!(x, Request::StimStop)),
        "a stop now would abort the analysis: {r:?}"
    );
    // Stored: the pane appears with it, focused. Nothing is armed again: the lease is given
    // back quietly and sweep mode ends; Shift+S sets up the next one.
    s.traces = vec![sweep_meta(7)];
    s.sweep = Some(sweep_run(SweepStatus::Done { trace: TraceId(7) }));
    let r = t.conn(mirror(s));
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
    assert!(t.st.layout.is_shown(PaneKind::Distortion));
    assert_eq!(t.st.layout.focus, PaneKind::Distortion);
    assert!(t.st.sweep.plan.is_none());
    assert_eq!(t.st.stimulus.signal, Signal::Pink);
    assert!(
        t.last_toast().contains("Space runs it again"),
        "{}",
        t.last_toast()
    );
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(!t.st.stimulus_live(), "STIM OFF after the sweep");
    assert!(
        t.last_toast().contains("Run 1 of Sweep 1 stored"),
        "{}",
        t.last_toast()
    );
    // Enter does not play anything: nothing is armed.
    let r = t.key("Enter");
    assert!(
        !r.iter()
            .any(|x| matches!(x, Request::Sweep { .. } | Request::StimSet(_))),
        "{r:?}"
    );
    assert_eq!(t.st.sweep.shown, Some(TraceId(7)));
    let (d, g) = sweep_data(7);
    t.conn(ConnEvent::Trace(d, g));
    assert_eq!(t.st.shown_sweep().map(|(d, _)| d.meta.id), Some(TraceId(7)));
    // The sweep is drawn like a transfer function in the transfer pane too.
    assert_eq!(t.st.view.distortion.unit, DistortionUnit::Db);
    t.key("U");
    assert_eq!(t.st.view.distortion.unit, DistortionUnit::Percent);
    // The pane's dB | % toggle: a click on either sets that unit (again: stays).
    for unit in [
        DistortionUnit::Db,
        DistortionUnit::Db,
        DistortionUnit::Percent,
    ] {
        let r = t.st.update(Msg::DistortionUnit(unit), &t.keys);
        assert!(r.is_empty(), "display only: {r:?}");
        assert_eq!(t.st.view.distortion.unit, unit);
    }
    // G steps the views: the impulse response, the room parameters, back; Shift+I goes
    // to the IR and back; Shift+G steps the IR's scale.
    use ac2_scene::view::SweepMode;
    assert_eq!(t.st.view.distortion.mode, SweepMode::Response);
    t.key("G");
    assert_eq!(t.st.view.distortion.mode, SweepMode::Ir);
    t.key("G");
    assert_eq!(t.st.view.distortion.mode, SweepMode::Room);
    assert_eq!(t.st.layout_prefs().sweep_view, SweepMode::Room);
    t.key("G");
    assert_eq!(t.st.view.distortion.mode, SweepMode::Response);
    t.key("Shift+I");
    assert_eq!(t.st.view.distortion.mode, SweepMode::Ir);
    t.key("Shift+G");
    assert_eq!(t.st.view.ir.mode, IrMode::Log);
    t.key("Shift+I");
    assert_eq!(t.st.view.distortion.mode, SweepMode::Response);

    // Shift+W hides the pane again.
    t.key("Shift+W");
    assert!(!t.st.layout.is_shown(PaneKind::Distortion));
    assert_eq!(t.st.layout.focus, PaneKind::Transfer);
}

/// A sweep submitted right after Esc, while that stop is still on its way, arms once the
/// stop has landed instead of being dropped with it.
#[test]
fn the_sweep_dialog_offers_fine_lf_harmonics() {
    use crate::forms::FieldId;
    let mut t = T::new();
    t.type_key("Shift+S", "S");
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("no dialog: {:?}", t.st.overlay);
    };
    let lf = f
        .fields
        .iter()
        .find(|x| x.id == FieldId::LfHarmonics)
        .expect("LF harmonics field");
    assert_eq!(lf.label, "LF harmonics");
    assert_eq!(
        lf.hint,
        "fine: finer low-frequency harmonics, higher floor there, longer silence after the sweep"
    );
    let crate::forms::Value::Choice { options, index } = &lf.value else {
        panic!("{:?}", lf.value);
    };
    assert_eq!(
        (options.as_slice(), *index),
        (&["standard".to_owned(), "fine".to_owned()][..], 0)
    );
    let pos = |f: &crate::forms::Form, id| f.fields.iter().position(|x| x.id == id).expect("field");
    let (level, lf) = (pos(f, FieldId::Level), pos(f, FieldId::LfHarmonics));
    // The reference has the focus: → picks the first input.
    t.key("Right");
    for _ in 0..level {
        t.key("Down");
    }
    t.text("-50");
    for _ in level..lf {
        t.key("Down");
    }
    t.key("Right");
    let r = t.key("Enter");
    let c = r
        .iter()
        .find_map(|x| match x {
            Request::CreateMeas { config } => Some(config.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no measurement made: {r:?}"));
    let MeasKind::Sweep { config: sc } = &c.kind else {
        panic!("{c:?}");
    };
    assert_eq!(sc.lf_harmonics, ac2_proto::model::LfHarmonics::Fine);
    // Editing it shows the setting it was made with.
    let m = Measurement {
        config: c.clone(),
        ..sweep_measurement()
    };
    let (inputs, outputs) = (t.st.session_input_names(), t.st.session_output_names());
    let e = crate::forms::Form::edit_sweep(&m, t.st.open_session(), &inputs, &outputs)
        .expect("edit dialog");
    assert_eq!(
        e.fields
            .iter()
            .find(|x| x.id == FieldId::LfHarmonics)
            .map(|x| &x.value),
        Some(&crate::forms::Value::Choice {
            options: vec!["standard".into(), "fine".into()],
            index: 1
        })
    );
}

#[test]
fn a_sweep_submitted_during_a_stop_arms_after_it() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-30.0));
    let r = t.key("Space");
    assert!(matches!(r.as_slice(), [Request::StimArm { .. }]), "{r:?}");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    let r = t.key("Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
    assert_eq!(t.st.stimulus.phase, StimPhase::Stopping);

    t.conn(mirror(sweep_state()));
    t.key("Alt+5");
    let r = t.key("Space");
    assert!(
        !r.iter().any(|x| matches!(x, Request::StimArm { .. })),
        "nothing armed into the lease being released: {r:?}"
    );
    assert!(t.st.sweep.plan.is_some());
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    let settings = r
        .iter()
        .find_map(|x| match x {
            Request::StimArm { settings, .. } => Some(settings.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no arm after the stop: {r:?}"));
    assert!(matches!(settings.signal, Signal::Ess { .. }));
    assert_eq!(settings.level, Dbfs(-50.0));
    assert_eq!(t.st.stimulus.phase, StimPhase::Arming);
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert!(
        t.last_toast().contains("Enter plays the sweep"),
        "{}",
        t.last_toast()
    );
}

#[test]
fn a_failed_sweep_says_why_and_disarms() {
    let mut t = T::new();
    sweep_armed(&mut t);
    t.key("Enter");
    t.conn(ConnEvent::Stimulus(StimEvent::SweepStarted(Box::new(
        sweep_run(SweepStatus::Playing { repeat: 1 }),
    ))));
    let mut s = sweep_state();
    s.sweep = Some(sweep_run(SweepStatus::Failed {
        reason: SweepFailure::NoReference,
        msg: "the reference input carries no sweep".into(),
    }));
    let r = t.conn(mirror(s));
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert!(
        t.last_toast().contains("carries no sweep"),
        "{}",
        t.last_toast()
    );
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(!t.st.stimulus_live());
    // The sweep measurement stays, waiting for the next Space.
    assert_eq!(t.st.sweep_meas().map(|m| m.id), Some(MeasId(5)));
}

/// The top bar's autosave indicator follows the daemon's `autosave` entity: nothing when the
/// daemon does not autosave, its age once saved, `saving…` while a change waits, the
/// failure as a warning.
#[test]
fn autosave_indicator_follows_the_daemon() {
    use ac2_scene::autosave::AutosaveTone;
    const S: u64 = 1_000_000_000;
    let now = WallNs(1_000 * S);
    assert_eq!(T::disconnected().st.autosave_label(now), None);
    let mut t = T::new();
    assert_eq!(
        t.st.autosave_label(now),
        None,
        "the daemon does not autosave"
    );

    let mut st = daemon_state();
    st.autosave = Autosave {
        state: AutosaveState::Saved,
        saved_at: Some(WallNs(1_000 * S - 3 * S)),
    };
    t.conn(mirror(st.clone()));
    let l = t.st.autosave_label(now).expect("label");
    assert_eq!(
        (l.text.as_str(), l.tone),
        ("autosaved just now", AutosaveTone::Quiet)
    );

    st.autosave.state = AutosaveState::Pending;
    t.conn(mirror(st.clone()));
    assert_eq!(t.st.autosave_label(now).expect("label").text, "saving…");

    st.autosave.state = AutosaveState::Failed {
        reason: "disk full".into(),
    };
    t.conn(mirror(st));
    let l = t.st.autosave_label(now).expect("label");
    assert_eq!(
        (l.text.as_str(), l.tone),
        ("autosave failed: disk full", AutosaveTone::Warning)
    );
}

/// The top bar's hint for the stimulus keys, as the scene words it.
fn next_hint(t: &T) -> Option<String> {
    t.st.stimulus_next()
        .map(|(n, w)| ac2_scene::stimulus::hint(n, &w, "L").0)
}

/// The sweep measurement from the dialog (−50 dBFS, 3 s, out 1), run once and stored as
/// trace 7: its settings.
fn sweep_once(t: &mut T) -> SweepConfig {
    sweep_stored_as(t, 7)
}

/// The sweep measurement from the dialog, run and stored as trace `id`: its settings.
fn sweep_stored_as(t: &mut T, id: u32) -> SweepConfig {
    if t.st.sweep_meas().is_none() {
        sweep_armed(t);
    } else {
        t.key("Space");
        t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    }
    assert_eq!(
        next_hint(t).as_deref(),
        Some("Enter fires: sweep Sweep 1 · 3 s −50 dBFS")
    );
    let r = t.key("Enter");
    let [Request::Sweep { meas, update, .. }] = r.as_slice() else {
        panic!("{r:?}");
    };
    assert_eq!((*meas, update.is_none()), (MeasId(5), true));
    let MeasKind::Sweep { config: request } = sweep_measurement().config.kind else {
        unreachable!()
    };
    t.conn(ConnEvent::Stimulus(StimEvent::SweepStarted(Box::new(
        sweep_run(SweepStatus::Playing { repeat: 1 }),
    ))));
    let mut s = sweep_state();
    s.traces = (7..=id).map(sweep_meta).collect();
    s.sweep = Some(sweep_run(SweepStatus::Done { trace: TraceId(id) }));
    t.conn(mirror(s));
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    request
}

/// A finished sweep frames the sweep pane's level axis once its data is in, as Shift+Home
/// would (frequency untouched); a zoom after that stays until the next result, and a
/// result fetched again does not refit.
#[test]
fn a_finished_sweep_fits_the_sweep_panes_level_axis() {
    let mut t = T::new();
    let start = t.st.view.distortion.range_db;
    let freq = t.st.view.freq;
    sweep_once(&mut t);
    // Not before its data: there is nothing to fit yet.
    assert_eq!(t.st.view.distortion.range_db, start);
    let (d, g) = sweep_data(7);
    t.conn(ConnEvent::Trace(d.clone(), g.clone()));
    let fitted = t.st.view.distortion.range_db;
    assert_ne!(fitted, start);
    // Harmonics at −40 dB over floors at −80 dB, framed with a margin.
    assert!(fitted.lo < -80.0 && fitted.lo > -100.0, "{fitted:?}");
    assert!(fitted.hi > -40.0 && fitted.hi < -20.0, "{fitted:?}");
    assert_eq!(t.st.view.freq, freq);
    // The operator zooms; the same result fetched again keeps the zoom.
    t.key("Ctrl+I");
    let zoomed = t.st.view.distortion.range_db;
    assert_ne!(zoomed, fitted);
    t.conn(ConnEvent::Trace(d, g));
    assert_eq!(t.st.view.distortion.range_db, zoomed);
    // The next sweep's result is framed again.
    sweep_stored_as(&mut t, 8);
    let (d, g) = sweep_data(8);
    t.conn(ConnEvent::Trace(d, g));
    assert_eq!(t.st.view.distortion.range_db, fitted);
}

/// On the sweep view Space arms a run of the sweep measurement with its settings and Enter
/// plays it, without the dialog; the top bar names it before it plays.
#[test]
fn space_on_the_sweep_view_re_sweeps_with_the_same_parameters() {
    let mut t = T::new();
    let first = sweep_once(&mut t);
    assert_eq!(t.st.layout.focus, PaneKind::Distortion);
    assert!(t.st.sweep_view());
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Space arms: sweep Sweep 1 · 3 s −50 dBFS")
    );
    let r = t.key("Space");
    assert_eq!(t.st.overlay, Overlay::None, "no dialog");
    let settings = match r.as_slice() {
        [Request::StimArm { settings, force }] => {
            assert!(!force);
            settings.clone()
        }
        other => panic!("{other:?}"),
    };
    assert!(matches!(settings.signal, Signal::Ess { sweep } if sweep == first.sweep));
    assert_eq!(settings.level, Dbfs(-50.0));
    assert_eq!(settings.outputs, first.outputs);
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Enter fires: sweep Sweep 1 · 3 s −50 dBFS")
    );
    // A level changed while armed is the measurement's from now on: stored before the run.
    t.key("Down");
    let r = t.key("Enter");
    match r.as_slice() {
        [
            Request::Sweep {
                meas,
                label,
                update,
            },
        ] => {
            assert_eq!((*meas, label.as_str()), (MeasId(5), "Sweep 1"));
            let Some(MeasKind::Sweep { config }) = update.as_ref().map(|u| &u.kind) else {
                panic!("{update:?}");
            };
            assert_eq!(config.level, Dbfs(-51.0));
            assert_eq!(config.sweep, first.sweep, "the same sweep");
        }
        other => panic!("{other:?}"),
    }
    // Its usual flow: stored, selected, the lease given back, nothing armed again.
    t.conn(ConnEvent::Stimulus(StimEvent::SweepStarted(Box::new(
        sweep_run(SweepStatus::Playing { repeat: 1 }),
    ))));
    let mut s = sweep_state();
    s.traces = vec![sweep_meta(7), sweep_meta(8)];
    s.sweep = Some(sweep_run(SweepStatus::Done { trace: TraceId(8) }));
    let r = t.conn(mirror(s));
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
    assert_eq!(t.st.selected_trace, Some(TraceId(8)));
    // Space while that stop is on its way: the re-sweep arms once it has landed.
    assert!(t.key("Space").is_empty());
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert!(
        r.iter().any(|x| matches!(
            x,
            Request::StimArm { settings, .. } if matches!(settings.signal, Signal::Ess { .. })
        )),
        "{r:?}"
    );
}

/// On the sweep view with no sweep yet, Space opens the sweep dialog.
#[test]
fn space_on_the_sweep_view_without_a_sweep_opens_the_dialog() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-40.0));
    t.key("Alt+5");
    assert!(t.st.sweep_view(), "{:?}", t.st.layout.focus);
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Space sets up a sweep measurement")
    );
    let r = t.key("Space");
    assert!(
        !r.iter().any(|x| matches!(x, Request::StimArm { .. })),
        "{r:?}"
    );
    assert!(
        matches!(&t.st.overlay, Overlay::Form(f) if f.kind == FormKind::Sweep),
        "{:?}",
        t.st.overlay
    );
}

/// On the transfer, spectrum and spectrograph views Space/Enter drive the generator (pink
/// at the operator's level), never the sweep; a re-sweep armed on the sweep view and still
/// silent turns into the noise on Space there.
#[test]
fn space_on_the_live_views_drives_the_generator() {
    let mut t = T::new();
    sweep_once(&mut t);
    for (key, pane) in [("Alt+1", PaneKind::Transfer), ("Alt+2", PaneKind::Spectrum)] {
        t.key(key);
        assert_eq!(t.st.layout.focus, pane);
        assert_eq!(
            next_hint(&t).as_deref(),
            Some("Space arms: pink −50 dBFS → out 1"),
            "{pane:?}"
        );
    }
    // The spectrograph is the spectrum pane's: the same rule.
    t.key("G");
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Space arms: pink −50 dBFS → out 1")
    );
    t.key("Alt+1");
    let r = t.key("Space");
    match r.as_slice() {
        [Request::StimArm { settings, .. }] => {
            assert_eq!(settings.signal, Signal::Pink);
            assert_eq!(settings.level, Dbfs(-50.0));
        }
        other => panic!("{other:?}"),
    }
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Enter fires: pink −50 dBFS → out 1")
    );
    // Over to the sweep view while armed and silent: Space there re-sets it to the re-sweep.
    t.key("Alt+5");
    let r = t.key("Space");
    assert!(
        r.iter().any(|x| matches!(
            x,
            Request::StimSet(d)
                if matches!(d.settings.signal, Signal::Ess { .. }) && d.armed && !d.firing
        )),
        "{r:?}"
    );
    assert_eq!(
        next_hint(&t).as_deref(),
        Some("Enter fires: sweep Sweep 1 · 3 s −50 dBFS")
    );
    // And back: Space on the transfer view makes it the noise again; Enter fires the noise.
    t.key("Alt+1");
    let r = t.key("Space");
    assert!(
        r.iter().any(|x| matches!(
            x,
            Request::StimSet(d) if d.settings.signal == Signal::Pink && d.armed && !d.firing
        )),
        "{r:?}"
    );
    assert!(t.st.sweep.plan.is_none());
    let r = t.key("Enter");
    assert!(
        r.iter().any(|x| matches!(
            x,
            Request::StimSet(d) if d.settings.signal == Signal::Pink && d.firing
        )),
        "{r:?}"
    );
    assert!(!r.iter().any(|x| matches!(x, Request::Sweep { .. })));
}

/// G in the spectrum pane steps its views: spectrum → spectrum + spectrograph →
/// spectrograph → spectrum. The history is kept from the split to the spectrograph alone and
/// dropped with it; the view is remembered.
#[test]
fn g_steps_the_spectrum_panes_views() {
    use ac2_scene::view::SpectrumMode;
    let mut t = T::new();
    t.key("Alt+2");
    assert_eq!(t.st.view.spectrum.mode, SpectrumMode::Spectrum);
    t.key("G");
    assert_eq!(t.st.view.spectrum.mode, SpectrumMode::Split);
    t.st.spectrographs.insert(
        MeasId(2),
        ac2_scene::spectrograph::SpectrographHistory::new(30),
    );
    t.key("G");
    assert_eq!(t.st.view.spectrum.mode, SpectrumMode::Spectrograph);
    assert!(!t.st.spectrographs.is_empty(), "the history stays");
    t.key("W");
    assert_eq!(t.st.layout.visible(), [PaneKind::Spectrum]);
    assert_eq!(
        t.st.layout_prefs().spectrum_view,
        SpectrumMode::Spectrograph
    );
    t.key("W");
    t.key("G");
    assert_eq!(t.st.view.spectrum.mode, SpectrumMode::Spectrum);
    assert!(
        t.st.spectrographs.is_empty(),
        "nothing kept for a hidden one"
    );

    // Remembered: a new app on these preferences opens on the spectrograph.
    let mut prefs = crate::prefs::UiPrefs::default();
    prefs.layout.spectrum_view = SpectrumMode::Spectrograph;
    let mut u = T::new();
    u.st.set_prefs(prefs);
    assert_eq!(u.st.view.spectrum.mode, SpectrumMode::Spectrograph);
}
