//! Transfer, IR and math panes.

use super::*;

#[test]
fn transfer_view_two_traces_and_banner() {
    if !have_gpu("transfer_view_two_traces_and_banner") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    // The pane draws Main L's group; Delay tower joins it compared (C), tagged `cmp`.
    rig.compare_into(
        1,
        vec![ac2_proto::model::Compared::Meas { meas: MeasId(2) }],
    );
    step_until(&mut h, "Delay tower compared", |a| {
        a.state
            .meas(MeasId(1))
            .is_some_and(|m| !m.compared.is_empty())
    });
    {
        let st = &h.state().state;
        assert_eq!(st.selected, Some(MeasId(1)));
        let s = ac2_ui::scenes::transfer(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Transfer),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1000.0,
                height: 450.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        let legend: Vec<&str> = s.legend.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(legend, ["Main L", "Delay tower"]);
        let banners: Vec<&str> = s.banners.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(banners, ["NO DELAY ESTIMATE"]);
    }
    snapshot(&mut h, "transfer_two_traces_banner");
}

/// A running transfer measurement with nothing on its reference and this app's stimulus
/// off: NO REFERENCE says which keys start it, beside the banner text.
#[test]
fn no_reference_reminds_of_the_stimulus_keys() {
    if !have_gpu("no_reference_reminds_of_the_stimulus_keys") {
        return;
    }
    let rig = common::Rig::start();
    rig.set_tf_protection(ac2_proto::frame::ProtectionFlags::NO_REFERENCE);
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    step_until(&mut h, "NO REFERENCE on the live frames", |a| {
        let s = ac2_ui::scenes::transfer(
            &a.state,
            crate::common::pane(&a.state, ac2_ui::state::PaneKind::Transfer),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1000.0,
                height: 450.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        s.banners.first().is_some_and(|b| {
            b.text == "NO REFERENCE"
                && b.detail.as_deref() == Some("stimulus off: Space arms, Enter starts it")
        })
    });
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "transfer_no_reference_reminder");
}

/// The IR pane of a stopped transfer measurement, maximised: its last IR is no longer the
/// system's, so none is drawn and the plot says S starts it, with no STALE banner. Started
/// again (S from here), the frequency panes' keys zoom its live IR.
#[test]
fn ir_pane_of_a_stopped_measurement() {
    if !have_gpu("ir_pane_of_a_stopped_measurement") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::ALT, Key::Num3);
    h.key_press(Key::W);
    h.key_press(Key::S);
    step_until(&mut h, "the IR pane alone, its measurement stopped", |a| {
        let st = &a.state;
        st.layout.maximized
            && st.layout.focused().shows_ir()
            && st.meas(MeasId(1)).is_some_and(|m| !m.running)
    });
    let scene = |h: &Harness<'_, App>| {
        let st = &h.state().state;
        ac2_ui::scenes::ir(
            st,
            crate::common::ir_pane(st),
            &h.state().keymap,
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1000.0,
                height: 450.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        )
        .expect("IR scene")
    };
    {
        let s = scene(&h);
        let note = s.note.unwrap_or_default();
        assert!(note.ends_with(" stopped — S starts it"), "{note}");
        assert_eq!(s.tag, None);
        assert!(s.banners.iter().all(|b| !b.text.starts_with("STALE")));
    }
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "ir_stopped");
    // S starts it again: its live IR. The frequency panes' keys on its time and amplitude
    // axes: I twice zooms time about the middle, Ctrl+I the amplitude, the cursor toggle
    // puts the time cursor there.
    h.key_press(Key::S);
    step_until(&mut h, "the live IR again", |a| {
        a.state.meas(MeasId(1)).is_some_and(|m| m.running)
            && a.state.ir_frame_of(ac2_scene::view::IrPane::Live).is_some()
    });
    assert_eq!(scene(&h).note, None);
    h.key_press(Key::I);
    h.key_press(Key::I);
    h.key_press_modifiers(Modifiers::COMMAND, Key::I);
    palette(&mut h, "comparison cursor");
    step_until(&mut h, "zoomed, the cursor on", |a| {
        let ax = a.state.view.ir.axes;
        ax.time_ms.is_some() && ax.amplitude.is_some() && ax.cursor_ms.is_some()
    });
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "ir_zoomed_cursor");
}

/// The top bar while recording: `REC` with the audio's length and size in the record
/// colour, with a dropout counted.
#[test]
fn recording_indicator() {
    use ac2_proto::Change;
    use ac2_proto::model::{RecordingRun, RecordingStatus};
    use ac2_proto::units::{ClientId, SampleIndex, Seconds, SessionEpoch, WallNs};
    if !have_gpu("recording_indicator") {
        return;
    }
    let rig = common::Rig::start();
    rig.fake.lock().commit(Change::Recording(RecordingRun {
        name: "rec-2026-10-05T18-00-00".into(),
        path: "/home/op/.local/share/ac2/recordings/rec-2026-10-05T18-00-00.wav".into(),
        inputs: vec![0, 1],
        sample_rate_hz: 48_000,
        session_epoch: SessionEpoch(1),
        start_sample: SampleIndex(0),
        started_at: WallNs(0),
        started_by: ClientId("ac2-ui test".into()),
        frames: 48_000 * 83,
        bytes: 116 + 48_000 * 83 * 8,
        discontinuities: 1,
        max_duration: Seconds(3600.0),
        max_bytes: None,
        status: RecordingStatus::Recording,
    }));
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames and the recording", |a| {
        live(a) && a.state.recording_label().is_some()
    });
    assert_eq!(
        h.state().state.recording_label().map(|l| l.text),
        Some("REC 1:23 · 31.9 MB · 1 dropout".to_owned())
    );
    snapshot(&mut h, "recording_indicator");
}

#[test]
fn math_average_legend_and_banner() {
    if !have_gpu("math_average_legend_and_banner") {
        return;
    }
    let rig = common::Rig::start();
    rig.add_math_average();
    let mut h = harness(options(Some(&rig)));
    let has_average = |app: &App| {
        app.state.measurements().len() == 6
            && app.state.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Data {
                        meas: MeasId(6),
                        stream: Stream::Tf,
                    })
                    .is_some()
            })
    };
    step_until(&mut h, "the average's frames", |a| {
        live_with(a, 6) && has_average(a)
    });
    // The transfer pane shows the average: its banner names the position left out.
    common::show_on(&mut h.state_mut().state, PaneKind::Transfer, MeasId(6));
    h.state_mut().state.selected = Some(MeasId(6));
    {
        let st = &h.state().state;
        let s = ac2_ui::scenes::transfer(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Transfer),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1000.0,
                height: 450.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        let legend: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
        assert!(
            legend
                .iter()
                .any(|l| l.starts_with("Audience · 2 of 3 positions · power avg")),
            "{legend:?}"
        );
        let banners: Vec<(&str, Option<&str>)> = s
            .banners
            .iter()
            .map(|b| (b.text.as_str(), b.detail.as_deref()))
            .collect();
        assert_eq!(
            banners,
            [(
                "AVERAGE · 2 OF 3 POSITIONS",
                Some("Audience: left out Seat 3: stopped")
            )]
        );
    }
    snapshot(&mut h, "transfer_math_average");
}

/// Shift+M: the math channel dialog over the rig's measurements, A ÷ B by name.
#[test]
fn math_dialog() {
    if !have_gpu("math_dialog") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::SHIFT, Key::M);
    h.event(Event::Text("M".into()));
    step_until(
        &mut h,
        "the math dialog",
        |a| matches!(&a.state.overlay, Overlay::Form(f) if f.kind == ac2_ui::forms::FormKind::Math),
    );
    {
        let Overlay::Form(f) = &h.state().state.overlay else {
            panic!("no dialog");
        };
        assert_eq!(f.text(ac2_ui::forms::FieldId::Name), "Main L ÷ Delay tower");
    }
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "math_dialog");
}

/// Spectrum math on the spectrum pane: its legend names the expression and what it means.
#[test]
fn spectrum_math() {
    if !have_gpu("spectrum_math") {
        return;
    }
    let rig = common::Rig::start();
    rig.add_spectrum_math();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "the spectrum math's frames", |a| {
        live_with(a, 6)
            && a.state.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Data {
                        meas: MeasId(7),
                        stream: Stream::Spec,
                    })
                    .is_some()
            })
    });
    h.key_press_modifiers(Modifiers::ALT, Key::Num2);
    h.key_press(Key::W);
    // The pane shows the math channel: its caption is the expression and what it means.
    common::show_on(&mut h.state_mut().state, PaneKind::Spectrum, MeasId(7));
    {
        let st = &h.state().state;
        let s = ac2_ui::scenes::spectrum(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Spectrum),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1000.0,
                height: 450.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        let legend: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
        assert!(
            legend.iter().any(|l| l.contains("Mic 1 − Mic 2")),
            "{legend:?}"
        );
        assert_eq!(s.caption, "Mic 1 FFT − Mic 2 FFT · level difference");
    }
    snapshot(&mut h, "spectrum_math");
}

/// Many curves from an empty daemon: target curves imported one after another fill the
/// transfer legend. Its rows sit on a plate in the plot's colour; at most 70 % of the pane tall,
/// the rest scroll with the wheel over it. A drag moves it, a drag on its grip makes it
/// narrower, the palette snaps it to a corner and hides it, and ui.toml keeps all that.
#[test]
fn transfer_legend_many_curves() {
    use ac2_scene::legend::{LegendCorner, LegendHover};
    if !have_gpu("transfer_legend_many_curves") {
        return;
    }
    let fake = ac2_client::fake::FakeDaemon::start(common::fake_options()).expect("fake daemon");
    let mut h = harness(options_at(Some(fake.endpoints())));
    step_until(&mut h, "synced", |a| {
        a.state.mirror.as_ref().is_some_and(|m| m.synced())
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ac2-traces/tests/fixtures/house_curve.txt");
    const CURVES: usize = 30;
    for n in 1..=CURVES {
        h.key_press(Key::Z);
        step_until(&mut h, "target prompt", |a| {
            matches!(a.state.overlay, Overlay::Prompt(_))
        });
        h.event(Event::Text(path.to_string_lossy().into_owned()));
        h.key_press(Key::Enter);
        step_until(&mut h, "one more curve", |a| {
            a.state.traces.len() == n && a.state.overlay == Overlay::None
        });
    }
    // Filed under a transfer measurement: under Imported no pane would draw them.
    common::file_traces_under_main_l(&fake);
    step_until(&mut h, "every curve under Main L", |a| {
        a.state.traces.values().all(|(d, _)| {
            d.meta.edit.owner
                == ac2_proto::model::TraceOwner::Meas {
                    meas: ac2_proto::units::MeasId(1),
                }
        }) && a.state.pane_shows_stored(a.state.layout.focus)
    });
    // The transfer pane alone.
    h.key_press(Key::W);
    step_until(&mut h, "maximised", |a| a.state.layout.maximized);
    settle(&mut h, 3);
    let legend = |h: &Harness<'_, App>| h.state().state.view.tf.legend;
    let (plate, _) = h.state().legend_rects().expect("legend drawn");
    let pane = h.state().state.layout.focus_kind();
    assert_eq!(pane, PaneKind::Transfer);

    // The wheel over it scrolls its rows (the frequency axis stays).
    let freq = h.state().state.view.freq;
    h.event(Event::PointerMoved(plate.center()));
    settle(&mut h, 3);
    assert_eq!(legend(&h).hover, Some(LegendHover::Plate));
    for _ in 0..2 {
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        settle(&mut h, 20);
    }
    step_until(&mut h, "scrolled", |a| a.state.view.tf.legend.first > 0);
    assert_eq!(h.state().state.view.freq, freq);
    h.event(Event::PointerMoved(egui::pos2(900.0, 700.0)));
    h.state_mut().state.toasts.clear();
    settle(&mut h, 3);
    assert_eq!(legend(&h).hover, None);
    dense_snapshot(&mut h, "transfer_legend_many_curves");

    // Dragged right and down: off its corner, the cursor not placed by the click.
    drag(
        &mut h,
        plate.center(),
        plate.center() + egui::vec2(400.0, 120.0),
    );
    step_until(&mut h, "moved", |a| {
        let l = a.state.view.tf.legend;
        l.x > 0.3 && l.y > 0.1
    });
    assert_eq!(h.state().state.view.cursor_hz, None);
    assert_eq!(legend(&h).corner(), None);
    // Its grip dragged left: narrower.
    let (_, grip) = h.state().legend_rects().expect("legend drawn");
    let width = legend(&h).max_width;
    drag(
        &mut h,
        grip.center(),
        grip.center() - egui::vec2(150.0, 0.0),
    );
    step_until(&mut h, "narrower", |a| {
        a.state.view.tf.legend.max_width < width
    });

    // Snapped to a corner from the palette, in the light theme.
    palette(&mut h, "legend: bottom-right");
    assert_eq!(legend(&h).corner(), Some(LegendCorner::BottomRight));
    palette(&mut h, "theme");
    step_until(&mut h, "light", |a| a.state.theme == ThemeName::Light);
    h.event(Event::PointerMoved(egui::pos2(900.0, 120.0)));
    h.state_mut().state.toasts.clear();
    settle(&mut h, 3);
    dense_snapshot(&mut h, "transfer_legend_many_curves_light");

    // Hidden, and kept so.
    palette(&mut h, "legend: hide");
    assert!(legend(&h).hidden);
    assert!(h.state().legend_rects().is_none());
    let kept = h.state().state.prefs.legend;
    assert!(kept.hidden);
    assert_eq!([kept.x, kept.y], [1.0, 1.0]);
    assert!(kept.max_width < width);
}

/// P on the transfer pane: min / max tracking marks the magnitude's peak (▲) and lists it on
/// a plate in a bottom corner, clear of the legend.
#[test]
fn transfer_min_max_tracking() {
    if !have_gpu("transfer_min_max_tracking") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press(Key::P);
    step_until(&mut h, "min / max on", |a| a.state.view.tf.extremes);
    {
        let st = &h.state().state;
        let s = ac2_ui::scenes::transfer(
            st,
            crate::common::pane(st, PaneKind::Transfer),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1000.0,
                height: 450.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        let list = s.extremes_list.expect("the list is up");
        assert!(list.starts_with("max\n1  "), "{list}");
    }
    h.state_mut().state.toasts.clear();
    snapshot(&mut h, "transfer_min_max");
}
