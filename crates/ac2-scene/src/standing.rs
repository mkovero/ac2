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
    let rel = relation_text(r, &s.name, &s.clock.relation);
    let mut parts = vec![format!("from levels {}", s.name), rel];
    if let (Some(p), Some(span)) = (s.clock.ppm, s.clock.span) {
        parts.push(format!(
            "{} ppm over {} s",
            format::signed(p, 2),
            format::fixed(span.0, 0)
        ));
    }
    if let Some(c) = s.counting {
        parts.push(format!("{} of {} min beyond 2 ppm", c.minutes, c.of));
    }
    if let Some(since) = s.marks_since {
        let s = now.0.saturating_sub(since.0) / 1_000_000_000;
        parts.push(format!("marks for {}", format::duration(s as f64)));
    }
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

/// The link in short: its state without the audio rate, which a levels feed does not have.
fn link_words(s: &StandingRig) -> String {
    match &s.link {
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
            "from levels ketunkolo · clocks agree · +0.12 ppm over 60 s · marks for 5 min 12 s · \
             96 kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(up.clone(), PeerRelation::Measuring, None)),
            "from levels ketunkolo · measuring clocks… · 96 kHz, 3 inputs · link up"
        );
        let mut counting = rig(up.clone(), PeerRelation::Measuring, Some(2.71));
        counting.standing[0].counting = Some(MinuteCount { minutes: 4, of: 10 });
        assert_eq!(
            one(&counting),
            "from levels ketunkolo · measuring clocks… · +2.71 ppm over 60 s · 4 of 10 min \
             beyond 2 ppm · marks for 5 min 12 s · 96 kHz, 3 inputs · link up"
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
            "from levels ketunkolo · CLOCKS NOT LOCKED +3.1 ppm over 60 s · +3.12 ppm over \
             60 s · marks for 5 min 12 s · 96 kHz, 3 inputs · link up"
        );
        assert_eq!(
            one(&rig(
                up.clone(),
                PeerRelation::CantTell {
                    reason: CantTellReason::NotSteady {
                        low: 0.42,
                        high: 3.13
                    }
                },
                Some(1.85)
            )),
            "from levels ketunkolo · CLOCKS: CAN'T TELL (figure not steady: +0.4 … +3.1 ppm) · \
             +1.85 ppm over 60 s · marks for 5 min 12 s · 96 kHz, 3 inputs · link up"
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
