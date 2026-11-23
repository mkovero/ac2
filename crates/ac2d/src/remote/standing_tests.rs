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
/// This rig's device period, frames.
const PERIOD: u32 = 64;
const MINUTE: f64 = 60.0;

/// Both rigs on one true time `t` (s): each sample clock `*_ppm` fast against it, this rig's
/// stamp clock `stamp_ppm` fast, the other rig's system clock `offset(t)` s ahead of this
/// rig's, its run `epoch`; this rig's run `ours`.
struct Rigs {
    t0: Instant,
    their_ppm: f64,
    our_ppm: f64,
    stamp_ppm: f64,
    offset: Box<dyn Fn(f64) -> f64>,
    /// Arrival after capture beyond 5 ms, s, per frame number.
    jitter: Box<dyn Fn(u64) -> f64>,
    theirs_rate: u32,
    epoch: SessionEpoch,
    ours: SessionEpoch,
    marks: bool,
    local: bool,
    feed: StandingFeed,
}

impl Rigs {
    fn new(their_ppm: f64) -> Self {
        Self {
            t0: Instant::now(),
            their_ppm,
            our_ppm: 0.0,
            stamp_ppm: 0.0,
            offset: Box::new(|_| 0.0),
            jitter: Box::new(|_| 0.0),
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

    /// This rig's sample index at true time `t`.
    fn our_sample(&self, t: f64) -> f64 {
        t * f64::from(RATE) * (1.0 + self.our_ppm * 1e-6)
    }

    /// This rig's mark as the fan-out writes it at `t`: its newest block started at the last
    /// period boundary, stamped on a stamp clock `stamp_ppm` fast, which its line puts back
    /// on system time.
    fn local(&self, t: f64) -> LocalMark {
        let k = 1.0 + self.stamp_ppm * 1e-6;
        let r = f64::from(RATE) * (1.0 + self.our_ppm * 1e-6);
        let block = (self.our_sample(t) / f64::from(PERIOD)).floor() * f64::from(PERIOD);
        let t_block = block / r;
        LocalMark {
            epoch: self.ours,
            rate: RATE,
            reference: (block as u64, (t_block * k * 1e9).round() as u64),
            line: StampLine {
                epoch_stamp_ns: 0,
                system_ns: SYS0,
                ppm: (1.0 / k - 1.0) * 1e6,
            },
            latest: self.our_sample(t) as u64,
            slack: 2 * u64::from(PERIOD),
            period: PERIOD,
            breaks: 0,
            at: self.at(t),
        }
    }

    /// The other rig's frame captured at true time `t`.
    fn frame(&self, t: f64) -> (FrameStamp, SourceLevelsFrame) {
        let sample = (AHEAD + t * f64::from(self.theirs_rate) * (1.0 + self.their_ppm * 1e-6))
            .round() as u64;
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
            system_ns: (SYS0 as f64 + (t + (self.offset)(t)) * 1e9).round() as u64,
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
        self.run_checking(from, to, |_, _| {});
    }

    /// [`Self::run`], handing `check` the report every whole second.
    fn run_checking(&mut self, from: f64, to: f64, mut check: impl FnMut(f64, &StandingReport)) {
        let first = (from * FPS).round() as u64;
        let last = (to * FPS).round() as u64;
        for n in first..last {
            let t = n as f64 / FPS;
            let (stamp, f) = self.frame(t);
            let arrive = t + 0.005 + (self.jitter)(n);
            let local = self.local.then(|| self.local(arrive));
            let now = self.at(arrive);
            self.feed.frame(&stamp, f, local, Some(PERIOD), now);
            if n % FPS as u64 == 0 {
                check(t, &self.feed.report(now));
            }
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

/// The relation shown from the report, as the control thread judges and holds it.
fn judged(r: &StandingReport) -> PeerRelation {
    let c = clocks();
    let e = Evidence {
        ours: (RigId(1), &c),
        theirs: (RigId(2), Some(&c)),
        capture_stamps: true,
        long: r.long,
        short: r.short,
        streaming_for: r.marks_for.filter(|_| r.absent.is_none()),
        disagree: r.ahead,
        late: None,
        absent: CantTellReason::NoMarks,
        other_rate: r.other_rate,
    };
    let mut w = PeerClockWatch::absent(CantTellReason::NoMarks);
    w.update(&e, Duration::from_secs(1), WallNs(1));
    shown(w.clock.relation, r.held)
}

fn ppm(r: &StandingReport) -> f64 {
    r.long.unwrap().0
}

fn cross_rate(r: &PeerRelation) -> bool {
    matches!(
        r,
        PeerRelation::NotLocked {
            reason: NotLockedReason::CrossRate { .. }
        }
    )
}

/// Long enough for the verdict: the first window, then every minute held.
fn held_by() -> f64 {
    (HOLD_WINDOWS as f64 + 1.0) * MINUTE + 5.0
}

#[test]
fn marks_alone_measure_a_minute_and_agree_once_held() {
    let mut r = Rigs::new(0.0);
    r.run(0.0, 30.0);
    let rep = r.report(30.0);
    assert_eq!(rep.absent, None);
    assert_eq!(judged(&rep), PeerRelation::Measuring);
    r.run(30.0, 65.0);
    let rep = r.report(65.0);
    let (p, span) = rep.long.unwrap();
    assert!(p.abs() < 0.01, "{p}");
    assert!(span > 54.0, "{span}");
    assert_eq!(rep.ahead, None);
    assert!(rep.marks_for.unwrap() >= Duration::from_secs(64));
    // A minute within the rule is a figure, not yet a verdict.
    assert_eq!(judged(&rep), PeerRelation::Measuring);
    assert_eq!(rep.levels.unwrap().meta.channels, vec![0, 1]);
    r.run(65.0, held_by());
    let rep = r.report(held_by());
    assert_eq!(rep.held, Some(Held::Agree));
    assert_eq!(judged(&rep), PeerRelation::Agree);
}

#[test]
fn a_rate_apart_is_not_locked_once_held() {
    let mut r = Rigs::new(5.0);
    r.run(0.0, 65.0);
    let rep = r.report(65.0);
    assert!((ppm(&rep) - 5.0).abs() < 0.05, "{}", ppm(&rep));
    assert_eq!(judged(&rep), PeerRelation::Measuring);
    r.run(65.0, held_by());
    let rep = r.report(held_by());
    assert_eq!(rep.held, Some(Held::Apart));
    assert!(cross_rate(&judged(&rep)), "{:?}", judged(&rep));
}

#[test]
fn both_rigs_off_system_time_by_the_same_rate_agree() {
    // Each sample clock 30 ppm fast of true time, this rig's stamp clock 12 ppm slow,
    // arrivals jittered by up to 3 ms: only the rate between the sample clocks is left.
    let mut r = Rigs::new(30.0);
    r.our_ppm = 30.0;
    r.stamp_ppm = -12.0;
    r.jitter = Box::new(|n| (n.wrapping_mul(2_654_435_761) % 3001) as f64 * 1e-6);
    r.run(0.0, 65.0);
    let rep = r.report(65.0);
    assert!(ppm(&rep).abs() < 0.05, "{}", ppm(&rep));
    assert_eq!(rep.ahead, None);
    // A sample clock apart reads as exactly that, whatever the stamp clock does.
    let mut r = Rigs::new(33.0);
    r.our_ppm = 30.0;
    r.stamp_ppm = 50.0;
    r.run(0.0, 65.0);
    let p = ppm(&r.report(65.0));
    assert!((p - 3.0).abs() < 0.05, "{p}");
}

#[test]
fn a_gap_longer_than_a_lapse_starts_the_minute_again() {
    // The other rig's system clock moves 3 ms while no mark comes for ten minutes.
    let mut r = Rigs::new(0.0);
    r.offset = Box::new(|t| if t < 100.0 { 0.0 } else { 0.003 });
    r.run(0.0, 40.0);
    let rep = r.report(55.0);
    assert_eq!(rep.absent, Some(Absent::TheirFrames));
    assert!(rep.long.is_none() && rep.marks_for.is_none());
    r.run(640.0, 645.0);
    let rep = r.report(645.0);
    // Five seconds of marks: no fit bridging the hole, no verdict, the count from 640 s.
    assert!(rep.long.is_none() && rep.short.is_none(), "{rep:?}");
    assert!(rep.marks_for.unwrap() < Duration::from_secs(6));
    assert_eq!(judged(&rep), PeerRelation::Measuring);
    r.run(645.0, 705.0);
    let rep = r.report(705.0);
    assert!(ppm(&rep).abs() < 0.05, "{}", ppm(&rep));
    assert_eq!(rep.held, None);
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
fn another_nominal_rate_is_not_locked_until_this_rig_closes() {
    let mut r = Rigs::new(0.0);
    r.theirs_rate = 96_000;
    r.run(0.0, 65.0);
    let rep = r.report(65.0);
    assert_eq!(rep.other_rate, Some((96_000, RATE)));
    assert!(rep.long.is_none());
    assert_eq!(
        judged(&rep),
        PeerRelation::NotLocked {
            reason: NotLockedReason::OtherRate {
                theirs: 96_000,
                ours: RATE
            }
        }
    );
    // This rig's session closes: no rate of its own to differ from.
    r.local = false;
    r.run(65.0, 66.0);
    let rep = r.report(66.0);
    assert_eq!(rep.other_rate, None);
    assert_eq!(rep.absent, Some(Absent::OurSession));
}

#[test]
fn system_clocks_apart_are_found_from_marks_that_hold_a_second() {
    // Ahead: its marks land in this rig's future, every one for a second.
    let mut r = Rigs::new(0.0);
    r.offset = Box::new(|_| 0.050);
    r.run(0.0, 0.5);
    assert_eq!(r.report(0.5).ahead, None);
    r.run(0.5, 3.0);
    // Less the 5 ms transit and this rig's slack.
    let by = r.report(3.0).ahead.unwrap();
    assert!(by > 0.035 && by < 0.045, "{by}");
    assert!(matches!(
        judged(&r.report(3.0)),
        PeerRelation::CantTell {
            reason: CantTellReason::ClocksDisagree { .. }
        }
    ));
    // One mark in the future among the others is a stamp's jitter, not a clock.
    let mut r = Rigs::new(0.0);
    r.offset = Box::new(|t| if (t - 2.0).abs() < 0.01 { 0.050 } else { 0.0 });
    r.run(0.0, 2.5);
    assert_eq!(r.report(2.5).ahead, None);
    // Behind by a second: the least age says how late, the control thread how much of it
    // a link offset allows.
    let mut r = Rigs::new(0.0);
    r.offset = Box::new(|_| -1.0);
    r.run(0.0, 3.0);
    let rep = r.report(3.0);
    let age = rep.least_age.unwrap();
    assert!((age - 1.005).abs() < 0.004, "{age}");
    assert_eq!(rep.ahead, None);
    // Within transit: nothing ahead, the least age the transit.
    let mut r = Rigs::new(0.0);
    r.run(0.0, 3.0);
    let rep = r.report(3.0);
    assert_eq!(rep.ahead, None);
    assert!(rep.least_age.unwrap() < 0.01);
}

#[test]
fn a_slew_on_one_host_never_reads_as_clocks_apart() {
    // A word-locked pair; the other host's clock is slewed 1 ms over a minute (17 ppm while
    // it lasts), twice.
    let mut r = Rigs::new(0.0);
    let slew = |t: f64, from: f64| ((t - from) / MINUTE).clamp(0.0, 1.0) * 0.001;
    r.offset = Box::new(move |t| slew(t, 200.0) + slew(t, 500.0));
    let mut beyond = false;
    r.run_checking(0.0, 1000.0, |t, rep| {
        let rel = judged(rep);
        assert!(
            !matches!(rel, PeerRelation::NotLocked { .. }),
            "{t} s: {rel:?} {rep:?}"
        );
        beyond |= rep.long.is_some_and(|(p, _)| p.abs() > 2.0);
    });
    // The fit saw it: a window over the slew read beyond the rule, and the verdict waited.
    assert!(beyond);
}

#[test]
fn an_opposing_slew_does_not_hide_a_steady_rate_past_the_hold() {
    let mut r = Rigs::new(3.0);
    r.offset = Box::new(|t| -((t - 300.0) / MINUTE).clamp(0.0, 1.0) * 0.001);
    let end = 360.0 + held_by();
    r.run(0.0, end);
    let rep = r.report(end);
    assert_eq!(rep.held, Some(Held::Apart));
    assert!(cross_rate(&judged(&rep)));
}

#[test]
fn frames_without_marks_or_without_a_session_here_say_why() {
    let mut r = Rigs::new(0.0);
    r.marks = false;
    r.run(0.0, 5.0);
    let rep = r.report(5.0);
    assert_eq!(rep.absent, Some(Absent::Marks));
    assert!(rep.levels.is_some());
    assert!(rep.marks_for.is_none());

    let mut r = Rigs::new(0.0);
    r.local = false;
    r.run(0.0, 5.0);
    assert_eq!(r.report(5.0).absent, Some(Absent::OurSession));
}

#[test]
fn figures_go_while_the_feed_is_absent() {
    let mut r = Rigs::new(0.0);
    r.run(0.0, 65.0);
    let rep = r.report(65.0);
    assert!(rep.long.is_some() && rep.marks_for.is_some());
    // Frames go: nothing is reported as if it were current.
    let rep = r.report(70.0);
    assert!(rep.long.is_none() && rep.short.is_none() && rep.marks_for.is_none());
    assert_eq!(rep.held, None);
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
