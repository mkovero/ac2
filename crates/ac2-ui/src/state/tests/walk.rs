//! The spatial walk from the keyboard: the Spatial walk window, the setup window (K9 keys,
//! typed fields, no letter keys), the plan editor, `walk.new` opening the walk's own pane,
//! and the walk pane's keys.

use super::*;
use crate::walk_setup::PlanMsg;
use ac2_scene::walk::SweepAt;
use ac2_scene::walk::setup::{EditorField, Layer, SetupField};

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
        markers: vec![Marker {
            name: "FOH".into(),
            kind: MarkerKind::Foh,
            at: [10.0, 3.0, 1.2],
            heading_deg: Some(90.0),
        }],
        surfaces: vec![],
    }
}

fn point(label: &str, x: f64, state: PointState) -> WalkPoint {
    WalkPoint {
        label: label.into(),
        at: [x, 6.0, 1.2],
        // One mic: a step per point, in walking order.
        step: (x / 4.0) as u16 - 1,
        mic: WalkMic {
            meas: MeasId(1),
            input: 1,
        },
        state,
    }
}

/// Mains: P1 captured, the mic at P2, P3 pending.
fn mains() -> WalkState {
    WalkState {
        id: WalkId(1),
        name: "Mains".into(),
        plan: "Hall".into(),
        kind: WalkKind::LiveTransfer,
        mics: vec![WalkMic {
            meas: MeasId(1),
            input: 1,
        }],
        points: vec![
            point(
                "P1",
                4.0,
                PointState::Captured {
                    trace: TraceId(5),
                    context: ac2_proto::model::CaptureContext {
                        at: ac2_proto::units::WallNs(0),
                        mic: None,
                        eq_in: vec![],
                    },
                },
            ),
            point("P2", 8.0, PointState::Pending),
            point("P3", 12.0, PointState::Pending),
        ],
        step: Some(1),
        average: None,
        room: None,
    }
}

fn with_plan() -> T {
    let mut t = T::new();
    t.remirror(|s| s.plans = vec![hall()]);
    t
}

fn setup(t: &T) -> &crate::walk_setup::WalkSetup {
    match &t.st.overlay {
        Overlay::WalkSetup(s) => s,
        o => panic!("no setup window: {o:?}"),
    }
}

/// Shift+W lists *New walk…* first, then the session's walks; Enter on a walk opens its
/// pane, on *New walk…* the setup window.
#[test]
fn shift_w_lists_new_walk_then_the_walks() {
    let mut t = with_plan();
    t.remirror(|s| {
        s.walk_plans = vec![hall()];
        s.walks = vec![mains()];
    });
    t.key("Shift+W");
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!("no window: {:?}", t.st.overlay);
    };
    assert_eq!(c.title, "Spatial walk");
    let rows: Vec<&str> = c.choices.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(rows, ["New walk…", "Mains · 1 of 3 captured · Hall"]);
    t.key("Down");
    let before = t.st.layout.focus;
    t.key("Enter");
    let v = t.st.layout.focused();
    assert_eq!((v.kind, v.walk), (PaneKind::Walk, Some(WalkId(1))));
    assert_ne!(t.st.layout.focus, before, "a pane of its own");
    assert!(
        t.st.layout
            .view(before)
            .is_some_and(|v| v.kind != PaneKind::Walk)
    );
    // Asked again, the walk's pane on screen takes the focus: no second one.
    let panes = t.st.layout.panes().len();
    t.st.apply(Msg::FocusPane(before), &t.keys);
    t.key("Shift+W");
    t.key("Down");
    t.key("Enter");
    assert_eq!(t.st.layout.panes().len(), panes);
    assert_eq!(t.st.layout.focus_kind(), PaneKind::Walk);
    // Esc closes the window.
    t.key("Shift+W");
    t.key("Esc");
    assert_eq!(t.st.overlay, Overlay::None);
}

/// The setup window: ↑/↓ rows, ←/→ choices, typed values; letters are typed, never
/// shortcuts; Esc closes it whatever is typed.
#[test]
fn the_setup_window_follows_k9() {
    let mut t = with_plan();
    t.key("Shift+W");
    t.key("Enter");
    let s = setup(&t);
    assert_eq!(s.name, "Walk 1");
    assert_eq!(s.plan.as_deref(), Some("Hall"));
    assert_eq!(s.mics, [MeasId(1)], "the session's transfer");
    assert_eq!(s.points.len(), 8, "tuning's default");
    assert_eq!(s.focus, SetupField::Plan);
    // Down to the preset; → is ISO 3382-1 (400 seats: 6 positions), whose RT row appears.
    t.key("Down");
    t.key("Down");
    assert_eq!(setup(&t).focus, SetupField::Preset);
    t.key("Right");
    let s = setup(&t);
    assert_eq!(s.points.len(), 6);
    assert!(s.fields().contains(&SetupField::ReverbTime));
    // A typed count: the letters a pane would take are text here.
    t.key("Down");
    t.key("Down");
    assert_eq!(setup(&t).focus, SetupField::Count);
    t.type_key("X", "x");
    assert_eq!(setup(&t).typed.as_deref(), Some("x"));
    t.key("Backspace");
    t.text("4");
    t.key("Down");
    let s = setup(&t);
    assert_eq!((s.points.len(), s.focus), (4, SetupField::Pattern));
    // → is the next pattern: other points, as many.
    let before = s.points.clone();
    t.key("Right");
    let s = setup(&t);
    assert_eq!(s.seed, 2);
    assert_eq!(s.points.len(), 4);
    assert_ne!(s.points, before);
    // The name row takes typed text.
    for _ in 0..8 {
        t.key("Up");
    }
    assert_eq!(setup(&t).focus, SetupField::Name);
    t.text("Mains");
    t.key("Down");
    assert_eq!(setup(&t).name, "Mains");
    t.text("half typed");
    t.key("Esc");
    assert_eq!(t.st.overlay, Overlay::None);
}

/// Points edited on the plan go to `walk.new` as edited; its answer opens the walk in a
/// pane of its own, focused, and the setup closes.
#[test]
fn enter_starts_the_walk_in_its_own_pane() {
    let mut t = with_plan();
    t.key("Shift+W");
    t.key("Enter");
    t.st.apply(
        Msg::Plan(PlanMsg::Press {
            at: [16.0, 9.0],
            reach_m: 0.01,
        }),
        &t.keys,
    );
    t.st.apply(Msg::Plan(PlanMsg::Release), &t.keys);
    assert_eq!(setup(&t).points.len(), 9);
    let r = t.key("Enter");
    let [Request::NewWalk { cmd, name }] = r.as_slice() else {
        panic!("{r:?}");
    };
    assert_eq!(name, "Walk 1");
    let Command::WalkNew {
        plan, meas, points, ..
    } = cmd
    else {
        panic!("{cmd:?}");
    };
    assert_eq!((plan.as_str(), meas.as_slice()), ("Hall", &[MeasId(1)][..]));
    assert_eq!(points.len(), 9);
    assert_eq!(points[8].label, "P9");
    assert_eq!(points[8].at, [16.0, 9.0, 1.2]);
    // Sent once: a second Enter waits for the answer.
    assert!(t.key("Enter").is_empty());
    let focused = t.st.layout.focus;
    let mut w = mains();
    w.name = "Walk 1".into();
    t.remirror(|s| {
        s.walk_plans = vec![hall()];
        s.walks = vec![w.clone()];
    });
    t.conn(ConnEvent::WalkCreated(Box::new(w)));
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.layout.focus_kind(), PaneKind::Walk);
    assert_ne!(t.st.layout.focus, focused);
    assert!(
        t.last_toast().contains("move the mic to P2"),
        "{}",
        t.last_toast()
    );
}

/// Without a plan, Enter on *New plan…* opens the editor: vertices typed or clicked, the
/// verdict in words, Enter refused until the plan validates, then `plan.save` and back to
/// the setup with the plan chosen. Esc in the editor closes back to the setup.
#[test]
fn the_plan_editor_makes_the_walks_plan() {
    let mut t = T::new();
    t.key("Shift+W");
    t.key("Enter");
    assert_eq!(setup(&t).plan, None);
    t.key("Enter");
    let Overlay::PlanEditor(e) = &t.st.overlay else {
        panic!("no editor: {:?}", t.st.overlay);
    };
    assert_eq!(e.plan.name, "Room 1");
    assert!(e.verdict().is_err());
    // Esc goes back to the setup, Enter there opens a fresh editor.
    t.key("Esc");
    assert!(matches!(t.st.overlay, Overlay::WalkSetup(_)));
    t.key("Enter");
    t.text("0 0");
    t.key("Insert");
    t.text("12 0");
    // Two vertices make no floor: Enter is refused, the editor stays.
    let r = t.key("Enter");
    assert!(calls(&r).is_empty(), "{r:?}");
    let Overlay::PlanEditor(e) = &t.st.overlay else {
        panic!("closed on a refused plan");
    };
    assert_eq!(e.plan.outline.len(), 2);
    assert!(e.verdict().is_err());
    t.key("Insert");
    t.text("12 9");
    // A fourth vertex clicked, then a zone from the editing row, its corners clicked.
    t.st.apply(
        Msg::Plan(PlanMsg::Press {
            at: [0.04, 8.97],
            reach_m: 0.2,
        }),
        &t.keys,
    );
    t.st.apply(Msg::Plan(PlanMsg::Release), &t.keys);
    for _ in 0..9 {
        t.key("Up");
    }
    t.key("Down");
    t.key("Down");
    let Overlay::PlanEditor(e) = &t.st.overlay else {
        panic!();
    };
    assert_eq!(e.focus, EditorField::Layer);
    assert_eq!(e.plan.outline[3], [0.0, 9.0], "snapped to 0.1 m");
    t.key("Insert");
    for at in [[1.0, 1.0], [11.0, 1.0], [11.0, 7.0], [1.0, 7.0]] {
        t.st.apply(Msg::Plan(PlanMsg::Press { at, reach_m: 0.2 }), &t.keys);
        t.st.apply(Msg::Plan(PlanMsg::Release), &t.keys);
    }
    let Overlay::PlanEditor(e) = &t.st.overlay else {
        panic!();
    };
    assert_eq!(e.layer, Layer::Zone(0));
    assert_eq!(e.verdict(), Ok(()));
    let r = t.key("Enter");
    let c = calls(&r);
    assert!(
        matches!(c.as_slice(), [(Command::PlanSave { plan }, _)] if plan.name == "Room 1"),
        "{r:?}"
    );
    let s = setup(&t);
    assert_eq!(s.plan.as_deref(), Some("Room 1"));
    assert_eq!(s.points.len(), 8, "suggested on the new plan");
}

/// The walk pane's keys: X captures, PageDown/PageUp go to the next and previous point,
/// Shift+R redoes the last capture, K skips; on another pane X is the transfer's own.
#[test]
fn the_walk_pane_keys_drive_the_walk() {
    let mut t = with_plan();
    t.remirror(|s| {
        s.walk_plans = vec![hall()];
        s.walks = vec![mains()];
    });
    let r = t.key("X");
    assert!(
        !calls(&r)
            .iter()
            .any(|(c, _)| matches!(c, Command::WalkCapture { .. })),
        "a transfer pane's X"
    );
    t.st.open_walk_pane(WalkId(1));
    let only = |r: Vec<Request>| match calls(&r).as_slice() {
        [(c, what)] => ((*c).clone(), (*what).to_owned()),
        _ => panic!("{r:?}"),
    };
    let (c, what) = only(t.key("X"));
    assert_eq!(c, Command::WalkCapture { walk: WalkId(1) });
    assert_eq!(what, "Mains captured at P2");
    let (c, _) = only(t.key("PageDown"));
    assert_eq!(
        c,
        Command::WalkGoto {
            walk: WalkId(1),
            step: 2
        }
    );
    let (c, _) = only(t.key("PageUp"));
    assert_eq!(
        c,
        Command::WalkGoto {
            walk: WalkId(1),
            step: 0
        }
    );
    // The mic at a pending point: Shift+R redoes the last one captured before it.
    let (c, what) = only(t.key("Shift+R"));
    assert_eq!(
        c,
        Command::WalkRedo {
            walk: WalkId(1),
            point: 0
        }
    );
    assert!(what.contains("out of the average"), "{what}");
    let (c, _) = only(t.key("K"));
    assert_eq!(
        c,
        Command::WalkSkip {
            walk: WalkId(1),
            point: 1
        }
    );
    // One mic: no other to trade places with.
    assert!(calls(&t.key("M")).is_empty());
    assert!(t.last_toast().contains("no other"), "{}", t.last_toast());
    // Every point done: X says so and sends nothing.
    t.remirror(|s| s.walks[0].step = None);
    assert!(calls(&t.key("X")).is_empty());
    assert!(t.last_toast().contains("every point"), "{}", t.last_toast());
}

/// A transfer on input 2, the second mic.
fn second_mic() -> Measurement {
    let mut m = super::meas(3, "Main R", super::transfer());
    if let MeasKind::Transfer { config } = &mut m.config.kind {
        config.measurement_input = 2;
    }
    m
}

/// Two mics: Main L (1) and Main R (3); step 0 at P1 and P2, step 1 at P3 (Main R) and P4
/// (Main L).
fn two_mics() -> WalkState {
    let mut w = mains();
    let mic = |m: u32| WalkMic {
        meas: MeasId(m),
        input: if m == 1 { 1 } else { 2 },
    };
    w.mics = vec![mic(1), mic(3)];
    w.points = vec![
        point("P1", 4.0, PointState::Pending),
        point("P2", 8.0, PointState::Pending),
        point("P3", 12.0, PointState::Pending),
        point("P4", 16.0, PointState::Pending),
    ];
    for (p, (step, mic_id)) in w.points.iter_mut().zip([(0, 1), (0, 3), (1, 3), (1, 1)]) {
        p.step = step;
        p.mic = mic(mic_id);
    }
    w.step = Some(1);
    w
}

/// Two mics: the setup adds the second on its own input, X captures both, M has them trade
/// places, K and Shift+R ask which point (the whole step first).
#[test]
fn a_walk_with_two_mics_from_the_keyboard() {
    let mut t = with_plan();
    t.remirror(|s| s.measurements.push(second_mic()));
    t.key("Shift+W");
    t.key("Enter");
    // Down to the mic rows: one chosen, one to add the other transfer (on input 2).
    let fields = setup(&t).fields();
    assert_eq!(
        &fields[fields.len() - 2..],
        [SetupField::Mic(0), SetupField::Mic(1)]
    );
    for _ in 0..fields.len() {
        t.key("Down");
    }
    assert_eq!(setup(&t).focus, SetupField::Mic(1));
    t.key("Right");
    assert_eq!(setup(&t).mics, [MeasId(1), MeasId(3)]);
    assert_eq!(
        setup(&t).fields().last(),
        Some(&SetupField::Mic(1)),
        "no third transfer to add"
    );
    let r = t.key("Enter");
    let [
        Request::NewWalk {
            cmd: Command::WalkNew { meas, .. },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}");
    };
    assert_eq!(meas, &vec![MeasId(1), MeasId(3)]);
    t.key("Esc");

    t.remirror(|s| {
        s.walk_plans = vec![hall()];
        s.walks = vec![two_mics()];
    });
    t.st.open_walk_pane(WalkId(1));
    let only = |r: Vec<Request>| match calls(&r).as_slice() {
        [(c, what)] => ((*c).clone(), (*what).to_owned()),
        _ => panic!("{r:?}"),
    };
    let (c, what) = only(t.key("X"));
    assert_eq!(c, Command::WalkCapture { walk: WalkId(1) });
    assert_eq!(what, "Mains captured at P4, P3");
    let (c, what) = only(t.key("M"));
    assert_eq!(
        c,
        Command::WalkSwap {
            walk: WalkId(1),
            points: vec![3, 2]
        }
    );
    assert_eq!(what, "Mains: move Main R to P4, Main L to P3");
    // K: which point, the whole step first.
    t.key("K");
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    let rows: Vec<&str> = c.choices.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(
        rows,
        ["the whole step: P4, P3", "P4 · Main L", "P3 · Main R"]
    );
    t.key("Down");
    t.key("Down");
    let (c, _) = only(t.key("Enter"));
    assert_eq!(
        c,
        Command::WalkSkip {
            walk: WalkId(1),
            point: 2
        }
    );
    // Esc closes the chooser, nothing sent.
    t.key("K");
    assert!(calls(&t.key("Esc")).is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    // The whole step: a skip per point.
    t.key("K");
    let r = t.key("Enter");
    assert_eq!(calls(&r).len(), 2, "{r:?}");
}

/// A walk pane is stored with its walk and comes back with it; another kind drops it.
#[test]
fn a_walk_pane_keeps_its_walk() {
    use ac2_proto::model::layout as wire;
    let stored = PaneLayout::Pane {
        pane: Box::new(wire::LayoutPane {
            kind: wire::PaneKind::Walk,
            transfer_view: wire::TransferView::Magnitude,
            spectrum_view: wire::SpectrumView::Spectrum,
            spl_view: wire::SplView::MeterLeq,
            sweep_view: wire::SweepView::Response,
            programme_view: wire::ProgrammeView::Meters,
            chrome: wire::PlotChrome::Full,
            content: Some(wire::PaneContent::Walk { walk: WalkId(1) }),
            levels: wire::LayoutLevels::default(),
            plates: wire::LayoutPlates::default(),
            rig: wire::RigRef::ThisRig,
        }),
    };
    let mut t = with_plan();
    t.remirror(|s| {
        s.walk_plans = vec![hall()];
        s.walks = vec![mains()];
        s.measurements[1].layout = Some(stored.clone());
    });
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    let v = t.st.layout.focused();
    assert_eq!((v.kind, v.walk), (PaneKind::Walk, Some(WalkId(1))));
    assert_eq!(t.st.pane_layout(), stored);
    let p = t.st.layout.focus;
    t.st.set_pane_kind(p, PaneKind::Transfer);
    assert_eq!(t.st.layout.view(p).expect("the pane").walk, None);
}

/// Room: a sweep walk recording with Sweep 1 (on input 2), P1 and P2 at the first step's
/// pending points, the lease this client's.
fn sweep_walk() -> WalkState {
    let mic = WalkMic {
        meas: MeasId(5),
        input: 1,
    };
    let mut w = mains();
    w.id = WalkId(2);
    w.name = "Room".into();
    w.kind = WalkKind::Sweep;
    w.mics = vec![mic];
    w.points = vec![
        point("P1", 4.0, PointState::Pending),
        point("P2", 8.0, PointState::Pending),
    ];
    for p in &mut w.points {
        p.mic = mic;
    }
    w.step = Some(0);
    w
}

/// The setup offers the sweep measurement on the mic row: choosing it makes a sweep walk.
/// On the sweep walk's pane X arms the walk's sweep, then plays it (`sweep.run`, under the
/// lease and level path every sweep takes); the pane reads the run, not a readiness.
#[test]
fn a_sweep_walk_from_the_setup_to_its_run() {
    let mut t = with_plan();
    t.remirror(|s| {
        s.measurements.push(super::sweep_measurement());
        s.generator.owner = Some(ClientId("c1".into()));
    });
    t.key("Shift+W");
    t.key("Enter");
    let fields = setup(&t).fields();
    assert_eq!(fields.last(), Some(&SetupField::Mic(0)));
    for _ in 0..fields.len() {
        t.key("Down");
    }
    assert_eq!(setup(&t).focus, SetupField::Mic(0));
    t.key("Right");
    let s = setup(&t);
    assert_eq!(
        (s.kind, s.mics.as_slice()),
        (WalkKind::Sweep, &[MeasId(5)][..])
    );
    let r = t.key("Enter");
    let [
        Request::NewWalk {
            cmd: Command::WalkNew { kind, meas, .. },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}");
    };
    assert_eq!(
        (*kind, meas.as_slice()),
        (WalkKind::Sweep, &[MeasId(5)][..])
    );
    t.key("Esc");

    t.remirror(|s| {
        s.walk_plans = vec![hall()];
        s.walks = vec![sweep_walk()];
    });
    t.st.open_walk_pane(WalkId(2));
    let w = sweep_walk();
    assert_eq!(t.st.walk_sweep_at(&w), Some(SweepAt::Idle));
    assert_eq!(SweepAt::Idle.text(&["P1"]), "sweep plays at P1");
    assert!(t.st.sweep_view(), "the sweep walk's pane is a sweep's");
    let r = t.key("X");
    assert!(
        r.iter().any(|x| matches!(x, Request::StimArm { .. })),
        "{r:?}"
    );
    assert!(
        !calls(&r)
            .iter()
            .any(|(c, _)| matches!(c, Command::WalkCapture { .. }))
    );
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    let r = t.key("X");
    let [Request::Sweep { meas, .. }] = r.as_slice() else {
        panic!("{r:?}");
    };
    assert_eq!(*meas, MeasId(5));
}
