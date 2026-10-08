//! The SPL meter, Leq windows and limits, the SPL log and its history.

use crate::harness::{Driver, NAME, R, calibrate, measure_from_empty, scene_texts};
use ac2_proto::FrameData;
use ac2_proto::model::MeasKind;
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_scene::primitives::Color;
use ac2_scene::theme::Theme;
use ac2_scene::view::LeqStyle;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::{CommandId, Keymap};
use ac2_ui::state::{AppState, Msg, Overlay, Severity, StimPhase};
use std::time::{Duration, Instant};

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
    // Up wraps to the last rows: the band meter (off: its one row), then the Extras.
    for _ in 0..3 {
        d.key("ArrowUp");
    }
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
