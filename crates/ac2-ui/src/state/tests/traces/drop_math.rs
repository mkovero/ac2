//! Reducer tests of a list row dropped onto another: the math dialog opens prefilled with
//! the operator the dragged curve asks for, a pair that does not combine is refused with a
//! toast, and an EQ set's row opens, edits and deletes the set.

use super::*;
use crate::forms::FormKind;
use ac2_proto::model::{
    EqBody, EqEdit, EqRealisation, EqSet, MathExpr, MathOp, Operand, PeakQ, TargetCurve,
};
use ac2_proto::units::{EqSetId, Phon};
use ac2_scene::meas_list::TreeKey;

fn owned(id: u32, name: &str, meas: u32) -> TraceMeta {
    let mut t = stored(id, None, 2);
    t.edit.name = name.into();
    t.edit.owner = TraceOwner::Meas { meas: MeasId(meas) };
    t
}

/// Main L (1) and Delay tower (3), transfer functions, and Sub (2), a spectrum. Main L
/// holds a capture (10), a loudness shift (12), a flat target (13) and EQ 1 (its curve 9);
/// Delay tower a capture (11).
fn rig() -> T {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.push(meas(3, "Delay tower", transfer()));
    let m = s
        .measurements
        .iter_mut()
        .find(|m| m.id == MeasId(1))
        .expect("Main L");
    m.eq = vec![EqSet {
        id: EqSetId(1),
        rev: 1,
        body: EqBody {
            name: "EQ 1".into(),
            realisation: EqRealisation::Digital {
                rate: ac2_proto::units::Hz(48_000.0),
            },
            peak_q: PeakQ::HalfGain,
            enabled: true,
            sections: Vec::new(),
        },
        in_console: None,
        trace: TraceId(9),
    }];
    let mut eq = owned(9, "EQ 1", 1);
    eq.edit.locked = true;
    eq.source = TraceSource::Eq {
        meas: MeasId(1),
        set: EqSetId(1),
        rev: 1,
    };
    let mut loud = owned(12, "loudness 85 → 100 phon", 1);
    loud.kind = TraceKind::Target;
    loud.source = TraceSource::Generated {
        curve: TargetCurve::LoudnessShift {
            from: Phon(85.0),
            to: Phon(100.0),
        },
    };
    let mut flat = owned(13, "flat", 1);
    flat.kind = TraceKind::Target;
    flat.source = TraceSource::Generated {
        curve: TargetCurve::Flat,
    };
    s.traces = vec![
        eq,
        owned(10, "Main L S1", 1),
        owned(11, "Delay tower S1", 3),
        loud,
        flat,
    ];
    t.conn(mirror(s));
    t
}

fn drop(t: &mut T, dragged: TreeKey, onto: TreeKey) -> Vec<Request> {
    let mut r = t.st.apply(Msg::RowDrag(DragMsg::Start(dragged)), &t.keys);
    r.extend(t.st.apply(Msg::RowDrag(DragMsg::OverRow(Some(onto))), &t.keys));
    r.extend(t.st.apply(Msg::RowDrag(DragMsg::Drop { compare: false }), &t.keys));
    r
}

/// The math channel Enter makes from the open dialog: its name, owner and expression.
fn made(t: &mut T) -> (String, TraceOwner, MathExpr) {
    assert!(
        matches!(&t.st.overlay, Overlay::Form(f) if f.kind == FormKind::Math),
        "{:?}",
        t.st.overlay
    );
    let r = t.key("Enter");
    let [Request::CreateMeas { config }] = r.as_slice() else {
        panic!("{r:?}");
    };
    let MeasKind::Math { config: m } = &config.kind else {
        panic!("{config:?}");
    };
    (config.name.clone(), m.owner, m.expr.clone())
}

fn binary(a: Operand, op: MathOp, b: Operand) -> MathExpr {
    MathExpr::Binary { a, op, b }
}

fn tr(id: u32) -> Operand {
    Operand::Trace { trace: TraceId(id) }
}

const MAIN_L: TraceOwner = TraceOwner::Meas { meas: MeasId(1) };

#[test]
fn a_curve_dropped_on_a_trace_opens_the_math_dialog_with_its_reading() {
    // A loudness shift acts on the curve: ×, named as the expression reads.
    let mut t = rig();
    t.st.apply(
        Msg::RowDrag(DragMsg::Start(TreeKey::Trace(TraceId(12)))),
        &t.keys,
    );
    t.st.apply(
        Msg::RowDrag(DragMsg::OverRow(Some(TreeKey::Trace(TraceId(10))))),
        &t.keys,
    );
    assert_eq!(
        t.st.drag_label().as_deref(),
        Some("math channel: Main L S1 × loudness 85 → 100 phon")
    );
    t.st.apply(Msg::RowDrag(DragMsg::Drop { compare: false }), &t.keys);
    assert_eq!(
        made(&mut t),
        (
            "Main L S1 × loudness 85 → 100 phon".into(),
            MAIN_L,
            binary(tr(10), MathOp::Multiply, tr(12))
        )
    );
    // A target is read against: ÷.
    let mut t = rig();
    assert!(
        drop(
            &mut t,
            TreeKey::Trace(TraceId(13)),
            TreeKey::Trace(TraceId(10))
        )
        .is_empty()
    );
    assert_eq!(made(&mut t).2, binary(tr(10), MathOp::Divide, tr(13)));
    // An EQ set filters: ×, the set as its operand, named as P names a prediction.
    let mut t = rig();
    drop(
        &mut t,
        TreeKey::Trace(TraceId(9)),
        TreeKey::Trace(TraceId(10)),
    );
    let eq = Operand::Eq {
        meas: MeasId(1),
        set: EqSetId(1),
    };
    assert_eq!(
        made(&mut t),
        (
            "Main L S1 after EQ 1".into(),
            MAIN_L,
            binary(tr(10), MathOp::Multiply, eq)
        )
    );
    // Either way round: the capture dropped on the loudness shift is still A.
    let mut t = rig();
    drop(
        &mut t,
        TreeKey::Trace(TraceId(10)),
        TreeKey::Trace(TraceId(12)),
    );
    assert_eq!(made(&mut t).2, binary(tr(10), MathOp::Multiply, tr(12)));
}

/// Another measurement's trace dropped on a capture compares the two (÷), listed under the
/// capture's measurement; Esc makes nothing.
#[test]
fn a_trace_dropped_across_measurements_is_listed_with_the_row_dropped_on() {
    let mut t = rig();
    drop(
        &mut t,
        TreeKey::Trace(TraceId(11)),
        TreeKey::Trace(TraceId(10)),
    );
    assert_eq!(
        made(&mut t),
        (
            "Main L S1 ÷ Delay tower S1".into(),
            MAIN_L,
            binary(tr(10), MathOp::Divide, tr(11))
        )
    );
    let mut t = rig();
    drop(
        &mut t,
        TreeKey::Trace(TraceId(10)),
        TreeKey::Live(MeasId(3)),
    );
    let (_, owner, expr) = made(&mut t);
    assert_eq!(owner, TraceOwner::Meas { meas: MeasId(3) });
    assert_eq!(
        expr,
        binary(Operand::Meas { meas: MeasId(3) }, MathOp::Divide, tr(10))
    );
    let mut t = rig();
    drop(
        &mut t,
        TreeKey::Trace(TraceId(11)),
        TreeKey::Trace(TraceId(10)),
    );
    assert!(t.key("Esc").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn a_drop_that_does_not_combine_says_why_and_opens_nothing() {
    let mut t = rig();
    t.st.apply(
        Msg::RowDrag(DragMsg::Start(TreeKey::Live(MeasId(2)))),
        &t.keys,
    );
    t.st.apply(
        Msg::RowDrag(DragMsg::OverRow(Some(TreeKey::Trace(TraceId(10))))),
        &t.keys,
    );
    let why = "Sub is a spectrum: it does not combine with Main L S1, a transfer function";
    assert_eq!(t.st.drag_label().as_deref(), Some(why));
    assert!(
        t.st.apply(Msg::RowDrag(DragMsg::Drop { compare: false }), &t.keys)
            .is_empty()
    );
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.last_toast(), why);
    // An EQ set has nothing to filter in another EQ set.
    let mut t = rig();
    drop(
        &mut t,
        TreeKey::Trace(TraceId(9)),
        TreeKey::Trace(TraceId(9)),
    );
    assert_eq!(t.st.overlay, Overlay::None);
}

/// An EQ set's row is the set: a double click or Alt+E opens the editor on it, Delete asks
/// and then deletes the set (its curve goes with it).
#[test]
fn an_eq_sets_row_opens_and_deletes_the_set() {
    let mut t = rig();
    t.st.apply(Msg::RenameTrace(TraceId(9)), &t.keys);
    assert!(
        matches!(&t.st.overlay, Overlay::Eq(e) if e.meas == MeasId(1) && e.set == Some(EqSetId(1))),
        "{:?}",
        t.st.overlay
    );
    t.key("Esc");
    t.st.apply(Msg::SelectTrace(TraceId(9)), &t.keys);
    t.key("Alt+E");
    assert!(
        matches!(&t.st.overlay, Overlay::Eq(e) if e.set == Some(EqSetId(1))),
        "{:?}",
        t.st.overlay
    );
    t.key("Esc");
    t.st.apply(Msg::SelectTrace(TraceId(9)), &t.keys);
    t.key("Delete");
    let Overlay::Delete(p) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(p.confirm.title, "Delete EQ 1?");
    let r = t.key("Delete");
    assert!(
        matches!(
            calls(&r).as_slice(),
            [(
                Command::MeasEq {
                    meas: MeasId(1),
                    edit: EqEdit::Delete { set: EqSetId(1) },
                },
                "EQ 1 deleted"
            )]
        ),
        "{r:?}"
    );
}

/// In the math dialog an EQ set is a B operand only: it filters a curve, it is none.
#[test]
fn the_math_dialog_offers_an_eq_set_as_b_only() {
    let mut t = rig();
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("Shift+M");
    let row = |t: &T, label: &str| match &t.st.overlay {
        Overlay::Form(f) => f
            .fields
            .iter()
            .find(|x| x.label == label)
            .map(|x| x.display())
            .unwrap_or_default(),
        o => panic!("{o:?}"),
    };
    let mut seen_a = Vec::new();
    for _ in 0..12 {
        seen_a.push(row(&t, "A"));
        t.key("Right");
    }
    assert!(!seen_a.iter().any(|a| a.contains("EQ")), "{seen_a:?}");
    t.key("Down");
    t.key("Down");
    let mut seen_b = Vec::new();
    for _ in 0..12 {
        seen_b.push(row(&t, "B"));
        t.key("Right");
    }
    assert!(
        seen_b.iter().any(|b| b == "EQ 1 (EQ set of Main L)"),
        "{seen_b:?}"
    );
}
