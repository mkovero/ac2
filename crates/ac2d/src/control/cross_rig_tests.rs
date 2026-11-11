//! The relation between two fake rigs, second by second in simulated time, through the
//! clock gate of a transfer function between them: what it sends its job and what its
//! entry in the `remote` entity says.

use std::time::{Duration, Instant};

use ac2_proto::model::{
    CantTellReason, ClockBasis, Clocks, DeviceClock, DeviceId, PeerRelation, SampleClock,
    SystemClock,
};
use ac2_proto::units::{RigId, Seconds, WallNs};

use super::*;
use crate::remote::peer_clock::{Evidence, PeerClockWatch};

const SYNCED: SystemClock = SystemClock::Synced { maxerror_us: 100 };

fn clocks(system: SystemClock, ppm: f64) -> Clocks {
    Clocks {
        system,
        capture: Some(DeviceClock {
            device: DeviceId("fake".into()),
            rate: SampleClock::Measured {
                ppm,
                span: Seconds(60.0),
                against: ClockBasis::SystemClock,
            },
        }),
        stamp_map: None,
    }
}

/// Two rigs, the cross-rig rate as a function of time (s → ppm), and a transfer function
/// between them, stepped a second at a time.
struct Sim {
    ours: Clocks,
    theirs: Clocks,
    rate: Box<dyn Fn(u64) -> f64>,
    /// When the stream started or last re-anchored, s.
    stream_from: u64,
    watch: PeerClockWatch,
    tf: CrossTf,
    tracking: bool,
    t0: Instant,
    t: u64,
    gates: Vec<(u64, ClockGate)>,
}

impl Sim {
    fn new(rate: impl Fn(u64) -> f64 + 'static) -> Self {
        Self {
            ours: clocks(SYNCED, 0.1),
            theirs: clocks(SYNCED, -0.1),
            rate: Box::new(rate),
            stream_from: 0,
            watch: PeerClockWatch::default(),
            tf: CrossTf::new(RigId(2), RemoteSide::Measurement),
            tracking: true,
            t0: Instant::now(),
            t: 0,
            gates: Vec::new(),
        }
    }

    /// The cross-rig rate over the `span` s before now, as the joined feed fits it.
    fn window(&self, span: u64) -> Option<(f64, f64)> {
        if self.t - self.stream_from < span {
            return None;
        }
        let sum: f64 = (self.t - span..self.t).map(|s| (self.rate)(s)).sum();
        Some((sum / span as f64, span as f64))
    }

    fn now(&self) -> Instant {
        self.t0 + Duration::from_secs(self.t)
    }

    fn step(&mut self) {
        self.t += 1;
        let e = Evidence {
            ours: (RigId(1), &self.ours),
            theirs: (RigId(2), Some(&self.theirs)),
            capture_stamps: true,
            long: self.window(60),
            short: self.window(40),
            streaming_for: Some(Duration::from_secs(self.t - self.stream_from)),
            disagree: None,
        };
        self.watch
            .update(&e, Duration::from_secs(self.t), WallNs(self.t));
        let rel = self.watch.clock.relation;
        let at = self.now();
        if let Some(g) = self.tf.judge(&rel, self.tracking, at) {
            self.gates.push((self.t, g));
        }
    }

    fn run_to(&mut self, t: u64) {
        while self.t < t {
            self.step();
        }
    }

    fn entry(&mut self) -> CrossRigTransfer {
        let at = self.now();
        self.tf.entry(MeasId(3), at)
    }

    fn relation(&self) -> PeerRelation {
        self.watch.clock.relation
    }

    fn first_gate(&self, pred: impl Fn(&ClockGate) -> bool) -> Option<u64> {
        self.gates.iter().find(|(_, g)| pred(g)).map(|(t, _)| *t)
    }

    fn event(&mut self, e: CrossEvent) {
        let at = self.now();
        self.tf.event(&e, 48_000.0, at);
    }
}

fn stopped(g: &ClockGate, w: StopWhy) -> bool {
    matches!(g, ClockGate::Stop { why, .. } if *why == w)
}

#[test]
fn locked_rigs_measure_a_minute_then_run() {
    let mut s = Sim::new(|_| 0.0);
    s.run_to(30);
    assert_eq!(s.relation(), PeerRelation::Measuring);
    assert_eq!(
        s.entry().state,
        CrossTfState::Stopped {
            why: StopWhy::Measuring,
            held: false
        }
    );
    s.run_to(61);
    assert_eq!(s.relation(), PeerRelation::Agree);
    assert_eq!(s.entry().state, CrossTfState::Running);
    assert_eq!(s.entry().relock, None, "a first agreement is no relock");
}

#[test]
fn five_ppm_never_runs() {
    let mut s = Sim::new(|_| 5.0);
    s.run_to(300);
    assert!(matches!(s.relation(), PeerRelation::NotLocked { .. }));
    assert!(s.first_gate(|g| *g == ClockGate::Run).is_none());
    let tripped = s
        .first_gate(|g| stopped(g, StopWhy::NotLocked))
        .expect("not locked");
    assert!(tripped <= 41, "not locked at {tripped} s");
}

#[test]
fn a_pull_is_not_locked_within_40_s_and_relocks_after_a_clean_minute() {
    // The word clock out for 3 minutes: the FF400 falls back to its crystal (+5 ppm).
    const PULL: u64 = 120;
    const BACK: u64 = PULL + 180;
    let mut s = Sim::new(|t| if (PULL..BACK).contains(&t) { 5.0 } else { 0.0 });
    s.run_to(PULL);
    assert_eq!(s.entry().state, CrossTfState::Running);
    s.run_to(BACK + 200);
    let stop = s
        .first_gate(|g| stopped(g, StopWhy::NotLocked))
        .expect("not locked");
    assert!(
        stop - PULL <= 40,
        "not locked {} s after the pull",
        stop - PULL
    );
    // The stop names the window that failed, so the job holds the result before it.
    let (_, g) = s
        .gates
        .iter()
        .find(|(t, _)| *t == stop)
        .expect("the stop's gate");
    assert_eq!(
        *g,
        ClockGate::Stop {
            why: StopWhy::NotLocked,
            window_s: 40.0
        }
    );
    let run = s
        .gates
        .iter()
        .rev()
        .find(|(_, g)| *g == ClockGate::Run)
        .map(|(t, _)| *t)
        .expect("a return to agreement");
    // Clean once the 60 s window holds less than 24 s of the pull, then a minute.
    assert!(
        run > BACK + 60 && run <= BACK + 100,
        "running again at {run} s"
    );
    let r = s.entry().relock.expect("a relock");
    assert_eq!(r.from, StopWhy::NotLocked);
    assert_eq!(r.outcome, RelockOutcome::Tracking);
    assert_eq!(r.after.0, (run - stop) as f64);

    // The job's tracking agrees: the delay it held and where it is now.
    s.event(CrossEvent::Relocked {
        was: 594.24,
        now: 643.68,
    });
    assert_eq!(
        s.entry().relock.map(|r| r.outcome),
        Some(RelockOutcome::Tracked {
            delay: Seconds(643.68 / 48_000.0),
            was: Seconds(594.24 / 48_000.0)
        })
    );
    // Said for a minute.
    s.run_to(s.t + 61);
    assert_eq!(s.entry().relock, None);
}

#[test]
fn tracking_off_relocks_to_find_it_again_until_a_find() {
    let mut s = Sim::new(|t| if (100..130).contains(&t) { 5.0 } else { 0.0 });
    s.tracking = false;
    s.run_to(400);
    assert_eq!(
        s.entry().relock.map(|r| r.outcome),
        Some(RelockOutcome::FindAgain)
    );
    // Not said for a minute only: it stays until the operator finds the delay.
    s.run_to(600);
    assert!(s.entry().relock.is_some());
    s.tf.resolved();
    assert_eq!(s.entry().relock, None);
}

#[test]
fn an_ambiguous_relock_waits_for_the_pick() {
    use ac2_core::delay::{Band, Confidence, FinderResult, Outcome};
    let mut s = Sim::new(|t| if (100..130).contains(&t) { 5.0 } else { 0.0 });
    s.run_to(400);
    s.event(CrossEvent::RelockAmbiguous(Box::new(FinderResult {
        outcome: Outcome::NoEstimate {
            reasons: Vec::new(),
        },
        candidates: Vec::new(),
        confidence: Confidence {
            psr_db: 0.0,
            psr_acq_db: 0.0,
            band_snr_db: 0.0,
            excited_fraction: 0.0,
            pulse_width: 0.0,
            nominal_width: 0.0,
            period: None,
            refinement_window: None,
        },
        band: Band::FullRange,
        meas_window: 0..1,
    })));
    assert_eq!(
        s.entry().relock.map(|r| r.outcome),
        Some(RelockOutcome::Ambiguous)
    );
    s.run_to(600);
    assert!(s.entry().relock.is_some(), "kept until picked");
}

#[test]
fn cant_tell_is_refused_and_a_short_passage_relocks() {
    let mut s = Sim::new(|_| 0.0);
    s.run_to(100);
    assert_eq!(s.entry().state, CrossTfState::Running);
    // A PTP fault long enough to clear the kernel's sync flag on the other host.
    s.theirs.system = SystemClock::Unsynced {
        maxerror_us: 16_000,
    };
    s.run_to(116);
    assert_eq!(
        s.relation(),
        PeerRelation::CantTell {
            reason: CantTellReason::SystemClockUnsynced { rig: RigId(2) }
        }
    );
    assert!(matches!(
        s.entry().state,
        CrossTfState::Stopped {
            why: StopWhy::CantTell,
            ..
        }
    ));
    s.theirs.system = SYNCED;
    s.run_to(117);
    assert_eq!(s.entry().state, CrossTfState::Running);
    let r = s.entry().relock.expect("a relock");
    assert_eq!((r.from, r.after.0), (StopWhy::CantTell, 16.0));
}

#[test]
fn a_source_without_capture_stamps_cannot_tell() {
    let mut s = Sim::new(|_| 0.0);
    s.run_to(100);
    let e = Evidence {
        ours: (RigId(1), &s.ours),
        theirs: (RigId(2), Some(&s.theirs)),
        capture_stamps: false,
        long: s.window(60),
        short: s.window(40),
        streaming_for: Some(Duration::from_secs(100)),
        disagree: None,
    };
    s.watch.update(&e, Duration::from_secs(101), WallNs(101));
    let rel = s.watch.clock.relation;
    assert_eq!(
        s.tf.judge(&rel, true, s.now()),
        Some(ClockGate::Stop {
            why: StopWhy::CantTell,
            window_s: 0.0
        })
    );
}

#[test]
fn a_re_anchor_returns_to_measuring_and_relocks_after_it() {
    let mut s = Sim::new(|_| 0.0);
    s.run_to(100);
    // The source rig restarted its stream: a new incarnation, anchored again.
    s.stream_from = 100;
    s.run_to(101);
    assert!(matches!(
        s.entry().state,
        CrossTfState::Stopped {
            why: StopWhy::Measuring,
            ..
        }
    ));
    s.run_to(161);
    assert_eq!(s.entry().state, CrossTfState::Running);
    assert_eq!(s.entry().relock.map(|r| r.from), Some(StopWhy::Measuring));
}

#[test]
fn two_crystals_1_5_ppm_apart_agree_and_the_delay_track_cues_it() {
    let mut s = Sim::new(|_| 1.5);
    s.run_to(200);
    assert_eq!(s.relation(), PeerRelation::Agree);
    assert_eq!(s.entry().state, CrossTfState::Running);
    // What the job's delay track reports at 1.5 ppm (`jobs::transfer::cross_tests`).
    s.event(CrossEvent::Drift(Some(0.09e-3)));
    let e = s.entry();
    assert_eq!(e.delay_drift, Some(Seconds(0.09e-3)));
    assert_eq!(e.state, CrossTfState::Running, "a cue, never a refusal");
}

#[test]
fn a_slip_is_said_a_minute() {
    let mut s = Sim::new(|_| 0.0);
    s.run_to(100);
    s.event(CrossEvent::Slipped { by: -235 });
    assert_eq!(
        s.entry().slip.map(|x| (x.by.0 * 48_000.0).round()),
        Some(-235.0)
    );
    s.run_to(161);
    assert_eq!(s.entry().slip, None);
}

#[test]
fn only_one_local_and_one_remote_channel_make_a_cross_rig_transfer_function() {
    use ac2_proto::model::{REMOTE_BASE, TransferConfig};
    let inputs = vec![
        RemoteInput {
            slot: 0,
            rig: RigId(2),
            rig_name: "b".into(),
            channel: 0,
        },
        RemoteInput {
            slot: 1,
            rig: RigId(2),
            rig_name: "b".into(),
            channel: 1,
        },
    ];
    let tf = |r, m| MeasKind::Transfer {
        config: TransferConfig::with_inputs(r, m),
    };
    assert_eq!(
        cross_side(&tf(0, REMOTE_BASE), &inputs),
        Some((RigId(2), RemoteSide::Measurement))
    );
    assert_eq!(
        cross_side(&tf(REMOTE_BASE + 1, 1), &inputs),
        Some((RigId(2), RemoteSide::Reference))
    );
    // Both on one remote rig: one clock, like a local measurement.
    assert_eq!(cross_side(&tf(REMOTE_BASE, REMOTE_BASE + 1), &inputs), None);
    assert_eq!(cross_side(&tf(0, 1), &inputs), None);
}
