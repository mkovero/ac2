//! The stimulus and audio state: banners, following the view, output routes and max level.

use super::harness::{Driver, NAME, R, arm_new_sweep, measure_from_empty, sweep_count};
use ac2_client::{ClientConfig, Endpoints};
use ac2_proto::topic::Stream;
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::CommandId;
use ac2_ui::state::{AppState, Msg, Overlay, StimPhase};
use std::sync::Arc;
use std::time::Instant;

/// The transfer pane's legend entries now.
fn legend_texts(s: &AppState) -> Vec<String> {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(wall),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 900.0,
        height: 500.0,
    };
    ac2_ui::scenes::transfer(s, &Theme::dark(), size, now)
        .legend
        .into_iter()
        .map(|e| e.text)
        .collect()
}

/// The banners every pane shows now.
fn banner_texts(s: &AppState) -> Vec<String> {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(wall),
    };
    ac2_scene::banner::banners(&ac2_ui::scenes::status(s, &[], None, now))
        .into_iter()
        .map(|b| b.text)
        .collect()
}

/// From an empty daemon on the simulated rig: a session opened from the app; the device
/// vanishes and AUDIO STOPPED comes up with the attempts to reopen it, and once the device
/// is back the banner goes and the session is open again — nobody touched the app.
#[test]
fn audio_stopped_comes_and_goes_by_itself() -> R {
    let rig = ac2d::fake_rig()?;
    let listen = ac2d::Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let handle = ac2d::Daemon::start(ac2d::DaemonConfig::new(
        Arc::new(rig.clone()),
        listen,
        -10.0,
    ))?;
    let ep = Endpoints {
        ctrl: handle.ctrl_endpoint().to_owned(),
        data: handle.data_endpoint().to_owned(),
    };
    let mut d = Driver::connect(ClientConfig::new(ep, NAME), "local daemon")?;
    d.synced()?;
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the device list", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.session.device_info().is_some())
    })?;
    d.key("Enter");
    d.until("the session", |s| s.open_session().is_some())?;
    // The offered transfer measurement, so a curve is up when the audio stops.
    d.until("the measurement offer", |s| {
        matches!(s.overlay, Overlay::Offer(_))
    })?;
    d.key("Enter");
    d.until("the measurement, running and selected", |s| {
        s.selected_meas().is_some_and(|m| m.running)
    })?;
    let meas = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;
    d.until("a transfer frame", |s| {
        ac2_ui::scenes::frame(s, meas, Stream::Tf).is_some()
    })?;
    let epoch = d.st.daemon().ok_or("state")?.session.epoch;
    let calm = banner_texts(&d.st);
    assert!(
        !calm.iter().any(|t| t.starts_with("AUDIO STOPPED")),
        "{calm:?}"
    );

    rig.vanish(None);
    d.until("AUDIO STOPPED", |s| {
        banner_texts(s)
            .iter()
            .any(|t| t.starts_with("AUDIO STOPPED · audio host ended the stream at "))
    })?;
    d.until("a failed attempt in the top bar", |s| {
        ac2_ui::scenes::audio_stopped(s, ac2_proto::units::WallNs(0))
            .is_some_and(|t| t.detail.contains("the simulated device is gone"))
    })?;
    let bar = ac2_ui::scenes::audio_stopped(&d.st, ac2_proto::units::WallNs(0))
        .ok_or("stopped")?
        .bar;
    assert!(
        bar[0].starts_with("audio stopped · reopening (attempt "),
        "{bar:?}"
    );
    assert!(d.st.open_session().is_some(), "the session stays open");
    // Under the banner the curve says the audio stopped, not a STALE age of its own.
    d.until("the legend says the audio stopped", |s| {
        legend_texts(s)
            .iter()
            .any(|t| t.ends_with(" · audio stopped"))
    })?;
    let legend = legend_texts(&d.st);
    assert!(!legend.iter().any(|t| t.contains("STALE")), "{legend:?}");

    rig.restore();
    d.until("the banner gone and the session back", |s| {
        s.daemon()
            .is_some_and(|st| st.session.stopped.is_none() && st.session.epoch.0 > epoch.0)
            && !banner_texts(s)
                .iter()
                .any(|t| t.starts_with("AUDIO STOPPED"))
    })?;
    assert!(d.st.open_session().is_some());
    drop(d);
    handle.shutdown();
    Ok(())
}

/// The stimulus follows the view, from an empty daemon on the simulated rig: a sweep
/// measurement from the dialog, run once; then on the sweep view Space arms and Enter plays
/// it again with the same settings (a second run under it, no dialog); on the transfer view Space and Enter play pink
/// noise at the level typed for the sweep, and the transfer measurement sees the rig's path.
#[test]
fn the_stimulus_follows_the_view_from_the_app() -> R {
    use ac2_proto::model::{Signal, TraceKind, TraceSource};
    use ac2_ui::forms::FieldId;
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let meas = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;

    let sweeps_before = sweep_count(&d.st);
    d.key("Shift+S");
    d.send(Msg::Text("S".into()));
    d.until(
        "the sweep dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Sweep),
    )?;
    if let Overlay::Form(f) = &mut d.st.overlay {
        f.set_text(FieldId::Level, "-26");
        f.set_text(FieldId::From, "100 Hz");
        f.set_text(FieldId::To, "5 kHz");
        f.focus = f
            .fields
            .iter()
            .position(|x| x.id == FieldId::Duration)
            .ok_or("duration")?;
        f.cycle(-1);
    }
    d.key("Enter");
    arm_new_sweep(&mut d, sweeps_before)?;
    d.until("armed with the sweep", |s| {
        s.stimulus.phase == StimPhase::Armed
    })?;
    d.key("Enter");
    let sweeps = |s: &AppState| {
        s.daemon().map_or(Vec::new(), |x| {
            x.traces
                .iter()
                .filter(|t| t.kind == TraceKind::Sweep)
                .map(|t| (t.edit.name.clone(), t.source.clone()))
                .collect::<Vec<_>>()
        })
    };
    d.until("the first sweep stored, the stimulus off", |s| {
        sweeps(s).len() == 1
            && s.sweep.run.is_none()
            && s.stimulus.phase == StimPhase::Idle
            && s.daemon().is_some_and(|x| x.generator.owner.is_none())
    })?;
    assert_eq!(d.st.layout.focus, PaneKind::Distortion);
    let hint = |s: &AppState| {
        s.stimulus_next()
            .map(|(n, w)| ac2_scene::stimulus::hint(n, &w, "L").0)
    };
    assert_eq!(
        hint(&d.st).as_deref(),
        Some("Space arms: sweep Sweep 1 · 1 s −26 dBFS")
    );

    // The sweep view: Space arms the next run (no dialog), Enter plays it.
    d.key("Space");
    assert_eq!(d.st.overlay, Overlay::None);
    d.until("armed with the re-sweep", |s| {
        s.stimulus.phase == StimPhase::Armed
            && s.daemon().is_some_and(|x| {
                x.generator.settings.as_ref().is_some_and(|g| {
                    matches!(g.signal, Signal::Ess { .. }) && (g.level.0 + 26.0).abs() < 1e-9
                })
            })
    })?;
    assert_eq!(
        hint(&d.st).as_deref(),
        Some("Enter fires: sweep Sweep 1 · 1 s −26 dBFS")
    );
    d.key("Enter");
    d.until("the second sweep stored", |s| {
        sweeps(s).len() == 2 && s.sweep.run.is_none() && s.stimulus.phase == StimPhase::Idle
    })?;
    let all = sweeps(&d.st);
    let settings = |src: &TraceSource| match src {
        TraceSource::Sweep {
            sweep,
            level,
            repeats,
            reference_input,
            measurement_input,
            ..
        } => Some((
            *sweep,
            *level,
            *repeats,
            *reference_input,
            *measurement_input,
        )),
        _ => None,
    };
    assert_eq!((all[0].0.as_str(), all[1].0.as_str()), ("Run 1", "Run 2"));
    assert!(settings(&all[0].1).is_some());
    assert_eq!(
        settings(&all[0].1),
        settings(&all[1].1),
        "the same settings"
    );

    // The transfer view: Space and Enter play pink noise on the sweep's outputs (the
    // speaker and the loopback); the measurement gets signal.
    d.until("the lease given back", |s| {
        s.daemon().is_some_and(|x| x.generator.owner.is_none())
            && s.stimulus.phase == StimPhase::Idle
    })?;
    d.key("Alt+1");
    assert_eq!(
        hint(&d.st).as_deref(),
        Some("Space arms: pink −26 dBFS → out 2, 1")
    );
    d.key("Space");
    d.until("armed with the noise", |s| {
        s.stimulus.phase == StimPhase::Armed
    })?;
    assert_eq!(
        hint(&d.st).as_deref(),
        Some("Enter fires: pink −26 dBFS → out 2, 1")
    );
    d.key("Enter");
    d.until("pink noise playing", |s| {
        s.daemon().is_some_and(|x| {
            x.generator.firing
                && x.generator
                    .settings
                    .as_ref()
                    .is_some_and(|g| g.signal == Signal::Pink)
        })
    })?;
    d.tf_frames(meas, 240)?;
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// Settings from an empty daemon: the Audio page, then Inputs & outputs: output 1 named for
/// the rig, output 2 ticked as the only stimulus output, Enter opens; the stimulus plays on
/// output 2 alone. The system max level lowered below the playing stimulus stops it; a raise
/// is refused while armed and needs the typed word; a second client sees the new level.
#[test]
fn settings_name_an_output_route_the_stimulus_and_set_the_max_level() -> R {
    use ac2_ui::session_dialog::Row;
    use ac2_ui::settings::Page;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    let mut other = Driver::connect(daemon.client_config("second client"), &daemon.describe())?;
    d.synced()?;
    other.synced()?;
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the Audio page with the device", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.page == Page::Audio && x.session.device_info().is_some())
    })?;
    d.key("Ctrl+PageUp");
    let focus = |s: &AppState| s.overlay.settings().map(|x| (x.page, x.session.focus));
    assert_eq!(focus(&d.st), Some((Page::Io, Row::Input(0))));
    for _ in 0..4 {
        d.key("ArrowDown");
    }
    assert_eq!(focus(&d.st), Some((Page::Io, Row::Output(0))));
    // N names output 1 for the rig; S takes it off the stimulus; output 2 gets it.
    d.key("N");
    d.send(Msg::Text("n".into()));
    d.send(Msg::Text("Main L".into()));
    d.key("Enter");
    d.until("the label on the daemon", |s| {
        s.daemon()
            .is_some_and(|x| x.outputs.first().and_then(|o| o.label.as_deref()) == Some("Main L"))
    })?;
    other.until("the label on the other client", |s| {
        s.daemon().is_some_and(|x| !x.outputs.is_empty())
    })?;
    d.key("S");
    d.key("ArrowDown");
    d.key("S");
    d.key("Enter");
    d.until("the session", |s| s.open_session().is_some())?;
    d.until("the measurement offer", |s| {
        matches!(s.overlay, Overlay::Offer(_))
    })?;
    d.key("Escape");
    assert_eq!(d.st.stimulus.outputs, vec![1]);
    d.fire()?;
    let outputs =
        d.st.daemon()
            .and_then(|x| x.generator.settings.as_ref())
            .map(|g| g.outputs.clone());
    assert_eq!(outputs, Some(vec![1]), "plays on output 2 alone");

    // The max level row: ↑ from the first channel. −30 is below the playing −20: stopped.
    d.key("Ctrl+P");
    assert_eq!(d.st.overlay.settings().map(|x| x.page), Some(Page::Io));
    for _ in 0..8 {
        if d.st.overlay.settings().is_some_and(|x| x.on_ceiling) {
            break;
        }
        d.key("ArrowUp");
    }
    d.send(Msg::Text("-30".into()));
    d.key("Enter");
    d.until("lowered and stopped", |s| {
        s.daemon().is_some_and(|x| {
            x.generator.ceiling.0 == -30.0 && !x.generator.firing && !x.generator.armed
        })
    })?;
    other.until("the other client sees it", |s| {
        s.daemon().is_some_and(|x| x.generator.ceiling.0 == -30.0)
    })?;
    // Esc closes Settings; the next Esc gives the lease back, as after any remote stop.
    d.key("Escape");
    d.stop()?;

    // Armed at −40: a raise is refused here; stopped, it needs "raise".
    d.send(Msg::Command(CommandId::StimulusLevel));
    if let Overlay::Prompt(p) = &mut d.st.overlay {
        p.text.clear();
    }
    d.send(Msg::Text("-40".into()));
    d.key("Enter");
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Ctrl+P");
    d.send(Msg::Settings(ac2_ui::state::SettingsMsg::Ceiling));
    d.send(Msg::Text("-20".into()));
    d.key("Enter");
    assert_eq!(
        d.st.overlay
            .settings()
            .and_then(|x| x.ceiling.error.clone())
            .as_deref(),
        Some(ac2_scene::rig::RAISE_WHILE_LIVE)
    );
    d.key("Shift+Escape");
    d.until("disarmed", |s| {
        s.stimulus.phase == StimPhase::Idle && s.daemon().is_some_and(|x| !x.generator.armed)
    })?;
    d.key("Enter");
    assert!(
        d.st.overlay
            .settings()
            .is_some_and(|x| x.ceiling.confirm.is_some()),
        "the raise asks first"
    );
    d.send(Msg::Text("raise".into()));
    d.key("Enter");
    d.until("raised", |s| {
        s.daemon().is_some_and(|x| x.generator.ceiling.0 == -20.0)
    })?;
    other.until("the other client sees the raise", |s| {
        s.daemon().is_some_and(|x| {
            x.generator.ceiling.0 == -20.0
                && x.generator.last_action.as_ref().map(|a| a.action)
                    == Some(ac2_proto::model::GenAction::CeilingRaised)
        })
    })?;
    Ok(())
}
