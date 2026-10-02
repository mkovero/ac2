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
/// dialog's defaults for the simulated rig, Enter; the hint again, the palette's new
/// transfer measurement, Enter; the stimulus.
fn measure_from_empty(d: &mut Driver) -> R {
    d.synced()?;
    assert!(d.st.open_session().is_none());
    let hint = d.st.empty_hint(&d.keys).unwrap_or_default();
    assert!(
        hint.starts_with(&format!(
            "No audio session — press {}",
            Chord::parse("Shift+O")?.label()
        )),
        "{hint}"
    );

    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the device list", |s| {
        matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Session && f.device().is_some())
    })?;
    d.key("Enter");
    d.until("the session", |s| s.open_session().is_some())?;
    let open = d.st.open_session().cloned().ok_or("session")?;
    assert_eq!(open.config.input_channels, vec![0, 1]);
    assert_eq!(
        open.config.loopback.map(|l| (l.output, l.input)),
        Some((0, 0))
    );
    let hint = d.st.empty_hint(&d.keys).unwrap_or_default();
    assert!(hint.starts_with("No measurements"), "{hint}");

    // The palette: Ctrl+K, "new transfer", Enter opens the dialog; Enter creates.
    d.key("Ctrl+K");
    d.send(Msg::Text("new transfer".into()));
    d.key("Enter");
    assert!(
        matches!(&d.st.overlay, Overlay::Form(f) if f.kind == FormKind::Transfer),
        "{:?}",
        d.st.overlay
    );
    d.key("Enter");
    d.until("the measurement, running and selected", |s| {
        s.selected_meas().is_some_and(|m| m.running)
    })?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    assert_eq!(m.config.name, "TF 1");
    assert_eq!(d.st.empty_hint(&d.keys), None);
    let MeasKind::Transfer { config } = &m.config.kind else {
        return Err("not a transfer measurement".into());
    };
    assert_eq!((config.reference_input, config.measurement_input), (0, 1));
    assert_eq!(config.averaging, TfAveraging::Fifo { blocks: 8 });

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

#[test]
fn demo_setup_is_for_the_simulated_rig_only() {
    for b in [EmbeddedBackend::Cpal, EmbeddedBackend::Jack] {
        match start_embedded_with(b, Setup::Demo) {
            Err(EmbeddedError::Setup(_)) => {}
            other => panic!("{b:?}: {other:?}"),
        }
    }
}
