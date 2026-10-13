//! Band Leq and band transfer.

use super::*;

/// Publishes the given `band_leq` frame on the fake daemon every 200 ms, fresh each time.
struct BandPublisher {
    frame: Arc<std::sync::Mutex<Option<ac2_proto::frame::BandLeqFrame>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl BandPublisher {
    fn start(fake: Arc<ac2_client::fake::FakeDaemon>) -> Self {
        use ac2_proto::{Frame, FrameData};
        let frame: Arc<std::sync::Mutex<Option<ac2_proto::frame::BandLeqFrame>>> =
            Arc::new(std::sync::Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (f, s) = (Arc::clone(&frame), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            let mut seq = 1;
            while !s.load(Ordering::Acquire) {
                let data = f.lock().ok().and_then(|g| g.clone());
                if let Some(data) = data {
                    let mut st = fake.lock();
                    let frame = Frame {
                        stamp: st.stamp(seq, None),
                        data: FrameData::BandLeq(Box::new(data)),
                    };
                    st.publish(&frame);
                    seq += 1;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        Self {
            frame,
            stop,
            thread: Some(thread),
        }
    }

    fn set(&self, f: ac2_proto::frame::BandLeqFrame) {
        if let Ok(mut g) = self.frame.lock() {
            *g = Some(f);
        }
    }
}

impl Drop for BandPublisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A night hour of a band meter judged at the mic (no transfer): the LZeq hour windows full,
/// 63 Hz 3.2 dB over its limit and cooling down for 6 min 52 s, 50 Hz on course to go over;
/// the LAeq quarter windows of 50, 63 and 80 Hz half full, 63 Hz near the 20 dB typed for
/// it.
fn band_frame(meas: MeasId, calibrated_at: u64) -> ac2_proto::frame::BandLeqFrame {
    use ac2_proto::frame::{BandLeqFrame, BandLeqMeta, BandWindowState, LeqFlags};
    use ac2_proto::model::{
        BandLimitPlace, BandPeriod, CalStatus, LF_BAND_COUNT, LeqJudgement, LevelScale, Weighting,
    };
    use ac2_proto::units::{Db, DbSpl, Seconds, WallNs};
    fn flags(j: LeqJudgement, on_course: bool) -> LeqFlags {
        let judged = LeqFlags::LIMIT.with(LeqFlags::JUDGED);
        let f = match j {
            LeqJudgement::NoLimit => LeqFlags::NONE,
            LeqJudgement::NotCalibrated => LeqFlags::LIMIT,
            LeqJudgement::Ok => judged,
            LeqJudgement::Near => judged.with(LeqFlags::NEAR),
            LeqJudgement::Over => judged.with(LeqFlags::OVER),
        };
        if on_course {
            f.with(LeqFlags::ON_COURSE)
        } else {
            f
        }
    }
    // A-weighting at the bands' mid-band frequencies, dB.
    const A_DB: [f32; LF_BAND_COUNT] = [
        -50.5, -44.7, -39.4, -34.6, -30.2, -26.2, -22.5, -19.1, -16.1, -13.4, -10.9,
    ];
    let night = ac2_proto::model::BandLeqPreset::FINLAND_545_NIGHT_DB;
    // The LAeq quarter's bands: 50, 63 and 80 Hz.
    const A_BANDS: [usize; 3] = [4, 5, 6];
    let mut f = BandLeqFrame {
        meas,
        meta: BandLeqMeta {
            scale: LevelScale::DbSpl,
            cal: CalStatus::Verified {
                calibrated_at: WallNs(calibrated_at),
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: DbSpl(94.0),
                },
            },
            mic_curve: false,
            horizon: Seconds(60.0),
            correction: Db(0.0),
            limits_from: BandLimitPlace::AtMic,
            windows: (0..LF_BAND_COUNT)
                .map(|i| (i, 3600.0, Weighting::Z, 3600.0))
                .chain(A_BANDS.map(|i| (i, 900.0, Weighting::A, 450.0)))
                .map(|(i, d, weighting, e)| BandWindowState {
                    band: ac2_proto::units::Hz(ac2_proto::model::BAND_NOMINAL_HZ[i]),
                    duration: Seconds(d),
                    weighting,
                    elapsed: Seconds(e),
                    measured: Seconds(e),
                    period: BandPeriod::Night,
                    period_after_horizon: BandPeriod::Night,
                })
                .collect(),
            predicted: None,
        },
        leq: Vec::new(),
        limit: Vec::new(),
        allowed: Vec::new(),
        recover: Vec::new(),
        flags: Vec::new(),
    };
    let z: Vec<f32> = (0..LF_BAND_COUNT)
        .map(|i| {
            let limit = night[i] as f32;
            let leq = match i {
                5 => limit + 3.2,
                4 => limit - 1.4,
                0 | 1 => limit - 18.0,
                _ => limit - 7.5 + (i as f32 * 0.7).sin() * 3.0,
            };
            let j = match i {
                5 => LeqJudgement::Over,
                4 => LeqJudgement::Near,
                _ => LeqJudgement::Ok,
            };
            f.leq.push(leq);
            f.limit.push(limit);
            f.allowed.push(if i == 5 {
                f32::NAN
            } else {
                limit + (limit - leq).max(0.0) * 1.6
            });
            f.recover.push(if i == 5 { 412.0 } else { f32::NAN });
            f.flags.push(flags(j, i == 4));
            leq
        })
        .collect();
    for i in A_BANDS {
        let a = z[i] + A_DB[i] - 0.4;
        f.leq.push(a);
        if i == 5 {
            f.limit.push(20.0);
            f.allowed.push(23.1);
            f.flags.push(flags(LeqJudgement::Near, false));
        } else {
            f.limit.push(f32::NAN);
            f.allowed.push(f32::NAN);
            f.flags.push(flags(LeqJudgement::NoLimit, false));
        }
        f.recover.push(f32::NAN);
    }
    f
}

/// A session and an SPL meter from the palette, from an empty fake daemon: the meter's id.
fn spl_meter_from_an_empty_daemon(h: &mut Harness<'_, App>) -> MeasId {
    use ac2_proto::model::MeasKind;
    step_until(h, "synced", |a| {
        a.state.mirror.as_ref().is_some_and(|m| m.synced())
    });
    h.key_press_modifiers(Modifiers::SHIFT, Key::O);
    step_until(h, "the session dialog with its devices", |a| {
        session_dialog_of(a).is_some_and(|d| d.device_info().is_some())
            && a.state.input_meters().len() == 4
    });
    h.key_press(Key::Enter);
    // The mirror can show the session before the reply that offers the rig's transfer
    // measurement: a key sent in between would land on the offer.
    step_until(h, "session open, the transfer measurement offered", |a| {
        a.state.open_session().is_some() && matches!(a.state.overlay, Overlay::Offer(_))
    });
    h.key_press(Key::N);
    step_until(h, "no dialog", |a| a.state.overlay == Overlay::None);
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.event(Event::Text("new spl".into()));
    step_until(h, "palette typed", |a| {
        matches!(&a.state.overlay, Overlay::Palette(_))
    });
    h.key_press(Key::Enter);
    step_until(
        h,
        "the SPL dialog",
        |a| matches!(&a.state.overlay, Overlay::Form(f) if f.kind == ac2_ui::forms::FormKind::Spl),
    );
    h.key_press(Key::Enter);
    step_until(h, "the meter, running", |a| {
        a.state
            .measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
    });
    h.state()
        .state
        .measurements()
        .iter()
        .find(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
        .map(|m| m.id)
        .expect("meter")
}

/// From an empty fake daemon, using the app: a session, an SPL meter from the palette, Shift+L
/// and ↑ to the band meter's row under the Leq windows — →→ turns it on with the STM
/// 545/2015 low-frequency preset (a window per band 20 … 200 Hz), ↓ to its first window,
/// Shift+Insert opens the range row, → / ← make it 50 … 80 Hz LAeq 15 min, Enter adds a
/// window per band, ↓ Tab… type a 20 dB limit for the 63 Hz one, ↓ → a +5 dB impulse
/// correction — Enter sends it. The daemon (the test, through the fake) then reports 63 Hz
/// over in the hour; G goes on from meter + Leq to the bands, which draw the hour's windows
/// as one row and the quarter's as another, and name the hour's 63 Hz in the headline.
#[test]
fn slow_band_leq_from_an_empty_daemon() {
    use ac2_proto::model::{BandLeqPreset, ImpulseCorrection, MeasKind};
    use ac2_ui::leq_dialog::{BandCol, BandFocus, Focus, RangeCol};
    if !have_gpu("slow_band_leq_from_an_empty_daemon") {
        return;
    }
    let fake =
        Arc::new(ac2_client::fake::FakeDaemon::start(common::fake_options()).expect("fake daemon"));
    let _meters = Meters::start(Arc::clone(&fake));
    let bands = BandPublisher::start(Arc::clone(&fake));
    let mut h = harness(options_at(Some(fake.endpoints())));
    h.state_mut().state.local_zone = SHOW_ZONE;
    let meas = spl_meter_from_an_empty_daemon(&mut h);

    // Shift+L, ↑: from the preset row up past the wrap to the band meter's row (off: its
    // only one).
    h.key_press_modifiers(Modifiers::SHIFT, Key::L);
    step_until(&mut h, "the Leq dialog", |a| {
        a.state.overlay.leq().is_some()
    });
    h.key_press(Key::ArrowUp);
    h.key_press(Key::ArrowRight);
    h.key_press(Key::ArrowRight);
    step_until(&mut h, "the 545 low-frequency preset", |a| {
        a.state.overlay.leq().is_some_and(|d| {
            d.bands.meter == ac2_ui::leq_dialog::BandMeter::Preset(BandLeqPreset::Finland545Lf)
                && d.bands.rows.len() == 11
        })
    });
    // ↓ to the first window; Shift+Insert: the range row, 20 … 200 Hz LZeq 60 min.
    h.key_press(Key::ArrowDown);
    h.key_press_modifiers(Modifiers::SHIFT, Key::Insert);
    step_until(&mut h, "the range row", |a| {
        a.state.overlay.leq().is_some_and(|d| {
            d.bands.range.is_some() && d.focus == Focus::Band(BandFocus::Range(RangeCol::From))
        })
    });
    // From 50 Hz, to 80 Hz, LAeq 15 min.
    for _ in 0..4 {
        h.key_press(Key::ArrowRight);
    }
    h.key_press(Key::Tab);
    for _ in 0..4 {
        h.key_press(Key::ArrowLeft);
    }
    h.key_press(Key::Tab);
    h.key_press(Key::ArrowLeft);
    h.key_press(Key::ArrowLeft);
    h.key_press(Key::Tab);
    h.key_press(Key::ArrowLeft);
    h.key_press(Key::ArrowLeft);
    step_until(&mut h, "50 … 80 Hz LAeq 15 min", |a| {
        a.state.overlay.leq().is_some_and(|d| {
            d.bands.range.is_some_and(|r| {
                RangeCol::ALL.map(|c| r.cell(c)) == ["50 Hz", "80 Hz", "LAeq 15 min", "A"]
            })
        })
    });
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "leq_dialog_band_range");
    // Enter on the range row adds it: the focus on the first added window, 50 Hz.
    h.key_press(Key::Enter);
    step_until(&mut h, "three windows added", |a| {
        a.state.overlay.leq().is_some_and(|d| {
            d.bands.range.is_none()
                && d.bands.rows.len() == 14
                && d.focus
                    == Focus::Band(BandFocus::Window {
                        row: 11,
                        col: BandCol::Band,
                    })
        })
    });
    // ↓ to 63 Hz, Tab ×3 to its limit.
    h.key_press(Key::ArrowDown);
    for _ in 0..3 {
        h.key_press(Key::Tab);
    }
    h.event(Event::Text("20".into()));
    h.key_press(Key::ArrowDown);
    h.key_press(Key::ArrowDown);
    h.key_press(Key::ArrowRight);
    step_until(&mut h, "fourteen band windows, +5 dB impulse", |a| {
        a.state.overlay.leq().is_some_and(|d| {
            d.bands.rows.len() == 14
                && d.bands.rows[12].name() == "63 Hz band LAeq 15 min"
                && d.bands.rows[12].limit == "20"
                && d.bands.impulse == ImpulseCorrection::Plus5
        })
    });
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "leq_dialog_band_meter");
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "the band meter set", |a| {
        a.state.overlay == Overlay::None
            && a.state.measurements().iter().any(|m| match &m.config.kind {
                MeasKind::Spl { config } => config.bands.as_ref().is_some_and(|b| {
                    b.correction.impulse == ImpulseCorrection::Plus5
                        && b.windows.len() == 14
                        && b.windows[12].limit == Some(ac2_proto::units::DbSpl(20.0))
                        && b.windows[12].day_offset.is_none()
                }),
                _ => false,
            })
    });

    let cal_at = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64))
    .saturating_sub(3 * 3600 * 1_000_000_000);
    bands.set(band_frame(meas, cal_at));
    // G from meter + Leq (the default) on to the bands: the meter has a band meter now.
    h.key_press(Key::G);
    step_until(&mut h, "the bands", |a| {
        a.state.kind_modes(ac2_ui::state::PaneKind::Spl).spl == ac2_scene::view::SplMode::Bands
    });
    // The G hint is the same in every pane: it steps the views, the bands among them.
    let hints = h
        .state()
        .state
        .key_hint_line(
            &h.state().keymap,
            common::pane(&h.state().state, PaneKind::Spl),
            ac2_ui::keys::LabelStyle::Pc,
        )
        .expect("the SPL pane focused");
    assert_eq!(hints[0].text(), "G/Shift+G view");
    h.key_press(Key::W);
    step_until(&mut h, "maximised", |a| a.state.layout.maximized);
    let has_frame = move |a: &ac2_ui::App| {
        a.state.data.as_ref().is_some_and(|d| {
            d.latest
                .get(&Topic::Data {
                    meas,
                    stream: Stream::BandLeq,
                })
                .is_some()
        })
    };
    step_until(&mut h, "the band frame", has_frame);
    let theme = ac2_scene::Theme::dark();
    let scene = ac2_ui::scenes::band_leq(
        &h.state().state,
        crate::common::pane(&h.state().state, ac2_ui::state::PaneKind::Spl),
        &theme,
        ac2_scene::Viewport {
            width: 1200.0,
            height: 700.0,
        },
        ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        },
    )
    .expect("the band view");
    let texts: Vec<&str> = scene
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.as_str()))
        .collect();
    assert!(
        texts
            .contains(&"63 Hz band LZeq 60 min 3.2 dB over its limit · cooling down in 6 min 52 s"),
        "{texts:?}"
    );
    for want in [
        "LZeq 60 min · night limits (22–07)",
        "LAeq 15 min · so far · 7:30 / 15:00",
    ] {
        assert!(texts.contains(&want), "{want:?} not in {texts:?}");
    }
    // One row per length and weighting: the hour's eleven bars, the quarter's three.
    assert_eq!(
        scene.columns.iter().map(Vec::len).collect::<Vec<_>>(),
        [11, 3]
    );
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("transfer") || t.contains("bedroom")),
        "{texts:?}"
    );
    // The key to the two marks across the columns.
    assert!(texts.contains(&"limit"), "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("next ") && t.ends_with(": stay ≤")),
        "{texts:?}"
    );
    snapshot_when(
        &mut h,
        "band_leq_over",
        |a| {
            a.state.toasts.clear();
        },
        has_frame,
    );
    // T, the plot chrome: no grid drops the 10 dB lines and the limit text, traces only the
    // scale's numbers too; the bars, values, band labels, limit lines and the allowed-level
    // marks with their key stay where they were. T back to full.
    let chrome =
        |c| move |a: &ac2_ui::App| a.state.kind_modes(ac2_ui::state::PaneKind::Spl).chrome == c;
    for (c, name) in [
        (ac2_scene::view::PlotChrome::NoGrid, "band_leq_no_grid"),
        (ac2_scene::view::PlotChrome::Bare, "band_leq_no_scale"),
    ] {
        h.key_press(Key::T);
        step_until(&mut h, name, chrome(c));
        snapshot_when(
            &mut h,
            name,
            |a| {
                a.state.toasts.clear();
            },
            has_frame,
        );
    }
    h.key_press(Key::T);
    step_until(
        &mut h,
        "grid, scale and limit text",
        chrome(ac2_scene::view::PlotChrome::Full),
    );

    band_transfer_step(&mut h, &fake, meas);
}

/// From an empty fake daemon, with the mouse and keys: Shift+L, + on the band windows'
/// heading turns the band meter on with one window, 20 Hz LZeq 60 min; + again adds the
/// next band up, 25 Hz; ↑ Tab to the first's length, ← to 1 min, Tab Tab to its limit, 80
/// typed, Enter. The daemon (the test, through the fake) reports the windows; the band view
/// shows each as a row of one bar. Then − on a row removes it.
#[test]
fn a_single_band_window_with_plus_and_minus() {
    use ac2_proto::frame::{BandLeqFrame, BandLeqMeta, BandWindowState, LeqFlags};
    use ac2_proto::model::{
        BandLimitPlace, BandPeriod, CalStatus, LevelScale, MeasKind, Weighting,
    };
    use ac2_proto::units::{Db, DbSpl, Hz, Seconds, WallNs};
    use ac2_ui::leq_dialog::BandMeter;
    if !have_gpu("a_single_band_window_with_plus_and_minus") {
        return;
    }
    let fake =
        Arc::new(ac2_client::fake::FakeDaemon::start(common::fake_options()).expect("fake daemon"));
    let _meters = Meters::start(Arc::clone(&fake));
    let bands = BandPublisher::start(Arc::clone(&fake));
    let mut h = harness(options_at(Some(fake.endpoints())));
    h.state_mut().state.local_zone = SHOW_ZONE;
    let meas = spl_meter_from_an_empty_daemon(&mut h);
    h.key_press_modifiers(Modifiers::SHIFT, Key::L);
    step_until(&mut h, "the Leq dialog", |a| {
        a.state.overlay.leq().is_some()
    });
    let names = |d: &ac2_ui::leq_dialog::LeqDialog| -> Vec<String> {
        d.bands.rows.iter().map(|r| r.name()).collect()
    };
    // The band windows' + is the page's last.
    h.get_all_by_label("+").last().expect("the band +").click();
    step_until(&mut h, "a band window, the meter on", |a| {
        a.state.overlay.leq().is_some_and(|d| {
            d.bands.meter == BandMeter::On && names(d) == ["20 Hz band LZeq 60 min"]
        })
    });
    h.get_all_by_label("+").last().expect("the band +").click();
    step_until(&mut h, "the next band up", |a| {
        a.state
            .overlay
            .leq()
            .is_some_and(|d| names(d) == ["20 Hz band LZeq 60 min", "25 Hz band LZeq 60 min"])
    });
    // The focus is on the new row's band: ↑ to the first, Tab to its length.
    h.key_press(Key::ArrowUp);
    h.key_press(Key::Tab);
    for _ in 0..5 {
        h.key_press(Key::ArrowLeft);
    }
    h.key_press(Key::Tab);
    h.key_press(Key::Tab);
    h.event(Event::Text("80".into()));
    step_until(&mut h, "20 Hz LZeq 1 min, limit 80", |a| {
        a.state.overlay.leq().is_some_and(|d| {
            d.bands.rows[0].name() == "20 Hz band LZeq 1 min" && d.bands.rows[0].limit == "80"
        })
    });
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "leq_dialog_band_single");
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "the band windows set", |a| {
        a.state.overlay == Overlay::None
            && a.state.measurements().iter().any(|m| match &m.config.kind {
                MeasKind::Spl { config } => config.bands.as_ref().is_some_and(|b| {
                    b.windows.len() == 2
                        && b.windows[0].band == Hz(20.0)
                        && b.windows[0].duration == Seconds(60.0)
                        && b.windows[0].weighting == Weighting::Z
                        && b.windows[0].limit == Some(DbSpl(80.0))
                        && b.windows[1].band == Hz(25.0)
                        && b.windows[1].duration == Seconds(3600.0)
                        && b.windows[1].limit.is_none()
                }),
                _ => false,
            })
    });
    let judged = LeqFlags::LIMIT.with(LeqFlags::JUDGED);
    bands.set(BandLeqFrame {
        meas,
        meta: BandLeqMeta {
            scale: LevelScale::DbSpl,
            cal: CalStatus::Verified {
                calibrated_at: WallNs(0),
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: DbSpl(94.0),
                },
            },
            mic_curve: false,
            horizon: Seconds(60.0),
            correction: Db(0.0),
            limits_from: BandLimitPlace::AtMic,
            windows: vec![
                BandWindowState {
                    band: Hz(20.0),
                    duration: Seconds(60.0),
                    weighting: Weighting::Z,
                    elapsed: Seconds(60.0),
                    measured: Seconds(60.0),
                    period: BandPeriod::Day,
                    period_after_horizon: BandPeriod::Day,
                },
                BandWindowState {
                    band: Hz(25.0),
                    duration: Seconds(3600.0),
                    weighting: Weighting::Z,
                    elapsed: Seconds(60.0),
                    measured: Seconds(60.0),
                    period: BandPeriod::Day,
                    period_after_horizon: BandPeriod::Day,
                },
            ],
            predicted: None,
        },
        leq: vec![72.0, 55.0],
        limit: vec![80.0, f32::NAN],
        allowed: vec![f32::NAN, f32::NAN],
        recover: vec![f32::NAN, f32::NAN],
        flags: vec![judged, LeqFlags::NONE],
    });
    h.key_press(Key::G);
    step_until(&mut h, "the bands", |a| {
        a.state.kind_modes(ac2_ui::state::PaneKind::Spl).spl == ac2_scene::view::SplMode::Bands
    });
    step_until(&mut h, "the band frame", move |a| {
        a.state.data.as_ref().is_some_and(|d| {
            d.latest
                .get(&Topic::Data {
                    meas,
                    stream: Stream::BandLeq,
                })
                .is_some()
        })
    });
    let scene = ac2_ui::scenes::band_leq(
        &h.state().state,
        crate::common::pane(&h.state().state, ac2_ui::state::PaneKind::Spl),
        &ac2_scene::Theme::dark(),
        ac2_scene::Viewport {
            width: 1200.0,
            height: 700.0,
        },
        ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: WallNs(0),
        },
    )
    .expect("the band view");
    // Two lengths: two rows of one bar each.
    assert_eq!(
        scene.columns.iter().map(Vec::len).collect::<Vec<_>>(),
        [1, 1]
    );
    let texts: Vec<&str> = scene
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.as_str()))
        .collect();
    for want in [
        "20 Hz band LZeq 1 min 8.0 dB under its limit",
        "LZeq 1 min",
        "LZeq 60 min · so far · 1:00 / 1:00:00",
        "20",
        "25",
        "72.0",
    ] {
        assert!(texts.contains(&want), "{want:?} not in {texts:?}");
    }
    // − on each row: gone.
    h.key_press_modifiers(Modifiers::SHIFT, Key::L);
    step_until(&mut h, "the Leq dialog again", |a| {
        a.state
            .overlay
            .leq()
            .is_some_and(|d| d.bands.rows.len() == 2)
    });
    h.get_all_by_label("−").last().expect("the band −").click();
    step_until(&mut h, "one band window", |a| {
        a.state
            .overlay
            .leq()
            .is_some_and(|d| names(d) == ["20 Hz band LZeq 1 min"])
    });
    h.get_all_by_label("−").last().expect("the band −").click();
    step_until(&mut h, "no band window", |a| {
        a.state
            .overlay
            .leq()
            .is_some_and(|d| d.bands.rows.is_empty())
    });
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "the band windows gone", |a| {
        a.state.overlay == Overlay::None
            && a.state.measurements().iter().any(|m| match &m.config.kind {
                MeasKind::Spl { config } => {
                    config.bands.as_ref().is_some_and(|b| b.windows.is_empty())
                }
                _ => false,
            })
    });
}

/// The band log of `meas` from `from` on, every second at `db_spl` in every band (dBFS at
/// a 120 dB sensitivity); the seconds before `from` stay as they were.
fn log_from(
    fake: &ac2_client::fake::FakeDaemon,
    meas: MeasId,
    from: ac2_proto::units::WallNs,
    db_spl: f32,
) {
    use ac2_proto::model::{BAND_COUNT, BandPeriod};
    use ac2_proto::units::{Db, Seconds, WallNs};
    use ac2_traces::band_log::BandLogRow;
    const S: u64 = 1_000_000_000;
    let mut st = fake.lock();
    let rows = st.band_rows.entry(meas).or_default();
    rows.retain(|r| r.start < from);
    let first = from.0.div_ceil(S);
    rows.extend((first..first + 600).map(|s| BandLogRow {
        start: WallNs(s * S),
        measured: Seconds(1.0),
        levels: [db_spl - 120.0; BAND_COUNT],
        correction: Db(0.0),
        period: BandPeriod::Day,
        sensitivity: Some(Db(120.0)),
    }));
}

/// The band transfer step, on from the Leq dialog's band rows (T): ↑ to the place's name,
/// typed `flat 4`; Space starts and stops each span on the meter's clock; the FOH span
/// reads 90 dB, the place 60 dB, the background 30 dB (the log is rewritten from each span's start), each read back with
/// `spl.band_log_get`; Enter stores a 30 dB clean transfer in the meter.
fn band_transfer_step(h: &mut Harness<'_, App>, fake: &ac2_client::fake::FakeDaemon, meas: MeasId) {
    use ac2_proto::model::{BandTransferBand, MeasKind};
    use ac2_scene::band_transfer::{SpanRole, SpanState};
    use ac2_ui::leq_dialog::TransferStep;
    fn step_of(a: &App) -> Option<&TransferStep> {
        a.state.overlay.leq().and_then(|d| d.transfer.as_ref())
    }
    h.key_press(Key::W);
    h.key_press_modifiers(Modifiers::SHIFT, Key::L);
    step_until(h, "the Leq dialog", |a| a.state.overlay.leq().is_some());
    h.key_press(Key::ArrowUp);
    h.key_press(Key::T);
    step_until(h, "the band transfer step", |a| {
        step_of(a).is_some_and(|t| t.focus == SpanRole::Foh)
    });
    h.key_press(Key::ArrowUp);
    for _ in 0.."receiving room".len() {
        h.key_press(Key::Backspace);
    }
    h.event(Event::Text("flat 4".into()));
    h.key_press(Key::ArrowDown);
    step_until(h, "the place named", |a| {
        step_of(a).is_some_and(|t| t.place == "flat 4" && !t.on_place)
    });
    let next = step_of(h.state())
        .map(TransferStep::next_step)
        .unwrap_or_default();
    assert!(
        next.starts_with("Play a steady test signal (pink noise)"),
        "{next}"
    );

    let levels = [90.0, 60.0, 30.0];
    for (role, db) in SpanRole::ALL.into_iter().zip(levels) {
        h.key_press(Key::Space);
        step_until(h, &role.title("flat 4"), |a| {
            step_of(a).is_some_and(|t| matches!(t.span(role).state, SpanState::Marking { .. }))
        });
        let Some(SpanState::Marking { from }) = step_of(h.state()).map(|t| t.span(role).state)
        else {
            panic!("marking");
        };
        log_from(fake, meas, from, db);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(2200) {
            h.step();
            std::thread::sleep(Duration::from_millis(20));
        }
        h.key_press(Key::Space);
        step_until(h, "the span read back", |a| {
            step_of(a).is_some_and(|t| t.span(role).average.is_some())
        });
        let t = step_of(h.state()).expect("step");
        let a = t.span(role).average.as_ref().expect("average");
        assert!(a.seconds >= 1, "{a:?}");
        let l = a.levels.expect("dB SPL")[5].expect("63 Hz").0;
        assert!((l - f64::from(db)).abs() < 1e-3, "{role:?}: {l}");
    }
    let next = step_of(h.state())
        .map(TransferStep::next_step)
        .unwrap_or_default();
    assert!(
        next.starts_with("Enter computes the band transfer and stores it in "),
        "{next}"
    );

    h.key_press(Key::Enter);
    step_until(h, "the transfer stored", |a| {
        step_of(a).is_some_and(|t| t.stored.is_some())
    });
    let t = step_of(h.state()).expect("step");
    assert!(
        t.next_step().starts_with("Band transfer stored in "),
        "{}",
        t.next_step()
    );
    let lines = t.result_lines();
    assert_eq!(lines[0], "transfer from flat 4, 20–200 Hz: 11 clean");
    assert_eq!(lines[6], "63 Hz 30.0 dB");
    let stored = h
        .state()
        .state
        .measurements()
        .iter()
        .find(|m| m.id == meas)
        .and_then(|m| match &m.config.kind {
            MeasKind::Spl { config } => config.bands.as_ref().and_then(|b| b.transfer.clone()),
            _ => None,
        });
    assert_eq!(stored.as_ref().map(|s| s.place.as_str()), Some("flat 4"));
    assert!(
        matches!(stored.map(|s| s.bands[5]), Some(BandTransferBand::Clean { attenuation }) if (attenuation.0 - 30.0).abs() < 1e-6),
        "the transfer stored"
    );
    // The picture at fixed times: 21:00–21:02 at FOH, 21:04–21:06 in flat 4, 21:07–
    // 21:08 silent (the show's zone), each second logged.
    h.event(Event::PointerGone);
    h.state_mut().state.local_zone = SHOW_ZONE;
    snapshot_when(
        h,
        "band_transfer_step",
        |a| {
            a.state.toasts.clear();
            let base = 20_734 * 86_400 + 19 * 3600;
            let spans = [(0, 120), (240, 360), (420, 480)];
            if let Some(t) = a.state.overlay.leq_mut().and_then(|d| d.transfer.as_mut()) {
                for (span, (from, until)) in t.spans.iter_mut().zip(spans) {
                    span.state = SpanState::Marked {
                        from: ac2_proto::units::WallNs((base + from) * 1_000_000_000),
                        until: ac2_proto::units::WallNs((base + until) * 1_000_000_000),
                    };
                    if let Some(a) = &mut span.average {
                        a.seconds = (until - from) as u32;
                        a.measured = ac2_proto::units::Seconds((until - from) as f64);
                    }
                }
            }
        },
        |_| true,
    );
}
