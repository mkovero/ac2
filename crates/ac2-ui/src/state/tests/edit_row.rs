//! Editing from the measurement tree: F2 on the row selected last, a row's double click and
//! its edit button.

use ac2_scene::meas_list::TreeKey;

use super::*;

/// The tree of `tree_state` with an SPL meter, which no pane shows.
fn t() -> T {
    let mut s = tree_state();
    s.measurements.push(meas(7, "SPL A", spl_meter()));
    let mut t = T::new();
    t.conn(mirror(s));
    t
}

/// The measurement whose form is open, a math trace's dialog included.
fn editing(t: &T) -> Option<MeasId> {
    match &t.st.overlay {
        Overlay::Form(f) => f.edits(),
        _ => None,
    }
}

/// The meter Settings is open on at its SPL/Leq page.
fn leq_page(t: &T) -> Option<(MeasId, String)> {
    match &t.st.overlay {
        Overlay::Settings(s) if s.page == crate::settings::Page::Leq => {
            s.leq.as_ref().map(|d| (d.meas, d.name.clone()))
        }
        _ => None,
    }
}

/// F2 edits the row selected last: a measurement's form, a stored trace's name.
#[test]
fn f2_edits_a_measurement_and_renames_a_trace() {
    let mut t = t();
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("F2");
    assert_eq!(editing(&t), Some(MeasId(1)), "the transfer's form");
    t.st.overlay = Overlay::None;
    // A trace selected after it has the key.
    t.st.apply(Msg::SelectTrace(TraceId(3)), &t.keys);
    t.key("F2");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceRename(TraceId(3)) && p.text == "pre-EQ"
    ));
    t.st.overlay = Overlay::None;
    // A math trace is a measurement: its dialog.
    t.st.apply(Msg::SelectMath(MeasId(6)), &t.keys);
    t.key("F2");
    assert_eq!(editing(&t), Some(MeasId(6)), "the math dialog");
    t.st.overlay = Overlay::None;
    // The key is the command the palette runs, on either kind of row.
    t.st.apply(Msg::SelectLive(MeasId(2)), &t.keys);
    t.st.apply(Msg::Command(CommandId::EditSelected), &t.keys);
    assert_eq!(
        editing(&t),
        Some(MeasId(2)),
        "a live-curve row is its measurement's"
    );
}

/// A double click on a measurement's row (or its edit button) selects the row as a click
/// does and opens its form, whatever was selected and whatever pane has the keys.
#[test]
fn a_row_double_clicked_is_selected_and_edited() {
    let mut t = t();
    t.st.apply(Msg::SelectTrace(TraceId(3)), &t.keys);
    t.st.apply(Msg::EditRow(TreeKey::Meas(MeasId(2))), &t.keys);
    assert_eq!(t.st.rig.selected, Some(MeasId(2)));
    assert_eq!(
        t.st.rig.selected_trace, None,
        "the keys are the measurement's"
    );
    assert_eq!(editing(&t), Some(MeasId(2)), "the spectrum's form");
    t.st.overlay = Overlay::None;
    t.st.apply(Msg::EditRow(TreeKey::Math(MeasId(6))), &t.keys);
    assert_eq!(t.st.rig.selected, Some(MeasId(6)));
    assert_eq!(editing(&t), Some(MeasId(6)), "the math dialog");
    t.st.overlay = Overlay::None;
    t.st.apply(Msg::EditRow(TreeKey::Live(MeasId(1))), &t.keys);
    assert_eq!(
        t.st.rig.selected_live,
        Some(MeasId(1)),
        "the live row stays the pick"
    );
    assert_eq!(editing(&t), Some(MeasId(1)));
    t.st.overlay = Overlay::None;
    // An SPL meter is edited on Settings › SPL/Leq: the page on the row's meter, though
    // no SPL pane shows it.
    t.st.apply(Msg::EditRow(TreeKey::Meas(MeasId(7))), &t.keys);
    assert_eq!(t.st.rig.selected, Some(MeasId(7)));
    assert_eq!(leq_page(&t), Some((MeasId(7), "SPL A".to_owned())));
    t.st.overlay = Overlay::None;
    // The key and the palette's command open the same on the selected meter.
    t.key("F2");
    assert_eq!(leq_page(&t), Some((MeasId(7), "SPL A".to_owned())));
    t.st.overlay = Overlay::None;
    t.st.apply(Msg::Command(CommandId::EditMeas), &t.keys);
    assert_eq!(leq_page(&t), Some((MeasId(7), "SPL A".to_owned())));
    t.st.overlay = Overlay::None;
    // A heading stands for no measurement.
    t.st.apply(Msg::EditRow(TreeKey::Session), &t.keys);
    assert_eq!(t.st.rig.selected, Some(MeasId(7)));
    assert_eq!(t.st.overlay, Overlay::None);
}

/// A row names its measurement, so a focused Programme pane does not turn the edit away as
/// it does the command (which reads what is in focus).
#[test]
fn a_row_is_edited_with_a_programme_pane_in_focus() {
    let mut t = t();
    let p = t.st.layout.focus;
    t.drag_drop(DragItem::Input(0), p, None);
    assert_eq!(t.st.layout.focus_kind(), PaneKind::Programme);
    t.st.apply(Msg::Command(CommandId::EditMeas), &t.keys);
    assert_eq!(t.last_toast(), ac2_scene::pane_edit::PROGRAMME_EDIT);
    t.st.apply(Msg::EditRow(TreeKey::Meas(MeasId(1))), &t.keys);
    assert_eq!(editing(&t), Some(MeasId(1)));
}

/// Open Settings owns the keys: the edit key goes to its page, so the SPL/Leq page stays
/// on the meter it was opened on and what is typed there is kept.
#[test]
fn the_edit_key_is_the_open_settings_views() {
    let mut t = t();
    t.st.apply(Msg::EditRow(TreeKey::Meas(MeasId(7))), &t.keys);
    let before = t.st.overlay.clone();
    assert_eq!(leq_page(&t), Some((MeasId(7), "SPL A".to_owned())));
    t.key("F2");
    assert_eq!(t.st.overlay, before);
}
