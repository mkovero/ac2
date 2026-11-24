//! Tags and tips resting on the clocks the rigs' always-on levels show
//! (`State.remote.standing`), where no stream runs between the two.

use ac2_proto::model::{DeviceClock, DeviceId, MinuteCount, RemoteLink, StandingSession};
use ac2_proto::units::Seconds;

use super::*;

const PUPU: RigId = RigId(1);
const KETUNKOLO: RigId = RigId(2);
const NOW: u64 = 1_790_000_000_000_000_000;
const LEVELS: &str = "measured from the rigs' level frames, no audio streamed";

/// pupu's own clocks: a sample clock measured against a synced system clock, so a pair
/// judged on system clocks alone would agree with a rig at the same figure.
fn pupu_clocks() -> Clocks {
    Clocks {
        system: SystemClock::Synced { maxerror_us: 3027 },
        capture: Some(DeviceClock {
            device: DeviceId("system".into()),
            rate: SampleClock::Measured {
                ppm: 0.2,
                span: Seconds(59.9),
                against: ClockBasis::SystemClock,
            },
        }),
        stamp_map: None,
    }
}

/// pupu's daemon paired with ketunkolo, its levels relation `relation` at `ppm`, marks for
/// 5 min 12 s when a figure is given.
fn standing(relation: PeerRelation, ppm: Option<f64>) -> RemoteState {
    RemoteState {
        standing: vec![StandingRig {
            rig: KETUNKOLO,
            name: "ketunkolo".into(),
            link: RemoteLink::Up,
            session: Some(StandingSession {
                sample_rate_hz: 96_000,
                channels: vec![0, 1],
            }),
            clock: PeerClock {
                relation,
                ppm,
                span: ppm.map(|_| Seconds(59.96)),
                last_change: None,
            },
            marks_since: ppm.map(|_| WallNs(NOW - 312_400_000_000)),
            counting: None,
        }],
        ..RemoteState::default()
    }
}

/// pupu shown, reading `remote`; ketunkolo not read by the app, at the rate pupu's daemon
/// hears in its levels (96 kHz), with no clocks of its own.
fn judge(remote: &RemoteState, pupu_sees: &[RemoteRig]) -> ClockDomains {
    let (pupu, unread) = (pupu_clocks(), Clocks::default());
    clock_domains(&[
        RigClockFacts {
            id: Some(PUPU),
            name: "pupu",
            rate_hz: Some(96_000),
            clocks: &pupu,
            between: pupu_sees,
            levels: Some(Levels {
                remote,
                now: WallNs(NOW),
            }),
        },
        RigClockFacts {
            id: Some(KETUNKOLO),
            name: "ketunkolo",
            rate_hz: Some(96_000),
            clocks: &unread,
            between: &[],
            levels: None,
        },
    ])
}

fn tags(d: &ClockDomains) -> Vec<Option<&str>> {
    d.rigs.iter().map(|r| r.tag.as_deref()).collect()
}

fn tip(d: &ClockDomains, i: usize) -> &str {
    d.rigs[i].tip.as_deref().expect("a tooltip")
}

fn cant_tell(reason: CantTellReason) -> PeerRelation {
    PeerRelation::CantTell { reason }
}

/// The stream pupu's session takes from ketunkolo, with its daemon's verdict.
fn stream(relation: PeerRelation, ppm: Option<f64>) -> Vec<RemoteRig> {
    vec![RemoteRig {
        rig: KETUNKOLO,
        name: "ketunkolo".into(),
        link: RemoteLink::Up,
        clock: PeerClock {
            relation,
            ppm,
            span: ppm.map(|_| Seconds(60.0)),
            last_change: None,
        },
        bytes_per_s: 0.0,
        window: 0,
    }]
}

#[test]
fn levels_that_agree_tag_both_rigs_and_say_what_backs_it() {
    let d = judge(&standing(PeerRelation::Agree, Some(0.12)), &[]);
    assert_eq!(tags(&d), [Some("AGREE"), Some("AGREE")]);
    assert_eq!(
        tip(&d, 1),
        format!(
            "clocks agree with pupu: {LEVELS}, within 2 ppm (+0.12 ppm over 60 s · marks for \
             5 min 12 s); a lock is not proven below 2 ppm"
        )
    );
}

#[test]
fn levels_still_measuring_cannot_tell_and_say_the_count() {
    let d = judge(&standing(PeerRelation::Measuring, None), &[]);
    assert_eq!(tags(&d), [Some("UNKNOWN"), Some("UNKNOWN")]);
    assert_eq!(
        tip(&d, 1),
        format!("can't tell: measuring clocks… · {LEVELS}")
    );

    let mut r = standing(PeerRelation::Measuring, Some(2.71));
    r.standing[0].counting = Some(MinuteCount {
        beyond: 4,
        counted: 4,
        needed: 10,
    });
    let d = judge(&r, &[]);
    assert_eq!(tags(&d), [Some("UNKNOWN"), Some("UNKNOWN")]);
    assert_eq!(
        tip(&d, 1),
        format!(
            "can't tell: measuring clocks… · +2.71 ppm over 60 s · beyond 2 ppm for 4 of the \
             10 min a verdict needs · marks for 5 min 12 s · {LEVELS}"
        )
    );
}

#[test]
fn levels_not_locked_free_both_rigs_each_seen_from_itself() {
    let d = judge(
        &standing(
            PeerRelation::NotLocked {
                reason: NotLockedReason::CrossRate {
                    ppm: 3.12,
                    over: Seconds(60.0),
                },
            },
            Some(3.12),
        ),
        &[],
    );
    assert_eq!(tags(&d), [Some("FREE"), Some("FREE")]);
    assert_eq!(
        tip(&d, 1),
        format!("ketunkolo runs +3.1 ppm against pupu over 60 s · marks for 5 min 12 s · {LEVELS}")
    );
    assert_eq!(
        tip(&d, 0),
        format!("pupu runs −3.1 ppm against ketunkolo over 60 s · marks for 5 min 12 s · {LEVELS}")
    );

    let d = judge(
        &standing(
            PeerRelation::NotLocked {
                reason: NotLockedReason::OwnClock {
                    rig: KETUNKOLO,
                    ppm: 41.0,
                },
            },
            None,
        ),
        &[],
    );
    assert_eq!(tags(&d), [Some("FREE"), Some("FREE")]);
    assert_eq!(
        tip(&d, 1),
        format!("ketunkolo's interface runs +41.0 ppm off its system clock · {LEVELS}")
    );
}

/// A rig at another rate differs on the rates alone, whatever the levels say.
#[test]
fn levels_at_another_rate_read_as_the_rates() {
    let r = standing(
        PeerRelation::NotLocked {
            reason: NotLockedReason::OtherRate {
                theirs: 48_000,
                ours: 96_000,
            },
        },
        None,
    );
    let (pupu, unread) = (pupu_clocks(), Clocks::default());
    let d = clock_domains(&[
        RigClockFacts {
            id: Some(PUPU),
            name: "pupu",
            rate_hz: Some(96_000),
            clocks: &pupu,
            between: &[],
            levels: Some(Levels {
                remote: &r,
                now: WallNs(NOW),
            }),
        },
        RigClockFacts {
            id: Some(KETUNKOLO),
            name: "ketunkolo",
            rate_hz: Some(48_000),
            clocks: &unread,
            between: &[],
            levels: None,
        },
    ]);
    assert_eq!(tags(&d), [Some("FREE"), Some("FREE")]);
    assert_eq!(tip(&d, 1), "48 kHz here; pupu at 96 kHz");
}

#[test]
fn levels_that_cannot_tell_say_why() {
    let why = |reason| {
        let d = judge(&standing(cant_tell(reason), None), &[]);
        assert_eq!(tags(&d), [Some("UNKNOWN"), Some("UNKNOWN")]);
        tip(&d, 1).to_owned()
    };
    assert_eq!(
        why(CantTellReason::NoMarks),
        format!("can't tell: no timing in ketunkolo's levels · {LEVELS}")
    );
    assert_eq!(
        why(CantTellReason::MarksLate { by: Seconds(0.25) }),
        format!(
            "can't tell: ketunkolo's levels arrive 250 ms late: the network, or its system \
             clock behind · {LEVELS}"
        )
    );
    assert_eq!(
        why(CantTellReason::NotSteady {
            low: 0.42,
            high: 3.13,
            line: None
        }),
        format!("can't tell: figure not steady: +0.4 … +3.1 ppm · {LEVELS}")
    );
    assert_eq!(
        why(CantTellReason::NoSession { rig: PUPU }),
        format!("can't tell: pupu has no audio session · {LEVELS}")
    );
    assert_eq!(
        why(CantTellReason::ClocksDisagree {
            by: Seconds(0.0123)
        }),
        format!(
            "can't tell: the system clocks of pupu and ketunkolo disagree by 12.3 ms · {LEVELS}"
        )
    );
    assert_eq!(
        why(CantTellReason::LinkDown),
        format!("can't tell: no link to ketunkolo · {LEVELS}")
    );
}

/// The stream's verdict is the tag; the levels' line joins its tip only where it would tag
/// the rig otherwise.
#[test]
fn a_streams_verdict_stays_the_tag_and_levels_that_differ_add_a_line() {
    let agree = stream(PeerRelation::Agree, Some(0.1));
    let d = judge(&standing(PeerRelation::Agree, Some(0.12)), &agree);
    assert_eq!(tags(&d), [Some("AGREE"), Some("AGREE")]);
    assert_eq!(
        tip(&d, 1),
        "clocks agree with pupu: measured between the rigs, within 2 ppm (+0.1 ppm over the \
         last 60 s); a lock is not proven below 2 ppm"
    );

    let d = judge(&standing(PeerRelation::Measuring, Some(0.4)), &agree);
    assert_eq!(tags(&d), [Some("AGREE"), Some("AGREE")]);
    assert_eq!(
        tip(&d, 1),
        format!(
            "clocks agree with pupu: measured between the rigs, within 2 ppm (+0.1 ppm over \
             the last 60 s); a lock is not proven below 2 ppm\nmeasuring clocks… · ketunkolo \
             runs +0.40 ppm against this rig over 60 s · marks for 5 min 12 s · {LEVELS}"
        )
    );

    let not_locked = PeerRelation::NotLocked {
        reason: NotLockedReason::CrossRate {
            ppm: 3.12,
            over: Seconds(60.0),
        },
    };
    let d = judge(
        &standing(not_locked, Some(3.12)),
        &stream(cant_tell(CantTellReason::NoAudio), None),
    );
    assert_eq!(tags(&d), [Some("UNKNOWN"), Some("UNKNOWN")]);
    assert_eq!(
        tip(&d, 1),
        format!(
            "can't tell: no audio between pupu and ketunkolo\nCLOCKS NOT LOCKED: ketunkolo runs \
             +3.1 ppm against this rig over 60 s · marks for 5 min 12 s · {LEVELS}"
        )
    );
}

/// A rig the daemon lists no standing relation with is judged as before: here on the
/// system clocks, which ketunkolo's unread state does not give.
#[test]
fn no_standing_entry_leaves_the_evidence_as_it_was() {
    let none = RemoteState::default();
    let d = judge(&none, &[]);
    assert_eq!(tags(&d), [Some("UNKNOWN"), Some("UNKNOWN")]);
    assert_eq!(tip(&d, 1), "can't tell: ketunkolo reports no sample clock");
}
