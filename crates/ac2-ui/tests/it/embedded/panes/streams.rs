//! Streams: subscriptions follow the panes, the spectrograph (one and two side by side), and
//! record and replay.

use super::{hint_line, new_spectrum, spectrum_panes};
use crate::embedded::harness::{Driver, NAME, R, measure_from_empty, session_from_empty};
use ac2_client::{ClientConfig, Endpoints};
use ac2_proto::FrameData;
use ac2_proto::model::MeasKind;
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_scene::theme::Theme;
use ac2_scene::view::SpectrumMode;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::CommandId;
use ac2_ui::state::{AppState, Msg, Overlay, StimPhase};
use std::time::{Duration, Instant};

/// The streams of `meas` the app holds frames of now.
fn streams_of(s: &AppState, meas: MeasId) -> Vec<Stream> {
    s.rig
        .data
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
/// arrives while its pane is shown and stops when it is closed (the daemon derives it only
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
    d.until("armed", |s| s.rig.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    let has = |s: &AppState, st: Stream| streams_of(s, m).contains(&st);
    d.until("the IR, its pane shown", |s| {
        has(s, Stream::Tf) && has(s, Stream::Ir)
    })?;
    assert!(!has(&d.st, Stream::Levels));

    d.key("Alt+3");
    d.key("Q");
    assert!(d.st.layout.views.values().all(|v| !v.shows_ir()));
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
    // A new pane beside the transfer pane, stepped (G) to its IR view.
    d.key("Alt+1");
    d.key("N");
    for _ in 0..3 {
        d.key("G");
    }
    assert!(d.st.layout.focused().shows_ir());
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
        hint_line(&d.st).iter().any(|h| h == "G/Shift+G view"),
        "{:?}",
        hint_line(&d.st)
    );
    d.key("G");
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Spectrum).spectrum,
        SpectrumMode::Split
    );
    // The level typed for the transfer measurement is still set: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.rig.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|d| d.generator.firing))?;
    let filled = |s: &AppState, n: usize| {
        s.rig
            .spectrographs
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
    let s = ac2_ui::scenes::spectrograph(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    );
    assert_eq!(s.caption, format!("{name} · last 30 s · dBFS (tone)"));
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
    let r = crate::common::spectrum_level(&d.st);
    assert_eq!(history.range, [r.lo as f32, r.hi as f32]);
    // The rig's noise at 1 kHz, just now.
    d.send(Msg::SpectrographCursor {
        hz: 1000.0,
        before_s: 0.1,
    });
    let s = ac2_ui::scenes::spectrograph(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    );
    let text = s.cursor.ok_or("cursor")?.text;
    assert!(
        text.starts_with("1.00 kHz · 0.1 s ago · ") && text.ends_with(" dBFS (tone)"),
        "{text}"
    );
    // The cursor toggle turns it off, time and all.
    d.send(Msg::Command(CommandId::ToggleCursor));
    assert_eq!(d.st.view.spectrum.spectrograph.cursor_s, None);

    // A minute of history (Shift+B), the measurement's, started afresh.
    d.send(Msg::Command(CommandId::SpectrographSpan));
    d.until("the measurement's minute", |s| {
        s.meas(sp)
            .is_some_and(|m| s.display_of(m).spectrograph_span_s == 60)
    })?;
    d.until("a history of a minute", |s| {
        s.rig
            .spectrographs
            .get(&sp)
            .is_some_and(|h| h.span_s() == 60)
    })?;
    d.until("frames in the minute", |s| filled(s, 10))?;
    let s = ac2_ui::scenes::spectrograph(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    );
    assert!(s.caption.contains("last 60 s"), "{}", s.caption);

    // Stopped: the picture stays and says so.
    d.stop()?;
    d.key("S");
    d.until("the spectrum stopped", |s| {
        s.measurements().iter().any(|m| m.id == sp && !m.running)
    })?;
    let s = ac2_ui::scenes::spectrograph(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    );
    assert!(s.caption.ends_with(" · stopped"), "{}", s.caption);
    assert!(filled(&d.st, 10));
    // G: the spectrograph alone, its history kept; W makes it the only pane, full size.
    d.key("G");
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Spectrum).spectrum,
        SpectrumMode::Spectrograph
    );
    assert!(filled(&d.st, 10));
    d.key("W");
    assert_eq!(
        crate::common::visible(&d.st),
        [ac2_ui::state::PaneKind::Spectrum]
    );
    let alone = ac2_ui::scenes::spectrograph(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    );
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
            .starts_with(&format!("{name} · last 60 s · dBFS (tone) · stopped · ")),
        "{}",
        alone.caption
    );
    // The level keys still move the colours.
    let before = crate::common::spectrum_level(&d.st);
    d.key("Ctrl+I");
    assert_ne!(crate::common::spectrum_level(&d.st), before);
    d.key("W");
    // G again hides it and nothing is kept.
    d.key("G");
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Spectrum).spectrum,
        SpectrumMode::Spectrum
    );
    assert!(d.st.rig.spectrographs.is_empty());
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
    d.until("armed", |s| s.rig.stimulus.phase == StimPhase::Armed)?;
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
        s.rig.data.as_ref().is_some_and(|x| {
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

/// The operator's case from an empty daemon: two spectrum measurements on two inputs, the
/// spectrum pane split (N), Spectrum 2 picked on the new half from its title chip, both
/// halves stepped (G) to the spectrograph alone. Left Spectrum 1's spectrograph, right
/// Spectrum 2's, both filling; and so they stay while the focus moves between them, the
/// list's rows are clicked and Tab / Shift+Tab step through every row and back.
#[test]
fn two_spectrographs_side_by_side_from_an_empty_daemon() -> R {
    use ac2_ui::state::PaneMenuRow;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    // No transfer measurement: its own panes would come with its row on the Tab round.
    session_from_empty(&mut d)?;
    let s1 = new_spectrum(&mut d, false)?;
    let s2 = new_spectrum(&mut d, true)?;
    let of = |s: &AppState, id: MeasId| {
        s.meas(id).map(|m| match &m.config.kind {
            MeasKind::Spectrum { config } => (m.config.name.clone(), config.input),
            _ => (m.config.name.clone(), u16::MAX),
        })
    };
    let (n1, i1) = of(&d.st, s1).ok_or("Spectrum 1")?;
    let (n2, i2) = of(&d.st, s2).ok_or("Spectrum 2")?;
    assert_eq!((n1.as_str(), n2.as_str()), ("Spectrum 1", "Spectrum 2"));
    assert_ne!(i1, i2, "two inputs");

    // The spectrum pane shows Spectrum 1; N splits it side by side.
    d.key("Alt+2");
    d.send(Msg::SelectMeas(s1));
    d.key("N");
    let right = d.st.layout.focus;
    // The new half's chip: Spectrum 2.
    d.send(Msg::PaneMenu(right));
    assert!(matches!(d.st.overlay, Overlay::PaneMenu(_)));
    d.send(Msg::PanePick(right, PaneMenuRow::Meas(s2)));
    let shown = |s: &AppState| -> Vec<(Option<MeasId>, SpectrumMode)> {
        spectrum_panes(s)
            .into_iter()
            .map(|(m, v, _)| (m, v))
            .collect()
    };
    assert_eq!(
        shown(&d.st),
        [
            (Some(s1), SpectrumMode::Spectrum),
            (Some(s2), SpectrumMode::Spectrum)
        ]
    );
    // G twice on the right half, then the left half clicked and G twice.
    d.key("G");
    d.key("G");
    let left =
        d.st.visible_panes()
            .into_iter()
            .find(|p| d.st.layout.kind(*p) == ac2_ui::state::PaneKind::Spectrum)
            .ok_or("the left half")?;
    d.send(Msg::FocusPane(left));
    d.key("G");
    d.key("G");
    let both = [
        (Some(s1), SpectrumMode::Spectrograph),
        (Some(s2), SpectrumMode::Spectrograph),
    ];
    assert_eq!(shown(&d.st), both, "after G on both halves");
    let captions = |s: &AppState| -> Vec<String> {
        spectrum_panes(s)
            .into_iter()
            .map(|(_, _, c)| c.split(" · ").next().unwrap_or_default().to_owned())
            .collect()
    };
    assert_eq!(captions(&d.st), ["Spectrum 1", "Spectrum 2"]);
    // Both histories fill, side by side (30 slots a second at 30 s).
    let filled = |s: &AppState, id: MeasId, n: usize| {
        s.rig
            .spectrographs
            .get(&id)
            .is_some_and(|h| h.ring().iter().flatten().count() >= n)
    };
    d.until("both spectrographs filling", |s| {
        filled(s, s1, 30) && filled(s, s2, 30)
    })?;
    assert_eq!(shown(&d.st), both, "once the daemon has the layouts");

    // The focus moving between the halves, and each one's list row clicked with its half
    // focused: nothing moves.
    let half = |s: &AppState, i: usize| {
        s.visible_panes()
            .into_iter()
            .filter(|p| s.layout.kind(*p) == ac2_ui::state::PaneKind::Spectrum)
            .nth(i)
    };
    for (i, id) in [(1, s2), (0, s1), (1, s2), (0, s1)] {
        let p = half(&d.st, i).ok_or("a half")?;
        d.send(Msg::FocusPane(p));
        assert_eq!(shown(&d.st), both, "half {i} clicked");
        d.send(Msg::SelectMeas(id));
        assert_eq!(shown(&d.st), both, "{id:?}'s row clicked on half {i}");
        d.pump();
        assert_eq!(
            shown(&d.st),
            both,
            "{id:?}'s row clicked on half {i}, pumped"
        );
    }

    // Tab through every row and round to the half's own measurement again, then Shift+Tab
    // round the other way, on each half: the other half never moves, and back on its own
    // measurement the half is its spectrograph again.
    let rows = d.st.tree_rows().len() + 1;
    for (i, id) in [(1, s2), (0, s1)] {
        for key in ["Tab", "Shift+Tab"] {
            let p = half(&d.st, i).ok_or("a half")?;
            d.send(Msg::FocusPane(p));
            let mut steps = 0;
            loop {
                d.key(key);
                d.pump();
                steps += 1;
                if d.st.rig.selected == Some(id)
                    && d.st.rig.selected_trace.is_none()
                    && !d.st.live_row_selected()
                {
                    break;
                }
                assert!(steps <= rows, "{key} never came back to {id:?}");
            }
            assert_eq!(shown(&d.st), both, "{key} round on half {i}");
            assert_eq!(captions(&d.st), ["Spectrum 1", "Spectrum 2"]);
        }
    }
    assert!(
        filled(&d.st, s1, 30) && filled(&d.st, s2, 30),
        "the histories stay"
    );
    drop(d);
    drop(daemon);
    Ok(())
}
