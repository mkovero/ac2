//! Reducer tests of a dialog's channel list and of the outputs a new sweep plays on.

use super::*;
use crate::forms::FieldId;

fn form(t: &T) -> &crate::forms::Form {
    match &t.st.overlay {
        Overlay::Form(f) => f,
        o => panic!("no dialog: {o:?}"),
    }
}

/// Space on a channel field opens its list; ↑/↓ move in it, Enter picks and closes it; Esc
/// closes the list and leaves the dialog open, a second Esc closes the dialog.
#[test]
fn space_lists_a_channel_field_and_esc_closes_only_the_list() {
    let mut t = T::new();
    t.type_key("Shift+S", "S");
    assert_eq!(form(&t).kind, FormKind::Sweep);
    // The reference has the focus and names nothing yet.
    assert_eq!(form(&t).channel(FieldId::Reference), None);
    assert!(form(&t).keys_hint().contains("Space lists them"));
    t.type_key("Space", " ");
    let l = form(&t).list.clone().expect("list");
    assert!(l.inputs);
    assert_eq!(l.cursor, 0);
    let second = l.rows[1].channel;
    t.key("Down");
    t.key("Enter");
    assert!(form(&t).list.is_none());
    assert_eq!(form(&t).channel(FieldId::Reference), second);
    // Open again: Esc closes the list only.
    t.type_key("Space", " ");
    assert!(form(&t).list.is_some());
    t.key("Escape");
    assert!(form(&t).list.is_none());
    assert_eq!(form(&t).channel(FieldId::Reference), second);
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
}

/// In a text field Space is a space.
#[test]
fn space_in_a_text_field_types() {
    let mut t = T::new();
    t.type_key("Shift+S", "S");
    let Overlay::Form(f) = &mut t.st.overlay else {
        panic!("no dialog");
    };
    let i = f
        .fields
        .iter()
        .position(|x| x.id == FieldId::Name)
        .expect("name");
    f.focus_field(i);
    t.text("a");
    t.type_key("Space", " ");
    t.text("b");
    assert_eq!(form(&t).text(FieldId::Name), "a b");
    assert!(form(&t).list.is_none());
}

/// A new sweep plays on the app's stimulus outputs (the S roles of Inputs & outputs).
#[test]
fn a_new_sweep_plays_on_the_stimulus_outputs() {
    let mut t = T::new();
    t.st.rig.stimulus.outputs = vec![1];
    t.type_key("Shift+S", "S");
    assert_eq!(form(&t).sweep_outputs(), [1]);
    t.key("Escape");
    t.st.rig.stimulus.outputs = vec![0, 1];
    t.type_key("Shift+S", "S");
    assert_eq!(form(&t).sweep_outputs(), [0, 1]);
}
