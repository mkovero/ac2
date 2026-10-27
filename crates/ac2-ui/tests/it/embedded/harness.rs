//! The driver shared by every area: the app without its window, and measuring from an empty daemon.

use ac2_client::ClientConfig;
use ac2_proto::FrameData;
use ac2_proto::model::{MeasKind, TfAveraging};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_ui::conn::{Conn, Target};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::{Chord, CommandId, Keymap};
use ac2_ui::state::{AppState, ChoicePurpose, Msg, Overlay, StimPhase};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub const DEADLINE: Duration = Duration::from_secs(30);
pub const NAME: &str = "ac2-ui e2e test";

/// The app without its window: messages through the reducer, its requests to the link.
pub struct Driver {
    pub st: AppState,
    pub keys: Keymap,
    pub conn: Conn,
}

impl Driver {
    pub fn connect(config: ClientConfig, describe: &str) -> R<Self> {
        let conn = Conn::start(
            Target {
                config,
                describe: describe.into(),
            },
            Arc::new(|| {}),
        )?;
        let mut st = AppState::default();
        st.set_prefs(crate::common::grid_ui_prefs());
        Ok(Self {
            st,
            keys: Keymap::default(),
            conn,
        })
    }

    pub fn send(&mut self, m: Msg) {
        let mut reqs = self.st.update(m, &self.keys);
        reqs.extend(self.st.sync_link());
        // The harness holds the one rig's link.
        for (rig, r) in reqs {
            assert_eq!(rig, self.st.active(), "{r:?} for another rig");
            self.conn.send(r);
        }
    }

    pub fn key(&mut self, chord: &str) {
        let c = Chord::parse(chord).unwrap_or_else(|e| panic!("{e}"));
        self.send(Msg::Key(c));
    }

    pub fn pump(&mut self) {
        for e in self.conn.drain() {
            self.send(Msg::Conn(self.st.active(), Box::new(e)));
        }
    }

    /// Pumps the link until `cond` holds; on timeout fails with the toasts seen.
    pub fn until(&mut self, what: &str, cond: impl Fn(&AppState) -> bool) -> R {
        let end = Instant::now() + DEADLINE;
        loop {
            self.pump();
            if cond(&self.st) {
                return Ok(());
            }
            if Instant::now() > end {
                let toasts: Vec<&str> = self
                    .st
                    .toasts
                    .list
                    .iter()
                    .map(|t| t.text.as_str())
                    .collect();
                let mut held: Vec<String> = self
                    .st
                    .rig
                    .data
                    .as_ref()
                    .map(|d| d.latest.frames.keys().map(|k| k.to_string()).collect())
                    .unwrap_or_default();
                held.sort();
                let mut wanted: Vec<String> = self
                    .st
                    .wanted_topics()
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                wanted.sort();
                return Err(format!(
                    "timed out waiting for {what}; overlay {:?}; toasts {toasts:?}; \
                     frames held {held:?}; topics wanted {wanted:?}",
                    self.st.overlay
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn synced(&mut self) -> R {
        self.until("sync", |s| {
            s.connected() && s.rig.mirror.as_ref().is_some_and(|m| m.synced())
        })
    }

    /// Types a level, arms and fires through the keys (the simulated rig: no real audio).
    pub fn fire(&mut self) -> R {
        self.send(Msg::Command(CommandId::StimulusLevel));
        self.send(Msg::Text("-20".into()));
        self.key("Enter");
        self.until("level", |s| s.rig.stimulus.level.is_some())?;
        self.key("Space");
        self.until("armed", |s| s.rig.stimulus.phase == StimPhase::Armed)?;
        self.key("Enter");
        self.until("firing", |s| s.daemon().is_some_and(|d| d.generator.firing))
    }

    /// Frames of `meas`'s transfer function arrive, and once averaged the mid band shows the
    /// rig's −6 dB acoustic path (column `mid` is 1 kHz).
    pub fn tf_frames(&mut self, meas: MeasId, mid: usize) -> R {
        let topic = Topic::Data {
            meas,
            stream: Stream::Tf,
        };
        self.until("the −6 dB path at 1 kHz", |s| {
            s.rig.data.as_ref().is_some_and(|d| {
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

    /// Esc, until the daemon says stopped and the app has taken the stop's reply: the
    /// mirror's generator event and the link's reply arrive separately, and Space is not
    /// taken while the app still waits for the stop to finish.
    pub fn stop(&mut self) -> R {
        self.key("Escape");
        self.until("stopped", |s| {
            s.rig.stimulus.phase == StimPhase::Idle
                && s.daemon()
                    .is_some_and(|d| !d.generator.firing && !d.generator.armed)
        })
    }
}

/// A session just opened on a daemon without measurements: the New measurement chooser it
/// opens, Enter on Transfer (highlighted on the transfer pane), Enter on its form; until the
/// measurement runs. The form lays out the transfer stack (magnitude, phase, coherence) in
/// the focused transfer pane: Q on phase and coherence, so the tests that follow start on
/// the panes they had, the first focused.
pub fn first_transfer(d: &mut Driver) -> R {
    let panes = crate::common::visible(&d.st);
    d.until(
        "the New measurement chooser",
        |s| matches!(&s.overlay, Overlay::Choose(c) if c.purpose == ChoicePurpose::NewMeasurement),
    )?;
    d.key("Enter");
    d.until(
        "the transfer form",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Transfer),
    )?;
    d.key("Enter");
    d.until("the measurement, running and selected", |s| {
        s.selected_meas().is_some_and(|m| m.running)
    })?;
    d.until("the transfer stack", |s| {
        crate::common::visible(s).len() == panes.len() + 2
    })?;
    for _ in 0..2 {
        d.key("Alt+2");
        d.key("Q");
    }
    d.key("Alt+1");
    assert_eq!(crate::common::visible(&d.st), panes);
    Ok(())
}

/// From a daemon with no session to an open one and no measurement: Shift+O, Enter on the
/// simulated rig's roles, Esc on the New measurement chooser the session opens.
pub fn session_from_empty(d: &mut Driver) -> R {
    d.synced()?;
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the device list", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.session.device_info().is_some())
    })?;
    d.key("Enter");
    d.until(
        "the New measurement chooser",
        |s| matches!(&s.overlay, Overlay::Choose(c) if c.purpose == ChoicePurpose::NewMeasurement),
    )?;
    d.key("Escape");
    Ok(())
}

/// From a daemon with no session to frames, using only the app: the hint, Alt+S and its
/// New session, the audio dialog's roles for the simulated rig with its meters, Enter; the chooser's transfer
/// measurement, Enter twice; the stimulus.
pub fn measure_from_empty(d: &mut Driver) -> R {
    d.synced()?;
    assert!(d.st.open_session().is_none());
    let hint =
        d.st.empty_hint(d.st.layout.focus, &d.keys)
            .map(|h| h.text)
            .unwrap_or_default();
    assert_eq!(
        hint,
        format!(
            "No session open — {}, then New session",
            Chord::parse("Alt+S")?.label()
        )
    );

    d.key("Alt+S");
    d.until("the Sessions dialog", |s| s.sessions_dialog().is_some())?;
    // The first row: New session, then the audio settings.
    d.key("Enter");
    d.until("the new session's audio", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.heading() == ac2_ui::settings::NEW_SESSION_AUDIO)
    })?;
    d.until("the device list", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.session.device_info().is_some())
    })?;
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
    first_transfer(d)?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    assert!(
        m.config.name.starts_with("Reference → "),
        "{}",
        m.config.name
    );
    assert_eq!(d.st.empty_hint(d.st.layout.focus, &d.keys), None);
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

/// Calibrates input 2 of the simulated rig against a 1 kHz tone as `ac2 cal spl` would (the
/// app has no calibration flow of its own): a client of its own takes the stimulus lease,
/// plays the tone at −20 dBFS and asks for 94 dB until the reading is steady, then gives
/// the lease back.
pub fn calibrate(config: &ClientConfig) -> R {
    use ac2_client::{Client, ClientError, OnDrop};
    use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal};
    use ac2_proto::units::{DbSpl, Dbfs, Hz};
    use ac2_proto::{Command, ReplyBody};
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut config = config.clone();
        config.name = "ac2-ui e2e calibrator".into();
        let c = Client::connect(config).await?;
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

/// Sweep measurements the daemon lists.
pub fn sweep_count(s: &AppState) -> usize {
    s.daemon().map_or(0, |x| {
        x.measurements
            .iter()
            .filter(|m| matches!(m.config.kind, ac2_proto::model::MeasKind::Sweep { .. }))
            .count()
    })
}

/// After the sweep dialog's Enter: the new sweep measurement is listed and selected, and
/// making it armed nothing; Space on the sweep pane then arms its run.
pub fn arm_new_sweep(d: &mut Driver, before: usize) -> R {
    d.until("the sweep measurement made, nothing armed", |s| {
        sweep_count(s) > before
            && s.rig.stimulus.phase == StimPhase::Idle
            && s.sweep_meas().is_some_and(|m| Some(m.id) == s.rig.selected)
    })?;
    d.key("Space");
    Ok(())
}

/// One short sweep from the dialog, played and stored (the simulated rig: no real audio).
pub fn sweep_from_the_dialog(d: &mut Driver) -> R<ac2_proto::units::TraceId> {
    use ac2_ui::forms::FieldId;
    let before = d.st.sweep_traces().len();
    let sweeps_before = sweep_count(&d.st);
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
        f.focus = f
            .fields
            .iter()
            .position(|x| x.id == FieldId::Duration)
            .ok_or("duration")?;
        f.cycle(-1);
    }
    d.key("Enter");
    arm_new_sweep(d, sweeps_before)?;
    d.until("armed with the sweep", |s| {
        s.rig.stimulus.phase == StimPhase::Armed && s.daemon().is_some_and(|x| x.generator.armed)
    })?;
    d.key("Enter");
    d.until("the sweep stored with its data", |s| {
        s.rig.sweep.run.is_none() && s.sweep_traces().len() == before + 1
    })?;
    d.until("the stimulus off and the lease given back", |s| {
        s.rig.stimulus.phase == StimPhase::Idle
            && s.daemon().is_some_and(|x| {
                !x.generator.armed && !x.generator.firing && x.generator.owner.is_none()
            })
    })?;
    // The new result is the selection.
    d.st.rig
        .selected_trace
        .ok_or_else(|| "the new sweep is not selected".into())
}

/// The text labels of a scene.
pub fn scene_texts(s: &ac2_scene::primitives::Scene) -> Vec<String> {
    s.layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.clone()))
        .collect()
}
