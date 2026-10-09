//! Reducer tests of the display edits: offset steps, the level axis, hiding and deleting
//! the selected measurement or stored trace, and which pane a selection brings up.

use super::*;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;

fn spec_meta(id: u32) -> TraceMeta {
    TraceMeta {
        kind: TraceKind::Spectrum {
            scale: LevelScale::Dbfs,
        },
        ..stored(id, None, 2)
    }
}

fn with_offset(mut t: TraceMeta, db: f64) -> TraceMeta {
    t.edit.offset = Db(db);
    t
}

fn data(meta: &TraceMeta, mag: Vec<f32>) -> ConnEvent {
    let grid = GridDef::Log {
        ppo: 1,
        k_min: -5,
        k_max: -5 + mag.len() as i32 - 1,
    };
    ConnEvent::Trace(
        Arc::new(TraceData {
            meta: meta.clone(),
            mag_db: mag,
            phase_deg: None,
            coherence: None,
            sweep: None,
        }),
        Arc::new(grid),
    )
}

fn now() -> crate::scenes::Now {
    crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    }
}

const SIZE: Viewport = Viewport {
    width: 900.0,
    height: 500.0,
};

#[test]
fn offset_steps_change_the_selected_trace() {
    let mut t = T::new();
    let mut locked = stored(12, None, 2);
    locked.edit.locked = true;
    t.conn(with_traces(vec![
        stored(13, Some(2), 2),
        locked,
        spec_meta(16),
    ]));
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    let (id, edit, what) = trace_update(&t.key("Alt+Shift+Up"));
    assert_eq!((id, edit.offset), (TraceId(13), Db(3.0)));
    assert_eq!(what, "slot 2 (t13): offset +3.0 dB");
    // Steps start from what the daemon holds.
    t.conn(with_traces(vec![with_offset(stored(13, Some(2), 2), 3.0)]));
    let (_, edit, what) = trace_update(&t.key("Alt+Down"));
    assert_eq!(edit.offset, Db(2.0));
    assert_eq!(what, "slot 2 (t13): offset +2.0 dB");
    let (_, edit, _) = trace_update(&t.key("Alt+Shift+Down"));
    assert_eq!(edit.offset, Db(0.0));
    let (_, edit, _) = trace_update(&t.key("Alt+Up"));
    assert_eq!(edit.offset, Db(4.0));
    // Alt+Home: no offset.
    let (_, edit, what) = trace_update(&t.key("Alt+Home"));
    assert_eq!(edit.offset, Db(0.0));
    assert_eq!(what, "slot 2 (t13): no offset");
    assert_eq!(t.st.edit(MeasId(1)), LiveEdit::default(), "live untouched");

    // Tenths stay tenths however many steps.
    t.conn(with_traces(vec![
        with_offset(stored(13, Some(2), 2), 0.3),
        spec_meta(16),
        {
            let mut l = stored(12, None, 2);
            l.edit.locked = true;
            l
        },
    ]));
    let (_, edit, _) = trace_update(&t.key("Alt+Down"));
    assert_eq!(edit.offset, Db(-0.7));

    // Any kind of trace, from any pane: a spectrum capture with the transfer pane focused.
    t.st.update(Msg::SelectTrace(TraceId(16)), &t.keys);
    let (id, edit, _) = trace_update(&t.key("Alt+Up"));
    assert_eq!((id, edit.offset), (TraceId(16), Db(1.0)));
    // J types it, in the spectrum pane too.
    t.key("Alt+2");
    assert_eq!(t.st.selected_trace, Some(TraceId(16)), "drawn there: kept");
    t.type_key("J", "j");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceOffset(TraceId(16))
    ));
    t.key("Esc");
    // A locked trace says so and sends nothing.
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert!(t.key("Alt+Up").is_empty());
    assert!(
        t.last_toast().contains("t12 is locked"),
        "{}",
        t.last_toast()
    );
}

#[test]
fn offset_steps_change_the_pane_measurement_without_a_trace() {
    let mut t = T::new();
    // The transfer pane: its measurement, display only (nothing is sent).
    assert!(t.key("Alt+Up").is_empty());
    assert_eq!(t.st.edit(MeasId(1)).offset_db, 1.0);
    assert_eq!(t.last_toast(), "Main L: offset +1.0 dB (display)");
    t.key("Alt+Shift+Up");
    assert_eq!(t.st.edit(MeasId(1)).offset_db, 4.0);
    // The spectrum pane: the spectrum.
    t.key("Alt+2");
    assert!(t.key("Alt+Shift+Down").is_empty());
    assert_eq!(t.st.edit(MeasId(2)).offset_db, -3.0);
    assert_eq!(t.last_toast(), "Sub: offset −3.0 dB (display)");
    assert_eq!(t.st.edit(MeasId(1)).offset_db, 4.0);
    t.key("Alt+Home");
    assert_eq!(t.st.edit(MeasId(2)).offset_db, 0.0);
    assert_eq!(t.last_toast(), "Sub: no offset (display)");
    // J types it there too.
    t.type_key("J", "j");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::Offset(MeasId(2))
    ));
    t.text("-2.5");
    assert!(t.key("Enter").is_empty());
    assert_eq!(t.st.edit(MeasId(2)).offset_db, -2.5);
    assert_eq!(t.last_toast(), "Sub: offset −2.5 dB (display)");
}

/// The offset is written next to the curve, so a spread trace is never read as a level
/// difference: the transfer legend's tag, the spectrum legend's.
#[test]
fn offsets_are_named_where_the_curves_are_drawn() {
    let mut t = T::new();
    let tf = with_offset(captured(13, Some(2), 2), 3.0);
    let sp = with_offset(spec_meta(16), -6.0);
    t.conn(with_traces(vec![tf.clone(), sp.clone()]));
    t.conn(data(&tf, vec![-10.0; 8]));
    t.conn(data(&sp, vec![-90.0; 8]));
    let theme = Theme::dark();
    let s = crate::scenes::transfer(&t.st, &theme, SIZE, now());
    let e = s
        .legend
        .iter()
        .find(|e| e.name == "t13")
        .expect("legend entry");
    assert!(e.tags.contains(&"+3.0 dB".to_owned()), "{e:?}");
    let d = s.traces.iter().find(|d| d.name == "t13").expect("trace");
    assert!(d.magnitude_db.iter().all(|m| *m == -7.0), "the curve moves");
    let s = crate::scenes::spectrum(&t.st, &theme, SIZE, now());
    let texts: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(texts, ["t16 · offset −6.0 dB"]);
}

/// The selected stored trace is marked in the legend of the pane that draws it (the title
/// names it too); V moving the selection moves the mark.
#[test]
fn the_selected_trace_is_marked_in_its_legend() {
    let mut t = T::new();
    let a = captured(13, Some(1), 2);
    let b = captured(14, Some(2), 2);
    let sp = spec_meta(16);
    t.conn(with_traces(vec![a.clone(), b.clone(), sp.clone()]));
    t.conn(data(&a, vec![-10.0; 8]));
    t.conn(data(&b, vec![-12.0; 8]));
    t.conn(data(&sp, vec![-90.0; 8]));
    let theme = Theme::dark();
    let marked = |t: &T| {
        let tf = crate::scenes::transfer(&t.st, &theme, SIZE, now());
        let spec = crate::scenes::spectrum(&t.st, &theme, SIZE, now());
        tf.legend
            .iter()
            .chain(&spec.legend)
            .filter(|e| e.selected)
            .map(|e| e.name.clone())
            .collect::<Vec<_>>()
    };
    assert!(marked(&t).is_empty());
    t.key("V");
    assert_eq!(marked(&t), ["t13"]);
    t.key("V");
    assert_eq!(marked(&t), ["t14"]);
    t.key("V");
    assert_eq!(marked(&t), ["t16"]);
}

fn spec_frame(t: &mut T, meas: u32, level: Vec<f32>) {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::Frame;
    use ac2_proto::FrameData;
    use ac2_proto::frame::{SpecFrame, SpecMeta};
    let grid = GridDef::Linear {
        fs: Hz(48_000.0),
        n: (level.len() as u32 - 1) * 2,
    };
    let data = FrameData::Spec(SpecFrame {
        meas: MeasId(meas),
        meta: SpecMeta {
            window: Window::Hann,
            scale: LevelScale::Dbfs,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            smoothing: None,
            math: None,
        },
        level,
    });
    let stamp = ac2_proto::samples::stamp(Some(grid.clone()));
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
    let mut grids = std::collections::BTreeMap::new();
    grids.insert(grid.id(), Arc::new(grid));
    t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    })));
}

#[test]
fn level_axis_zooms_pans_fits_and_resets_per_pane() {
    let mut t = T::new();
    let d = ViewState::default();
    let r = |t: &T, p| {
        level_range(&t.st.view, p, crate::scenes::spectrum_scale(&t.st)).expect("level axis")
    };
    // Transfer: Ctrl+I zooms in about the middle, Ctrl+↑ pans by a round step.
    t.key("Ctrl+I");
    assert_eq!(r(&t, PaneKind::Transfer), Range::new(-20.0, 20.0));
    t.key("Ctrl+Up");
    assert_eq!(r(&t, PaneKind::Transfer), Range::new(-15.0, 25.0));
    t.key("Ctrl+O");
    assert!((r(&t, PaneKind::Transfer).span() - 60.0).abs() < 1e-9);
    // ↑ alone is still the stimulus level: no axis change.
    let before = r(&t, PaneKind::Transfer);
    t.key("Up");
    assert_eq!(r(&t, PaneKind::Transfer), before);
    // Each pane keeps its own range.
    assert_eq!(r(&t, PaneKind::Spectrum), d.spectrum.level);
    t.key("Alt+2");
    t.key("Ctrl+Down");
    t.key("Ctrl+Down");
    t.key("Ctrl+Down");
    assert_eq!(r(&t, PaneKind::Spectrum), Range::new(-130.0, -30.0));
    assert_eq!(r(&t, PaneKind::Transfer), before);
    // The mouse: Ctrl+wheel about the pointer, Shift+wheel pans.
    t.st.update(
        Msg::LevelZoom {
            pane: PaneKind::Spectrum,
            about_db: Some(-80.0),
            factor: 2.0,
        },
        &t.keys,
    );
    assert_eq!(r(&t, PaneKind::Spectrum), Range::new(-105.0, -55.0));
    t.st.update(
        Msg::LevelPan {
            pane: PaneKind::Spectrum,
            db: 5.0,
        },
        &t.keys,
    );
    assert_eq!(r(&t, PaneKind::Spectrum), Range::new(-100.0, -50.0));
    // Ctrl+Home: the pane's default again.
    t.key("Ctrl+Home");
    assert_eq!(r(&t, PaneKind::Spectrum), d.spectrum.level);
    t.key("Alt+1");
    t.key("Ctrl+Home");
    assert_eq!(r(&t, PaneKind::Transfer), d.tf.magnitude_db);
    // The sweep pane's distortion axis.
    t.key("Alt+5");
    t.key("Ctrl+I");
    assert!((r(&t, PaneKind::Distortion).span() - 100.0 / 1.5).abs() < 1e-9);
    // The IR pane without an IR: said, nothing changes.
    t.key("Alt+3");
    let view = t.st.view;
    t.key("Ctrl+I");
    assert_eq!(t.st.view, view);
    assert!(
        t.last_toast().contains("no impulse response shown"),
        "{}",
        t.last_toast()
    );
}

/// Very low signals: Shift+Home frames what the spectrum pane shows, live and stored, as
/// drawn (offsets included), in the shown frequency range.
#[test]
fn fit_frames_low_signals_in_the_spectrum() {
    let mut t = T::new();
    t.key("Alt+2");
    // Nothing shown yet.
    t.key("Shift+Home");
    assert!(
        t.last_toast().contains("no curve shown"),
        "{}",
        t.last_toast()
    );
    assert_eq!(
        t.st.view.spectrum.level,
        ViewState::default().spectrum.level
    );
    // Bins at 0, 6, 12, 18 and 24 kHz: DC and 24 kHz lie outside 20 Hz – 20 kHz.
    spec_frame(&mut t, 2, vec![0.0, -138.0, -120.0, -95.0, 0.0]);
    t.key("Shift+Home");
    let r = t.st.view.spectrum.level;
    assert!(r.lo <= -138.0 && r.lo >= -150.0, "{r:?}");
    assert!(r.hi >= -95.0 && r.hi <= -85.0, "{r:?}");
    assert!(
        t.last_toast().starts_with("Spectrum / RTA: level "),
        "{}",
        t.last_toast()
    );
    // A stored trace with an offset counts where it is drawn.
    let sp = with_offset(spec_meta(16), 30.0);
    t.conn(with_traces(vec![sp.clone()]));
    t.conn(data(&sp, vec![-100.0; 8]));
    t.key("Shift+Home");
    let r = t.st.view.spectrum.level;
    assert!(r.hi >= -70.0 && r.hi <= -60.0, "{r:?}");
    // The transfer pane frames its own curves.
    t.key("Alt+1");
    t.key("Shift+Home");
    assert!(
        t.last_toast().contains("no curve shown"),
        "{}",
        t.last_toast()
    );
}

fn deleted(r: &[Request]) -> Option<(TraceId, String)> {
    match r {
        [
            Request::Call {
                cmd: Command::TraceDelete { trace },
                what,
            },
        ] => Some((*trace, what.clone())),
        _ => None,
    }
}

#[test]
fn delete_asks_then_deletes_the_selected_trace() {
    let mut t = T::new();
    let mut hidden = stored(12, None, 2);
    hidden.edit.visible = false;
    let mut locked = stored(15, None, 2);
    locked.edit.locked = true;
    t.conn(with_traces(vec![
        stored(10, Some(1), 2),
        stored(11, None, 2),
        hidden,
        stored(14, None, 2),
        locked,
    ]));
    // No trace selected: the measurement selected is what Delete is about.
    assert!(t.key("Delete").is_empty());
    assert!(
        matches!(&t.st.overlay, Overlay::Delete(p) if p.target == DeleteTarget::Meas(MeasId(1))),
        "{:?}",
        t.st.overlay
    );
    t.key("Esc");
    // Asks first, naming it.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    assert!(t.key("Delete").is_empty());
    let Overlay::Delete(p) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(p.target, DeleteTarget::Trace(TraceId(10)));
    assert_eq!(p.confirm.title, "Delete t10?");
    assert_eq!(p.confirm.lines[0], "capture · slot 1 · no data yet");
    assert_eq!(
        p.confirm.hint,
        "Delete, Backspace or Enter deletes it · Esc or N keeps it"
    );
    // Esc keeps it (the stimulus untouched); so does N.
    assert!(t.key("Esc").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(
        t.st.selected_trace,
        Some(TraceId(10)),
        "Esc closed the question only"
    );
    t.key("Delete");
    assert!(t.key("N").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    // Other keys wait for the answer.
    t.key("Delete");
    assert!(t.key("A").is_empty());
    assert!(matches!(t.st.overlay, Overlay::Delete(_)));
    // Delete again deletes; the selection moves to the next shown trace.
    let (id, what) = deleted(&t.key("Delete")).expect("deleted");
    assert_eq!(id, TraceId(10));
    assert_eq!(what, "slot 1 (t10) deleted · t11 selected");
    assert_eq!(t.st.selected_trace, Some(TraceId(11)));
    assert_eq!(t.st.overlay, Overlay::None);
    // Enter confirms too; past a hidden trace to the next shown one.
    t.key("Delete");
    let (id, what) = deleted(&t.key("Enter")).expect("deleted");
    assert_eq!(
        (id, what.as_str()),
        (TraceId(11), "t11 deleted · t14 selected")
    );
    // The last shown one: the selection goes back to the one before it.
    t.conn(with_traces(vec![stored(11, None, 2), stored(14, None, 2)]));
    assert_eq!(t.st.selected_trace, Some(TraceId(14)));
    t.key("Delete");
    let (_, what) = deleted(&t.st.update(Msg::Delete(true), &t.keys)).expect("deleted");
    assert_eq!(what, "t14 deleted · t11 selected");
    // The only one, with Backspace twice (keyboards without Delete): back to the live
    // measurement.
    t.conn(with_traces(vec![stored(11, None, 2)]));
    t.key("Backspace");
    let (_, what) = deleted(&t.st.update(Msg::Backspace, &t.keys)).expect("deleted");
    assert_eq!(what, "t11 deleted · keys act on the live measurement");
    assert_eq!(t.st.selected_trace, None);
    // The mouse's Keep.
    t.conn(with_traces(vec![stored(11, None, 2)]));
    t.st.update(Msg::SelectTrace(TraceId(11)), &t.keys);
    t.key("Delete");
    assert!(t.st.update(Msg::Delete(false), &t.keys).is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    // A locked trace refuses before asking.
    let mut locked = stored(15, None, 2);
    locked.edit.locked = true;
    t.conn(with_traces(vec![locked]));
    t.st.update(Msg::SelectTrace(TraceId(15)), &t.keys);
    assert!(t.key("Delete").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.last_toast(), "t15 is locked: it is not deleted");
    // The palette's entry asks the same.
    t.conn(with_traces(vec![stored(11, None, 2)]));
    t.st.update(Msg::SelectTrace(TraceId(11)), &t.keys);
    t.key("Ctrl+K");
    t.text("delete selected measurement or trace");
    t.key("Enter");
    assert!(
        matches!(&t.st.overlay, Overlay::Delete(p) if p.target == DeleteTarget::Trace(TraceId(11)))
    );
}

/// Picking a measurement brings up the pane that draws it: in the maximised layout the one
/// pane switches (and stays maximised), in the split layout the focus moves there.
#[test]
fn picking_a_measurement_brings_up_its_pane() {
    for maximised in [true, false] {
        let mut t = T::new();
        let mut s = four();
        s.measurements.push(meas(5, "FOH SPL", spl_meter()));
        t.conn(mirror(s));
        if maximised {
            t.key("W");
        }
        assert_eq!(t.st.layout.maximized, maximised);
        let shows = |t: &T, p: PaneKind, id: u32| {
            assert_eq!(t.st.layout.focus, p, "maximised {maximised}");
            assert_eq!(t.st.pane_meas(p).map(|m| m.id), Some(MeasId(id)));
            assert_eq!(t.st.layout.maximized, maximised);
            if maximised {
                assert_eq!(t.st.layout.visible(), [p]);
            }
        };
        // An RTA: the spectrum / RTA pane.
        t.st.update(Msg::SelectMeas(MeasId(4)), &t.keys);
        shows(&t, PaneKind::Spectrum, 4);
        // A narrowband spectrum stays there, and the pane shows it.
        t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
        shows(&t, PaneKind::Spectrum, 2);
        // A transfer measurement: the transfer pane.
        t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
        shows(&t, PaneKind::Transfer, 3);
        // An SPL meter: the SPL pane.
        t.st.update(Msg::SelectMeas(MeasId(5)), &t.keys);
        shows(&t, PaneKind::Spl, 5);
        // The IR pane draws transfer measurements: it keeps the focus.
        t.key("Alt+3");
        t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
        assert_eq!(t.st.layout.focus, PaneKind::Ir);
        assert_eq!(t.st.pane_meas(PaneKind::Ir).map(|m| m.id), Some(MeasId(1)));
        // The pane's chip list picks for that pane.
        t.st.update(Msg::PaneShow(PaneKind::Spectrum, MeasId(4)), &t.keys);
        shows(&t, PaneKind::Spectrum, 4);
    }
}

/// A stored trace picked with the layout maximised: the pane that draws it comes up (a
/// sweep stays on the transfer pane, which draws it too, else goes to the sweep pane). The
/// split layout keeps its focus: every pane is on screen.
#[test]
fn picking_a_trace_while_maximised_brings_up_its_pane() {
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(13, None, 2),
        spec_meta(16),
        sweep_meta(14),
    ]));
    t.key("W");
    t.st.update(Msg::SelectTrace(TraceId(16)), &t.keys);
    assert_eq!(t.st.layout.visible(), [PaneKind::Spectrum]);
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    assert_eq!(t.st.layout.visible(), [PaneKind::Transfer]);
    t.st.update(Msg::SelectTrace(TraceId(14)), &t.keys);
    assert_eq!(t.st.layout.visible(), [PaneKind::Transfer]);
    t.st.update(Msg::SelectTrace(TraceId(16)), &t.keys);
    t.st.update(Msg::SelectTrace(TraceId(14)), &t.keys);
    assert_eq!(t.st.layout.visible(), [PaneKind::Distortion]);
    // V steps the same way.
    t.st.update(Msg::SelectTrace(TraceId(14)), &t.keys);
    t.key("Alt+2");
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(13)));
    assert_eq!(t.st.layout.visible(), [PaneKind::Transfer]);
    assert!(t.st.layout.maximized);
    // Split (W: full screen, W: split): the focus stays.
    t.key("W");
    t.key("W");
    assert!(!t.st.layout.maximized);
    t.key("Alt+2");
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    assert_eq!(t.st.layout.focus, PaneKind::Spectrum);
}

/// The state with measurement `id` running or stopped.
fn with_running(id: u32, running: bool) -> ConnEvent {
    let mut s = daemon_state();
    for m in &mut s.measurements {
        if m.id == MeasId(id) {
            m.running = running;
        }
    }
    mirror(s)
}

/// The newest frame of spectrum measurement `id`, aged `age_s` with the client's STALE flag.
fn aged_spec(t: &T, id: u32, age_s: f64) -> ac2_client::TopicFrame {
    let mut f =
        t.st.spectrum_frame(MeasId(id))
            .expect("spectrum frame")
            .clone();
    f.age = Some(age_s);
    f.since_new = std::time::Duration::from_secs_f64(age_s);
    f.stale = true;
    f
}

/// A stopped measurement's last frame is its final result: no STALE banner (nor its last
/// protection flags), the curve not dimmed, its caption saying `stopped`. A running one
/// whose frames stop is STALE.
#[test]
fn a_stopped_measurement_is_not_stale() {
    let mut t = T::new();
    spec_frame(&mut t, 2, vec![-60.0; 5]);
    let now = crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    };
    let f = aged_spec(&t, 2, 23.0);
    let running = crate::scenes::status(&t.st, &[&f], None, now);
    assert_eq!(running.frame_age_s, Some(23.0));
    assert!(
        ac2_scene::banner::banners(&running)
            .iter()
            .any(|b| b.text.starts_with("STALE")),
        "{running:?}"
    );

    t.conn(with_running(2, false));
    let stopped = crate::scenes::status(&t.st, &[&f], None, now);
    assert_eq!(stopped.frame_age_s, None);
    assert!(
        ac2_scene::banner::banners(&stopped).is_empty(),
        "{stopped:?}"
    );
    let fresh = crate::scenes::freshness(&t.st, &f);
    assert!(fresh.is_stopped() && !fresh.is_stale(), "{fresh:?}");
}

/// Each start of a spectrum fits the level axis on its first frame, as Shift+Home; frames
/// after it leave the operator's range alone.
#[test]
fn a_started_spectrum_fits_its_level_axis_once() {
    let mut t = T::new();
    let default = ViewState::default().spectrum.level;
    // Bins at 0, 6, 12, 18 and 24 kHz: only 6–18 kHz is in 20 Hz – 20 kHz.
    spec_frame(&mut t, 2, vec![0.0, -138.0, -120.0, -95.0, 0.0]);
    let r = t.st.view.spectrum.level;
    assert_ne!(r, default);
    assert!(r.lo <= -138.0 && r.lo >= -150.0, "{r:?}");
    assert!(r.hi >= -95.0 && r.hi <= -85.0, "{r:?}");

    // The operator's own range stays while the run goes on.
    t.key("Alt+2");
    t.key("Ctrl+Home");
    spec_frame(&mut t, 2, vec![0.0, -40.0, -40.0, -40.0, 0.0]);
    assert_eq!(t.st.view.spectrum.level, default);

    // Stopped: nothing fits; started again: the first new frame does.
    t.conn(with_running(2, false));
    spec_frame(&mut t, 2, vec![0.0, -40.0, -40.0, -40.0, 0.0]);
    assert_eq!(t.st.view.spectrum.level, default);
    t.conn(with_running(2, true));
    assert_eq!(
        t.st.view.spectrum.level, default,
        "the old frame is no start"
    );
    spec_frame(&mut t, 2, vec![0.0, -30.0, -35.0, -40.0, 0.0]);
    let r = t.st.view.spectrum.level;
    assert!(r.lo <= -40.0 && r.hi >= -30.0 && r.hi <= -20.0, "{r:?}");
}

/// The daemon's committed drift reaches every pane's banners once it is a warning; a value
/// below the threshold shows nothing.
#[test]
fn committed_clock_drift_shows_a_banner() {
    let mut t = T::new();
    let with_drift = |ppm: f64, warning: bool| {
        let mut s = daemon_state();
        s.timing.drift = Some(ac2_proto::model::Drift {
            ppm,
            span: ac2_proto::units::Seconds(30.0),
            warning,
            at: WallNs(0),
        });
        mirror(s)
    };
    t.conn(with_drift(0.3, false));
    let calm = crate::scenes::status(&t.st, &[], None, now());
    assert_eq!(calm.clock_drift_ppm, None);
    assert!(ac2_scene::banner::banners(&calm).is_empty(), "{calm:?}");
    t.conn(with_drift(49.6, true));
    let s = crate::scenes::status(&t.st, &[], None, now());
    let texts: Vec<String> = ac2_scene::banner::banners(&s)
        .into_iter()
        .map(|b| b.text)
        .collect();
    assert_eq!(texts, ["CLOCK DRIFT · 50 ppm"]);
}

/// A stopped session's audio reaches every pane's banners in place of STALE, and the top
/// bar's texts; it clears when the daemon reopens the session.
#[test]
fn stopped_audio_shows_a_banner_until_the_session_is_back() {
    use ac2_proto::model::{AudioStopped, Recovery, StopCause};
    let mut t = T::new();
    let mut s = daemon_state();
    s.session.stopped = Some(AudioStopped {
        since: WallNs(0),
        cause: StopCause::NotDelivering { after_ms: 1000 },
        recovery: Recovery::Waiting {
            attempt: 2,
            error: "No JACK server: start JACK".into(),
            next_at: WallNs(5_000_000_000),
        },
    });
    t.st.local_zone = crate::scenes::LocalZone::Fixed { offset_s: 0 };
    t.conn(mirror(s.clone()));
    let st = crate::scenes::status(&t.st, &[], None, now());
    let b = ac2_scene::banner::banners(&st);
    assert_eq!(
        b[0].text,
        "AUDIO STOPPED · device not delivering since 0:00"
    );
    assert_eq!(
        b[0].detail.as_deref(),
        Some("attempt 2 failed: No JACK server: start JACK · next in 5 s · measurements paused")
    );
    let bar = crate::scenes::audio_stopped(&t.st, WallNs(0)).map(|a| a.bar);
    assert_eq!(
        bar.as_ref().map(|b| b[0].as_str()),
        Some("audio stopped · reopening (attempt 2, next in 5 s)")
    );
    s.session.stopped = None;
    t.conn(mirror(s));
    let st = crate::scenes::status(&t.st, &[], None, now());
    assert!(ac2_scene::banner::banners(&st).is_empty());
    assert!(crate::scenes::audio_stopped(&t.st, WallNs(0)).is_none());
}

/// The IR pane follows its transfer measurement's state: the transfer stream's NO
/// REFERENCE as a banner and as the reason there is no IR; a stopped measurement says so and
/// which key starts it; a kept IR of a stopped measurement is tagged `stopped`, no STALE.
#[test]
fn the_ir_pane_says_why_there_is_no_ir() {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{IrFrame, IrMeta, ProtectionFlags};
    use ac2_proto::{Frame, FrameData};
    let theme = Theme::dark();
    let snapshot = |frames: Vec<Frame>| {
        let mut latest = Latest::default();
        for f in frames {
            let f = TopicFrame {
                topic: f.data.topic(),
                frame: Arc::new(f),
                received: Instant::now(),
                since_new: std::time::Duration::ZERO,
                age: Some(0.0),
                stale: false,
            };
            latest.frames.insert(f.topic.to_string().into(), f);
        }
        ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
            latest,
            grids: std::collections::BTreeMap::new(),
            drained: Instant::now(),
        }))
    };
    let mut tf = ac2_proto::samples::tf_frame();
    tf.stamp.protection = ProtectionFlags::NO_REFERENCE;
    let ir = Frame {
        stamp: ac2_proto::samples::stamp(None),
        data: FrameData::Ir(IrFrame {
            meas: MeasId(1),
            meta: IrMeta {
                sample_rate: Hz(48_000.0),
                t0: Seconds(-0.0005),
                dt: Seconds(0.0001),
                inserted_delay: Seconds(0.0125),
            },
            linear: vec![0.0, 0.5, 0.1, -0.05, 0.0],
            etc: None,
        }),
    };
    let mut t = T::new();
    t.key("Alt+3");
    let texts = |s: &ac2_scene::ir::IrScene| -> Vec<String> {
        s.banners.iter().map(|b| b.text.clone()).collect()
    };
    // Nothing yet from a running measurement.
    let s = crate::scenes::ir(&t.st, &t.keys, &theme, SIZE, now()).expect("scene");
    assert_eq!(s.note.as_deref(), Some("Main L: no IR frame yet"));
    // Its transfer stream says nothing drives the reference: banner and reason, which
    // follow this app's stimulus.
    t.conn(snapshot(vec![tf.clone()]));
    let ir_and_banner = |t: &T| {
        let s = crate::scenes::ir(&t.st, &t.keys, &theme, SIZE, now()).expect("scene");
        assert!(
            texts(&s).contains(&"NO REFERENCE".to_owned()),
            "{:?}",
            texts(&s)
        );
        let detail = s
            .banners
            .iter()
            .find(|r| r.text == "NO REFERENCE")
            .and_then(|r| r.detail.clone());
        (s.note.unwrap_or_default(), detail.unwrap_or_default())
    };
    assert_eq!(
        ir_and_banner(&t),
        (
            "no reference: nothing is playing — Space arms, Enter starts the stimulus".into(),
            "stimulus off: Space arms, Enter starts it".into()
        )
    );
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert_eq!(
        ir_and_banner(&t),
        (
            "no reference: armed — Enter starts the stimulus".into(),
            "stimulus armed: Enter starts it".into()
        )
    );
    t.key("Enter");
    t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
    assert_eq!(
        ir_and_banner(&t),
        (
            "no reference: nothing is driving the loopback".into(),
            "reference silent: check the loopback cable".into()
        )
    );
    t.key("Escape");
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    // On the sweep view Space arms a sweep: the noise is armed from a transfer pane.
    t.st.layout.focus = PaneKind::Distortion;
    assert_eq!(
        ir_and_banner(&t),
        (
            "no reference: nothing is playing — arm the stimulus from a transfer pane".into(),
            "stimulus off: arm it from a transfer pane".into()
        )
    );
    t.st.layout.focus = PaneKind::Ir;
    // Another client's stimulus, armed and silent: not this app's keys to press.
    let mut other = daemon_state();
    other.generator.owner = Some(ClientId("other".into()));
    other.generator.armed = true;
    t.conn(mirror(other));
    assert_eq!(
        ir_and_banner(&t).0,
        "no reference: nothing is driving the loopback"
    );
    // Stopped: no fault from a measurement that no longer runs, and the key that starts it.
    let mut st = daemon_state();
    st.measurements[1].running = false;
    t.conn(mirror(st.clone()));
    let s = crate::scenes::ir(&t.st, &t.keys, &theme, SIZE, now()).expect("scene");
    assert_eq!(s.note.as_deref(), Some("Main L stopped — S starts it"));
    assert!(!texts(&s).contains(&"NO REFERENCE".to_owned()));
    let r = t.key("S");
    assert!(
        r.iter().any(|x| matches!(
            x,
            Request::Call { cmd: Command::MeasStart { meas }, .. } if *meas == MeasId(1)
        )),
        "{r:?}"
    );
    // Its kept IR: drawn, tagged as its transfer curve is, never STALE.
    t.conn(snapshot(vec![tf, ir]));
    let s = crate::scenes::ir(&t.st, &t.keys, &theme, SIZE, now()).expect("scene");
    assert_eq!(s.note, None);
    assert_eq!(s.tag.as_deref(), Some("stopped"));
    assert!(texts(&s).iter().all(|b| !b.starts_with("STALE")));
    let line = format!("{} · stopped", s.origin);
    assert!(
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .any(|l| l.text == line),
        "{line}"
    );
}

fn meas_deleted(r: &[Request]) -> Option<(MeasId, String)> {
    match r {
        [
            Request::Call {
                cmd: Command::MeasDelete { meas, .. },
                what,
            },
        ] => Some((*meas, what.clone())),
        _ => None,
    }
}

/// Delete and Backspace act on what was selected last: a measurement asks first (naming
/// it and what goes), Esc or N keeps it, Delete / Backspace / Enter deletes it; a stored
/// trace selected after it is what they delete instead.
#[test]
fn delete_and_backspace_ask_then_delete_the_selected_measurement() {
    let mut t = T::new();
    t.conn(with_traces(vec![stored(10, Some(1), 2)]));
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    for key in ["Delete", "Backspace"] {
        assert!(t.key(key).is_empty(), "{key} only asks");
        let Overlay::Delete(p) = &t.st.overlay else {
            panic!("{key}: {:?}", t.st.overlay);
        };
        assert_eq!(p.target, DeleteTarget::Meas(MeasId(1)));
        assert_eq!(p.confirm.title, "Delete measurement Main L?");
        assert_eq!(
            p.confirm.lines,
            [
                "transfer function · running",
                "Its live curve and settings go; it owns no stored traces."
            ]
        );
        assert!(!p.confirm.refused);
        // Esc keeps it, the stimulus untouched.
        assert!(t.key("Esc").is_empty());
        assert_eq!(t.st.overlay, Overlay::None);
    }
    t.key("Backspace");
    assert!(t.key("N").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    // Delete then Enter, Backspace twice, Delete twice: each deletes it.
    for (first, second) in [
        ("Delete", "Enter"),
        ("Backspace", "Backspace"),
        ("Delete", "Delete"),
    ] {
        t.key(first);
        assert_eq!(
            meas_deleted(&t.key(second)),
            Some((MeasId(1), "Main L deleted".to_owned())),
            "{first} {second}"
        );
        assert_eq!(t.st.overlay, Overlay::None);
    }
    // A stored trace selected after it: Backspace is about the trace.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    t.key("Backspace");
    assert!(
        matches!(&t.st.overlay, Overlay::Delete(p) if p.target == DeleteTarget::Trace(TraceId(10))),
        "{:?}",
        t.st.overlay
    );
    t.key("Esc");
    // Selecting the measurement again gives it the keys back.
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("Delete");
    assert!(
        matches!(&t.st.overlay, Overlay::Delete(p) if p.target == DeleteTarget::Meas(MeasId(1)))
    );
}

/// A measurement a math channel computes from cannot go (the daemon refuses it): the
/// window says so in the confirmation's place, naming the channel, and only closes.
#[test]
fn deleting_a_math_operand_says_which_channel_uses_it() {
    use ac2_proto::model::{MathConfig, MathDomain, MathExpr, MathOp, Operand};
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.push(meas(
        3,
        "Sum",
        MeasKind::Math {
            config: MathConfig::of(
                ac2_proto::model::TraceOwner::Imported,
                MathDomain::Transfer,
                MathExpr::Binary {
                    a: Operand::Meas { meas: MeasId(1) },
                    op: MathOp::Add,
                    b: Operand::Meas { meas: MeasId(1) },
                },
            ),
        },
    ));
    t.conn(mirror(s));
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert!(t.key("Backspace").is_empty());
    let Overlay::Delete(p) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(p.target, DeleteTarget::Refused);
    assert_eq!(p.confirm.title, "Main L cannot be deleted");
    assert_eq!(
        p.confirm.lines,
        [
            "transfer function · an operand of the math channel Sum",
            "Edit or delete Sum first: it computes from Main L."
        ]
    );
    assert_eq!(p.confirm.hint, "Enter or Esc closes");
    // Nothing is deleted whatever the answer.
    for key in ["Enter", "Delete", "Backspace"] {
        assert!(t.key(key).is_empty(), "{key}");
        assert_eq!(t.st.overlay, Overlay::None);
        t.key("Delete");
    }
    assert!(t.st.update(Msg::Delete(true), &t.keys).is_empty());
    // The math channel itself goes after a confirmation.
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    t.key("Delete");
    assert_eq!(
        meas_deleted(&t.key("Enter")),
        Some((MeasId(3), "Sum deleted".to_owned()))
    );
}

/// An open window owns Backspace: in a prompt it erases typed text and never deletes the
/// selected measurement behind it.
#[test]
fn backspace_in_a_prompt_edits_its_text() {
    let mut t = T::new();
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.type_key("D", "d");
    t.text("12");
    assert!(t.key("Backspace").is_empty());
    assert!(t.st.update(Msg::Backspace, &t.keys).is_empty());
    let Overlay::Prompt(p) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    // Prefilled with the delay in use (12.5 ms): the two typed digits are gone.
    assert_eq!(p.text, "12.5");
    assert!(t.key("Backspace").is_empty());
    assert!(matches!(t.st.overlay, Overlay::Prompt(_)));
}

/// TF and spectrum frames of measurements 1 (Main L) and 2 (Sub).
fn live_frames(t: &mut T) {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{SpecFrame, SpecMeta};
    use ac2_proto::{Frame, FrameData};
    let tf = ac2_proto::samples::tf_frame();
    let spec_grid = GridDef::Linear {
        fs: Hz(48_000.0),
        n: 14,
    };
    let spec = Frame {
        stamp: ac2_proto::samples::stamp(Some(spec_grid.clone())),
        data: FrameData::Spec(SpecFrame {
            meas: MeasId(2),
            meta: SpecMeta {
                window: Window::Hann,
                scale: LevelScale::Dbfs,
                cal: CalStatus::Uncalibrated,
                mic_curve: false,
                smoothing: None,
                math: None,
            },
            level: vec![-60.0; 8],
        }),
    };
    let mut latest = Latest::default();
    for f in [tf, spec] {
        let f = TopicFrame {
            topic: f.data.topic(),
            frame: Arc::new(f),
            received: Instant::now(),
            since_new: std::time::Duration::ZERO,
            age: Some(0.0),
            stale: false,
        };
        latest.frames.insert(f.topic.to_string().into(), f);
    }
    let mut grids = std::collections::BTreeMap::new();
    for g in [ac2_proto::samples::log_grid(), spec_grid] {
        grids.insert(g.id(), Arc::new(g));
    }
    t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    })));
}

/// A on a selected measurement hides its live curves in every pane (display only: nothing
/// goes to the daemon, it keeps running); its list row, its pane's title and the IR pane
/// say so; A again shows it. The app remembers it by name. On a stored trace selected
/// after it, A shows / hides the trace as before.
#[test]
fn a_hides_and_shows_the_selected_measurement() {
    let mut t = T::new();
    t.conn(with_traces(vec![stored(10, Some(1), 2)]));
    live_frames(&mut t);
    let theme = Theme::dark();
    let tf_names = |t: &T| -> Vec<String> {
        crate::scenes::transfer(&t.st, &theme, SIZE, now())
            .legend
            .iter()
            .map(|e| e.name.clone())
            .collect()
    };
    let spec_names = |t: &T| -> Vec<String> {
        crate::scenes::spectrum(&t.st, &theme, SIZE, now())
            .legend
            .iter()
            .map(|e| e.name.clone())
            .collect()
    };
    let row = |t: &T, id: u32| {
        t.st.meas_rows()
            .into_iter()
            .find(|r| r.id == MeasId(id))
            .expect("row")
    };
    assert_eq!(tf_names(&t), ["Main L"]);
    assert_eq!(spec_names(&t), ["Sub"]);
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert_eq!(row(&t, 1).mark, ac2_scene::meas_list::Mark::Active);
    assert!(t.key("A").is_empty(), "display only");
    assert_eq!(
        t.last_toast(),
        "Main L hidden: it keeps measuring · A shows it"
    );
    assert!(tf_names(&t).is_empty());
    assert_eq!(spec_names(&t), ["Sub"]);
    assert!(t.st.meas(MeasId(1)).expect("still there").running);
    assert!(row(&t, 1).hidden);
    assert!(
        row(&t, 1).text.contains("running · hidden"),
        "{}",
        row(&t, 1).text
    );
    assert!(!row(&t, 2).hidden);
    let caption = t.st.pane_caption(PaneKind::Transfer).unwrap_or_default();
    assert!(caption.starts_with("Main L hidden"), "{caption}");
    assert_eq!(
        t.st.pane_caption_variants(PaneKind::Transfer)
            .last()
            .map(String::as_str),
        Some("Main L hidden")
    );
    assert_eq!(
        t.st.pane_menu_rows(PaneKind::Transfer),
        [(MeasId(1), "TF  Main L · hidden".to_owned())]
    );
    let ir = crate::scenes::ir(&t.st, &t.keys, &theme, SIZE, now()).expect("scene");
    assert_eq!(ir.note.as_deref(), Some("Main L hidden — A shows it"));
    // Remembered by name.
    assert_eq!(
        t.st.layout_prefs().hidden,
        ["Main L".to_owned()].into_iter().collect()
    );
    // The spectrum's measurement too.
    t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
    assert!(t.key("A").is_empty());
    assert!(spec_names(&t).is_empty());
    assert!(
        t.st.pane_caption(PaneKind::Spectrum)
            .is_some_and(|c| c.starts_with("Sub hidden"))
    );
    // A stored trace selected after it: A is about the trace (the daemon keeps that).
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    assert_eq!(row(&t, 2).mark, ac2_scene::meas_list::Mark::Selected);
    let r = t.key("A");
    assert!(
        matches!(r.as_slice(), [Request::Call { cmd: Command::TraceUpdate { trace: TraceId(10), edit }, .. }] if !edit.visible),
        "{r:?}"
    );
    // Back on the measurements: A shows them again.
    t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
    assert!(t.key("A").is_empty());
    assert_eq!(t.last_toast(), "Sub shown");
    assert_eq!(spec_names(&t), ["Sub"]);
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("A");
    assert_eq!(tf_names(&t), ["Main L"]);
    assert!(t.st.layout_prefs().hidden.is_empty());
    assert!(!row(&t, 1).text.contains("hidden"));
}

/// The hidden measurements come back by name on the next start; a deleted one's name is
/// forgotten.
#[test]
fn hidden_measurements_are_remembered_by_name() {
    let mut t = T::new();
    t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
    t.key("A");
    let p = crate::prefs::UiPrefs {
        layout: t.st.layout_prefs(),
        ..Default::default()
    };
    let mut u = T::new();
    u.st.set_prefs(p);
    assert!(u.st.meas_hidden(u.st.meas(MeasId(2)).expect("Sub")));
    u.st.update(Msg::SelectMeas(MeasId(2)), &u.keys);
    u.key("Delete");
    assert!(meas_deleted(&u.key("Enter")).is_some());
    assert!(u.st.hidden_meas.is_empty());
}
