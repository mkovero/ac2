//! Reducer tests of selection: which pane a picked measurement or trace brings up, and
//! hiding and showing the selected measurement.

use super::*;

/// Picking a measurement in the list puts it in the focused pane, which turns into the kind
/// of pane the measurement belongs in, maximised or not; every other pane keeps what it
/// shows.
#[test]
fn picking_a_measurement_puts_it_in_the_focused_pane() {
    for maximised in [true, false] {
        let mut t = T::new();
        let mut s = four();
        s.measurements.push(meas(5, "FOH SPL", spl_meter()));
        t.conn(mirror(s));
        if maximised {
            t.key("W");
        }
        assert_eq!(t.st.layout.maximized, maximised);
        let f = t.st.layout.focus;
        let others = |t: &T| {
            let l = &t.st.layout;
            l.views
                .iter()
                .filter(|(id, _)| **id != l.focus)
                .map(|(id, v)| (*id, *v))
                .collect::<Vec<_>>()
        };
        let before = others(&t);
        let shows = |t: &T, p: PaneKind, id: u32| {
            assert_eq!(t.st.layout.focus, f, "maximised {maximised}");
            assert_eq!(t.focus_kind(), p, "maximised {maximised}");
            assert_eq!(t.st.pane_meas(f).map(|m| m.id), Some(MeasId(id)));
            assert_eq!(t.st.layout.maximized, maximised);
            assert_eq!(others(t), before);
            if maximised {
                assert_eq!(t.visible(), [p]);
            }
        };
        // An RTA: the focused transfer pane turns into a spectrum / RTA pane.
        t.st.update(Msg::SelectMeas(MeasId(4)), &t.keys);
        shows(&t, PaneKind::Spectrum, 4);
        // A narrowband spectrum stays there, and the pane shows it.
        t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
        shows(&t, PaneKind::Spectrum, 2);
        // A transfer measurement: back to a transfer pane.
        t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
        shows(&t, PaneKind::Transfer, 3);
        // An SPL meter: an SPL pane.
        t.st.update(Msg::SelectMeas(MeasId(5)), &t.keys);
        shows(&t, PaneKind::Spl, 5);
        // The IR view is a transfer pane's: a transfer measurement keeps the view.
        t.key("Alt+3");
        let ir = t.st.layout.focus;
        t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
        assert_eq!(t.st.layout.focus, ir);
        assert!(t.st.layout.focused().shows_ir());
        assert_eq!(t.st.pane_meas(ir).map(|m| m.id), Some(MeasId(1)));
        // The pane's chip list picks for that pane.
        let sp = t.pane(PaneKind::Spectrum);
        t.st.update(Msg::PanePick(sp, PaneMenuRow::Meas(MeasId(4))), &t.keys);
        assert_eq!(t.st.layout.focus, sp);
        assert_eq!(t.st.pane_meas(sp).map(|m| m.id), Some(MeasId(4)));
    }
}

/// A stored trace picked with the layout maximised: the pane that draws it comes up (a
/// sweep stays on the transfer pane, which draws it too, else the maximised pane turns into
/// the sweep pane). The split layout keeps its focus: every pane is on screen.
#[test]
fn picking_a_trace_while_maximised_brings_up_its_pane() {
    let mut t = T::new();
    t.conn(with_sweeps(vec![
        stored(13, None, 2),
        spec_meta(16),
        sweep_meta(14),
    ]));
    t.key("W");
    t.st.update(Msg::SelectTrace(TraceId(16)), &t.keys);
    assert_eq!(t.visible(), [PaneKind::Spectrum]);
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    assert_eq!(t.visible(), [PaneKind::Transfer]);
    t.st.update(Msg::SelectTrace(TraceId(14)), &t.keys);
    assert_eq!(t.visible(), [PaneKind::Transfer]);
    t.st.update(Msg::SelectTrace(TraceId(16)), &t.keys);
    t.st.update(Msg::SelectTrace(TraceId(14)), &t.keys);
    assert_eq!(t.visible(), [PaneKind::Distortion]);
    // V steps the same way (from the SPL pane: the sweep pane's V steps its runs), in the
    // tree's order: from the sweep's run on to Sub's spectrum.
    t.st.update(Msg::SelectTrace(TraceId(14)), &t.keys);
    t.key("Alt+4");
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(16)));
    assert_eq!(t.visible(), [PaneKind::Spectrum]);
    assert!(t.st.layout.maximized);
    // Split (W: full screen, W: split): the focus stays.
    t.key("W");
    t.key("W");
    assert!(!t.st.layout.maximized);
    t.key("Alt+3");
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    assert!(t.st.layout.focused().shows_ir());
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
                level: ac2_proto::model::SpecLevel::Tone,
                cal: CalStatus::Uncalibrated,
                mic_curve: false,
                smoothing: None,
                math: None,
                bands: None,
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
        crate::scenes::transfer(&t.st, t.pane(PaneKind::Transfer), &theme, SIZE, now())
            .legend
            .iter()
            .map(|e| e.name.clone())
            .collect()
    };
    let spec_names = |t: &T| -> Vec<String> {
        crate::scenes::spectrum(&t.st, t.pane(PaneKind::Spectrum), &theme, SIZE, now())
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
    let caption =
        t.st.pane_caption(t.pane(PaneKind::Transfer))
            .unwrap_or_default();
    assert!(caption.starts_with("Main L hidden"), "{caption}");
    assert_eq!(
        t.st.pane_caption_variants(t.pane(PaneKind::Transfer))
            .last()
            .map(String::as_str),
        Some("Main L hidden")
    );
    assert_eq!(
        t.st.pane_menu_rows(t.pane(PaneKind::Transfer))[0],
        (
            PaneMenuRow::Meas(MeasId(1)),
            "TF  Main L · hidden".to_owned()
        )
    );
    let ir = crate::scenes::ir(&t.st, t.ir_pane(), &t.keys, &theme, SIZE, now()).expect("scene");
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
        t.st.pane_caption(t.pane(PaneKind::Spectrum))
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

/// Sub and two transfer measurements both named Main (names may repeat).
fn two_mains() -> State {
    let mut s = daemon_state();
    s.measurements = vec![
        meas(1, "Main", transfer()),
        meas(2, "Sub", spectrum()),
        meas(3, "Main", transfer()),
    ];
    s
}

/// Hiding or comparing one of two measurements of one name is that measurement's alone.
#[test]
fn hiding_one_of_two_same_named_measurements_hides_only_it() {
    let mut t = T::new();
    t.conn(mirror(two_mains()));
    t.st.update(Msg::ToggleMeasShown(MeasId(1)), &t.keys);
    let m = |t: &T, id| t.st.meas(MeasId(id)).cloned().expect("meas");
    assert!(t.st.meas_hidden(&m(&t, 1)));
    assert!(!t.st.meas_hidden(&m(&t, 3)), "hiding id 1 also hid id 3");
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    t.key("C");
    assert!(t.st.meas_compared(&m(&t, 3)));
    assert!(
        !t.st.meas_compared(&m(&t, 1)),
        "comparing id 3 also compared id 1"
    );
}

/// A hidden measurement deleted elsewhere does not pass its choice to another of its name.
#[test]
fn a_deleted_duplicates_choice_does_not_pass_to_the_other() {
    let mut t = T::new();
    t.conn(mirror(two_mains()));
    t.st.update(Msg::ToggleMeasShown(MeasId(1)), &t.keys);
    let mut s = two_mains();
    s.measurements.retain(|m| m.id != MeasId(1));
    t.conn(mirror(s));
    assert!(!t.st.meas_hidden(t.st.meas(MeasId(3)).expect("Main")));
    assert!(t.st.hidden_meas.is_empty());
}

/// A rename keeps the choice on the measurement; a new one under the old name starts shown.
#[test]
fn a_renamed_measurement_keeps_its_hidden_choice() {
    let mut t = T::new();
    t.st.update(Msg::ToggleMeasShown(MeasId(1)), &t.keys);
    let mut s = daemon_state();
    s.measurements = vec![
        meas(1, "Main X", transfer()),
        meas(2, "Sub", spectrum()),
        meas(5, "Main L", transfer()),
    ];
    t.conn(mirror(s));
    assert!(t.st.meas_hidden(t.st.meas(MeasId(1)).expect("renamed")));
    assert!(!t.st.meas_hidden(t.st.meas(MeasId(5)).expect("new")));
    // Remembered under its new name.
    assert_eq!(t.st.layout_prefs().hidden, ["Main X".to_owned()].into());
}

/// `ui.toml` keeps choices by name; they become ids once the daemon lists measurements. A
/// name several measurements share applies to none of them and is forgotten (it does not
/// land later on whichever is left), and is never written for one of them.
#[test]
fn remembered_names_resolve_to_ids_and_a_shared_name_applies_to_none() {
    let mut t = T::new();
    t.conn(mirror(two_mains()));
    t.st.update(Msg::ToggleMeasShown(MeasId(1)), &t.keys);
    t.st.update(Msg::ToggleMeasShown(MeasId(2)), &t.keys);
    let kept = t.st.layout_prefs();
    assert_eq!(
        kept.hidden,
        ["Sub".to_owned()].into(),
        "Main is not one measurement's"
    );

    // A file naming both: Sub is the one Sub; Main matches two.
    let p = crate::prefs::UiPrefs {
        layout: crate::prefs::LayoutPrefs {
            hidden: ["Main".to_owned(), "Sub".to_owned()].into(),
            compared: ["Main".to_owned()].into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let p = crate::prefs::UiPrefs::from_toml(&p.to_toml()).expect("parse");
    let mut u = T::fresh();
    u.st.layout = grid();
    u.st.pane_auto = false;
    u.st.set_prefs(p);
    u.connect(two_mains());
    assert_eq!(u.st.hidden_meas, [MeasId(2)].into());
    assert!(u.st.compared_meas.is_empty());
    // One Main deleted elsewhere: the other does not take the forgotten choice.
    let mut s = two_mains();
    s.measurements.retain(|m| m.id != MeasId(3));
    u.conn(mirror(s));
    assert!(!u.st.meas_hidden(u.st.meas(MeasId(1)).expect("Main")));
    assert!(u.st.compared_meas.is_empty());
}

/// A daemon restarting numbers its measurements afresh: a hidden one waits by name and
/// is hidden again under its new id.
#[test]
fn a_hidden_measurement_stays_hidden_across_a_daemon_restart() {
    let mut t = T::new();
    t.st.update(Msg::ToggleMeasShown(MeasId(2)), &t.keys);
    let mut s = daemon_state();
    s.measurements = vec![meas(7, "Sub", spectrum()), meas(8, "Main L", transfer())];
    t.conn(mirror_of(s, 2, Some("c1")));
    assert_eq!(t.st.hidden_meas, [MeasId(7)].into());
}
