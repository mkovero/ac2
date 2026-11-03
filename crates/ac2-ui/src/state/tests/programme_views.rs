//! Reducer tests of the Programme pane's two views: an Inputs row dragged onto a pane meters
//! it there (a pair on its goniometer), the form opens a pair as two panes, G steps a pair's
//! pane between them, the docked plate floats on its first drag, and the ember rings follow
//! the decay set.

use super::meter_form::{meter_t, rig_with_pair};
use super::*;
use ac2_scene::plate::PlateKind;
use ac2_scene::view::ProgrammeMode as M;

const PAIR: Metered = Metered::Pair { left: 2, right: 3 };

fn programme_panes(t: &T) -> Vec<PaneId> {
    t.st.visible_panes()
        .into_iter()
        .filter(|id| t.st.layout.kind(*id) == PaneKind::Programme)
        .collect()
}

fn mode(t: &T, p: PaneId) -> M {
    t.st.layout.view(p).expect("pane").modes.programme
}

/// The Inputs row of input `c` dropped on the focused pane's right edge: a pane of its own.
fn open_row(t: &mut T, c: u16) {
    let f = t.st.layout.focus;
    t.drag_drop(
        DragItem::Input(c),
        f,
        Some(ac2_scene::pane_edit::Edge::Right),
    );
}

/// A pair's row dropped on an edge: one new pane on its goniometer with the focus, holding
/// no measurement; one Ctrl+Z takes it away.
#[test]
fn a_pair_dropped_on_an_edge_opens_its_goniometer_one_undo_step() {
    let mut t = meter_t();
    let before = t.st.visible_panes();
    open_row(&mut t, 2);
    let p = programme_panes(&t);
    assert_eq!(p.len(), 1, "{:?}", t.visible());
    let v = t.st.layout.view(p[0]).expect("view");
    assert_eq!(
        (v.meters, v.modes.programme, v.meas),
        (Some(PAIR), M::Goniometer, None)
    );
    assert_eq!(t.st.layout.focus, p[0]);
    t.key("Ctrl+Z");
    assert_eq!(t.st.visible_panes(), before);
}

/// G on a pair's pane steps goniometer ↔ meters, Shift+G back; each step is one undo step.
#[test]
fn g_steps_a_pairs_pane_between_its_views() {
    let mut t = meter_t();
    open_row(&mut t, 2);
    let p = t.st.layout.focus;
    assert_eq!(mode(&t, p), M::Goniometer);
    t.key("G");
    assert_eq!(mode(&t, p), M::Meters);
    t.key("Shift+G");
    assert_eq!(mode(&t, p), M::Goniometer);
    t.key("Ctrl+Z");
    assert_eq!(mode(&t, p), M::Meters);
}

/// A pane of single inputs has no goniometer: G says so and leaves it on its meters.
#[test]
fn g_on_inputs_says_the_goniometer_needs_a_pair() {
    let mut t = meter_t();
    open_row(&mut t, 0);
    assert_eq!(programme_panes(&t).len(), 1, "one pane for an input");
    let p = t.st.layout.focus;
    t.key("G");
    assert_eq!(mode(&t, p), M::Meters);
    assert_eq!(t.last_toast(), "Programme: the goniometer needs a pair");
}

/// With room for the meters but not for a second pane beside them: the meters alone, and
/// a toast naming the key that turns the pane to the goniometer.
#[test]
fn no_room_for_the_goniometer_opens_the_meters_alone() {
    use crate::state::panes::{Axis, Side};
    let mut t = meter_t();
    t.st.pane_area = (1000.0, 1000.0);
    // Four halvings across leave a pane too narrow to halve again across, still tall
    // enough to halve down.
    for _ in 0..4 {
        let p =
            t.st.split_pane_at(t.st.layout.focus, Axis::Row, Side::B)
                .expect("room");
        t.st.layout.set_focus(p);
    }
    t.key("Ctrl+M");
    t.st.apply(Msg::Choose(Some(8)), &t.keys);
    t.key("Enter");
    let p = programme_panes(&t);
    assert_eq!(p.len(), 1, "{:?}", t.visible());
    assert_eq!(mode(&t, p[0]), M::Meters);
    assert_eq!(
        t.last_toast(),
        "no room for the goniometer pane: G turns this pane to it"
    );
}

/// The pair's Inputs row dropped on a pane opens it on the goniometer, an input's row on its
/// meters; E on a Programme pane changes nothing and says what does.
#[test]
fn an_inputs_row_dropped_opens_its_view_and_e_says_drag() {
    let mut t = meter_t();
    open_row(&mut t, 2);
    let p = t.st.layout.focus;
    t.key("G");
    t.st.apply(Msg::Command(CommandId::EditMeas), &t.keys);
    assert_eq!(t.st.overlay, Overlay::None, "no form");
    assert_eq!(t.last_toast(), ac2_scene::pane_edit::PROGRAMME_EDIT);
    t.drag_drop(DragItem::Input(4), p, None);
    assert_eq!(mode(&t, p), M::Meters, "an input opens on its bars");
    t.drag_drop(DragItem::Input(2), p, None);
    assert_eq!(mode(&t, p), M::Goniometer, "a pair opens on its goniometer");
}

/// The form's Ember decay row: Enter keeps the value in this app's settings, Esc leaves it.
#[test]
fn the_forms_ember_decay_is_this_apps_setting() {
    let mut t = meter_t();
    let was = t.st.prefs.ember_decay_s;
    for (close, kept) in [("Escape", false), ("Enter", true)] {
        t.key("Ctrl+M");
        t.st.apply(Msg::Choose(Some(8)), &t.keys);
        let Overlay::Form(f) = &t.st.overlay else {
            panic!("no form: {:?}", t.st.overlay)
        };
        let at = f
            .fields
            .iter()
            .position(|x| x.id == crate::forms::FieldId::EmberDecay)
            .expect("ember row");
        for _ in 0..at {
            t.key("ArrowDown");
        }
        t.key("ArrowRight");
        t.key(close);
        let now = t.st.prefs.ember_decay_s;
        if kept {
            assert!(now > was, "{was} → {now}");
        } else {
            assert_eq!(now, was);
        }
    }
}

/// The form's Ember glow row: Enter keeps the value in this app's settings, Esc leaves it.
#[test]
fn the_forms_ember_glow_is_this_apps_setting() {
    let mut t = meter_t();
    for (close, want) in [("Escape", 1.0), ("Enter", 1.5)] {
        t.key("Ctrl+M");
        t.st.apply(Msg::Choose(Some(8)), &t.keys);
        let Overlay::Form(f) = &t.st.overlay else {
            panic!("no form: {:?}", t.st.overlay)
        };
        let at = f
            .fields
            .iter()
            .position(|x| x.id == crate::forms::FieldId::EmberGlow)
            .expect("glow row");
        for _ in 0..at {
            t.key("ArrowDown");
        }
        t.key("ArrowRight");
        t.key(close);
        assert_eq!(t.st.prefs.ember_glow, want, "{close}");
    }
}

/// A ring is sized for the decay set now: a longer decay grows it on the next tick, so
/// every ember lives its whole ramp.
#[test]
fn a_rings_capacity_follows_the_decay() {
    use ac2_scene::gonio::{Cooling, EmberRing, ring_capacity};
    let mut t = T::new();
    t.conn(mirror(rig_with_pair()));
    t.st.prefs.ember_decay_s = 0.4;
    t.st.rig
        .programme
        .rings
        .insert((2, 3), EmberRing::new(ring_capacity(Cooling::default())));
    t.st.prefs.ember_decay_s = 2.0;
    t.tick(1.0);
    let cap = |decay, glow| ring_capacity(Cooling::new(decay, glow));
    assert_eq!(t.st.rig.programme.rings[&(2, 3)].capacity(), cap(2.0, 1.0));
    // A hotter glow lives longer: the ring grows for it too.
    t.st.prefs.ember_glow = 4.0;
    t.tick(1.0);
    assert_eq!(t.st.rig.programme.rings[&(2, 3)].capacity(), cap(2.0, 4.0));
}

/// The docked loudness plate's first drag starts where it was docked, at the size it was
/// drawn (the view sends its place and size first); the drag is one undo step, and
/// a double-click docks it again.
#[test]
fn a_docked_plate_floats_on_its_first_drag() {
    let mut t = meter_t();
    open_row(&mut t, 2);
    let pane = t.st.layout.focus;
    let kind = PlateKind::Loudness;
    let steps = t.st.history.undo_len();
    for m in [
        PlateMsg::Move {
            pane,
            kind,
            x: 0.7,
            y: 0.1,
        },
        PlateMsg::Scale {
            pane,
            kind,
            scale: 2.0,
        },
        PlateMsg::Move {
            pane,
            kind,
            x: 0.5,
            y: 0.4,
        },
        PlateMsg::End,
    ] {
        t.st.apply(Msg::Plate(m), &t.keys);
    }
    let l = t.st.layout.view(pane).expect("pane").plates.loudness;
    assert_eq!((l.at, l.scale()), (Some([0.5, 0.4]), 2.0));
    assert_eq!(t.st.history.undo_len(), steps + 1);
    t.st.apply(Msg::Plate(PlateMsg::Reset { pane, kind }), &t.keys);
    t.st.apply(Msg::Plate(PlateMsg::End), &t.keys);
    let l = t.st.layout.view(pane).expect("pane").plates.loudness;
    assert_eq!((l.at, l.scale()), (None, 1.0), "docked again");
}
