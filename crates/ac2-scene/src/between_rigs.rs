//! The words for the clocks and the link between this rig and the rigs its remote inputs
//! come from (`docs/design/remote-inputs.md` §5): one line per remote rig and one per
//! remote input, as `ac2 status` prints them under *between rigs*.

use ac2_proto::model::{
    Anchor, ArrivalReason, CantTellReason, NotLockedReason, PeerRelation, RemoteAudio, RemoteInput,
    RemoteLink, RemoteRig, RemoteState,
};
use ac2_proto::units::RigId;

use crate::format;

/// A rig's name among the remote rigs, else its id.
fn name_of(r: &RemoteState, rig: RigId) -> String {
    r.rigs
        .iter()
        .find(|x| x.rig == rig)
        .map_or_else(|| rig.to_string(), |x| x.name.clone())
}

/// The relation in words: *clocks agree*, *CLOCKS NOT LOCKED +5.6 ppm*, …
pub fn relation_text(r: &RemoteState, rel: &PeerRelation) -> String {
    let ppm = |p: f64| format!("{} ppm", format::signed(p, 1));
    match rel {
        PeerRelation::Agree => "clocks agree".into(),
        PeerRelation::Measuring => "measuring clocks…".into(),
        PeerRelation::NotLocked { reason } => match reason {
            NotLockedReason::CrossRate { ppm: p, over } => format!(
                "CLOCKS NOT LOCKED {} over {} s",
                ppm(*p),
                format::fixed(over.0, 0)
            ),
            NotLockedReason::OwnClock { rig, ppm: p } => format!(
                "CLOCKS NOT LOCKED: {}'s interface runs {} off its system clock",
                name_of(r, *rig),
                ppm(*p)
            ),
        },
        PeerRelation::CantTell { reason } => {
            let why = match reason {
                CantTellReason::NoAudio => "no audio".to_owned(),
                CantTellReason::SystemClockUnsynced { rig } => {
                    format!("{}'s system clock is not synced", name_of(r, *rig))
                }
                CantTellReason::SystemClockNotStated { rig } => {
                    format!("{}'s system clock does not say", name_of(r, *rig))
                }
                CantTellReason::NotAgainstSystemClock { rig } => format!(
                    "{}'s interface is measured against its own crystal",
                    name_of(r, *rig)
                ),
                CantTellReason::NoCaptureStamps => "no capture times".to_owned(),
                CantTellReason::ClocksDisagree { by } => format!(
                    "the system clocks disagree by {} ms",
                    format::fixed(by.0 * 1e3, 1)
                ),
            };
            format!("CLOCKS: CAN'T TELL ({why})")
        }
    }
}

fn link_text(r: &RemoteRig) -> String {
    match &r.link {
        RemoteLink::NotPaired => "not paired: pair the rigs from the app".into(),
        RemoteLink::NotNetworked => "this rig listens on this computer only".into(),
        RemoteLink::Connecting => "connecting".into(),
        RemoteLink::Up => format!("up, {} Mbit/s", format::fixed(r.bytes_per_s * 8.0 / 1e6, 1)),
        RemoteLink::Unreachable { .. } => "unreachable".into(),
        RemoteLink::NotAdmitted => format!("{} has not admitted this rig", r.name),
        RemoteLink::KeyChanged => {
            format!("{}'s key changed: pair the rigs again from the app", r.name)
        }
        RemoteLink::OtherRig { found } => format!("another rig answers there ({found})"),
    }
}

fn audio_text(a: RemoteAudio, rig: &str) -> String {
    match a {
        RemoteAudio::Waiting => format!("waiting for {rig}"),
        RemoteAudio::OnThisRig => "that input is on this rig".into(),
        RemoteAudio::NoSession => format!("no audio from {rig} (no session)"),
        RemoteAudio::NotCaptured => format!("{rig} does not capture it (open it there)"),
        RemoteAudio::Replaying => format!("{rig} is replaying a recording"),
        RemoteAudio::RateMismatch { theirs, ours } => {
            format!("{rig} runs {} kHz, this rig {} kHz", khz(theirs), khz(ours))
        }
        RemoteAudio::Starting => "starting".into(),
        RemoteAudio::Streaming => "streaming".into(),
    }
}

/// A rate in kHz, without a decimal where it has none (48, 44.1).
fn khz(hz: u32) -> String {
    let k = f64::from(hz) / 1e3;
    format::fixed(k, if k.fract() == 0.0 { 0 } else { 1 })
}

fn anchor_text(a: &Anchor) -> &'static str {
    match a {
        Anchor::SystemTime => "placed by capture time",
        Anchor::Arrival { reason } => match reason {
            ArrivalReason::NoCaptureStamps => "placed on arrival (no capture times)",
            ArrivalReason::SystemClock => "placed on arrival (a system clock is not synced)",
            ArrivalReason::NoStampMap => "placed on arrival (no clock map yet)",
            ArrivalReason::ClocksDisagree { .. } => {
                "placed on arrival (the system clocks disagree)"
            }
        },
    }
}

/// `between rigs` lines: one per remote rig, then one per remote input of `inputs` (the
/// session's, in order). Empty without remote inputs.
pub fn status_lines(r: &RemoteState, inputs: &[RemoteInput]) -> Vec<String> {
    let mut out = Vec::new();
    for rig in &r.rigs {
        out.push(format!(
            "between rigs {} · {} · link {}",
            rig.name,
            relation_text(r, &rig.clock.relation),
            link_text(rig)
        ));
    }
    if let Some(o) = r.link_offset {
        out.push(format!("link offset  {} ms", format::fixed(o.0 * 1e3, 0)));
    }
    for i in inputs {
        let Some(st) = r.inputs.iter().find(|s| s.slot == i.slot) else {
            continue;
        };
        let rig = name_of(r, i.rig);
        let mut line = format!(
            "remote input {} in {}: {}",
            rig,
            i.channel + 1,
            audio_text(st.audio, &rig)
        );
        if let Some(a) = &st.anchor {
            line.push_str(&format!(", {}", anchor_text(a)));
        }
        line.push_str(&format!(
            " · {} late frames in the last hour · {} slips",
            st.late_frames_hour, st.slips
        ));
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{CalStatus, PeerClock, RemoteInputStatus};
    use ac2_proto::units::Seconds;

    fn state(relation: PeerRelation, audio: RemoteAudio) -> (RemoteState, Vec<RemoteInput>) {
        let rig = RigId(2);
        let r = RemoteState {
            peers: Vec::new(),
            rigs: vec![RemoteRig {
                rig,
                name: "ketunkolo".into(),
                link: RemoteLink::Up,
                clock: PeerClock {
                    relation,
                    ppm: None,
                    span: None,
                    last_change: None,
                },
                bytes_per_s: 390_000.0,
                window: 262_144,
            }],
            inputs: vec![RemoteInputStatus {
                slot: 0,
                audio,
                anchor: Some(Anchor::SystemTime),
                late_frames: 0,
                late_frames_hour: 0,
                slips: 0,
                last_slip: None,
                arrival_margin: None,
                setup: None,
                cal_status: CalStatus::Uncalibrated,
                calibration: None,
                curve: None,
            }],
            link_offset: Some(Seconds(0.01)),
        };
        let inputs = vec![RemoteInput {
            slot: 0,
            rig,
            rig_name: "ketunkolo".into(),
            channel: 0,
        }];
        (r, inputs)
    }

    #[test]
    fn locked_rigs_read_as_agreeing_with_the_link_and_the_input() {
        let (r, i) = state(PeerRelation::Agree, RemoteAudio::Streaming);
        assert_eq!(
            status_lines(&r, &i),
            vec![
                "between rigs ketunkolo · clocks agree · link up, 3.1 Mbit/s",
                "link offset  10 ms",
                "remote input ketunkolo in 1: streaming, placed by capture time · 0 late \
                 frames in the last hour · 0 slips",
            ]
        );
    }

    #[test]
    fn problems_are_said_in_words() {
        let (r, i) = state(
            PeerRelation::NotLocked {
                reason: NotLockedReason::CrossRate {
                    ppm: 5.6,
                    over: Seconds(40.0),
                },
            },
            RemoteAudio::RateMismatch {
                theirs: 48_000,
                ours: 96_000,
            },
        );
        let l = status_lines(&r, &i);
        assert!(
            l[0].contains("CLOCKS NOT LOCKED +5.6 ppm over 40 s"),
            "{l:?}"
        );
        assert!(
            l[2].contains("ketunkolo runs 48 kHz, this rig 96 kHz"),
            "{l:?}"
        );
        let (r, _) = state(
            PeerRelation::CantTell {
                reason: CantTellReason::SystemClockUnsynced { rig: RigId(2) },
            },
            RemoteAudio::Streaming,
        );
        assert_eq!(
            relation_text(&r, &r.rigs[0].clock.relation),
            "CLOCKS: CAN'T TELL (ketunkolo's system clock is not synced)"
        );
    }
}
