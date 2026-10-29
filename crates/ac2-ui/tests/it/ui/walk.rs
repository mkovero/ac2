//! The walk pane and the walk setup window as drawn, against the fake daemon holding a plan
//! and a walk half done.

use super::*;
use ac2_proto::Change;
use ac2_proto::event::Patch;
use ac2_proto::model::{
    CaptureContext, EqMark, Marker, MarkerKind, PlanSpec, PointState, WalkKind, WalkPoint,
    WalkState, ZoneSpec,
};
use ac2_proto::units::{EqSetId, TraceId, WalkId, WallNs};

fn hall() -> PlanSpec {
    PlanSpec {
        name: "Hall".into(),
        outline: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 15.0], [0.0, 15.0]],
        ceiling: Some(8.0),
        zones: vec![ZoneSpec {
            name: "Floor".into(),
            outline: vec![[2.0, 2.0], [18.0, 2.0], [18.0, 11.0], [2.0, 11.0]],
            ear_height: 1.2,
            weight: None,
            seats: Some(400),
        }],
        markers: vec![
            Marker {
                name: "FOH".into(),
                kind: MarkerKind::Foh,
                at: [10.0, 3.0, 1.2],
                heading_deg: Some(90.0),
            },
            Marker {
                name: "Main L".into(),
                kind: MarkerKind::Speaker,
                at: [6.0, 14.0, 3.0],
                heading_deg: Some(270.0),
            },
            Marker {
                name: "Main R".into(),
                kind: MarkerKind::Speaker,
                at: [14.0, 14.0, 3.0],
                heading_deg: Some(270.0),
            },
        ],
        surfaces: vec![],
    }
}

const DAY_NS: u64 = 86_400_000_000_000;

/// The wall clock now: the pane reads the capture days against it.
fn wall_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// A capture at `at` with EQ set 1 marked in the console at revision `rev`.
fn captured(trace: u32, at: u64, rev: u32) -> PointState {
    PointState::Captured {
        trace: TraceId(trace),
        context: CaptureContext {
            at: WallNs(at),
            mic: None,
            eq_in: vec![EqMark {
                set: EqSetId(1),
                rev,
            }],
        },
    }
}

/// Mains on Main L (meas 1): P1 captured yesterday, the mic at P2, P3 pending, P4 captured
/// today after the EQ changed.
fn walk_rig() -> common::Rig {
    let rig = common::Rig::start();
    let now = wall_now();
    {
        let mut s = rig.fake.lock();
        // One mic: a step per point, in walking order.
        let point = |label: &str, x: f64, y: f64, state| WalkPoint {
            label: label.into(),
            at: [x, y, 1.2],
            step: label[1..].parse::<u16>().unwrap_or(1) - 1,
            mic: MeasId(1),
            state,
        };
        s.commit(Change::Plan(Patch::Set(hall())));
        s.commit(Change::WalkPlan(Patch::Set(hall())));
        s.commit(Change::Walk(Patch::Set(WalkState {
            id: WalkId(1),
            name: "Mains".into(),
            plan: "Hall".into(),
            kind: WalkKind::LiveTransfer,
            meas: vec![MeasId(1)],
            points: vec![
                point("P1", 4.0, 9.0, captured(900, now - DAY_NS, 1)),
                point("P2", 12.0, 7.0, PointState::Pending),
                point("P3", 16.0, 4.0, PointState::Pending),
                point("P4", 7.0, 4.5, captured(901, now, 2)),
            ],
            step: Some(1),
            average: None,
        })));
    }
    rig
}

fn walk_texts(h: &Harness<'_, App>) -> Vec<String> {
    let st = &h.state().state;
    let s = ac2_ui::scenes::walk(
        st,
        st.layout.focus,
        &ac2_scene::theme::Theme::dark(),
        ac2_scene::primitives::Viewport {
            width: 900.0,
            height: 600.0,
        },
        ac2_ui::scenes::Now {
            instant: std::time::Instant::now(),
            wall: WallNs(wall_now()),
        },
    )
    .expect("a walk scene");
    s.scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.clone()))
        .collect()
}

/// Shift+W, then the walk's row: its pane opens beside the others with the step read from
/// FOH, whether the measurement is ready, the average still to come, and the warning that
/// the captures were taken on two days with the EQ changed between them.
#[test]
fn walk_pane_on_its_plan() {
    if !have_gpu("walk_pane_on_its_plan") {
        return;
    }
    let rig = walk_rig();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "the walk mirrored", |a| {
        live(a) && !a.state.walks().is_empty()
    });
    h.key_press_modifiers(Modifiers::SHIFT, Key::W);
    step_until(&mut h, "the Spatial walk window", |a| {
        matches!(&a.state.overlay, Overlay::Choose(_))
    });
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Enter);
    step_until(&mut h, "the walk pane", |a| {
        a.state.layout.focus_kind() == PaneKind::Walk
    });
    let texts = walk_texts(&h);
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("Move mic to P2 · ") && t.contains("from FOH")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "NO AVERAGE · 2 OF 4 POSITIONS"),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t == "P1 yesterday, P4 today · EQ changed"),
        "{texts:?}"
    );
    snapshot(&mut h, "walk_pane");
}

/// Shift+W, *New walk…*: the setup window with its rows, the plan with the suggested points
/// and the rules relaxed or broken named under it.
#[test]
fn walk_setup_window() {
    if !have_gpu("walk_setup_window") {
        return;
    }
    let rig = walk_rig();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "the plan mirrored", |a| {
        !a.state.walk_plan_choices().is_empty() && a.state.measurements().len() == 4
    });
    h.key_press_modifiers(Modifiers::SHIFT, Key::W);
    h.key_press(Key::Enter);
    step_until(
        &mut h,
        "the setup window",
        |a| matches!(&a.state.overlay, Overlay::WalkSetup(s) if s.points.len() == 8),
    );
    h.get_by_label("New spatial walk");
    h.get_by_label("Hall · 1 zone");
    snapshot(&mut h, "walk_setup");
}
