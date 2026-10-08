//! The system max level at run time (`gen.ceiling`) on the fake rig: lowering applies at
//! once and stops a stimulus above it (silence observed on the loopback), raising needs a
//! confirmation and a silent generator, the `--max-level` bound is never exceeded, every
//! client sees the change, and the level and the output labels survive a restart.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_audio::FakeDriver;
use ac2_proto::frame::FrameData;
use ac2_proto::model::{
    GenAction, Generator, GeneratorDesired, GeneratorSettings, OutputSetup, Signal, State,
};
use ac2_proto::units::{Dbfs, LeaseToken, MeasId};
use ac2_proto::{Change, Command, ErrorCode, ReplyBody};
use ac2d::Daemon;
use common::*;

const T: Duration = Duration::from_secs(5);

fn hello(c: &mut Client) -> String {
    match c.ok(Command::Hello {
        client: "test".into(),
    }) {
        ReplyBody::Welcome(w) => w.client_id.0,
        other => panic!("{other:?}"),
    }
}

fn state(c: &mut Client) -> State {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    }
}

fn acquire(c: &mut Client) -> LeaseToken {
    match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    }
}

fn fire(level: f64) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::White,
            level: Dbfs(level),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    }
}

fn ceiling(c: &mut Client, dbfs: f64, confirm_raise: bool) -> Result<Generator, ErrorCode> {
    c.call(Command::GenCeiling {
        ceiling: Dbfs(dbfs),
        confirm_raise,
    })
    .map(|r| match r {
        ReplyBody::Generator(g) => g,
        other => panic!("{other:?}"),
    })
    .map_err(|e| e.code)
}

/// Loopback input meter: a levels subscription that remembers how far it has read.
struct Meter {
    sub: Sub,
    seen_end: u64,
}

impl Meter {
    fn next(&mut self) -> f32 {
        let f = self
            .sub
            .frame(T, |f| matches!(f.data, FrameData::Levels(_)))
            .expect("levels frame");
        let FrameData::Levels(l) = &f.data else {
            unreachable!()
        };
        self.seen_end = f.stamp.audio_sample.0 + 1;
        l.peak[0]
    }

    /// Runs `seconds` of audio; the loopback input's sample peak (dBFS) over exactly that
    /// span.
    fn peak(&mut self, d: &mut FakeDriver, seconds: f64) -> f32 {
        let before = end_sample(d);
        while self.seen_end < before {
            self.next();
        }
        run(d, seconds);
        let end = end_sample(d);
        let mut peak = f32::NEG_INFINITY;
        while self.seen_end < end {
            peak = peak.max(self.next());
        }
        peak
    }
}

#[test]
fn lowering_applies_at_once_raising_needs_a_confirmation_and_silence() {
    init_log();
    let backend = manual_rig();
    let h = Daemon::start(config(backend.clone(), inproc("ceiling"))).unwrap();
    let (mut a, events) = connect(&h, &[b"evt"]);
    let mut m = Meter {
        sub: Sub::connect(h.context(), h.data_endpoint(), &[b"d/1/levels"]),
        seen_end: 0,
    };
    let mut b = Client::connect(h.context(), h.ctrl_endpoint());
    hello(&mut a);
    let id_b = hello(&mut b);
    let g = state(&mut a).generator;
    assert_eq!((g.ceiling, g.ceiling_bound), (Dbfs(-10.0), Dbfs(-10.0)));

    a.ok(Command::SessionOpen {
        config: session(false),
    });
    a.ok(Command::MeasCreate {
        config: spl("loopback", 0),
    });
    a.ok(Command::MeasStart { meas: MeasId(1) });
    let tok = acquire(&mut a);
    a.ok(Command::GenSet {
        lease_token: tok,
        desired: fire(-20.0),
    });
    let mut d = driver(&backend);
    let peak = m.peak(&mut d, 0.3);
    assert!(peak > -20.0, "playing: {peak}");

    // Above the bound: never.
    assert_eq!(ceiling(&mut b, -6.0, true), Err(ErrorCode::Invalid));

    // B (no lease) lowers it to −15: the stimulus at −20 carries on.
    let g = ceiling(&mut b, -15.0, false).unwrap();
    assert_eq!(g.ceiling, Dbfs(-15.0));
    assert!(g.firing, "below the new maximum: untouched");
    let peak = m.peak(&mut d, 0.2);
    assert!(peak > -20.0, "still playing: {peak}");

    // Lowered below the stimulus: stopped and disarmed at once, the output silent.
    let g = ceiling(&mut b, -30.0, false).unwrap();
    assert_eq!(g.ceiling, Dbfs(-30.0));
    assert!(!g.armed && !g.firing);
    let audit = g.last_action.unwrap();
    assert_eq!(
        (audit.action, audit.client.map(|c| c.0)),
        (GenAction::CeilingLowered, Some(id_b.clone()))
    );
    // The stop came first, in its own event naming the same client.
    events
        .event(T, |e| {
            matches!(&e.change, Change::Generator(g)
                if g.last_action.as_ref().is_some_and(|x| x.action == GenAction::Stop))
        })
        .expect("stop event");
    // A's mirror sees the new maximum (every client does).
    let ev = events
        .event(
            T,
            |e| matches!(&e.change, Change::Generator(g) if g.ceiling == Dbfs(-30.0)),
        )
        .expect("ceiling event");
    let Change::Generator(g) = ev.change else {
        unreachable!()
    };
    assert_eq!(g.ceiling_bound, Dbfs(-10.0));
    let _fade = m.peak(&mut d, 0.05);
    let peak = m.peak(&mut d, 0.2);
    assert_eq!(peak, f32::NEG_INFINITY, "silent after the lowering");
    // Requests above it are refused from now on.
    let e = a
        .call(Command::GenSet {
            lease_token: tok,
            desired: fire(-20.0),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);

    // Raising: refused while armed, refused without the confirmation.
    let mut armed = fire(-35.0);
    armed.firing = false;
    a.ok(Command::GenSet {
        lease_token: tok,
        desired: armed,
    });
    assert_eq!(ceiling(&mut b, -20.0, true), Err(ErrorCode::Refused));
    a.ok(Command::GenStop);
    assert_eq!(ceiling(&mut b, -20.0, false), Err(ErrorCode::Refused));
    let g = ceiling(&mut b, -20.0, true).unwrap();
    assert_eq!(g.ceiling, Dbfs(-20.0));
    let audit = g.last_action.unwrap();
    assert_eq!(
        (audit.action, audit.client.map(|c| c.0)),
        (GenAction::CeilingRaised, Some(id_b))
    );
    a.ok(Command::GenSet {
        lease_token: tok,
        desired: fire(-25.0),
    });
    let _fade = m.peak(&mut d, 0.1);
    let peak = m.peak(&mut d, 0.2);
    assert!(peak > -25.0, "plays again under the raised maximum: {peak}");
    a.ok(Command::GenStop);
    drop(h);
}

#[test]
fn level_and_output_labels_survive_a_restart_under_the_bound() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rig.json");
    let start = |bound: f64| {
        let mut cfg = config(manual_rig(), inproc("ceiling-restart"));
        cfg.max_level_dbfs = bound;
        cfg.rig_settings = Some(file.clone());
        Daemon::start(cfg).unwrap()
    };
    let label = |channel: u16, l: &str| OutputSetup {
        channel,
        label: Some(l.into()),
    };

    let h = start(-10.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    ceiling(&mut c, -32.0, false).unwrap();
    match c.ok(Command::SessionOutputs {
        outputs: vec![label(0, "Main L"), label(1, "Sub")],
    }) {
        ReplyBody::Outputs(o) => assert_eq!(o, vec![label(0, "Main L"), label(1, "Sub")]),
        other => panic!("{other:?}"),
    }
    // A label clears with none; a bad one is refused.
    c.ok(Command::SessionOutputs {
        outputs: vec![OutputSetup {
            channel: 1,
            label: None,
        }],
    });
    let e = c
        .call(Command::SessionOutputs {
            outputs: vec![label(2, " padded")],
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    h.shutdown();

    // The same bound: the level and the labels come back.
    let h = start(-10.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    let s = state(&mut c);
    assert_eq!(s.generator.ceiling, Dbfs(-32.0));
    assert_eq!(s.outputs, vec![label(0, "Main L")]);
    h.shutdown();

    // A lower bound at start wins over the kept level.
    let h = start(-40.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    let g = state(&mut c).generator;
    assert_eq!((g.ceiling, g.ceiling_bound), (Dbfs(-40.0), Dbfs(-40.0)));
    // And the bound refuses anything above it.
    assert_eq!(ceiling(&mut c, -32.0, true), Err(ErrorCode::Invalid));
    h.shutdown();

    // A higher bound again: the kept level (it was never raised) is still in force.
    let h = start(-6.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    assert_eq!(state(&mut c).generator.ceiling, Dbfs(-32.0));
    h.shutdown();
}
