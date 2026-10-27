//! Reducer tests of deleting the selected stored trace or measurement: the question
//! first, its answers, and what a prompt does with Backspace.

use super::*;

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
    t.st.apply(Msg::SelectTrace(TraceId(10)), &t.keys);
    assert!(t.key("Delete").is_empty());
    let Overlay::Delete(p) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(p.target, DeleteTarget::Trace(TraceId(10)));
    assert_eq!(p.confirm.title, "Delete t10?");
    assert_eq!(p.confirm.lines[0], "capture · slot 1 · no data yet");
    assert_eq!(
        p.confirm.hint,
        "Delete or Backspace deletes it · Esc or N keeps it"
    );
    // Esc keeps it (the stimulus untouched); so does N.
    assert!(t.key("Esc").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(
        t.st.rig.selected_trace,
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
    assert_eq!(t.st.rig.selected_trace, Some(TraceId(11)));
    assert_eq!(t.st.overlay, Overlay::None);
    // Enter, the stimulus's key, deletes nothing; Delete again does, past a hidden trace
    // to the next shown one.
    t.key("Delete");
    assert!(t.key("Enter").is_empty());
    assert!(matches!(t.st.overlay, Overlay::Delete(_)));
    let (id, what) = deleted(&t.key("Delete")).expect("deleted");
    assert_eq!(
        (id, what.as_str()),
        (TraceId(11), "t11 deleted · t14 selected")
    );
    // The last shown one: the selection goes back to the one before it.
    t.conn(with_traces(vec![stored(11, None, 2), stored(14, None, 2)]));
    assert_eq!(t.st.rig.selected_trace, Some(TraceId(14)));
    t.key("Delete");
    let (_, what) = deleted(&t.st.apply(Msg::Delete(true), &t.keys)).expect("deleted");
    assert_eq!(what, "t14 deleted · t11 selected");
    // The only one, with Backspace twice (keyboards without Delete): back to the live
    // measurement.
    t.conn(with_traces(vec![stored(11, None, 2)]));
    t.key("Backspace");
    let (_, what) = deleted(&t.st.apply(Msg::Backspace, &t.keys)).expect("deleted");
    assert_eq!(what, "t11 deleted · keys act on the live measurement");
    assert_eq!(t.st.rig.selected_trace, None);
    // The mouse's Keep.
    t.conn(with_traces(vec![stored(11, None, 2)]));
    t.st.apply(Msg::SelectTrace(TraceId(11)), &t.keys);
    t.key("Delete");
    assert!(t.st.apply(Msg::Delete(false), &t.keys).is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    // A locked trace refuses before asking.
    let mut locked = stored(15, None, 2);
    locked.edit.locked = true;
    t.conn(with_traces(vec![locked]));
    t.st.apply(Msg::SelectTrace(TraceId(15)), &t.keys);
    assert!(t.key("Delete").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.last_toast(), "t15 is locked: it is not deleted");
    // The palette's entry asks the same.
    t.conn(with_traces(vec![stored(11, None, 2)]));
    t.st.apply(Msg::SelectTrace(TraceId(11)), &t.keys);
    t.key("Ctrl+K");
    t.text("delete selected");
    t.key("Enter");
    assert!(
        matches!(&t.st.overlay, Overlay::Delete(p) if p.target == DeleteTarget::Trace(TraceId(11)))
    );
}

/// Delete and Backspace act on what was selected last: a measurement asks first (naming
/// it and what goes), Esc or N keeps it, Delete / Backspace deletes it (Enter does not); a stored
/// trace selected after it is what they delete instead.
#[test]
fn delete_and_backspace_ask_then_delete_the_selected_measurement() {
    let mut t = T::new();
    t.conn(with_traces(vec![stored(10, Some(1), 2)]));
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
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
    // Enter deletes nothing.
    t.key("Delete");
    assert!(t.key("Enter").is_empty());
    assert!(matches!(t.st.overlay, Overlay::Delete(_)));
    t.key("Esc");
    // Delete then Backspace, Backspace twice, Delete twice: each deletes it.
    for (first, second) in [
        ("Delete", "Backspace"),
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
    t.st.apply(Msg::SelectTrace(TraceId(10)), &t.keys);
    t.key("Backspace");
    assert!(
        matches!(&t.st.overlay, Overlay::Delete(p) if p.target == DeleteTarget::Trace(TraceId(10))),
        "{:?}",
        t.st.overlay
    );
    t.key("Esc");
    // Selecting the measurement again gives it the keys back.
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
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
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
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
    assert!(t.st.apply(Msg::Delete(true), &t.keys).is_empty());
    // The math channel itself goes after a confirmation.
    t.st.apply(Msg::SelectMeas(MeasId(3)), &t.keys);
    t.key("Delete");
    assert_eq!(
        meas_deleted(&t.key("Delete")),
        Some((MeasId(3), "Sum deleted".to_owned()))
    );
}

/// An open window owns Backspace: in a prompt it erases typed text and never deletes the
/// selected measurement behind it.
#[test]
fn backspace_in_a_prompt_edits_its_text() {
    let mut t = T::new();
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.type_key("D", "d");
    t.text("12");
    assert!(t.key("Backspace").is_empty());
    assert!(t.st.apply(Msg::Backspace, &t.keys).is_empty());
    let Overlay::Prompt(p) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    // Prefilled with the delay in use (12.5 ms): the two typed digits are gone.
    assert_eq!(p.text, "12.5");
    assert!(t.key("Backspace").is_empty());
    assert!(matches!(t.st.overlay, Overlay::Prompt(_)));
}
