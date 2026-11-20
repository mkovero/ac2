//! Reducer tests of the Dynamics pane: the form, the curves from the step tables, the keys,
//! the envelopes asked for on demand, and a run from Space to the stored trace.

use super::*;
use ac2_proto::model::layout::{DynamicsMetric, DynamicsView};
use ac2_proto::model::{DynamicsConfig, DynamicsRun, DynamicsStatus, DynamicsTable};
use ac2_proto::units::DynamicsId;
use ac2_scene::dynamics::Side;

const MEAS: MeasId = MeasId(12);

fn config() -> DynamicsConfig {
    let MeasKind::Dynamics { config } = ac2_proto::samples::dynamics_measurement().config.kind
    else {
        unreachable!()
    };
    DynamicsConfig {
        reference_input: 0,
        measurement_inputs: vec![1],
        outputs: vec![0],
        // Below the test daemon's ceiling (−10 dBFS).
        bursts: vec![Dbfs(-20.0), Dbfs(-14.0)],
        ..config
    }
}

fn measurement() -> Measurement {
    let mut m = ac2_proto::samples::dynamics_measurement();
    m.config.kind = MeasKind::Dynamics { config: config() };
    m
}

fn run_meta(id: u32, number: u32) -> TraceMeta {
    let base = fixtures::sweep_meta(id);
    TraceMeta {
        kind: TraceKind::Dynamics,
        edit: TraceEdit {
            owner: TraceOwner::Meas { meas: MEAS },
            name: format!("Run {number}"),
            ..base.edit
        },
        source: TraceSource::Dynamics {
            meas: MEAS,
            meas_name: "Studer VCA".into(),
            run: DynamicsId(number),
            number,
            epoch: SessionEpoch(2),
        },
        ..base
    }
}

/// The sample table with a second input: one run, two curves (VCA L, VCA R).
fn table() -> DynamicsTable {
    let mut t = ac2_proto::samples::dynamics_data().table();
    let mut r = t.inputs[0].clone();
    r.input = 5;
    r.name = "VCA R".into();
    t.inputs.push(r);
    t
}

fn run(status: DynamicsStatus) -> DynamicsRun {
    DynamicsRun {
        id: DynamicsId(3),
        meas: MEAS,
        owner: ClientId("c1".into()),
        name: "Run 1".into(),
        reference_input: 0,
        measurement_inputs: vec![1],
        outputs: vec![0],
        carrier: Hz(1000.0),
        onset: ac2_proto::model::DynamicsOnset::Zero,
        pilot: Dbfs(-33.0),
        bursts: vec![Dbfs(-8.0), Dbfs(0.0)],
        durations: vec![Seconds(0.001), Seconds(0.1)],
        rest_max: Seconds(30.0),
        rests_from: None,
        longest: Seconds(130.0),
        status,
        started_at: WallNs(0),
    }
}

fn with_runs(runs: &[(u32, u32)]) -> State {
    let mut s = daemon_state();
    s.measurements.push(measurement());
    s.generator.owner = Some(ClientId("c1".into()));
    s.traces = runs.iter().map(|(id, n)| run_meta(*id, *n)).collect();
    s
}

/// A Dynamics pane on the measurement, focused, its run's table in.
fn pane_with_run() -> (T, PaneId) {
    let mut t = T::new();
    t.conn(mirror(with_runs(&[(14, 1)])));
    t.conn(ConnEvent::DynamicsTable(TraceId(14), Box::new(table())));
    t.go(PaneKind::Spectrum);
    let p = t.st.layout.focus;
    let v = t.st.layout.view_mut(p).expect("a pane");
    v.kind = PaneKind::Dynamics;
    v.meas = Some(MEAS);
    (t, p)
}

#[test]
fn the_chooser_opens_the_dynamics_form_and_it_creates_without_playing() {
    let mut t = T::new();
    t.st.open_new_measurement(t.st.layout.focus, &t.keys.clone());
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!("no chooser");
    };
    let at = ac2_scene::new_meas::NewKind::ALL
        .iter()
        .position(|k| *k == ac2_scene::new_meas::NewKind::Dynamics)
        .expect("listed");
    assert!(
        c.choices[at].label.starts_with("Dynamics — "),
        "{:?}",
        c.choices[at]
    );
    let mut out = Vec::new();
    t.st.new_measurement_pick(at, &mut out);
    let Overlay::Form(f) = &mut t.st.overlay else {
        panic!("no form: {:?}", t.st.overlay);
    };
    assert_eq!(f.kind, FormKind::Dynamics);
    assert_eq!(f.text(crate::forms::FieldId::Name), "Dynamics 1");
    assert!(f.set_channel(crate::forms::FieldId::Reference, 0));
    // No pilot typed: refused in place, nothing sent.
    let r = t.key("Enter");
    assert!(r.is_empty(), "{r:?}");
    let Overlay::Form(f) = &mut t.st.overlay else {
        panic!("the form closed");
    };
    assert!(
        f.error.as_deref().is_some_and(|e| e.contains("pilot")),
        "{:?}",
        f.error
    );
    f.set_text(crate::forms::FieldId::Pilot, "-33");
    // Above the daemon's ceiling: refused in place.
    f.set_text(crate::forms::FieldId::Bursts, "-14, -8");
    assert!(t.key("Enter").is_empty());
    let Overlay::Form(f) = &mut t.st.overlay else {
        panic!("the form closed");
    };
    assert!(
        f.error.as_deref().is_some_and(|e| e.contains("ceiling")),
        "{:?}",
        f.error
    );
    f.set_text(crate::forms::FieldId::Bursts, "-20, -14");
    f.set_text(crate::forms::FieldId::Durations, "10ms, 1s");
    let r = t.key("Enter");
    let Some(Request::CreateMeas { config }) = r.first() else {
        panic!("{r:?} {:?} {}", t.st.overlay, t.last_toast());
    };
    let MeasKind::Dynamics { config: c } = &config.kind else {
        panic!("{config:?}");
    };
    assert_eq!(c.reference_input, 0);
    assert_eq!(c.measurement_inputs, vec![1]);
    assert_eq!(c.pilot, Dbfs(-33.0));
    assert_eq!(c.bursts, vec![Dbfs(-20.0), Dbfs(-14.0)]);
    assert_eq!(c.durations, vec![Seconds(0.01), Seconds(1.0)]);
    assert!(!r.iter().any(|x| matches!(x, Request::StimArm { .. })));
}

#[test]
fn every_input_of_every_run_is_a_curve_and_the_keys_change_the_picks() {
    let (mut t, p) = pane_with_run();
    let curves = t.st.dynamics_curves(p);
    let names: Vec<&str> = curves.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["VCA L · Run 1", "VCA R · Run 1"]);
    let modes = |t: &T| t.st.layout.view(p).expect("pane").modes.dynamics;
    assert_eq!(modes(&t).view, DynamicsView::Steps);
    t.key("G");
    assert_eq!(modes(&t).view, DynamicsView::Envelopes);
    t.key("Shift+G");
    t.key("Shift+G");
    assert_eq!(modes(&t).view, DynamicsView::Table);
    t.key("K");
    assert_eq!(modes(&t).metric, DynamicsMetric::AttackTau);
    t.key("Shift+K");
    t.key("Shift+K");
    assert_eq!(modes(&t).metric, DynamicsMetric::Drift);
    // B: A moves to the second curve; Shift+B: B passes through none.
    t.key("B");
    assert!(matches!(modes(&t).a, Side::Curve(c) if c.input == 5));
    t.key("Shift+B");
    t.key("Shift+B");
    t.key("Shift+B");
    assert!(
        matches!(modes(&t).b, Side::Off | Side::Curve(_)),
        "{:?}",
        modes(&t).b
    );
    t.key("D");
    assert!(modes(&t).thd);
    t.key("E");
    assert!(modes(&t).all_levels);
    // Z / X move the picked step among the run's levels and lengths, stopping at the ends.
    t.key("Shift+Z");
    let s = modes(&t).step.expect("picked");
    t.key("Shift+Z");
    assert_eq!(modes(&t).step, Some(s), "the loudest level is the end");
}

#[test]
fn the_envelopes_view_asks_for_the_picked_steps_envelopes_once() {
    let (mut t, p) = pane_with_run();
    let r = t.key("G");
    let asked: Vec<_> = r
        .iter()
        .filter_map(|x| match x {
            Request::DynamicsEnvelope { trace, input, step } => Some((*trace, *input, *step)),
            _ => None,
        })
        .collect();
    assert_eq!(asked.len(), 2, "one per curve: {r:?}");
    assert!(asked.iter().all(|(t, _, _)| *t == TraceId(14)));
    // Asked once: the next pass asks nothing until the pick changes.
    let r = t.key("D");
    assert!(
        !r.iter()
            .any(|x| matches!(x, Request::DynamicsEnvelope { .. })),
        "{r:?}"
    );
    let env = ac2_proto::samples::dynamics_data()
        .envelope(4, asked[0].2)
        .expect("the sample's step");
    t.conn(ConnEvent::DynamicsEnvelope {
        key: asked[0],
        result: Ok(Box::new(env)),
    });
    let held = t.st.dynamics_curves(p);
    let modes = t.st.layout.view(p).expect("pane").modes.dynamics;
    let theme = ac2_scene::theme::Theme::dark();
    let curves = t.st.dynamics_frame_curves(&held, &modes, &theme);
    assert!(curves[0].envelope.is_some());
}

#[test]
fn space_arms_the_carrier_at_the_loudest_burst_and_enter_runs_it() {
    let mut t = T::new();
    t.conn(mirror(with_runs(&[])));
    t.go(PaneKind::Spectrum);
    let p = t.st.layout.focus;
    let v = t.st.layout.view_mut(p).expect("a pane");
    v.kind = PaneKind::Dynamics;
    v.meas = Some(MEAS);
    let r = t.key("Space");
    let Some(Request::StimArm { settings, .. }) =
        r.iter().find(|x| matches!(x, Request::StimArm { .. }))
    else {
        panic!("{r:?} {:?} {}", t.st.overlay, t.last_toast());
    };
    assert_eq!(settings.level, Dbfs(-14.0));
    assert_eq!(settings.signal, Signal::Sine { freq: Hz(1000.0) });
    assert_eq!(settings.outputs, vec![0]);
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert!(t.last_toast().contains("Studer VCA"), "{}", t.last_toast());
    let r = t.key("Enter");
    assert!(
        r.iter().any(|x| matches!(
            x,
            Request::DynamicsRun {
                meas: MEAS,
                rests_from: None
            }
        )),
        "{r:?}"
    );
    let playing = DynamicsStatus::Playing {
        step: 2,
        steps: 4,
        phase: ac2_proto::model::DynamicsPhase::Burst,
    };
    t.conn(ConnEvent::Stimulus(StimEvent::DynamicsStarted(Box::new(
        run(playing.clone()),
    ))));
    t.remirror(|s| s.dynamics = Some(run(playing)));
    assert!(
        t.st.pane_heading(p).ends_with("step 2 of 4 · burst"),
        "{}",
        t.st.pane_heading(p)
    );
    t.remirror(|s| {
        s.dynamics = Some(run(DynamicsStatus::Done { trace: TraceId(20) }));
        s.traces.push(run_meta(20, 1));
    });
    assert_eq!(t.st.rig.selected_trace, Some(TraceId(20)));
    assert!(t.st.rig.dynamics_run.run.is_none());
}

#[test]
fn a_measurement_with_runs_asks_whether_to_replay_one_s_rests() {
    let (mut t, _) = pane_with_run();
    t.key("Space");
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!("no question: {:?}", t.st.overlay);
    };
    assert_eq!(c.choices.len(), 2);
    assert!(c.choices[1].label.contains("Run 1"), "{:?}", c.choices);
    let r = t.st.apply(Msg::Choose(Some(1)), &t.keys.clone());
    assert!(
        r.iter().any(|x| matches!(x, Request::StimArm { .. })),
        "{r:?} {}",
        t.last_toast()
    );
    assert_eq!(
        t.st.rig
            .dynamics_run
            .plan
            .as_ref()
            .and_then(|p| p.rests_from),
        Some(TraceId(14))
    );
}

#[test]
fn a_failed_run_raises_a_banner_with_the_daemons_reason() {
    let (mut t, _) = pane_with_run();
    t.st.rig.dynamics_run.run = Some(DynamicsId(3));
    t.remirror(|s| {
        s.dynamics = Some(run(DynamicsStatus::Failed {
            reason: ac2_proto::model::DynamicsFailure::Mismatch,
            msg: "the reference does not match the programme".into(),
        }));
    });
    assert_eq!(
        t.last_toast(),
        "dynamics run failed: the reference does not match the programme"
    );
    assert!(t.st.rig.dynamics_run.run.is_none());
}
