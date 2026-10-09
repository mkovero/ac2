//! The driver shared by every area: the app without its window, and measuring from an empty daemon.

use ac2_client::ClientConfig;
use ac2_proto::FrameData;
use ac2_proto::model::{MeasKind, TfAveraging};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_ui::conn::{Conn, Target};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::{Chord, CommandId, Keymap};
use ac2_ui::state::{AppState, Msg, Overlay, StimPhase};
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
        for r in reqs {
            self.conn.send(r);
        }
    }

    pub fn key(&mut self, chord: &str) {
        let c = Chord::parse(chord).unwrap_or_else(|e| panic!("{e}"));
        self.send(Msg::Key(c));
    }

    pub fn pump(&mut self) {
        for e in self.conn.drain() {
            self.send(Msg::Conn(Box::new(e)));
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

    pub fn synced(&mut self) -> R {
        self.until("sync", |s| {
            s.connected() && s.mirror.as_ref().is_some_and(|m| m.synced())
        })
    }

    /// Types a level, arms and fires through the keys (the simulated rig: no real audio).
    pub fn fire(&mut self) -> R {
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
    pub fn tf_frames(&mut self, meas: MeasId, mid: usize) -> R {
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

    /// Esc, until the daemon says stopped and the app has taken the stop's reply: the
    /// mirror's generator event and the link's reply arrive separately, and Space is not
    /// taken while the app still waits for the stop to finish.
    pub fn stop(&mut self) -> R {
        self.key("Escape");
        self.until("stopped", |s| {
            s.stimulus.phase == StimPhase::Idle
                && s.daemon()
                    .is_some_and(|d| !d.generator.firing && !d.generator.armed)
        })
    }
}

/// From a daemon with no session to frames, using only the app: the hint, Shift+O, the
/// dialog's roles for the simulated rig with its meters, Enter; the offered transfer
/// measurement, Enter; the stimulus.
pub fn measure_from_empty(d: &mut Driver) -> R {
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
            && s.stimulus.phase == StimPhase::Idle
            && s.sweep_meas().is_some_and(|m| Some(m.id) == s.selected)
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
        s.stimulus.phase == StimPhase::Armed && s.daemon().is_some_and(|x| x.generator.armed)
    })?;
    d.key("Enter");
    d.until("the sweep stored with its data", |s| {
        s.sweep.run.is_none() && s.sweep_traces().len() == before + 1
    })?;
    d.until("the stimulus off and the lease given back", |s| {
        s.stimulus.phase == StimPhase::Idle
            && s.daemon().is_some_and(|x| {
                !x.generator.armed && !x.generator.firing && x.generator.owner.is_none()
            })
    })?;
    // The new result is the selection.
    d.st.selected_trace
        .ok_or_else(|| "the new sweep is not selected".into())
}

/// The text labels of a scene.
pub fn scene_texts(s: &ac2_scene::primitives::Scene) -> Vec<String> {
    s.layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.clone()))
        .collect()
}
