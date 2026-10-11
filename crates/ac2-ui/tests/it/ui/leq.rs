//! Leq tiles and the SPL log.

use super::*;

/// Publishes the given `leq` frame of an SPL meter on the fake daemon every 200 ms, with a
/// new `seq` and the fake's clock each time (fresh, as the daemon's once-a-second frames
/// are); the test swaps the frame.
struct LeqPublisher {
    frame: Arc<std::sync::Mutex<Option<ac2_proto::frame::LeqFrame>>>,
    /// The meter's own frame, published alongside once set.
    spl: Arc<std::sync::Mutex<Option<ac2_proto::frame::SplFrame>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LeqPublisher {
    fn start(fake: Arc<ac2_client::fake::FakeDaemon>) -> Self {
        use ac2_proto::{Frame, FrameData};
        let frame: Arc<std::sync::Mutex<Option<ac2_proto::frame::LeqFrame>>> =
            Arc::new(std::sync::Mutex::new(None));
        let spl: Arc<std::sync::Mutex<Option<ac2_proto::frame::SplFrame>>> =
            Arc::new(std::sync::Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (f, m, s) = (Arc::clone(&frame), Arc::clone(&spl), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            let mut seq = 1;
            while !s.load(Ordering::Acquire) {
                let data = f.lock().ok().and_then(|g| g.clone());
                if let Some(data) = data {
                    let mut st = fake.lock();
                    let frame = Frame {
                        stamp: st.stamp(seq, None),
                        data: FrameData::Leq(Box::new(data)),
                    };
                    st.publish(&frame);
                    seq += 1;
                }
                let meter = m.lock().ok().and_then(|g| g.clone());
                if let Some(data) = meter {
                    let mut st = fake.lock();
                    let frame = Frame {
                        stamp: st.stamp(seq, None),
                        data: FrameData::Spl(data),
                    };
                    st.publish(&frame);
                    seq += 1;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        Self {
            frame,
            spl,
            stop,
            thread: Some(thread),
        }
    }

    fn set(&self, f: ac2_proto::frame::LeqFrame) {
        if let Ok(mut g) = self.frame.lock() {
            *g = Some(f);
        }
    }

    fn set_spl(&self, f: ac2_proto::frame::SplFrame) {
        if let Ok(mut g) = self.spl.lock() {
            *g = Some(f);
        }
    }
}

impl Drop for LeqPublisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 17:02 UTC on 3 October 2026.
const SHOW_START_S: u64 = 1_791_046_920;

/// One `leq` frame of meter `meas`: a value per window, over / near as given, a one-minute
/// horizon. Window order is the default one with DIN 15905-5 on 30 min and a typed limit on
/// 1 min: 1, 5, 10, 30, 60 min.
fn leq_frame(
    meas: MeasId,
    leq: [f32; 5],
    states: [Option<ac2_proto::frame::LeqFlags>; 5],
    allowed: [f32; 5],
    elapsed: [f32; 5],
    calibrated_at: u64,
) -> ac2_proto::frame::LeqFrame {
    use ac2_proto::frame::{LeqFlags, LeqFrame, LeqMeta, LeqRun};
    use ac2_proto::model::{CalStatus, LevelScale};
    use ac2_proto::units::{Seconds, WallNs};
    LeqFrame {
        meas,
        meta: LeqMeta {
            scale: LevelScale::DbSpl,
            cal: CalStatus::Verified {
                calibrated_at: WallNs(calibrated_at),
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0),
                },
            },
            mic_curve: false,
            horizon: Seconds(60.0),
            logged: 2718,
            // The show so far: 45:30 from 19:02 (UTC+2, [`SHOW_ZONE`]), 12 s of it lost.
            run: Some(LeqRun {
                started_at: WallNs(SHOW_START_S * 1_000_000_000),
                until: WallNs((SHOW_START_S + 2730) * 1_000_000_000),
                measured: Seconds(2718.0),
                gaps: Seconds(12.0),
                trimmed: false,
                laeq: f64::from(leq[4]) + 0.4,
                lceq: f64::from(leq[4]) + 6.0,
                lzeq: f64::from(leq[4]) + 9.0,
            }),
            lcpeak: None,
            lafmax: None,
            position: None,
        },
        leq: leq.to_vec(),
        elapsed: elapsed.to_vec(),
        measured: elapsed.to_vec(),
        allowed: allowed.to_vec(),
        recover: vec![f32::NAN; 5],
        // A filling window ends at its energy so far over its whole length if the rest is
        // silent.
        least: leq
            .iter()
            .zip(elapsed)
            .zip([60.0f32, 300.0, 600.0, 1800.0, 3600.0])
            .map(|((l, e), d)| l + 10.0 * (e / d).min(1.0).log10())
            .collect(),
        over_in: vec![f32::NAN; 5],
        flags: states
            .iter()
            .map(|s| match s {
                None => LeqFlags::NONE,
                Some(f) => LeqFlags::LIMIT.with(LeqFlags::JUDGED).with(*f),
            })
            .collect(),
    }
}

/// From an empty fake daemon, using the app: the session from its dialog, an SPL meter from
/// the palette, Shift+L for its Leq windows — a preset shown (DIN 15905-5: its LAeq 30 min
/// window alone) and left, typed limits on LAeq 1 min and 30 min — Enter sends them and the
/// SPL pane shows them as columns. The
/// daemon (here the test, through the fake) then reports the 1 min and 30 min windows over:
/// their columns turn red and the alarms toast; maximised, the columns fill the screen. B and
/// H switch to tiles with the history strip below. Then they recover, and back on columns,
/// F11 is the stage view.
/// ↑ in the open Leq dialog until `to` has the focus: the rows between are counted by the
/// dialog, not by the test.
fn arrow_up_to(h: &mut Harness<'_, App>, to: ac2_ui::leq_dialog::Focus) {
    let at = |h: &Harness<'_, App>| h.state().state.overlay.leq().map(|d| d.focus);
    for _ in 0..32 {
        if at(h) == Some(to) {
            return;
        }
        h.key_press(Key::ArrowUp);
        h.step();
    }
    panic!("↑ never reached {to:?}: focus {:?}", at(h));
}

#[test]
fn leq_tiles_from_an_empty_daemon() {
    use ac2_proto::frame::LeqFlags;
    use ac2_proto::model::{LeqAlarm, LeqAlarmKind, LeqJudgement, MeasKind, SplLog};
    use ac2_proto::units::{DbSpl, Seconds, WallNs};
    use ac2_proto::{Change, Patch};
    if !have_gpu("leq_tiles_from_an_empty_daemon") {
        return;
    }
    let fake =
        Arc::new(ac2_client::fake::FakeDaemon::start(common::fake_options()).expect("fake daemon"));
    let _meters = Meters::start(Arc::clone(&fake));
    let leq = LeqPublisher::start(Arc::clone(&fake));
    let mut h = harness(options_at(Some(fake.endpoints())));
    h.state_mut().state.local_zone = SHOW_ZONE;
    step_until(&mut h, "synced", |a| {
        a.state.mirror.as_ref().is_some_and(|m| m.synced())
    });

    // The session from its dialog.
    h.key_press_modifiers(Modifiers::SHIFT, Key::O);
    step_until(&mut h, "the session dialog with its devices", |a| {
        session_dialog_of(a).is_some_and(|d| d.device_info().is_some())
            && a.state.input_meters().len() == 4
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "session open", |a| a.state.open_session().is_some());
    if matches!(h.state().state.overlay, Overlay::Offer(_)) {
        h.key_press(Key::N);
    }
    step_until(&mut h, "no dialog", |a| a.state.overlay == Overlay::None);

    // An SPL meter from the palette: Ctrl+K, "new spl", Enter, Enter.
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.event(Event::Text("new spl".into()));
    step_until(&mut h, "palette typed", |a| {
        matches!(&a.state.overlay, Overlay::Palette(_))
    });
    h.key_press(Key::Enter);
    step_until(
        &mut h,
        "the SPL dialog",
        |a| matches!(&a.state.overlay, Overlay::Form(f) if f.kind == ac2_ui::forms::FormKind::Spl),
    );
    h.key_press(Key::Enter);
    step_until(&mut h, "the meter, running, with its windows", |a| {
        a.state
            .measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
            && a.state.daemon().is_some_and(|s| s.spl_logs.len() == 1)
    });
    let meas = h
        .state()
        .state
        .measurements()
        .iter()
        .find(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
        .map(|m| m.id)
        .expect("meter");

    // Shift+L: → the DIN preset, its one window; ← back to the meter's windows. ↓↓ the
    // 1 min window, Tab Tab its limit, 102; ↓↓↓ the 30 min one's, 99.
    h.key_press_modifiers(Modifiers::SHIFT, Key::L);
    step_until(&mut h, "the Leq dialog", |a| {
        a.state.overlay.leq().is_some()
    });
    h.key_press(Key::ArrowRight);
    step_until(&mut h, "the DIN preset alone", |a| {
        a.state
            .overlay
            .leq()
            .is_some_and(|d| d.preset == Some(0) && d.rows.len() == 1)
    });
    h.key_press(Key::ArrowLeft);
    h.key_press(Key::ArrowDown);
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Tab);
    h.key_press(Key::Tab);
    h.event(Event::Text("102".into()));
    for _ in 0..3 {
        h.key_press(Key::ArrowDown);
    }
    h.event(Event::Text("99".into()));
    step_until(&mut h, "the limits typed", |a| {
        a.state.overlay.leq().is_some_and(|d| {
            d.preset.is_none()
                && d.rows.len() == 5
                && d.rows[0].limit == "102"
                && d.rows[3].limit == "99"
        })
    });
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "settings_leq");
    // ↑ to the preset row, → to the French preset for children (its two windows, the
    // longest name and source), then ← back to "none": the windows as typed.
    let typed = h
        .state()
        .state
        .overlay
        .leq()
        .expect("the Leq dialog")
        .rows
        .clone();
    let to = ac2_proto::model::LeqPreset::ALL
        .iter()
        .position(|p| *p == ac2_proto::model::LeqPreset::FranceChildren)
        .expect("listed");
    arrow_up_to(&mut h, ac2_ui::leq_dialog::Focus::Preset);
    for _ in 0..=to {
        h.key_press(Key::ArrowRight);
    }
    step_until(&mut h, "two windows from one preset", |a| {
        a.state
            .overlay
            .leq()
            .is_some_and(|d| d.preset == Some(to) && d.rows.len() == 2)
    });
    snapshot(&mut h, "leq_dialog_two_window_preset");
    for _ in 0..=to {
        h.key_press(Key::ArrowLeft);
    }
    step_until(&mut h, "back to the windows as typed", |a| {
        a.state
            .overlay
            .leq()
            .is_some_and(|d| d.preset.is_none() && d.rows == typed)
    });
    // egui walks its own widget focus on arrow keys too; the app's keys never need it.
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "the windows set", |a| {
        a.state.overlay == Overlay::None
            && a.state.measurements().iter().any(|m| match &m.config.kind {
                MeasKind::Spl { config } => {
                    config.leq.windows[0].limit == Some(DbSpl(102.0))
                        && config.leq.windows[3].limit == Some(DbSpl(99.0))
                }
                _ => false,
            })
    });
    assert_eq!(fake.executions("meas.update"), 1);
    assert!(
        h.state()
            .state
            .kind_modes(ac2_ui::state::PaneKind::Spl)
            .spl
            .shows_leq()
    );
    assert_eq!(h.state().state.layout.focus_kind(), PaneKind::Spl);
    assert_eq!(
        h.state().state.view.spl.layout,
        ac2_scene::view::LeqLayout::default()
    );

    // The daemon's view: calibrated 3 h ago, the 1 min and 30 min windows over.
    let cal_at = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64))
    .saturating_sub(3 * 3600 * 1_000_000_000 + 600_000_000_000);
    let full = [60.0, 300.0, 600.0, 1800.0, 2730.0];
    let over_frame = leq_frame(
        meas,
        [103.1, 99.6, 98.7, 99.4, 97.2],
        [Some(LeqFlags::OVER), None, None, Some(LeqFlags::OVER), None],
        [102.0, f32::NAN, f32::NAN, 85.3, f32::NAN],
        full,
        cal_at,
    );
    leq.set(over_frame.clone());
    let alarm = |at: u64, duration: f64, kind, leq: f64, limit: f64| LeqAlarm {
        at: WallNs(at),
        subject: ac2_proto::model::AlarmSubject::Window {
            duration: Seconds(duration),
            weighting: ac2_proto::model::Weighting::A,
        },
        kind,
        level: DbSpl(leq),
        limit: DbSpl(limit),
        position: None,
    };
    let mut log = fake.lock().state.spl_logs[0].clone();
    log.windows[0].judgement = LeqJudgement::Over;
    log.windows[3].judgement = LeqJudgement::Over;
    log.alarms = vec![
        alarm(cal_at, 60.0, LeqAlarmKind::Over, 102.3, 102.0),
        alarm(cal_at, 1800.0, LeqAlarmKind::Over, 99.1, 99.0),
    ];
    fake.lock()
        .commit(Change::SplLog(Patch::Set(SplLog { ..log.clone() })));
    step_until(&mut h, "the alarms toasted", |a| {
        a.state
            .toasts
            .iter()
            .filter(|t| t.severity == Severity::Fault && t.text.contains("over its limit"))
            .count()
            == 2
    });
    // The history the app would have gathered over the last 45 minutes, pinned so the
    // picture does not depend on when frames arrived.
    let history = {
        let st = &h.state().state;
        let m = st
            .measurements()
            .into_iter()
            .find(|m| m.id == meas)
            .cloned()
            .expect("meter");
        let MeasKind::Spl { config } = m.config.kind else {
            unreachable!()
        };
        let mut hist = ac2_scene::leq::LeqHistory::default();
        for k in 0..2700u32 {
            let t = f64::from(k);
            // A show building up: 92 dB rising to 101 at the end, a loud passage at 30 min.
            let base = 92.0 + 6.0 * (t / 2700.0) as f32;
            let peak = if (1700..2000).contains(&k) { 7.0 } else { 0.0 };
            let one = base + peak + 2.0 * ((t / 40.0).sin() as f32);
            let longer = |w: f32| base + peak * (60.0 / w).min(1.0) * 0.8;
            let vals = [
                one,
                longer(300.0),
                longer(600.0),
                longer(1800.0) + 0.6,
                base - 1.0,
            ];
            let flags = [
                if vals[0] > 102.0 {
                    LeqFlags::OVER
                } else {
                    LeqFlags::NONE
                },
                LeqFlags::NONE,
                LeqFlags::NONE,
                if vals[3] > 99.0 {
                    LeqFlags::OVER
                } else {
                    LeqFlags::NONE
                },
                LeqFlags::NONE,
            ];
            let f = leq_frame(meas, vals, flags.map(Some), [f32::NAN; 5], full, cal_at);
            hist.push(&config.leq, &f, t);
        }
        hist
    };
    // G twice: from meter + Leq (the default) past the meter alone to the windows alone.
    assert_eq!(
        h.state().state.kind_modes(ac2_ui::state::PaneKind::Spl).spl,
        ac2_scene::view::SplMode::MeterLeq
    );
    h.key_press(Key::G);
    h.key_press(Key::G);
    step_until(&mut h, "the windows alone", |a| {
        a.state.kind_modes(ac2_ui::state::PaneKind::Spl).spl == ac2_scene::view::SplMode::Leq
    });
    h.key_press(Key::W);
    step_until(&mut h, "maximised", |a| a.state.layout.maximized);
    let pin = |a: &mut ac2_ui::App| {
        a.state.toasts.clear();
        a.state.local_zone = SHOW_ZONE;
        a.state
            .leq_history
            .insert(meas, (u64::MAX, history.clone()));
    };
    let first_is = move |over: bool| {
        move |a: &ac2_ui::App| {
            a.state.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Data {
                        meas,
                        stream: Stream::Leq,
                    })
                    .is_some_and(|f| match &f.frame.data {
                        ac2_proto::FrameData::Leq(l) => (l.leq[0] > 103.0) == over,
                        _ => false,
                    })
            })
        }
    };
    snapshot_when(&mut h, "leq_columns_over", pin, first_is(true));
    // Tiles, and the history strip under them (Settings › Display, the palette).
    h.state_mut().dispatch(ac2_ui::state::Msg::Command(
        ac2_ui::keys::CommandId::SplLeqStyle,
    ));
    h.state_mut().dispatch(ac2_ui::state::Msg::Command(
        ac2_ui::keys::CommandId::SplLeqHistory,
    ));
    step_until(&mut h, "tiles with history", |a| {
        a.state.view.spl.layout
            == ac2_scene::view::LeqLayout {
                style: ac2_scene::view::LeqStyle::Tiles,
                history: true,
            }
    });
    snapshot_when(&mut h, "leq_tiles_over", pin, first_is(true));

    // Back under: the 1 min window plain again, the 30 min one near its limit.
    leq.set(leq_frame(
        meas,
        [96.4, 97.9, 98.2, 98.6, 97.0],
        [Some(LeqFlags::NONE), None, None, Some(LeqFlags::NEAR), None],
        [102.0, f32::NAN, f32::NAN, 100.4, f32::NAN],
        full,
        cal_at,
    ));
    log.windows[0].judgement = LeqJudgement::Ok;
    log.windows[3].judgement = LeqJudgement::Near;
    log.alarms
        .push(alarm(cal_at, 60.0, LeqAlarmKind::Recovered, 101.9, 102.0));
    log.alarms
        .push(alarm(cal_at, 1800.0, LeqAlarmKind::Recovered, 98.9, 99.0));
    fake.lock().commit(Change::SplLog(Patch::Set(log)));
    step_until(&mut h, "the recoveries toasted", |a| {
        a.state
            .toasts
            .iter()
            .filter(|t| t.text.contains("back within its limit"))
            .count()
            == 2
    });
    snapshot_when(&mut h, "leq_tiles_recovered", pin, first_is(false));
    // Back to columns without the strip.
    h.state_mut().dispatch(ac2_ui::state::Msg::Command(
        ac2_ui::keys::CommandId::SplLeqStyle,
    ));
    h.state_mut().dispatch(ac2_ui::state::Msg::Command(
        ac2_ui::keys::CommandId::SplLeqHistory,
    ));
    step_until(&mut h, "columns again", |a| {
        a.state.view.spl.layout == ac2_scene::view::LeqLayout::default()
    });
    snapshot_when(&mut h, "leq_columns_recovered", pin, first_is(false));

    // Shift+R asks before a new log, naming the run that ends; N keeps it.
    h.key_press_modifiers(Modifiers::SHIFT, Key::R);
    step_until(&mut h, "the new log confirmation", |a| {
        matches!(a.state.overlay, Overlay::NewLog(_))
    });
    snapshot_when(&mut h, "leq_new_log_confirm", pin, first_is(false));
    h.key_press(Key::N);
    step_until(&mut h, "no dialog", |a| a.state.overlay == Overlay::None);

    // F11 with the pane maximised: the stage view, the columns alone; over again.
    leq.set(over_frame);
    h.key_press(Key::F11);
    step_until(&mut h, "the stage view", |a| a.state.stage_view());
    snapshot_when(&mut h, "leq_columns_fullscreen", pin, first_is(true));

    // G: the meter's number over the windows, on the stage; F11 again, in the window.
    leq.set_spl(spl_frame_at(meas, 101.84, cal_at));
    h.key_press(Key::G);
    let held = move |a: &ac2_ui::App| {
        a.state.kind_modes(ac2_ui::state::PaneKind::Spl).spl == ac2_scene::view::SplMode::MeterLeq
            && a.state.spl_hold.contains_key(&meas)
            && first_is(true)(a)
    };
    snapshot_when(&mut h, "spl_meter_leq_stage", pin, held);
    h.key_press(Key::F11);
    step_until(&mut h, "maximised in the window", |a| {
        a.state.layout.maximized && !a.state.stage_view()
    });
    snapshot_when(&mut h, "spl_meter_leq", pin, held);

    // Shift+L again: ↑ from the preset row past the band meter's rows to the position
    // correction, 4 dB; Shift+Tab ×2 to the LCpeak limit, 135; the dialog with its
    // settings under the windows.
    h.key_press_modifiers(Modifiers::SHIFT, Key::L);
    step_until(&mut h, "the Leq dialog again", |a| {
        a.state.overlay.leq().is_some()
    });
    arrow_up_to(
        &mut h,
        ac2_ui::leq_dialog::Focus::Extra(ac2_ui::leq_dialog::Extra::Position),
    );
    h.event(Event::Text("4".into()));
    h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
    h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
    h.event(Event::Text("135".into()));
    step_until(&mut h, "the peak limit and the correction typed", |a| {
        a.state.overlay.leq().is_some_and(|d| {
            d.extra_text(ac2_ui::leq_dialog::Extra::LcPeak) == "135"
                && d.extra_text(ac2_ui::leq_dialog::Extra::Position) == "4"
        })
    });
    h.event(Event::PointerGone);
    snapshot_when(&mut h, "leq_dialog_peaks_position", pin, |_| true);
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "the peak limit and the correction set", |a| {
        a.state.overlay == Overlay::None
            && a.state.measurements().iter().any(|m| match &m.config.kind {
                MeasKind::Spl { config } => {
                    config.position.is_some() && config.leq.peaks.lcpeak.is_some()
                }
                _ => false,
            })
    });
    // The daemon's view: every level 4 dB up, LCpeak held over its limit.
    let mut corrected = leq_frame(
        meas,
        [103.1, 99.6, 98.7, 99.4, 97.2],
        [Some(LeqFlags::OVER), None, None, Some(LeqFlags::OVER), None],
        [102.0, f32::NAN, f32::NAN, 85.3, f32::NAN],
        full,
        cal_at,
    );
    let pos = ac2_proto::model::PositionCorrection::both(4.0);
    corrected.meta.position = Some(pos);
    corrected.meta.lcpeak = Some(ac2_proto::frame::LeqPeak {
        level: 136.4,
        judgement: LeqJudgement::Over,
    });
    leq.set(corrected);
    let mut meter = spl_frame_at(meas, 101.84, cal_at);
    meter.meta.position = Some(pos);
    leq.set_spl(meter);
    let peaked = move |a: &ac2_ui::App| {
        held(a)
            && a.state
                .spl_hold
                .get(&meas)
                .is_some_and(|x| x.frame.meta.position.is_some())
            && a.state.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Data {
                        meas,
                        stream: Stream::Leq,
                    })
                    .is_some_and(|f| {
                        matches!(&f.frame.data,
                            ac2_proto::FrameData::Leq(l) if l.meta.lcpeak.is_some())
                    })
            })
    };
    snapshot_when(&mut h, "leq_columns_peak_corrected", pin, peaked);
    drop(leq);
}

/// The meter's frame for the Leq test's meter: LAF `level`, calibrated at `calibrated_at`.
fn spl_frame_at(meas: MeasId, level: f64, calibrated_at: u64) -> ac2_proto::frame::SplFrame {
    use ac2_proto::frame::{SplFrame, SplMeta};
    use ac2_proto::model::{CalStatus, LevelScale, PeakWeighting, TimeWeighting, Weighting};
    use ac2_proto::units::{Seconds, WallNs};
    SplFrame {
        meas,
        meta: SplMeta {
            scale: LevelScale::DbSpl,
            weighting: Weighting::A,
            time_weighting: TimeWeighting::Fast,
            peak_weighting: PeakWeighting::C,
            level,
            lmax: level + 4.0,
            lmin: level - 20.0,
            leq: level - 1.5,
            lpeak: level + 15.0,
            duration: Seconds(2730.0),
            cal: CalStatus::Verified {
                calibrated_at: WallNs(calibrated_at),
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0),
                },
            },
            mic_curve: false,
            position: None,
        },
    }
}
