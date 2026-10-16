//! Leq limits: windows going over and recovering, a peak limit with the position correction,
//! filling windows judged on their budgets.

use super::{columns, judgement, spl_log, tiles};
use crate::embedded::harness::{Driver, NAME, R, calibrate, measure_from_empty, scene_texts};
use ac2_proto::model::MeasKind;
use ac2_scene::primitives::Color;
use ac2_scene::theme::Theme;
use ac2_scene::view::LeqStyle;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::state::{AppState, Msg, Overlay, Severity, StimPhase};
use std::time::{Duration, Instant};

/// Leq windows with limits from an empty daemon, using the app: the session from its dialog,
/// an SPL meter from the palette, its windows from the Leq dialog by keys (a 5 s and a 10 s
/// window limited to 85 dB), the stimulus from the keys. Pink noise at −20 dBFS reads about
/// 91 dB(A) on the calibrated mic: both windows' columns (the default layout) and tiles turn
/// red, the alarms arrive as toasts; the stop brings both back under the limit, and that is
/// a toast too.
#[test]
fn slow_leq_limits_go_over_and_recover_from_the_app() -> R {
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
        d.st.kind_modes(ac2_ui::state::PaneKind::Spl)
            .spl
            .shows_leq(),
        "the SPL pane shows the windows"
    );
    assert_eq!(d.st.layout.focus_kind(), PaneKind::Spl);
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
fn slow_a_peak_limit_and_the_position_correction_from_the_app() -> R {
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
    // Up wraps to the last rows: the cue fold, the rig scope, the band meter (off: its one
    // row), then the Extras.
    for _ in 0..5 {
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
    let scene = ac2_ui::scenes::leq(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spl),
        &Theme::dark(),
        size,
        now,
    )
    .ok_or("the Leq view")?;
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
    ac2_ui::scenes::leq(
        s,
        crate::common::pane(s, ac2_ui::state::PaneKind::Spl),
        &Theme::dark(),
        size,
        now,
    )
    .and_then(|x| x.columns)
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
fn slow_filling_windows_go_red_only_when_their_budget_is_spent() -> R {
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
    assert_eq!(d.st.layout.focus_kind(), PaneKind::Spl);
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
