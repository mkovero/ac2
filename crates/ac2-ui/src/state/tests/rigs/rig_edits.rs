//! Each rig's measurement edits wait in that rig's own chains: both daemons number their
//! measurements from 1, so an edit counted on the wrong rig would hold back the other's.

use super::*;

fn meas_calls(r: &[(RigId, Request)]) -> Vec<(RigId, &Command)> {
    r.iter()
        .filter_map(|(rig, q)| match q {
            Request::Call { cmd, .. } if crate::conn::meas_edit_target(cmd).is_some() => {
                Some((*rig, cmd))
            }
            _ => None,
        })
        .collect()
}

/// A split dragged on one rig and another rig shown before the drag rests: the layout goes
/// to the rig it was dragged on, is answered there, and the edits of the same measurement
/// id on the rig now shown go at once, to that rig.
#[test]
fn a_drag_pending_at_a_switch_stays_with_its_rig() {
    let mut t = two_mirrored();
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.tick(1.0);
    t.st.apply(
        Msg::DragSplit {
            path: vec![],
            ratio: 0.6,
        },
        &t.keys,
    );
    let r = pick(&mut t, RigRow::Rig(PUPU));
    assert_eq!(t.st.active(), PUPU);
    // Each rig's layout of its measurement 1: the one dragged, and pupu's first screen.
    let layouts: Vec<(RigId, f64, String)> = r
        .iter()
        .filter_map(|(rig, q)| match q {
            Request::Call {
                cmd:
                    Command::MeasLayout {
                        layout: Some(PaneLayout::Split { ratio, .. }),
                        ..
                    },
                what,
            } => Some((*rig, *ratio, what.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(layouts.len(), 2, "{r:?}");
    let dragged = layouts
        .iter()
        .find(|l| (l.1 - 0.6).abs() < 1e-6)
        .expect("the drag");
    assert_eq!(dragged.0, RIG, "the drag goes to the rig it was made on");
    let own = layouts
        .iter()
        .find(|l| (l.1 - 0.6).abs() > 1e-6)
        .expect("pupu's own");
    assert_eq!(own.0, PUPU);
    let (what, pupu_what) = (dragged.2.clone(), own.2.clone());
    to(
        &mut t,
        PUPU,
        ConnEvent::MeasEdited {
            meas: MeasId(1),
            link: 0,
            what: pupu_what,
            result: Ok(None),
        },
    );

    // On pupu, measurement 1: a whole-configuration edit goes at once, to pupu.
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.go(PaneKind::Transfer);
    let r =
        t.st.update(Msg::Key(Chord::parse("K").expect("key")), &t.keys);
    let updates: Vec<_> = meas_calls(&r)
        .into_iter()
        .filter(|(_, c)| matches!(c, Command::MeasUpdate { .. }))
        .collect();
    assert_eq!(updates.len(), 1, "{r:?}");
    assert_eq!(updates[0].0, PUPU);

    // The layout's answer is the first rig's; pupu's next edit is not held by it.
    to(
        &mut t,
        RIG,
        ConnEvent::MeasEdited {
            meas: MeasId(1),
            link: 0,
            what,
            result: Ok(None),
        },
    );
    let colour =
        t.st.update(Msg::Key(Chord::parse("Shift+C").expect("key")), &t.keys);
    assert!(
        meas_calls(&colour).iter().all(|(rig, _)| *rig == PUPU),
        "{colour:?}"
    );
}
