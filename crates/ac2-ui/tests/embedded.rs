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
use ac2_scene::view::{LeqStyle, SpectrumMode};
use ac2_ui::conn::{Conn, Target};
use ac2_ui::embedded::{
    EmbeddedBackend, EmbeddedError, Setup, start_embedded, start_embedded_with,
};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::{Chord, CommandId, Keymap};
use ac2_ui::state::{AppState, Msg, Overlay, PromptKind, Severity, StimPhase};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

const DEADLINE: Duration = Duration::from_secs(30);
const NAME: &str = "ac2-ui e2e test";

/// The app without its window: messages through the reducer, its requests to the link.
struct Driver {
    st: AppState,
    keys: Keymap,
    conn: Conn,
}

impl Driver {
    fn connect(config: ClientConfig, describe: &str) -> R<Self> {
        let conn = Conn::start(
            Target {
                config,
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
        let mut reqs = self.st.update(m, &self.keys);
        reqs.extend(self.st.sync_link());
        for r in reqs {
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

    /// Esc, until the daemon says stopped and the app has taken the stop's reply: the
    /// mirror's generator event and the link's reply arrive separately, and Space is not
    /// taken while the app still waits for the stop to finish.
    fn stop(&mut self) -> R {
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

/// An open window owns the keyboard: from an empty daemon, with the noise playing, the help
/// and the input setup take ↑/↓, the page keys and Esc and the noise plays on at its level;
/// the stop chord stops it from inside a window.
#[test]
fn windows_leave_the_stimulus_alone_and_the_stop_chord_stops_from_one() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    // The level typed there stays: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    let level = d.st.stimulus.level;
    let playing = |s: &AppState| {
        s.daemon()
            .is_some_and(|x| x.generator.firing && x.generator.armed)
    };

    d.key("H");
    assert_eq!(d.st.overlay, Overlay::Help);
    for k in [
        "ArrowDown",
        "ArrowDown",
        "PageDown",
        "ArrowUp",
        "End",
        "Home",
    ] {
        d.key(k);
    }
    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);

    d.key("Ctrl+K");
    d.send(Msg::Text("input setup".into()));
    d.key("Enter");
    assert!(
        d.st.overlay
            .settings()
            .is_some_and(|s| s.page == ac2_ui::settings::Page::Io),
        "{:?}",
        d.st.overlay
    );
    for k in ["ArrowDown", "ArrowUp", "PageDown", "Home", "Shift+ArrowUp"] {
        d.key(k);
    }
    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);

    // A while later the daemon still plays, at the level it was given.
    let end = Instant::now() + Duration::from_millis(600);
    while Instant::now() < end {
        d.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(playing(&d.st), "the windows touched the stimulus");
    assert_eq!(d.st.stimulus.level, level);
    assert_eq!(d.st.stimulus.phase, StimPhase::Firing);

    // The stop chord from inside a window: stopped, the window still open.
    d.key("H");
    d.key("Shift+Escape");
    assert_eq!(d.st.overlay, Overlay::Help);
    d.until("stopped by the stop chord", |s| {
        s.daemon()
            .is_some_and(|x| !x.generator.firing && !x.generator.armed)
    })?;
    drop(d);
    drop(daemon);
    Ok(())
}

#[test]
fn simulated_rig_starts_measuring() -> R {
    let daemon = start_embedded(EmbeddedBackend::Fake)?;
    assert_eq!(daemon.describe(), "embedded daemon (fake rig)");
    let config = daemon.client_config(NAME);
    // In-process: inproc endpoints in the daemon's own context.
    assert!(config.endpoints.ctrl.starts_with("inproc://"), "{config:?}");
    assert!(config.context.is_some());

    let mut d = Driver::connect(config, &daemon.describe())?;
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
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// The transfer pane's NO REFERENCE detail, while it shows.
fn no_reference(s: &AppState) -> Option<String> {
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 1100.0,
        height: 600.0,
    };
    ac2_ui::scenes::transfer(s, &Theme::dark(), size, now)
        .banners
        .into_iter()
        .find(|b| b.text == "NO REFERENCE")
        .and_then(|b| b.detail)
}

/// From an empty daemon: with the transfer measurement running and nothing playing, NO
/// REFERENCE names the keys (Space arms, then Enter plays); with the noise playing for the
/// only transfer measurement, S stops
/// the measurement and the stimulus with it (faded and released as Esc does), and one toast
/// says both.
#[test]
fn stopping_the_last_transfer_stops_the_noise_from_the_app() -> R {
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    assert_eq!(d.st.layout.focus, PaneKind::Transfer);

    // Running with nothing playing: NO REFERENCE says which keys start the noise.
    d.until("the off reminder", |s| {
        no_reference(s).as_deref() == Some("stimulus off: Space arms, Enter starts it")
    })?;
    d.key("Space");
    d.until("armed and its reminder", |s| {
        s.stimulus.phase == StimPhase::Armed
            && no_reference(s).as_deref() == Some("stimulus armed: Enter starts it")
    })?;
    d.key("Enter");
    d.until("pink playing", |s| {
        s.daemon().is_some_and(|x| {
            x.generator.firing
                && x.generator.settings.as_ref().map(|g| g.signal)
                    == Some(ac2_proto::model::Signal::Pink)
        })
    })?;
    // Only what this stop says.
    d.st.toasts.clear();
    d.key("S");
    d.until("the measurement and the noise stopped", |s| {
        s.stimulus.phase == StimPhase::Idle
            && s.meas(m.id).is_some_and(|x| !x.running)
            && s.daemon().is_some_and(|x| {
                !x.generator.firing && !x.generator.armed && x.generator.owner.is_none()
            })
    })?;
    let stopped = format!(
        "{} stopped · stimulus stopped (no transfer measurement left running)",
        m.config.name
    );
    d.until("the toast", |s| s.toasts.iter().any(|t| t.text == stopped))?;
    assert!(
        !d.st.toasts.iter().any(|t| t.text == "stimulus stopped"),
        "said twice"
    );
    drop(d);
    drop(daemon);
    Ok(())
}

/// The IR pane from an empty daemon: the wheel zooms its time axis about the pointer, a drag
/// pans it, Ctrl+wheel zooms the amplitude, a click puts the cursor on the arrival and the
/// readout reads that sample of the IR as drawn.
#[test]
fn the_ir_pane_zooms_pans_and_reads_from_the_mouse() -> R {
    use ac2_scene::view::IrPane;
    use ac2_ui::state::IrNavMsg;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.key("Alt+3");
    // The level typed before is kept: Space arms, Enter fires.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    // The rig's −6 dB path: an arrival of about 0.5 FS.
    d.until("an IR with its arrival", |s| {
        s.ir_frame_of(IrPane::Live)
            .is_some_and(|f| f.linear.iter().map(|x| x.abs()).fold(0.0, f32::max) > 0.3)
    })?;
    d.stop()?;
    let full = d.st.ir_extent(IrPane::Live).ok_or("extent")?;
    // Wheel up (about four notches) with the pointer on t = 0.
    d.send(Msg::IrNav(
        IrPane::Live,
        IrNavMsg::Zoom {
            about_ms: 0.0,
            factor: 4.0,
        },
    ));
    let r = d.st.view.ir.axes.time_ms.ok_or("zoomed")?;
    assert!((r.span() - full.full.span() / 4.0).abs() < 1e-9, "{r:?}");
    let frac = |r: ac2_scene::axis::Range| -r.lo / r.span();
    assert!((frac(r) - frac(full.full)).abs() < 1e-9, "t = 0 stays put");
    // A drag of a tenth of the width to the right shows earlier times.
    d.send(Msg::IrNav(
        IrPane::Live,
        IrNavMsg::Pan {
            ms: -r.span() / 10.0,
        },
    ));
    let p = d.st.view.ir.axes.time_ms.ok_or("panned")?;
    assert!((p.lo - (r.lo - r.span() / 10.0)).abs() < 1e-9, "{p:?}");
    d.send(Msg::IrNav(
        IrPane::Live,
        IrNavMsg::ValueZoom {
            about: Some(0.0),
            factor: 2.0,
        },
    ));
    let a = d.st.view.ir.axes.amplitude.ok_or("amplitude")?;
    assert!((a.lo + a.hi).abs() < 1e-9, "zoomed about 0: {a:?}");
    // A click at the arrival: the cursor on its sample, the readout that sample's value.
    d.send(Msg::IrNav(IrPane::Live, IrNavMsg::Cursor { t_ms: 0.0 }));
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 1100.0,
        height: 600.0,
    };
    let s = ac2_ui::scenes::ir(&d.st, &d.keys, &Theme::dark(), size, now).ok_or("scene")?;
    let cur = s.cursor.ok_or("no cursor")?;
    assert!(cur.t_ms.abs() <= full.dt_ms / 2.0 + 1e-9, "{cur:?}");
    let f = d.st.ir_frame_of(IrPane::Live).ok_or("frame")?;
    let i = ac2_scene::ir::extent(&f).nearest(0.0).ok_or("sample")?;
    let want = ac2_scene::format::amplitude_readout(f64::from(f.linear[i]));
    assert_eq!(cur.value, want);
    assert!(
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .any(|l| l.text == cur.text()),
        "the readout is drawn"
    );
    drop(d);
    drop(daemon);
    Ok(())
}

/// The measurement's own delay from the keys: Ctrl+. a sample later, Alt+, a tenth earlier.
/// The daemon keeps the averages, so the very next frames carry the new delay with the
/// 1 kHz column still valid (never back to settling), and the measurement list shows the
/// delay to the microsecond.
#[test]
fn delay_nudges_from_the_keys_keep_the_curve() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    // The level typed there stays: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    d.tf_frames(m.id, 240)?;
    let applied = |s: &AppState| {
        s.selected_meas()
            .and_then(|m| m.delay.as_ref())
            .map(|d| (d.applied.0, d.applied_samples))
    };
    let (_, before) = applied(&d.st).ok_or("delay")?;
    for k in ["Ctrl+.", "Ctrl+.", "Ctrl+.", "Alt+,"] {
        d.key(k);
    }
    let want = before + 2.9;
    d.until("the nudged delay", |s| {
        applied(s).is_some_and(|(_, n)| (n - want).abs() < 1e-6)
    })?;
    let (secs, _) = applied(&d.st).ok_or("delay")?;
    let rate = f64::from(d.st.open_session().ok_or("session")?.sample_rate_hz);
    assert!((secs - want / rate).abs() < 1e-12);
    assert_eq!(
        ac2_scene::format::delay(secs),
        format!("{:.3} ms", want / rate * 1000.0)
    );
    let topic = Topic::Data {
        meas: m.id,
        stream: Stream::Tf,
    };
    d.until("a frame at the new delay, 1 kHz still valid", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Tf(tf) => {
                    (tf.meta.delay.0 - secs).abs() < 1e-12
                        && tf.validity[240] == ac2_proto::frame::ValidityMask::NONE
                }
                _ => false,
            })
        })
    })?;
    assert!(
        d.st.toasts
            .iter()
            .any(|t| t.text.ends_with("delay −0.1 sample")),
        "{:?}",
        d.st.toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
    );
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// The transfer pane as drawn: each curve's wrapped phase at column `col`, and its legend.
fn drawn_phase(s: &AppState, col: usize) -> Vec<(ac2_scene::trace::TraceKey, f64, String)> {
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 1100.0,
        height: 600.0,
    };
    let sc = ac2_ui::scenes::transfer(s, &Theme::dark(), size, now);
    sc.traces
        .iter()
        .map(|t| {
            let legend = sc
                .legend
                .iter()
                .find(|l| l.key == t.key)
                .map_or(String::new(), |l| l.text.clone());
            (t.key, t.phase_wrapped_deg[col], legend)
        })
        .collect()
}

fn phase_of(v: &[(ac2_scene::trace::TraceKey, f64, String)], k: ac2_scene::trace::TraceKey) -> f64 {
    v.iter().find(|x| x.0 == k).map_or(f64::NAN, |x| x.1)
}

/// The operator's case from an empty daemon: a running transfer measurement whose live
/// curve is the phase reference, a capture of it and a sweep run on the same pane. Ctrl+.
/// steps of the measurement's delay move its live curve the way `.` moves a stored trace
/// (both later: phase leads more, e^{+jωΔ}), and neither stored curve moves at all, though
/// the moved curve is the reference. Stopped, the keys say why they do nothing.
#[test]
fn a_measurement_delay_step_moves_only_its_live_curve() -> R {
    use ac2_scene::trace::TraceKey;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    let run = sweep_from_the_dialog(&mut d)?;
    d.send(Msg::Command(CommandId::FocusTransfer));
    for _ in 0..4 {
        if d.st.selected == Some(m.id) {
            break;
        }
        d.send(Msg::Command(CommandId::NextMeasurement));
    }
    assert_eq!(d.st.selected, Some(m.id));
    let running = |s: &AppState| {
        s.daemon()
            .is_some_and(|x| x.measurements.iter().any(|y| y.id == m.id && y.running))
    };
    if !running(&d.st) {
        d.send(Msg::Command(CommandId::StartStop));
    }
    d.until("running", |s| {
        s.daemon()
            .is_some_and(|x| x.measurements.iter().any(|y| y.id == m.id && y.running))
    })?;
    // The level typed before stays: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    d.tf_frames(m.id, 240)?;
    d.send(Msg::Command(CommandId::Slot1));
    d.until("the capture in slot 1", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().any(|t| t.edit.slot == Some(1)))
    })?;
    let cap =
        d.st.daemon()
            .and_then(|x| x.traces.iter().find(|t| t.edit.slot == Some(1)))
            .map(|t| t.id)
            .ok_or("capture")?;
    d.until("the capture's data", |s| s.traces.contains_key(&cap))?;
    d.until("the run's data", |s| s.traces.contains_key(&run))?;
    let live = TraceKey::Live(m.id);
    let before = drawn_phase(&d.st, 240);
    let legend = |v: &[(TraceKey, f64, String)], k| {
        v.iter()
            .find(|x| x.0 == k)
            .map_or(String::new(), |x| x.2.clone())
    };
    assert!(legend(&before, live).contains("· ref"), "{before:?}");
    let applied = |s: &AppState| {
        s.daemon()
            .and_then(|x| x.measurements.iter().find(|y| y.id == m.id))
            .and_then(|m| m.delay.as_ref())
            .map(|d| d.applied_samples)
    };
    let a0 = applied(&d.st).ok_or("delay")?;
    for _ in 0..10 {
        d.key("Ctrl+.");
    }
    let want = a0 + 10.0;
    let topic = Topic::Data {
        meas: m.id,
        stream: Stream::Tf,
    };
    let rate = f64::from(d.st.open_session().ok_or("session")?.sample_rate_hz);
    d.until("a frame at the new delay", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Tf(tf) => (tf.meta.delay.0 - want / rate).abs() < 1e-9,
                _ => false,
            })
        })
    })?;
    let after = drawn_phase(&d.st, 240);
    let f = 1000.0;
    let wrap = |x: f64| (x + 180.0).rem_euclid(360.0) - 180.0;
    let moved = wrap(phase_of(&after, live) - phase_of(&before, live));
    // Ten samples later: the curve leads by 360°·f·10/fs (75° at 1 kHz, 48 kHz).
    let expect = 360.0 * f * 10.0 / rate;
    assert!(
        (moved - expect).abs() < 5.0,
        "live moved {moved:.1}°, expected {expect:.1}° — before {before:?} after {after:?}"
    );
    for k in [TraceKey::Stored(cap), TraceKey::Stored(run)] {
        assert_eq!(
            phase_of(&after, k).to_bits(),
            phase_of(&before, k).to_bits(),
            "{k:?} moved: before {before:?} after {after:?}"
        );
    }
    assert!(legend(&after, live).contains("· ref"), "{after:?}");
    assert!(
        legend(&after, live).contains(&format!(
            "nudge {} ms",
            ac2_scene::format::signed(10.0 / rate * 1000.0, 2)
        )),
        "{after:?}"
    );
    // A capture now carries the steps as its display nudge and is drawn where the live
    // curve is.
    d.send(Msg::Command(CommandId::Slot2));
    d.until("the capture in slot 2", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().any(|t| t.edit.slot == Some(2)))
    })?;
    let cap2 =
        d.st.daemon()
            .and_then(|x| x.traces.iter().find(|t| t.edit.slot == Some(2)))
            .ok_or("capture 2")?
            .clone();
    assert!(
        (cap2.edit.delay_nudge.0 - 10.0 / rate).abs() < 1e-12,
        "{:?}",
        cap2.edit
    );
    d.until("the second capture's data", |s| {
        s.traces.contains_key(&cap2.id)
    })?;
    let with2 = drawn_phase(&d.st, 240);
    let off = wrap(phase_of(&with2, TraceKey::Stored(cap2.id)) - phase_of(&with2, live));
    assert!(
        off.abs() < 3.0,
        "capture {off:.1}° off the live curve: {with2:?}"
    );
    // The same direction as `.` on the stored capture: a 0.1 ms step leads by 36°.
    for _ in 0..6 {
        if d.st.selected_trace == Some(cap) {
            break;
        }
        d.send(Msg::Command(CommandId::NextTrace));
    }
    assert_eq!(d.st.selected_trace, Some(cap));
    let pre = drawn_phase(&d.st, 240);
    let n0 =
        d.st.daemon()
            .and_then(|x| x.traces.iter().find(|t| t.id == cap))
            .map(|t| t.edit.delay_nudge.0)
            .ok_or("capture")?;
    d.key(".");
    d.until("the capture nudged", |s| {
        s.daemon().is_some_and(|x| {
            x.traces
                .iter()
                .any(|t| t.id == cap && t.edit.delay_nudge.0 > n0 + 0.000_05)
        })
    })?;
    let post = drawn_phase(&d.st, 240);
    let nudged =
        wrap(phase_of(&post, TraceKey::Stored(cap)) - phase_of(&pre, TraceKey::Stored(cap)));
    assert!(
        moved.signum() == nudged.signum() && (nudged - 36.0).abs() < 1.0,
        "Ctrl+. moved the live curve {moved:.1}°, `.` the capture {nudged:.1}°"
    );

    // The sweep run as the reference: the steps move the live curve against it, alone.
    for _ in 0..8 {
        if d.st.selected_trace == Some(run) {
            break;
        }
        d.send(Msg::Command(CommandId::NextTrace));
    }
    assert_eq!(d.st.selected_trace, Some(run));
    d.send(Msg::Command(CommandId::PhaseReference));
    let pre = drawn_phase(&d.st, 240);
    assert!(
        legend(&pre, TraceKey::Stored(run)).contains("· ref"),
        "{pre:?}"
    );
    for _ in 0..10 {
        d.key("Ctrl+,");
    }
    d.until("a frame back at the first delay", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Tf(tf) => (tf.meta.delay.0 - a0 / rate).abs() < 1e-9,
                _ => false,
            })
        })
    })?;
    let post = drawn_phase(&d.st, 240);
    let back = wrap(phase_of(&post, live) - phase_of(&pre, live));
    assert!(
        (back + expect).abs() < 5.0,
        "live moved {back:.1}°, expected {:.1}° — before {pre:?} after {post:?}",
        -expect
    );
    for (k, _, _) in &pre {
        if *k != live {
            assert_eq!(
                phase_of(&post, *k).to_bits(),
                phase_of(&pre, *k).to_bits(),
                "{k:?} moved: before {pre:?} after {post:?}"
            );
        }
    }
    d.stop()?;

    // Stopped: no live curve to move, so the keys change nothing and say why.
    d.send(Msg::Command(CommandId::StartStop));
    d.until("stopped", |s| {
        s.daemon()
            .is_some_and(|x| x.measurements.iter().any(|y| y.id == m.id && !y.running))
    })?;
    let a1 = applied(&d.st).ok_or("delay")?;
    d.key("Ctrl+.");
    let toast =
        d.st.toasts
            .last()
            .map(|t| t.text.clone())
            .unwrap_or_default();
    assert!(toast.contains("is stopped"), "{toast}");
    d.synced()?;
    assert_eq!(applied(&d.st), Some(a1));
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
    let mut d = Driver::connect(ClientConfig::new(ep, NAME), "local daemon")?;
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
fn session_dialog_meters_return_every_round() -> R {
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
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;

    let sweeps_before = sweep_count(&d.st);
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
    arm_new_sweep(&mut d, sweeps_before)?;
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
    // The cursor at 1 kHz (a click in the pane) reads every order there, in dB and in %;
    // the IR view (G) keeps its own time cursor.
    d.send(Msg::CursorAt(Some(1000.0)));
    let rows = |d: &Driver| -> R<Vec<(String, String)>> {
        let now = ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        };
        let size = ac2_scene::primitives::Viewport {
            width: 1100.0,
            height: 600.0,
        };
        match ac2_ui::scenes::sweep(&d.st, &Theme::dark(), size, now) {
            ac2_ui::scenes::SweepPane::Distortion(s) => {
                Ok(s.cursor.ok_or("no distortion cursor")?.rows)
            }
            _ => Err("not the distortion view".into()),
        }
    };
    let value = |rows: &[(String, String)], n: &str| {
        rows.iter()
            .find(|r| r.0 == n)
            .map(|r| r.1.clone())
            .unwrap_or_default()
    };
    let db = rows(&d)?;
    let h2: f64 = value(&db, "H2")
        .trim_end_matches(" dB")
        .replace('\u{2212}', "-")
        .parse()?;
    assert!((h2 + 40.0).abs() < 1.0, "{db:?}");
    assert!(value(&db, "THD").ends_with(" dB"), "{db:?}");
    d.key("U");
    let pc = rows(&d)?;
    let h2: f64 = value(&pc, "H2").trim_end_matches(" %").parse()?;
    assert!((h2 - 1.0).abs() < 0.15, "{pc:?}");
    d.key("U");
    d.key("G");
    assert_eq!(
        d.st.ir_target(),
        Some(ac2_scene::view::IrPane::Sweep),
        "the sweep's IR view"
    );
    d.key("C");
    assert!(d.st.view.distortion.ir.cursor_ms.is_some());
    assert_eq!(
        d.st.view.cursor_hz,
        Some(1000.0),
        "the frequency cursor stays"
    );
    d.key("G");
    d.key("G");
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

/// Room parameters from the app: from an empty daemon on the simulated rig, the session
/// dialog takes the rig's hall mic (in 3) as a second mic, a sweep plays to it with 2 s of
/// silence after it, and the sweep's impulse response (Shift+I) carries the octave table
/// with the hall's reverberation time (0.8 s), every string from `ac2_scene::room`.
#[test]
fn room_parameters_of_a_sweep_from_the_app() -> R {
    use ac2_ui::forms::FieldId;
    use ac2_ui::session_dialog::Row;
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    d.synced()?;
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the meters of the device", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.session.device_info().is_some())
            && s.input_meters().len() == 4
    })?;
    d.send(Msg::Session(ac2_ui::state::SessionMsg::Focus(Row::Input(
        2,
    ))));
    d.key("M");
    d.key("Enter");
    d.until("the session", |s| s.open_session().is_some())?;
    let open = d.st.open_session().cloned().ok_or("session")?;
    assert_eq!(open.config.input_channels, vec![0, 1, 2]);
    d.until("the measurement offer", |s| {
        matches!(s.overlay, Overlay::Offer(_))
    })?;
    d.key("Escape");

    let sweeps_before = sweep_count(&d.st);
    d.key("Shift+S");
    d.send(Msg::Text("S".into()));
    d.until(
        "the sweep dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Sweep),
    )?;
    if let Overlay::Form(f) = &mut d.st.overlay {
        assert!(f.set_channel(FieldId::Measurement, 2), "the hall mic");
        f.set_text(FieldId::Level, "-20");
        f.set_text(FieldId::From, "100 Hz");
        f.set_text(FieldId::To, "10 kHz");
        let at = |f: &ac2_ui::forms::Form, id| f.fields.iter().position(|x| x.id == id);
        f.focus = at(f, FieldId::Duration).ok_or("duration")?;
        f.cycle(-1);
        f.focus = at(f, FieldId::Tail).ok_or("tail")?;
        assert_eq!(f.fields[f.focus].display(), "1 s (small rooms)");
        f.cycle(1);
        assert_eq!(f.fields[f.focus].display(), "2 s");
    }
    d.key("Enter");
    arm_new_sweep(&mut d, sweeps_before)?;
    d.until("armed with the sweep", |s| {
        s.stimulus.phase == StimPhase::Armed && s.daemon().is_some_and(|x| x.generator.armed)
    })?;
    d.key("Enter");
    d.until("the sweep stored and shown", |s| {
        s.sweep.run.is_none() && s.layout.focus == PaneKind::Distortion && s.shown_sweep().is_some()
    })?;
    d.key("Shift+I");
    assert_eq!(d.st.view.distortion.mode, ac2_scene::view::SweepMode::Ir);
    let theme = Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 1100.0,
        height: 600.0,
    };
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let ac2_ui::scenes::SweepPane::Ir(ir) = ac2_ui::scenes::sweep(&d.st, &theme, size, now) else {
        return Err("the sweep pane does not show the impulse response".into());
    };
    let t = ir.room.as_ref().ok_or("no room table")?;
    assert!(
        t.caption
            .starts_with("Room (ISO 3382-1) · octave bands · decay to "),
        "{}",
        t.caption
    );
    assert_eq!(t.bands, ["250", "500", "1k", "2k", "4k", "All"]);
    let row = |p| t.rows.iter().find(|r| r.param == p).ok_or("row");
    for p in [ac2_scene::room::Param::T20, ac2_scene::room::Param::T30] {
        for c in &row(p)?.cells {
            let v: f64 = c.text.trim_end_matches('*').parse()?;
            assert!((v / 0.8 - 1.0).abs() < 0.1, "{} {}: {v}", p.name(), c.text);
        }
    }
    // The table is drawn: its strings are in the scene.
    let labels: Vec<&str> = ir
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|x| x.text.as_str()))
        .collect();
    assert!(labels.contains(&"T30 (s)") && labels.contains(&t.caption.as_str()));

    // G: the room parameters alone, the whole pane, every band at a larger size.
    d.key("G");
    assert_eq!(d.st.view.distortion.mode, ac2_scene::view::SweepMode::Room);
    let ac2_ui::scenes::SweepPane::Room(room) = ac2_ui::scenes::sweep(&d.st, &theme, size, now)
    else {
        return Err("the sweep pane does not show the room parameters".into());
    };
    let rt = room.table.as_ref().ok_or("no room table")?;
    assert_eq!(rt.bands, t.bands);
    assert_eq!(rt.rows, t.rows);
    assert!(room.font_size > theme.font_size, "{}", room.font_size);
    let labels: Vec<&str> = room
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|x| x.text.as_str()))
        .collect();
    assert!(labels.contains(&"T30 (s)") && labels.contains(&"250"));
    assert!(!labels.iter().any(|x| x.contains("hidden")), "{labels:?}");
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
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
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
    arm_new_sweep(&mut d, sweeps_before)?;
    d.until("armed with the sweep", |s| {
        s.stimulus.phase == StimPhase::Armed && s.daemon().is_some_and(|x| x.generator.armed)
    })?;
    d.key("Enter");
    let step = |s: &AppState| s.operation().map(|p| p.step);
    d.until("sweep 1 of 2", |s| {
        step(s).as_deref() == Some("sweep 1 of 2")
    })?;
    let p = d.st.operation().ok_or("progress")?;
    assert_eq!(p.title, "sweep \"Run 1\"");
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
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    d.synced()?;
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the device list", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.session.device_info().is_some())
    })?;
    // The rig's mic is on input 2, on the Inputs & outputs page: N names it.
    d.key("Ctrl+PageUp");
    d.key("ArrowDown");
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

    // The calibrations open on the selected measurement's input.
    d.key("Ctrl+K");
    d.send(Msg::Text("calibrations".into()));
    d.key("Enter");
    assert!(d.st.overlay.cal().is_some(), "{:?}", d.st.overlay);
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
    // Esc closes the view and leaves the noise playing; the next Esc stops it.
    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);
    assert!(d.st.daemon().is_some_and(|x| x.generator.firing));
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// Calibrates input 2 of the simulated rig against a 1 kHz tone as `ac2 cal spl` would (the
/// app has no calibration flow of its own): a client of its own takes the stimulus lease,
/// plays the tone at −20 dBFS and asks for 94 dB until the reading is steady, then gives
/// the lease back.
fn calibrate(config: &ClientConfig) -> R {
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
    let ep = daemon.client_config(NAME);
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
    d.until("the Leq dialog", |s| s.overlay.leq().is_some())?;
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
    if let Some(x) = d.st.overlay.leq() {
        let names: Vec<String> = x
            .rows
            .iter()
            .map(|r| r.cell(ac2_ui::leq_dialog::Col::Length))
            .collect();
        assert_eq!(names[..2], ["LAeq 5 s", "LAeq 10 s"], "{names:?}");
    }
    d.key("Enter");
    assert_eq!(d.st.overlay, Overlay::None);
    assert!(
        d.st.view.spl.mode.shows_leq(),
        "the SPL pane shows the windows"
    );
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
    let seen_toast = d.st.toasts.last().map_or(0, |t| t.id);
    let new_toasts = move |s: &AppState, what: &str, error: bool| {
        s.toasts
            .iter()
            .filter(|t| {
                t.id > seen_toast
                    && (t.severity != Severity::Info) == error
                    && t.text.contains(what)
            })
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
        // One weighting: the caption says LAeq once, the columns only their lengths.
        ("5 s", "10 s")
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
        .map(|a| (a.subject.duration().map_or(f64::NAN, |d| d.0), a.kind))
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

/// A peak limit and a measuring-position correction from an empty daemon, using the app:
/// in the Leq dialog ↑ from the preset row reaches the correction (4 dB), Shift+Tab twice
/// the LCpeak limit (95 dB). The pane gets an LCpeak column right of the windows, every
/// value marked corrected and the caption saying by how much; pink noise from the keys
/// puts LCpeak over (a toast naming LCpeak and the correction); stopped, it stays over for
/// the 10 s hold, then recovers (a toast).
#[test]
fn a_peak_limit_and_the_position_correction_from_the_app() -> R {
    use ac2_proto::model::{LeqJudgement, PeakQuantity, PositionCorrection};
    use ac2_scene::leq::{TileKind, TileState};
    use ac2_ui::leq_dialog::{Extra, Focus};
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
    d.until("the SPL meter running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
    })?;
    calibrate(&ep)?;

    d.key("Shift+L");
    d.send(Msg::Text("L".into()));
    d.until("the Leq dialog", |s| s.overlay.leq().is_some())?;
    d.key("ArrowUp");
    d.key("ArrowUp");
    let Some(x) = d.st.overlay.leq() else {
        return Err("the Leq dialog".into());
    };
    assert_eq!(x.focus, Focus::Extra(Extra::Position));
    d.send(Msg::Text("4".into()));
    d.key("Shift+Tab");
    d.key("Shift+Tab");
    d.send(Msg::Text("95".into()));
    d.key("Enter");
    assert_eq!(d.st.overlay, Overlay::None);
    d.until("the meter with the limit and the correction", |s| {
        s.measurements().iter().any(|m| match &m.config.kind {
            MeasKind::Spl { config } => {
                config.position == Some(PositionCorrection::both(4.0))
                    && config.leq.peaks.lcpeak.is_some()
            }
            _ => false,
        })
    })?;
    d.until("an LCpeak tile, corrected", |s| {
        tiles(s).iter().any(|t| {
            t.kind == TileKind::Peak(PeakQuantity::LcPeak)
                && t.weighted_unit == "dB(C) corr."
                && t.corrected.as_deref() == Some("corrected +4.0 dB")
        })
    })?;
    // The LCpeak column right of the windows, named whole; the caption names the
    // correction.
    let c = columns(&d.st);
    assert_eq!(c.last().map(|x| x.0.as_str()), Some("LCpeak"), "{c:?}");
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
    let scene = ac2_ui::scenes::leq(&d.st, &Theme::dark(), size, now).ok_or("the Leq view")?;
    let texts = scene_texts(&scene.scene);
    assert!(
        texts.iter().any(|t| t.contains("corrected +4.0 dB")),
        "{texts:?}"
    );

    // The calibrator's seconds (97 dB peaks, 101 corrected) leave the 10 s hold first.
    d.until("LCpeak judged and not over", |s| {
        spl_log(s).is_some_and(|l| {
            matches!(
                l.peaks.lcpeak.judgement,
                LeqJudgement::Ok | LeqJudgement::Near
            )
        })
    })?;
    let seen_toast = d.st.toasts.last().map_or(0, |t| t.id);
    let new_toasts = move |s: &AppState, what: &str, error: bool| {
        s.toasts
            .iter()
            .filter(|t| {
                t.id > seen_toast
                    && (t.severity != Severity::Info) == error
                    && t.text.contains(what)
            })
            .count()
    };
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("LCpeak over", |s| {
        spl_log(s).is_some_and(|l| l.peaks.lcpeak.judgement == LeqJudgement::Over)
    })?;
    d.until("its over toast, corrected", |s| {
        new_toasts(s, "LCpeak over its limit", true) == 1
            && new_toasts(s, "(corrected +4.0 dB)", true) >= 1
    })?;
    d.until("the LCpeak tile red", |s| {
        tiles(s)
            .iter()
            .any(|t| t.name == "LCpeak" && t.state == TileState::Over)
    })?;
    d.stop()?;
    let stopped = Instant::now();
    d.until("LCpeak back under after the hold", |s| {
        spl_log(s).is_some_and(|l| l.peaks.lcpeak.judgement != LeqJudgement::Over)
    })?;
    assert!(
        stopped.elapsed() >= Duration::from_secs(8),
        "held {:?}",
        stopped.elapsed()
    );
    d.until("its recovery toast", |s| {
        new_toasts(s, "LCpeak back within its limit", false) == 1
    })?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// The columns as the SPL pane lays them out now (the pane at 1280 × 720).
fn leq_columns(s: &AppState) -> Option<ac2_scene::leq::LeqColumns> {
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
    ac2_ui::scenes::leq(s, &Theme::dark(), size, now).and_then(|x| x.columns)
}

/// Filling windows judged on their budgets, from an empty daemon, using the app: an SPL
/// meter from the palette, calibrated; its default windows (1, 5, 10, 30, 60 min) limited to
/// 80 dB in the Leq dialog; Shift+R, Enter: a new log, so every window fills from nothing.
/// Pink noise at −20 dBFS (about 91 dB(A), 12 times the limit's power) puts every window's
/// Leq so far over 80 at once: all amber, "ON COURSE". The 1 min window has spent its budget
/// after about 5 s and turns red; the 5 min one after about 25 s; the 10, 30 and 60 min ones
/// stay amber on course, their bars under the limit line, with the time until their budgets
/// are spent; the only alarms are the two short windows going over.
#[test]
fn filling_windows_go_red_only_when_their_budget_is_spent() -> R {
    use ac2_proto::model::{LeqAlarmKind, LeqJudgement};
    use ac2_scene::leq::TileState;
    use ac2_ui::state::PaneKind;
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
    d.until("the SPL meter running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
    })?;
    d.until("its windows", |s| {
        judgement(s, 4) == Some(LeqJudgement::NoLimit)
    })?;
    calibrate(&ep)?;

    // Shift+L: ↓↓ to the first window, Tab Tab to its limit, 80; ↓ and 80 for each other.
    d.key("Shift+L");
    d.send(Msg::Text("L".into()));
    d.until("the Leq dialog", |s| s.overlay.leq().is_some())?;
    for k in ["ArrowDown", "ArrowDown", "Tab", "Tab"] {
        d.key(k);
    }
    for i in 0..5 {
        if i > 0 {
            d.key("ArrowDown");
        }
        d.send(Msg::Text("80".into()));
    }
    d.key("Enter");
    assert_eq!(d.st.overlay, Overlay::None);
    assert_eq!(d.st.layout.focus, PaneKind::Spl);
    d.until("five windows judged", |s| {
        (0..5).all(|i| {
            judgement(s, i).is_some_and(|j| {
                matches!(
                    j,
                    LeqJudgement::Ok | LeqJudgement::Near | LeqJudgement::Over
                )
            })
        })
    })?;
    d.until("the frames judging the 80 dB limits", |s| {
        let limits: Vec<Option<f64>> = tiles(s).iter().map(|t| t.limit_db).collect();
        limits == [Some(80.0); 5]
    })?;

    // A new log: the calibrator's seconds go, every window fills from nothing.
    d.key("Shift+R");
    d.key("Enter");
    d.until("the new log", |s| {
        s.toasts
            .iter()
            .any(|t| t.text.contains("new SPL log started"))
            && spl_log(s).is_some_and(|l| l.alarms.is_empty())
            && tiles(s)
                .first()
                .is_some_and(|t| t.elapsed_s < 30.0 && t.filling())
    })?;

    // Loud: every window on course at once, amber, the value the Leq so far.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    let th = Theme::dark();
    let amber = th.banner_warning.background;
    let red = th.banner_fault.background;
    d.until("every window amber, on course", |s| {
        let t = tiles(s);
        t.len() == 5
            && t.iter().all(|x| {
                x.state == TileState::Near
                    && x.on_course
                    && x.state_text.as_deref() == Some("ON COURSE")
            })
    })?;
    let t = tiles(&d.st);
    assert!(
        t[4].course
            .as_deref()
            .is_some_and(|c| c.starts_with("on course — over in ")),
        "{:?}",
        t[4].course
    );
    assert!(
        t[4].filling
            .as_deref()
            .is_some_and(|f| f.starts_with("so far · ")),
        "{:?}",
        t[4].filling
    );
    assert!(
        t[4].value.parse::<f64>().is_ok_and(|v| v > 80.0),
        "{}",
        t[4].value
    );

    // The 1 min window spends its budget first: red, its alarm; the others still amber.
    d.until("the 1 min window red", |s| {
        tiles(s).first().is_some_and(|t| t.state == TileState::Over)
    })?;
    let t = tiles(&d.st);
    assert!(t[0].filling(), "red while filling: {:?}", t[0].filling);
    assert!(t[1..].iter().all(|x| x.on_course), "{t:?}");
    // The 5 min window red too, the long ones amber on course.
    d.until("the 5 min window red", |s| {
        tiles(s).get(1).is_some_and(|t| t.state == TileState::Over)
    })?;
    let t = tiles(&d.st);
    for x in &t[2..] {
        assert_eq!(x.state, TileState::Near, "{}", x.name);
        assert_eq!(x.state_text.as_deref(), Some("ON COURSE"), "{}", x.name);
        assert!(x.bar_db() < 80.0 && x.leq_db > 80.0, "{x:?}");
    }
    let k = leq_columns(&d.st).ok_or("columns")?;
    let col = |w: usize| k.columns.iter().find(|c| c.window == w).ok_or("column");
    for w in 0..2 {
        assert_eq!(col(w)?.bar_color, red);
    }
    let long = col(4)?;
    assert_eq!(long.bar_color, amber);
    assert!(long.filling);
    let (bar, limit_y) = (long.bar.ok_or("bar")?, long.limit_y.ok_or("limit")?);
    assert!(bar.y > limit_y, "the 60 min bar under its limit line");
    let l = spl_log(&d.st).ok_or("log")?;
    let alarms: Vec<_> = l
        .alarms
        .iter()
        .map(|a| (a.subject.duration().map_or(f64::NAN, |d| d.0), a.kind))
        .collect();
    assert_eq!(
        alarms,
        [(60.0, LeqAlarmKind::Over), (300.0, LeqAlarmKind::Over)]
    );
    assert!(
        l.windows[2..]
            .iter()
            .all(|w| w.judgement == LeqJudgement::Near)
    );
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// The caption's run as the SPL pane draws it now (the pane at 1280 × 720).
fn run_caption(s: &AppState) -> Option<String> {
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
    ac2_ui::scenes::leq(s, &Theme::dark(), size, now).and_then(|x| x.run)
}

/// Seconds on the caption's run clock (`running 0:01:05 …`).
fn run_seconds(s: &AppState) -> Option<u64> {
    let text = run_caption(s)?;
    let clock = text.strip_prefix("running ")?.split(' ').next()?;
    let parts: Vec<u64> = clock
        .split(':')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    match parts[..] {
        [h, m, sec] => Some(h * 3600 + m * 60 + sec),
        _ => None,
    }
}

/// The run clock and a new log from an empty daemon, using the app: a session and an SPL
/// meter from the app; the Leq view's caption counts the meter's log (`running 0:00:05
/// since … · LAeq total …`); Shift+R asks first, naming the run that ends; N keeps it,
/// Shift+R and Enter start a new log, and the clock starts again from zero.
#[test]
fn run_clock_and_a_new_log_from_the_app() -> R {
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let ep = daemon.client_config(NAME);
    let mut d = Driver::connect(ep.clone(), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.stop()?;
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
    // G: the windows; the run (clock, start, total, offline) only with the history on.
    d.key("Alt+4");
    assert_eq!(d.st.layout.focus, PaneKind::Spl);
    if !d.st.view.spl.mode.shows_leq() {
        d.key("G");
    }
    assert!(!d.st.view.spl.layout.history);
    d.key("Shift+B");
    assert!(d.st.view.spl.layout.history);
    d.until("the run clock past 4 s", |s| {
        run_seconds(s).is_some_and(|t| t >= 4)
    })?;
    let caption = run_caption(&d.st).ok_or("caption")?;
    assert!(caption.contains(" since "), "{caption}");
    assert!(caption.contains("LAeq total "), "{caption}");
    // Shift+B off: the run goes with the history; on again, it is back.
    d.key("Shift+B");
    assert_eq!(run_caption(&d.st), None);
    d.key("Shift+B");
    assert!(run_caption(&d.st).is_some());
    let before = run_seconds(&d.st).ok_or("clock")?;

    // Shift+R asks first; N keeps the log and its clock.
    d.key("Shift+R");
    let Overlay::NewLog(p) = &d.st.overlay else {
        return Err(format!("no confirmation: {:?}", d.st.overlay).into());
    };
    assert!(p.confirm.title.starts_with("Start a new SPL log for "));
    assert!(
        p.confirm.lines[0].starts_with("The current log ends: running 0:00:"),
        "{:?}",
        p.confirm.lines
    );
    assert!(p.confirm.lines[1].contains("the run clock and the total"));
    d.key("N");
    assert_eq!(d.st.overlay, Overlay::None);
    d.until("the clock going on", |s| {
        run_seconds(s).is_some_and(|t| t >= before + 2)
    })?;
    let ended = run_seconds(&d.st).ok_or("clock")?;

    // Shift+R, Enter: a new log; the clock starts from zero again.
    d.key("Shift+R");
    d.key("Enter");
    assert_eq!(d.st.overlay, Overlay::None);
    d.until("the new log toasted", |s| {
        s.toasts
            .iter()
            .any(|t| t.text.contains("new SPL log started"))
    })?;
    d.until("the clock started again", |s| {
        run_seconds(s).is_some_and(|t| t + 3 < ended)
    })?;
    d.until("the new clock counting", |s| {
        run_seconds(s).is_some_and(|t| t >= 2)
    })?;
    let l = spl_log(&d.st).ok_or("log")?;
    assert!(l.alarms.is_empty());
    drop(d);
    drop(daemon);
    Ok(())
}

/// The SPL meter's history: its id and the points of its first (1 min) window.
fn history_points(s: &AppState) -> Option<(MeasId, Vec<ac2_scene::leq::HistoryPoint>)> {
    let m = s
        .measurements()
        .into_iter()
        .find(|m| matches!(m.config.kind, MeasKind::Spl { .. }))?;
    let MeasKind::Spl { config } = &m.config.kind else {
        return None;
    };
    let (_, h) = s.leq_history.get(&m.id)?;
    let p = h.points(config.leq.windows.first()?)?;
    Some((m.id, p.iter().copied().collect()))
}

/// The history strip as the SPL pane draws it (the pane at 1280 × 720), its first line.
fn strip_line(s: &AppState) -> Vec<[f32; 2]> {
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
        .and_then(|x| x.history)
        .and_then(|h| h.lines.into_iter().next())
        .map(|l| l.points)
        .unwrap_or_default()
}

/// The app restarted while an SPL meter runs: the history strip shows what the meter
/// logged before, rebuilt from its log, the same as the app that was running then saw it
/// (within 0.01 dB). From an empty daemon: the meter from the palette, the stimulus from
/// the keys (−20 dBFS pink noise for some seconds, a level change the 1 min window follows),
/// then a new app on the same daemon. A new log started from that app clears the history
/// of another app watching the meter too.
#[test]
fn a_restarted_app_shows_the_history_from_the_log() -> R {
    use ac2_ui::state::PaneKind;
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
    d.until("a few quiet seconds in the history", |s| {
        history_points(s).is_some_and(|(_, p)| p.len() >= 4)
    })?;
    let quiet = history_points(&d.st).ok_or("history")?.1[1].leq;
    // The level typed before is kept: Space arms, Enter fires.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("the 1 min window 10 dB up", |s| {
        history_points(s).is_some_and(|(_, p)| p.last().is_some_and(|x| x.leq > quiet + 10.0))
    })?;
    d.stop()?;
    let n = history_points(&d.st).ok_or("history")?.1.len();
    d.until("a few more seconds", |s| {
        history_points(s).is_some_and(|(_, p)| p.len() >= n + 3)
    })?;
    let (meas, before) = history_points(&d.st).ok_or("history")?;
    drop(d);

    // The app again, from nothing: the strip holds the seconds before it started.
    let mut d = Driver::connect(ep.clone(), &daemon.describe())?;
    d.synced()?;
    d.key("Alt+4");
    assert_eq!(d.st.layout.focus, PaneKind::Spl);
    if !d.st.view.spl.mode.shows_leq() {
        d.key("G");
    }
    if !d.st.view.spl.layout.history {
        d.send(Msg::Command(CommandId::SplLeqHistory));
    }
    assert!(d.st.view.spl.layout.history);
    let first = before[0].t;
    d.until("the history from before the restart", |s| {
        history_points(s).is_some_and(|(_, p)| p.first().is_some_and(|x| x.t <= first + 0.5))
    })?;
    let (_, after) = history_points(&d.st).ok_or("history")?;
    let mut matched = 0;
    for b in &before {
        let Some(a) = after.iter().find(|a| (a.t - b.t).abs() < 0.5) else {
            continue;
        };
        matched += 1;
        assert!(
            (a.leq - b.leq).abs() < 0.01 || (a.leq.is_nan() && b.leq.is_nan()),
            "{a:?} vs {b:?}"
        );
        assert_eq!(a.over, b.over);
    }
    assert!(
        matched + 2 >= before.len(),
        "{matched} of {} seconds",
        before.len()
    );
    assert!(
        after.iter().any(|p| p.leq > quiet + 10.0),
        "the loud stretch"
    );
    // Live frames go on from there, one point a second.
    let len = after.len();
    d.until("live seconds after the rebuilt ones", |s| {
        history_points(s).is_some_and(|(_, p)| p.len() >= len + 2)
    })?;
    let (_, now) = history_points(&d.st).ok_or("history")?;
    let close: Vec<_> = now.windows(2).filter(|w| w[1].t - w[0].t <= 0.5).collect();
    assert!(close.is_empty(), "no second twice: {close:?} of {now:?}");
    // Drawn: seconds sharing a pixel column of the strip are thinned to its extremes, so
    // the line has at most a point per second, and it runs from the oldest to the newest.
    let line = strip_line(&d.st);
    let xs = line.iter().filter(|p| p[0].is_finite()).map(|p| p[0]);
    let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
    assert!(
        line.len() >= 2 && line.len() <= now.len() && hi > lo,
        "drawn: {line:?}"
    );

    // Another app on the meter; a new log from this one clears both histories.
    let mut other = Driver::connect(ep.clone(), &daemon.describe())?;
    other.synced()?;
    other.until("the other app's history", |s| {
        history_points(s).is_some_and(|(_, p)| p.first().is_some_and(|x| x.t <= first + 0.5))
    })?;
    d.key("Shift+R");
    d.key("Enter");
    d.until("the new log toasted", |s| {
        s.toasts
            .iter()
            .any(|t| t.text.contains("new SPL log started"))
    })?;
    let newest = now.last().map_or(0.0, |p| p.t);
    for x in [&mut d, &mut other] {
        x.until("only the new log's seconds", |s| {
            history_points(s).is_some_and(|(id, p)| {
                id == meas && !p.is_empty() && p.iter().all(|x| x.t > newest)
            })
        })?;
    }
    drop(other);
    drop(d);
    drop(daemon);
    Ok(())
}

/// Rows logged as of the newest `leq` frame of the SPL meter.
fn logged(s: &AppState) -> Option<u64> {
    let m = s
        .measurements()
        .into_iter()
        .find(|m| matches!(m.config.kind, MeasKind::Spl { .. }))?;
    let topic = Topic::Data {
        meas: m.id,
        stream: Stream::Leq,
    };
    match s.data.as_ref()?.latest.get(&topic).map(|f| &f.frame.data) {
        Some(FrameData::Leq(f)) => Some(f.meta.logged),
        _ => None,
    }
}

/// A preset chosen in the Leq dialog replaces the meter's windows: DIN 15905-5 from the
/// preset row, Enter, and the meter has only LAeq 30 min ≤ 99 dB. The log carries on (in
/// place, not a new log), and the history of the new window is rebuilt from it.
#[test]
fn a_preset_replaces_the_windows_from_the_app() -> R {
    use ac2_proto::model::LeqPreset;
    use ac2_proto::units::DbSpl;
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
    d.until("a few seconds logged", |s| {
        logged(s).is_some_and(|n| n >= 4)
    })?;
    let before = logged(&d.st).ok_or("logged")?;

    // Shift+L; → on the preset row: DIN 15905-5, its one window in the dialog; Enter.
    d.key("Shift+L");
    d.send(Msg::Text("L".into()));
    d.until("the Leq dialog", |s| s.overlay.leq().is_some())?;
    d.key("ArrowRight");
    let Some(x) = d.st.overlay.leq() else {
        return Err("the Leq dialog".into());
    };
    assert_eq!(
        x.preset_text(),
        "DIN 15905-5: LAeq 30 min ≤ 99 dB, LCpeak ≤ 135 dB"
    );
    assert!(x.preset_note().contains("replaces the windows"));
    let shown: Vec<(String, String)> = x
        .rows
        .iter()
        .map(|r| (r.cell(ac2_ui::leq_dialog::Col::Length), r.limit.clone()))
        .collect();
    assert_eq!(shown, [("LAeq 30 min".to_owned(), "99".to_owned())]);
    d.key("Enter");
    assert_eq!(d.st.overlay, Overlay::None);
    d.until("only LAeq 30 min ≤ 99 dB", |s| {
        s.measurements().iter().any(|m| match &m.config.kind {
            MeasKind::Spl { config } => config.leq.windows == LeqPreset::Din15905.windows(),
            _ => false,
        })
    })?;
    let w = LeqPreset::Din15905.windows();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].limit, Some(DbSpl(99.0)));
    // The same log: its rows go on counting from where they were.
    d.until("the log going on", |s| {
        logged(s).is_some_and(|n| n >= before + 2)
    })?;
    // The new window's history reaches back over the seconds logged before the change.
    d.until("the 30 min window's history from the log", |s| {
        history_points(s).is_some_and(|(_, p)| p.len() as u64 >= before)
    })?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// Sweep measurements the daemon lists.
fn sweep_count(s: &AppState) -> usize {
    s.daemon().map_or(0, |x| {
        x.measurements
            .iter()
            .filter(|m| matches!(m.config.kind, ac2_proto::model::MeasKind::Sweep { .. }))
            .count()
    })
}

/// After the sweep dialog's Enter: the new sweep measurement is listed and selected, and
/// making it armed nothing; Space on the sweep pane then arms its run.
fn arm_new_sweep(d: &mut Driver, before: usize) -> R {
    d.until("the sweep measurement made, nothing armed", |s| {
        sweep_count(s) > before
            && s.stimulus.phase == StimPhase::Idle
            && s.sweep_meas().is_some_and(|m| Some(m.id) == s.selected)
    })?;
    d.key("Space");
    Ok(())
}

/// One short sweep from the dialog, played and stored (the simulated rig: no real audio).
fn sweep_from_the_dialog(d: &mut Driver) -> R<ac2_proto::units::TraceId> {
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

/// The selected sweep measurement run again (Space, Enter): its second run, selected.
fn run_again(d: &mut Driver) -> R<ac2_proto::units::TraceId> {
    let before = d.st.sweep_traces().len();
    d.key("Space");
    d.until("armed with the sweep", |s| {
        s.stimulus.phase == StimPhase::Armed
    })?;
    d.key("Enter");
    d.until("the run stored, the stimulus off", |s| {
        s.sweep.run.is_none()
            && s.sweep_traces().len() == before + 1
            && s.stimulus.phase == StimPhase::Idle
            && s.daemon().is_some_and(|x| x.generator.owner.is_none())
    })?;
    d.st.selected_trace
        .ok_or_else(|| "the new run is not selected".into())
}

/// From an empty daemon, two sweeps, then the transfer pane chooses between them: V selects
/// each in turn and the sweep pane follows; A hides one, V skips it; N on the sweep pane
/// selects for the transfer pane too.
#[test]
fn two_sweeps_chosen_between_in_the_transfer_pane() -> R {
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let first = sweep_from_the_dialog(&mut d)?;
    let second = run_again(&mut d)?;
    assert_ne!(first, second);
    let shown = |s: &AppState| s.shown_sweep().map(|(t, _)| t.meta.id);
    let name = |s: &AppState, id| {
        s.trace_list()
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.edit.name.clone())
            .unwrap_or_default()
    };
    assert_eq!(shown(&d.st), Some(second));
    let (n1, n2) = (name(&d.st, first), name(&d.st, second));
    assert_ne!(n1, n2);

    // Both in the list, by name, as sweeps; the newest selected.
    let rows = d.st.trace_rows();
    let listed: Vec<(&str, &str, bool, bool)> = rows
        .iter()
        .map(|r| (r.name.as_str(), r.kind, r.shown, r.selected))
        .collect();
    assert_eq!(
        listed,
        [
            (n1.as_str(), "sweep run", true, false),
            (n2.as_str(), "sweep run", true, true)
        ]
    );

    // Live again, the transfer pane focused: V selects the first sweep, then the second;
    // the sweep pane shows whichever is selected.
    d.key("Escape");
    d.key("Alt+1");
    assert_eq!(d.st.layout.focus, PaneKind::Transfer);
    assert_eq!(d.st.selected_trace, None);
    d.key("V");
    assert_eq!(d.st.selected_trace, Some(first));
    assert_eq!(shown(&d.st), Some(first));
    assert_eq!(
        d.st.pane_caption(PaneKind::Transfer),
        Some(format!("{n1}: smoothing off"))
    );
    let theme = Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 500.0,
    };
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let caption = |s: &AppState| match ac2_ui::scenes::sweep(s, &theme, size, now) {
        ac2_ui::scenes::SweepPane::Distortion(x) => x.caption.clone(),
        ac2_ui::scenes::SweepPane::Ir(_) | ac2_ui::scenes::SweepPane::Room(_) => String::new(),
    };
    assert!(caption(&d.st).starts_with(&n1), "{}", caption(&d.st));
    d.key("V");
    assert_eq!(d.st.selected_trace, Some(second));
    assert_eq!(shown(&d.st), Some(second));
    assert!(caption(&d.st).starts_with(&n2), "{}", caption(&d.st));

    // A hides the selected sweep: the transfer pane stops drawing it, V skips it.
    d.key("A");
    d.until("the second sweep hidden", |s| {
        s.trace_rows()
            .iter()
            .any(|r| r.id == second && !r.shown && r.details[0].contains("hidden"))
            && s.traces
                .get(&second)
                .is_some_and(|(t, _)| !t.meta.edit.visible)
    })?;
    let legend: Vec<String> = ac2_ui::scenes::transfer(&d.st, &theme, size, now)
        .legend
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(legend.contains(&n1), "{legend:?}");
    assert!(!legend.contains(&n2), "{legend:?}");
    d.key("V");
    assert_eq!(d.st.selected_trace, None, "past the last shown: live");
    d.key("V");
    assert_eq!(d.st.selected_trace, Some(first));
    d.key("V");
    assert_eq!(d.st.selected_trace, None);
    // Alt+V reaches it hidden; the sweep pane still shows what is selected.
    d.key("Alt+Shift+V");
    assert_eq!(d.st.selected_trace, Some(second));
    assert_eq!(shown(&d.st), Some(second));

    // N on the sweep pane steps the sweeps and selects them for the transfer pane.
    d.key("Alt+5");
    assert_eq!(d.st.layout.focus, PaneKind::Distortion);
    d.key("N");
    assert_eq!(d.st.selected_trace, Some(first));
    assert_eq!(shown(&d.st), Some(first));
    d.key("Alt+1");
    assert_eq!(
        d.st.selected_trace,
        Some(first),
        "kept on the way to the transfer pane"
    );

    // Shown again from the list's eye.
    d.send(Msg::ToggleShown(second));
    d.until("the second sweep shown again", |s| {
        s.trace_rows().iter().all(|r| r.shown)
    })?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// The hint line of the focused pane, as the app draws it (PC labels).
fn hint_line(s: &AppState) -> Vec<String> {
    use ac2_ui::state::PaneKind;
    let focus = s.layout.focus;
    let keys = Keymap::default();
    let style = ac2_ui::keys::LabelStyle::Pc;
    // Only the focused pane has one.
    for p in PaneKind::ALL.into_iter().filter(|p| *p != focus) {
        assert_eq!(s.key_hint_line(&keys, p, style), None);
    }
    s.key_hint_line(&keys, focus, style)
        .map(|v| v.iter().map(ac2_ui::hints::KeyHint::text).collect())
        .unwrap_or_default()
}

/// From an empty daemon: the hint line follows the focused pane as the operator moves
/// around, H opens and closes every key, Shift+H turns the hints off and the palette turns
/// them on again.
#[test]
fn key_hints_follow_the_panes_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    d.synced()?;
    // Nothing set up yet: the transfer pane has the focus and its hints.
    let tf = hint_line(&d.st);
    assert_eq!(tf.first().map(String::as_str), Some("V select trace"));
    assert_eq!(tf.last().map(String::as_str), Some("H all keys"));
    // A session and its transfer measurement, from the app.
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the device list", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.session.device_info().is_some())
    })?;
    d.key("Enter");
    d.until("the measurement offer", |s| {
        matches!(s.overlay, Overlay::Offer(_))
    })?;
    d.key("Enter");
    d.until("the measurement", |s| s.selected_meas().is_some())?;
    assert_eq!(hint_line(&d.st), tf);
    // Each pane its own line.
    d.key("Alt+2");
    let sp = hint_line(&d.st);
    assert!(sp.contains(&"P peak hold".to_owned()), "{sp:?}");
    d.key("Alt+3");
    let ir = hint_line(&d.st);
    assert_eq!(ir.first().map(String::as_str), Some("G linear/log/ETC"));
    d.key("Alt+4");
    let spl = hint_line(&d.st);
    assert_eq!(spl.first().map(String::as_str), Some("G meter/Leq/both"));
    d.key("Alt+5");
    let sw = hint_line(&d.st);
    assert_eq!(sw.first().map(String::as_str), Some("Shift+S new sweep"));
    // Shift+I shows the sweep's IR: dB / % gives way to the IR mode.
    assert!(sw.contains(&"U dB/%".to_owned()), "{sw:?}");
    d.key("Shift+I");
    let sw = hint_line(&d.st);
    assert!(sw.contains(&"Shift+G linear/log/ETC".to_owned()), "{sw:?}");
    // H: every key, and closed again.
    d.key("H");
    assert_eq!(d.st.overlay, Overlay::Help);
    d.key("H");
    assert_eq!(d.st.overlay, Overlay::None);
    // Shift+H: no line anywhere, remembered.
    d.key("Shift+H");
    assert!(!d.st.prefs.key_hints);
    assert!(d.st.prefs_dirty);
    assert!(hint_line(&d.st).is_empty());
    d.key("Alt+1");
    assert!(hint_line(&d.st).is_empty());
    // The palette brings it back.
    d.key("Ctrl+K");
    d.send(Msg::Text("key hints".into()));
    d.key("Enter");
    assert!(d.st.prefs.key_hints);
    assert_eq!(hint_line(&d.st), tf);
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon: two captures, one spread by +3 dB with the keys (its legend says
/// so and its curve moves, the other stays), a spectrum whose level axis goes down to its
/// low levels with the keys, a capture deleted with Delete and a confirmation, and a
/// measurement picked from the list with the layout maximised brings up its pane.
#[test]
fn spread_zoom_and_delete_from_an_empty_daemon() -> R {
    use ac2_proto::units::TraceId;
    use ac2_scene::trace::TraceKey;
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let tf = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;

    // Two captures, their data fetched.
    d.key("Ctrl+1");
    d.until("slot 1", |s| s.slots()[0].is_some())?;
    d.key("Ctrl+2");
    d.until("both captures with their data", |s| {
        s.slots()[1].is_some() && s.traces.len() == 2
    })?;
    let id = |s: &AppState, slot: usize| s.slots()[slot].map(|t| t.id);
    let (a, b) = (id(&d.st, 0).ok_or("slot 1")?, id(&d.st, 1).ok_or("slot 2")?);
    let theme = Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 600.0,
    };
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let curve = |s: &AppState, t: TraceId| {
        let scene = ac2_ui::scenes::transfer(s, &theme, size, now());
        let legend = scene
            .legend
            .iter()
            .find(|e| e.key == TraceKey::Stored(t))
            .map(|e| e.text.clone())
            .unwrap_or_default();
        let mag = scene
            .traces
            .iter()
            .find(|x| x.key == TraceKey::Stored(t))
            .map(|x| x.magnitude_db.clone())
            .unwrap_or_default();
        (legend, mag)
    };
    let (legend_b, before_b) = curve(&d.st, b);
    let (_, before_a) = curve(&d.st, a);
    assert!(!legend_b.contains("dB"), "{legend_b}");

    // V to slot 2, Alt+Shift+↑: +3 dB, said in the legend.
    d.key("V");
    d.key("V");
    assert_eq!(d.st.selected_trace, Some(b));
    d.key("Alt+Shift+Up");
    d.until("the offset held by the daemon", |s| {
        s.traces
            .get(&b)
            .is_some_and(|(t, _)| t.meta.edit.offset.0 == 3.0)
    })?;
    let (legend_b, after_b) = curve(&d.st, b);
    assert!(legend_b.contains("+3.0 dB"), "{legend_b}");
    let moved: Vec<f64> = before_b
        .iter()
        .zip(&after_b)
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .map(|(x, y)| y - x)
        .collect();
    assert!(!moved.is_empty());
    assert!(moved.iter().all(|m| (m - 3.0).abs() < 1e-9), "{moved:?}");
    let after_a = curve(&d.st, a).1;
    assert_eq!(after_a.len(), before_a.len());
    assert!(
        after_a
            .iter()
            .zip(&before_a)
            .all(|(x, y)| x == y || (x.is_nan() && y.is_nan())),
        "the other capture stays"
    );

    // A spectrum from the palette; its pane's level axis down to the low levels.
    d.key("Ctrl+K");
    d.send(Msg::Text("new spectrum".into()));
    d.key("Enter");
    d.until(
        "the spectrum dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spectrum),
    )?;
    d.key("Enter");
    d.until("the spectrum measurement", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }))
    })?;
    let sp =
        d.st.measurements()
            .iter()
            .find(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }))
            .map(|m| m.id)
            .ok_or("spectrum")?;
    d.until("spectrum frames", |s| {
        ac2_ui::scenes::frame(s, sp, Stream::Spec).is_some()
    })?;
    d.key("Alt+2");
    // Its levels are per FFT bin: the axis names the bin width, the tooltip what it means.
    let pane = ac2_ui::scenes::spectrum(&d.st, &theme, size, now());
    assert!(
        pane.unit.starts_with("dBFS per ") && pane.unit.ends_with(" Hz bin (tone)"),
        "{}",
        pane.unit
    );
    assert!(
        pane.unit_help.as_deref().is_some_and(|h| h.contains("RTA")),
        "{:?}",
        pane.unit_help
    );
    let level = |s: &AppState| s.view.spectrum.level;
    // The new spectrum's first frame framed the level axis (as Shift+Home).
    let default = ac2_scene::view::ViewState::default().spectrum.level;
    assert_ne!(level(&d.st), default);
    d.key("Ctrl+Home");
    let start = level(&d.st);
    assert_eq!(start, default);
    for _ in 0..4 {
        d.key("Ctrl+Down");
    }
    assert_eq!(level(&d.st), ac2_scene::axis::Range::new(-140.0, -40.0));
    d.key("Ctrl+I");
    let r = level(&d.st);
    assert!(r.span() < start.span() && r.lo < -100.0, "{r:?}");
    let labels: Vec<String> = ac2_ui::scenes::spectrum(&d.st, &theme, size, now())
        .y_axis
        .labels()
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert!(labels.contains(&"\u{2212}120".to_owned()), "{labels:?}");
    // Shift+Home frames what is shown; Ctrl+Home is the default again.
    d.key("Shift+Home");
    let fit = level(&d.st);
    assert!(fit.is_valid() && fit != r, "{fit:?}");
    assert!(
        d.st.toasts
            .iter()
            .any(|t| t.text.starts_with("Spectrum / RTA: level"))
    );
    d.key("Ctrl+Home");
    assert_eq!(level(&d.st), start);

    // Delete slot 1: asked first, then gone; the selection moves to slot 2.
    d.key("Alt+1");
    d.key("V");
    assert_eq!(d.st.selected_trace, Some(a));
    d.key("Delete");
    assert!(
        matches!(&d.st.overlay, Overlay::Delete(p) if p.target == ac2_ui::state::DeleteTarget::Trace(a))
    );
    d.key("Delete");
    assert_eq!(d.st.selected_trace, Some(b));
    d.until("slot 1 deleted", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().all(|t| t.id != a))
    })?;
    assert!(!d.st.traces.contains_key(&a));
    assert_eq!(d.st.trace_rows().len(), 1);

    // Maximised: a measurement picked in the list brings up its pane.
    d.key("W");
    assert!(d.st.layout.maximized);
    d.send(Msg::SelectMeas(sp));
    assert_eq!(d.st.layout.visible(), [PaneKind::Spectrum]);
    assert_eq!(d.st.pane_meas(PaneKind::Spectrum).map(|m| m.id), Some(sp));
    d.send(Msg::SelectMeas(tf));
    assert_eq!(d.st.layout.visible(), [PaneKind::Transfer]);
    assert!(d.st.layout.maximized);
    drop(d);
    drop(daemon);
    Ok(())
}

/// A level axis remembered from the last run is where the pane starts; a spectrum that
/// starts still fits the axis on its first frame.
/// The keys act on what was selected last: from an empty daemon, two transfer
/// measurements; the first selected, A hides its curve from the transfer pane while it keeps
/// running and measuring, A shows it again; Backspace asks before it goes and Enter deletes
/// it, the other measuring on.
#[test]
fn a_hides_and_backspace_deletes_the_selected_measurement() -> R {
    use ac2_ui::state::{DeleteTarget, PaneKind};
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let first = d.st.selected_meas().cloned().ok_or("measurement")?;

    // A second transfer measurement from the palette's dialog, as it comes.
    d.key("Ctrl+K");
    d.send(Msg::Text("new transfer".into()));
    d.key("Enter");
    assert!(matches!(&d.st.overlay, Overlay::Form(f) if f.kind == FormKind::Transfer));
    d.key("Enter");
    d.until("the second measurement, running and selected", |s| {
        s.measurements().len() == 2
            && s.selected_meas()
                .is_some_and(|m| m.id != first.id && m.running)
    })?;
    let second = d.st.selected_meas().cloned().ok_or("second")?;
    // The level typed for the first is kept: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    d.tf_frames(second.id, 240)?;

    let theme = Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 600.0,
    };
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let names = |s: &AppState| -> Vec<String> {
        ac2_ui::scenes::transfer(s, &theme, size, now())
            .legend
            .iter()
            .map(|e| e.name.clone())
            .collect()
    };
    d.until("both curves", |s| names(s).len() == 2)?;

    // Select the first in the list; A hides its curve.
    d.send(Msg::SelectMeas(first.id));
    d.key("A");
    assert_eq!(names(&d.st), std::slice::from_ref(&second.config.name));
    let row =
        d.st.meas_rows()
            .into_iter()
            .find(|r| r.id == first.id)
            .ok_or("row")?;
    assert!(row.hidden && row.text.contains("hidden"), "{}", row.text);
    assert!(
        d.st.pane_caption(PaneKind::Transfer)
            .is_some_and(|c| c.starts_with(&format!("{} hidden", first.config.name)))
    );
    // Hidden, it keeps running and its frames keep coming.
    let seq = |s: &AppState| {
        s.data.as_ref().and_then(|x| {
            x.latest
                .get(&Topic::Data {
                    meas: first.id,
                    stream: Stream::Tf,
                })
                .map(|f| f.frame.stamp.seq)
        })
    };
    let before = seq(&d.st);
    d.until("a newer frame of the hidden measurement", |s| {
        seq(s) > before
    })?;
    assert!(
        d.st.daemon()
            .and_then(|x| x.measurements.iter().find(|m| m.id == first.id))
            .is_some_and(|m| m.running)
    );
    d.key("A");
    assert_eq!(names(&d.st).len(), 2);
    d.key("A");

    // Backspace asks first; Enter deletes it.
    d.key("Backspace");
    assert!(
        matches!(&d.st.overlay, Overlay::Delete(p) if p.target == DeleteTarget::Meas(first.id)),
        "{:?}",
        d.st.overlay
    );
    d.key("Enter");
    assert_eq!(d.st.overlay, Overlay::None);
    d.until("the first measurement gone", |s| {
        s.measurements().len() == 1 && s.meas(first.id).is_none()
    })?;
    assert!(d.st.hidden_meas.is_empty());
    assert!(d.st.meas(second.id).is_some_and(|m| m.running));
    d.until("the other curve alone", |s| {
        names(s) == [second.config.name.clone()]
    })?;
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

#[test]
fn a_remembered_level_axis_still_fits_a_started_spectrum() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    let remembered = ac2_scene::axis::Range::new(-160.0, -150.0);
    let mut prefs = ac2_ui::prefs::UiPrefs::default();
    prefs.levels.spectrum_dbfs = remembered;
    let prefs = ac2_ui::prefs::UiPrefs::from_toml(&prefs.to_toml())?;
    d.st.set_prefs(prefs);
    assert_eq!(d.st.view.spectrum.level, remembered);
    measure_from_empty(&mut d)?;
    assert_eq!(d.st.view.spectrum.level, remembered);
    d.key("Ctrl+K");
    d.send(Msg::Text("new spectrum".into()));
    d.key("Enter");
    d.until(
        "the spectrum dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spectrum),
    )?;
    d.key("Enter");
    d.until("the started spectrum fitted", |s| {
        s.view.spectrum.level != remembered
    })?;
    let fit = d.st.view.spectrum.level;
    assert!(fit.is_valid() && fit.hi > -150.0, "{fit:?}");
    // The fit is what the next start comes back to.
    assert_eq!(d.st.prefs.levels.spectrum_dbfs, fit);
    drop(d);
    drop(daemon);
    Ok(())
}

/// Types `text` into the open prompt in place of what it holds.
fn retype(d: &mut Driver, text: &str) {
    while matches!(&d.st.overlay, Overlay::Prompt(p) if !p.text.is_empty()) {
        d.send(Msg::Backspace);
    }
    d.send(Msg::Text(text.into()));
}

/// The selected stored trace exported from the palette: to a folder (under its own name),
/// then to a typed file in the folder the prompt then starts in; then A − B of an unslotted
/// selected trace and the next shown one.
#[test]
fn the_selected_trace_exports_and_subtracts_from_the_palette() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.key("Ctrl+1");
    d.until("slot 1", |s| s.slots()[0].is_some())?;
    let a = d.st.slots()[0].map(|t| t.id).ok_or("slot 1")?;
    let name = d.st.slots()[0].map(|t| t.edit.name.clone()).ok_or("name")?;
    let dir = tempfile::tempdir()?;
    let export = |d: &mut Driver| {
        d.key("Ctrl+K");
        d.send(Msg::Text("export the selected".into()));
        d.key("Enter");
    };

    // Nothing selected: it says so.
    d.send(Msg::Command(CommandId::SelectLive));
    export(&mut d);
    assert!(!matches!(d.st.overlay, Overlay::Prompt(_)));
    assert!(
        d.st.toasts
            .iter()
            .any(|t| t.severity != Severity::Info && t.text.contains("select"))
    );

    d.key("V");
    assert_eq!(d.st.selected_trace, Some(a));
    export(&mut d);
    let Overlay::Prompt(p) = &d.st.overlay else {
        return Err("the export prompt".into());
    };
    assert_eq!(p.kind, PromptKind::TraceExport(a));
    // It starts in the home directory.
    let home = std::env::home_dir().ok_or("home")?.display().to_string();
    assert_eq!(p.text, format!("{home}{}", std::path::MAIN_SEPARATOR));
    retype(&mut d, &dir.path().display().to_string());
    d.key("Enter");
    let file = dir.path().join(format!("{name}.csv"));
    d.until("the export written", |s| {
        s.toasts
            .iter()
            .any(|t| t.severity == Severity::Info && t.text.contains("exported to"))
    })?;
    let csv = std::fs::read_to_string(&file)?;
    assert!(csv.starts_with("# ac2 trace export"), "{csv:.80}");
    assert!(csv.contains(&format!("# name: {name}\n")), "{csv:.80}");
    assert!(csv.lines().filter(|l| !l.starts_with('#')).count() > 100);

    // The next export starts in that folder; a typed name is the file.
    export(&mut d);
    let Overlay::Prompt(p) = &d.st.overlay else {
        return Err("the export prompt".into());
    };
    let folder = format!("{}{}", dir.path().display(), std::path::MAIN_SEPARATOR);
    assert_eq!(p.text, folder);
    d.send(Msg::Text("front fill.csv".into()));
    d.key("Enter");
    let named = dir.path().join("front fill.csv");
    d.until("the second export written", |s| {
        s.toasts
            .iter()
            .any(|t| t.text.contains("front fill.csv") && t.text.contains("bytes"))
    })?;
    assert_eq!(std::fs::read_to_string(&named)?, csv);

    // A relative path is relative to the last export's folder.
    export(&mut d);
    retype(&mut d, "rel.csv");
    d.key("Enter");
    d.until("the relative export written", |s| {
        s.toasts.iter().any(|t| t.text.contains("rel.csv"))
    })?;
    assert_eq!(std::fs::read_to_string(dir.path().join("rel.csv"))?, csv);

    // A folder that is not there: the error names the path.
    export(&mut d);
    retype(&mut d, &dir.path().join("gone/x.csv").display().to_string());
    d.key("Enter");
    d.until("the write error", |s| {
        s.toasts.iter().any(|t| {
            t.severity != Severity::Info
                && t.text.contains("cannot write")
                && t.text.contains("gone")
        })
    })?;

    // A math channel of an unslotted stored trace: Shift+M starts with the selected trace
    // as A, by name.
    d.key("Ctrl+2");
    d.until("slot 2", |s| s.slots()[1].is_some())?;
    let b = d.st.slots()[1].map(|t| t.id).ok_or("slot 2")?;
    d.send(Msg::SelectTrace(b));
    d.key("Ctrl+K");
    d.send(Msg::Text("move the selected trace to slot".into()));
    d.key("Enter");
    retype(&mut d, "none");
    d.key("Enter");
    d.until("slot 2 freed", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().any(|t| t.id == b && t.edit.slot.is_none()))
    })?;
    let b_name =
        d.st.selected_trace_meta()
            .map(|t| t.edit.name.clone())
            .ok_or("b")?;
    d.key("Shift+M");
    let Overlay::Form(f) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    let name = f.text(ac2_ui::forms::FieldId::Name).to_owned();
    assert!(name.starts_with(&format!("{b_name} ÷ ")), "{name}");
    d.key("Enter");
    let operand = ac2_proto::model::Operand::Trace { trace: b };
    d.until("the math channel of the trace", |s| {
        s.measurements().iter().any(|m| {
            m.config.name == name
                && matches!(&m.config.kind, MeasKind::Math { config } if config.expr.names(operand))
        })
    })?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// A steady 1 kHz tone at −20 dBFS on the simulated rig's output from a client of its own:
/// the operator's own tone source of an in-line electrical calibration. Holds the stimulus
/// lease until dropped.
struct Tone {
    rt: tokio::runtime::Runtime,
    lease: Option<ac2_client::StimulusLease>,
    _client: ac2_client::Client,
}

impl Tone {
    fn start(config: &ClientConfig) -> R<Self> {
        use ac2_client::{Client, OnDrop};
        use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal};
        use ac2_proto::units::{Dbfs, Hz};
        let rt = tokio::runtime::Runtime::new()?;
        let (client, lease) = rt.block_on(async {
            let mut config = config.clone();
            config.name = "ac2-ui e2e tone".into();
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
            Ok::<_, Box<dyn std::error::Error>>((c, lease))
        })?;
        Ok(Self {
            rt,
            lease: Some(lease),
            _client: client,
        })
    }
}

impl Drop for Tone {
    fn drop(&mut self) {
        if let Some(l) = self.lease.take() {
            let _ = self.rt.block_on(l.end());
        }
    }
}

/// The text labels of a scene.
fn scene_texts(s: &ac2_scene::primitives::Scene) -> Vec<String> {
    s.layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.clone()))
        .collect()
}

/// Electrical calibration (no acoustic calibrator) from an empty daemon with the app
/// alone: a session from its dialog, an SPL meter from the palette, the mic named and its
/// curve file (which states 15.0 mV/Pa) imported in the calibrations view, then E on the mic's input:
/// the dialog offers the data sheet's sensitivity, the operator types the voltage the meter
/// shows (15 mV) while a steady 1 kHz tone plays, Enter. The SPL meter then reads dB SPL,
/// says the calibration is electrical with its uncertainty, and so does the Leq caption.
#[test]
fn electrical_calibration_from_the_app() -> R {
    use ac2_proto::model::{CalBasis, CalStatus, LevelScale};
    use ac2_ui::cal_view::{CalLine, lines};
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
    d.until("the SPL meter running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
    })?;

    // The calibrations view, on the mic's input (in 2, "Room mic").
    d.key("Ctrl+K");
    d.send(Msg::Text("calibrations".into()));
    d.key("Enter");
    let focused = |s: &AppState| match s.overlay.cal() {
        Some(v) => s.daemon().and_then(|st| v.focused(st)),
        None => None,
    };
    for _ in 0..8 {
        if focused(&d.st) == Some(CalLine::Input(1)) {
            break;
        }
        d.key("ArrowDown");
    }
    assert_eq!(
        focused(&d.st),
        Some(CalLine::Input(1)),
        "{:?}",
        d.st.daemon().map(lines)
    );
    // N names the mic on it; I imports its curve file.
    d.key("N");
    d.send(Msg::Text("n".into()));
    d.send(Msg::Text("MM1 34804".into()));
    d.key("Enter");
    d.until("the mic named", |s| {
        s.daemon().is_some_and(|x| {
            x.inputs
                .iter()
                .any(|i| i.channel == 1 && i.mic.as_deref() == Some("MM1 34804"))
        })
    })?;
    d.key("I");
    d.send(Msg::Text("i".into()));
    d.send(Msg::Text(mic_curve_file("449350_34804_0Grad.txt")));
    d.key("Enter");
    d.until("the curve with its data-sheet sensitivity", |s| {
        s.daemon().is_some_and(|x| {
            x.mics
                .iter()
                .any(|m| m.name == "MM1 34804" && m.curves[0].stated_sensitivity == Some(15.0))
        })
    })?;

    let tone = Tone::start(&ep)?;
    d.key("E");
    d.send(Msg::Text("e".into()));
    let dialog = |s: &AppState| match s.overlay.cal() {
        Some(v) => v.electrical.clone(),
        None => None,
    };
    let dl = dialog(&d.st).ok_or("the electrical dialog")?;
    assert_eq!(dl.sensitivity, "15.0 mV/Pa");
    assert_eq!(dl.sensitivity_source(), "data sheet (MM1 34804 0°)");
    assert!(dl.safety().contains("pins 2 and 3"), "{}", dl.safety());
    d.send(Msg::Text("15 mV".into()));
    // Enter reads the input; until the tone is there and steady the daemon says so in the
    // dialog, and Enter again retries.
    let end = Instant::now() + DEADLINE;
    loop {
        d.key("Enter");
        d.until("the reply", |s| {
            dialog(s).is_none_or(|x| x.pending.is_none())
        })?;
        match dialog(&d.st) {
            None => break,
            Some(x) => {
                let e = x.error.unwrap_or_default();
                assert!(
                    e.contains("not steady") || e.contains("no tone"),
                    "refused: {e}"
                );
            }
        }
        if Instant::now() > end {
            return Err("the tone never read steady".into());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let notice = match d.st.overlay.cal() {
        Some(v) => v.notice.clone().unwrap_or_default(),
        None => String::new(),
    };
    assert!(notice.contains("stored"), "{notice}");
    d.key("Escape");

    // −20 dBFS out reads −26.02 dBFS in; 15 mV there with 15 mV/Pa is 1 Pa: 94.0 dB SPL.
    let now = || ac2_ui::scenes::Now {
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
    let want = "electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB";
    d.until("the SPL meter in dB SPL, electrically calibrated", |s| {
        ac2_ui::scenes::spl(s, &Keymap::default(), &Theme::dark(), size, now()).is_some_and(|x| {
            let t = scene_texts(&x.scene);
            t.iter().any(|l| l.contains(want))
                && t.iter().any(|l| l == "94.0")
                && t.iter().any(|l| l.contains("dB SPL"))
        })
    })?;
    d.until("the Leq caption naming the electrical calibration", |s| {
        ac2_ui::scenes::leq(s, &Theme::dark(), size, now())
            .is_some_and(|x| scene_texts(&x.scene).iter().any(|l| l.contains(want)))
    })?;
    // The Leq frames are in dB SPL and say what the calibration rests on: limits are
    // judged on them.
    let m =
        d.st.measurements()
            .into_iter()
            .find(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
            .ok_or("meter")?;
    let topic = Topic::Data {
        meas: m.id,
        stream: Stream::Leq,
    };
    d.until("the leq frame calibrated", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Leq(l) => {
                    l.meta.scale == LevelScale::DbSpl
                        && matches!(
                            l.meta.cal,
                            CalStatus::Verified {
                                basis: CalBasis::Electrical { .. },
                                ..
                            }
                        )
                }
                _ => false,
            })
        })
    })?;
    drop(tone);
    drop(d);
    drop(daemon);
    Ok(())
}

/// Acoustic calibration (a calibrator on the mic) from an empty daemon with the app: the
/// calibrations view, C on the mic's input opens the dialog with the input still unnamed,
/// the mic typed, the calibrator's 94 dB at 1 kHz as offered; Enter reads the input (the
/// simulated rig's tone stands in for the calibrator), retried in the dialog until steady,
/// stores the calibration, names the mic on the input and closes with what to do next; the
/// SPL meter then reads 94.0 dB SPL with the calibrator named.
#[test]
fn acoustic_calibration_from_the_app() -> R {
    use ac2_proto::model::CalMethod;
    use ac2_ui::cal_view::{CalLine, lines};
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
    d.until("the SPL meter running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
    })?;

    d.key("Ctrl+K");
    d.send(Msg::Text("calibrations".into()));
    d.key("Enter");
    let focused = |s: &AppState| match s.overlay.cal() {
        Some(v) => s.daemon().and_then(|st| v.focused(st)),
        None => None,
    };
    for _ in 0..8 {
        if focused(&d.st) == Some(CalLine::Input(1)) {
            break;
        }
        d.key("ArrowDown");
    }
    assert_eq!(
        focused(&d.st),
        Some(CalLine::Input(1)),
        "{:?}",
        d.st.daemon().map(lines)
    );
    let tone = Tone::start(&ep)?;
    d.key("C");
    d.send(Msg::Text("c".into()));
    let dialog = |s: &AppState| match s.overlay.cal() {
        Some(v) => v.acoustic.clone(),
        None => None,
    };
    let dl = dialog(&d.st).ok_or("the acoustic dialog")?;
    assert_eq!(dl.title(), "Acoustic calibration · in 2");
    assert_eq!(
        (dl.mic.as_str(), dl.level.as_str(), dl.freq.as_str()),
        ("", "94 dB", "1 kHz")
    );
    assert!(dl.instructions().contains("calibrator"));
    // The focus starts on the unnamed mic: typed there.
    d.send(Msg::Text("MM1 34804".into()));
    let end = Instant::now() + DEADLINE;
    loop {
        d.key("Enter");
        d.until("the reply", |s| {
            dialog(s).is_none_or(|x| x.pending.is_none())
        })?;
        match dialog(&d.st) {
            None => break,
            Some(x) => {
                let e = x.error.unwrap_or_default();
                assert!(
                    e.contains("not steady") || e.contains("no calibrator"),
                    "refused: {e}"
                );
            }
        }
        if Instant::now() > end {
            return Err("the calibrator never read steady".into());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let notice = match d.st.overlay.cal() {
        Some(v) => v.notice.clone().unwrap_or_default(),
        None => String::new(),
    };
    assert!(
        notice.contains("acoustic calibration of input 2 (MM1 34804): stored")
            && notice.contains("take the calibrator off"),
        "{notice}"
    );
    d.until("the calibration stored, the mic named on the input", |s| {
        s.daemon().is_some_and(|x| {
            x.inputs
                .iter()
                .any(|i| i.channel == 1 && i.mic.as_deref() == Some("MM1 34804"))
                && x.calibrations.iter().any(|e| {
                    e.key.channel == 1
                        && e.key.mic == "MM1 34804"
                        && matches!(e.spl.method, CalMethod::Acoustic { .. })
                })
        })
    })?;
    d.key("Escape");
    let now = || ac2_ui::scenes::Now {
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
    d.until("the SPL meter at the calibrator's 94.0 dB SPL", |s| {
        ac2_ui::scenes::spl(s, &Keymap::default(), &Theme::dark(), size, now()).is_some_and(|x| {
            let t = scene_texts(&x.scene);
            t.iter().any(|l| l.contains("cal 94 dB"))
                && t.iter().any(|l| l == "94.0")
                && t.iter().any(|l| l.contains("dB SPL"))
        })
    })?;
    drop(tone);
    drop(d);
    drop(daemon);
    Ok(())
}

/// The SPL meter's weightings from the keys, from an empty daemon: an SPL meter from the
/// palette, F (Fast → Slow) and Z (A → C) in its pane; the meter reads `LCS` on the next
/// frames, and its number takes a new reading no more often than once a second (Slow's
/// display period), however often frames arrive.
#[test]
fn spl_weightings_from_the_keys_and_a_readable_number() -> R {
    use ac2_proto::model::{TimeWeighting, Weighting};
    use ac2_ui::state::PaneKind;
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
    d.until("the SPL meter running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
    })?;
    d.key("Alt+4");
    assert_eq!(d.st.layout.focus, PaneKind::Spl);
    let config = |s: &AppState| {
        s.measurements()
            .into_iter()
            .find_map(|m| match &m.config.kind {
                MeasKind::Spl { config } => Some((m.id, config.clone())),
                _ => None,
            })
    };
    d.key("F");
    d.until("Slow", |s| {
        config(s).is_some_and(|(_, c)| c.time_weighting == TimeWeighting::Slow)
    })?;
    // Z steps A → C → Z → A from wherever the dialog's meter starts.
    for _ in 0..3 {
        let w = config(&d.st).map(|(_, c)| c.weighting);
        if w == Some(Weighting::C) {
            break;
        }
        d.key("Z");
        d.until("the next weighting", |s| {
            config(s).map(|(_, c)| c.weighting) != w
        })?;
    }
    d.until("C and Slow", |s| {
        config(s).is_some_and(|(_, c)| {
            c.weighting == Weighting::C && c.time_weighting == TimeWeighting::Slow
        })
    })?;
    assert_eq!(
        d.st.view.spl.mode,
        ac2_scene::view::SplMode::MeterLeq,
        "the view stays"
    );
    let (id, cfg) = config(&d.st).ok_or("meter")?;
    assert_eq!(cfg.leq, ac2_proto::model::LeqConfig::default_windows());
    let size = ac2_scene::primitives::Viewport {
        width: 1280.0,
        height: 720.0,
    };
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    // The statistics under one heading: since when the meter runs, and its reset key.
    d.until("the meter labelled LCS, its statistics headed once", |s| {
        ac2_ui::scenes::spl(s, &Keymap::default(), &Theme::dark(), size, now()).is_some_and(|x| {
            let t = scene_texts(&x.scene);
            t.iter().any(|l| l.starts_with("LCS · "))
                && t.iter()
                    .filter(|l| l.starts_with("meter since") && l.ends_with("· R resets"))
                    .count()
                    == 1
                && t.iter().any(|l| l.starts_with("LCeq "))
        })
    })?;
    // Readings for 3.5 s: each new one at least a display period after the last.
    let mut at = Vec::new();
    let end = Instant::now() + Duration::from_millis(3500);
    let mut frames = std::collections::BTreeSet::new();
    while Instant::now() < end {
        d.pump();
        if let Some(h) = d.st.spl_hold.get(&id)
            && h.frame.meta.time_weighting == TimeWeighting::Slow
            && at.last() != Some(&h.at_ns)
        {
            at.push(h.at_ns);
        }
        if let Some(f) = d.st.data.as_ref().and_then(|x| {
            x.latest.get(&Topic::Data {
                meas: id,
                stream: Stream::Spl,
            })
        }) {
            frames.insert(f.frame.stamp.seq);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        frames.len() > 2 * at.len(),
        "{} frames, {} readings",
        frames.len(),
        at.len()
    );
    assert!(at.len() >= 3, "{at:?}");
    for w in at.windows(2) {
        assert!(
            w[1] - w[0] >= 1_000_000_000,
            "readings {} ms apart",
            (w[1] - w[0]) / 1_000_000
        );
    }
    drop(d);
    drop(daemon);
    Ok(())
}

/// A new SPL meter's pane, from an empty daemon: the meter's number over its Leq windows by
/// default (one caption for both), in the split layout and, W twice, full screen; G steps
/// meter → Leq windows → meter + Leq.
#[test]
fn spl_pane_shows_meter_and_leq_from_an_empty_daemon() -> R {
    use ac2_scene::meter_leq::MeterForm;
    use ac2_scene::primitives::Viewport;
    use ac2_scene::view::SplMode;
    use ac2_ui::state::PaneKind;
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
    d.until("the SPL meter running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
    })?;
    d.key("Alt+4");
    assert_eq!(d.st.layout.focus, PaneKind::Spl);
    assert_eq!(d.st.view.spl.mode, SplMode::MeterLeq, "the default view");
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let keys = Keymap::default();
    let both = |s: &AppState, width: f32, height: f32| {
        let size = Viewport { width, height };
        ac2_ui::scenes::meter_leq(s, &keys, &Theme::dark(), size, now()).filter(|x| {
            let t = scene_texts(&x.leq.scene);
            x.meter.form == MeterForm::Block
                && x.leq.columns.as_ref().is_some_and(|k| k.columns.len() == 5)
                && t.iter().filter(|l| l.starts_with("LAF · ")).count() == 1
                // The caption names the meter once; on the stage, without the history,
                // there is no caption.
                && t.iter().filter(|l| l.starts_with("SPL 1 · ")).count()
                    == usize::from(!s.stage_view() || s.view.spl.layout.history)
                && !t.iter().any(|l| l.starts_with("meter since"))
        })
    };
    // A pane of the split layout: the number over the windows.
    d.until("the meter over its windows", |s| {
        both(s, 640.0, 420.0).is_some()
    })?;
    // W twice: full screen, the same two parts.
    d.key("W");
    d.key("W");
    assert!(d.st.stage_view(), "full screen");
    let stage = both(&d.st, 1920.0, 1080.0).ok_or("both parts full screen")?;
    let under = 1080.0 - stage.leq.caption.bottom();
    assert!(
        stage.meter.region.h > 0.25 * under && stage.meter.region.h < 0.4 * under,
        "{:?} of {under}",
        stage.meter.region
    );
    // G: the meter alone, the windows alone, both again.
    d.key("G");
    assert_eq!(d.st.view.spl.mode, SplMode::Meter);
    d.key("G");
    assert_eq!(d.st.view.spl.mode, SplMode::Leq);
    d.key("G");
    assert_eq!(d.st.view.spl.mode, SplMode::MeterLeq);
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

/// The streams of `meas` the app holds frames of now.
fn streams_of(s: &AppState, meas: MeasId) -> Vec<Stream> {
    s.data
        .as_ref()
        .map(|d| {
            d.latest
                .frames
                .values()
                .filter_map(|f| match f.topic {
                    Topic::Data { meas: m, stream } if m == meas => Some(stream),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// From an empty daemon: the app receives the streams its panes draw and no others. The IR
/// arrives while its pane is shown and stops when it is hidden (the daemon derives it only
/// for subscribers); a maximised spectrum pane drops the transfer function; the
/// measurement's input levels, which no pane draws, never arrive.
#[test]
fn subscriptions_follow_the_panes_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?.id;
    // The level typed there stays: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    let has = |s: &AppState, st: Stream| streams_of(s, m).contains(&st);
    d.until("the IR, its pane shown", |s| {
        has(s, Stream::Tf) && has(s, Stream::Ir)
    })?;
    assert!(!has(&d.st, Stream::Levels));

    d.key("Shift+I");
    assert!(!d.st.layout.is_shown(ac2_ui::state::PaneKind::Ir));
    d.until("no IR, its pane hidden", |s| {
        has(s, Stream::Tf) && !has(s, Stream::Ir)
    })?;
    // Still none a while later: nothing subscribes to it.
    let end = Instant::now() + Duration::from_millis(500);
    while Instant::now() < end {
        d.pump();
        assert!(!has(&d.st, Stream::Ir));
        std::thread::sleep(Duration::from_millis(20));
    }

    // The spectrum pane alone: no transfer function either.
    d.key("Alt+2");
    d.key("W");
    assert!(d.st.layout.maximized);
    d.until("nothing of the transfer measurement", |s| {
        streams_of(s, m).is_empty()
    })?;
    // Back to the panes (W cycles maximised, full screen, back), and the IR shown again:
    // both arrive again.
    d.key("W");
    d.key("W");
    assert!(!d.st.layout.maximized);
    d.key("Alt+1");
    d.key("Shift+I");
    d.until("the TF and the IR again", |s| {
        has(s, Stream::Tf) && has(s, Stream::Ir)
    })?;
    assert!(!has(&d.st, Stream::Levels));
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// The spectrograph from an empty daemon: a spectrum from the palette, G shows the
/// spectrograph under it, the rig's noise fills it from the frames, the cursor reads a level
/// back, Shift+G changes the history length, a stopped measurement says so and G hides it
/// and lets the history go.
#[test]
fn spectrograph_from_an_empty_daemon() -> R {
    use ac2_scene::primitives::HeatmapAxes;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.key("Ctrl+K");
    d.send(Msg::Text("new spectrum".into()));
    d.key("Enter");
    d.until(
        "the spectrum dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spectrum),
    )?;
    d.key("Enter");
    d.until("the spectrum measurement, running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }) && m.running)
    })?;
    let (sp, name) =
        d.st.measurements()
            .iter()
            .find(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }))
            .map(|m| (m.id, m.config.name.clone()))
            .ok_or("spectrum")?;
    d.key("Alt+2");
    assert!(
        hint_line(&d.st)
            .iter()
            .any(|h| h == "G spectrum/both/spectrograph"),
        "{:?}",
        hint_line(&d.st)
    );
    d.key("G");
    assert_eq!(d.st.view.spectrum.mode, SpectrumMode::Split);
    // The level typed for the transfer measurement is still set: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|d| d.generator.firing))?;
    let filled = |s: &AppState, n: usize| {
        s.spectrographs
            .get(&sp)
            .is_some_and(|h| h.ring().iter().flatten().count() >= n)
    };
    // A second of history (30 slots a second at 30 s).
    d.until("a second of spectrograph", |s| filled(s, 30))?;

    let theme = Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 1000.0,
        height: 600.0,
    };
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let s = ac2_ui::scenes::spectrograph(&d.st, &theme, size, now());
    assert_eq!(s.caption, format!("{name} · last 30 s · dBFS"));
    assert_eq!(s.message, None);
    let history = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.heatmaps)
        .find(|h| h.axes == HeatmapAxes::TimeUp)
        .ok_or("the history in the scene")?;
    assert!(history.data.iter().flatten().count() >= 30);
    // Its colours span the pane's level axis, the one the level keys move.
    let r = d.st.view.spectrum.level;
    assert_eq!(history.range, [r.lo as f32, r.hi as f32]);
    // The rig's noise at 1 kHz, just now.
    d.send(Msg::SpectrographCursor {
        hz: 1000.0,
        before_s: 0.1,
    });
    let s = ac2_ui::scenes::spectrograph(&d.st, &theme, size, now());
    let text = s.cursor.ok_or("cursor")?.text;
    assert!(
        text.starts_with("1.00 kHz · 0.1 s ago · ") && text.ends_with(" dBFS"),
        "{text}"
    );
    // C turns the cursor off, time and all.
    d.key("C");
    assert_eq!(d.st.view.spectrum.spectrograph.cursor_s, None);

    // Shift+G: a minute of history, started afresh.
    d.key("Shift+G");
    assert_eq!(d.st.view.spectrum.spectrograph.span_s, 60);
    assert!(!filled(&d.st, 1));
    d.until("frames in the minute", |s| filled(s, 10))?;
    let s = ac2_ui::scenes::spectrograph(&d.st, &theme, size, now());
    assert!(s.caption.contains("last 60 s"), "{}", s.caption);

    // Stopped: the picture stays and says so.
    d.stop()?;
    d.key("S");
    d.until("the spectrum stopped", |s| {
        s.measurements().iter().any(|m| m.id == sp && !m.running)
    })?;
    let s = ac2_ui::scenes::spectrograph(&d.st, &theme, size, now());
    assert!(s.caption.ends_with(" · stopped"), "{}", s.caption);
    assert!(filled(&d.st, 10));
    // G: the spectrograph alone, its history kept; W makes it the only pane, full size.
    d.key("G");
    assert_eq!(d.st.view.spectrum.mode, SpectrumMode::Spectrograph);
    assert!(filled(&d.st, 10));
    d.key("W");
    assert_eq!(d.st.layout.visible(), [ac2_ui::state::PaneKind::Spectrum]);
    let alone = ac2_ui::scenes::spectrograph(&d.st, &theme, size, now());
    assert!(alone.spectrum.is_none());
    assert!(
        alone.plot.h > s.plot.h * 1.5,
        "{:?} vs {:?}",
        alone.plot,
        s.plot
    );
    assert!(
        alone
            .caption
            .starts_with(&format!("{name} · last 60 s · dBFS · stopped · ")),
        "{}",
        alone.caption
    );
    // The level keys still move the colours.
    let before = d.st.view.spectrum.level;
    d.key("Ctrl+I");
    assert_ne!(d.st.view.spectrum.level, before);
    d.key("W");
    // G again hides it and nothing is kept.
    d.key("G");
    assert_eq!(d.st.view.spectrum.mode, SpectrumMode::Spectrum);
    assert!(d.st.spectrographs.is_empty());
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon: measure, then record from the palette while the noise plays — the
/// top bar shows REC with the audio's length and size, then what was recorded — and replay
/// the recording from the palette: the session plays the file, the bar says so, and the
/// transfer function shows the rig's path again from the recorded inputs.
#[test]
fn record_and_replay_from_the_app() -> R {
    let dir = tempfile::tempdir()?;
    let listen = ac2d::Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let audio = ac2d::backend(ac2d::BackendChoice::Fake)?;
    let mut config = ac2d::DaemonConfig::new(audio, listen, -10.0);
    config.session_dir = dir.path().join("sessions");
    config.recording_dir = Some(dir.path().join("recordings"));
    let handle = ac2d::Daemon::start(config)?;
    let ep = Endpoints {
        ctrl: handle.ctrl_endpoint().to_owned(),
        data: handle.data_endpoint().to_owned(),
    };
    let mut d = Driver::connect(ClientConfig::new(ep, NAME), "local daemon")?;
    measure_from_empty(&mut d)?;
    assert_eq!(d.st.recording_label(), None, "nothing recorded yet");
    let meas = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;

    // The level typed for the first run stands: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    d.key("Ctrl+K");
    d.send(Msg::Text("record".into()));
    d.key("Enter");
    d.until("two seconds recorded", |s| {
        s.daemon()
            .and_then(|x| x.recording.as_ref())
            .is_some_and(|r| r.active() && r.frames >= 2 * u64::from(r.sample_rate_hz))
    })?;
    let rec = d.st.recording_label().ok_or("no indicator")?;
    assert!(rec.text.starts_with("REC 0:0"), "{}", rec.text);
    assert!(
        rec.text.contains(" kB") || rec.text.contains(" MB"),
        "{}",
        rec.text
    );
    assert_eq!(rec.tone, ac2_scene::recording::RecordingTone::Recording);
    assert!(rec.detail.contains("Room mic"), "{}", rec.detail);

    d.key("Ctrl+K");
    d.send(Msg::Text("record".into()));
    d.key("Enter");
    d.until("the recording ended", |s| {
        s.daemon()
            .and_then(|x| x.recording.as_ref())
            .is_some_and(|r| !r.active())
    })?;
    d.stop()?;
    let run =
        d.st.daemon()
            .and_then(|x| x.recording.clone())
            .ok_or("recording")?;
    let label = d.st.recording_label().ok_or("no indicator")?;
    assert!(
        label
            .text
            .starts_with(&format!("recorded {} · 0:0", run.name)),
        "{}",
        label.text
    );

    d.key("Ctrl+K");
    d.send(Msg::Text("replay".into()));
    d.key("Enter");
    d.send(Msg::Text(run.name.clone()));
    d.key("Enter");
    d.until("the replay session", |s| {
        s.open_session().is_some_and(|o| o.replay.is_some())
    })?;
    let replay = d.st.replay_label().ok_or("no replay label")?;
    assert!(
        replay.starts_with(&format!("REPLAY {} · 0:0", run.name)),
        "{replay}"
    );
    let epoch = d.st.daemon().map(|x| x.session.epoch).ok_or("state")?;
    let topic = Topic::Data {
        meas,
        stream: Stream::Tf,
    };
    d.until("the −6 dB path at 1 kHz from the recording", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| {
                f.frame.stamp.session_epoch == epoch
                    && match &f.frame.data {
                        FrameData::Tf(tf) => tf
                            .mag
                            .get(240)
                            .is_some_and(|m| m.is_finite() && (m - (-6.02)).abs() < 0.5),
                        _ => false,
                    }
            })
        })
    })?;
    drop(d);
    handle.shutdown();
    Ok(())
}

/// The legend text of live measurement `id` on the transfer pane.
fn tf_legend(s: &AppState, id: MeasId) -> String {
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 600.0,
    };
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    ac2_ui::scenes::transfer(s, &Theme::dark(), size, now)
        .legend
        .iter()
        .find(|e| e.key == ac2_scene::trace::TraceKey::Live(id))
        .map(|e| e.text.clone())
        .unwrap_or_default()
}

/// The math channel of the mirrored state, if there is one running.
fn running_math(s: &AppState) -> Option<(MeasId, String)> {
    s.measurements()
        .iter()
        .find(|m| matches!(m.config.kind, MeasKind::Math { .. }) && m.running)
        .map(|m| (m.id, m.config.name.clone()))
}

/// From an empty daemon: two transfer measurements (two positions of the room mic); the
/// math channel dialog by name (Shift+M) makes their average, drawn as a transfer curve whose
/// legend counts its positions; Ctrl+1 freezes it into a stored trace naming the
/// expression; edited into A ÷ B it reads 0 dB (one path over itself); a position stopped
/// is named in a banner and the ratio says it has no result.
#[test]
fn math_channels_from_an_empty_daemon() -> R {
    use ac2_proto::Command;
    use ac2_proto::frame::OperandStatus;
    use ac2_proto::model::{MathExpr, MathOp, TraceSource};
    use ac2_ui::conn::Request;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let first = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;

    // One transfer function: nothing to combine it with yet, and the dialog says so.
    d.key("Shift+M");
    assert!(!matches!(d.st.overlay, Overlay::Form(_)));
    assert!(
        d.st.toasts
            .iter()
            .any(|t| t.text.contains("two transfer functions, spectra or RTAs")),
        "{:?}",
        d.st.toasts
    );

    // A second position, from the transfer dialog.
    d.send(Msg::Command(CommandId::NewTransfer));
    d.key("Enter");
    d.until("two transfer measurements", |s| {
        s.measurements()
            .iter()
            .filter(|m| matches!(m.config.kind, MeasKind::Transfer { .. }) && m.running)
            .count()
            == 2
    })?;
    let second =
        d.st.measurements()
            .iter()
            .map(|m| m.id)
            .find(|id| *id != first)
            .ok_or("second")?;

    // A, the operator and B by name; the operator steps on to the average, which takes both
    // positions; Enter makes and starts it.
    d.key("Shift+M");
    let Overlay::Form(f) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    assert_eq!(f.kind, FormKind::Math);
    let rows: Vec<(String, String)> = f
        .fields
        .iter()
        .map(|x| (x.label.clone(), x.display()))
        .collect();
    assert_eq!(rows[0].0, "A");
    assert!(rows[0].1.ends_with("(live)"), "{rows:?}");
    assert_eq!(rows[1], ("Operator".into(), "÷  A relative to B".into()));
    assert_eq!(rows[2].0, "B");
    d.key("Down");
    for _ in 0..4 {
        d.key("Right");
    }
    let Overlay::Form(f) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    let rows: Vec<(String, String)> = f
        .fields
        .iter()
        .map(|x| (x.label.clone(), x.display()))
        .collect();
    assert_eq!(rows[0], ("Operator".into(), "average of several".into()));
    assert_eq!(rows[1].1, "in the average");
    assert_eq!(rows[2].1, "in the average");
    assert!(
        rows.iter()
            .any(|r| r == &("Name".into(), "Average of 2".into())),
        "{rows:?}"
    );
    d.key("Enter");
    d.until("the math channel, running", |s| running_math(s).is_some())?;
    let (avg, name) = running_math(&d.st).ok_or("math channel")?;
    assert_eq!(name, "Average of 2");

    // Two positions of the same −6 dB path average to −6 dB (the level is still typed).
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|d| d.generator.firing))?;
    d.tf_frames(avg, 240)?;
    assert!(
        tf_legend(&d.st, avg).starts_with("Average of 2 · 2 positions · power avg"),
        "{}",
        tf_legend(&d.st, avg)
    );

    // Ctrl+1 freezes it: a stored trace in slot 1 naming the expression and its operands.
    d.send(Msg::SelectMeas(avg));
    d.key("Ctrl+1");
    d.until("the capture in slot 1", |s| {
        s.daemon().is_some_and(|x| {
            x.traces.iter().any(|t| {
                t.edit.slot == Some(1)
                    && matches!(
                        &t.source,
                        TraceSource::Math { expr: MathExpr::Average { .. }, operands, .. }
                            if operands.len() == 2
                    )
            })
        })
    })?;

    // Edited in the same dialog into A ÷ B: one path over itself is 0 dB.
    d.key("Ctrl+K");
    d.send(Msg::Text("edit the selected math".into()));
    d.key("Enter");
    assert!(
        matches!(&d.st.overlay, Overlay::Form(f) if f.kind == FormKind::MathEdit),
        "{:?}",
        d.st.overlay
    );
    for _ in 0..4 {
        d.key("Left");
    }
    d.key("Enter");
    let topic = Topic::Data {
        meas: avg,
        stream: Stream::Tf,
    };
    d.until("the ratio at 0 dB", |s| {
        let ratio = s.meas(avg).is_some_and(|m| {
            matches!(
                &m.config.kind,
                MeasKind::Math { config } if matches!(
                    config.expr,
                    MathExpr::Binary { op: MathOp::Divide, .. }
                )
            )
        });
        ratio
            && s.data.as_ref().is_some_and(|x| {
                x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                    FrameData::Tf(tf) => tf
                        .mag
                        .get(240)
                        .is_some_and(|m| m.is_finite() && m.abs() < 0.5),
                    _ => false,
                })
            })
    })?;
    assert!(
        tf_legend(&d.st, avg).contains(" ÷ "),
        "{}",
        tf_legend(&d.st, avg)
    );

    // One operand stopped: no ratio, and the banner says which and why.
    d.conn.send(Request::Call {
        cmd: Command::MeasStop { meas: second },
        what: "stopped".into(),
    });
    d.until("the ratio without its second operand", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Tf(tf) => tf.meta.math.as_ref().is_some_and(|a| {
                    a.operands
                        .iter()
                        .any(|m| m.status == OperandStatus::Stopped)
                }),
                _ => false,
            })
        })
    })?;
    let second_name =
        d.st.measurements()
            .iter()
            .find(|m| m.id == second)
            .map(|m| m.config.name.clone())
            .unwrap_or_default();
    d.st.pane_meas
        .insert(ac2_ui::state::PaneKind::Transfer, avg);
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 600.0,
    };
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let scene = ac2_ui::scenes::transfer(&d.st, &Theme::dark(), size, now);
    let banners: Vec<(String, Option<String>)> = scene
        .banners
        .iter()
        .map(|b| (b.text.clone(), b.detail.clone()))
        .collect();
    assert!(
        banners.contains(&(
            "NO RESULT · 1 OF 2 OPERANDS".into(),
            Some(format!("Average of 2: left out {second_name}: stopped"))
        )),
        "{banners:?}"
    );
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// Spectrum math lands on the spectrum pane, never the transfer pane: two spectra of the
/// room mic, A − B by the dialog with the spectrum pane focused, both operands in its frames.
#[test]
fn spectrum_math_lands_on_the_spectrum_pane() -> R {
    use ac2_proto::model::MathDomain;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    for n in 1..=2 {
        d.send(Msg::Command(CommandId::NewSpectrum));
        d.key("Enter");
        d.until("the spectrum, running", |s| {
            s.measurements()
                .iter()
                .filter(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }) && m.running)
                .count()
                == n
        })?;
    }
    d.key("Alt+2");
    d.key("Shift+M");
    let Overlay::Form(f) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    let op = f
        .fields
        .iter()
        .find(|x| x.label == "Operator")
        .map(|x| x.display())
        .unwrap_or_default();
    assert_eq!(op, "−  level difference (dB)");
    d.key("Enter");
    d.until("the spectrum math, running", |s| running_math(s).is_some())?;
    let (id, name) = running_math(&d.st).ok_or("math channel")?;
    assert_eq!(name, "Spectrum 2 − Spectrum 1");
    assert!(d.st.meas(id).is_some_and(|m| matches!(
        &m.config.kind,
        MeasKind::Math { config } if config.domain == MathDomain::Spectrum
    )));
    // The level is still typed: Space arms, Enter fires.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|d| d.generator.firing))?;
    let topic = Topic::Data {
        meas: id,
        stream: Stream::Spec,
    };
    // Both spectra went in, on the spectrum stream (the levels are checked against
    // analytic values in the daemon's tests: here each operand is its own unaveraged FFT).
    d.until("the difference on the spectrum stream", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Spec(sp) => {
                    sp.meta.math.as_ref().is_some_and(|m| m.included() == 2)
                        && sp.level.iter().filter(|v| v.is_finite()).count() > 100
                }
                _ => false,
            })
        })
    })?;
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 600.0,
    };
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let live = ac2_scene::trace::TraceKey::Live(id);
    d.st.pane_meas.insert(ac2_ui::state::PaneKind::Spectrum, id);
    let spec = ac2_ui::scenes::spectrum(&d.st, &Theme::dark(), size, now());
    assert!(
        spec.legend.iter().any(|e| e.key == live),
        "not on the spectrum pane"
    );
    // Shown by the pane, its caption is the expression and what it means.
    assert!(
        spec.caption
            .contains("Spectrum 2 − Spectrum 1 · level difference"),
        "{}",
        spec.caption
    );
    let tf = ac2_ui::scenes::transfer(&d.st, &Theme::dark(), size, now());
    assert!(
        tf.legend.iter().all(|e| e.key != live),
        "spectrum math on the transfer pane"
    );
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

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

/// The measurement tree from an empty daemon, keyboard only: a transfer measurement, two
/// captures filed under it, a math channel made on it; a sweep measurement that plays
/// nothing until Space and Enter, two runs under it; the transfer measurement deleted with
/// its traces kept, which then list under Imported.
#[test]
fn the_measurement_tree_from_an_empty_daemon() -> R {
    use ac2_proto::model::{MathOp, Operand, TraceOwner};
    use ac2_scene::meas_list::TreeKey;
    use ac2_ui::forms::FieldId;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let tf = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;
    let under_tf = TraceOwner::Meas { meas: tf };

    // Two captures, filed under the measurement they came from.
    d.key("Ctrl+1");
    d.until("slot 1", |s| s.slots()[0].is_some())?;
    d.key("Ctrl+2");
    d.until("slot 2", |s| s.slots()[1].is_some())?;
    let (a, b) = (
        d.st.slots()[0].map(|t| t.id).ok_or("slot 1")?,
        d.st.slots()[1].map(|t| t.id).ok_or("slot 2")?,
    );
    for id in [a, b] {
        assert_eq!(d.st.trace_meta(id)?.edit.owner, under_tf);
    }

    // A math channel made with the measurement selected lives under it.
    d.send(Msg::SelectMeas(tf));
    d.key("Shift+M");
    d.send(Msg::Text("M".into()));
    d.until(
        "the math dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.math.is_some()),
    )?;
    if let Overlay::Form(f) = &mut d.st.overlay {
        assert!(f.pick_operand(FieldId::OperandA, Operand::Trace { trace: a }));
        assert!(f.pick_operand(FieldId::OperandB, Operand::Trace { trace: b }));
    }
    d.key("Enter");
    let math = |s: &AppState| {
        s.measurements()
            .into_iter()
            .find(
                |m| matches!(&m.config.kind, MeasKind::Math { config } if config.owner == under_tf),
            )
            .map(|m| m.id)
    };
    d.until("the math channel under the measurement", |s| {
        math(s).is_some()
    })?;
    let math_id = math(&d.st).ok_or("math")?;
    if let Some(MeasKind::Math { config }) = d.st.meas(math_id).map(|m| &m.config.kind) {
        assert!(matches!(
            config.expr,
            ac2_proto::model::MathExpr::Binary {
                op: MathOp::Divide,
                ..
            }
        ));
    }
    let keys: Vec<TreeKey> = d.st.tree_rows().iter().map(|r| r.key).collect();
    assert_eq!(
        &keys[..5],
        &[
            TreeKey::Meas(tf),
            TreeKey::Live(tf),
            TreeKey::Trace(a),
            TreeKey::Trace(b),
            TreeKey::Math(math_id)
        ]
    );

    // A sweep measurement: made by the dialog, it waits; nothing plays.
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
    d.until("the sweep measurement listed", |s| {
        sweep_count(s) > sweeps_before && s.sweep_meas().is_some_and(|m| Some(m.id) == s.selected)
    })?;
    let sweep = d.st.sweep_meas().map(|m| m.id).ok_or("sweep measurement")?;
    std::thread::sleep(Duration::from_millis(300));
    d.pump();
    let st = d.st.daemon().ok_or("state")?;
    assert!(!st.generator.armed && !st.generator.firing, "nothing armed");
    assert!(st.sweep.is_none(), "nothing played");
    // Space arms its run, Enter plays it; twice.
    for n in 1..=2 {
        d.key("Space");
        d.until("armed with the sweep", |s| {
            s.stimulus.phase == StimPhase::Armed
        })?;
        d.key("Enter");
        d.until("the run stored, the stimulus off", |s| {
            runs_of(s, sweep).len() == n
                && s.sweep.run.is_none()
                && s.stimulus.phase == StimPhase::Idle
                && s.daemon().is_some_and(|x| x.generator.owner.is_none())
        })?;
    }
    assert_eq!(runs_of(&d.st, sweep), ["Run 1", "Run 2"]);

    // Delete the transfer measurement, keeping what it owns: under Imported now.
    d.send(Msg::SelectMeas(tf));
    d.key("Delete");
    let Overlay::Choose(c) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    assert_eq!(
        c.lines[0],
        "transfer function · running · it has 2 traces and 1 math channel."
    );
    assert_eq!(c.index, ac2_scene::meas_list::KEEP);
    d.key("Enter");
    d.until("the measurement gone, its traces under Imported", |s| {
        s.meas(tf).is_none()
            && [a, b].iter().all(|t| {
                s.trace_meta(*t)
                    .is_ok_and(|m| m.edit.owner == TraceOwner::Imported)
            })
            && matches!(s.meas(math_id).map(|m| &m.config.kind),
                Some(MeasKind::Math { config }) if config.owner == TraceOwner::Imported)
    })?;
    let names: Vec<String> = d.st.tree_rows().iter().map(|r| r.name.clone()).collect();
    let at = names
        .iter()
        .position(|n| n == "Imported")
        .ok_or("Imported")?;
    assert_eq!(names.len(), at + 4, "{names:?}");
    drop(d);
    drop(daemon);
    Ok(())
}

/// The names of sweep measurement `meas`'s runs, oldest first.
fn runs_of(s: &AppState, meas: ac2_proto::units::MeasId) -> Vec<String> {
    let owner = ac2_proto::model::TraceOwner::Meas { meas };
    s.daemon().map_or(Vec::new(), |x| {
        x.traces
            .iter()
            .filter(|t| t.edit.owner == owner && t.kind == ac2_proto::model::TraceKind::Sweep)
            .map(|t| t.edit.name.clone())
            .collect()
    })
}
