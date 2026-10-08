//! Clock drift between output and input (`docs/design/multi-device.md`), from an empty
//! daemon on a fake rig whose DAC runs on its own clock: a stable clock shows no drift, a
//! 50 ppm mismatch is published with its value, and an output timing jump alone is a jump,
//! never drift.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakeFault, FakePath};
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::frame::FrameData;
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal, TimingState, TimingStatus};
use ac2_proto::units::{Dbfs, LeaseToken, Samples};
use ac2_proto::{Change, Command, DataMessage, ReplyBody};
use ac2d::Daemon;
use common::*;

/// Loopback cable delay: long enough that the offset stays well inside the search range
/// while it drifts.
const LOOP: u32 = 2000;

fn drift_rig(drift_ppm: f64, faults: Vec<FakeFault>) -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 2,
        outputs: 2,
        drive: FakeDrive::Manual,
        seed: 11,
        paths: vec![FakePath::loopback(0, 0, LOOP)],
        drift_ppm,
        drift_horizon_seconds: 120.0,
        faults,
        ..FakeConfig::default()
    })
    .unwrap()
}

struct Rig {
    _h: ac2d::Handle,
    c: Client,
    timing: Sub,
    d: FakeDriver,
    tok: LeaseToken,
}

/// An empty daemon on `backend`: session with a loopback, pink noise playing.
fn start(name: &str, backend: FakeBackend) -> Rig {
    init_log();
    let mut cfg = config(backend.clone(), inproc(name));
    // Debug builds run the DSP slower than real time; the lease is not under test.
    cfg.lease_expiry = Duration::from_secs(60);
    let h = Daemon::start(cfg).unwrap();
    let (mut c, _) = connect(&h, &[]);
    let timing = Sub::connect(h.context(), h.data_endpoint(), &[b"timing", b"evt"]);
    c.ok(Command::SessionOpen {
        config: session(true),
    });
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
    let d = driver(&backend);
    Rig {
        _h: h,
        c,
        timing,
        d,
        tok,
    }
}

impl Rig {
    /// Runs `seconds` of audio, keeping the lease, and returns the timing status of the
    /// frame covering the end; `seen` gets every status on the way, published (frames) and
    /// committed (events: every state change, so a one-window Jumped is never missed).
    fn run(&mut self, seconds: f64, seen: &mut Vec<TimingStatus>) -> TimingStatus {
        let mut left = seconds;
        while left > 0.0 {
            let s = left.min(1.0);
            run(&mut self.d, s);
            left -= s;
            self.c.ok(Command::GenRefresh {
                lease_token: self.tok,
            });
        }
        // The last window that fits in the audio run starts a window length before its end.
        let end = end_sample(&self.d);
        let need = end.saturating_sub(48_000);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(
                Instant::now() < deadline,
                "timing never caught up: {seen:?}"
            );
            match self.timing.next(Duration::from_secs(5)) {
                Some(DataMessage::Frame(f)) => {
                    if let FrameData::Timing(m) = &f.data {
                        seen.push(m.status);
                        if f.stamp.audio_sample.0 >= need {
                            return m.status;
                        }
                    }
                }
                Some(DataMessage::Event(e)) => {
                    if let Change::Timing(t) = e.change {
                        seen.push(t);
                    }
                }
                None => {}
            }
        }
    }

    /// The committed timing status, once control has applied what the job reported.
    fn committed(&mut self, until: impl Fn(&TimingStatus) -> bool) -> TimingStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let t = match self.c.ok(Command::StateSnapshot) {
                ReplyBody::Snapshot(s) => s.state.timing,
                other => panic!("{other:?}"),
            };
            if until(&t) || Instant::now() > deadline {
                return t;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn stable_clock_shows_no_drift() {
    let mut r = start("drift0", drift_rig(0.0, vec![]));
    let mut seen = Vec::new();
    let t = r.run(14.0, &mut seen);
    assert_eq!(
        t.state,
        TimingState::Locked {
            offset: Samples(i64::from(LOOP))
        }
    );
    let d = t.drift.expect("drift estimate after 14 s");
    assert!(!d.warning && d.ppm.abs() < 0.2, "{d:?}");
    assert!(d.span.0 >= 10.0, "{d:?}");
    assert!(seen.iter().all(|s| s.drift.is_none_or(|d| !d.warning)));
    let c = r.committed(|t| t.drift.is_some_and(|d| d.span.0 >= 10.0));
    assert!(c.drift.is_some_and(|d| !d.warning), "{c:?}");
    assert!(c.internal_reference);
}

#[test]
fn fifty_ppm_is_published_and_a_new_session_forgets_it() {
    // The fake's DAC runs 50 ppm slow: the loopback offset grows by 2.4 samples per second.
    let mut r = start("drift50", drift_rig(-50.0, vec![]));
    let mut seen = Vec::new();
    let t = r.run(14.0, &mut seen);
    assert!(
        seen.iter()
            .all(|s| !matches!(s.state, TimingState::Jumped { .. })),
        "drift read as a jump: {seen:?}"
    );
    assert!(matches!(t.state, TimingState::Locked { .. }), "{t:?}");
    let d = t.drift.expect("drift");
    println!("50 ppm rig: {:.3} ppm over {:.1} s", d.ppm, d.span.0);
    assert!(d.warning && (d.ppm - 50.0).abs() < 0.5, "{d:?}");
    assert!(!t.internal_reference);
    let c = r.committed(|t| t.drift.is_some_and(|d| d.warning));
    let cd = c.drift.expect("committed drift");
    assert!(cd.warning && (cd.ppm - 50.0).abs() < 1.0, "{c:?}");
    assert!(cd.at.0 > 0, "{cd:?}");

    // The clock relation belongs to the stream: a reopened session starts without it.
    r.c.ok(Command::SessionClose);
    r.c.ok(Command::SessionOpen {
        config: session(true),
    });
    let c = r.committed(|t| t.drift.is_none());
    assert_eq!(c.drift, None, "{c:?}");
}

#[test]
fn output_jump_alone_is_a_jump_not_drift() {
    // 17 output frames never reach the DAC at 4 s: the offset steps by −17 once.
    let fault = FakeFault::DropOutputFrames {
        at_output_sample: 4 * u64::from(FS),
        frames: 17,
    };
    let mut r = start("drift-jump", drift_rig(0.0, vec![fault]));
    let mut seen = Vec::new();
    let t = r.run(14.0, &mut seen);
    let jumped: Vec<_> = seen
        .iter()
        .filter_map(|s| match s.state {
            TimingState::Jumped { from, to } => Some((from.0, to.0)),
            _ => None,
        })
        .collect();
    assert!(
        jumped.first() == Some(&(i64::from(LOOP), i64::from(LOOP) - 17)),
        "{jumped:?}"
    );
    assert_eq!(
        t.state,
        TimingState::Locked {
            offset: Samples(i64::from(LOOP) - 17)
        }
    );
    let d = t.drift.expect("drift");
    assert!(!d.warning && d.ppm.abs() < 0.3, "{d:?}");
    assert!(seen.iter().all(|s| s.drift.is_none_or(|d| !d.warning)));
}
