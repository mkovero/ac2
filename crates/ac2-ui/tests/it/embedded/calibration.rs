//! Mic curves and electrical/acoustic calibration from the app.

use super::harness::{DEADLINE, Driver, NAME, R, measure_from_empty, scene_texts};
use ac2_client::ClientConfig;
use ac2_proto::FrameData;
use ac2_proto::model::MeasKind;
use ac2_proto::topic::{Stream, Topic};
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::Keymap;
use ac2_ui::state::{AppState, Msg, Overlay};
use std::time::{Duration, Instant};

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
