#![allow(clippy::unwrap_used)]

use ac2_proto::frame::{LevelsMark, ProtectionFlags, SourceLevelsMeta};
use ac2_proto::model::{
    Ballistic, CantTellReason, ClockBasis, Clocks, DeviceClock, DeviceId, MinuteCount,
    NotLockedReason, PeerRelation, SampleClock, SystemClock,
};
use ac2_proto::units::{Rev, RigId, SampleIndex, Seconds, WallNs};

use super::super::peer_clock::Evidence;
use super::super::standing_verdict::{HOLD_MINUTES, Shown, StandingWatch, late};
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
    /// The other rig's own sample clock against its system clock, as it states it, ppm.
    own_ppm: f64,
    feed: StandingFeed,
    watch: StandingWatch,
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
            own_ppm: 0.0,
            feed: StandingFeed::new(),
            watch: StandingWatch::new(),
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
        self.run_checking(from, to, |_, _, _| {});
    }

    /// [`Self::run`], judging every whole second as the control thread does and handing
    /// `check` the report and what is shown.
    fn run_checking(
        &mut self,
        from: f64,
        to: f64,
        mut check: impl FnMut(f64, &StandingReport, &Shown),
    ) {
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
                let rep = self.feed.report(now);
                let shown = self.judge(&rep, arrive);
                check(t, &rep, &shown);
            }
        }
    }

    fn report(&self, t: f64) -> StandingReport {
        self.feed.report(self.at(t))
    }

    /// The report at `t` as the control thread judges it, with no link offset set.
    fn judge(&mut self, r: &StandingReport, t: f64) -> Shown {
        let (ours, theirs) = (clocks(0.0), clocks(self.own_ppm));
        let fed = Some(r).filter(|r| r.absent.is_none());
        let e = Evidence {
            ours: (RigId(1), &ours),
            theirs: (RigId(2), Some(&theirs)),
            capture_stamps: true,
            long: fed.and_then(|r| r.long),
            short: fed.and_then(|r| r.short),
            streaming_for: fed.and_then(|r| r.marks_for),
            disagree: fed.and_then(|r| r.ahead),
            late: fed.and_then(|r| late(r, None)),
            absent: CantTellReason::NoMarks,
            other_rate: r.other_rate,
        };
        self.watch.update(
            e,
            fed.and_then(|r| r.minutes),
            Duration::from_secs_f64(t),
            WallNs(1),
        )
    }

    /// The relation shown at `t`, after the frames run so far.
    fn shown(&mut self, t: f64) -> PeerRelation {
        let rep = self.report(t);
        self.judge(&rep, t).relation
    }
}

fn clocks(own_ppm: f64) -> Clocks {
    Clocks {
        system: SystemClock::Synced { maxerror_us: 100 },
        capture: Some(DeviceClock {
            device: DeviceId("fake".into()),
            rate: SampleClock::Measured {
                ppm: own_ppm,
                span: Seconds(60.0),
                against: ClockBasis::SystemClock,
            },
        }),
        stamp_map: None,
    }
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

fn not_steady(r: &PeerRelation) -> Option<(f64, f64)> {
    match r {
        PeerRelation::CantTell {
            reason: CantTellReason::NotSteady { low, high },
        } => Some((*low, *high)),
        _ => None,
    }
}

/// The minute a not-locked verdict closes at: the [`HOLD_MINUTES`]th.
fn held_at() -> f64 {
    HOLD_MINUTES as f64 * MINUTE
}

/// The other rig's system clock offset that reads as `ppm(k)` in whole minute `k` (from
/// 0 s): a rate on the marks' system time alone.
fn minute_rates(ppm: impl Fn(u64) -> f64 + 'static) -> Box<dyn Fn(f64) -> f64> {
    Box::new(move |t| {
        let whole = (t / MINUTE).floor() as u64;
        let before: f64 = (0..whole).map(|k| ppm(k) * MINUTE).sum();
        (before + ppm(whole) * (t - whole as f64 * MINUTE)) * 1e-6
    })
}

#[test]
fn marks_alone_agree_once_the_first_minute_closes_within() {
    let mut r = Rigs::new(0.0);
    r.run(0.0, 30.0);
    let rep = r.report(30.0);
    assert_eq!(rep.absent, None);
    assert_eq!(r.shown(30.0), PeerRelation::Measuring);
    r.run(30.0, 59.9);
    assert_eq!(r.shown(59.95), PeerRelation::Measuring);
    r.run(59.9, 61.0);
    let rep = r.report(61.0);
    let (p, span) = rep.long.unwrap();
    assert!(p.abs() < 0.01, "{p}");
    assert!(span > 54.0, "{span}");
    assert_eq!(rep.ahead, None);
    assert!(rep.marks_for.unwrap() >= Duration::from_secs(60));
    assert_eq!(rep.levels.unwrap().meta.channels, vec![0, 1]);
    assert_eq!(r.shown(61.0), PeerRelation::Agree);
}

#[test]
fn a_rate_apart_counts_ten_minutes_then_is_not_locked() {
    let mut r = Rigs::new(5.0);
    r.run(0.0, 65.0);
    let rep = r.report(65.0);
    assert!((ppm(&rep) - 5.0).abs() < 0.05, "{}", ppm(&rep));
    r.run(65.0, held_at() - 5.0);
    let rep = r.report(held_at() - 5.0);
    let shown = r.judge(&rep, held_at() - 5.0);
    assert_eq!(shown.relation, PeerRelation::Measuring);
    assert_eq!(
        shown.counting,
        Some(MinuteCount {
            minutes: HOLD_MINUTES as u32 - 1,
            of: HOLD_MINUTES as u32
        })
    );
    r.run(held_at() - 5.0, held_at() + 5.0);
    let rel = r.shown(held_at() + 5.0);
    assert!(cross_rate(&rel), "{rel:?}");
}

#[test]
fn the_feeds_rate_is_the_sample_clocks_alone() {
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
fn a_minute_beyond_the_rule_drops_agreement_and_counts() {
    let mut r = Rigs::new(0.0);
    r.offset = minute_rates(|k| if k == 3 { 5.0 } else { 0.0 });
    r.run(0.0, 185.0);
    assert_eq!(r.shown(185.0), PeerRelation::Agree);
    r.run(185.0, 245.0);
    let rep = r.report(245.0);
    let shown = r.judge(&rep, 245.0);
    assert_eq!(shown.relation, PeerRelation::Measuring);
    assert_eq!(shown.counting.unwrap().minutes, 1);
}

#[test]
fn minutes_on_both_sides_or_of_both_signs_are_not_steady() {
    // Beyond and within in turn.
    let mut r = Rigs::new(0.0);
    r.offset = minute_rates(|k| if k % 2 == 0 { 3.5 } else { 0.5 });
    r.run(0.0, held_at() + 5.0);
    let (low, high) = not_steady(&r.shown(held_at() + 5.0)).unwrap();
    assert!(
        (low + 3.5).abs() < 0.1 && (high + 0.5).abs() < 0.1,
        "{low} {high}"
    );
    // Beyond the rule every minute, but the sign turns: never a rate.
    let mut r = Rigs::new(0.0);
    r.offset = minute_rates(|k| if k % 2 == 0 { 5.0 } else { -5.0 });
    r.run_checking(0.0, held_at() + 5.0, |t, rep, shown| {
        assert!(
            !matches!(shown.relation, PeerRelation::NotLocked { .. }),
            "{t} s: {shown:?} {rep:?}"
        );
    });
    assert!(not_steady(&r.shown(held_at() + 5.0)).is_some());
}

#[test]
fn a_rigs_own_line_beyond_the_rule_waits_its_ten_minutes() {
    let mut r = Rigs::new(0.0);
    r.own_ppm = 4.0;
    r.run(0.0, held_at() - 5.0);
    assert_eq!(r.shown(held_at() - 5.0), PeerRelation::Measuring);
    r.run(held_at() - 5.0, held_at() + 5.0);
    assert_eq!(
        r.shown(held_at() + 5.0),
        PeerRelation::NotLocked {
            reason: NotLockedReason::OwnClock {
                rig: RigId(2),
                ppm: 4.0
            }
        }
    );
}

#[test]
fn a_link_that_stalls_and_catches_up_loses_nothing() {
    // Arrivals held for 3 s, then the frames come at once: every mark is there.
    let mut r = Rigs::new(0.0);
    r.jitter = Box::new(|n| {
        let t = n as f64 / FPS;
        if (100.0..103.0).contains(&t) {
            103.0 - t
        } else {
            0.0
        }
    });
    r.run(0.0, 99.0);
    let before = r.report(99.0).minutes.unwrap();
    assert_eq!(r.shown(99.0), PeerRelation::Agree);
    r.run(99.0, 125.0);
    let after = r.report(125.0).minutes.unwrap();
    assert_eq!(
        (after.series, after.closed),
        (before.series, before.closed + 1)
    );
    assert_eq!(r.shown(125.0), PeerRelation::Agree);
}

#[test]
fn a_hole_in_the_marks_breaks_the_fit_and_keeps_the_minutes_closed() {
    // The other rig's system clock moves 3 ms while no mark comes for ten minutes.
    let mut r = Rigs::new(0.0);
    r.offset = Box::new(|t| if t < 200.0 { 0.0 } else { 0.003 });
    r.run(0.0, 130.0);
    let before = r.report(130.0).minutes.unwrap();
    assert_eq!(before.closed, 2);
    let rep = r.report(140.0);
    assert_eq!(rep.absent, Some(Absent::TheirFrames));
    assert!(rep.long.is_none() && rep.marks_for.is_none() && rep.marks_since.is_none());
    r.run(740.0, 745.0);
    let rep = r.report(745.0);
    // Five seconds of marks: no fit bridging the hole, the minutes closed before it kept.
    assert!(rep.long.is_none() && rep.short.is_none(), "{rep:?}");
    assert_eq!(rep.minutes.unwrap().series, before.series);
    assert_eq!(rep.minutes.unwrap().closed, 2);
    // The open minute counts from after the hole; the step is no rate.
    r.run(745.0, 801.0);
    let rep = r.report(801.0);
    assert!(ppm(&rep).abs() < 0.05, "{}", ppm(&rep));
    assert_eq!(rep.minutes.unwrap().closed, 3);
    assert_eq!(r.shown(801.0), PeerRelation::Agree);
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
    assert_eq!(rep.minutes.unwrap().closed, 0);
    assert_eq!(r.shown(70.0), PeerRelation::Measuring);

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
        r.shown(65.0),
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
        r.shown(3.0),
        PeerRelation::CantTell {
            reason: CantTellReason::ClocksDisagree { .. }
        }
    ));
    // One mark in the future among the others is a stamp's jitter, not a clock.
    let mut r = Rigs::new(0.0);
    r.offset = Box::new(|t| if (t - 2.0).abs() < 0.01 { 0.050 } else { 0.0 });
    r.run(0.0, 2.5);
    assert_eq!(r.report(2.5).ahead, None);
    // Behind by a second: late past the longest link offset once it has held.
    let mut r = Rigs::new(0.0);
    r.offset = Box::new(|_| -1.0);
    // (Its first second of marks falls before this rig's capture began.)
    r.run(0.0, 2.5);
    let age = r.report(2.5).least_age.unwrap();
    assert!((age - 1.005).abs() < 0.004, "{age}");
    assert_eq!(r.report(2.5).ahead, None);
    assert_eq!(r.shown(2.5), PeerRelation::Measuring);
    r.run(2.5, 7.0);
    assert!(matches!(
        r.shown(7.0),
        PeerRelation::CantTell {
            reason: CantTellReason::MarksLate { .. }
        }
    ));
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
    r.run_checking(0.0, 1000.0, |t, rep, shown| {
        assert!(
            matches!(
                shown.relation,
                PeerRelation::Agree | PeerRelation::Measuring
            ),
            "{t} s: {shown:?} {rep:?}"
        );
        beyond |= shown.counting.is_some();
    });
    // The fit saw it, counted, and let it go.
    assert!(beyond);
    assert_eq!(r.shown(1000.0), PeerRelation::Agree);
}

#[test]
fn an_opposing_slew_does_not_hide_a_steady_rate_past_the_hold() {
    let mut r = Rigs::new(3.0);
    r.offset = Box::new(|t| ((t - 300.0) / MINUTE).clamp(0.0, 1.0) * 0.001);
    // The slewed minute rolls out of the ten after ten more.
    let end = 360.0 + held_at() + 5.0;
    r.run(0.0, end);
    let rel = r.shown(end);
    assert!(cross_rate(&rel), "{rel:?}");
}

#[test]
fn a_steady_pair_shows_the_same_entry_second_after_second() {
    let mut r = Rigs::new(0.0);
    r.jitter = Box::new(|n| (n.wrapping_mul(2_654_435_761) % 1001) as f64 * 1e-6);
    let mut seen = Vec::new();
    r.run_checking(0.0, 180.0, |t, _, shown| {
        if t > 65.0 {
            seen.push(shown.clone());
        }
    });
    assert!(seen.iter().all(|s| *s == seen[0]), "{:?}", seen[0]);
    assert_eq!(seen[0].relation, PeerRelation::Agree);
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
    assert_eq!(rep.minutes, None);
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
