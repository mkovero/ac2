//! Reducer tests of the SPL meter and Leq: windows, logs, history and alarms.

use super::*;

/// A once-a-second Leq frame is a little over 1 s old just before the next one arrives; only
/// the client's per-stream flag makes it stale, so the Leq view does not dim for a moment.
#[test]
fn leq_frame_a_little_over_a_second_old_is_fresh() {
    let ConnEvent::Data(d) = leq_data(1, 100, 80.0, ac2_proto::frame::LeqFlags::NONE) else {
        unreachable!()
    };
    let mut f = d.latest.frames.values().next().expect("frame").clone();
    f.age = Some(1.2);
    f.since_new = std::time::Duration::from_millis(1200);
    assert!(!crate::scenes::freshness(&AppState::default(), &f).is_stale());
    f.stale = true;
    assert!(crate::scenes::freshness(&AppState::default(), &f).is_stale());
}

/// Shift+L opens the SPL meter's Leq windows by name; a preset (its windows replace the
/// meter's), a window added and its limit typed, Enter sends the meter's configuration with
/// the new windows and the SPL pane shows them. G
/// switches the pane between the meter and its windows.
#[test]
fn leq_windows_from_the_keyboard() {
    let mut t = T::new();
    // No meter: the key says what to do.
    t.key("Shift+L");
    assert!(
        t.last_toast().contains("no SPL meter"),
        "{}",
        t.last_toast()
    );
    t.conn(mirror(with_spl()));
    t.conn(leq_data(1, 100, 80.0, ac2_proto::frame::LeqFlags::NONE));
    t.type_key("Shift+L", "L");
    let Some(d) = t.st.overlay.leq() else {
        panic!("{:?}", t.st.overlay)
    };
    assert_eq!(d.name, "FOH SPL");
    assert!(d.calibrated);
    assert_eq!(d.rows.len(), 5);
    // → on the preset: DIN 15905-5, its LAeq 30 min window alone.
    t.key("ArrowRight");
    // ↓ ↓ to it, Insert adds a longer one, Tab Tab to its limit, typed.
    t.key("ArrowDown");
    t.key("ArrowDown");
    t.key("Insert");
    t.key("Tab");
    t.key("Tab");
    t.text("100");
    let Some(d) = t.st.overlay.leq() else {
        panic!()
    };
    assert_eq!(d.rows.len(), 2);
    assert_eq!(d.rows[0].limit, "99");
    assert_eq!(d.rows[1].limit, "100");
    let r = t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    let sent = r
        .iter()
        .find_map(|r| match r {
            Request::Call {
                cmd: Command::MeasUpdate { meas, config },
                what,
            } => Some((*meas, config.clone(), what.clone())),
            _ => None,
        })
        .expect("meas.update");
    assert_eq!(sent.0, MeasId(4));
    assert_eq!(sent.2, "FOH SPL: Leq windows set");
    let MeasKind::Spl { config } = sent.1.kind else {
        panic!()
    };
    let windows: Vec<(f64, Option<DbSpl>)> = config
        .leq
        .windows
        .iter()
        .map(|w| (w.duration.0, w.limit))
        .collect();
    assert_eq!(
        windows,
        [(1800.0, Some(DbSpl(99.0))), (3600.0, Some(DbSpl(100.0)))]
    );
    assert_eq!(config.input, 1);
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::MeterLeq);
    assert_eq!(t.focus_kind(), PaneKind::Spl);
    // G: the meter, the windows alone, both again.
    t.key("G");
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::Meter);
    t.key("G");
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::Leq);
    t.key("G");
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::MeterLeq);
    // A refused value keeps the dialog open and says why.
    t.type_key("Shift+L", "L");
    t.key("ArrowDown");
    t.key("ArrowDown");
    t.key("Tab");
    t.key("Tab");
    t.text("loud");
    t.key("Enter");
    let Some(d) = t.st.overlay.leq() else {
        panic!()
    };
    assert!(d.error.as_deref().is_some_and(|e| e.contains("LAeq 1 min")));
}

/// Shift+R in the SPL pane (or "Start a new SPL log…" in Ctrl+K) asks first, naming the
/// run that ends and what starts over; N keeps the log, Enter sends `spl.log_new`.
#[test]
fn new_spl_log_asks_first() {
    let mut t = T::new();
    t.st.local_zone = crate::scenes::LocalZone::Fixed { offset_s: 7200 };
    t.key("Alt+4");
    t.key("Shift+R");
    assert!(
        t.last_toast().contains("no SPL meter"),
        "{}",
        t.last_toast()
    );
    assert_eq!(t.st.overlay, Overlay::None);
    t.conn(mirror(with_spl()));
    // A log of 2:14:05.
    t.conn(leq_data(
        8045,
        1_791_055_000,
        97.84,
        ac2_proto::frame::LeqFlags::NONE,
    ));
    let r = t.key("Shift+R");
    assert!(r.is_empty(), "{r:?}");
    let Overlay::NewLog(p) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay)
    };
    assert_eq!(p.meas, MeasId(4));
    assert_eq!(p.confirm.title, "Start a new SPL log for FOH SPL?");
    // 8045 s up to 19:16:40 UTC+2 on 3 October 2026: since 19:02.
    assert_eq!(
        p.confirm.lines[0],
        "The current log ends: running 2:14:05 since 19:02 · LAeq total 97.8."
    );
    assert!(p.confirm.lines[1].contains("the 5 Leq windows and their states, the alarms"));
    // N keeps the log: nothing sent.
    let r = t.key("N");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    // Esc closes it too.
    t.key("Shift+R");
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
    // From the palette, then Enter: spl.log_new for the pane's meter.
    t.key("Ctrl+K");
    t.text("new spl log");
    t.key("Enter");
    assert!(
        matches!(t.st.overlay, Overlay::NewLog(_)),
        "{:?}",
        t.st.overlay
    );
    let r = t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(
        r.iter().any(|r| matches!(
            r,
            Request::Call { cmd: Command::SplLogNew { meas }, what }
                if *meas == MeasId(4) && what == "FOH SPL: new SPL log started"
        )),
        "{r:?}"
    );
    // The mouse: "Keep the current log" sends nothing, "Start a new log" sends it.
    t.key("Shift+R");
    let r = t.st.update(Msg::NewLog(false), &t.keys);
    assert!(r.is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    t.key("Shift+R");
    let r = t.st.update(Msg::NewLog(true), &t.keys);
    assert!(r.iter().any(|r| matches!(
        r,
        Request::Call {
            cmd: Command::SplLogNew { .. },
            ..
        }
    )));
}

/// The Leq view starts as columns without the history strip; B switches columns / tiles, H
/// the strip, each showing the windows on the SPL pane and remembered in the preferences,
/// which the next start applies. Full screen with the pane maximised is the stage view,
/// unless a stimulus may be sounding.
#[test]
fn leq_layout_keys_and_prefs() {
    use ac2_scene::view::{LeqLayout, LeqStyle};
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    t.conn(leq_data(1, 100, 80.0, ac2_proto::frame::LeqFlags::NONE));
    assert_eq!(
        t.st.view.spl.layout,
        LeqLayout {
            style: LeqStyle::Columns,
            history: false
        }
    );
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::MeterLeq);
    t.key("Alt+4");
    assert_eq!(t.focus_kind(), PaneKind::Spl);
    t.key("G");
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::Meter);
    // Columns / tiles from the meter (the palette): the windows, as tiles, under the meter.
    t.st.update(Msg::Command(CommandId::SplLeqStyle), &t.keys);
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::MeterLeq);
    assert_eq!(t.st.view.spl.layout.style, LeqStyle::Tiles);
    assert!(t.st.prefs_dirty);
    assert_eq!(t.st.prefs.leq, t.st.view.spl.layout);
    t.st.prefs_dirty = false;
    t.st.update(Msg::Command(CommandId::SplLeqHistory), &t.keys);
    assert!(t.st.view.spl.layout.history);
    assert!(t.st.prefs_dirty);
    assert_eq!(
        t.st.prefs.leq,
        LeqLayout {
            style: LeqStyle::Tiles,
            history: true
        }
    );
    // G still steps the views and leaves the layout alone.
    t.key("G");
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::Meter);
    t.key("G");
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::Leq);
    assert_eq!(t.st.prefs.leq.style, LeqStyle::Tiles);
    t.st.update(Msg::Command(CommandId::SplLeqStyle), &t.keys);
    t.st.update(Msg::Command(CommandId::SplLeqHistory), &t.keys);
    assert_eq!(t.st.view.spl.layout, LeqLayout::default());
    // The layout is set once (Settings › Display, the palette): B and Shift+B leave it.
    t.key("B");
    t.key("Shift+B");
    assert_eq!(t.st.view.spl.layout, LeqLayout::default());
    // The next start takes the remembered layout.
    let prefs = crate::prefs::UiPrefs {
        leq: LeqLayout {
            style: LeqStyle::Tiles,
            history: true,
        },
        ..Default::default()
    };
    let mut u = T::new();
    u.st.set_prefs(prefs.clone());
    assert_eq!(u.st.view.spl.layout, prefs.leq);
    // The stage view: full screen, the SPL pane maximised on its windows.
    t.key("Alt+4");
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::Leq);
    assert!(!t.st.stage_view());
    t.key("W");
    assert!(!t.st.stage_view());
    t.key("F11");
    assert!(t.st.stage_view());
    // Armed too: full screen stays the pane alone.
    t.st.stimulus.phase = StimPhase::Armed;
    assert!(t.st.stage_view());
    t.st.stimulus.phase = StimPhase::Idle;
    // The meter is full screen as well; W goes back to the split layout.
    t.key("G");
    assert!(t.st.stage_view());
    t.key("W");
    assert!(!t.st.stage_view());
    assert!(!t.st.layout.maximized && !t.st.fullscreen);
}

/// A resync (no daemon state for a moment) is not "every meter deleted": the history strip
/// keeps what it gathered and goes on from there.
#[test]
fn leq_history_survives_a_resync() {
    use ac2_proto::frame::LeqFlags;
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    let judged = LeqFlags::LIMIT.with(LeqFlags::JUDGED);
    t.conn(leq_data(1, 100, 98.0, judged));
    t.conn(leq_data(2, 101, 98.5, judged));
    t.st.mirror = None;
    t.conn(leq_data(3, 102, 99.0, judged));
    t.conn(mirror(with_spl()));
    t.conn(leq_data(4, 103, 99.2, judged));
    let cfg = LeqConfig::default_windows();
    let h = &t.st.leq_history[&MeasId(4)].1;
    let p = h.points(&cfg.windows[0]).expect("series");
    assert!(p.len() >= 3, "kept through the resync: {} points", p.len());
    assert_eq!(p[0].t, 100.0);
}

/// Each new `leq` frame goes into the meter's history once; a window going over or coming
/// back is a toast, alarms that were there before the app connected are not.
#[test]
fn leq_history_and_alarm_toasts() {
    use ac2_proto::frame::LeqFlags;
    let mut t = T::new();
    let mut s = with_spl();
    let old = LeqAlarm {
        at: WallNs(5),
        subject: ac2_proto::model::AlarmSubject::Window {
            duration: Seconds(1800.0),
            weighting: Weighting::A,
        },
        kind: LeqAlarmKind::Over,
        level: DbSpl(99.4),
        limit: DbSpl(99.0),
        position: None,
    };
    s.spl_logs = vec![SplLog {
        meas: MeasId(4),
        started_at: Some(WallNs(1)),
        windows: vec![],
        alarms: vec![old],
        peaks: ac2_proto::model::PeakStates {
            lcpeak: ac2_proto::model::LeqPeakState {
                judgement: ac2_proto::model::LeqJudgement::NoLimit,
                since: ac2_proto::units::WallNs(0),
            },
            lafmax: ac2_proto::model::LeqPeakState {
                judgement: ac2_proto::model::LeqJudgement::NoLimit,
                since: ac2_proto::units::WallNs(0),
            },
        },
    }];
    t.conn(mirror(s.clone()));
    let toasts = t.st.toasts.len();
    let judged = LeqFlags::LIMIT.with(LeqFlags::JUDGED);
    t.conn(leq_data(1, 100, 98.0, judged));
    t.conn(leq_data(1, 100, 98.0, judged));
    t.conn(leq_data(2, 101, 99.5, judged.with(LeqFlags::OVER)));
    let cfg = LeqConfig::default_windows();
    let h = &t.st.leq_history[&MeasId(4)].1;
    let p = h.points(&cfg.windows[0]).expect("series");
    assert_eq!(p.len(), 2);
    assert!(p[1].over && !p[0].over);
    assert_eq!(t.st.toasts.len(), toasts, "old alarms are history");
    // Over, then recovered: one toast each, the over one an error.
    let over = LeqAlarm {
        at: WallNs(10),
        ..old
    };
    s.spl_logs[0].alarms.push(over);
    t.conn(mirror(s.clone()));
    assert_eq!(
        t.last_toast(),
        "FOH SPL: LAeq 30 min over its limit — 99.4 dB > 99.0 dB"
    );
    assert!(
        t.st.toasts
            .last()
            .is_some_and(|x| x.severity == Severity::Fault)
    );
    t.conn(mirror(s.clone()));
    assert_eq!(t.st.toasts.len(), toasts + 1, "the same alarm toasts once");
    s.spl_logs[0].alarms.push(LeqAlarm {
        at: WallNs(20),
        kind: LeqAlarmKind::Recovered,
        level: DbSpl(98.9),
        ..old
    });
    t.conn(mirror(s.clone()));
    assert_eq!(
        t.last_toast(),
        "FOH SPL: LAeq 30 min back within its limit — 98.9 dB"
    );
    // Warning toasts off: the alarms go to the log only, the over one still an error.
    t.st.prefs.warning_toasts = false;
    let toasts = t.st.toasts.len();
    s.spl_logs[0].alarms.push(LeqAlarm {
        at: WallNs(30),
        level: DbSpl(99.8),
        ..old
    });
    t.conn(mirror(s));
    assert_eq!(t.st.toasts.len(), toasts, "a muted alarm does not pop up");
    let n = t.st.notices.back().expect("logged");
    assert_eq!(
        n.text,
        "FOH SPL: LAeq 30 min over its limit — 99.8 dB > 99.0 dB"
    );
    assert_eq!(n.severity, Severity::Fault);
}

/// The history rebuild the reducer asked for, if any: (meter, number).
fn backfill_asked(r: &[Request]) -> Option<(MeasId, u64)> {
    r.iter().find_map(|r| match r {
        Request::LeqBackfill { meas, ask } => Some((*meas, *ask)),
        _ => None,
    })
}

/// The history the daemon rebuilt for meter 4's default windows: `seconds` seconds at
/// 80 dB SPL ending at wall second `end_s`.
fn rebuilt(seconds: u64, end_s: u64) -> SplHistory {
    let windows = LeqConfig::default_windows().windows;
    let n = windows.len();
    SplHistory {
        meas: MeasId(4),
        windows,
        scale: LevelScale::DbSpl,
        at: (0..seconds)
            .map(|k| WallNs((end_s - seconds + 1 + k) * 1_000_000_000))
            .collect(),
        leq: vec![vec![80.0; seconds as usize]; n],
        over: vec![vec![false; seconds as usize]; n],
    }
}

/// A restarted app (a new state) seeing a meter that has been logging: it asks for the
/// history from the meter's log, puts it in the strip, and the frames it receives carry
/// it on without a second twice. An answer to an earlier request is stale.
#[test]
fn a_restarted_app_rebuilds_the_history_from_the_log() {
    let mut t = T::new();
    let r = t.conn(mirror(with_spl()));
    let (meas, ask) = backfill_asked(&r).expect("asked for the history");
    assert_eq!(meas, MeasId(4));
    let cfg = LeqConfig::default_windows();
    // The same state again: nothing more to ask.
    assert_eq!(backfill_asked(&t.conn(mirror(with_spl()))), None);
    let b = rebuilt(600, 1000);
    t.conn(ConnEvent::LeqBackfill {
        meas,
        ask: ask + 7,
        result: Ok(Box::new(b.clone())),
    });
    assert!(!t.st.leq_history.contains_key(&meas), "stale: ignored");
    t.conn(ConnEvent::LeqBackfill {
        meas,
        ask,
        result: Ok(Box::new(b)),
    });
    let h = &t.st.leq_history[&meas].1;
    let p = h.points(&cfg.windows[0]).expect("series");
    assert_eq!(p.len(), 600);
    assert_eq!(p[0].t, 401.0);
    assert_eq!(p[599].t, 1000.0);
    assert!((p[599].leq - 80.0).abs() < 1e-4, "{:?}", p[599]);
    // Live frames: the one for the newest logged second is that point, the next one new.
    let flags = ac2_proto::frame::LeqFlags::LIMIT;
    t.conn(leq_data(601, 1000, 80.0, flags));
    t.conn(leq_data(602, 1001, 80.0, flags));
    let p = t.st.leq_history[&meas]
        .1
        .points(&cfg.windows[0])
        .expect("series");
    assert_eq!(p.len(), 601);
    assert_eq!(p[600].t, 1001.0);
}

/// A new log started from anywhere (another client's `spl.log_new`): the log empties, or
/// its rows are numbered from 0 again; the history clears and is rebuilt. New windows are
/// rebuilt too.
#[test]
fn a_new_log_or_new_windows_rebuild_the_history() {
    let flags = ac2_proto::frame::LeqFlags::LIMIT;
    let mut t = T::new();
    let mut s = with_spl();
    s.spl_logs = vec![SplLog {
        meas: MeasId(4),
        started_at: Some(WallNs(400_000_000_000)),
        windows: vec![],
        alarms: vec![],
        peaks: ac2_proto::model::PeakStates {
            lcpeak: ac2_proto::model::LeqPeakState {
                judgement: ac2_proto::model::LeqJudgement::NoLimit,
                since: ac2_proto::units::WallNs(0),
            },
            lafmax: ac2_proto::model::LeqPeakState {
                judgement: ac2_proto::model::LeqJudgement::NoLimit,
                since: ac2_proto::units::WallNs(0),
            },
        },
    }];
    t.conn(mirror(s.clone()));
    t.conn(leq_data(600, 1000, 80.0, flags));
    t.conn(leq_data(601, 1001, 80.0, flags));
    assert!(!t.st.leq_history[&MeasId(4)].1.is_empty());
    // Emptied by a new log.
    s.spl_logs[0].started_at = None;
    let r = t.conn(mirror(s.clone()));
    assert!(backfill_asked(&r).is_some());
    assert!(t.st.leq_history[&MeasId(4)].1.is_empty(), "cleared");
    // Its first rows: the frames say so, nothing more to ask.
    s.spl_logs[0].started_at = Some(WallNs(1_002_000_000_000));
    assert_eq!(backfill_asked(&t.conn(mirror(s.clone()))), None);
    assert_eq!(
        backfill_asked(&t.conn(leq_data(602, 1003, 70.0, flags))),
        None
    );
    t.conn(leq_data(603, 1004, 70.0, flags));
    // Another new log seen only in the frames (the entity's moment missed): fewer rows.
    let r = t.conn(leq_data_logged(604, 3, 1005, 60.0, flags));
    assert!(backfill_asked(&r).is_some());
    let h = &t.st.leq_history[&MeasId(4)].1;
    let p = h
        .points(&LeqConfig::default_windows().windows[0])
        .expect("series");
    assert_eq!(p.len(), 1, "only the new log's second: {p:?}");
    // The windows changed (a preset): rebuilt for the new ones.
    let mut m = with_spl();
    if let MeasKind::Spl { config } = &mut m.measurements.last_mut().expect("spl").config.kind {
        config.leq.windows = LeqPreset::Din15905.windows();
    }
    m.spl_logs = s.spl_logs.clone();
    assert!(backfill_asked(&t.conn(mirror(m))).is_some(), "asked again");
}

/// Reconnected (the link dropped, or the daemon restarted), the history is asked for again;
/// a failure says what it was for.
#[test]
fn a_reconnect_rebuilds_the_history_and_a_failure_is_shown() {
    let mut t = T::new();
    assert!(backfill_asked(&t.conn(mirror(with_spl()))).is_some());
    t.conn(ConnEvent::Connected {
        target: "local daemon".into(),
        server: "ac2d test".into(),
        client_id: ClientId("c1".into()),
    });
    let (meas, ask) = backfill_asked(&t.conn(mirror(with_spl()))).expect("asked again");
    t.conn(ConnEvent::LeqBackfill {
        meas,
        ask,
        result: Err("daemon did not answer `spl.log_get` (3 attempts)".into()),
    });
    assert_eq!(
        t.last_toast(),
        "FOH SPL: Leq history from the log: daemon did not answer `spl.log_get` (3 attempts)"
    );
}

/// An `spl` frame of meter 4 at `at_ms` (daemon clock) under `rev`.
fn spl_data(seq: u64, at_ms: u64, level: f64, tw: TimeWeighting, rev: u64) -> ConnEvent {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{Frame, FrameData, SplFrame, SplMeta};
    let data = FrameData::Spl(SplFrame {
        meas: MeasId(4),
        meta: SplMeta {
            scale: LevelScale::DbSpl,
            weighting: Weighting::A,
            time_weighting: tw,
            peak_weighting: PeakWeighting::C,
            level,
            lmax: 100.0,
            lmin: 80.0,
            leq: 90.0,
            lpeak: 110.0,
            duration: Seconds(60.0),
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            position: None,
        },
    });
    let mut stamp = ac2_proto::samples::stamp(None);
    stamp.seq = seq;
    stamp.capture_wall_ns = WallNs(at_ms * 1_000_000);
    stamp.config_rev = Rev(rev);
    let f = TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame { stamp, data }),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    latest.frames.insert(f.topic.to_string().into(), f);
    ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: Default::default(),
        drained: Instant::now(),
    }))
}

fn spl_update(r: &[Request]) -> Option<(SplConfig, String)> {
    r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::MeasUpdate { config, .. },
            what,
        } => match &config.kind {
            MeasKind::Spl { config } => Some((config.clone(), what.clone())),
            _ => None,
        },
        _ => None,
    })
}

/// F steps the meter's time weighting F → S → I → F, Z its frequency weighting A → C → Z →
/// A, in place (`meas.update` of the same input, the Leq windows as they were); the pane
/// keeps its view (meter or Leq windows). The palette names each choice. Without a meter, it
/// says so.
#[test]
fn spl_keys_cycle_the_weightings() {
    let mut t = T::new();
    assert!(
        t.st.update(Msg::Command(CommandId::SplSlow), &t.keys)
            .is_empty()
    );
    assert!(
        t.last_toast().contains("no SPL meter"),
        "{}",
        t.last_toast()
    );
    let mut state = with_spl();
    t.conn(mirror(state.clone()));
    t.key("Alt+4");
    t.key("G");
    t.key("G");
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::Leq);
    let mut seen = Vec::new();
    for key in ["Shift+F", "Shift+F", "Shift+F", "Z", "Z", "Z"] {
        let (cfg, what) = spl_update(&t.key(key)).expect(key);
        assert_eq!(cfg.input, 1);
        assert_eq!(cfg.leq, LeqConfig::default_windows());
        seen.push(what);
        // The daemon applies it.
        state.measurements[2].config.kind = MeasKind::Spl { config: cfg };
        t.conn(mirror(state.clone()));
    }
    assert_eq!(
        seen,
        [
            "FOH SPL: LAS",
            "FOH SPL: LAI",
            "FOH SPL: LAF",
            "FOH SPL: LCF",
            "FOH SPL: LZF",
            "FOH SPL: LAF"
        ]
    );
    assert_eq!(
        t.st.kind_modes(PaneKind::Spl).spl,
        SplMode::Leq,
        "the Leq windows stay: they do not change"
    );
    // The reply names the meter's new metric over the Leq view.
    t.conn(ConnEvent::Reply {
        what: seen[3].clone(),
        result: Ok(()),
    });
    assert_eq!(t.last_toast(), "FOH SPL: LCF");
    t.key("G");
    t.key("G");
    assert_eq!(t.st.kind_modes(PaneKind::Spl).spl, SplMode::Meter);
    t.key("Shift+F");
    assert_eq!(
        t.st.kind_modes(PaneKind::Spl).spl,
        SplMode::Meter,
        "the meter stays"
    );
    for (c, want) in [
        (CommandId::SplSlow, "FOH SPL: LAS"),
        (CommandId::SplImpulse, "FOH SPL: LAI"),
        (CommandId::SplC, "FOH SPL: LCF"),
        (CommandId::SplZ, "FOH SPL: LZF"),
        (CommandId::SplFast, "FOH SPL: LAF"),
        (CommandId::SplA, "FOH SPL: LAF"),
    ] {
        t.key("Alt+1");
        let r = t.st.update(Msg::Command(c), &t.keys);
        assert_eq!(spl_update(&r).map(|x| x.1).as_deref(), Some(want));
        assert_eq!(t.focus_kind(), PaneKind::Spl);
    }
}

/// Frames every 100 ms: the number takes a new reading every 0.5 s with F and every 1 s with
/// S (the reading at that instant, the bar live in between); a new weighting shows at once;
/// the preference overrides the period.
#[test]
fn spl_number_holds_for_the_display_period() {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    let theme = ac2_scene::theme::Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 800.0,
        height: 500.0,
    };
    let now = crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    };
    let held = |t: &T| t.st.spl_hold.get(&MeasId(4)).map(|h| h.frame.meta.level);
    let mut changes = Vec::new();
    let mut last = None;
    for k in 0..20u64 {
        t.conn(spl_data(
            k + 1,
            1000 + k * 100,
            90.0 + k as f64,
            TimeWeighting::Fast,
            1,
        ));
        if held(&t) != last {
            last = held(&t);
            changes.push(k);
        }
        // The number is the held reading; the bar follows the newest frame.
        let s = crate::scenes::spl(&t.st, t.pane(PaneKind::Spl), &t.keys, &theme, size, now)
            .expect("scene");
        let texts: Vec<&str> = s.scene.layers[2]
            .labels
            .iter()
            .map(|l| l.text.as_str())
            .collect();
        let want = ac2_scene::format::level(last.expect("held"));
        assert!(texts.contains(&want.as_str()), "{k}: {want} in {texts:?}");
    }
    assert_eq!(changes, [0, 5, 10, 15]);
    // Slow under a new rev: at once, then once a second.
    let mut changes = Vec::new();
    for k in 20..50u64 {
        t.conn(spl_data(
            k + 1,
            1000 + k * 100,
            90.0 + k as f64,
            TimeWeighting::Slow,
            2,
        ));
        if held(&t) != last {
            last = held(&t);
            changes.push(k);
        }
    }
    assert_eq!(changes, [20, 30, 40]);
    // A display period of 200 ms from ui.toml.
    t.st.prefs.spl_hold_ms = Some(200);
    assert_eq!(t.st.spl_display_period_s(TimeWeighting::Slow), 0.2);
    let mut n = 0;
    for k in 50..60u64 {
        t.conn(spl_data(
            k + 1,
            1000 + k * 100,
            90.0 + k as f64,
            TimeWeighting::Slow,
            2,
        ));
        if held(&t) != last {
            last = held(&t);
            n += 1;
        }
    }
    assert_eq!(n, 5);
}
