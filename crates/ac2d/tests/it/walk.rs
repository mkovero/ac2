//! Spatial walks on the fake rig: moving the mic restarts each walk measurement with
//! tracking on, captures carry their positions and whether they were settled, the average
//! follows captures, redos and skips, the owners checks hold, a session keeps the walk
//! and its plan, and a loaded walk restarts its measurements on their first run.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_audio::{FakeBackend, FakeDriver};
use ac2_proto::frame::{Frame, FrameData};
use ac2_proto::model::layout::*;
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2d::Daemon;
use common::*;

const T: Duration = Duration::from_secs(10);

struct Rig {
    h: ac2d::Handle,
    c: Client,
    sub: Sub,
    tok: LeaseToken,
    d: FakeDriver,
    backend: FakeBackend,
    dir: tempfile::TempDir,
}

/// A daemon with a plan library and a session directory, one running transfer measurement
/// that fills in half a second, pink noise playing, and the sample plan in the library.
fn rig(name: &str, backend: FakeBackend) -> Rig {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = steady_config(backend.clone(), inproc(name));
    cfg.session_dir = dir.path().join("sessions");
    cfg.plan_dir = Some(dir.path().join("plans"));
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"d/1/tf"]);
    // A third input for a second mic.
    let mut s = session(true);
    s.input_channels = vec![0, 1, 2];
    c.ok(Command::SessionOpen { config: s });
    let mut m = transfer("main");
    if let MeasKind::Transfer { config } = &mut m.kind {
        config.depth = DepthPolicy::FastLf {
            max_settle_s: Seconds(0.5),
        };
    }
    c.ok(Command::MeasCreate { config: m });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    let tok = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    c.ok(Command::GenSet {
        lease_token: tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    c.ok(Command::PlanSave {
        plan: ac2_proto::samples::plan(),
    });
    let d = driver(&backend);
    Rig {
        h,
        c,
        sub,
        tok,
        d,
        backend,
        dir,
    }
}

impl Rig {
    /// The newest TF frame after `seconds` more audio, of config rev `min_rev` or later.
    fn frame(&mut self, seconds: f64, min_rev: u64) -> Frame {
        run(&mut self.d, seconds);
        self.c.ok(Command::GenRefresh {
            lease_token: self.tok,
        });
        let end = end_sample(&self.d);
        self.sub
            .frame(T, |f| {
                matches!(f.data, FrameData::Tf(_))
                    && f.stamp.audio_sample.0 + 1 >= end
                    && f.stamp.config_rev.0 >= min_rev
            })
            .expect("tf frame covering the audio")
    }

    /// Runs until the averages are full; the first reading.
    fn fill(&mut self) -> f32 {
        let rev = self.meas(MeasId(1)).config_rev.0;
        let first = filled(&self.frame(0.1, rev));
        for _ in 0..100 {
            if filled(&self.frame(0.1, rev)) == 1.0 {
                return first;
            }
        }
        panic!("never filled");
    }

    fn state(&mut self) -> State {
        match self.c.ok(Command::StateSnapshot) {
            ReplyBody::Snapshot(s) => s.state,
            other => panic!("{other:?}"),
        }
    }

    fn meas(&mut self, id: MeasId) -> Measurement {
        self.state()
            .measurements
            .into_iter()
            .find(|m| m.id == id)
            .unwrap()
    }

    fn trace(&mut self, id: TraceId) -> TraceMeta {
        self.state()
            .traces
            .into_iter()
            .find(|t| t.id == id)
            .unwrap()
    }

    fn walk(&mut self, cmd: Command) -> WalkState {
        match self.c.ok(cmd) {
            ReplyBody::Walk(w) => w,
            other => panic!("{other:?}"),
        }
    }

    fn new_walk(&mut self, labels: &[&str]) -> WalkState {
        self.walk(Command::WalkNew {
            name: "Mains".into(),
            plan: "Hall".into(),
            kind: WalkKind::LiveTransfer,
            meas: vec![MeasId(1)],
            points: points(labels),
        })
    }

    /// Fills, captures; the walk after.
    fn capture(&mut self, walk: WalkId) -> WalkState {
        self.fill();
        self.walk(Command::WalkCapture { walk })
    }

    /// The operands of the walk's average.
    fn average_of(&mut self, w: &WalkState) -> Vec<TraceId> {
        let m = self.meas(w.average.expect("an average"));
        let MeasKind::Math { config } = m.config.kind else {
            panic!("{m:?}");
        };
        let MathExpr::Average { of, .. } = config.expr else {
            panic!("{config:?}");
        };
        of.into_iter()
            .map(|o| match o {
                Operand::Trace { trace } => trace,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    fn shutdown(mut self) {
        self.c.ok(Command::GenStop);
        self.h.shutdown();
    }
}

fn points(labels: &[&str]) -> Vec<WalkPointSpec> {
    labels
        .iter()
        .enumerate()
        .map(|(i, l)| WalkPointSpec {
            label: (*l).into(),
            at: [4.0 + 2.0 * i as f64, 12.0, 1.2],
        })
        .collect()
}

fn filled(f: &Frame) -> f32 {
    match &f.data {
        FrameData::Tf(t) => t.meta.filled,
        other => panic!("{other:?}"),
    }
}

fn captured(w: &WalkState, i: usize) -> TraceId {
    match &w.points[i].state {
        PointState::Captured { trace, .. } => *trace,
        other => panic!("{other:?}"),
    }
}

#[test]
fn moving_to_a_point_resets_each_walk_measurement_and_turns_tracking_on() {
    let mut r = rig("walk-move", manual_rig());
    r.fill();
    assert!(!r.meas(MeasId(1)).delay.unwrap().tracking);

    let w = r.new_walk(&["P1", "P2", "P3"]);
    assert_eq!(w.step, Some(0));
    assert!(r.meas(MeasId(1)).delay.unwrap().tracking);
    let first = r.fill();
    assert!(first < 0.5, "walk.new restarts the averages: {first}");

    r.walk(Command::WalkGoto {
        walk: w.id,
        step: 2,
    });
    let first = r.fill();
    assert!(first < 0.5, "walk.goto restarts the averages: {first}");

    // The capture's advance is a move too.
    let w = r.walk(Command::WalkCapture { walk: w.id });
    assert_eq!(w.step, Some(0), "on to the next pending point, wrapping");
    let first = r.fill();
    assert!(first < 0.5, "the advance restarts the averages: {first}");
    r.shutdown();
}

#[test]
fn captures_carry_positions_and_a_delay_step_restarts_filled() {
    let mut r = rig("walk-capture", manual_rig());
    let w = r.new_walk(&["P1", "P2", "P3"]);
    r.fill();
    // A delay step mid-walk starts the stages over: a capture now is unsettled.
    let rev = match r.c.ok(Command::DelaySet {
        meas: MeasId(1),
        delay: Seconds(0.05),
    }) {
        ReplyBody::Measurement(m) => m.config_rev.0,
        other => panic!("{other:?}"),
    };
    let f = filled(&r.frame(0.1, rev));
    assert!(f < 1.0, "{f}");
    let w = r.walk(Command::WalkCapture { walk: w.id });
    let t1 = r.trace(captured(&w, 0));
    assert!(t1.captured_unsettled);
    assert_eq!(t1.edit.name, "P1 · main");
    assert_eq!(
        t1.position,
        Some(TracePosition {
            label: "P1".into(),
            at: [4.0, 12.0, 1.2],
            plan: "Hall".into(),
            walk: w.id,
        })
    );
    assert!(!t1.superseded);
    // The point records what its trace records of the chain, and when.
    let PointState::Captured { context, .. } = &w.points[0].state else {
        panic!("{w:?}");
    };
    assert_eq!(context.at, t1.created_at);
    assert_eq!(context.mic, t1.mic);
    assert_eq!(context.eq_in.len(), t1.eq_in.len());
    assert_eq!(w.points[0].mic, MeasId(1));
    assert_eq!(w.average, None, "one capture is no average");

    // Settled this time; the second capture makes the average, under the walk's
    // measurement.
    let w = r.capture(w.id);
    let t2 = r.trace(captured(&w, 1));
    assert!(!t2.captured_unsettled);
    assert_eq!(t2.position.as_ref().unwrap().label, "P2");
    assert_eq!(r.average_of(&w), vec![t1.id, t2.id]);
    let avg = r.meas(w.average.unwrap());
    let MeasKind::Math { config } = &avg.config.kind else {
        panic!("{avg:?}");
    };
    assert_eq!(config.owner, TraceOwner::Meas { meas: MeasId(1) });
    assert_eq!(config.domain, MathDomain::Transfer);
    assert_eq!(avg.config.name, "Mains · average");
    assert!(avg.running);

    // Capturing a captured point is refused; the walk is at P3.
    assert_eq!(w.step, Some(2));
    r.walk(Command::WalkGoto {
        walk: w.id,
        step: 0,
    });
    let e = r.c.call(Command::WalkCapture { walk: w.id }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    r.shutdown();
}

#[test]
fn redo_and_skip_supersede_and_the_average_follows() {
    let mut r = rig("walk-redo", manual_rig());
    let w = r.new_walk(&["P1", "P2", "P3"]);
    r.capture(w.id);
    r.capture(w.id);
    let w = r.capture(w.id);
    assert_eq!(w.step, None, "every point captured");
    let (t1, t2, t3) = (captured(&w, 0), captured(&w, 1), captured(&w, 2));
    assert_eq!(r.average_of(&w), vec![t1, t2, t3]);

    let w = r.walk(Command::WalkRedo {
        walk: w.id,
        point: 0,
    });
    assert_eq!(w.step, Some(0));
    assert_eq!(w.points[0].state, PointState::Pending);
    assert!(r.trace(t1).superseded, "kept, superseded");
    assert_eq!(r.average_of(&w), vec![t2, t3]);
    let first = r.fill();
    assert!(first < 0.5, "a redo is a move: {first}");

    let w = r.walk(Command::WalkCapture { walk: w.id });
    let t1b = captured(&w, 0);
    assert_ne!(t1b, t1);
    assert_eq!(r.average_of(&w), vec![t1b, t2, t3], "in point order");

    let w = r.walk(Command::WalkSkip {
        walk: w.id,
        point: 1,
    });
    assert_eq!(w.points[1].state, PointState::Skipped);
    assert!(r.trace(t2).superseded);
    assert_eq!(r.average_of(&w), vec![t1b, t3]);

    // Fewer than two captures: the average stays, with what is left, and says NO AVERAGE.
    let avg = w.average;
    let w = r.walk(Command::WalkRedo {
        walk: w.id,
        point: 2,
    });
    assert_eq!(w.average, avg, "the panes showing it keep it");
    assert_eq!(r.average_of(&w), vec![t1b]);
    let w = r.walk(Command::WalkRedo {
        walk: w.id,
        point: 0,
    });
    assert_eq!(w.average, avg);
    assert!(r.average_of(&w).is_empty());
    // The next captures fill the same average again.
    r.walk(Command::WalkGoto {
        walk: w.id,
        step: 0,
    });
    let w = r.capture(w.id);
    let w = r.capture(w.id);
    assert_eq!(w.average, avg);
    assert_eq!(r.average_of(&w).len(), 2);
    r.shutdown();
}

/// The second mic: a transfer on input 2, running.
fn second_mic(r: &mut Rig) -> MeasId {
    let mut m = transfer("second");
    if let MeasKind::Transfer { config } = &mut m.kind {
        config.measurement_input = 2;
    }
    let id = match r.c.ok(Command::MeasCreate { config: m }) {
        ReplyBody::Measurement(m) => m.id,
        other => panic!("{other:?}"),
    };
    r.c.ok(Command::MeasStart { meas: id });
    id
}

fn tracking(r: &mut Rig, meas: MeasId) -> bool {
    r.meas(meas).delay.is_some_and(|d| d.tracking)
}

#[test]
fn two_mics_capture_a_step_at_a_time_and_swap_skip_redo_per_point() {
    let mut r = rig("walk-two-mics", manual_rig());
    let m2 = second_mic(&mut r);
    // P1 left and P2 right of the hall, then a row further back, then one more on the right.
    let at = [
        [2.0, 10.0],
        [18.0, 10.0],
        [16.0, 14.0],
        [4.0, 14.0],
        [17.0, 18.0],
    ];
    let points: Vec<WalkPointSpec> = at
        .iter()
        .enumerate()
        .map(|(i, a)| WalkPointSpec {
            label: format!("P{}", i + 1),
            at: [a[0], a[1], 1.2],
        })
        .collect();
    let w = r.walk(Command::WalkNew {
        name: "Mains".into(),
        plan: "Hall".into(),
        kind: WalkKind::LiveTransfer,
        meas: vec![MeasId(1), m2],
        points,
    });
    // Each mic walks its own side: the cables cross nowhere.
    let plan: Vec<(u16, MeasId)> = w.points.iter().map(|p| (p.step, p.mic)).collect();
    assert_eq!(
        plan,
        vec![(0, MeasId(1)), (0, m2), (1, m2), (1, MeasId(1)), (2, m2)]
    );
    assert_eq!(w.step, Some(0));
    assert_eq!(w.standing(), vec![0, 1]);
    assert!(tracking(&mut r, MeasId(1)) && tracking(&mut r, m2));

    // One capture, both mics: two traces at two points, and the average of them.
    r.c.ok(Command::DelayTrack {
        meas: m2,
        enabled: false,
    });
    let w = r.capture(w.id);
    let (t1, t2) = (captured(&w, 0), captured(&w, 1));
    assert_eq!(r.trace(t1).edit.name, "P1 · main");
    assert_eq!(r.trace(t2).edit.name, "P2 · second");
    assert_eq!(r.trace(t2).position.unwrap().label, "P2");
    assert_eq!(r.average_of(&w), vec![t1, t2]);
    assert_eq!(w.step, Some(1));
    // The move to the next step restarts both measurements, tracking on.
    assert!(
        tracking(&mut r, m2),
        "the advance restarts every walk measurement"
    );
    let first = r.fill();
    assert!(first < 0.5, "the advance restarts the averages: {first}");

    // The mics trade places at P3 and P4; P5, planned from where the second mic stood at P3,
    // goes to the mic that stands there now.
    let w = r.walk(Command::WalkSwap {
        walk: w.id,
        points: vec![2, 3],
    });
    assert_eq!((w.points[2].mic, w.points[3].mic), (MeasId(1), m2));
    assert_eq!(w.points[4].mic, MeasId(1));
    let e =
        r.c.call(Command::WalkSwap {
            walk: w.id,
            points: vec![0, 2],
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "P1 is captured: {e:?}");

    // Skipping one point of a step leaves the other mic standing.
    let w = r.walk(Command::WalkSkip {
        walk: w.id,
        point: 3,
    });
    assert_eq!(w.points[3].state, PointState::Skipped);
    assert_eq!(w.step, Some(1));
    assert_eq!(w.standing(), vec![2]);
    let w = r.capture(w.id);
    let t3 = captured(&w, 2);
    assert_eq!(r.trace(t3).edit.name, "P3 · main");
    assert_eq!(w.points[2].mic, MeasId(1));
    assert_eq!(w.step, Some(2));
    assert_eq!(r.average_of(&w), vec![t1, t2, t3]);

    // Redoing one point goes back to its step with only its mic: the other's capture stays.
    let w = r.walk(Command::WalkRedo {
        walk: w.id,
        point: 0,
    });
    assert_eq!(w.step, Some(0));
    assert_eq!(w.standing(), vec![0]);
    assert!(r.trace(t1).superseded);
    assert_eq!(r.average_of(&w), vec![t2, t3]);
    let w = r.capture(w.id);
    let t1b = captured(&w, 0);
    assert_eq!(w.step, Some(2), "on to the step still pending");
    let w = r.capture(w.id);
    assert_eq!(w.step, None);
    let t5 = captured(&w, 4);
    assert_eq!(r.trace(t5).edit.name, "P5 · main");
    assert_eq!(r.average_of(&w), vec![t1b, t2, t3, t5]);
    r.shutdown();
}

#[test]
fn what_a_walk_names_cannot_be_deleted_and_its_delete_keeps_them() {
    let mut r = rig("walk-owners", manual_rig());
    let w = r.new_walk(&["P1", "P2"]);
    let w1 = r.capture(w.id);
    let t1 = captured(&w1, 0);
    // Before the average, only the walk names the first capture.
    let e = r.c.call(Command::TraceDelete { trace: t1 }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    let w = r.capture(w.id);
    let avg = w.average.unwrap();
    for meas in [MeasId(1), avg] {
        for traces in [OwnedTraces::Keep, OwnedTraces::Delete] {
            let e = r.c.call(Command::MeasDelete { meas, traces }).unwrap_err();
            assert_eq!(e.code, ErrorCode::Refused, "{meas}: {e:?}");
        }
    }
    let base = r.meas(MeasId(1)).config_rev;
    let e =
        r.c.call(Command::MeasUpdate {
            meas: MeasId(1),
            base,
            config: spl("main", 1),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    // A second walk cannot take the measurement.
    let e =
        r.c.call(Command::WalkNew {
            name: "Again".into(),
            plan: "Hall".into(),
            kind: WalkKind::LiveTransfer,
            meas: vec![MeasId(1)],
            points: points(&["A"]),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");

    r.c.ok(Command::WalkDelete { walk: w.id });
    let st = r.state();
    assert!(st.walks.is_empty());
    assert!(st.walk_plans.is_empty(), "no walk walks the copy");
    assert_eq!(st.traces.len(), 2, "the captures stay");
    assert!(
        st.measurements.iter().any(|m| m.id == avg),
        "the average stays"
    );
    // Now an ordinary average of ordinary traces.
    r.c.ok(Command::MeasDelete {
        meas: avg,
        traces: OwnedTraces::Keep,
    });
    r.c.ok(Command::TraceDelete { trace: t1 });
    r.shutdown();
}

#[test]
fn walk_new_refuses_what_it_cannot_walk() {
    let mut r = rig("walk-refusals", manual_rig());
    r.c.ok(Command::MeasCreate {
        config: transfer("second"),
    });
    let new = |meas: Vec<MeasId>, plan: &str, labels: &[&str]| Command::WalkNew {
        name: "Mains".into(),
        plan: plan.into(),
        kind: WalkKind::LiveTransfer,
        meas,
        points: points(labels),
    };
    let cases = [
        (
            new(vec![MeasId(1), MeasId(2)], "Hall", &["P1"]),
            ErrorCode::Invalid,
        ),
        (
            new(vec![MeasId(1), MeasId(1)], "Hall", &["P1"]),
            ErrorCode::Invalid,
        ),
        (
            new(vec![MeasId(1)], "Nowhere", &["P1"]),
            ErrorCode::NotFound,
        ),
        (new(vec![], "Hall", &["P1"]), ErrorCode::Invalid),
        (
            new(vec![MeasId(1)], "Hall", &["P1", "P1"]),
            ErrorCode::Invalid,
        ),
        (new(vec![MeasId(1)], "Hall", &[]), ErrorCode::Invalid),
        (new(vec![MeasId(9)], "Hall", &["P1"]), ErrorCode::NotFound),
    ];
    for (cmd, code) in cases {
        let e = r.c.call(cmd).unwrap_err();
        assert_eq!(e.code, code, "{e:?}");
    }
    let e =
        r.c.call(new(vec![MeasId(1), MeasId(2)], "Hall", &["P1"]))
            .unwrap_err();
    assert!(e.msg.contains("its own input"), "{}", e.msg);
    assert!(r.state().walks.is_empty());
    r.shutdown();
}

#[test]
fn awaiting_pick_reaches_the_walk_pane_and_marks_the_capture_unsettled() {
    let mut r = rig("walk-pick", crate::transfer::two_arrival_rig());
    let w = r.new_walk(&["P1", "P2"]);
    // A walk pane under the walk's measurement, naming the walk.
    let pane = LayoutPane {
        kind: PaneKind::Walk,
        transfer_view: TransferView::Magnitude,
        spectrum_view: SpectrumView::Spectrograph,
        spl_view: SplView::MeterLeq,
        sweep_view: SweepView::Response,
        programme_view: ProgrammeView::Meters,
        chrome: PlotChrome::Full,
        content: Some(PaneContent::Walk { walk: w.id }),
        levels: LayoutLevels::default(),
        plates: LayoutPlates::default(),
        rig: RigRef::ThisRig,
    };
    r.c.ok(Command::MeasLayout {
        meas: MeasId(1),
        layout: Some(PaneLayout::Pane {
            pane: Box::new(pane),
        }),
    });
    for _ in 0..6 {
        r.frame(0.5, 0);
    }
    match r.c.ok(Command::DelayFind {
        meas: MeasId(1),
        band: FinderBand::Full,
        observation: None,
    }) {
        ReplyBody::DelayFinding(f) => {
            assert!(matches!(f.outcome, DelayOutcome::Ambiguous { .. }), "{f:?}");
        }
        other => panic!("{other:?}"),
    }
    // What the walk pane reads: its walk, at P1, with the measurement waiting for a pick.
    let st = r.state();
    let m = st.measurements.iter().find(|m| m.id == MeasId(1)).unwrap();
    assert!(m.delay.as_ref().unwrap().awaiting_pick);
    let Some(PaneLayout::Pane { pane }) = &m.layout else {
        panic!("{:?}", m.layout);
    };
    assert_eq!(pane.content, Some(PaneContent::Walk { walk: w.id }));
    assert_eq!(st.walks[0].step, Some(0));

    let w = r.capture(w.id);
    assert!(r.trace(captured(&w, 0)).captured_unsettled);
    r.shutdown();
}

#[test]
fn a_session_round_trip_keeps_the_walk_and_its_plan() {
    let mut r = rig("walk-session", manual_rig());
    let w = r.new_walk(&["P1", "P2", "P3"]);
    r.capture(w.id);
    r.capture(w.id);
    let saved = r.state();
    r.c.ok(Command::FileSave {
        session: SessionRef::Name {
            name: "walked".into(),
        },
        overwrite: false,
    });
    // The library's plan goes; the session holds its copy.
    r.c.ok(Command::PlanDelete {
        name: "Hall".into(),
    });
    r.c.ok(Command::FileLoad {
        session: SessionRef::Name {
            name: "walked".into(),
        },
        discard: true,
    });
    let back = r.state();
    assert_eq!(back.walks, saved.walks);
    assert_eq!(back.walk_plans, saved.walk_plans);
    assert_eq!(back.walk_plans[0], ac2_proto::samples::plan());
    assert!(back.plans.is_empty());
    let positions = |s: &State| {
        let mut p: Vec<_> = s
            .traces
            .iter()
            .map(|t| (t.id, t.position.clone(), t.captured_unsettled))
            .collect();
        p.sort_by_key(|x| x.0);
        p
    };
    assert_eq!(positions(&back), positions(&saved));
    // The loaded average still averages the loaded captures, and the walk goes on from
    // where it was.
    let w = back.walks[0].clone();
    assert_eq!(r.average_of(&w), vec![captured(&w, 0), captured(&w, 1)]);
    let w = r.walk(Command::WalkSkip {
        walk: w.id,
        point: w.standing()[0] as u16,
    });
    assert_eq!(w.points[2].state, PointState::Skipped);
    r.h.shutdown();
}

#[test]
fn a_loaded_walk_restarts_its_measurements_on_their_first_run_only() {
    let mut r = rig("walk-resume", manual_rig());
    let w = r.new_walk(&["P1", "P2"]);
    r.capture(w.id);
    // Saved stopped and not tracking: the load alone must not restart anything.
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    r.c.ok(Command::DelayTrack {
        meas: MeasId(1),
        enabled: false,
    });
    r.c.ok(Command::FileSave {
        session: SessionRef::Name {
            name: "resumed".into(),
        },
        overwrite: false,
    });
    r.c.ok(Command::FileLoad {
        session: SessionRef::Name {
            name: "resumed".into(),
        },
        discard: true,
    });
    let m = r.meas(MeasId(1));
    assert!(!m.running);
    assert!(!m.delay.unwrap().tracking);
    resume_audio(&mut r);

    // The first run is a move: tracking on, the average starts empty.
    match r.c.ok(Command::MeasStart { meas: MeasId(1) }) {
        ReplyBody::Measurement(m) => assert!(m.delay.as_ref().unwrap().tracking, "{m:?}"),
        other => panic!("{other:?}"),
    }
    let first = r.fill();
    assert!(first < 0.5, "the first run restarts the averages: {first}");

    // Once: a second start neither resets the full average nor turns tracking back on.
    r.c.ok(Command::DelayTrack {
        meas: MeasId(1),
        enabled: false,
    });
    let rev = r.meas(MeasId(1)).config_rev.0;
    r.c.ok(Command::MeasStart { meas: MeasId(1) });
    assert!(!r.meas(MeasId(1)).delay.unwrap().tracking);
    let again = filled(&r.frame(0.1, rev));
    assert_eq!(again, 1.0, "a second start keeps the average");
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    r.c.ok(Command::MeasStart { meas: MeasId(1) });
    assert!(!r.meas(MeasId(1)).delay.unwrap().tracking);

    // Saved running, the load is its first run.
    r.c.ok(Command::FileSave {
        session: SessionRef::Name {
            name: "resumed-running".into(),
        },
        overwrite: false,
    });
    r.c.ok(Command::FileLoad {
        session: SessionRef::Name {
            name: "resumed-running".into(),
        },
        discard: true,
    });
    let m = r.meas(MeasId(1));
    assert!(m.running);
    assert!(m.delay.unwrap().tracking);
    r.shutdown();
}

/// After a load: the reopened stream's driver, and pink noise playing again (a load
/// disarms the generator and ends the lease).
fn resume_audio(r: &mut Rig) {
    r.d = driver(&r.backend);
    r.tok = match r.c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    r.c.ok(Command::GenSet {
        lease_token: r.tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
}

#[test]
fn the_plan_library_keeps_checked_plans_on_disk() {
    let mut r = rig("walk-plans", manual_rig());
    let dir = r.dir.path().join("plans");
    assert!(dir.join("Hall.json").is_file());
    let mut bad = ac2_proto::samples::plan();
    bad.name = "Crossed".into();
    bad.outline = vec![[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]];
    let e = r.c.call(Command::PlanSave { plan: bad }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    let mut named = ac2_proto::samples::plan();
    named.name = "../escape".into();
    let e = r.c.call(Command::PlanSave { plan: named }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    let mut case = ac2_proto::samples::plan();
    case.name = "hall".into();
    let e = r.c.call(Command::PlanSave { plan: case }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    match r.c.ok(Command::PlanList) {
        ReplyBody::Plans(p) => assert_eq!(p, vec![ac2_proto::samples::plan()]),
        other => panic!("{other:?}"),
    }
    let plan_dir = dir.clone();
    let tmp = std::mem::replace(&mut r.dir, tempfile::tempdir().unwrap());
    r.shutdown();

    // A restarted daemon lists the library from its files.
    let mut cfg = config(manual_rig(), inproc("walk-plans-2"));
    cfg.plan_dir = Some(plan_dir.clone());
    let h = Daemon::start(cfg).unwrap();
    let (mut c, _sub) = connect(&h, &[]);
    match c.ok(Command::PlanList) {
        ReplyBody::Plans(p) => assert_eq!(p, vec![ac2_proto::samples::plan()]),
        other => panic!("{other:?}"),
    }
    c.ok(Command::PlanDelete {
        name: "Hall".into(),
    });
    assert!(!plan_dir.join("Hall.json").exists());
    let e = c
        .call(Command::PlanDelete {
            name: "Hall".into(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound, "{e:?}");
    h.shutdown();
    drop(tmp);
}
