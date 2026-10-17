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
    // none of them needs (a sweep pane here), then the New measurement chooser.
    let spl = t.pane(PaneKind::Spl);
    t.st.update(Msg::PaneMenu(spl), &t.keys);
    assert!(matches!(t.st.overlay, Overlay::PaneMenu(m) if m.pane == spl && m.index == 0));
    let mut want: Vec<PaneMenuRow> =
        t.st.tree_meas_order()
            .into_iter()
            .map(PaneMenuRow::Meas)
            .collect();
    want.push(PaneMenuRow::Kind(PaneKind::Distortion));
    want.push(PaneMenuRow::NewMeasurement);
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
                cmd: Command::MeasUpdate { meas, config, .. },
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
    // Tab steps to the next row of the list, a hidden trace's too (a click selects it).
    t.key("V");
    t.key("Tab");
    assert_eq!(t.st.selected_trace, Some(TraceId(11)));
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
        "select a measurement or a stored trace first (click it in the list, Tab, V)"
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
        level: ac2_proto::model::SpecLevel::Tone,
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
    // A spectrum trace's row: the focused pane turns into a spectrum pane to draw it.
    t.st.update(Msg::SelectTrace(TraceId(16)), &t.keys);
    assert_eq!(t.st.layout.focus_kind(), PaneKind::Spectrum);
    // The pane back to a transfer pane, which does not draw it: the keys act on the live
    // measurement.
    let f = t.st.layout.focus;
    t.st.update(
        Msg::PanePick(f, PaneMenuRow::Kind(PaneKind::Transfer)),
        &t.keys,
    );
    assert!(t.key("U").is_empty());
    assert!(t.st.edit(MeasId(1)).inverted);
}

/// One selection: a sweep selected in the transfer pane is what the sweep pane shows, and
/// N on the sweep pane selects the sweep it steps to.
#[test]
fn the_sweep_pane_follows_the_selection_and_selects() {
    let mut t = T::new();
    // The capture is Main L's: the tree (and V) list it before the sweep's runs.
    let mut capture = stored(10, Some(1), 2);
    capture.edit.owner = TraceOwner::Meas { meas: MeasId(1) };
    t.conn(with_sweeps(vec![capture, sweep_meta(14), sweep_meta(15)]));
    for id in [14, 15] {
        let (d, g) = sweep_data(id);
        t.conn(ConnEvent::Trace(d, g));
    }
    let shown = |t: &T| t.st.kind_sweep().map(|(d, _)| d.meta.id.0);
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

#[path = "state_traces_smoothing_tests.rs"]
mod smoothing;

#[path = "state_traces_math_tests.rs"]
mod math;

#[path = "state_traces_import_tests.rs"]
mod import;

#[path = "state_traces_toast_tests.rs"]
mod toast;
