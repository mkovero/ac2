//! Reducer tests of the display edits: offset steps, the level axis, deleting a stored
//! trace, and which pane a selection brings up.

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
/// difference: the transfer legend's tag, the spectrum's note.
#[test]
fn offsets_are_named_where_the_curves_are_drawn() {
    let mut t = T::new();
    let tf = with_offset(stored(13, Some(2), 2), 3.0);
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
    assert_eq!(s.offsets, ["t16 · offset −6.0 dB"]);
}

fn spec_frame(t: &mut T, meas: u32, level: Vec<f32>) {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::Frame;
    use ac2_proto::FrameData;
    use ac2_proto::frame::{SpecFrame, SpecMeta, ValidityMask};
    let grid = GridDef::Linear {
        fs: Hz(48_000.0),
        n: (level.len() as u32 - 1) * 2,
    };
    let n = level.len();
    let data = FrameData::Spec(SpecFrame {
        meas: MeasId(meas),
        meta: SpecMeta {
            window: Window::Hann,
            scale: LevelScale::Dbfs,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            smoothing: None,
        },
        level,
        validity: vec![ValidityMask::NONE; n],
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
    latest.frames.insert(f.topic.to_string(), f);
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
    // No level axis in the IR pane: said, nothing changes.
    t.key("Alt+3");
    let view = t.st.view;
    t.key("Ctrl+I");
    assert_eq!(t.st.view, view);
    assert!(
        t.last_toast().contains("no level axis"),
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
    // Nothing selected: never the live measurement.
    assert!(t.key("Delete").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(
        t.last_toast().contains("stored trace"),
        "{}",
        t.last_toast()
    );
    // Asks first, naming it.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    assert!(t.key("Delete").is_empty());
    let Overlay::DeleteTrace(p) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(p.trace, TraceId(10));
    assert_eq!(p.confirm.title, "Delete t10?");
    assert_eq!(p.confirm.lines[0], "capture · slot 1 · no data yet");
    // Esc keeps it (and stops the stimulus, as always); so do N and Backspace.
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
    t.key("Delete");
    assert!(t.st.update(Msg::Backspace, &t.keys).is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    // Other keys wait for the answer.
    t.key("Delete");
    assert!(t.key("A").is_empty());
    assert!(matches!(t.st.overlay, Overlay::DeleteTrace(_)));
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
    let (_, what) = deleted(&t.st.update(Msg::DeleteTrace(true), &t.keys)).expect("deleted");
    assert_eq!(what, "t14 deleted · t11 selected");
    // The only one: back to the live measurement.
    t.conn(with_traces(vec![stored(11, None, 2)]));
    t.key("Delete");
    let (_, what) = deleted(&t.key("Delete")).expect("deleted");
    assert_eq!(what, "t11 deleted · keys act on the live measurement");
    assert_eq!(t.st.selected_trace, None);
    // The mouse's Keep.
    t.conn(with_traces(vec![stored(11, None, 2)]));
    t.st.update(Msg::SelectTrace(TraceId(11)), &t.keys);
    t.key("Delete");
    assert!(t.st.update(Msg::DeleteTrace(false), &t.keys).is_empty());
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
    t.text("delete selected trace");
    t.key("Enter");
    assert!(matches!(&t.st.overlay, Overlay::DeleteTrace(p) if p.trace == TraceId(11)));
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
