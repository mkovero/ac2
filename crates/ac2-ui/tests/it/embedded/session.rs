//! Daemons and sessions: local and autosaving daemons, setups, backends, restarts.

use super::harness::{DEADLINE, Driver, NAME, R, measure_from_empty};
use ac2_client::{ClientConfig, Endpoints};
use ac2_proto::model::MeasKind;
use ac2_proto::topic::Topic;
use ac2_ui::embedded::{
    EmbeddedBackend, EmbeddedError, Setup, start_embedded, start_embedded_with,
};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::CommandId;
use ac2_ui::state::{Msg, Overlay};
use std::time::{Duration, Instant};

#[test]
fn empty_local_daemon_measures_from_the_app() -> R {
    // A stand-alone daemon as `ac2 daemon start` runs it, here on the simulated rig.
    let dir = tempfile::tempdir()?;
    #[cfg(unix)]
    let listen = ac2d::Listen::Local {
        ctrl: format!("ipc://{}", dir.path().join("ctrl.sock").display()),
        data: format!("ipc://{}", dir.path().join("data.sock").display()),
    };
    #[cfg(not(unix))]
    let listen = ac2d::Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let audio = ac2d::backend(ac2d::BackendChoice::Fake)?;
    let mut config = ac2d::DaemonConfig::new(audio, listen, -10.0);
    config.session_dir = dir.path().join("sessions");
    let handle = ac2d::Daemon::start(config)?;
    let ep = Endpoints {
        ctrl: handle.ctrl_endpoint().to_owned(),
        data: handle.data_endpoint().to_owned(),
    };
    let mut d = Driver::connect(ClientConfig::new(ep, NAME), "local daemon")?;
    measure_from_empty(&mut d)?;
    drop(d);
    handle.shutdown();
    Ok(())
}

/// From an empty daemon whose rig is listed as an input-only and an output-only device (as
/// WASAPI lists one interface): the dialog picks the output device by itself, the session
/// opens with the two, and the stimulus routed to the output device's output 1 reaches the
/// measurement (the simulated rig only: nothing plays on hardware).
#[test]
fn a_session_plays_on_another_device_than_it_captures_from() -> R {
    use ac2_proto::model::ClockRelation;
    let dir = tempfile::tempdir()?;
    let listen = ac2d::Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let audio = std::sync::Arc::new(ac2d::fake_rig_endpoints()?);
    let mut config = ac2d::DaemonConfig::new(audio, listen, -10.0);
    config.session_dir = dir.path().join("sessions");
    let handle = ac2d::Daemon::start(config)?;
    let ep = Endpoints {
        ctrl: handle.ctrl_endpoint().to_owned(),
        data: handle.data_endpoint().to_owned(),
    };
    let mut d = Driver::connect(ClientConfig::new(ep, NAME), "local daemon")?;
    measure_from_empty(&mut d)?;
    let open = d.st.open_session().cloned().ok_or("session")?;
    assert_eq!(open.input_device.0, "fake:in");
    assert_eq!(open.output_device.0, "fake:out");
    assert_eq!(open.config.output_channels, 2);
    assert_eq!(open.clock, ClockRelation::Unknown);
    let gen_outputs =
        d.st.daemon()
            .and_then(|s| s.generator.settings.as_ref())
            .map(|g| g.outputs.clone())
            .ok_or("generator settings")?;
    assert_eq!(gen_outputs, vec![0]);
    drop(d);
    handle.shutdown();
    Ok(())
}

/// A stand-alone daemon on the simulated rig with an autosave directory.
fn autosaving_daemon(dir: &std::path::Path) -> R<(ac2d::Handle, Endpoints)> {
    let listen = ac2d::Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let audio = ac2d::backend(ac2d::BackendChoice::Fake)?;
    let mut config = ac2d::DaemonConfig::new(audio, listen, -10.0);
    config.session_dir = dir.join("sessions");
    config.autosave = Some(ac2d::AutosaveConfig {
        dir: dir.join("autosave"),
        restore: true,
    });
    let handle = ac2d::Daemon::start(config)?;
    let ep = Endpoints {
        ctrl: handle.ctrl_endpoint().to_owned(),
        data: handle.data_endpoint().to_owned(),
    };
    Ok((handle, ep))
}

/// From an empty autosaving daemon: measure, capture to slot 1 from the keys, the top bar
/// says it is autosaved; the daemon restarts and the trace is back in its slot, nothing
/// armed, and the bar still says when it was saved.
#[test]
fn a_captured_trace_survives_a_daemon_restart() -> R {
    let now = || {
        ac2_proto::units::WallNs(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64),
        )
    };
    let dir = tempfile::tempdir()?;
    let (handle, ep) = autosaving_daemon(dir.path())?;
    let mut d = Driver::connect(ClientConfig::new(ep, NAME), "local daemon")?;
    d.synced()?;
    assert_eq!(
        d.st.autosave_label(now()).map(|l| l.text),
        Some("autosave on".to_owned())
    );
    measure_from_empty(&mut d)?;
    d.send(Msg::Command(CommandId::Slot1));
    d.until("the trace in slot 1", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().any(|t| t.edit.slot == Some(1)))
    })?;
    d.until("autosaved", |s| {
        s.daemon().is_some_and(|x| {
            x.autosave.state == ac2_proto::model::AutosaveState::Saved
                && x.autosave.saved_at.is_some()
        })
    })?;
    let label = d.st.autosave_label(now()).ok_or("no indicator")?;
    assert_eq!(label.text, "autosaved just now");
    let traces = d.st.daemon().ok_or("state")?.traces.clone();
    drop(d);
    handle.shutdown();

    let (handle, ep) = autosaving_daemon(dir.path())?;
    let mut d = Driver::connect(ClientConfig::new(ep, NAME), "local daemon")?;
    d.synced()?;
    let st = d.st.daemon().ok_or("state")?;
    assert_eq!(st.traces, traces);
    assert!(st.session.open.is_none());
    assert!(!st.generator.armed && !st.generator.firing);
    assert_eq!(st.measurements.len(), 1);
    assert_eq!(
        d.st.autosave_label(now()).map(|l| l.text),
        Some("autosaved just now".to_owned())
    );
    drop(d);
    handle.shutdown();
    Ok(())
}

#[test]
fn demo_setup_is_for_the_simulated_rig_only() {
    for b in [EmbeddedBackend::Cpal, EmbeddedBackend::Jack] {
        match start_embedded_with(b, Setup::Demo) {
            Err(EmbeddedError::Setup(_)) => {}
            other => panic!("{b:?}: {other:?}"),
        }
    }
}

/// One real backend per platform: JACK on Linux (no ALSA), the system audio elsewhere. The
/// other platform's is refused with what to use instead, never swapped for something else.
#[test]
fn the_platform_audio_is_the_only_real_backend() {
    let (native, other) = if cfg!(target_os = "linux") {
        (EmbeddedBackend::Jack, EmbeddedBackend::Cpal)
    } else {
        (EmbeddedBackend::Cpal, EmbeddedBackend::Jack)
    };
    assert_eq!(EmbeddedBackend::platform(), native);
    match start_embedded(other) {
        Err(EmbeddedError::Backend(e)) => assert!(e.contains("--backend"), "{e}"),
        other => panic!("{other:?}"),
    }
}

/// The session dialog's meters come back at once every time it is closed and opened again
/// (Esc, Shift+O, with no frame in between). Its stop and its new preview reach the daemon
/// in that order; were they to cross, the daemon would close the new preview and the meters
/// would stay blank until a renewal.
///
/// The reducer's clock stands still, so it never renews: meters that come back at all came
/// from the preview opened by the reopening, however long (within the daemon's 5 s preview
/// expiry) a loaded machine takes to show them.
#[test]
fn slow_session_dialog_meters_return_every_round() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    d.synced()?;
    let tick = |d: &mut Driver| {
        d.send(Msg::Tick {
            now_s: 0.0,
            dt_s: 0.02,
        })
    };
    let seq = |d: &Driver| {
        d.st.data
            .as_ref()
            .and_then(|x| x.latest.get(&Topic::PreviewLevels))
            .map_or(0, |f| f.frame.stamp.seq)
    };
    let pump_for = |d: &mut Driver, ms: u64| {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            d.pump();
            tick(d);
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    for round in 0..20 {
        d.key("Escape");
        d.key("Shift+O");
        d.send(Msg::Text("O".into()));
        // Long enough for a stop that overtook the preview to have closed it.
        pump_for(&mut d, 300);
        let before = seq(&d);
        let end = Instant::now() + DEADLINE;
        loop {
            d.pump();
            tick(&mut d);
            if seq(&d) > before && d.st.input_meters().len() == 4 {
                break;
            }
            if Instant::now() > end {
                return Err(format!("round {round}: the meters stopped").into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    drop(d);
    drop(daemon);
    Ok(())
}

/// The layout comes back on the next start: maximised on the SPL pane, showing the same
/// meter by name. The first run's preferences are saved as the app saves them, the second
/// run reads the file and connects to the same daemon.
#[test]
fn a_restart_comes_back_to_the_same_pane() -> R {
    use ac2_ui::prefs::UiPrefs;
    use ac2_ui::state::PaneKind;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("ui.toml");
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let ep = daemon.client_config(NAME);
    let mut d = Driver::connect(ep.clone(), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.key("Ctrl+K");
    d.send(Msg::Text("new spl".into()));
    d.key("Enter");
    d.until(
        "the SPL dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spl),
    )?;
    d.key("Enter");
    d.until("the SPL meter", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
    })?;
    d.key("Alt+4");
    d.key("W");
    assert!(d.st.layout.maximized);
    assert!(d.st.prefs_dirty);
    d.st.prefs.save(&path)?;
    let name =
        d.st.pane_meas(PaneKind::Spl)
            .map(|m| m.config.name.clone())
            .ok_or("meter")?;
    drop(d);

    let (prefs, err) = UiPrefs::load(Some(&path));
    assert_eq!(err, None);
    let mut d = Driver::connect(ep, &daemon.describe())?;
    d.st.set_prefs(prefs);
    d.synced()?;
    assert_eq!(d.st.layout.focus, PaneKind::Spl);
    assert!(d.st.layout.maximized && !d.st.fullscreen);
    assert_eq!(d.st.layout.visible(), [PaneKind::Spl]);
    assert_eq!(
        d.st.pane_meas(PaneKind::Spl).map(|m| m.config.name.clone()),
        Some(name)
    );
    assert!(d.st.daemon().is_some_and(|s| !s.generator.armed));
    drop(d);
    drop(daemon);
    Ok(())
}
