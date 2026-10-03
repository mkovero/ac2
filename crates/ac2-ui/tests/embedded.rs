//! End to end without a window or GPU: the reducer and the link thread driven exactly as the
//! app drives them, against real daemons on the simulated rig (never real audio).
//!
//! - The embedded simulated rig starts measuring: session open, "demo" running, frames that
//!   show the rig's acoustic path once the stimulus plays.
//! - An empty daemon (embedded with no setup, as on real audio; or a stand-alone local
//!   daemon) is made to measure from the app alone: the session dialog opens a session, the
//!   new-measurement dialog creates and starts a measurement, frames arrive.
#![cfg(feature = "embedded")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_client::{ClientConfig, Endpoints};
use ac2_proto::FrameData;
use ac2_proto::model::{MeasKind, TfAveraging};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_scene::primitives::Color;
use ac2_scene::theme::Theme;
use ac2_scene::view::LeqStyle;
use ac2_ui::conn::{Conn, Target};
use ac2_ui::embedded::{
    EmbeddedBackend, EmbeddedError, Setup, start_embedded, start_embedded_with,
};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::{Chord, CommandId, Keymap};
use ac2_ui::state::{AppState, Msg, Overlay, StimPhase};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

const DEADLINE: Duration = Duration::from_secs(30);

/// The app without its window: messages through the reducer, its requests to the link.
struct Driver {
    st: AppState,
    keys: Keymap,
    conn: Conn,
}

impl Driver {
    fn connect(endpoints: Endpoints, describe: &str) -> R<Self> {
        let conn = Conn::start(
            Target {
                config: ClientConfig::new(endpoints, "ac2-ui e2e test"),
                describe: describe.into(),
            },
            Arc::new(|| {}),
        )?;
        Ok(Self {
            st: AppState::default(),
            keys: Keymap::default(),
            conn,
        })
    }

    fn send(&mut self, m: Msg) {
        for r in self.st.update(m, &self.keys) {
            self.conn.send(r);
        }
    }

    fn key(&mut self, chord: &str) {
        let c = Chord::parse(chord).unwrap_or_else(|e| panic!("{e}"));
        self.send(Msg::Key(c));
    }

    fn pump(&mut self) {
        for e in self.conn.drain() {
            self.send(Msg::Conn(Box::new(e)));
        }
    }

    /// Pumps the link until `cond` holds; on timeout fails with the toasts seen.
    fn until(&mut self, what: &str, cond: impl Fn(&AppState) -> bool) -> R {
        let end = Instant::now() + DEADLINE;
        loop {
            self.pump();
            if cond(&self.st) {
                return Ok(());
            }
            if Instant::now() > end {
                let toasts: Vec<&str> = self.st.toasts.iter().map(|t| t.text.as_str()).collect();
                return Err(format!(
                    "timed out waiting for {what}; overlay {:?}; toasts {toasts:?}",
                    self.st.overlay
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn synced(&mut self) -> R {
        self.until("sync", |s| {
            s.connected() && s.mirror.as_ref().is_some_and(|m| m.synced())
        })
    }

    /// Types a level, arms and fires through the keys (the simulated rig: no real audio).
    fn fire(&mut self) -> R {
        self.send(Msg::Command(CommandId::StimulusLevel));
        self.send(Msg::Text("-20".into()));
        self.key("Enter");
        self.until("level", |s| s.stimulus.level.is_some())?;
        self.key("Space");
        self.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
        self.key("Enter");
        self.until("firing", |s| s.daemon().is_some_and(|d| d.generator.firing))
    }

    /// Frames of `meas`'s transfer function arrive, and once averaged the mid band shows the
    /// rig's −6 dB acoustic path (column `mid` is 1 kHz).
    fn tf_frames(&mut self, meas: MeasId, mid: usize) -> R {
        let topic = Topic::Data {
            meas,
            stream: Stream::Tf,
        };
        self.until("the −6 dB path at 1 kHz", |s| {
            s.data.as_ref().is_some_and(|d| {
                d.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                    FrameData::Tf(tf) => tf
                        .mag
                        .get(mid)
                        .is_some_and(|m| m.is_finite() && (m - (-6.02)).abs() < 0.5),
                    _ => false,
                })
            })
        })
    }

    fn stop(&mut self) -> R {
        self.key("Escape");
        self.until("stopped", |s| {
            s.daemon()
                .is_some_and(|d| !d.generator.firing && !d.generator.armed)
        })
    }
}

/// From a daemon with no session to frames, using only the app: the hint, Shift+O, the
/// dialog's roles for the simulated rig with its meters, Enter; the offered transfer
/// measurement, Enter; the stimulus.
fn measure_from_empty(d: &mut Driver) -> R {
    d.synced()?;
    assert!(d.st.open_session().is_none());
    let hint = d.st.empty_hint(&d.keys).map(|h| h.text).unwrap_or_default();
    assert!(
        hint.starts_with(&format!(
            "No audio session — press {}",
            Chord::parse("Shift+O")?.label()
        )),
        "{hint}"
    );

    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until(
        "the device list",
        |s| matches!(&s.overlay, Overlay::Session(x) if x.device_info().is_some()),
    )?;
    // The rig's own wiring as roles: in 1 the reference (loopback of out 1), in 2 the mic;
    // every input of the device metered before the session opens.
    d.until("the meters of the device", |s| s.input_meters().len() == 4)?;
    d.key("Enter");
    d.until("the session", |s| s.open_session().is_some())?;
    let open = d.st.open_session().cloned().ok_or("session")?;
    assert_eq!(open.config.input_channels, vec![0, 1]);
    assert_eq!(
        open.config.loopback.map(|l| (l.output, l.input)),
        Some((0, 0))
    );
    // One key: a transfer measurement per mic.
    d.until("the measurement offer", |s| {
        matches!(s.overlay, Overlay::Offer(_))
    })?;
    d.key("Enter");
    d.until("the measurement, running and selected", |s| {
        s.selected_meas().is_some_and(|m| m.running)
    })?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    assert_eq!(m.config.name, "Reference \u{2192} Room mic");
    assert_eq!(d.st.empty_hint(&d.keys), None);
    let MeasKind::Transfer { config } = &m.config.kind else {
        return Err("not a transfer measurement".into());
    };
    assert_eq!((config.reference_input, config.measurement_input), (0, 1));
    assert_eq!(config.averaging, TfAveraging::Fifo { blocks: 8 });

    // The palette still makes more: Ctrl+K, "new transfer", Enter opens the dialog.
    d.key("Ctrl+K");
    d.send(Msg::Text("new transfer".into()));
    d.key("Enter");
    assert!(
        matches!(&d.st.overlay, Overlay::Form(f) if f.kind == FormKind::Transfer),
        "{:?}",
        d.st.overlay
    );
    d.key("Escape");

    d.fire()?;
    d.tf_frames(m.id, 240)?;
    d.stop()
}

#[test]
fn simulated_rig_starts_measuring() -> R {
    let daemon = start_embedded(EmbeddedBackend::Fake)?;
    assert_eq!(daemon.describe(), "embedded daemon (fake rig)");
    let ep = daemon.endpoints();
    #[cfg(unix)]
    assert!(ep.ctrl.starts_with("ipc://"), "{ep:?}");
    #[cfg(not(unix))]
    assert!(ep.ctrl.starts_with("tcp://127.0.0.1:"), "{ep:?}");

    let mut d = Driver::connect(ep, &daemon.describe())?;
    d.synced()?;
    // Session open as the rig is wired, "demo" running and selected: nothing to set up.
    let open = d.st.open_session().cloned().ok_or("no session")?;
    assert_eq!(open.config.input_channels, vec![0, 1]);
    assert_eq!(open.config.output_channels, 1);
    assert_eq!(
        open.config.loopback.map(|l| (l.output, l.input)),
        Some((0, 0))
    );
    let m = d.st.selected_meas().cloned().ok_or("nothing selected")?;
    assert_eq!(m.config.name, "demo");
    assert!(m.running);
    assert!(matches!(
        &m.config.kind,
        MeasKind::Transfer { config } if (config.reference_input, config.measurement_input) == (0, 1)
    ));
    assert_eq!(d.st.empty_hint(&d.keys), None);

    d.fire()?;
    d.tf_frames(m.id, 240)?;
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

#[test]
fn empty_embedded_daemon_measures_from_the_app() -> R {
    // As on real audio (no setup), but on the simulated rig.
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.endpoints(), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    drop(d);
    drop(daemon);
    Ok(())
}

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
    let mut d = Driver::connect(ep, "local daemon")?;
    measure_from_empty(&mut d)?;
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
    let mut d = Driver::connect(ep, "local daemon")?;
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
    let mut d = Driver::connect(ep, "local daemon")?;
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
fn session_dialog_meters_return_every_round() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.endpoints(), &daemon.describe())?;
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

/// From an empty daemon to a stored sweep using only the app, on the simulated rig whose
/// "speaker" distorts (H2 −40 dB, H3 −50 dB at −20 dBFS): the session from the dialog,
/// Shift+S, a typed level, Enter arms, Enter plays, the result opens the distortion pane
/// with the rig's harmonics, and the stimulus is off again: nothing re-arms after a sweep.
#[test]
fn empty_embedded_daemon_sweeps_from_the_app() -> R {
    use ac2_scene::distortion::{Reading, reading_at};
    use ac2_ui::forms::FieldId;
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.endpoints(), &daemon.describe())?;
    measure_from_empty(&mut d)?;

    d.key("Shift+S");
    d.send(Msg::Text("S".into()));
    d.until(
        "the sweep dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Sweep),
    )?;
    if let Overlay::Form(f) = &mut d.st.overlay {
        assert_eq!(f.channel(FieldId::Reference), Some(0));
        assert_eq!(f.channel(FieldId::Measurement), Some(1));
        // A short sweep over the band the rig's harmonics stay below Nyquist in.
        f.set_text(FieldId::Level, "-20");
        f.set_text(FieldId::From, "100 Hz");
        f.set_text(FieldId::To, "5 kHz");
        f.focus = f
            .fields
            .iter()
            .position(|x| x.id == FieldId::Duration)
            .ok_or("duration")?;
        f.cycle(-1);
        assert_eq!(f.fields[f.focus].display(), "1 s (quick look)");
    }
    d.key("Enter");
    d.until("armed with the sweep", |s| {
        s.stimulus.phase == StimPhase::Armed
            && s.daemon().is_some_and(|x| {
                x.generator.armed
                    && x.generator
                        .settings
                        .as_ref()
                        .is_some_and(|g| matches!(g.signal, ac2_proto::model::Signal::Ess { .. }))
            })
    })?;
    d.key("Enter");
    d.until("the sweep stored and shown", |s| {
        s.sweep.run.is_none() && s.layout.focus == PaneKind::Distortion && s.shown_sweep().is_some()
    })?;
    let (data, grid) = d.st.shown_sweep().ok_or("no sweep")?;
    let freqs = ac2_scene::grid::column_frequencies(grid);
    let s = data.sweep.as_ref().ok_or("no sweep data")?;
    let m = s.info.floor_margin.0;
    for (order, want) in [(2u8, -40.0), (3, -50.0)] {
        let h = s
            .harmonics
            .iter()
            .find(|h| h.order == order)
            .ok_or("order")?;
        match reading_at(&h.curve, &freqs, 1000.0, m) {
            Reading::Level(v) => assert!((v - want).abs() < 1.0, "H{order} at 1 kHz: {v}"),
            other => return Err(format!("H{order} at 1 kHz: {other:?}").into()),
        }
    }
    d.until("the stimulus off and the lease given back", |s| {
        s.stimulus.phase == StimPhase::Idle
            && s.daemon().is_some_and(|x| {
                !x.generator.armed && !x.generator.firing && x.generator.owner.is_none()
            })
    })?;
    assert!(!d.st.stimulus_live(), "STIM OFF");
    assert!(d.st.sweep.plan.is_none());
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon, using only the app: the session's inputs metered by name and role
/// in the sidebar, then a set of two sweeps followed on the progress strip, sweep 1 of 2
/// then 2 of 2, stopped from it: the output stops, the generator is disarmed and the run
/// is discarded.
#[test]
fn input_meters_and_a_stopped_sweep_set_from_the_app() -> R {
    use ac2_proto::model::{SweepFailure, SweepStatus};
    use ac2_scene::meter::{InputUse, MeterState};
    use ac2_ui::forms::FieldId;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.endpoints(), &daemon.describe())?;
    measure_from_empty(&mut d)?;

    // Every captured input, named with its role, metering the rig's signal.
    d.until("the input meters with names", |s| {
        let rows = s.session_inputs();
        rows.len() == 2
            && s.devices.is_some()
            && rows.iter().all(|r| r.reading.state != MeterState::NoData)
    })?;
    let rows = d.st.session_inputs();
    assert!(
        rows[0].label.contains("reference") && rows[0].label.contains("(in 1)"),
        "{}",
        rows[0].label
    );
    assert!(
        rows[1].label.starts_with("Room mic · mic"),
        "{}",
        rows[1].label
    );
    // The selected transfer measurement's inputs are the marked ones.
    assert_eq!(rows[0].used, Some(InputUse::Reference));
    assert_eq!(rows[1].used, Some(InputUse::Measurement));
    assert_eq!(d.st.operation(), None);

    d.key("Shift+S");
    d.send(Msg::Text("S".into()));
    d.until(
        "the sweep dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Sweep),
    )?;
    if let Overlay::Form(f) = &mut d.st.overlay {
        f.set_text(FieldId::Level, "-20");
        f.set_text(FieldId::From, "100 Hz");
        f.set_text(FieldId::To, "5 kHz");
        for (id, step, want) in [
            (FieldId::Duration, -1, "1 s (quick look)"),
            (FieldId::Repeats, 1, "2"),
        ] {
            f.focus = f.fields.iter().position(|x| x.id == id).ok_or("field")?;
            f.cycle(step);
            assert_eq!(f.fields[f.focus].display(), want);
        }
    }
    d.key("Enter");
    d.until("armed with the sweep", |s| {
        s.stimulus.phase == StimPhase::Armed && s.daemon().is_some_and(|x| x.generator.armed)
    })?;
    d.key("Enter");
    let step = |s: &AppState| s.operation().map(|p| p.step);
    d.until("sweep 1 of 2", |s| {
        step(s).as_deref() == Some("sweep 1 of 2")
    })?;
    let p = d.st.operation().ok_or("progress")?;
    assert_eq!(p.title, "sweep \"Sweep 1\"");
    assert!(
        p.remaining
            .is_some_and(|r| r.ends_with("left") || r == "finishing")
    );
    // The meters keep running during the sweep.
    let seq = |s: &AppState| {
        s.data
            .as_ref()
            .and_then(|x| x.latest.get(&Topic::SessionLevels))
            .map_or(0, |f| f.frame.stamp.seq)
    };
    let before = seq(&d.st);
    d.until("meter frames during the sweep", |s| seq(s) > before)?;
    d.until("sweep 2 of 2", |s| {
        step(s).as_deref() == Some("sweep 2 of 2")
    })?;

    // The strip's Stop button (the same command as Esc).
    d.send(Msg::Command(CommandId::StimulusStop));
    // The app ends its sweep mode once it has seen the stop, which can be a step after the
    // daemon's state says so: wait for both.
    d.until("stopped and disarmed, the run discarded", |s| {
        s.sweep.plan.is_none()
            && s.operation().is_none()
            && s.daemon().is_some_and(|x| {
                !x.generator.firing
                    && !x.generator.armed
                    && x.sweep.as_ref().is_some_and(|r| {
                        matches!(
                            r.status,
                            SweepStatus::Failed {
                                reason: SweepFailure::Stopped,
                                ..
                            }
                        )
                    })
            })
    })?;
    assert_eq!(d.st.operation(), None);
    assert!(d.st.sweep.plan.is_none());
    drop(d);
    drop(daemon);
    Ok(())
}

fn mic_curve_file(name: &str) -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/mic_curves")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// The sidebar label of input `channel`.
fn meter_label(s: &AppState, channel: u16) -> String {
    s.session_inputs()
        .into_iter()
        .find(|r| r.channel == channel)
        .map(|r| r.label)
        .unwrap_or_default()
}

/// A mic's two curves (beyerdynamic MM1, 0° and 90°) from an empty daemon, with the app
/// alone: name the mic in the session dialog, open, import both curves in the input setup
/// view, switch 0° ↔ 90° ↔ off there; the sidebar label and the transfer pane's caption
/// always say which curve is in use.
#[test]
fn mic_curves_imported_and_switched_in_the_input_setup() -> R {
    use ac2_proto::model::CurveChoice;
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.endpoints(), &daemon.describe())?;
    d.synced()?;
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until(
        "the device list",
        |s| matches!(&s.overlay, Overlay::Session(x) if x.device_info().is_some()),
    )?;
    // The rig's mic is on input 2 (the dialog's fourth row): N names it.
    for _ in 0..3 {
        d.key("ArrowDown");
    }
    d.key("N");
    d.send(Msg::Text("n".into()));
    d.send(Msg::Text("MM1 34804".into()));
    d.key("Enter");
    d.key("Enter");
    d.until("the session", |s| s.open_session().is_some())?;
    d.until("the measurement offer", |s| {
        matches!(s.overlay, Overlay::Offer(_))
    })?;
    d.key("Enter");
    d.until("the measurement, running and selected", |s| {
        s.selected_meas().is_some_and(|m| m.running)
    })?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    assert_eq!(m.config.name, "Reference \u{2192} MM1 34804");
    // No curve stored yet: the label and the caption say so instead of nothing.
    d.until("the input labelled", |s| {
        meter_label(s, 1) == "MM1 34804 · no curve stored · mic (in 2)"
    })?;
    d.fire()?;
    d.tf_frames(m.id, 240)?;
    d.until("the caption without a curve", |s| {
        s.pane_caption(PaneKind::Transfer)
            .is_some_and(|c| c.contains("no mic curve stored for MM1 34804"))
    })?;

    // The input setup view opens on the selected measurement's input.
    d.key("Ctrl+K");
    d.send(Msg::Text("input setup".into()));
    d.key("Enter");
    assert!(
        matches!(&d.st.overlay, Overlay::Calibrations(_)),
        "{:?}",
        d.st.overlay
    );
    let import = |d: &mut Driver, file: &str| {
        d.key("I");
        d.send(Msg::Text("i".into()));
        d.send(Msg::Text(mic_curve_file(file)));
        d.key("Enter");
    };
    import(&mut d, "449350_34804_0Grad.txt");
    let curve_of = |s: &AppState| {
        s.daemon()
            .and_then(|x| x.inputs.iter().find(|i| i.channel == 1))
            .map(|i| i.curve.clone())
    };
    let label = |l: &str| CurveChoice::Curve { label: l.into() };
    d.until("0° imported and chosen (the mic's only curve)", |s| {
        curve_of(s) == Some(label("0°"))
    })?;
    import(&mut d, "449350_34804_90Grad.txt");
    d.until("90° imported, 0° still chosen", |s| {
        s.daemon()
            .is_some_and(|x| x.mics.first().is_some_and(|m| m.curves.len() == 2))
    })?;
    assert_eq!(curve_of(&d.st), Some(label("0°")));
    let shows = |d: &mut Driver, what: &str, label_part: &str, caption: &str| {
        let (lp, cap) = (label_part.to_owned(), caption.to_owned());
        d.until(what, move |s| {
            meter_label(s, 1) == format!("MM1 34804 · {lp} · mic (in 2)")
                && s.pane_caption(PaneKind::Transfer)
                    .is_some_and(|c| c.contains(cap.as_str()))
        })
    };
    shows(&mut d, "0° in use", "0°", "mic curve: MM1 34804 0°")?;
    // → 90°, ← back to 0°, ← off: one key each, applied at once.
    d.key("ArrowRight");
    shows(&mut d, "90° in use", "90°", "mic curve: MM1 34804 90°")?;
    d.key("ArrowLeft");
    shows(&mut d, "0° again", "0°", "mic curve: MM1 34804 0°")?;
    d.key("ArrowLeft");
    shows(&mut d, "no curve", "curve off", "mic curve off")?;
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// Calibrates input 2 of the simulated rig against a 1 kHz tone as `ac2 cal spl` would (the
/// app has no calibration flow of its own): a client of its own takes the stimulus lease,
/// plays the tone at −20 dBFS and asks for 94 dB until the reading is steady, then gives
/// the lease back.
fn calibrate(ep: &Endpoints) -> R {
    use ac2_client::{Client, ClientError, OnDrop};
    use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal};
    use ac2_proto::units::{DbSpl, Dbfs, Hz};
    use ac2_proto::{Command, ReplyBody};
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let c = Client::connect(ClientConfig::new(ep.clone(), "ac2-ui e2e calibrator")).await?;
        c.wait_synced(Duration::from_secs(10)).await?;
        let lease = c.acquire_lease(false, OnDrop::Release).await?;
        lease
            .set(GeneratorDesired {
                settings: GeneratorSettings {
                    signal: Signal::Sine { freq: Hz(1000.0) },
                    level: Dbfs(-20.0),
                    band: None,
                    outputs: vec![0],
                },
                armed: true,
                firing: true,
            })
            .await?;
        let end = Instant::now() + DEADLINE;
        loop {
            match c
                .call(Command::CalSpl {
                    input: 1,
                    mic: "Room mic".into(),
                    calibrator_level: DbSpl(94.0),
                    calibrator_freq: Hz(1000.0),
                })
                .await
            {
                Ok(ReplyBody::Calibration(_)) => break,
                Err(ClientError::Daemon(p)) if p.msg.contains("not steady") => {}
                other => return Err(format!("cal.spl: {other:?}").into()),
            }
            if Instant::now() > end {
                return Err("the calibrator never read steady".into());
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        lease.end().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}

fn spl_log(s: &AppState) -> Option<&ac2_proto::model::SplLog> {
    s.daemon()?.spl_logs.first()
}

fn judgement(s: &AppState, i: usize) -> Option<ac2_proto::model::LeqJudgement> {
    spl_log(s)?.windows.get(i).map(|w| w.judgement)
}

/// The tiles the SPL pane would show now, from the newest `leq` frame.
fn tiles(s: &AppState) -> Vec<ac2_scene::leq::LeqTile> {
    let Some(m) = s
        .measurements()
        .into_iter()
        .find(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
    else {
        return Vec::new();
    };
    let MeasKind::Spl { config } = &m.config.kind else {
        return Vec::new();
    };
    let topic = Topic::Data {
        meas: m.id,
        stream: Stream::Leq,
    };
    match s
        .data
        .as_ref()
        .and_then(|d| d.latest.get(&topic))
        .map(|f| &f.frame.data)
    {
        Some(FrameData::Leq(f)) => ac2_scene::leq::leq_tiles(&config.leq, f),
        _ => Vec::new(),
    }
}

/// The columns the SPL pane draws now, as the app builds them (the pane at 1280 × 720):
/// each window's name, background and bar colour, shortest window first.
fn columns(s: &AppState) -> Vec<(String, Color, Color)> {
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64),
        ),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 1280.0,
        height: 720.0,
    };
    ac2_ui::scenes::leq(s, &Theme::dark(), size, now)
        .and_then(|x| x.columns)
        .map(|k| {
            k.columns
                .into_iter()
                .map(|c| (c.name, c.background, c.bar_color))
                .collect()
        })
        .unwrap_or_default()
}

/// Leq windows with limits from an empty daemon, using the app: the session from its dialog,
/// an SPL meter from the palette, its windows from the Leq dialog by keys (a 5 s and a 10 s
/// window limited to 85 dB), the stimulus from the keys. Pink noise at −20 dBFS reads about
/// 91 dB(A) on the calibrated mic: both windows' columns (the default layout) and tiles turn
/// red, the alarms arrive as toasts; the stop brings both back under the limit, and that is
/// a toast too.
#[test]
fn leq_limits_go_over_and_recover_from_the_app() -> R {
    use ac2_proto::model::LeqJudgement;
    use ac2_scene::leq::TileState;
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let ep = daemon.endpoints();
    let mut d = Driver::connect(ep.clone(), &daemon.describe())?;
    measure_from_empty(&mut d)?;

    // Ctrl+K, "new spl", Enter: the dialog picks the mic; Enter creates and starts it.
    d.key("Ctrl+K");
    d.send(Msg::Text("new spl".into()));
    d.key("Enter");
    d.until(
        "the SPL dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spl),
    )?;
    d.key("Enter");
    d.until("the SPL meter running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
    })?;
    d.until("its windows, not judged yet", |s| {
        judgement(s, 0) == Some(LeqJudgement::NoLimit)
    })?;
    calibrate(&ep)?;

    // Shift+L: the meter's windows. ↓↓ to the first, ←←← to 5 s, Tab Tab to its limit,
    // 85; ↓ to the second window's limit, 85, Shift+Tab ×2 to its length, ←← to 10 s.
    d.key("Shift+L");
    d.send(Msg::Text("L".into()));
    d.until("the Leq dialog", |s| matches!(s.overlay, Overlay::Leq(_)))?;
    for k in [
        "ArrowDown",
        "ArrowDown",
        "ArrowLeft",
        "ArrowLeft",
        "ArrowLeft",
        "Tab",
        "Tab",
    ] {
        d.key(k);
    }
    d.send(Msg::Text("85".into()));
    d.key("ArrowDown");
    d.send(Msg::Text("85".into()));
    for k in [
        "Shift+Tab",
        "Shift+Tab",
        "ArrowLeft",
        "ArrowLeft",
        "ArrowLeft",
    ] {
        d.key(k);
    }
    if let Overlay::Leq(x) = &d.st.overlay {
        let names: Vec<String> = x
            .rows
            .iter()
            .map(|r| r.cell(ac2_ui::leq_dialog::Col::Length))
            .collect();
        assert_eq!(names[..2], ["LAeq 5 s", "LAeq 10 s"], "{names:?}");
    }
    d.key("Enter");
    assert_eq!(d.st.overlay, Overlay::None);
    assert!(d.st.view.spl.leq, "the SPL pane shows the windows");
    assert_eq!(d.st.layout.focus, PaneKind::Spl);
    assert_eq!(d.st.view.spl.layout.style, LeqStyle::Columns);
    let th = Theme::dark();
    let red = th.banner_fault.background;
    let column_is = |s: &AppState, i: usize, f: &dyn Fn(Color, Color) -> bool| {
        columns(s).get(i).is_some_and(|c| f(c.1, c.2))
    };
    // The windows are rebuilt from the meter's log, which may still hold the calibrator's
    // 94 dB seconds (they go over and come back on their own). Then quiet, judged.
    let quiet = |t: &ac2_scene::leq::LeqTile| {
        t.state == TileState::Ok && t.value.parse::<f64>().is_ok_and(|v| v < 80.0)
    };
    d.until("both windows quiet and judged", |s| {
        let t = tiles(s);
        t.len() >= 2 && t[..2].iter().all(quiet)
    })?;
    let seen = spl_log(&d.st).map_or(0, |l| l.alarms.len());
    let toasts = d.st.toasts.len();
    let new_toasts = move |s: &AppState, what: &str, error: bool| {
        s.toasts[toasts.min(s.toasts.len())..]
            .iter()
            .filter(|t| t.error == error && t.text.contains(what))
            .count()
    };

    // The level typed before is kept: Space arms, Enter fires.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    d.until("both tiles red", |s| {
        let t = tiles(s);
        t.len() >= 2 && t[0].state == TileState::Over && t[1].state == TileState::Over
    })?;
    // The 5 s and 10 s columns, leftmost, the whole column red.
    d.until("both columns red", |s| {
        (0..2).all(|i| column_is(s, i, &|bg, bar| bg != th.plot_background && bar == red))
    })?;
    let c = columns(&d.st);
    assert_eq!(
        (c[0].0.as_str(), c[1].0.as_str()),
        ("LAeq 5 s", "LAeq 10 s")
    );
    let t = tiles(&d.st);
    assert_eq!(t[0].name, "LAeq 5 s");
    assert_eq!(t[0].state_text.as_deref(), Some("OVER"));
    assert_eq!(t[0].limit.as_deref(), Some("limit 85.0 dB"));
    assert_eq!(t[0].unit, "dB SPL");
    d.until("the over alarms as toasts", |s| {
        new_toasts(s, "over its limit", true) == 2
    })?;

    d.stop()?;
    d.until("both back under the limit", |s| {
        let t = tiles(s);
        t.len() >= 2
            && t[..2].iter().all(|x| x.state == TileState::Ok)
            && judgement(s, 1) == Some(LeqJudgement::Ok)
    })?;
    d.until("both columns back to plain", |s| {
        (0..2).all(|i| {
            column_is(s, i, &|bg, bar| {
                bg == th.plot_background && bar == th.level_ok
            })
        })
    })?;
    d.until("the recoveries as toasts", |s| {
        new_toasts(s, "back within its limit", false) == 2
    })?;
    let l = spl_log(&d.st).ok_or("log")?;
    let kinds: Vec<_> = l.alarms[seen..]
        .iter()
        .map(|a| (a.duration.0, a.kind))
        .collect();
    use ac2_proto::model::LeqAlarmKind::{Over, Recovered};
    assert_eq!(
        kinds,
        [
            (5.0, Over),
            (10.0, Over),
            (5.0, Recovered),
            (10.0, Recovered)
        ]
    );
    // The history holds the excursion: over-limit seconds, and ones after it that are not.
    let (_, h) = d.st.leq_history.values().next().ok_or("history")?;
    let m =
        d.st.measurements()
            .into_iter()
            .find(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
            .ok_or("meter")?
            .clone();
    let MeasKind::Spl { config } = &m.config.kind else {
        return Err("not an SPL meter".into());
    };
    let p = h.points(&config.leq.windows[0]).ok_or("points")?;
    assert!(p.iter().any(|x| x.over));
    assert!(!p.back().ok_or("newest")?.over);
    drop(d);
    drop(daemon);
    Ok(())
}
