//! Reducer tests of panes, smoothing, stored traces and slots, math channels, the palette and the toasts.

use super::*;

#[test]
fn panes_show_and_select_their_measurement() {
    let mut t = T::new();
    t.conn(mirror(four()));
    let shown = |t: &T, p: PaneKind| t.st.kind_meas(p).map(|m| m.id.0);
    assert_eq!(t.st.selected, Some(MeasId(1)));
    let ir = |t: &T| t.st.pane_meas(t.ir_pane()).map(|m| m.id.0);
    assert_eq!(shown(&t, PaneKind::Transfer), Some(1));
    assert_eq!(ir(&t), Some(1));
    assert_eq!(shown(&t, PaneKind::Spectrum), Some(2));
    assert_eq!(shown(&t, PaneKind::Spl), None);

    // A click in a pane focuses it and selects what it shows.
    t.st.update(Msg::FocusPane(t.pane(PaneKind::Spectrum)), &t.keys);
    assert_eq!(t.focus_kind(), PaneKind::Spectrum);
    assert_eq!(t.st.selected, Some(MeasId(2)));
    // Its list offers the measurements of its kind: spectrum and RTA here.
    let sp = t.pane(PaneKind::Spectrum);
    t.st.update(Msg::PanePick(sp, PaneMenuRow::Meas(MeasId(4))), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(4)));
    assert_eq!(shown(&t, PaneKind::Spectrum), Some(4));
    t.st.update(Msg::PanePick(sp, PaneMenuRow::Meas(MeasId(2))), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(2)));
    // The transfer pane kept its own measurement.
    t.st.update(Msg::FocusPane(t.pane(PaneKind::Transfer)), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(1)));
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(3)));
    assert_eq!(shown(&t, PaneKind::Transfer), Some(3));
    assert_eq!(ir(&t), Some(3), "an unchosen IR view follows the selection");
    // Focus by key selects too.
    t.key("Alt+2");
    assert_eq!(t.st.selected, Some(MeasId(2)));
    // The IR view follows the selection, a spectrum now: it leads with the first transfer
    // measurement.
    t.key("Alt+3");
    assert_eq!(t.st.selected, Some(MeasId(1)));

    // A list selection goes into the focused pane; the others keep theirs.
    t.key("Alt+2");
    t.st.update(Msg::SelectMeas(MeasId(4)), &t.keys);
    assert_eq!(shown(&t, PaneKind::Spectrum), Some(4));
    assert_eq!(t.st.pane_meas(PaneId(1)).map(|m| m.id.0), Some(3));

    // The title chip: the pane's list (every measurement, in the tree's order) with what it
    // shows highlighted; the arrows and Enter show another.
    let row_of = |t: &T, p: PaneId, id: u32| {
        t.st.pane_menu_rows(p)
            .iter()
            .position(|(r, _)| *r == PaneMenuRow::Meas(MeasId(id)))
            .expect("listed")
    };
    let tf = PaneId(1);
    t.st.update(Msg::FocusPane(tf), &t.keys);
    t.st.update(Msg::PaneMenu(tf), &t.keys);
    let at = row_of(&t, tf, 3);
    assert_eq!(
        t.st.overlay,
        Overlay::PaneMenu(PaneMenu {
            pane: tf,
            index: at
        })
    );
    // Keys other than the list's do nothing while it is open.
    assert!(t.key("X").is_empty());
    let to = row_of(&t, tf, 1);
    for _ in 0..at.abs_diff(to) {
        t.key(if to < at { "Up" } else { "Down" });
    }
    t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.selected, Some(MeasId(1)));
    assert_eq!(shown(&t, PaneKind::Transfer), Some(1));
    // A click on the chip again closes the list; a pick by mouse shows it.
    t.st.update(Msg::PaneMenu(t.pane(PaneKind::Spectrum)), &t.keys);
    assert!(
        matches!(t.st.overlay, Overlay::PaneMenu(m) if m.pane == t.pane(PaneKind::Spectrum) && m.index == row_of(&t, m.pane, 4))
    );
    t.st.update(Msg::PaneMenu(t.pane(PaneKind::Spectrum)), &t.keys);
    assert_eq!(t.st.overlay, Overlay::None);
    t.st.update(Msg::PaneMenu(t.pane(PaneKind::Spectrum)), &t.keys);
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Spectrum), PaneMenuRow::Meas(MeasId(2))),
        &t.keys,
    );
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.focus_kind(), PaneKind::Spectrum);
    assert_eq!(t.st.selected, Some(MeasId(2)));
    // A measurement the pane cannot draw turns it into the kind that does, and back.
    let sp = t.pane(PaneKind::Spectrum);
    t.st.update(Msg::PanePick(sp, PaneMenuRow::Meas(MeasId(3))), &t.keys);
    assert_eq!(t.st.layout.kind(sp), PaneKind::Transfer);
    assert_eq!(t.st.pane_meas(sp).map(|m| m.id.0), Some(3));
    t.st.update(Msg::PanePick(sp, PaneMenuRow::Meas(MeasId(2))), &t.keys);
    assert_eq!(t.st.layout.kind(sp), PaneKind::Spectrum);
    assert_eq!(shown(&t, PaneKind::Spectrum), Some(2));
    // Palette entry for the keyboard: the focused pane's list.
    t.st.update(Msg::Command(CommandId::PaneMeasurement), &t.keys);
    assert!(matches!(t.st.overlay, Overlay::PaneMenu(m) if m.pane == t.pane(PaneKind::Spectrum)));
    t.key("Esc");
    assert_eq!(t.st.overlay, Overlay::None);
    // No measurement of its kind: the list offers every measurement, then the kinds of pane
    // none of them needs (a sweep pane here).
    let spl = t.pane(PaneKind::Spl);
    t.st.update(Msg::PaneMenu(spl), &t.keys);
    assert!(matches!(t.st.overlay, Overlay::PaneMenu(m) if m.pane == spl && m.index == 0));
    let mut want: Vec<PaneMenuRow> =
        t.st.tree_meas_order()
            .into_iter()
            .map(PaneMenuRow::Meas)
            .collect();
    want.push(PaneMenuRow::Kind(PaneKind::Distortion));
    let rows: Vec<PaneMenuRow> =
        t.st.pane_menu_rows(spl)
            .into_iter()
            .map(|(r, _)| r)
            .collect();
    assert_eq!(rows, want);
    t.key("Esc");

    // A deleted measurement leaves its pane showing the next one that fits.
    t.st.update(Msg::FocusPane(t.pane(PaneKind::Transfer)), &t.keys);
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    assert_eq!(shown(&t, PaneKind::Transfer), Some(3));
    let mut s = four();
    s.measurements.retain(|m| m.id != MeasId(3));
    t.conn(mirror(s));
    assert_eq!(shown(&t, PaneKind::Transfer), Some(1));
}

/// The smoothing a request sets, and on what.
fn smoothing_set(r: &[Request]) -> (String, Option<Smoothing>) {
    match r {
        [
            Request::Call {
                cmd: Command::MeasUpdate { meas, config },
                ..
            },
        ] => match &config.kind {
            MeasKind::Transfer { config } => (format!("m{}", meas.0), config.smoothing),
            MeasKind::Spectrum { config } => (
                format!("m{}", meas.0),
                config.smoothing.map(|fraction| Smoothing {
                    fraction,
                    mode: SmoothingMode::Magnitude,
                }),
            ),
            other => panic!("{other:?}"),
        },
        [
            Request::Call {
                cmd: Command::TraceUpdate { trace, edit },
                ..
            },
        ] => (format!("t{}", trace.0), edit.smoothing),
        other => panic!("{other:?}"),
    }
}

#[test]
fn smoothing_keys_step_the_pane_measurement() {
    let mut t = T::new();
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Transfer))
            .as_deref(),
        Some("smoothing off")
    );
    // K coarser: off → 1/48 of magnitude and phase, with the measurement's config
    // otherwise unchanged.
    let r = t.key("K");
    assert_eq!(
        smoothing_set(&r),
        (
            "m1".into(),
            smoothing(
                SmoothingFraction::FortyEighth,
                SmoothingMode::MagnitudePhase
            )
        )
    );
    match r.as_slice() {
        [
            Request::Call {
                cmd: Command::MeasUpdate { config, .. },
                what,
            },
        ] => {
            assert_eq!(what, "Main L: smoothing 1/48 oct");
            let MeasKind::Transfer { config } = &config.kind else {
                unreachable!()
            };
            let MeasKind::Transfer { config: was } = transfer() else {
                unreachable!()
            };
            assert_eq!(config.averaging, was.averaging);
            assert_eq!(config.grid, was.grid);
        }
        other => panic!("{other:?}"),
    }
    // Finer than off: nothing sent, said.
    assert!(t.key("Shift+K").is_empty());
    assert!(t.last_toast().contains("finest"), "{}", t.last_toast());

    // The mirror says 1/6 magnitude only (set by a script): K → 1/3 (mode kept),
    // Shift+K → 1/12, at 1/3 K stops.
    let mut s = daemon_state();
    if let MeasKind::Transfer { config } = &mut s.measurements[1].config.kind {
        config.smoothing = smoothing(SmoothingFraction::Sixth, SmoothingMode::Magnitude);
    }
    t.conn(mirror(s.clone()));
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Transfer))
            .as_deref(),
        Some("smoothing 1/6 oct mag only")
    );
    assert_eq!(
        smoothing_set(&t.key("K")).1,
        smoothing(SmoothingFraction::Third, SmoothingMode::Magnitude)
    );
    assert_eq!(
        smoothing_set(&t.key("Shift+K")).1,
        smoothing(SmoothingFraction::Twelfth, SmoothingMode::Magnitude)
    );
    if let MeasKind::Transfer { config } = &mut s.measurements[1].config.kind {
        config.smoothing = smoothing(SmoothingFraction::Third, SmoothingMode::Magnitude);
    }
    t.conn(mirror(s));
    assert!(t.key("K").is_empty());
    assert!(t.last_toast().contains("widest"), "{}", t.last_toast());
    // Palette entries set a step directly.
    let r = t.st.update(Msg::Command(CommandId::SmoothOff), &t.keys);
    assert_eq!(smoothing_set(&r).1, None);
    let r = t.st.update(Msg::Command(CommandId::Smooth24), &t.keys);
    assert_eq!(
        smoothing_set(&r).1,
        smoothing(SmoothingFraction::TwentyFourth, SmoothingMode::Magnitude)
    );
    // In the spectrum pane K acts on its spectrum: power smoothing, no phase to name.
    t.key("Alt+2");
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Spectrum))
            .as_deref(),
        Some("smoothing off")
    );
    let r = t.key("K");
    assert_eq!(
        smoothing_set(&r),
        (
            "m2".into(),
            smoothing(SmoothingFraction::FortyEighth, SmoothingMode::Magnitude)
        )
    );
    match r.as_slice() {
        [Request::Call { what, .. }] => assert_eq!(what, "Sub: smoothing 1/48 oct"),
        other => panic!("{other:?}"),
    }
    // The transfer pane's caption still speaks of its own measurement.
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Transfer))
            .as_deref(),
        Some("smoothing 1/3 oct mag only")
    );
}

/// RTA bands are fractional-octave already: K in the spectrum pane on an RTA says so and
/// sends nothing.
#[test]
fn smoothing_keys_explain_rta() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements[0].config.kind = MeasKind::Rta {
        config: ac2_proto::model::RtaConfig::on_input(1, ac2_proto::model::BandFraction::Third),
    };
    t.conn(mirror(s));
    t.key("Alt+2");
    assert_eq!(t.st.smoothing_caption(t.pane(PaneKind::Spectrum)), None);
    assert!(t.key("K").is_empty());
    assert!(
        t.last_toast().contains("already are fractional-octave"),
        "{}",
        t.last_toast()
    );
}

#[test]
fn smoothing_keys_change_a_selected_slot() {
    let mut t = T::new();
    let mut spec = stored(11, Some(4), 2);
    spec.kind = TraceKind::Spectrum {
        scale: LevelScale::Dbfs,
    };
    let mut locked = stored(12, Some(5), 2);
    locked.edit.locked = true;
    t.conn(with_traces(vec![stored(10, Some(3), 2), spec, locked]));
    // A click on slot 3 selects it: K changes its trace, the caption says so.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Transfer))
            .as_deref(),
        Some("slot 3 (t10): smoothing off")
    );
    let r = t.key("K");
    assert_eq!(
        smoothing_set(&r),
        (
            "t10".into(),
            smoothing(
                SmoothingFraction::FortyEighth,
                SmoothingMode::MagnitudePhase
            )
        )
    );
    match r.as_slice() {
        [
            Request::Call {
                cmd: Command::TraceUpdate { edit, .. },
                what,
            },
        ] => {
            assert_eq!(what, "slot 3 (t10): smoothing 1/48 oct");
            // Its other edits are sent unchanged.
            let mut want = stored(10, Some(3), 2).edit;
            want.smoothing = edit.smoothing;
            assert_eq!(*edit, want);
        }
        other => panic!("{other:?}"),
    }
    // A spectrum slot is power-smoothed; its caption goes to the spectrum pane.
    t.st.update(Msg::SelectTrace(TraceId(11)), &t.keys);
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Spectrum))
            .as_deref(),
        Some("slot 4 (t11): smoothing off")
    );
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Transfer))
            .as_deref(),
        Some("smoothing off")
    );
    assert_eq!(
        smoothing_set(&t.key("K")),
        (
            "t11".into(),
            smoothing(SmoothingFraction::FortyEighth, SmoothingMode::Magnitude)
        )
    );
    // A locked slot says so and sends nothing.
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert!(t.key("K").is_empty());
    assert!(t.last_toast().contains("locked"));
    // A second click deselects; the keys act on the pane's measurement again.
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert_eq!(t.st.selected_trace, None);
    assert_eq!(smoothing_set(&t.key("K")).0, "m1");
    // Selecting a measurement (list, pane click, N) deselects the slot.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    t.st.update(Msg::FocusPane(t.pane(PaneKind::Transfer)), &t.keys);
    assert_eq!(t.st.selected_trace, None);
    // A selected trace that goes away is forgotten.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    t.conn(with_traces(vec![]));
    assert_eq!(t.st.selected_trace, None);
}

#[test]
fn traces_are_selected_from_the_keyboard() {
    let mut t = T::new();
    // No shown trace: V says so.
    assert!(t.key("V").is_empty());
    assert!(t.last_toast().contains("no shown stored traces"));
    let mut hidden = stored(11, Some(2), 2);
    hidden.edit.visible = false;
    let mut hidden_sweep = sweep_meta(14);
    hidden_sweep.edit.visible = false;
    t.conn(with_traces(vec![
        stored(12, Some(5), 2),
        hidden,
        stored(10, Some(1), 2),
        stored(13, None, 2),
        hidden_sweep,
    ]));
    // The list's order: slotted by slot, then the rest oldest first.
    let order: Vec<u32> = t.st.trace_list().iter().map(|x| x.id.0).collect();
    assert_eq!(order, [10, 11, 12, 13, 14]);
    // V steps through the shown traces in that order, slotted or not (hidden skipped), then
    // back to the live measurement; it sends nothing.
    assert!(t.key("V").is_empty());
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    assert!(t.last_toast().contains("slot 1 (t10) selected"));
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(12)));
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(13)));
    assert_eq!(t.last_toast(), "t13 selected");
    t.key("V");
    assert_eq!(t.st.selected_trace, None);
    assert!(t.last_toast().contains("live measurement"));
    // Shift+V goes the other way, from live to the last shown trace.
    t.key("Shift+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(13)));
    // K then changes the selected trace, though it has no slot.
    assert_eq!(smoothing_set(&t.key("K")).0, "t13");
    t.key("Shift+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(12)));
    // Alt+V reaches the hidden ones too.
    t.key("Alt+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(13)));
    t.key("Alt+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(14)));
    assert_eq!(t.last_toast(), "t14 (hidden) selected");
    t.key("Alt+Shift+V");
    t.key("Alt+Shift+V");
    t.key("Alt+Shift+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(11)));
    // From a hidden trace, V and Shift+V go to the shown ones beside it.
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(12)));
    t.st.update(Msg::SelectTrace(TraceId(11)), &t.keys);
    t.key("Shift+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    // The palette entry deselects.
    t.st.update(Msg::Command(CommandId::SelectLive), &t.keys);
    assert_eq!(t.st.selected_trace, None);
    // Esc with a dialog open only closes it (and stops); with nothing open it also hands
    // the keys back to the live measurement.
    t.key("V");
    t.key("H");
    assert!(t.key("Esc").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    t.key("Esc");
    assert_eq!(t.st.selected_trace, None);
    assert_eq!(smoothing_set(&t.key("K")).0, "m1");
    // Tab (another measurement) deselects too.
    t.key("V");
    t.key("Tab");
    assert_eq!(t.st.selected_trace, None);
}

#[test]
fn a_shows_and_hides_the_selected_trace_and_the_eye_any_trace() {
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(10, Some(1), 2),
        stored(13, None, 2),
    ]));
    // Nothing selected: A says how to select.
    t.conn(mirror(empty_state()));
    assert!(t.key("A").is_empty());
    assert_eq!(
        t.last_toast(),
        "select a measurement or a stored trace first (click it in the list, N, V)"
    );
    t.conn(with_traces(vec![
        stored(10, Some(1), 2),
        stored(13, None, 2),
    ]));
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    let (id, edit, what) = trace_update(&t.key("A"));
    assert_eq!(id, TraceId(13));
    assert!(!edit.visible);
    assert_eq!(edit.slot, None);
    assert_eq!(what, "t13 hidden");
    // The eye in the list toggles any trace, selected or not, and selects nothing.
    let (id, edit, what) = trace_update(&t.st.update(Msg::ToggleShown(TraceId(10)), &t.keys));
    assert_eq!(id, TraceId(10));
    assert!(!edit.visible);
    assert_eq!(what, "slot 1 (t10) hidden");
    assert_eq!(t.st.selected_trace, Some(TraceId(13)));
    // The list says what each one is and which is selected.
    let rows = t.st.trace_rows();
    assert_eq!(
        rows.iter()
            .map(|r| (r.name.as_str(), r.details[0].as_str(), r.selected))
            .collect::<Vec<_>>(),
        [
            ("t10", "capture · slot 1 · no data yet", false),
            ("t13", "capture · no data yet", true),
        ]
    );
}

#[test]
fn the_selected_trace_moves_to_a_slot() {
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(10, Some(1), 2),
        stored(13, None, 2),
    ]));
    t.st.update(Msg::Command(CommandId::TraceSlot), &t.keys);
    assert_eq!(t.st.overlay, Overlay::None, "nothing selected");
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    t.st.update(Msg::Command(CommandId::TraceSlot), &t.keys);
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceSlot(TraceId(13)) && p.text.is_empty()
    ));
    t.text("1");
    // The daemon takes slot 1 from its holder.
    let (id, edit, what) = trace_update(&t.key("Enter"));
    assert_eq!((id, edit.slot, edit.visible), (TraceId(13), Some(1), true));
    assert_eq!(what, "t13 in slot 1");
    let r = prompt_text(&mut t, CommandId::TraceSlot, "none");
    let (_, edit, what) = trace_update(&r);
    assert_eq!(edit.slot, None);
    assert_eq!(what, "t13: slot freed");
    let r = prompt_text(&mut t, CommandId::TraceSlot, "10");
    assert!(r.is_empty());
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.error.as_deref().is_some_and(|e| e.contains("1 … 9"))
    ));
    assert_eq!(parse_slot("slot 3"), Ok(Some(3)));
    assert_eq!(parse_slot(" "), Ok(None));
    assert!(parse_slot("0").is_err());
}

/// F2 (or the palette) renames the selected trace; a double click on a row in the list
/// selects it and asks for the name in one go.
#[test]
fn the_selected_trace_is_renamed() {
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(10, Some(1), 2),
        stored(13, None, 2),
    ]));
    t.st.update(Msg::Command(CommandId::TraceRename), &t.keys);
    assert_eq!(t.st.overlay, Overlay::None, "nothing selected");
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    t.key("F2");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceRename(TraceId(13)) && p.text == "t13"
    ));
    let r = prompt_text(&mut t, CommandId::TraceRename, "  1083 on axis ");
    let (id, edit, what) = trace_update(&r);
    assert_eq!((id, edit.name.as_str()), (TraceId(13), "1083 on axis"));
    assert_eq!(what, "t13 renamed to 1083 on axis");
    // An empty name is refused in the prompt.
    let r = prompt_text(&mut t, CommandId::TraceRename, "   ");
    assert!(r.is_empty());
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.error.as_deref() == Some("type a name")
    ));
    t.key("Escape");
    // Double click on another row: selected, and its name asked for.
    t.st.update(Msg::RenameTrace(TraceId(10)), &t.keys);
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceRename(TraceId(10)) && p.text == "t10"
    ));
}

#[test]
fn trace_keys_act_on_the_selected_trace() {
    let mut t = T::new();
    let mut locked = stored(12, None, 2);
    locked.edit.locked = true;
    let mut target = stored(15, None, 2);
    target.kind = TraceKind::Target;
    let mut spec = stored(16, None, 2);
    spec.kind = TraceKind::Spectrum {
        scale: LevelScale::Dbfs,
    };
    t.conn(with_traces(vec![
        stored(13, None, 2),
        locked,
        target,
        spec,
        sweep_meta(14),
    ]));
    // An unslotted capture: U, J, , . and E change it, not the live measurement.
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    let (id, edit, what) = trace_update(&t.key("U"));
    assert_eq!((id, edit.polarity), (TraceId(13), Polarity::Inverted));
    assert_eq!(what, "t13: polarity inverted");
    t.type_key("J", "j");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceOffset(TraceId(13)) && p.text.is_empty()
    ));
    t.text("-3,5 dB");
    let (_, edit, what) = trace_update(&t.key("Enter"));
    assert_eq!(edit.offset, Db(-3.5));
    assert_eq!(what, "t13: offset −3.5 dB");
    let (_, edit, what) = trace_update(&t.key("."));
    assert!((edit.delay_nudge.0 - 0.000_1).abs() < 1e-15);
    assert_eq!(what, "t13: delay +0.1 ms → +0.10 ms from arrival");
    // The mirror is still at the arrival (no reply here): the step from there.
    let (_, edit, what) = trace_update(&t.key(","));
    assert!((edit.delay_nudge.0 + 0.000_1).abs() < 1e-15);
    assert_eq!(what, "t13: delay −0.1 ms → −0.10 ms from arrival");
    assert!(t.key("E").is_empty());
    assert_eq!(
        t.st.view.tf.phase_reference,
        Some(TraceKey::Stored(TraceId(13)))
    );
    assert_eq!(t.last_toast(), "phase reference: t13");
    assert_eq!(t.st.edit(MeasId(1)), LiveEdit::default(), "live untouched");
    // A sweep result too.
    t.st.update(Msg::SelectTrace(TraceId(14)), &t.keys);
    assert_eq!(trace_update(&t.key("U")).0, TraceId(14));
    // A locked trace says so and sends nothing.
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert!(t.key("U").is_empty());
    assert!(t.last_toast().contains("t12 is locked"));
    // A target curve has an offset but no phase.
    t.st.update(Msg::SelectTrace(TraceId(15)), &t.keys);
    assert!(t.key("U").is_empty());
    assert!(t.last_toast().contains("no phase"));
    assert!(t.key("E").is_empty());
    assert!(t.last_toast().contains("no phase"));
    t.type_key("J", "j");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceOffset(TraceId(15))
    ));
    t.key("Esc");
    // The caption names a selected target.
    t.st.update(Msg::SelectTrace(TraceId(15)), &t.keys);
    t.st.update(Msg::SelectTrace(TraceId(15)), &t.keys);
    assert_eq!(
        t.st.pane_caption(t.pane(PaneKind::Transfer)).as_deref(),
        Some("t15")
    );
    // A spectrum trace is not on the transfer pane: the keys act on the live measurement.
    t.st.update(Msg::SelectTrace(TraceId(16)), &t.keys);
    assert!(t.key("U").is_empty());
    assert!(t.st.edit(MeasId(1)).inverted);
}

/// One selection: a sweep selected in the transfer pane is what the sweep pane shows, and
/// N on the sweep pane selects the sweep it steps to.
#[test]
fn the_sweep_pane_follows_the_selection_and_selects() {
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(10, Some(1), 2),
        sweep_meta(14),
        sweep_meta(15),
    ]));
    for id in [14, 15] {
        let (d, g) = sweep_data(id);
        t.conn(ConnEvent::Trace(d, g));
    }
    let shown = |t: &T| t.st.shown_sweep().map(|(d, _)| d.meta.id.0);
    // Nothing chosen: the newest.
    assert_eq!(shown(&t), Some(15));
    // V in the transfer pane: slot 1, then the first sweep, which the sweep pane shows.
    t.key("V");
    assert_eq!(
        shown(&t),
        Some(15),
        "a capture selected: the sweep pane keeps its sweep"
    );
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(14)));
    assert_eq!(shown(&t), Some(14));
    assert_eq!(
        t.st.pane_caption(t.pane(PaneKind::Transfer)).as_deref(),
        Some("t14: smoothing off")
    );
    // Back to live: the sweep pane keeps the sweep selected last.
    t.key("Esc");
    assert_eq!(t.st.selected_trace, None);
    assert_eq!(shown(&t), Some(14));
    // V on the sweep pane steps the sweep runs and selects them for the transfer pane.
    t.go(PaneKind::Distortion);
    assert_eq!(t.focus_kind(), PaneKind::Distortion);
    t.key("V");
    assert_eq!(shown(&t), Some(15));
    assert_eq!(t.st.selected_trace, Some(TraceId(15)));
    assert_eq!(t.last_toast(), "t15 selected");
    t.key("Shift+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(14)));
    // U on the sweep pane is its unit; on the transfer pane it inverts the selected sweep.
    assert!(t.key("U").is_empty());
    t.key("Alt+1");
    assert_eq!(trace_update(&t.key("U")).0, TraceId(14));
    // A click on a sweep in the list does the same.
    t.st.update(Msg::SelectTrace(TraceId(15)), &t.keys);
    assert_eq!(shown(&t), Some(15));
}

#[test]
fn escape_stops_the_stimulus_with_a_slot_selected() {
    let mut t = T::new();
    t.conn(with_traces(vec![stored(10, Some(1), 2)]));
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    let r = t.key("Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
    assert_eq!(t.st.selected_trace, None);
}

#[test]
fn resmoothed_trace_data_keeps_its_smoothing_until_refetched() {
    let mut t = T::new();
    let meta = stored(10, Some(3), 2);
    t.conn(with_traces(vec![meta.clone()]));
    let data = Arc::new(TraceData {
        meta: meta.clone(),
        mag_db: vec![0.0; 4],
        phase_deg: None,
        coherence: None,
        sweep: None,
        ir: None,
    });
    let grid = Arc::new(GridDef::Log {
        ppo: 1,
        k_min: 0,
        k_max: 3,
    });
    t.conn(ConnEvent::Trace(data, grid.clone()));
    // The mirror says 1/6 now; the columns held were served unsmoothed, so the drawn
    // trace keeps saying so (other edits follow at once) until the new data arrives.
    let mut m2 = meta.clone();
    m2.edit.smoothing = smoothing(SmoothingFraction::Sixth, SmoothingMode::Magnitude);
    m2.edit.name = "renamed".into();
    t.conn(with_traces(vec![m2.clone()]));
    let held = &t.st.traces[&TraceId(10)].0.meta;
    assert_eq!(held.edit.smoothing, None);
    assert_eq!(held.edit.name, "renamed");
    let fresh = Arc::new(TraceData {
        meta: m2.clone(),
        mag_db: vec![1.0; 4],
        phase_deg: None,
        coherence: None,
        sweep: None,
        ir: None,
    });
    t.conn(ConnEvent::Trace(fresh, grid));
    assert_eq!(t.st.traces[&TraceId(10)].0.meta, m2);
}

#[test]
fn mic_curve_goes_on_the_selected_trace_from_the_palette() {
    let mut t = T::new();
    let mut a = stored(10, Some(3), 2);
    a.mic = Some(MicState {
        name: "MM1 34804".into(),
        curve: None,
    });
    let mut s = daemon_state();
    s.traces = vec![a];
    s.mics = vec![mm1()];
    t.conn(mirror(s));
    // Nothing selected: refused with the way to select.
    t.st.update(Msg::Command(CommandId::TraceMicCurve), &t.keys);
    assert!(!matches!(&t.st.overlay, Overlay::Prompt(_)));
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    // Prefilled with the mic the trace was captured with.
    t.st.update(Msg::Command(CommandId::TraceMicCurve), &t.keys);
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceMicCurve(TraceId(10))
            && p.text == "MM1 34804 "
    ));
    let call = |r: &[Request]| {
        r.iter().find_map(|r| match r {
            Request::Call {
                cmd: Command::TraceMicCurve { trace, curve },
                ..
            } => Some((*trace, curve.clone())),
            _ => None,
        })
    };
    // Two curves: the label is required, and the prompt says which there are.
    let r = t.key("Enter");
    assert!(call(&r).is_none());
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.error.as_deref().is_some_and(|e| e.contains("0°, 90°"))
    ));
    t.text("90°");
    let r = t.key("Enter");
    assert_eq!(
        call(&r),
        Some((
            TraceId(10),
            Some(MicCurveId {
                mic: "MM1 34804".into(),
                label: "90°".into()
            })
        ))
    );
    let r = prompt_text(&mut t, CommandId::TraceMicCurve, "MM1 34804 45°");
    assert!(call(&r).is_none());
    let r = prompt_text(&mut t, CommandId::TraceMicCurve, "none");
    assert_eq!(call(&r), Some((TraceId(10), None)));
    let r = prompt_text(&mut t, CommandId::TraceMicCurve, " ");
    assert!(call(&r).is_none());
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
}

#[test]
fn slots_capture_and_replace() {
    let mut t = T::new();
    let r = t.key("Ctrl+3");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            meas: MeasId(1),
            slot: 3,
            replace: None,
            name,
        }] if name == "Main L S3"
    ));
    // Slots are the daemon's: a mirrored trace in slot 3 is what Ctrl+3 replaces.
    t.conn(with_traces(vec![stored(9, Some(3), 2)]));
    assert_eq!(t.st.slots()[2].map(|t| t.id), Some(TraceId(9)));
    let r = t.key("Ctrl+3");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            slot: 3,
            replace: Some(TraceId(9)),
            ..
        }]
    ));
    // A locked trace is not deleted; it only gives up the slot.
    let mut locked = stored(9, Some(3), 2);
    locked.edit.locked = true;
    t.conn(with_traces(vec![locked]));
    let r = t.key("Ctrl+3");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            slot: 3,
            replace: None,
            ..
        }]
    ));
    // Spectrum measurements capture too.
    t.st.selected = Some(MeasId(2));
    let r = t.key("Ctrl+1");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            meas: MeasId(2),
            slot: 1,
            ..
        }]
    ));
    // A trace deleted daemon-side frees its slot.
    t.conn(mirror(daemon_state()));
    assert!(t.st.slots()[2].is_none());
}

#[test]
fn digits_show_and_hide_slots_alt_digits_focus_panes() {
    let mut t = T::new();
    t.conn(with_traces(vec![stored(9, Some(3), 2)]));
    let r = t.key("3");
    let [
        Request::Call {
            cmd: Command::TraceUpdate { trace, edit },
            what,
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}")
    };
    assert_eq!(*trace, TraceId(9));
    assert!(!edit.visible);
    assert_eq!(edit.slot, Some(3));
    assert_eq!(what, "slot 3 (t9) hidden");
    // The pane did not change focus.
    assert_eq!(t.focus_kind(), PaneKind::Transfer);
    // An empty slot says how to fill it.
    assert!(t.key("5").is_empty());
    assert!(t.last_toast().contains("slot 5 is empty"));
    t.key("Alt+2");
    assert_eq!(t.focus_kind(), PaneKind::Spectrum);
    t.key("Alt+1");
    assert_eq!(t.focus_kind(), PaneKind::Transfer);
}

#[test]
fn m_averages_the_shown_stored_traces() {
    let mut t = T::new();
    // One shown trace is not enough.
    t.conn(with_traces(vec![captured(4, Some(1), 2)]));
    assert!(t.key("M").is_empty());
    assert!(
        t.st.toasts
            .last()
            .is_some_and(|x| x.severity == Severity::Warning)
    );
    let mut hidden = captured(6, Some(3), 2);
    hidden.edit.visible = false;
    let mut target = captured(7, None, 2);
    target.kind = TraceKind::Target;
    t.conn(with_traces(vec![
        captured(4, Some(2), 2),
        captured(5, Some(1), 2),
        hidden,
        target,
    ]));
    let r = t.key("M");
    let [
        Request::Call {
            cmd:
                Command::TraceAverage {
                    traces,
                    method,
                    reference,
                    name,
                },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}")
    };
    // Slot order, hidden and target traces left out, reference = the first.
    assert_eq!(traces, &[TraceId(5), TraceId(4)]);
    assert_eq!(*method, AverageMethod::Power);
    assert_eq!(*reference, DelayReference::Trace { trace: TraceId(5) });
    assert_eq!(name, "avg S1+S2");
    // The phase reference (decision 8b) wins when it is one of them.
    t.st.view.tf.phase_reference = Some(TraceKey::Stored(TraceId(4)));
    let r = t.key("M");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::TraceAverage {
                reference: DelayReference::Trace { trace: TraceId(4) },
                ..
            },
            ..
        }]
    ));
    // Complex averaging from the palette.
    t.key("Ctrl+K");
    t.text("average complex");
    let r = t.key("Enter");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::TraceAverage {
                method: AverageMethod::Complex,
                ..
            },
            ..
        }]
    ));
}

/// Shift+M opens the math channel dialog: A is what the pane shows, B the next of its kind
/// (live or stored), the operator ÷, the name the expression; Enter creates it.
#[test]
fn math_channel_dialog_by_name() {
    use crate::forms::{FieldId, FormKind};
    use ac2_proto::model::{MathDomain, MathExpr, MathOp, Operand};
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(4, Some(2), 2),
        stored(5, Some(7), 2),
    ]));
    t.key("Shift+M");
    assert!(matches!(&t.st.overlay, Overlay::Form(f) if f.kind == FormKind::Math));
    assert_eq!(form(&t).fields[0].display(), "Main L (live)");
    assert_eq!(form(&t).text(FieldId::Name), "Main L ÷ t4");
    // B steps on to the other stored trace; the name follows.
    t.key("Down");
    t.key("Down");
    t.key("Right");
    assert_eq!(form(&t).text(FieldId::Name), "Main L ÷ t5");
    let r = t.key("Enter");
    let c = created(&r).unwrap_or_else(|| panic!("{r:?}"));
    assert_eq!(c.name, "Main L ÷ t5");
    let MeasKind::Math { config } = &c.kind else {
        panic!("{c:?}");
    };
    assert_eq!(config.domain, MathDomain::Transfer);
    assert_eq!(
        config.expr,
        MathExpr::Binary {
            a: Operand::Meas { meas: MeasId(1) },
            op: MathOp::Divide,
            b: Operand::Trace { trace: TraceId(5) },
        }
    );
    // From the palette too; with nothing of a kind to pair, it says what is missing.
    t.conn(with_traces(vec![]));
    t.key("Ctrl+K");
    t.text("new math channel");
    t.key("Enter");
    assert!(matches!(t.st.overlay, Overlay::None));
    assert!(
        t.last_toast()
            .contains("two transfer functions, spectra or RTAs"),
        "{}",
        t.last_toast()
    );
}

/// The selected math channel edits in the same dialog: Enter sends `meas.update` with its
/// name kept.
#[test]
fn math_channel_edit() {
    use crate::forms::FormKind;
    use ac2_proto::model::{MathConfig, MathDomain, MathExpr, MathOp, Operand};
    let mut t = T::new();
    let mut s = daemon_state();
    s.traces = vec![stored(4, Some(2), 2)];
    s.measurements.push(meas(
        3,
        "Prediction",
        MeasKind::Math {
            config: MathConfig::of(
                ac2_proto::model::TraceOwner::Imported,
                MathDomain::Transfer,
                MathExpr::Binary {
                    a: Operand::Meas { meas: MeasId(1) },
                    op: MathOp::Add,
                    b: Operand::Trace { trace: TraceId(4) },
                },
            ),
        },
    ));
    t.conn(mirror(s));
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    t.key("Ctrl+K");
    t.text("edit the selected math");
    t.key("Enter");
    assert!(matches!(&t.st.overlay, Overlay::Form(f) if f.kind == FormKind::MathEdit));
    // Operator: + → − (complex difference).
    t.key("Down");
    t.key("Right");
    let r = t.key("Enter");
    let [
        Request::Call {
            cmd: Command::MeasUpdate { meas, config },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}");
    };
    assert_eq!(*meas, MeasId(3));
    assert_eq!(config.name, "Prediction");
    let MeasKind::Math { config } = &config.kind else {
        panic!()
    };
    assert!(matches!(
        config.expr,
        MathExpr::Binary {
            op: MathOp::Subtract,
            ..
        }
    ));
}

#[test]
fn z_loads_a_target_curve_file() {
    let mut t = T::new();
    t.type_key("Z", "z");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::ImportFile(ImportRole::Target) && p.text.is_empty()
    ));
    t.text("/home/foh/house.txt");
    let r = t.key("Enter");
    assert!(matches!(
        r.as_slice(),
        [Request::Import { path, role: ImportRole::Target }] if path == std::path::Path::new("/home/foh/house.txt")
    ));
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn sessions_from_the_palette() {
    let mut t = T::new();
    t.key("Ctrl+K");
    t.text("session save");
    t.key("Enter");
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.kind == PromptKind::SessionSave));
    t.text("friday show");
    let r = t.key("Enter");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::FileSave {
                session: SessionRef::Name { name }
            },
            ..
        }] if name == "friday show"
    ));
    t.key("Ctrl+K");
    t.text("session load");
    t.key("Enter");
    t.text("./shows/fri");
    let r = t.key("Enter");
    let [
        Request::Call {
            cmd:
                Command::FileLoad {
                    session: SessionRef::Path { path },
                },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}")
    };
    assert!(std::path::Path::new(path).is_absolute());
    assert!(path.ends_with("fri"));
}

#[test]
fn stored_trace_metadata_follows_the_mirror() {
    let mut t = T::new();
    let meta = stored(9, Some(1), 2);
    t.conn(with_traces(vec![meta.clone()]));
    let data = TraceData {
        meta: meta.clone(),
        mag_db: vec![0.0; 3],
        phase_deg: None,
        coherence: None,
        sweep: None,
        ir: None,
    };
    t.conn(ConnEvent::Trace(
        Arc::new(data),
        Arc::new(ac2_proto::GridDef::Log {
            ppo: 1,
            k_min: 0,
            k_max: 2,
        }),
    ));
    let mut hidden = meta.clone();
    hidden.edit.visible = false;
    hidden.edit.slot = None;
    t.conn(with_traces(vec![hidden.clone()]));
    assert_eq!(t.st.traces[&TraceId(9)].0.meta, hidden);
    t.conn(with_traces(vec![]));
    assert!(t.st.traces.is_empty());
}

#[test]
fn palette_runs_commands() {
    let mut t = T::new();
    t.key("Ctrl+K");
    t.text("group");
    t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(matches!(t.st.view.tf.phase, PhaseView::GroupDelay { .. }));
    // Keys do not act while the palette is open; arrows move the highlight.
    t.key("Ctrl+K");
    let r = t.type_key("X", "x");
    assert!(r.is_empty());
    t.key("Down");
    match &t.st.overlay {
        Overlay::Palette(p) => {
            assert_eq!(p.query, "x");
            assert_eq!(p.selected, 1);
        }
        o => panic!("{o:?}"),
    }
    // Ctrl+K again closes.
    t.key("Ctrl+K");
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn navigation_animates_values_do_not() {
    let mut t = T::new();
    let before = t.st.view.freq;
    t.key("I");
    // The target moves at once; the drawn range follows over a few frames.
    assert_ne!(t.st.nav.target, before);
    assert_eq!(t.st.view.freq, before);
    t.st.update(
        Msg::Tick {
            now_s: 0.016,
            dt_s: 0.016,
        },
        &t.keys.clone(),
    );
    let mid = t.st.view.freq;
    assert!(mid != before && mid != t.st.nav.target);
    assert!(t.st.animating());
    for i in 0..100 {
        let now = 0.016 * f64::from(i + 2);
        t.st.update(
            Msg::Tick {
                now_s: now,
                dt_s: 0.016,
            },
            &t.keys.clone(),
        );
    }
    assert!(!t.st.animating());
    assert_eq!(t.st.view.freq, t.st.nav.target);
    // Drag pans directly.
    t.st.update(Msg::Pan { octaves: 1.0 }, &t.keys.clone());
    assert!(!t.st.animating());
    t.key("Home");
    assert_eq!(t.st.nav.target, ac2_scene::view::FreqRange::default());
}

#[test]
fn cursor_keys() {
    let mut t = T::new();
    t.st.update(Msg::Command(CommandId::ToggleCursor), &t.keys);
    let hz = t.st.view.cursor_hz.expect("cursor");
    assert!((hz - (20.0f64 * 20_000.0).sqrt()).abs() < 1e-9);
    t.key("Shift+Right");
    let up = t.st.view.cursor_hz.expect("cursor");
    assert!((up / hz - 2f64.powf(1.0 / 12.0)).abs() < 1e-12);
    t.st.update(Msg::Command(CommandId::ToggleCursor), &t.keys);
    assert_eq!(t.st.view.cursor_hz, None);
}

#[test]
fn toasts_expire() {
    let mut t = T::new();
    t.key("M");
    assert!(!t.st.toasts.is_empty());
    t.st.update(
        Msg::Tick {
            now_s: 100.0,
            dt_s: 0.016,
        },
        &t.keys.clone(),
    );
    assert!(t.st.toasts.is_empty());
}

#[test]
fn toast_time_follows_the_text_and_the_severity() {
    let mut t = T::new();
    t.st.toast("t13 selected");
    t.st.warn("no shown stored traces (1 … 9 show a slot; Alt+V reaches hidden traces)");
    t.st.fault("sweep failed: the audio stopped");
    let until: Vec<f64> = t.st.toasts.iter().map(|x| x.until_s).collect();
    let sev: Vec<Severity> = t.st.toasts.iter().map(|x| x.severity).collect();
    assert_eq!(sev, [Severity::Info, Severity::Warning, Severity::Fault]);
    for x in &t.st.toasts {
        assert_eq!(x.until_s, ac2_scene::toast::duration_s(x.severity, &x.text));
    }
    assert!(until[0] < until[1] && until[1] < until[2], "{until:?}");
    // The info goes first, the error stays longest.
    t.tick(until[0] + 0.1);
    assert_eq!(t.st.toasts.len(), 2);
    t.tick(until[1] + 0.1);
    assert_eq!(t.last_toast(), "sweep failed: the audio stopped");
}

#[test]
fn hovering_holds_the_toasts_and_a_click_dismisses_one() {
    let mut t = T::new();
    t.st.toast("first");
    t.st.toast("second");
    let until = t.st.toasts[0].until_s;
    t.tick(1.0);
    let keys = t.keys.clone();
    t.st.update(Msg::ToastsHeld(true), &keys);
    // Held far past their time: still there, with the time they had left.
    t.tick(until + 30.0);
    assert_eq!(t.st.toasts.len(), 2);
    t.st.update(Msg::ToastsHeld(false), &keys);
    // They had `until - 1` left when the pointer came.
    let gone = until + 30.0 + (until - 1.0);
    t.tick(gone - 0.1);
    assert_eq!(t.st.toasts.len(), 2, "the time held does not count");
    t.tick(gone + 0.1);
    assert!(t.st.toasts.is_empty());
    // A click takes exactly that one away.
    t.st.toast("a");
    t.st.toast("b");
    let a = t.st.toasts[0].id;
    t.st.update(Msg::DismissToast(a), &keys);
    assert_eq!(t.st.toasts.len(), 1);
    assert_eq!(t.last_toast(), "b");
}

#[test]
fn the_same_message_replaces_the_one_up_and_counts_in_the_log() {
    let mut t = T::new();
    t.st.warn("not connected");
    t.st.toast("t13 selected");
    t.tick(2.0);
    t.st.warn("not connected");
    let texts: Vec<&str> = t.st.toasts.iter().map(|x| x.text.as_str()).collect();
    assert_eq!(texts, ["t13 selected", "not connected"]);
    t.st.warn("not connected");
    let n = t.st.notices.back().cloned();
    assert_eq!(n.as_ref().map(|n| n.count), Some(2));
    assert_eq!(t.st.notices.len(), 3);
}

#[test]
fn the_notification_log_keeps_the_last_ones_and_opens_from_the_palette() {
    let mut t = T::new();
    for i in 0..(ac2_scene::toast::LOG_LEN + 10) {
        t.st.toast(format!("message {i}"));
    }
    assert_eq!(t.st.notices.len(), ac2_scene::toast::LOG_LEN);
    assert_eq!(
        t.st.notices.front().map(|n| n.text.as_str()),
        Some("message 10")
    );
    assert!(t.st.toasts.len() <= MAX_TOASTS);
    // Expired toasts are still in the log.
    t.tick(1000.0);
    assert!(t.st.toasts.is_empty());
    assert_eq!(t.st.notices.len(), ac2_scene::toast::LOG_LEN);
    // The palette finds it by name and opens the window; the keys scroll it, Esc closes.
    let keys = t.keys.clone();
    let scope = t.focus_kind().scope();
    let mut p = crate::palette::Palette::default();
    p.type_text("recent notif");
    assert_eq!(
        p.entries(&keys, scope).first().map(|e| e.command),
        Some(CommandId::Notifications)
    );
    t.st.update(Msg::Command(CommandId::Notifications), &keys);
    assert_eq!(t.st.overlay, Overlay::Notifications);
    t.key("Down");
    assert_eq!(t.st.help_scroll, crate::state::HELP_LINE);
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
}

/// Warning toasts off: a warning goes only to the log, an error and information still pop
/// up; on again, warnings pop up again. The palette and its toast name it the same way.
#[test]
fn warning_toasts_off_keeps_warnings_in_the_log_only() {
    let mut t = T::new();
    assert!(t.st.prefs.warning_toasts);
    let keys = t.keys.clone();
    let scope = t.focus_kind().scope();
    let mut p = crate::palette::Palette::default();
    p.type_text("warning toasts");
    assert_eq!(
        p.entries(&keys, scope).first().map(|e| e.command),
        Some(CommandId::WarningToasts)
    );
    t.st.prefs_dirty = false;
    t.st.update(Msg::Command(CommandId::WarningToasts), &keys);
    assert!(!t.st.prefs.warning_toasts);
    assert!(t.st.prefs_dirty);
    assert_eq!(
        t.last_toast(),
        "warning toasts off: warnings and Leq limit alarms go only to the notification log"
    );
    let toasts = t.st.toasts.len();
    t.st.warn("no stimulus level yet");
    assert_eq!(t.st.toasts.len(), toasts, "a warning does not pop up");
    let n = t.st.notices.back().expect("logged");
    assert_eq!(n.text, "no stimulus level yet");
    assert_eq!(n.severity, Severity::Warning);
    t.st.fault("the link went down");
    assert_eq!(t.last_toast(), "the link went down");
    t.st.toast("saved");
    assert_eq!(t.last_toast(), "saved");
    t.st.update(Msg::Command(CommandId::WarningToasts), &keys);
    assert!(t.st.prefs.warning_toasts);
    t.st.warn("no stimulus level yet");
    assert_eq!(t.last_toast(), "no stimulus level yet");
}
