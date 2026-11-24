//! Transfer, IR and math panes.

use super::*;

#[test]
fn transfer_view_two_traces_and_banner() {
    if !have_gpu("transfer_view_two_traces_and_banner") {
        return;
    }
    let _rig = Rig::start();
    let mut h = harness(options());
    step_until(&mut h, "live frames", live);
    // The pane draws Main L's group; Delay tower joins it compared (C), tagged `cmp`.
    crate::common::draw_in(
        &mut h.state_mut().state,
        ac2_ui::state::PaneKind::Transfer,
        &[ac2_proto::model::layout::EntryRef::Group { meas: MeasId(2) }],
    );
    {
        let st = &h.state().state;
        assert_eq!(st.rig.selected, Some(MeasId(1)));
        let s = ac2_ui::scenes::transfer(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Transfer),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport::new(1000.0, 450.0),
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
    let rig = Rig::start();
    rig.set_tf_protection(ac2_proto::frame::ProtectionFlags::NO_REFERENCE);
    let mut h = harness(options());
    step_until(&mut h, "live frames", live);
    step_until(&mut h, "NO REFERENCE on the live frames", |a| {
        let s = ac2_ui::scenes::transfer(
            &a.state,
            crate::common::pane(&a.state, ac2_ui::state::PaneKind::Transfer),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport::new(1000.0, 450.0),
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
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "transfer_no_reference_reminder");
}

/// The alignment view of "Delay tower ↔ Main L": the readout says what to do, the phase
/// difference and the summation before (dim) and after (bright), the band shaded; the
/// frequency cursor, a line through both plots, reads the four curves on its plate.
#[test]
fn alignment_view() {
    if !have_gpu("alignment_view") {
        return;
    }
    let rig = Rig::scene("alignment");
    let math = rig.id("Delay tower ↔ Main L");
    let mut h = harness(options());
    step_until(&mut h, "the alignment's frames", |a| {
        live_with(a, 5)
            && a.state.rig.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Data {
                        meas: math,
                        stream: Stream::Tf,
                    })
                    .is_some()
            })
    });
    {
        let st = &mut h.state_mut().state;
        common::show_on(st, PaneKind::Transfer, math);
        let p = common::pane(st, PaneKind::Transfer);
        if let Some(v) = st.layout.view_mut(p) {
            v.modes.transfer = ac2_ui::state::TransferView::Alignment;
        }
    }
    h.state_mut()
        .dispatch(ac2_ui::state::Msg::CursorAt(Some(100.0)));
    {
        let st = &h.state().state;
        let s = ac2_ui::scenes::alignment(
            st,
            crate::common::pane(st, PaneKind::Transfer),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport::new(1000.0, 600.0),
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        let r = s.readout.expect("the readout");
        assert_eq!(
            r.headline,
            "Delay tower: delay 4.21 ms · 1.44 m @ 20 °C · polarity normal"
        );
        let cur = s.cursor.expect("the cursor's reading");
        assert!(cur.measured.starts_with("as measured  "), "{cur:?}");
        assert!(cur.aligned.starts_with("aligned  "), "{cur:?}");
        assert_eq!(
            s.readout_box.map(|b| b.kind),
            Some(ac2_scene::plate::PlateKind::Readout)
        );
        assert_eq!(
            r.lines[1],
            "other polarity: Delay tower +9.80 ms inverted, ±41°"
        );
        assert_eq!(
            s.titles,
            [
                "phase difference Delay tower − Main L (°)",
                "summation (dB, 0 = they add fully)"
            ]
        );
    }
    snapshot(&mut h, "transfer_alignment");
}

#[test]
fn math_average_legend_and_banner() {
    if !have_gpu("math_average_legend_and_banner") {
        return;
    }
    let rig = Rig::scene("math_average");
    let math = rig.id("Audience");
    let mut h = harness(options());
    let has_average = |app: &App| {
        app.state.measurements().len() == 6
            && app.state.rig.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Data {
                        meas: math,
                        stream: Stream::Tf,
                    })
                    .is_some()
            })
    };
    step_until(&mut h, "the average's frames", |a| {
        live_with(a, 6) && has_average(a)
    });
    // The transfer pane shows the average: its banner names the position left out.
    common::show_on(&mut h.state_mut().state, PaneKind::Transfer, math);
    h.state_mut().state.rig.selected = Some(math);
    {
        let st = &h.state().state;
        let s = ac2_ui::scenes::transfer(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Transfer),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport::new(1000.0, 450.0),
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

/// Shift+M: the math trace dialog over the rig's measurements, A ÷ B by name.
#[test]
fn math_dialog() {
    if !have_gpu("math_dialog") {
        return;
    }
    let _rig = Rig::start();
    let mut h = harness(options());
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
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "math_dialog");
}

/// Spectrum math on the spectrum pane: its legend names the expression and what it means.
#[test]
fn spectrum_math() {
    if !have_gpu("spectrum_math") {
        return;
    }
    let rig = Rig::scene("spectrum_math");
    let math = rig.id("Mic 1 − Mic 2");
    let mut h = harness(options());
    step_until(&mut h, "the spectrum math's frames", |a| {
        live_with(a, 6)
            && a.state.rig.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Data {
                        meas: math,
                        stream: Stream::Spec,
                    })
                    .is_some()
            })
    });
    h.key_press_modifiers(Modifiers::ALT, Key::Num2);
    h.key_press(Key::W);
    // The pane shows the math trace: its caption is the expression and what it means.
    common::show_on(&mut h.state_mut().state, PaneKind::Spectrum, math);
    // The level axis follows the difference (±6 dB) only once a frame of it has been drawn on
    // the pane; until then it stands on the dBFS default, above which the curve is off screen.
    step_until(&mut h, "the level axis fitted to the math curve", |a| {
        let st = &a.state;
        st.level_range(crate::common::pane(st, PaneKind::Spectrum))
            .is_some_and(|r| r.hi > 0.0 && r.lo < 0.0)
    });
    {
        let st = &h.state().state;
        let s = ac2_ui::scenes::spectrum(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Spectrum),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport::new(1000.0, 450.0),
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
    // The pane frames a measurement newly shown on it when that one's next frame arrives
    // (the frame already in came before the pick), and the curve eases to it: the picture
    // waits for both. The difference runs −6 … 6 dB; Mic 1 FFT's range sits far below.
    snapshot_when(
        &mut h,
        "spectrum_math",
        |_| {},
        |a| {
            let st = &a.state;
            let pane = common::pane(st, PaneKind::Spectrum);
            st.level_range(pane)
                .is_some_and(|r| r.lo <= -6.0 && r.hi >= 6.0 && r.hi - r.lo <= 30.0)
                && !st.levels_following()
                && !st.spectrum_gliding(Instant::now())
        },
    );
}

/// A transfer measurement whose peak mode tracks: the magnitude's peak is marked (▲) and
/// listed on the peak list's plate in a bottom corner, clear of the legend.
#[test]
fn transfer_peak_tracking() {
    if !have_gpu("transfer_peak_tracking") {
        return;
    }
    let rig = Rig::start();
    rig.set_peak("Main L", ac2_proto::model::PeakMode::Track);
    let mut h = harness(options());
    step_until(&mut h, "live frames", live);
    {
        let st = &h.state().state;
        let s = ac2_ui::scenes::transfer(
            st,
            crate::common::pane(st, PaneKind::Transfer),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport::new(1000.0, 450.0),
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        let list = s.extremes_list.expect("the list is up");
        assert!(list.starts_with("max\n1  "), "{list}");
    }
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "transfer_peak_tracking");
}

/// Main L at `bins`, its pane maximised and zoomed to 10–16 kHz: the top window's bins are
/// more than a pixel apart there, so the curve is drawn through them (a comb the 1/96 grid
/// averages away) and the cursor, on a bin, reads it with its width.
#[test]
fn transfer_bins_zoomed() {
    if !have_gpu("transfer_bins_zoomed") {
        return;
    }
    let rig = Rig::start();
    rig.set_bins("Main L");
    let main = rig.id("Main L");
    let mut h = harness(options());
    step_until(&mut h, "live frames", live);
    {
        let st = &mut h.state_mut().state;
        let p = common::pane(st, PaneKind::Transfer);
        st.layout.set_focus(p);
        st.view.freq = ac2_scene::view::FreqRange {
            lo: 10_000.0,
            hi: 16_000.0,
        };
        st.nav = ac2_ui::anim::FreqNav::new(st.view.freq);
    }
    h.key_press(Key::W);
    // Bin 1024 of the 4096-point window is at 12 kHz exactly.
    h.state_mut()
        .dispatch(ac2_ui::state::Msg::CursorAt(Some(12_000.0)));
    step_until(&mut h, "stage 0's bins", |a| {
        a.state.layout.maximized
            && a.state.rig.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Bins {
                        meas: main,
                        stage: 0,
                    })
                    .is_some()
            })
    });
    {
        let st = &h.state().state;
        let p = common::pane(st, PaneKind::Transfer);
        let px = st.plot_px.get(&p).copied().expect("the pane was drawn");
        let s = ac2_ui::scenes::transfer(
            st,
            p,
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport::new(px + 200.0, 600.0),
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        let drawn = s.bins.as_ref().expect("stage 0 drawn");
        assert!(drawn.spans[0].iter().any(|sp| sp.stage == 0), "{drawn:?}");
        let cur = s.cursor.expect("the cursor's reading");
        assert!(cur.freq.ends_with(" · 12 Hz bin"), "{}", cur.freq);
        assert!(s.legend[0].text.contains(" · bins"), "{}", s.legend[0].text);
    }
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "transfer_bins_zoomed");
}
