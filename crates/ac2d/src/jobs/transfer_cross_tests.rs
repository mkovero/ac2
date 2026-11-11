//! A transfer job between two rigs on synthetic audio, in audio time: the clock gate's
//! hold and relock, the finder kept over a remote gap, a slip moving the delay, the late cue.

use std::sync::mpsc::{Receiver, channel};

use ac2_audio::BlockFlags;
use ac2_proto::model::{StopWhy, TransferConfig};
use ac2_proto::units::{MeasId, Rev, SessionEpoch};

use super::*;
use crate::conv::FindBand;
use crate::jobs::cross_rig::{ClockGate, CrossEvent};
use crate::jobs::{Analysis, JobCmd, JobCtx};

const FS: u32 = 48_000;
const BLOCK: usize = 4800;
/// Column 0: this rig's reference; column 1: the remote measurement input.
const REMOTE: u16 = 1;

struct Rig {
    job: Transfer,
    rx: Receiver<ControlMsg>,
    /// White noise, the reference, long enough for any test.
    noise: Vec<f32>,
    at: u64,
    /// The measurement's arrival, samples.
    delay: usize,
    /// An earlier, weaker arrival (an ambiguous path): samples, gain.
    echo: Option<(usize, f32)>,
    /// Messages to the control thread, as read.
    msgs: Vec<ControlMsg>,
}

fn noise(n: usize, mut x: u64) -> Vec<f32> {
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x as f64 / u64::MAX as f64 * 2.0 - 1.0) as f32 * 0.5
        })
        .collect()
}

fn rig(delay: usize, tracking: bool, cross: bool) -> Rig {
    let (tx, rx) = channel();
    let d = DelayState {
        applied: Seconds(delay as f64 / f64::from(FS)),
        applied_samples: delay as f64,
        nudged: Seconds(0.0),
        nudged_samples: 0.0,
        tracking,
        awaiting_pick: false,
        last_finding: None,
    };
    let job = Transfer::new(
        JobCtx {
            meas: MeasId(3),
            cfg: TransferConfig::with_inputs(0, REMOTE),
            config_rev: Rev(1),
            epoch: SessionEpoch(1),
        },
        FS,
        (0, 1),
        &d,
        tx,
        None,
        cross,
    )
    .expect("a transfer job");
    Rig {
        job,
        rx,
        noise: noise(FS as usize * 200, 7),
        at: 0,
        delay,
        echo: None,
        msgs: Vec::new(),
    }
}

impl Rig {
    /// The next block, its remote column marked by `mark`.
    fn block(&mut self, mark: impl Fn(&mut crate::remote::ColumnMarks)) {
        let mut data = Vec::with_capacity(BLOCK * 2);
        for i in 0..BLOCK {
            let n = self.at as usize + i;
            let r = self.noise[n % self.noise.len()];
            let at = |d: usize| {
                n.checked_sub(d)
                    .map_or(0.0, |k| self.noise[k % self.noise.len()])
            };
            let m = at(self.delay) + self.echo.map_or(0.0, |(e, g)| g * at(e));
            data.push(r);
            data.push(m);
        }
        let mut b = Block::new(self.at, BLOCK as u32, 2, BlockFlags::NONE, 0, data);
        mark(&mut b.marks);
        self.job.push(&b);
        self.at += BLOCK as u64;
    }

    fn run(&mut self, seconds: f64) {
        let n = (seconds * f64::from(FS) / BLOCK as f64).round() as usize;
        for _ in 0..n {
            self.block(|_| {});
        }
    }

    fn gate(&mut self, g: ClockGate) {
        self.job.command(JobCmd::Clocks(g));
    }

    /// The cross-rig events sent since the last call.
    fn events(&mut self) -> Vec<CrossEvent> {
        self.msgs.extend(self.rx.try_iter());
        let (out, keep) = std::mem::take(&mut self.msgs)
            .into_iter()
            .partition::<Vec<_>, _>(|m| matches!(m, ControlMsg::CrossRig { .. }));
        self.msgs = keep;
        out.into_iter()
            .filter_map(|m| match m {
                ControlMsg::CrossRig { event, .. } => Some(event),
                _ => None,
            })
            .collect()
    }

    /// The tracked delays sent since the last call.
    fn tracked(&mut self) -> Vec<f64> {
        self.msgs.extend(self.rx.try_iter());
        let (out, keep) = std::mem::take(&mut self.msgs)
            .into_iter()
            .partition::<Vec<_>, _>(|m| matches!(m, ControlMsg::DelayTracked { .. }));
        self.msgs = keep;
        out.into_iter()
            .filter_map(|m| match m {
                ControlMsg::DelayTracked { samples, .. } => Some(samples),
                _ => None,
            })
            .collect()
    }

    /// What the control thread does with a tracked delay: sends it back.
    fn apply(&mut self, samples: f64) {
        self.job.command(JobCmd::SetDelay {
            samples,
            seconds: samples / f64::from(FS),
            nudged_samples: 0.0,
            rev: Rev(2),
            resume: false,
        });
    }
}

const NOT_LOCKED_40: ClockGate = ClockGate::Stop {
    why: StopWhy::NotLocked,
    window_s: 40.0,
};

#[test]
fn nothing_averages_before_the_clocks_agree() {
    let mut r = rig(480, false, true);
    r.run(2.0);
    assert!(
        r.job.capture().is_none(),
        "no result while measuring clocks"
    );
    r.gate(ClockGate::Run);
    r.run(3.0);
    assert!(r.job.capture().is_some());
    // The first agreement is a start, not a relock.
    assert!(r.events().is_empty());
}

#[test]
fn a_stop_holds_the_result_from_before_the_failed_window_or_nothing() {
    let mut r = rig(480, false, true);
    r.gate(ClockGate::Run);
    // Kept at 20 s and 40 s; the 40 s window failing at 50 s began at 10 s: nothing kept
    // before it.
    r.run(50.0);
    r.gate(NOT_LOCKED_40);
    assert!(matches!(
        r.events()[..],
        [CrossEvent::Stopped { held: false }]
    ));
    assert!(r.job.capture().is_none(), "nothing from before the failure");

    let mut r = rig(480, false, true);
    r.gate(ClockGate::Run);
    // Kept at 20, 40 and 60 s; failing at 65 s over 40 s: the result at 20 s stands.
    r.run(65.0);
    r.gate(NOT_LOCKED_40);
    assert!(matches!(
        r.events()[..],
        [CrossEvent::Stopped { held: true }]
    ));
    let held = r.job.capture().expect("the held result");
    // Stopped: the audio of the stop changes nothing shown.
    r.delay += 48;
    r.run(5.0);
    // NaN columns compare unequal; their printout does not.
    let shown = r.job.capture().map(|c| format!("{:?}", c.1));
    assert_eq!(shown, Some(format!("{:?}", held.1)));
}

#[test]
fn a_pull_relocks_tracking_from_the_held_delay() {
    let mut r = rig(480, true, true);
    r.gate(ClockGate::Run);
    r.run(8.0);
    r.tracked();
    r.gate(NOT_LOCKED_40);
    r.events();
    // The clocks slid 1 ms apart while not locked (a crystal fallback keeps what it slid).
    r.delay += 48;
    r.run(6.0);
    assert!(
        r.tracked().is_empty(),
        "tracking observes nothing while stopped"
    );
    r.gate(ClockGate::Run);
    r.run(8.0);
    let moved = r.tracked();
    assert_eq!(
        moved.iter().map(|d| d.round() as i64).collect::<Vec<_>>(),
        vec![528]
    );
    let ev = r.events();
    let relocked = ev
        .iter()
        .find_map(|e| match e {
            CrossEvent::Relocked { was, now } => Some((*was, *now)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("relocked: {ev:?}"));
    assert_eq!(relocked.0, 480.0);
    assert_eq!(relocked.1.round(), 528.0);
}

#[test]
fn a_relock_on_an_unmoved_delay_is_still_reported() {
    let mut r = rig(480, true, true);
    r.gate(ClockGate::Run);
    r.run(6.0);
    r.gate(ClockGate::Stop {
        why: StopWhy::CantTell,
        window_s: 0.0,
    });
    r.run(2.0);
    r.gate(ClockGate::Run);
    r.run(8.0);
    let ev = r.events();
    assert!(
        ev.iter()
            .any(|e| matches!(e, CrossEvent::Relocked { was, now } if *was == 480.0 && (now - 480.0).abs() < 1.0)),
        "{ev:?}"
    );
}

#[test]
fn an_ambiguous_relock_asks_the_operator() {
    let mut r = rig(480, true, true);
    r.gate(ClockGate::Run);
    r.run(6.0);
    r.gate(NOT_LOCKED_40);
    // A weaker arrival ahead of the strong one: which is the first is the operator's call.
    r.echo = Some((280, 10f32.powf(-12.3 / 20.0)));
    r.run(2.0);
    r.gate(ClockGate::Run);
    r.run(12.0);
    let ev = r.events();
    assert!(
        ev.iter()
            .any(|e| matches!(e, CrossEvent::RelockAmbiguous(_))),
        "{ev:?}"
    );
    assert!(r.job.finder.paused(), "tracking waits for the pick");
}

#[test]
fn tracking_off_says_find_it_again() {
    let mut r = rig(480, false, true);
    r.gate(ClockGate::Run);
    r.run(2.0);
    r.gate(NOT_LOCKED_40);
    r.run(1.0);
    r.gate(ClockGate::Run);
    let ev = r.events();
    assert!(
        matches!(ev[..], [CrossEvent::Stopped { .. }, CrossEvent::FindAgain]),
        "{ev:?}"
    );
}

#[test]
fn a_remote_gap_restarts_the_averages_but_keeps_the_finder() {
    let mut r = rig(480, false, false);
    r.run(6.0);
    r.block(|m| {
        for f in 100..200 {
            m.gap(REMOTE, f);
        }
    });
    // The history survived the gap: a find right after has its audio.
    r.job.command(JobCmd::Find {
        token: 1,
        band: FindBand::Auto,
        observation: None,
    });
    let found =
        r.rx.try_iter()
            .find_map(|m| match m {
                ControlMsg::DelayFound { result, .. } => Some(result),
                _ => None,
            })
            .expect("a finding");
    let pick = found
        .as_ref()
        .as_ref()
        .expect("valid")
        .pick()
        .map(|p| p.delay);
    assert_eq!(pick, Some(480));
    assert!(
        r.job.discontinuity_until > r.at - BLOCK as u64,
        "the restart is flagged"
    );
}

#[test]
fn a_slip_moves_the_delay_by_exactly_the_slip() {
    let mut r = rig(480, false, true);
    r.gate(ClockGate::Run);
    r.run(1.0);
    r.events();
    // The remote map moved 235 samples later: the measurement arrives that much later.
    r.delay += 235;
    r.block(|m| m.slip(REMOTE, 235));
    assert_eq!(r.job.delay_samples, 715.0);
    let ev = r.events();
    assert!(
        matches!(ev[..], [CrossEvent::Slipped { by: 235 }]),
        "{ev:?}"
    );

    // On the reference side the arrival moves against the slip.
    let (tx, _rx) = channel();
    let d = DelayState {
        applied: Seconds(0.01),
        applied_samples: 480.0,
        nudged: Seconds(0.0),
        nudged_samples: 0.0,
        tracking: false,
        awaiting_pick: false,
        last_finding: None,
    };
    let mut job = Transfer::new(
        JobCtx {
            meas: MeasId(4),
            cfg: TransferConfig::with_inputs(REMOTE, 0),
            config_rev: Rev(1),
            epoch: SessionEpoch(1),
        },
        FS,
        (1, 0),
        &d,
        tx,
        None,
        true,
    )
    .expect("a transfer job");
    let quiet = |start: u64| Block::new(start, 64, 2, BlockFlags::NONE, 0, vec![0.0; 128]);
    job.push(&quiet(0));
    let mut b = quiet(64);
    b.marks.slip(REMOTE, 100);
    job.push(&b);
    assert_eq!(job.delay_samples, 380.0);
}

#[test]
fn late_twice_in_a_settle_span_raises_the_cue() {
    let mut r = rig(480, false, true);
    r.gate(ClockGate::Run);
    r.run(1.0);
    r.events();
    r.block(|m| m.gap(REMOTE, 0));
    r.run(1.0);
    r.block(|m| m.gap(REMOTE, 0));
    let ev = r.events();
    assert!(matches!(ev[..], [CrossEvent::Late(true)]), "{ev:?}");
}

#[test]
fn the_tracked_delay_drifting_steadily_is_a_cue() {
    // Two crystals 1.5 ppm apart within the 2 ppm bound read Agree; the delay track shows
    // the rest: 0.09 ms a minute, a sample every 14 s at 48 kHz.
    let mut r = rig(480, true, true);
    r.gate(ClockGate::Run);
    let step_s = 1.0 / (1.5e-6 * f64::from(FS));
    let mut t = 0.0;
    let mut cue = None;
    while t < 130.0 {
        r.run(1.0);
        t += 1.0;
        r.delay = 480 + (t / step_s) as usize;
        for d in r.tracked() {
            r.apply(d);
        }
        for e in r.events() {
            if let CrossEvent::Drift(c) = e {
                cue = c;
            }
        }
    }
    let c = cue.expect("a drift cue");
    assert!((c * 1e3 - 0.09).abs() < 0.02, "{} ms/min", c * 1e3);
}
