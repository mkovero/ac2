//! Spatial walks on the fake rig: moving the mic restarts each walk measurement with
//! tracking on, captures carry their positions and whether they were settled, the average
//! follows captures, redos and skips, the owners checks hold, and a session keeps the walk
//! and its plan.
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
    c.ok(Command::SessionOpen {
        config: session(true),
    });
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

fn captured(w: &WalkState, i: usize) -> Vec<TraceId> {
    match &w.points[i].state {
        PointState::Captured { traces } => traces.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn moving_to_a_point_resets_each_walk_measurement_and_turns_tracking_on() {
    let mut r = rig("walk-move", manual_rig());
    r.fill();
    assert!(!r.meas(MeasId(1)).delay.unwrap().tracking);

    let w = r.new_walk(&["P1", "P2", "P3"]);
    assert_eq!(w.current, Some(0));
    assert!(r.meas(MeasId(1)).delay.unwrap().tracking);
    let first = r.fill();
    assert!(first < 0.5, "walk.new restarts the averages: {first}");

    r.walk(Command::WalkGoto {
        walk: w.id,
        point: 2,
    });
    let first = r.fill();
    assert!(first < 0.5, "walk.goto restarts the averages: {first}");

    // The capture's advance is a move too.
    let w = r.walk(Command::WalkCapture { walk: w.id });
    assert_eq!(w.current, Some(0), "on to the next pending point, wrapping");
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
    let t1 = r.trace(captured(&w, 0)[0]);
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
    assert_eq!(w.points[0].mic, Some(MeasId(1)));
    assert_eq!(w.average, None, "one capture is no average");

    // Settled this time; the second capture makes the average, under the walk's
    // measurement.
    let w = r.capture(w.id);
    let t2 = r.trace(captured(&w, 1)[0]);
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
    assert_eq!(w.current, Some(2));
    r.walk(Command::WalkGoto {
        walk: w.id,
        point: 0,
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
    assert_eq!(w.current, None, "every point captured");
    let (t1, t2, t3) = (captured(&w, 0)[0], captured(&w, 1)[0], captured(&w, 2)[0]);
    assert_eq!(r.average_of(&w), vec![t1, t2, t3]);

    let w = r.walk(Command::WalkRedo {
        walk: w.id,
        point: 0,
    });
    assert_eq!(w.current, Some(0));
    assert_eq!(w.points[0].state, PointState::Pending);
    assert!(r.trace(t1).superseded, "kept, superseded");
    assert_eq!(r.average_of(&w), vec![t2, t3]);
    let first = r.fill();
    assert!(first < 0.5, "a redo is a move: {first}");

    let w = r.walk(Command::WalkCapture { walk: w.id });
    let t1b = captured(&w, 0)[0];
    assert_ne!(t1b, t1);
    assert_eq!(r.average_of(&w), vec![t1b, t2, t3], "in point order");

    r.walk(Command::WalkGoto {
        walk: w.id,
        point: 1,
    });
    let w = r.walk(Command::WalkSkip { walk: w.id });
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
        point: 0,
    });
    let w = r.capture(w.id);
    let w = r.capture(w.id);
    assert_eq!(w.average, avg);
    assert_eq!(r.average_of(&w).len(), 2);
    r.shutdown();
}

#[test]
fn what_a_walk_names_cannot_be_deleted_and_its_delete_keeps_them() {
    let mut r = rig("walk-owners", manual_rig());
    let w = r.new_walk(&["P1", "P2"]);
    let w1 = r.capture(w.id);
    let t1 = captured(&w1, 0)[0];
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
            ErrorCode::Unsupported,
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
    assert!(e.msg.contains("stage 3"), "{}", e.msg);
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
    assert_eq!(st.walks[0].current, Some(0));

    let w = r.capture(w.id);
    assert!(r.trace(captured(&w, 0)[0]).captured_unsettled);
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
    assert_eq!(
        r.average_of(&w),
        [captured(&w, 0), captured(&w, 1)].concat()
    );
    let w = r.walk(Command::WalkSkip { walk: w.id });
    assert_eq!(w.points[2].state, PointState::Skipped);
    r.h.shutdown();
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
