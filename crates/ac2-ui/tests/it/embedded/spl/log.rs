//! The SPL log: the run clock, a new log, the history rebuilt from the log, presets.

use super::spl_log;
use crate::embedded::harness::{Driver, NAME, R, measure_from_empty};
use ac2_proto::FrameData;
use ac2_proto::model::MeasKind;
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::CommandId;
use ac2_ui::state::{AppState, Msg, Overlay, StimPhase};
use std::time::Instant;

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
    ac2_ui::scenes::leq(
        s,
        crate::common::pane(s, ac2_ui::state::PaneKind::Spl),
        &Theme::dark(),
        size,
        now,
    )
    .and_then(|x| x.run)
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
/// Shift+R twice starts a new log, and the clock starts again from zero.
#[test]
fn slow_run_clock_and_a_new_log_from_the_app() -> R {
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
    assert_eq!(d.st.layout.focus_kind(), PaneKind::Spl);
    if !d
        .st
        .kind_modes(ac2_ui::state::PaneKind::Spl)
        .spl
        .shows_leq()
    {
        d.key("G");
    }
    // The history strip is a set-once Display choice: the palette command (no key).
    assert!(!d.st.view.spl.layout.history);
    d.send(Msg::Command(CommandId::SplLeqHistory));
    assert!(d.st.view.spl.layout.history);
    d.until("the run clock past 4 s", |s| {
        run_seconds(s).is_some_and(|t| t >= 4)
    })?;
    let caption = run_caption(&d.st).ok_or("caption")?;
    assert!(caption.contains(" since "), "{caption}");
    assert!(caption.contains("LAeq total "), "{caption}");
    // History off: the run goes with it; on again, it is back.
    d.send(Msg::Command(CommandId::SplLeqHistory));
    assert_eq!(run_caption(&d.st), None);
    d.send(Msg::Command(CommandId::SplLeqHistory));
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

    // Shift+R, Shift+R again: a new log; the clock starts from zero again.
    d.key("Shift+R");
    d.key("Shift+R");
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
    ac2_ui::scenes::leq(
        s,
        crate::common::pane(s, ac2_ui::state::PaneKind::Spl),
        &Theme::dark(),
        size,
        now,
    )
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
fn slow_a_restarted_app_shows_the_history_from_the_log() -> R {
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
    assert_eq!(d.st.layout.focus_kind(), PaneKind::Spl);
    if !d
        .st
        .kind_modes(ac2_ui::state::PaneKind::Spl)
        .spl
        .shows_leq()
    {
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
    d.key("Shift+R");
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
/// preset row, Enter, and the meter has only LAeq 30 min ≤ 99 dB SPL. The log carries on (in
/// place, not a new log), and the history of the new window is rebuilt from it.
#[test]
fn slow_a_preset_replaces_the_windows_from_the_app() -> R {
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
        "DIN 15905-5: LAeq 30 min ≤ 99 dB SPL, LCpeak ≤ 135 dB SPL"
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
    d.until("only LAeq 30 min ≤ 99 dB SPL", |s| {
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
