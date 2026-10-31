//! The room simulation from the palette: *Simulate room*, *Fit simulation to walk*, *Coverage
//! map* and *Cancel simulation* pick the focused walk, its plan and the plan's speakers.

use super::walk::{hall, mains};
use super::*;

fn speaker(name: &str, x: f64) -> Marker {
    Marker {
        name: name.into(),
        kind: MarkerKind::Speaker,
        at: [x, 14.0, 4.0],
        heading_deg: Some(270.0),
    }
}

fn run(t: &mut T, c: CommandId) -> Vec<(Command, String)> {
    let keys = t.keys.clone();
    let r = t.st.apply(Msg::Command(c), &keys);
    calls(&r)
        .into_iter()
        .map(|(c, w)| (c.clone(), w.to_owned()))
        .collect()
}

/// A walk pane on Mains over the Hall, its plan holding `speakers`.
fn walk_pane(speakers: Vec<Marker>) -> T {
    let mut t = T::new();
    let mut plan = hall();
    plan.markers.extend(speakers);
    t.remirror(|s| {
        s.walk_plans = vec![plan];
        s.walks = vec![mains()];
    });
    t.st.open_walk_pane(WalkId(1));
    t
}

#[test]
fn the_palette_simulates_the_focused_walks_plan() {
    let mut t = T::new();
    assert!(run(&mut t, CommandId::SimRun).is_empty());
    assert!(
        t.last_toast().contains("Spatial walk pane"),
        "{}",
        t.last_toast()
    );

    let mut t = walk_pane(vec![]);
    assert!(run(&mut t, CommandId::SimRun).is_empty());
    assert!(
        t.last_toast().contains("no speaker marker"),
        "{}",
        t.last_toast()
    );

    let mut t = walk_pane(vec![speaker("Main L", 6.0), speaker("Main R", 14.0)]);
    match run(&mut t, CommandId::SimRun).as_slice() {
        [(Command::SimRun { request }, what)] => {
            assert_eq!(request.plan, "Hall");
            assert_eq!(request.sources, ["Main L", "Main R"]);
            assert_eq!(request.receivers, SimReceivers::Walk { walk: WalkId(1) });
            assert_eq!(request.settings, SimSettings::default());
            assert_eq!(what, "simulate Hall");
        }
        c => panic!("{c:?}"),
    }
    match run(&mut t, CommandId::SimMap).as_slice() {
        [(Command::SimMap { request }, _)] => {
            assert_eq!(request.sources, ["Main L", "Main R"]);
            assert_eq!(request.spacing, SimMapRequest::DEFAULT_SPACING_M);
            assert_eq!(request.band, None);
            assert_eq!(request.window, Seconds(SimMapRequest::DEFAULT_WINDOW_S));
        }
        c => panic!("{c:?}"),
    }
}

#[test]
fn a_fit_asks_which_speaker_the_sweep_walk_was_measured_from() {
    let mut t = walk_pane(vec![speaker("Main L", 6.0), speaker("Main R", 14.0)]);
    assert!(run(&mut t, CommandId::SimFit).is_empty());
    assert!(t.last_toast().contains("sweep walk"), "{}", t.last_toast());

    t.remirror(|s| s.walks[0].kind = WalkKind::Sweep);
    assert!(run(&mut t, CommandId::SimFit).is_empty());
    assert!(
        matches!(&t.st.overlay, Overlay::Choose(c) if matches!(c.purpose, ChoicePurpose::SimSource { .. })),
        "{:?}",
        t.st.overlay
    );
    t.key("Down");
    let r = t.key("Enter");
    match calls(&r).as_slice() {
        [(Command::SimFit { request }, what)] => {
            assert_eq!(request.walk, WalkId(1));
            assert_eq!(request.source, "Main R");
            assert_eq!(*what, "fit the simulation to Mains");
        }
        c => panic!("{c:?}"),
    }

    // One speaker: no question.
    let mut t = walk_pane(vec![speaker("Main", 10.0)]);
    t.remirror(|s| s.walks[0].kind = WalkKind::Sweep);
    match run(&mut t, CommandId::SimFit).as_slice() {
        [(Command::SimFit { request }, _)] => assert_eq!(request.source, "Main"),
        c => panic!("{c:?}"),
    }
}

#[test]
fn cancel_stops_only_a_running_simulation() {
    let mut t = walk_pane(vec![speaker("Main", 10.0)]);
    assert!(run(&mut t, CommandId::SimCancel).is_empty());
    assert!(
        t.last_toast().contains("no simulation"),
        "{}",
        t.last_toast()
    );
    t.remirror(|s| s.sim_map = Some(ac2_proto::samples::sim_running()));
    assert!(matches!(
        run(&mut t, CommandId::SimCancel).as_slice(),
        [(Command::SimCancel, _)]
    ));
}
