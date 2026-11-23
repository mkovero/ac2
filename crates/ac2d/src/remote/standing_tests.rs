#![allow(clippy::unwrap_used)]

use ac2_proto::frame::{LevelsMark, ProtectionFlags, SourceLevelsMeta};
use ac2_proto::model::{
    Ballistic, CantTellReason, ClockBasis, Clocks, DeviceClock, DeviceId, PeerRelation,
    SampleClock, SystemClock,
};
use ac2_proto::units::{Rev, RigId, SampleIndex, Seconds, WallNs};

use super::super::peer_clock::{Evidence, PeerClockWatch};
use super::*;

const RATE: u32 = 48_000;
/// System time at the start of the run, ns.
const SYS0: u64 = 1_790_000_000_000_000_000;
/// The other rig's sample index runs this far ahead of this rig's.
const AHEAD: f64 = 123_456.0;
const FPS: f64 = 30.0;

/// Both rigs as the marks show them: the other rig's clock `ppm` fast against this one's,
/// its system clock `offset_s` ahead, its run `epoch`; this rig's run `ours`.
struct Rigs {
    t0: Instant,
    ppm: f64,
    offset_s: f64,
    theirs_rate: u32,
    epoch: SessionEpoch,
    ours: SessionEpoch,
    marks: bool,
    local: bool,
    feed: StandingFeed,
}

impl Rigs {
    fn new(ppm: f64) -> Self {
        Self {
            t0: Instant::now(),
            ppm,
            offset_s: 0.0,
            theirs_rate: RATE,
            epoch: SessionEpoch(1),
            ours: SessionEpoch(1),
            marks: true,
            local: true,
            feed: StandingFeed::new(),
        }
    }

    fn at(&self, t: f64) -> Instant {
        self.t0 + Duration::from_secs_f64(t)
    }

    /// This rig's mark at `t` s: its newest block starts at `t`, stamps on a stamp clock
    /// that is system time less `SYS0`.
    fn local(&self, t: f64) -> LocalMark {
        let sample = (t * f64::from(RATE)).round() as u64;
        LocalMark {
            epoch: self.ours,
            rate: RATE,
            reference: (sample, (sample as f64 * 1e9 / f64::from(RATE)) as u64),
            line: StampLine {
                epoch_stamp_ns: 0,
                system_ns: SYS0,
                ppm: 0.0,
            },
            latest: sample + 64,
            slack: 128,
            period: 64,
            breaks: 0,
            at: self.at(t),
        }
    }

    /// The other rig's frame captured at `t` s of true time.
    fn frame(&self, t: f64) -> (FrameStamp, SourceLevelsFrame) {
        let sample =
            (AHEAD + t * f64::from(self.theirs_rate) * (1.0 + self.ppm * 1e-6)).round() as u64;
        let stamp = FrameStamp {
            seq: (t * FPS).round() as u64,
            audio_sample: SampleIndex(sample),
            session_epoch: self.epoch,
            daemon_incarnation: DaemonIncarnation(9),
            config_rev: Rev(1),
            config_applied_at: SampleIndex(0),
            capture_wall_ns: WallNs(0),
            grid_id: None,
            protection: ProtectionFlags::NONE,
        };
        let mark = self.marks.then(|| LevelsMark {
            sample: SampleIndex(sample),
            system_ns: (SYS0 as f64 + (t + self.offset_s) * 1e9).round() as u64,
            breaks: 0,
        });
        let f = SourceLevelsFrame {
            meta: SourceLevelsMeta {
                channels: vec![0, 1],
                ballistic: Ballistic::Type1,
                pair_levels: Vec::new(),
                loudness: Vec::new(),
                sample_rate_hz: self.theirs_rate,
                mark,
            },
            peak: vec![-20.0; 2],
            rms: vec![-30.0; 2],
            clip: Vec::new(),
            ppm: vec![-20.0; 2],
            true_peak: vec![-19.0; 2],
            max: vec![-10.0; 2],
            overs: vec![0; 2],
        };
        (stamp, f)
    }

    /// Frames at 30 a second over `from..to` s, each arriving 5 ms after its capture.
    fn run(&mut self, from: f64, to: f64) {
        let first = (from * FPS).round() as u64;
        let last = (to * FPS).round() as u64;
        for n in first..last {
            let t = n as f64 / FPS;
            let (stamp, f) = self.frame(t);
            let arrive = t + 0.005;
            let local = self.local.then(|| self.local(arrive));
            let now = self.at(arrive);
            self.feed.frame(&stamp, f, local, Some(64), now);
        }
    }

    fn report(&self, t: f64) -> StandingReport {
        self.feed.report(self.at(t))
    }
}

fn clocks() -> Clocks {
    Clocks {
        system: SystemClock::Synced { maxerror_us: 100 },
        capture: Some(DeviceClock {
            device: DeviceId("fake".into()),
            rate: SampleClock::Measured {
                ppm: 0.0,
                span: Seconds(60.0),
                against: ClockBasis::SystemClock,
            },
        }),
        stamp_map: None,
    }
}

/// The relation the judge gives the report, as the control thread asks it.
fn judged(r: &StandingReport) -> PeerRelation {
    let c = clocks();
    let e = Evidence {
        ours: (RigId(1), &c),
        theirs: (RigId(2), Some(&c)),
        capture_stamps: true,
        long: r.long,
        short: r.short,
        streaming_for: r.marks_for.filter(|_| r.absent.is_none()),
        disagree: r.disagree,
        absent: CantTellReason::NoMarks,
        other_rate: r.other_rate,
    };
    let mut w = PeerClockWatch::absent(CantTellReason::NoMarks);
    w.update(&e, Duration::from_secs(1), WallNs(1));
    w.clock.relation
}

#[test]
fn marks_alone_reach_agreement_after_a_minute() {
    let mut r = Rigs::new(0.0);
    r.run(0.0, 30.0);
    let rep = r.report(30.0);
    assert_eq!(rep.absent, None);
    assert_eq!(judged(&rep), PeerRelation::Measuring);
    r.run(30.0, 65.0);
    let rep = r.report(65.0);
    let (ppm, span) = rep.long.unwrap();
    assert!(ppm.abs() < 0.01, "{ppm}");
    assert!(span > 54.0, "{span}");
    assert_eq!(rep.disagree, None);
    assert!(rep.marks_for.unwrap() >= Duration::from_secs(64));
    assert_eq!(judged(&rep), PeerRelation::Agree);
    assert_eq!(rep.levels.unwrap().meta.channels, vec![0, 1]);
}

#[test]
fn a_rate_apart_is_measured_and_not_locked() {
    let mut r = Rigs::new(5.0);
    r.run(0.0, 65.0);
    let rep = r.report(65.0);
    let (ppm, _) = rep.long.unwrap();
    assert!((ppm - 5.0).abs() < 0.05, "{ppm}");
    assert!(matches!(judged(&rep), PeerRelation::NotLocked { .. }));
}

#[test]
fn a_link_gap_is_a_hole_and_the_feed_carries_on() {
    let mut r = Rigs::new(0.0);
    r.run(0.0, 40.0);
    // No frame for 15 s: nothing to judge, and nothing started again.
    let rep = r.report(55.0);
    assert_eq!(rep.absent, Some(Absent::TheirSession));
    assert!(rep.levels.is_none());
    assert!(matches!(judged(&rep), PeerRelation::CantTell { .. }));
    r.run(55.0, 70.0);
    let rep = r.report(70.0);
    assert_eq!(rep.absent, None);
    // The minute counts from the start, the hole inside it.
    assert!(rep.marks_for.unwrap() >= Duration::from_secs(69));
    let (ppm, _) = rep.long.unwrap();
    assert!(ppm.abs() < 0.01, "{ppm}");
    assert_eq!(judged(&rep), PeerRelation::Agree);
}

#[test]
fn either_rig_reopening_its_session_starts_the_windows_again() {
    let mut r = Rigs::new(0.0);
    r.run(0.0, 65.0);
    assert!(r.report(65.0).long.is_some());
    r.epoch = SessionEpoch(2);
    r.run(65.0, 70.0);
    let rep = r.report(70.0);
    assert!(rep.long.is_none());
    assert!(rep.marks_for.unwrap() < Duration::from_secs(6));
    assert_eq!(judged(&rep), PeerRelation::Measuring);

    let mut r = Rigs::new(0.0);
    r.run(0.0, 65.0);
    r.ours = SessionEpoch(7);
    r.run(65.0, 66.0);
    let rep = r.report(66.0);
    assert!(rep.long.is_none());
    assert!(rep.marks_for.unwrap() < Duration::from_secs(2));
}

#[test]
fn another_nominal_rate_is_not_locked_and_fits_nothing() {
    let mut r = Rigs::new(0.0);
    r.theirs_rate = 96_000;
    r.run(0.0, 65.0);
    let rep = r.report(65.0);
    assert_eq!(rep.other_rate, Some((96_000, RATE)));
    assert!(rep.long.is_none());
    assert!(matches!(
        judged(&rep),
        PeerRelation::NotLocked {
            reason: ac2_proto::model::NotLockedReason::OtherRate {
                theirs: 96_000,
                ours: RATE
            }
        }
    ));
}

#[test]
fn system_clocks_apart_are_found_from_the_marks() {
    // Ahead: its marks land in this rig's future.
    let mut r = Rigs::new(0.0);
    r.offset_s = 0.050;
    r.run(0.0, 3.0);
    // Less the 5 ms transit, this rig's block in hand and its slack.
    let by = r.report(3.0).disagree.unwrap();
    assert!(by > 0.035 && by < 0.045, "{by}");
    assert!(matches!(
        judged(&r.report(3.0)),
        PeerRelation::CantTell {
            reason: CantTellReason::ClocksDisagree { .. }
        }
    ));
    // Behind by a second: older than any transit, once the least age of a second says so.
    let mut r = Rigs::new(0.0);
    r.offset_s = -1.0;
    r.run(0.0, 0.5);
    assert_eq!(r.report(0.5).disagree, None);
    r.run(0.5, 3.0);
    let by = r.report(3.0).disagree.unwrap();
    assert!(by < -0.8, "{by}");
    // Within transit: nothing.
    let mut r = Rigs::new(0.0);
    r.run(0.0, 3.0);
    assert_eq!(r.report(3.0).disagree, None);
}

#[test]
fn frames_without_marks_or_without_a_session_here_say_why() {
    let mut r = Rigs::new(0.0);
    r.marks = false;
    r.run(0.0, 5.0);
    let rep = r.report(5.0);
    assert_eq!(rep.absent, Some(Absent::Marks));
    assert!(rep.levels.is_some());

    let mut r = Rigs::new(0.0);
    r.local = false;
    r.run(0.0, 5.0);
    assert_eq!(r.report(5.0).absent, Some(Absent::OurSession));
}

#[test]
fn a_local_mark_lapses_when_the_stream_stops_writing_it() {
    let c = LocalClock::default();
    let r = Rigs::new(0.0);
    c.set(Some(r.local(0.0)));
    assert!(c.get(r.at(0.5)).is_some());
    assert!(c.get(r.at(1.5)).is_none());
    c.set(None);
    assert!(c.get(r.at(0.1)).is_none());
}
