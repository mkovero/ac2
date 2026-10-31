//! Room simulation through the command path: a run on a plan of the library reports its
//! progress and its result as `sim` events, its T20 (ac2-core's evaluation of the simulated
//! impulse response) agrees with Eyring where the field is diffuse; refusals carry their
//! typed reason; `sim.cancel`, a plan change and a session close stop a run.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_proto::event::{Change, Event};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ErrorDetail, ReplyBody};
use ac2d::Daemon;
use common::*;

const T: Duration = Duration::from_secs(60);

fn tag(surface: Surface, material: &str) -> SurfaceTag {
    SurfaceTag {
        surface,
        material: material.into(),
    }
}

/// A 10 × 7 × 3.5 m room panelled in wood on every surface, a speaker near one end.
fn panelled() -> PlanSpec {
    PlanSpec {
        name: "Box".into(),
        outline: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 7.0], [0.0, 7.0]],
        ceiling: Some(3.5),
        zones: vec![],
        markers: vec![Marker {
            name: "Main".into(),
            kind: MarkerKind::Speaker,
            at: [1.0, 3.5, 2.0],
            heading_deg: None,
        }],
        surfaces: [
            Surface::Floor,
            Surface::Ceiling,
            Surface::Wall { edge: 0 },
            Surface::Wall { edge: 1 },
            Surface::Wall { edge: 2 },
            Surface::Wall { edge: 3 },
        ]
        .into_iter()
        .map(|s| tag(s, "wood_panel"))
        .collect(),
    }
}

fn request(plan: &str, rays: u32, budget_s: f64) -> SimRequest {
    SimRequest {
        plan: plan.into(),
        sources: vec!["Main".into()],
        receivers: SimReceivers::Points {
            points: vec![
                WalkPointSpec {
                    label: "P1".into(),
                    at: [6.0, 2.0, 1.2],
                },
                WalkPointSpec {
                    label: "P2".into(),
                    at: [8.0, 5.0, 1.2],
                },
            ],
        },
        settings: SimSettings {
            image_order: 2,
            rays_per_band: rays,
            budget: Seconds(budget_s),
            seed: 1,
        },
    }
}

struct Rig {
    h: ac2d::Handle,
    c: Client,
    sub: Sub,
    _dir: tempfile::TempDir,
}

fn rig(name: &str) -> Rig {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(manual_rig(), inproc(name));
    cfg.plan_dir = Some(dir.path().join("plans"));
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    c.ok(Command::Hello {
        client: "sim test".into(),
    });
    c.ok(Command::PlanSave { plan: panelled() });
    Rig {
        h,
        c,
        sub,
        _dir: dir,
    }
}

fn sim_event(e: &Event) -> Option<&SimState> {
    match &e.change {
        Change::Sim(s) => Some(s),
        _ => None,
    }
}

/// The first `sim` event of run `id` whose state satisfies `pred`.
fn wait(sub: &Sub, id: SimId, pred: impl Fn(&SimState) -> bool) -> SimState {
    let e = sub
        .event(T, |e| sim_event(e).is_some_and(|s| s.id == id && pred(s)))
        .expect("the sim event");
    sim_event(&e).unwrap().clone()
}

fn started(r: ReplyBody) -> SimState {
    match r {
        ReplyBody::Sim(s) => s,
        other => panic!("{other:?}"),
    }
}

fn refusal(r: Result<ReplyBody, ac2_proto::ProtoError>) -> SimRefusal {
    let e = r.expect_err("refused");
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    match e.detail {
        Some(ErrorDetail::Sim { reason }) => reason,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_run_reports_progress_and_its_t20_follows_eyring() {
    let mut r = rig("sim-run");
    let s = started(r.c.ok(Command::SimRun {
        request: request("Box", 6_000, 30.0),
    }));
    assert!(s.active());
    assert_eq!(s.absorption, SimAbsorption::Assumed);
    let running = wait(&r.sub, s.id, |s| {
        matches!(
            s.status,
            SimStatus::Running {
                stage: SimStage::RayTracing | SimStage::Analysis,
                ..
            }
        )
    });
    assert!(running.sources.is_empty(), "results only when done");
    let done = wait(&r.sub, s.id, |s| !s.active());
    assert_eq!(done.status, SimStatus::Done, "{:?}", done.status);
    assert_eq!(done.kind, SimKind::Run);
    assert_eq!(done.bands.len(), 7);
    assert!(done.valid_above.is_some_and(|f| f.0 > 20.0 && f.0 < 500.0));
    assert!(!done.budget_exhausted);
    let src = &done.sources[0];
    assert_eq!(src.receivers.len(), 2);
    let p1 = &src.receivers[0];
    assert_eq!(p1.label, "P1");
    assert!(p1.arrivals[0].path.is_empty(), "the direct sound first");
    assert!(p1.arrivals.iter().any(|a| a.path == [Surface::Floor]));
    assert!(
        p1.late
            .energy
            .iter()
            .all(|b| b.len() <= SimEchogram::MAX_BINS)
    );
    let room = src.room.as_ref().unwrap();
    assert_eq!(room.positions, 2);
    // 2–8 kHz: scattering makes the field nearly diffuse, where Eyring holds.
    for b in 4..7 {
        let t20 = room.octave[b].t20.unwrap().mean;
        let eyring = done.eyring[b].0;
        assert!(
            (t20 / eyring - 1.0).abs() < 0.08,
            "{} Hz: T20 {t20} against Eyring {eyring}",
            done.bands[b].0
        );
    }
    // The state holds the result.
    let st = match r.c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    };
    assert_eq!(st.sim.as_ref(), Some(&done));
    drop(r.h);
}

#[test]
fn refusals_name_their_reason() {
    let mut r = rig("sim-refuse");
    let mut outdoor = panelled();
    outdoor.name = "Field".into();
    outdoor.ceiling = None;
    outdoor.surfaces.retain(|t| t.surface != Surface::Ceiling);
    r.c.ok(Command::PlanSave { plan: outdoor });
    assert_eq!(
        refusal(r.c.call(Command::SimRun {
            request: request("Field", 100, 1.0)
        })),
        SimRefusal::Outdoor
    );
    let mut lino = panelled();
    lino.name = "Lino".into();
    lino.surfaces[0].material = "lino".into();
    r.c.ok(Command::PlanSave { plan: lino });
    assert_eq!(
        refusal(r.c.call(Command::SimRun {
            request: request("Lino", 100, 1.0)
        })),
        SimRefusal::UnknownMaterial {
            surface: Surface::Floor,
            name: "lino".into()
        }
    );
    let mut sub = request("Box", 100, 1.0);
    sub.sources = vec!["Sub".into()];
    assert_eq!(
        refusal(r.c.call(Command::SimRun { request: sub })),
        SimRefusal::NoSpeaker { name: "Sub".into() }
    );
    let mut out = request("Box", 100, 1.0);
    out.receivers = SimReceivers::Points {
        points: vec![WalkPointSpec {
            label: "Car park".into(),
            at: [20.0, 2.0, 1.2],
        }],
    };
    assert_eq!(
        refusal(r.c.call(Command::SimRun { request: out })),
        SimRefusal::Outside {
            name: "Car park".into()
        }
    );
    let e =
        r.c.call(Command::SimFit {
            request: SimFitRequest {
                walk: WalkId(9),
                source: "Main".into(),
                settings: SimSettings::default(),
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    let e = r.c.call(Command::SimCancel).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "nothing to cancel");
}

/// A run long enough to be stopped: a million rays per band under a minute's budget.
fn long_run(r: &mut Rig) -> SimState {
    let s = started(r.c.ok(Command::SimRun {
        request: request("Box", 1_000_000, 60.0),
    }));
    wait(&r.sub, s.id, |s| {
        matches!(
            s.status,
            SimStatus::Running {
                stage: SimStage::RayTracing,
                ..
            }
        )
    });
    s
}

#[test]
fn cancel_a_plan_change_and_a_session_close_stop_a_run() {
    let mut r = rig("sim-cancel");
    let s = long_run(&mut r);
    assert_eq!(
        refusal(r.c.call(Command::SimRun {
            request: request("Box", 100, 1.0)
        })),
        SimRefusal::Busy
    );
    let stopped = started(r.c.ok(Command::SimCancel));
    assert_eq!(
        stopped.status,
        SimStatus::Cancelled {
            why: SimCancel::Operator
        }
    );
    wait(&r.sub, s.id, |s| !s.active());

    // Saving the plan unchanged keeps the run; changing it stops it.
    let s = long_run(&mut r);
    r.c.ok(Command::PlanSave { plan: panelled() });
    let mut moved = panelled();
    moved.ceiling = Some(4.0);
    r.c.ok(Command::PlanSave { plan: moved });
    let end = wait(&r.sub, s.id, |s| !s.active());
    assert_eq!(
        end.status,
        SimStatus::Cancelled {
            why: SimCancel::PlanChanged
        }
    );

    r.c.ok(Command::SessionOpen {
        config: session(true),
    });
    let s = long_run(&mut r);
    r.c.ok(Command::SessionClose);
    let end = wait(&r.sub, s.id, |s| !s.active());
    assert_eq!(
        end.status,
        SimStatus::Cancelled {
            why: SimCancel::SessionClosed
        }
    );
    // A new run starts at once.
    let s = started(r.c.ok(Command::SimRun {
        request: request("Box", 1_000, 10.0),
    }));
    assert_eq!(wait(&r.sub, s.id, |s| !s.active()).status, SimStatus::Done);
}
