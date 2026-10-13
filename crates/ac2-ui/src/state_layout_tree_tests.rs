//! Reducer tests of the measurement tree: dots, folds, hiding, moving traces and Tab steps.

use super::*;

fn tree_names(t: &T) -> Vec<String> {
    t.st.tree_rows().iter().map(|r| r.name.clone()).collect()
}

/// Every tree row that stands for a curve has a dot in the colour the panes draw that curve
/// in (live curves, stored traces, math results alike), a ring once it is hidden; the
/// headers have none.
#[test]
fn tree_dots_have_the_colours_of_their_curves() {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{SpecFrame, SpecMeta};
    use ac2_proto::{Frame, FrameData};
    use ac2_scene::primitives::{Color, Viewport};
    let mut s = tree_state();
    for (i, t) in s.traces.iter_mut().enumerate() {
        let v = 40 * (i as u8 + 1);
        t.edit.color = Rgb {
            r: v,
            g: 255 - v,
            b: 7,
        };
    }
    let metas = s.traces.clone();
    let mut t = T::new();
    t.conn(mirror(s));
    for m in &metas {
        let grid = GridDef::Log {
            ppo: 1,
            k_min: -5,
            k_max: 2,
        };
        t.conn(ConnEvent::Trace(
            Arc::new(TraceData {
                meta: m.clone(),
                mag_db: vec![-10.0; 8],
                phase_deg: None,
                coherence: None,
                sweep: None,
                ir: None,
            }),
            Arc::new(grid),
        ));
    }
    // Live frames of Main L, of the math channel on it and of Sub.
    let spec_grid = GridDef::Linear {
        fs: Hz(48_000.0),
        n: 14,
    };
    let mut math = ac2_proto::samples::tf_frame();
    if let FrameData::Tf(f) = &mut math.data {
        f.meas = MeasId(6);
    }
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
    for f in [ac2_proto::samples::tf_frame(), math, spec] {
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
    // The import is under no measurement: the transfer pane draws it when compared.
    t.st.compared_traces.insert(TraceId(5));

    let theme = ac2_scene::theme::Theme::by_name(t.st.theme);
    let size = Viewport {
        width: 1000.0,
        height: 450.0,
    };
    let now = crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    };
    // The colour each curve is drawn in, by its name, from both panes.
    let mut drawn: Vec<(String, Color)> =
        crate::scenes::transfer(&t.st, t.pane(PaneKind::Transfer), &theme, size, now)
            .traces
            .iter()
            .map(|d| (d.name.clone(), d.color))
            .collect();
    drawn.extend(crate::scenes::with_spectrum(
        &t.st,
        t.pane(PaneKind::Spectrum),
        &theme,
        now,
        |traces, _, _| {
            traces
                .iter()
                .map(|d| (d.name.clone(), d.color))
                .collect::<Vec<_>>()
        },
    ));
    let curve = |name: &str| {
        drawn
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name} drawn: {drawn:?}"))
            .1
    };
    let rows = t.st.tree_rows();
    let dot = |name: &str| {
        rows.iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} listed"))
            .dot
    };
    for (row, name) in [
        ("Main L (live)", "Main L"),
        ("Sub (live)", "Sub"),
        ("pre ÷ post", "pre ÷ post"),
        ("pre-EQ", "pre-EQ"),
        ("post-EQ", "post-EQ"),
        ("1083 94cm", "1083 94cm"),
    ] {
        assert_eq!(dot(row), Some((curve(name), true)), "{row}");
    }
    let colours: std::collections::BTreeSet<[u8; 3]> = rows
        .iter()
        .filter_map(|r| r.dot)
        .map(|(c, _)| [c.r, c.g, c.b].map(|v| (v * 255.0).round() as u8))
        .collect();
    assert_eq!(colours.len(), 6, "each curve its own colour");
    // Each measurement its own colour family: Main L's live curve, captures and math result
    // share its hue, Sub has another, the import takes a hue no measurement holds, and the
    // daemon's per-trace colour is not what is drawn.
    let hue = |c: Color| ac2_scene::families::oklch(c).2.to_degrees();
    let gap = |a: Color, b: Color| {
        let d = (hue(a) - hue(b)).rem_euclid(360.0);
        d.min(360.0 - d)
    };
    let main = curve("Main L");
    assert_eq!(main, theme.families[0]);
    assert_eq!(curve("Sub"), theme.families[1]);
    for n in ["pre-EQ", "post-EQ", "pre ÷ post"] {
        assert!(gap(curve(n), main) < 8.0, "{n} in Main L's family");
    }
    assert_eq!(curve("1083 94cm"), theme.families[2]);
    for m in &metas {
        let c = m.edit.color;
        assert_ne!(curve(&m.edit.name), Color::from_rgba8([c.r, c.g, c.b, 255]));
    }
    for r in rows.iter().filter(|r| r.depth == 0) {
        assert_eq!(r.dot, None, "{} is a group", r.name);
    }
    // Hidden: a ring of the same colour.
    let sub = curve("Sub");
    t.st.update(Msg::ToggleMeasShown(MeasId(2)), &t.keys);
    let rows = t.st.tree_rows();
    let row = rows.iter().find(|r| r.name == "Sub (live)").expect("row");
    assert_eq!(row.dot, Some((sub, false)));
    t.st.update(Msg::ToggleMeasShown(MeasId(6)), &t.keys);
    let rows = t.st.tree_rows();
    let row = rows.iter().find(|r| r.name == "pre ÷ post").expect("row");
    assert_eq!(row.dot.map(|d| d.1), Some(false));

    // Moved under Sub, the import takes Sub's family.
    let mut moved = tree_state();
    moved.traces[2].edit.owner = TraceOwner::Meas { meas: MeasId(2) };
    let mut u = T::new();
    u.conn(mirror(moved));
    let rows = u.st.tree_rows();
    let row = rows.iter().find(|r| r.name == "1083 94cm").expect("row");
    let (c, _) = row.dot.expect("dot");
    assert!(gap(c, sub) < 8.0, "{c:?} in Sub's family");
    assert_ne!(c, sub, "a shade, not Sub's live curve");
}

#[test]
fn the_tree_lists_what_each_measurement_owns_and_folds() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    assert_eq!(
        tree_names(&t),
        [
            "TF  Main L",
            "Main L (live)",
            "pre-EQ",
            "post-EQ",
            "pre ÷ post",
            "FFT  Sub",
            "Sub (live)",
            "Imported",
            "1083 94cm"
        ]
    );
    // V steps through the traces in the tree's order.
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(3)));
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(4)));
    // Folding Main L hides its rows; its arrow (or the palette command) unfolds it.
    t.st.update(
        Msg::ToggleGroup(TraceOwner::Meas { meas: MeasId(1) }),
        &t.keys,
    );
    assert_eq!(
        tree_names(&t),
        [
            "TF  Main L",
            "FFT  Sub",
            "Sub (live)",
            "Imported",
            "1083 94cm"
        ]
    );
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.st.update(Msg::Command(CommandId::ToggleGroup), &t.keys);
    assert_eq!(tree_names(&t).len(), 9);
}

/// Shift+A on a measurement hides its live curve, its traces and its math channel; again
/// shows them.
#[test]
fn shift_a_hides_a_measurement_with_everything_under_it() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    let r = t.key("Shift+A");
    let hidden: Vec<TraceId> = r
        .iter()
        .filter_map(|x| match x {
            Request::Call {
                cmd: Command::TraceUpdate { trace, edit },
                ..
            } if !edit.visible => Some(*trace),
            _ => None,
        })
        .collect();
    assert_eq!(hidden, [TraceId(3), TraceId(4)]);
    assert!(hidden_named(&t, "Main L"));
    assert!(hidden_named(&t, "pre ÷ post"));
    assert!(!hidden_named(&t, "Sub"));
    // The daemon hid them: Shift+A again shows the whole group.
    let mut s = tree_state();
    for tr in &mut s.traces[..2] {
        tr.edit.visible = false;
    }
    t.conn(mirror(s));
    let r = t.key("Shift+A");
    let shown = r
        .iter()
        .filter(|x| {
            matches!(x, Request::Call { cmd: Command::TraceUpdate { edit, .. }, .. } if edit.visible)
        })
        .count();
    assert_eq!(shown, 2);
    assert!(!hidden_named(&t, "Main L"));
}

/// Shift+F2 asks where the selected trace goes; Enter files it there (a `trace.update` with
/// its new owner). Where it is now cannot be picked.
#[test]
fn move_files_a_trace_under_another_measurement() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.update(Msg::SelectTrace(TraceId(5)), &t.keys);
    t.key("Shift+F2");
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(c.title, "Move 1083 94cm to…");
    let labels: Vec<&str> = c.choices.iter().map(|x| x.label.as_str()).collect();
    assert_eq!(
        labels,
        ["TF  Main L", "FFT  Sub", "Imported (under no measurement)"]
    );
    assert!(c.choices[2].blocked.is_some(), "it is filed there");
    assert_eq!(c.index, 0);
    let r = t.key("Enter");
    match r.as_slice() {
        [
            Request::Call {
                cmd: Command::TraceUpdate { trace, edit },
                what,
            },
        ] => {
            assert_eq!(*trace, TraceId(5));
            assert_eq!(edit.owner, TraceOwner::Meas { meas: MeasId(1) });
            assert_eq!(what, "1083 94cm moved to Main L");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(t.st.overlay, Overlay::None);
    // A math channel moves with `meas.update`.
    t.st.update(Msg::SelectMeas(MeasId(6)), &t.keys);
    t.key("Shift+F2");
    t.key("Down");
    t.key("Down");
    let r = t.key("Enter");
    let moved = match r.as_slice() {
        [
            Request::Call {
                cmd: Command::MeasUpdate { meas, config },
                ..
            },
        ] => (*meas, config.kind.clone()),
        other => panic!("{other:?}"),
    };
    assert_eq!(moved.0, MeasId(6));
    assert!(matches!(moved.1, MeasKind::Math { config } if config.owner == TraceOwner::Imported));
}

/// Deleting a measurement that owns traces asks every time: keep them (the default),
/// delete them too, or cancel — keyboard only.
#[test]
fn deleting_a_measurement_with_traces_asks_keep_delete_or_cancel() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert!(t.key("Delete").is_empty());
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(c.title, "Delete measurement Main L?");
    assert_eq!(
        c.lines[0],
        "transfer function · running · it has 2 traces and 1 math channel."
    );
    assert_eq!(c.index, ac2_scene::meas_list::KEEP);
    // Esc cancels; nothing is sent.
    assert!(t.key("Esc").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    let deleted = |r: &[Request]| match r {
        [
            Request::Call {
                cmd: Command::MeasDelete { meas, traces },
                ..
            },
        ] => Some((*meas, *traces)),
        _ => None,
    };
    // Enter keeps them.
    t.key("Delete");
    let r = t.key("Enter");
    assert_eq!(deleted(&r), Some((MeasId(1), OwnedTraces::Keep)));
    // → then Enter deletes them too; → → then Enter cancels.
    t.key("Delete");
    t.key("Right");
    let r = t.key("Enter");
    assert_eq!(deleted(&r), Some((MeasId(1), OwnedTraces::Delete)));
    t.key("Delete");
    t.key("Right");
    t.key("Right");
    assert!(t.key("Enter").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
}

/// Shift+M files the new math channel under the measurement selected, and offers its live
/// curve and traces first.
#[test]
fn a_new_math_channel_lives_under_the_selected_measurement() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("Shift+M");
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    let m = f.math.as_ref().expect("math dialog");
    assert_eq!(m.owner, TraceOwner::Meas { meas: MeasId(1) });
    let first: Vec<&str> = m
        .candidates
        .iter()
        .take(3)
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(first, ["Main L", "pre-EQ", "post-EQ"]);
    let r = t.key("Enter");
    let c = r
        .iter()
        .find_map(|x| match x {
            Request::CreateMeas { config } => Some(config.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{r:?}"));
    assert!(matches!(&c.kind, MeasKind::Math { config }
        if config.owner == TraceOwner::Meas { meas: MeasId(1) }));
}

/// Tab / Shift+Tab walk the rows of the tree as listed (a measurement's header, its live
/// curve, its stored traces and math channels, the imported traces; the Imported header
/// skipped), wrapping; the focused pane shows each row's measurement.
#[test]
fn tab_steps_through_the_trees_rows() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.selected = None;
    // Main L, its live curve, pre-EQ, post-EQ, pre ÷ post, Sub, its live curve, the
    // imported trace; then round again.
    let fwd: Vec<_> = (0..9).map(|_| tab(&mut t, "Tab")).collect();
    assert_eq!(fwd, ["m1", "l1", "t3", "t4", "m6", "m2", "l2", "t5", "m1"]);
    let back: Vec<_> = (0..9).map(|_| tab(&mut t, "Shift+Tab")).collect();
    assert_eq!(back, ["t5", "l2", "m2", "m6", "t4", "t3", "l1", "m1", "t5"]);
    assert!(t.st.tree_reveal, "the tree scrolls to the row");

    // Nothing selected: Shift+Tab starts at the last. (A click on the selected trace
    // deselects it.)
    t.st.update(Msg::SelectTrace(TraceId(5)), &t.keys);
    t.st.selected = None;
    assert_eq!(t.st.selected_trace, None);
    assert_eq!(tab(&mut t, "Shift+Tab"), "t5");

    // The focused pane shows the row's measurement, as on a click on its row, turning into
    // its kind; the focus stays.
    assert_eq!(tab(&mut t, "Shift+Tab"), "l2");
    let pane = t.st.layout.focus;
    assert_eq!(t.focus_kind(), PaneKind::Spectrum);
    assert_eq!(t.st.pane_meas(pane).map(|m| m.id), Some(MeasId(2)));
    assert_eq!(tab(&mut t, "Shift+Tab"), "m2");
    assert_eq!(t.focus_kind(), PaneKind::Spectrum);
    assert_eq!(tab(&mut t, "Shift+Tab"), "m6");
    assert_eq!(
        (t.st.layout.focus, t.focus_kind()),
        (pane, PaneKind::Transfer)
    );
    assert_eq!(t.st.pane_meas(pane).map(|m| m.id), Some(MeasId(6)));
    // A stored trace of another measurement: the pane shows that measurement, the trace
    // selected in it.
    assert_eq!(tab(&mut t, "Shift+Tab"), "t4");
    assert_eq!(t.st.layout.focus, pane);
    assert_eq!(t.st.pane_meas(pane).map(|m| m.id), Some(MeasId(1)));
}

/// From a selected trace, Tab goes on from its row; from the imported trace (the tree's
/// last row) it starts over.
#[test]
fn tab_from_a_trace_goes_on_from_its_row() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.update(Msg::SelectTrace(TraceId(4)), &t.keys);
    assert_eq!(tab(&mut t, "Tab"), "m6");
    t.st.update(Msg::SelectTrace(TraceId(3)), &t.keys);
    assert_eq!(tab(&mut t, "Shift+Tab"), "l1");
    t.st.update(Msg::SelectTrace(TraceId(5)), &t.keys);
    assert_eq!(tab(&mut t, "Tab"), "m1");
    t.st.update(Msg::SelectTrace(TraceId(5)), &t.keys);
    assert_eq!(tab(&mut t, "Shift+Tab"), "l2");
}

/// A live curve reached by Tab is selected as a click on its legend row selects it: A hides
/// that curve, as it does a stored trace. A legend or list pick of the live curve is a Tab
/// stop to go on from; a header click is the header again.
#[test]
fn tab_stops_on_live_curves_and_a_hides_them() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert!(!t.st.live_row_selected());
    assert_eq!(tab(&mut t, "Tab"), "l1");
    t.key("A");
    assert!(t.st.hidden_meas.contains(&MeasId(1)), "{}", t.last_toast());
    assert_eq!(tab(&mut t, "Tab"), "t3");
    assert_eq!(
        tab(&mut t, "Shift+Tab"),
        "l1",
        "a hidden live curve is still a stop"
    );
    t.key("A");
    assert!(!t.st.hidden_meas.contains(&MeasId(1)));

    let pane = t.st.layout.focus;
    t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
    t.st.update(
        Msg::LegendPick(pane, ac2_scene::trace::TraceKey::Live(MeasId(1))),
        &t.keys,
    );
    assert!(t.st.live_row_selected());
    assert_eq!(tab(&mut t, "Tab"), "t3");
    t.st.update(Msg::SelectLive(MeasId(2)), &t.keys);
    assert!(t.st.live_row_selected());
    assert_eq!(tab(&mut t, "Shift+Tab"), "m2");
    t.st.update(Msg::SelectLive(MeasId(1)), &t.keys);
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert!(!t.st.live_row_selected());
    assert_eq!(tab(&mut t, "Tab"), "l1");
}

/// A folded group's rows are skipped, as the list does not show them, and the folds stay;
/// from a selection inside a folded group Tab goes on to the next row shown.
#[test]
fn tab_skips_the_rows_of_folded_groups() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    let main = TraceOwner::Meas { meas: MeasId(1) };
    let sub = TraceOwner::Meas { meas: MeasId(2) };
    t.st.collapsed.insert(main);
    t.st.collapsed.insert(sub);
    t.st.selected = Some(MeasId(1));
    let fwd: Vec<_> = (0..3).map(|_| tab(&mut t, "Tab")).collect();
    assert_eq!(fwd, ["m2", "t5", "m1"]);
    assert!(t.st.collapsed.contains(&main) && t.st.collapsed.contains(&sub));
    t.st.update(Msg::SelectTrace(TraceId(4)), &t.keys);
    assert_eq!(tab(&mut t, "Tab"), "m2");
    t.st.update(Msg::SelectTrace(TraceId(4)), &t.keys);
    assert_eq!(tab(&mut t, "Shift+Tab"), "m1");
}

/// No measurements: a toast says so and nothing is selected.
#[test]
fn tab_without_measurements_says_so() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.clear();
    t.conn(mirror(s));
    t.st.selected = None;
    t.key("Tab");
    assert_eq!(t.st.selected, None);
    assert_eq!(t.last_toast(), "no measurements to step through");
}

/// Whether the measurement named `name` has its live curves hidden.
fn hidden_named(t: &T, name: &str) -> bool {
    t.st.measurements()
        .iter()
        .any(|m| m.config.name == name && t.st.meas_hidden(m))
}
