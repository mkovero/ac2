//! The clocks between this rig and each paired rig as its always-on levels show them
//! (`State.remote.standing`), in the words the lines between rigs use; never more than the
//! evidence: rates within the 2 ppm rule read *clocks agree*, never *locked*.

use ac2_proto::model::{CantTellReason, PeerRelation, RemoteLink, RemoteState, StandingRig};
use ac2_proto::units::WallNs;

use crate::audio::rate_text;
use crate::between_rigs::relation_text;
use crate::format;

/// `from levels` lines for `ac2 status` at wall time `now`, one per paired rig: *from levels
/// ketunkolo · clocks agree · +0.12 ppm over 60 s · marks for 5 min 12 s · 96 kHz, 3 inputs ·
/// link up*.
pub fn status_lines(r: &RemoteState, now: WallNs) -> Vec<String> {
    r.standing.iter().map(|s| line(r, s, now)).collect()
}

fn line(r: &RemoteState, s: &StandingRig, now: WallNs) -> String {
    let mut parts = vec![format!("from levels {}", s.name), evidence(r, s, Some(now))];
    if s.link == RemoteLink::Up {
        parts.push(match &s.session {
            Some(x) => format!(
                "{}, {} input{}",
                rate_text(x.sample_rate_hz),
                x.channels.len(),
                if x.channels.len() == 1 { "" } else { "s" }
            ),
            None => "no session".into(),
        });
    }
    // The relation already says the link is down; the link's own state says why.
    let down = matches!(
        s.clock.relation,
        PeerRelation::CantTell {
            reason: CantTellReason::LinkDown
        }
    );
    parts.push(if down {
        link_words(s)
    } else {
        format!("link {}", link_words(s))
    });
    parts.join(" · ")
}

/// What a standing relation rests on, where a reader could take it for the audio-fed one.
pub const FROM_LEVELS: &str = "measured from the rigs' level frames, no audio streamed";

/// What rig `s`'s levels say of the clocks, in `status_lines`' words without its session
/// or its link, the figure naming whose clock runs against whose: *clocks agree · ketunkolo
/// runs +0.12 ppm against this rig over 60 s · marks for 5 min 12 s*. `now`: the reporting
/// daemon's wall time; `None` while it is not known, which leaves the marks' age out.
pub fn evidence(r: &RemoteState, s: &StandingRig, now: Option<WallNs>) -> String {
    evidence_at(r, s, now, "this rig")
}

/// [`evidence`] where the reporting rig has a name of its own beside others (the app's
/// tree): the daemon's words call that rig *this rig*, the tree names it `here`.
pub fn evidence_at(r: &RemoteState, s: &StandingRig, now: Option<WallNs>, here: &str) -> String {
    let rel = relation_text(r, &s.name, &s.clock.relation);
    // Every *this rig* in a relation's words is the reporting daemon's own rig.
    let mut parts = vec![rel.replace("this rig", here)];
    parts.extend(figure(s, &s.name, here));
    parts.extend(count(s));
    parts.extend(now.and_then(|n| marks(s, n)));
    parts.join(" · ")
}

/// [`evidence`] with what it is measured from: the line a hover adds beside the pairing.
pub fn evidence_line(r: &RemoteState, s: &StandingRig, now: Option<WallNs>) -> String {
    format!("{} · {FROM_LEVELS}", evidence(r, s, now))
}

/// [`evidence_at`] with what it is measured from: the line a tree tip adds beside a
/// stream's verdict or the other daemon's.
pub fn evidence_line_at(
    r: &RemoteState,
    s: &StandingRig,
    now: Option<WallNs>,
    here: &str,
) -> String {
    format!("{} · {FROM_LEVELS}", evidence_at(r, s, now, here))
}

/// Rig `who`'s sample clock against rig `against`'s: *ketunkolo runs +0.12 ppm against
/// pupu over 60 s*. Levels resolve a fit to hundredths; the wire figure is the paired rig's
/// clock against the reporting one, positive when fast.
pub fn figure_text(who: &str, against: &str, ppm: f64, span_s: f64) -> String {
    format!(
        "{who} runs {} ppm against {against} over {} s",
        format::signed(ppm, 2),
        format::fixed(span_s, 0)
    )
}

/// The verdict's figure as [`figure_text`] says it, the paired rig `who` against the
/// reporting rig `against`; none for a not-locked verdict, which states its own (a second
/// figure beside it would be another window's and could disagree).
pub fn figure(s: &StandingRig, who: &str, against: &str) -> Option<String> {
    if matches!(s.clock.relation, PeerRelation::NotLocked { .. }) {
        return None;
    }
    let (p, span) = s.clock.ppm.zip(s.clock.span)?;
    Some(figure_text(who, against, p, span.0))
}

/// The count toward a not-locked verdict, while one runs, with both ways out of it: the
/// verdict at the minutes it needs, or agreement after two minutes in a row within the rule.
pub fn count(s: &StandingRig) -> Option<String> {
    let c = s.counting?;
    Some(if c.beyond == c.counted {
        format!(
            "beyond 2 ppm for {} min running · not locked at {}, agree after 2 min running \
             within",
            c.beyond, c.needed
        )
    } else {
        format!(
            "beyond 2 ppm in {} of {} min · a verdict at {}, agree after 2 min running within",
            c.beyond, c.counted, c.needed
        )
    })
}

/// How long marks have come, at the reporting daemon's wall time `now`.
pub fn marks(s: &StandingRig, now: WallNs) -> Option<String> {
    let since = s.marks_since?;
    let secs = now.0.saturating_sub(since.0) / 1_000_000_000;
    Some(format!("marks for {}", whole_seconds(secs)))
}

/// A time in whole seconds: *12 s*, *5 min 12 s*, *2 h 05 min*.
fn whole_seconds(s: u64) -> String {
    if s < 60 {
        format!("{s} s")
    } else {
        format::duration(s as f64)
    }
}

/// The link in short: its state without the audio rate, which a levels feed does not have.
fn link_words(s: &StandingRig) -> String {
    link_short(&s.link)
}

/// A link's state in a word or two, for a line with no room for a sentence: *unreachable*,
/// *not admitted*.
pub fn link_short(link: &RemoteLink) -> String {
    match link {
        RemoteLink::Up => "up".into(),
        RemoteLink::NotPaired => "not paired".into(),
        RemoteLink::NoServerKey => "no server key".into(),
        RemoteLink::Connecting => "connecting".into(),
        RemoteLink::NetworkOff => "its network is off".into(),
        RemoteLink::Unreachable { .. } => "unreachable".into(),
        RemoteLink::NotAdmitted => "not admitted".into(),
        RemoteLink::KeyChanged => "key changed".into(),
        RemoteLink::OtherRig { found } => format!("another rig answers ({found})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{MinuteCount, NotLockedReason, PeerClock, StandingSession};
    use ac2_proto::units::{RigId, Seconds};

    fn rig(link: RemoteLink, relation: PeerRelation, ppm: Option<f64>) -> RemoteState {
        RemoteState {
            standing: vec![StandingRig {
                rig: RigId(2),
                name: "ketunkolo".into(),
                link: link.clone(),
                session: (link == RemoteLink::Up).then(|| StandingSession {
                    sample_rate_hz: 96_000,
                    channels: vec![0, 1, 4],
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

    const NOW: u64 = 1_790_000_000_000_000_000;

    fn one(r: &RemoteState) -> String {
        status_lines(r, WallNs(NOW)).remove(0)
    }

    #[test]
    fn each_verdict_reads_in_the_words_between_rigs() {
        let up = RemoteLink::Up;
        assert_eq!(
            one(&rig(up.clone(), PeerRelation::Agree, Some(0.12))),
            "from levels ketunkolo · clocks agree · ketunkolo runs +0.12 ppm against this rig \
             over 60 s · marks for 5 min 12 s · 96 kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(up.clone(), PeerRelation::Measuring, None)),
            "from levels ketunkolo · measuring clocks… · 96 kHz, 3 inputs · link up"
        );
        let mut counting = rig(up.clone(), PeerRelation::Measuring, Some(2.71));
        counting.standing[0].counting = Some(MinuteCount {
            beyond: 4,
            counted: 4,
            needed: 10,
        });
        assert_eq!(
            one(&counting),
            "from levels ketunkolo · measuring clocks… · ketunkolo runs +2.71 ppm against this \
             rig over 60 s · beyond 2 ppm for 4 min running · not locked at 10, agree after 2 \
             min running within · marks for 5 min 12 s · 96 kHz, 3 inputs · link up"
        );
        counting.standing[0].counting = Some(MinuteCount {
            beyond: 2,
            counted: 3,
            needed: 10,
        });
        counting.standing[0].marks_since = Some(WallNs(NOW - 12_400_000_000));
        assert_eq!(
            one(&counting),
            "from levels ketunkolo · measuring clocks… · ketunkolo runs +2.71 ppm against this \
             rig over 60 s · beyond 2 ppm in 2 of 3 min · a verdict at 10, agree after 2 min \
             running within · marks for 12 s · 96 kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(
                up.clone(),
                PeerRelation::NotLocked {
                    reason: NotLockedReason::CrossRate {
                        ppm: 3.12,
                        over: Seconds(60.0)
                    }
                },
                Some(3.12)
            )),
            "from levels ketunkolo · CLOCKS NOT LOCKED: ketunkolo runs +3.1 ppm against this \
             rig over 60 s · marks for 5 min 12 s · 96 kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(
                up.clone(),
                PeerRelation::CantTell {
                    reason: CantTellReason::NotSteady {
                        low: 0.42,
                        high: 3.13,
                        line: None
                    }
                },
                Some(1.85)
            )),
            "from levels ketunkolo · CLOCKS: CAN'T TELL (figure not steady: +0.4 … +3.1 ppm) · \
             ketunkolo runs +1.85 ppm against this rig over 60 s · marks for 5 min 12 s · 96 \
             kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(
                up.clone(),
                PeerRelation::CantTell {
                    reason: CantTellReason::NotSteady {
                        low: 0.42,
                        high: 4.0,
                        line: Some(RigId(2))
                    }
                },
                Some(0.21)
            )),
            "from levels ketunkolo · CLOCKS: CAN'T TELL (ketunkolo's interface against its \
             system clock not steady: +0.4 … +4.0 ppm) · ketunkolo runs +0.21 ppm against this \
             rig over 60 s · marks for 5 min 12 s · 96 kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(
                up.clone(),
                PeerRelation::CantTell {
                    reason: CantTellReason::MarksKeepBreaking
                },
                None
            )),
            "from levels ketunkolo · CLOCKS: CAN'T TELL (ketunkolo's timing keeps breaking off \
             before a minute is in) · 96 kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(
                up.clone(),
                PeerRelation::NotLocked {
                    reason: NotLockedReason::OtherRate {
                        theirs: 48_000,
                        ours: 96_000
                    }
                },
                None
            )),
            "from levels ketunkolo · CLOCKS NOT LOCKED: ketunkolo runs 48 kHz, this rig \
             96 kHz · 96 kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(
                up.clone(),
                PeerRelation::CantTell {
                    reason: CantTellReason::NoMarks
                },
                None
            )),
            "from levels ketunkolo · CLOCKS: CAN'T TELL (no timing in ketunkolo's levels) · \
             96 kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(
                up.clone(),
                PeerRelation::CantTell {
                    reason: CantTellReason::MarksLate { by: Seconds(0.25) }
                },
                None
            )),
            "from levels ketunkolo · CLOCKS: CAN'T TELL (ketunkolo's levels arrive 250 ms \
             late: the network, or its system clock behind) · 96 kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(
                up,
                PeerRelation::CantTell {
                    reason: CantTellReason::NoSession { rig: RigId(1) }
                },
                None
            )),
            "from levels ketunkolo · CLOCKS: CAN'T TELL (this rig has no audio session) · \
             96 kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(
                RemoteLink::Unreachable { since: WallNs(1) },
                PeerRelation::CantTell {
                    reason: CantTellReason::LinkDown
                },
                None
            )),
            "from levels ketunkolo · CLOCKS: CAN'T TELL (no link to ketunkolo) · unreachable"
        );
    }
}
