//! Reducer tests of the EQ editor's `put`s: each replaces the whole body and calls travel
//! independently, so one is outstanding at a time and steps taken meanwhile go as one.

use super::*;
use ac2_proto::model::{EqBody, EqEdit, EqRealisation, EqSet, PeakQ};
use ac2_proto::units::EqSetId;

fn with_set(t: &mut T) {
    t.remirror(|s| {
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
    });
}

/// The bodies the `put`s in `r` send.
fn puts(r: &[Request]) -> Vec<EqBody> {
    calls(r)
        .into_iter()
        .filter_map(|(c, _)| match c {
            Command::MeasEq {
                edit: EqEdit::Put { body, .. },
                ..
            } => Some(body.clone()),
            _ => None,
        })
        .collect()
}

fn answered(t: &mut T, result: Result<(), String>) -> Vec<Request> {
    t.conn(ConnEvent::Reply {
        what: "EQ edit".into(),
        result,
    })
}

#[test]
fn a_second_step_waits_for_the_first_put() {
    let mut t = T::new();
    with_set(&mut t);
    t.go(PaneKind::Transfer);
    t.key("Alt+E");
    assert!(matches!(t.st.overlay, Overlay::Eq(_)), "{:?}", t.st.overlay);

    // Bypassed, then back and bypassed again before the first is answered: one put out,
    // the newest body held.
    let first = puts(&t.key("Shift+B"));
    assert_eq!(first.iter().map(|b| b.enabled).collect::<Vec<_>>(), [false]);
    assert!(puts(&t.key("Shift+B")).is_empty());
    assert!(puts(&t.key("Shift+B")).is_empty());

    // The answer lets the newest go, alone.
    let next = puts(&answered(&mut t, Ok(())));
    assert_eq!(next.iter().map(|b| b.enabled).collect::<Vec<_>>(), [false]);
    assert!(puts(&answered(&mut t, Ok(()))).is_empty());

    // Nothing outstanding: the next step goes at once.
    assert_eq!(puts(&t.key("Shift+B")).len(), 1);
}

#[test]
fn a_refusal_drops_the_steps_queued_on_it() {
    let mut t = T::new();
    with_set(&mut t);
    t.go(PaneKind::Transfer);
    t.key("Alt+E");
    assert_eq!(puts(&t.key("Shift+B")).len(), 1);
    assert!(puts(&t.key("Shift+B")).is_empty());
    assert!(puts(&answered(&mut t, Err("no".into()))).is_empty());
    let Overlay::Eq(e) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(e.refusal.as_deref(), Some("no"));
    assert_eq!((e.pending, e.queued.is_none()), (0, true));
    assert_eq!(puts(&t.key("Shift+B")).len(), 1);
}
