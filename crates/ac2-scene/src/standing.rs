//! The clocks between this rig and each paired rig as its always-on levels show them
//! (`State.remote.standing`), in the words the lines between rigs use; never more than the
//! evidence: rates within the 2 ppm rule read *clocks agree*, never *locked*.

use ac2_proto::model::{CantTellReason, PeerRelation, RemoteLink, RemoteState, StandingRig};

use crate::audio::rate_text;
use crate::between_rigs::relation_text;
use crate::format;

/// `from levels` lines for `ac2 status`, one per paired rig: *from levels ketunkolo · clocks
/// agree +0.12 ppm over 60 s · marks for 5 min 12 s · 96 kHz, 3 inputs · link up*.
pub fn status_lines(r: &RemoteState) -> Vec<String> {
    r.standing.iter().map(|s| line(r, s)).collect()
}

fn line(r: &RemoteState, s: &StandingRig) -> String {
    let rel = relation_text(r, &s.name, &s.clock.relation);
    let mut parts = vec![format!("from levels {}", s.name), rel];
    if let (Some(p), Some(span)) = (s.clock.ppm, s.clock.span) {
        parts.push(format!(
            "{} ppm over {} s",
            format::signed(p, 2),
            format::fixed(span.0, 0)
        ));
    }
    if let Some(m) = s.marks_for {
        parts.push(format!("marks for {}", format::duration(m.0)));
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
    use ac2_proto::model::{NotLockedReason, PeerClock, StandingSession};
    use ac2_proto::units::{RigId, Seconds, WallNs};

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
                marks_for: ppm.map(|_| Seconds(312.4)),
            }],
            ..RemoteState::default()
        }
    }

    fn one(r: &RemoteState) -> String {
        status_lines(r).remove(0)
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
