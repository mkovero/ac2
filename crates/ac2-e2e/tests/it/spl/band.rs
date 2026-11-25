//! The band meter from the app against a real daemon: its windows set from the Leq dialog
//! by keys and by its + and −, the band view of the daemon's own band Leq, and the band
//! transfer step measured on the simulated rig.

use crate::harness::{Cli, DEADLINE, Driver, NAME, R, calibrate, measure_from_empty};
use ac2_client::StimulusLease;
use ac2_e2e::{Setup, SimDaemon, sim_daemon};
use ac2_proto::model::{BandLeqConfig, GeneratorDesired, GeneratorSettings, MeasKind, Signal};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{Dbfs, MeasId, WallNs};
use ac2_scene::theme::Theme;
use ac2_ui::forms::FormKind;
use ac2_ui::state::{AppState, Msg, Overlay, PaneKind};
use std::time::{Duration, Instant};

/// From an empty daemon, using the app: a stream, an SPL meter from the palette (the mic,
/// calibrated as `ac2 cal spl` would): the meter's id.
fn spl_meter_from_an_empty_daemon() -> R<(SimDaemon, Driver, Cli, MeasId)> {
    let daemon = sim_daemon(Setup::Empty)?;
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
    let meas =
        d.st.measurements()
            .iter()
            .find(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
            .map(|m| m.id)
            .ok_or("meter")?;
    calibrate(&ep)?;
    let cli = Cli::connect(daemon.endpoints())?;
    Ok((daemon, d, cli, meas))
}

/// The meter's band settings as the daemon holds them.
fn bands_of(s: &AppState, meas: MeasId) -> Option<&BandLeqConfig> {
    s.meas(meas).and_then(|m| match &m.config.kind {
        MeasKind::Spl { config } => config.bands.as_deref(),
        _ => None,
    })
}

/// Pink noise on out 1 at `level`, from the CLI's client.
fn pink(level: f64) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Pink,
            level: Dbfs(level),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    }
}

/// The band view of the SPL pane, once the daemon's band frame is in, and its texts.
fn band_view(d: &mut Driver, meas: MeasId) -> R<(ac2_scene::band_leq::BandLeqScene, Vec<String>)> {
    d.until("the band frame", move |s| {
        s.rig.data.as_ref().is_some_and(|x| {
            x.latest
                .get(&Topic::Data {
                    meas,
                    stream: Stream::BandLeq,
                })
                .is_some()
        })
    })?;
    let scene = ac2_ui::scenes::band_leq(
        &d.st,
        crate::common::pane(&d.st, PaneKind::Spl),
        &Theme::dark(),
        ac2_scene::primitives::Viewport::new(1200.0, 700.0),
        ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: WallNs(0),
        },
    )
    .ok_or("the band view")?;
    let texts = crate::harness::scene_texts(&scene.scene);
    Ok((scene, texts))
}

/// Holds the app's loop for `t` of real time (a span marked on the meter's clock).
fn hold(d: &mut Driver, what: &str, t: Duration) -> R {
    let until = Instant::now() + t;
    d.until(what, move |_| Instant::now() >= until)
}

/// From an empty daemon, using the app: Shift+L and ↑ to the band meter's row under the Leq
/// windows — →→ turns it on with the STM 545/2015 low-frequency preset (a window per band
/// 20 … 200 Hz), ↓ to its first window, Shift+Insert opens the range row, → / ← make it
/// 50 … 80 Hz LAeq 15 min, Enter adds a window per band, ↓ Tab… type a 20 dB limit for the
/// 63 Hz one, ↓ → a +5 dB impulse correction — Enter sends it, and the daemon keeps it.
/// With pink noise playing, the daemon's own band Leq puts 63 Hz LAeq 15 min over its
/// 20 dB limit; G goes on to the bands, which draw the hour's windows as one row and the
/// quarter's as another. Then the band transfer step on the meter's clock: FOH with the
/// noise at −20 dBFS, the place at −50 dBFS, the background with it off; Enter stores a
/// clean 30 dB transfer.
#[test]
fn slow_band_leq_from_an_empty_daemon() -> R {
    use ac2_proto::model::{BandLeqPreset, ImpulseCorrection};
    use ac2_proto::units::DbSpl;
    use ac2_ui::leq_dialog::{BandCol, BandFocus, BandMeter, Focus, RangeCol};
    let (_daemon, mut d, cli, meas) = spl_meter_from_an_empty_daemon()?;

    // Shift+L, ↑↑↑: from the preset row up past the wrap, the cue fold and the rig scope to
    // the band meter's row (off: its only one).
    d.key("Shift+L");
    d.send(Msg::Text("L".into()));
    d.until("the Leq dialog", |s| s.overlay.leq().is_some())?;
    for _ in 0..3 {
        d.key("ArrowUp");
    }
    d.key("ArrowRight");
    d.key("ArrowRight");
    let leq = |s: &AppState| s.overlay.leq().cloned();
    let x = leq(&d.st).ok_or("the dialog")?;
    assert_eq!(
        x.bands.meter,
        BandMeter::Preset(BandLeqPreset::Finland545Lf)
    );
    assert_eq!(x.bands.rows.len(), 11);
    // ↓ to the first window; Shift+Insert: the range row.
    d.key("ArrowDown");
    d.key("Shift+Insert");
    let x = leq(&d.st).ok_or("the dialog")?;
    assert!(x.bands.range.is_some());
    assert_eq!(x.focus, Focus::Band(BandFocus::Range(RangeCol::From)));
    // From 50 Hz, to 80 Hz, LAeq 15 min.
    for _ in 0..4 {
        d.key("ArrowRight");
    }
    d.key("Tab");
    for _ in 0..4 {
        d.key("ArrowLeft");
    }
    d.key("Tab");
    d.key("ArrowLeft");
    d.key("ArrowLeft");
    d.key("Tab");
    d.key("ArrowLeft");
    d.key("ArrowLeft");
    let x = leq(&d.st).ok_or("the dialog")?;
    let r = x.bands.range.ok_or("the range row")?;
    assert_eq!(
        RangeCol::ALL.map(|c| r.cell(c)),
        ["50 Hz", "80 Hz", "LAeq 15 min", "A"]
    );
    // Enter on the range row adds it: the focus on the first added window, 50 Hz.
    d.key("Enter");
    let x = leq(&d.st).ok_or("the dialog")?;
    assert!(x.bands.range.is_none());
    assert_eq!(x.bands.rows.len(), 14);
    assert_eq!(
        x.focus,
        Focus::Band(BandFocus::Window {
            row: 11,
            col: BandCol::Band,
        })
    );
    // ↓ to 63 Hz, Tab ×3 to its limit.
    d.key("ArrowDown");
    for _ in 0..3 {
        d.key("Tab");
    }
    d.send(Msg::Text("20".into()));
    d.key("ArrowDown");
    d.key("ArrowDown");
    d.key("ArrowRight");
    let x = leq(&d.st).ok_or("the dialog")?;
    assert_eq!(x.bands.rows[12].name(), "63 Hz band LAeq 15 min");
    assert_eq!(x.bands.rows[12].limit, "20");
    assert_eq!(x.bands.impulse, ImpulseCorrection::Plus5);
    d.key("Enter");
    d.until("the band meter set", |s| {
        s.overlay == Overlay::None
            && bands_of(s, meas).is_some_and(|b| {
                b.correction.impulse == ImpulseCorrection::Plus5
                    && b.windows.len() == 14
                    && b.windows[12].limit == Some(DbSpl(20.0))
                    && b.windows[12].day_offset.is_none()
            })
    })?;

    let lease = cli.lease(pink(-20.0))?;
    // G from meter + Leq (the default) on to the bands: the meter has a band meter now.
    d.key("G");
    assert_eq!(
        d.st.kind_modes(PaneKind::Spl).spl,
        ac2_scene::view::SplMode::Bands
    );
    // The G hint is the same in every pane: it steps the views, the bands among them.
    let hints =
        d.st.key_hint_line(
            &d.keys,
            crate::common::pane(&d.st, PaneKind::Spl),
            ac2_ui::keys::LabelStyle::Pc,
        )
        .ok_or("the SPL pane focused")?;
    assert_eq!(hints[0].text(), "G/Shift+G view");
    d.key("W");
    assert!(d.st.layout.maximized);
    // The 63 Hz quarter hour over its 20 dB limit already: the noise so far spent more than
    // the whole window's budget. The headline names the worst window, whichever it is.
    let topic = Topic::Data {
        meas,
        stream: Stream::BandLeq,
    };
    let quarter_over = |s: &AppState| {
        use ac2_proto::FrameData;
        use ac2_proto::model::LeqJudgement;
        match s
            .rig
            .data
            .as_ref()
            .and_then(|x| x.latest.get(&topic))
            .map(|f| &f.frame.data)
        {
            Some(FrameData::BandLeq(f)) => {
                f.meta
                    .windows
                    .get(12)
                    .is_some_and(|w| w.band == ac2_proto::units::Hz(63.0) && w.duration.0 == 900.0)
                    && f.flags
                        .get(12)
                        .is_some_and(|x| x.judgement() == LeqJudgement::Over)
            }
            _ => false,
        }
    };
    let end = Instant::now() + DEADLINE;
    let (scene, texts) = loop {
        let (scene, texts) = band_view(&mut d, meas)?;
        if quarter_over(&d.st)
            && texts
                .iter()
                .any(|t| t.contains(" band L") && t.contains(" over its limit"))
        {
            break (scene, texts);
        }
        if Instant::now() > end {
            return Err(format!(
                "never over: {}",
                crate::common::scene_texts_excerpt(&texts, crate::common::TEXTS_SHOWN)
            )
            .into());
        }
        d.pump();
    };
    let shown = crate::common::scene_texts_excerpt(&texts, crate::common::TEXTS_SHOWN);
    // One row per length and weighting: the hour's eleven bars, the quarter's three.
    assert_eq!(
        scene.columns.iter().map(Vec::len).collect::<Vec<_>>(),
        [11, 3],
        "{shown}"
    );
    assert!(
        texts.iter().any(|t| t.starts_with("LZeq 60 min · ")),
        "{shown}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("LAeq 15 min · so far · ") && t.ends_with(" / 15:00")),
        "{shown}"
    );
    // The key to the two marks across the columns.
    assert!(texts.iter().any(|t| t == "limit"), "{shown}");
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("transfer") || t.contains("bedroom")),
        "{shown}"
    );
    // T, the plot chrome, on to no grid, no scale and back.
    for c in [
        ac2_scene::view::PlotChrome::NoGrid,
        ac2_scene::view::PlotChrome::Bare,
        ac2_scene::view::PlotChrome::Full,
    ] {
        d.key("T");
        assert_eq!(d.st.kind_modes(PaneKind::Spl).chrome, c);
    }

    band_transfer_step(&mut d, &cli, lease, meas)
}

/// The band transfer step, on from the Leq dialog's band rows (T): ↑ to the place's name,
/// typed `flat 4`; Space starts and stops each span on the meter's clock, each read back
/// with `spl.band_log_get`: FOH with the noise at −20 dBFS, the place at −50 dBFS, the
/// background with it off. Enter stores the transfer in the meter: every band clean, 63 Hz
/// 30 dB down.
fn band_transfer_step(d: &mut Driver, cli: &Cli, lease: StimulusLease, meas: MeasId) -> R {
    use ac2_proto::model::BandTransferBand;
    use ac2_scene::band_transfer::{SpanRole, SpanState};
    use ac2_ui::leq_dialog::TransferStep;
    fn step_of(s: &AppState) -> Option<&TransferStep> {
        s.overlay.leq().and_then(|d| d.transfer.as_ref())
    }
    d.key("Shift+L");
    d.send(Msg::Text("L".into()));
    d.until("the Leq dialog", |s| s.overlay.leq().is_some())?;
    // ↑ past the wrap, the cue fold and the rig scope to the last band row.
    for _ in 0..3 {
        d.key("ArrowUp");
    }
    d.key("T");
    d.send(Msg::Text("t".into()));
    assert!(
        step_of(&d.st).is_some_and(|t| t.focus == SpanRole::Foh),
        "the band transfer step"
    );
    d.key("ArrowUp");
    for _ in 0.."receiving room".len() {
        d.key("Backspace");
    }
    d.send(Msg::Text("flat 4".into()));
    d.key("ArrowDown");
    assert!(
        step_of(&d.st).is_some_and(|t| t.place == "flat 4" && !t.on_place),
        "the place named"
    );
    let next = step_of(&d.st)
        .map(TransferStep::next_step)
        .unwrap_or_default();
    assert!(
        next.starts_with("Play a steady test signal (pink noise)"),
        "{next}"
    );

    let mut lease = Some(lease);
    for role in SpanRole::ALL {
        match role {
            SpanRole::Foh => {}
            SpanRole::Place => {
                let l = lease.as_ref().ok_or("lease")?;
                cli.block_on(l.set(pink(-50.0)))?;
            }
            SpanRole::Background => {
                if let Some(l) = lease.take() {
                    cli.block_on(l.end())?;
                }
            }
        }
        // The band filters' memory of the level before: let it ring out.
        hold(d, "the level settled", Duration::from_millis(1500))?;
        d.key("Space");
        d.send(Msg::Text(" ".into()));
        d.until(&role.title("flat 4"), |s| {
            step_of(s).is_some_and(|t| matches!(t.span(role).state, SpanState::Marking { .. }))
        })?;
        hold(d, "two seconds logged", Duration::from_millis(3200))?;
        d.key("Space");
        d.send(Msg::Text(" ".into()));
        d.until("the span read back", |s| {
            step_of(s).is_some_and(|t| t.span(role).average.is_some())
        })?;
        let a = step_of(&d.st)
            .and_then(|t| t.span(role).average)
            .ok_or("average")?;
        assert!(a.seconds >= 1, "{role:?}: {a:?}");
    }
    let next = step_of(&d.st)
        .map(TransferStep::next_step)
        .unwrap_or_default();
    assert!(
        next.starts_with("Enter computes the band transfer and stores it in "),
        "{next}"
    );

    d.key("Enter");
    d.until("the transfer stored", |s| {
        step_of(s).is_some_and(|t| t.stored.is_some())
            && bands_of(s, meas).is_some_and(|b| b.transfer.is_some())
    })?;
    let t = step_of(&d.st).ok_or("step")?;
    assert!(
        t.next_step().starts_with("Band transfer stored in "),
        "{}",
        t.next_step()
    );
    let lines = t.result_lines();
    assert_eq!(lines[0], "transfer from flat 4, 20–200 Hz: 11 clean");
    let stored = bands_of(&d.st, meas)
        .and_then(|b| b.transfer.clone())
        .ok_or("stored")?;
    assert_eq!(stored.place, "flat 4");
    // 63 Hz: the same pink noise 30 dB down, read over a few seconds of a third-octave band.
    let Some(BandTransferBand::Clean { attenuation }) = stored.bands.get(5).copied() else {
        return Err(format!("63 Hz not clean: {:?}", stored.bands).into());
    };
    assert!((attenuation.0 - 30.0).abs() < 2.0, "{attenuation:?}");
    Ok(())
}

/// From an empty daemon, with the dialog's + and −: + on the band windows' heading turns
/// the band meter on with one window, 20 Hz LZeq 60 min; + again adds the next band up,
/// 25 Hz; ↑ Tab to the first's length, ← to 1 min, Tab Tab to its limit, 80 typed, Enter.
/// The daemon keeps the windows and its band Leq of the playing noise shows each as a row
/// of one bar, the 20 Hz one judged against its limit. Then − on each row removes it.
#[test]
fn slow_a_single_band_window_with_plus_and_minus() -> R {
    use ac2_proto::model::Weighting;
    use ac2_proto::units::{DbSpl, Hz, Seconds};
    use ac2_ui::leq_dialog::{BandCol, BandField, BandFocus, BandMeter, Focus};
    use ac2_ui::state::LeqMsg;
    let (_daemon, mut d, cli, meas) = spl_meter_from_an_empty_daemon()?;
    let _noise = cli.lease(pink(-20.0))?;
    d.key("Shift+L");
    d.send(Msg::Text("L".into()));
    d.until("the Leq dialog", |s| s.overlay.leq().is_some())?;
    let names = |s: &AppState| -> Vec<String> {
        s.overlay
            .leq()
            .map(|d| d.bands.rows.iter().map(|r| r.name()).collect())
            .unwrap_or_default()
    };
    // The band windows' +, as its click sends it.
    let plus = Msg::Leq(LeqMsg::Add(Focus::Band(BandFocus::Field(BandField::Meter))));
    d.send(plus.clone());
    assert_eq!(
        d.st.overlay.leq().map(|x| x.bands.meter),
        Some(BandMeter::On)
    );
    assert_eq!(names(&d.st), ["20 Hz band LZeq 60 min"]);
    d.send(plus);
    assert_eq!(
        names(&d.st),
        ["20 Hz band LZeq 60 min", "25 Hz band LZeq 60 min"]
    );
    // The focus is on the new row's band: ↑ to the first, Tab to its length.
    d.key("ArrowUp");
    d.key("Tab");
    for _ in 0..5 {
        d.key("ArrowLeft");
    }
    d.key("Tab");
    d.key("Tab");
    d.send(Msg::Text("80".into()));
    let x = d.st.overlay.leq().ok_or("the dialog")?;
    assert_eq!(x.bands.rows[0].name(), "20 Hz band LZeq 1 min");
    assert_eq!(x.bands.rows[0].limit, "80");
    d.key("Enter");
    d.until("the band windows set", |s| {
        s.overlay == Overlay::None
            && bands_of(s, meas).is_some_and(|b| {
                b.windows.len() == 2
                    && b.windows[0].band == Hz(20.0)
                    && b.windows[0].duration == Seconds(60.0)
                    && b.windows[0].weighting == Weighting::Z
                    && b.windows[0].limit == Some(DbSpl(80.0))
                    && b.windows[1].band == Hz(25.0)
                    && b.windows[1].duration == Seconds(3600.0)
                    && b.windows[1].limit.is_none()
            })
    })?;
    d.key("G");
    assert_eq!(
        d.st.kind_modes(PaneKind::Spl).spl,
        ac2_scene::view::SplMode::Bands
    );
    // The 20 Hz window judged against its limit (pink noise at −20 dBFS reads under 80 dB
    // in the third octave at 20 Hz).
    let end = Instant::now() + DEADLINE;
    let (scene, texts) = loop {
        let (scene, texts) = band_view(&mut d, meas)?;
        if texts
            .iter()
            .any(|t| t.starts_with("20 Hz band LZeq 1 min ") && t.contains(" under its limit"))
        {
            break (scene, texts);
        }
        if Instant::now() > end {
            return Err(format!(
                "never judged: {}",
                crate::common::scene_texts_excerpt(&texts, crate::common::TEXTS_SHOWN)
            )
            .into());
        }
        d.pump();
    };
    let shown = crate::common::scene_texts_excerpt(&texts, crate::common::TEXTS_SHOWN);
    // Two lengths: two rows of one bar each.
    assert_eq!(
        scene.columns.iter().map(Vec::len).collect::<Vec<_>>(),
        [1, 1],
        "{shown}"
    );
    for want in ["20", "25"] {
        assert!(texts.iter().any(|t| t == want), "{want:?} not in {shown}");
    }
    for (row, of) in [("LZeq 1 min", " / 1:00"), ("LZeq 60 min", " / 1:00:00")] {
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with(&format!("{row} · so far · ")) && t.ends_with(of)),
            "{row} not in {shown}"
        );
    }

    // − on each row: gone.
    d.key("Shift+L");
    d.send(Msg::Text("L".into()));
    d.until("the Leq dialog again", |s| {
        s.overlay.leq().is_some_and(|d| d.bands.rows.len() == 2)
    })?;
    let minus = |row| {
        Msg::Leq(LeqMsg::Remove(Focus::Band(BandFocus::Window {
            row,
            col: BandCol::Band,
        })))
    };
    d.send(minus(1));
    assert_eq!(names(&d.st), ["20 Hz band LZeq 1 min"]);
    d.send(minus(0));
    assert!(names(&d.st).is_empty());
    d.key("Enter");
    d.until("the band windows gone", |s| {
        s.overlay == Overlay::None && bands_of(s, meas).is_none_or(|b| b.windows.is_empty())
    })
}
