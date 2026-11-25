//! The words for the clocks and the link between this rig and the rigs its remote inputs
//! come from (`docs/design/remote-inputs.md` §5): one line per remote rig and one per
//! remote input, as `ac2 status` prints them under *between rigs*, and the same words on the
//! app's Connection page (*Between rigs*) and Audio I/O rows.

use ac2_proto::model::{
    Anchor, ArrivalReason, CantTellReason, NotLockedReason, PeerClock, PeerRelation, RemoteAudio,
    RemoteInput, RemoteInputStatus, RemoteLink, RemoteRig, RemoteState,
};
use ac2_proto::units::{RigId, WallNs};

use crate::audio::rate_text;
use crate::format;

/// A rig's name among the remote rigs, else its id.
pub fn name_of(r: &RemoteState, rig: RigId) -> String {
    r.rigs
        .iter()
        .find(|x| x.rig == rig)
        .map_or_else(|| rig.to_string(), |x| x.name.clone())
}

/// The name of remote input `i`'s rig: as it answers, else as the stream keeps it (it is
/// named while unreachable too).
pub fn rig_name(r: &RemoteState, i: &RemoteInput) -> String {
    r.rigs
        .iter()
        .find(|x| x.rig == i.rig)
        .map_or_else(|| i.rig_name.clone(), |x| x.name.clone())
}

/// A relation with no verdict yet.
pub const MEASURING: &str = "measuring clocks…";

/// The relation with rig `other` in words: *clocks agree*, *CLOCKS NOT LOCKED +5.6 ppm*, …
pub fn relation_text(r: &RemoteState, other: &str, rel: &PeerRelation) -> String {
    let ppm = |p: f64| format!("{} ppm", format::signed(p, 1));
    match rel {
        PeerRelation::Agree => "clocks agree".into(),
        PeerRelation::Measuring => MEASURING.into(),
        PeerRelation::NotLocked { reason } => match reason {
            NotLockedReason::CrossRate { ppm: p, over } => format!(
                "CLOCKS NOT LOCKED: {other} runs {} against this rig over {} s",
                ppm(*p),
                format::fixed(over.0, 0)
            ),
            NotLockedReason::OwnClock { rig, ppm: p } => format!(
                "CLOCKS NOT LOCKED: {}'s interface runs {} off its system clock",
                name_of(r, *rig),
                ppm(*p)
            ),
            NotLockedReason::OtherRate {
                theirs_hz: theirs,
                ours_hz: ours,
            } => {
                format!("CLOCKS NOT LOCKED: {}", other_rate(other, *theirs, *ours))
            }
        },
        PeerRelation::CantTell { reason } => {
            format!("CLOCKS: CAN'T TELL ({})", cant_tell_why(r, other, reason))
        }
    }
}

/// Rig `other` and this one at two nominal rates: *ketunkolo runs 48 kHz, this rig 96 kHz*,
/// as a refused remote input says it.
pub fn other_rate(other: &str, theirs: u32, ours: u32) -> String {
    format!(
        "{other} runs {}, this rig {}",
        rate_text(theirs),
        rate_text(ours)
    )
}

/// A rig the remote state names (a remote or a paired rig), else this one: the state holds
/// only the others.
fn who(r: &RemoteState, rig: RigId) -> String {
    r.rigs
        .iter()
        .map(|x| (x.rig, &x.name))
        .chain(r.standing.iter().map(|x| (x.rig, &x.name)))
        .chain(r.peers.iter().map(|x| (x.rig, &x.name)))
        .find(|(id, _)| *id == rig)
        .map_or_else(|| "this rig".to_owned(), |(_, n)| n.clone())
}

/// Why the relation with rig `other` cannot be told: *ketunkolo's system clock is not
/// synced*, …
pub fn cant_tell_why(r: &RemoteState, other: &str, reason: &CantTellReason) -> String {
    match reason {
        CantTellReason::NoAudio => "no audio".to_owned(),
        CantTellReason::NoMarks => no_marks(other),
        CantTellReason::NoStream { rig } => no_stream(&who(r, *rig)),
        CantTellReason::LinkDown => link_down(other),
        CantTellReason::MarksLate { by } => marks_late(other, by.0),
        CantTellReason::NotSteady {
            low,
            high,
            line: None,
        } => not_steady(*low, *high),
        CantTellReason::NotSteady {
            low,
            high,
            line: Some(rig),
        } => line_not_steady(&who(r, *rig), *low, *high),
        CantTellReason::MarksKeepBreaking => marks_keep_breaking(other),
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
    }
}

/// The levels-fed figure over the minutes a verdict needs, on both sides of the rule or
/// of both signs: *figure not steady: +0.4 … +3.1 ppm*.
pub fn not_steady(low: f64, high: f64) -> String {
    format!(
        "figure not steady: {} … {} ppm",
        format::signed(low, 1),
        format::signed(high, 1)
    )
}

/// Rig `who`'s own line came and went beyond the rule over the minutes a verdict needs:
/// *ketunkolo's interface against its system clock not steady: +0.4 … +4.0 ppm*.
pub fn line_not_steady(who: &str, low: f64, high: f64) -> String {
    format!(
        "{who}'s interface against its system clock not steady: {} … {} ppm",
        format::signed(low, 1),
        format::signed(high, 1)
    )
}

/// Holes in rig `other`'s timing keep every minute from closing: *ketunkolo's timing keeps
/// breaking off before a minute is in*.
pub fn marks_keep_breaking(other: &str) -> String {
    format!("{other}'s timing keeps breaking off before a minute is in")
}

/// Rig `other`'s levels carry no timing: *no timing in ketunkolo's levels*.
pub fn no_marks(other: &str) -> String {
    format!("no timing in {other}'s levels")
}

/// *ketunkolo has no audio stream*, *this rig has no audio stream*.
pub fn no_stream(who: &str) -> String {
    format!("{who} has no audio stream")
}

/// *no link to ketunkolo*.
pub fn link_down(other: &str) -> String {
    format!("no link to {other}")
}

/// Marks of rig `other` later than the link offset allows by `by` s; the network or its
/// clock, which the age alone cannot tell apart.
pub fn marks_late(other: &str, by: f64) -> String {
    format!(
        "{other}'s levels arrive {} ms late: the network, or its system clock behind",
        format::fixed(by * 1e3, 0)
    )
}

/// The relation with the evidence behind it, for the Connection page: an agreement says
/// over how long and what it does not prove (stamps resolve about 2 ppm a minute, which two
/// free crystals can meet), a measurement in progress says why it has no answer yet.
pub fn relation_with_evidence(r: &RemoteState, other: &str, c: &PeerClock) -> String {
    let rel = relation_text(r, other, &c.relation);
    match (&c.relation, c.ppm, c.span) {
        (PeerRelation::Agree, Some(p), Some(span)) => format!(
            "{rel}: {} ppm over the last {} s, both system clocks synced; a lock is not proven \
             below 2 ppm",
            format::signed(p, 1),
            format::fixed(span.0, 0)
        ),
        (PeerRelation::Agree, _, _) => {
            format!("{rel}, both system clocks synced; a lock is not proven below 2 ppm")
        }
        (PeerRelation::Measuring, ..) => {
            format!("{rel} (less than a minute of audio since the stream started)")
        }
        _ => rel,
    }
}

/// The link to a remote rig in words: *up, 3.1 Mbit/s*, *unreachable*, *ketunkolo has not
/// admitted this rig*, …
pub fn link_text(r: &RemoteRig) -> String {
    match &r.link {
        RemoteLink::Up => format!("up, {}", bit_rate(r.bytes_per_s)),
        link => link_down_text(&r.name, link).unwrap_or_default(),
    }
}

/// The link to rig `name` in [`link_text`]'s words when it is not up; `None` when it is.
pub fn link_down_text(name: &str, link: &RemoteLink) -> Option<String> {
    Some(match link {
        RemoteLink::Up => return None,
        RemoteLink::NotPaired => "not paired: pair the rigs from the app".into(),
        RemoteLink::NoServerKey => {
            "this rig has no server key yet: turn its network on once".into()
        }
        RemoteLink::Connecting => "connecting".into(),
        RemoteLink::NetworkOff => format!("{name} turned its network off; not heard since"),
        RemoteLink::Unreachable { .. } => "unreachable".into(),
        RemoteLink::NotAdmitted => format!("{name} has not admitted this rig"),
        RemoteLink::KeyChanged => {
            format!("{name}'s key changed: pair the rigs again from the app")
        }
        RemoteLink::OtherRig { found } => format!("another rig answers there ({found})"),
    })
}

/// Audio received, `3.1 Mbit/s`.
fn bit_rate(bytes_per_s: f64) -> String {
    format!("{} Mbit/s", format::fixed(bytes_per_s * 8.0 / 1e6, 1))
}

/// Where a remote input's audio stands, `channel` its rig's input: *streaming*, *ketunkolo
/// does not capture in 1 (open it there)*, …
pub fn audio_text(a: RemoteAudio, rig: &str, channel: u16) -> String {
    let n = u32::from(channel) + 1;
    match a {
        RemoteAudio::Waiting => format!("waiting for {rig}"),
        RemoteAudio::OnThisRig => "that input is on this rig".into(),
        RemoteAudio::NoStream => format!("no audio from {rig} (no stream)"),
        RemoteAudio::NotCaptured => format!("{rig} does not capture in {n} (open it there)"),
        RemoteAudio::Replaying => format!("{rig} is replaying a recording"),
        RemoteAudio::RateMismatch { theirs, ours } => {
            format!(
                "{rig} runs {}, this rig {}",
                rate_text(theirs),
                rate_text(ours)
            )
        }
        RemoteAudio::Starting => "starting".into(),
        RemoteAudio::Streaming => "streaming".into(),
    }
}

/// How a remote input's samples are placed against this rig's.
pub fn anchor_text(a: &Anchor) -> &'static str {
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

/// A remote input's late frames and slips: `0 late frames in the last hour · 0 slips`.
pub fn input_counts(st: &RemoteInputStatus) -> String {
    format!(
        "{} late frames in the last hour · {} slips",
        st.late_frames_hour, st.slips
    )
}

/// A remote input's state in words, and how it is coloured: problems are said, never only
/// coloured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputState {
    pub text: String,
    pub tone: InputTone,
}

/// How a remote input's state is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputTone {
    /// Streaming, or on its way.
    Plain,
    /// No audio because its rig was told to stop serving (its network turned off): meant,
    /// so not a fault, yet nothing arrives.
    Warn,
    /// No audio, or audio that cannot be used.
    Fault,
}

impl InputTone {
    /// No audio, or audio that cannot be used, for whatever reason.
    pub fn is_problem(self) -> bool {
        self != Self::Plain
    }
}

/// Remote input `i`'s state: its rig's link first (no audio comes over a link that is not
/// up), then its audio, and while it streams how it is placed and its late frames and slips.
/// `at` gives a wall time as the reader's clock (`None`: the words go without it).
pub fn input_state(
    r: &RemoteState,
    i: &RemoteInput,
    at: &dyn Fn(WallNs) -> Option<String>,
) -> InputState {
    let rig = rig_name(r, i);
    let problem = |text: String| InputState {
        text,
        tone: InputTone::Fault,
    };
    let link = r.rigs.iter().find(|x| x.rig == i.rig).map(|x| &x.link);
    match link {
        Some(RemoteLink::NotPaired) => {
            return problem(format!(
                "not paired with {rig}: add the input again from the app"
            ));
        }
        Some(RemoteLink::NoServerKey) => {
            return problem(format!(
                "no audio from {rig}: this rig has no server key yet (turn its network on \
                 once)"
            ));
        }
        Some(RemoteLink::NetworkOff) => {
            return InputState {
                text: format!(
                    "no audio from {rig}: {rig} turned its network off and is not heard since \
                     (turn it on there)"
                ),
                tone: InputTone::Warn,
            };
        }
        Some(RemoteLink::Unreachable { since }) => {
            return problem(match at(*since) {
                Some(t) => format!("no audio from {rig} (unreachable since {t})"),
                None => format!("no audio from {rig} (unreachable)"),
            });
        }
        Some(RemoteLink::NotAdmitted) => {
            return problem(format!(
                "no audio from {rig}: {rig} has not admitted this rig (add the input again \
                 from the app)"
            ));
        }
        Some(RemoteLink::KeyChanged) => {
            return problem(format!(
                "{rig}'s key changed: pair the rigs again from the app"
            ));
        }
        Some(RemoteLink::OtherRig { .. }) => {
            return problem(format!(
                "no audio from {rig}: another rig answers at its address (pair the rigs again \
                 from the app)"
            ));
        }
        Some(RemoteLink::Connecting) => {
            return InputState {
                text: format!("connecting to {rig}…"),
                tone: InputTone::Plain,
            };
        }
        Some(RemoteLink::Up) | None => {}
    }
    let Some(st) = r.inputs.iter().find(|s| s.slot == i.slot) else {
        return problem(format!("waiting for {rig}"));
    };
    let mut text = audio_text(st.audio, &rig, i.channel);
    let streaming = matches!(st.audio, RemoteAudio::Streaming);
    if let Some(a) = &st.anchor {
        text.push_str(&format!(", {}", anchor_text(a)));
    }
    text.push_str(&format!(" · {}", input_counts(st)));
    let fault = !matches!(st.audio, RemoteAudio::Streaming | RemoteAudio::Starting)
        || (streaming && st.late_frames_hour > 0);
    InputState {
        text,
        tone: if fault {
            InputTone::Fault
        } else {
            InputTone::Plain
        },
    }
}

/// The *Between rigs* block of remote rig `rig` on the Connection page: the relation and
/// why, its last change, the link, then the link offset and what its inputs' audio met
/// (late frames, slips, the least arrival margin).
pub fn rig_block(
    r: &RemoteState,
    rig: &RemoteRig,
    inputs: &[RemoteInput],
    at: &dyn Fn(WallNs) -> Option<String>,
) -> Vec<String> {
    let mut out = vec![format!(
        "{}: {}",
        rig.name,
        relation_with_evidence(r, &rig.name, &rig.clock)
    )];
    if let Some(t) = rig.clock.last_change.and_then(at) {
        out.push(format!("last change {t}"));
    }
    let mut link = format!("link {}", link_text(rig));
    if let RemoteLink::Unreachable { since } = rig.link
        && let Some(t) = at(since)
    {
        link.push_str(&format!(" since {t}"));
    }
    link.push_str(&format!(" · window {} KiB", rig.window / 1024));
    out.push(link);
    let sts: Vec<&RemoteInputStatus> = inputs
        .iter()
        .filter(|i| i.rig == rig.rig)
        .filter_map(|i| r.inputs.iter().find(|s| s.slot == i.slot))
        .collect();
    let mut audio = Vec::new();
    if let Some(o) = r.link_offset {
        audio.push(format!("{} ms link offset", format::fixed(o.0 * 1e3, 0)));
    }
    if !sts.is_empty() {
        let late: u64 = sts.iter().map(|s| u64::from(s.late_frames_hour)).sum();
        let slips: u64 = sts.iter().map(|s| u64::from(s.slips)).sum();
        audio.push(format!("{late} late frames in the last hour"));
        audio.push(format!("{slips} slips"));
        let margin = sts
            .iter()
            .filter_map(|s| s.arrival_margin)
            .map(|m| m.0)
            .reduce(f64::min);
        if let Some(m) = margin {
            audio.push(format!("arrival margin {} ms", format::fixed(m * 1e3, 1)));
        }
    }
    if !audio.is_empty() {
        out.push(audio.join(" · "));
    }
    out
}

/// `between rigs` lines: one per remote rig, then one per remote input of `inputs` (the
/// stream's, in order). Empty without remote inputs.
pub fn status_lines(r: &RemoteState, inputs: &[RemoteInput]) -> Vec<String> {
    let mut out = Vec::new();
    for rig in &r.rigs {
        out.push(format!(
            "between rigs {} · {} · link {}",
            rig.name,
            relation_text(r, &rig.name, &rig.clock.relation),
            link_text(rig)
        ));
    }
    if let Some(o) = r.link_offset {
        out.push(format!("link offset  {} ms", format::fixed(o.0 * 1e3, 0)));
    }
    for i in inputs {
        if !r.inputs.iter().any(|s| s.slot == i.slot) {
            continue;
        }
        out.push(format!(
            "remote input {} in {}: {}",
            rig_name(r, i),
            i.channel + 1,
            input_state(r, i, &|_| None).text
        ));
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
            transfers: Vec::new(),
            standing: Vec::new(),
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
            l[0].contains("CLOCKS NOT LOCKED: ketunkolo runs +5.6 ppm against this rig over 40 s"),
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
            relation_text(&r, "ketunkolo", &r.rigs[0].clock.relation),
            "CLOCKS: CAN'T TELL (ketunkolo's system clock is not synced)"
        );
    }

    #[test]
    fn an_input_whose_rig_is_down_says_so_with_since_when() {
        let at = |t: WallNs| Some(format!("t{}", t.0));
        let (mut r, i) = state(PeerRelation::Measuring, RemoteAudio::Streaming);
        r.rigs[0].link = RemoteLink::Unreachable { since: WallNs(7) };
        let s = input_state(&r, &i[0], &at);
        assert_eq!(s.text, "no audio from ketunkolo (unreachable since t7)");
        assert_eq!(s.tone, InputTone::Fault);
        r.rigs[0].link = RemoteLink::KeyChanged;
        assert_eq!(
            input_state(&r, &i[0], &at).text,
            "ketunkolo's key changed: pair the rigs again from the app"
        );
        r.rigs[0].link = RemoteLink::NotAdmitted;
        assert!(
            input_state(&r, &i[0], &at)
                .text
                .contains("ketunkolo has not admitted this rig")
        );
        r.rigs[0].link = RemoteLink::NetworkOff;
        let s = input_state(&r, &i[0], &at);
        assert_eq!(
            s.text,
            "no audio from ketunkolo: ketunkolo turned its network off and is not heard since (turn \
             it on there)"
        );
        // Turned off on purpose: said as a warning, not a fault.
        assert_eq!(s.tone, InputTone::Warn);
        assert_eq!(
            link_text(&r.rigs[0]),
            "ketunkolo turned its network off; not heard since"
        );
        let (r, i) = state(PeerRelation::Measuring, RemoteAudio::NotCaptured);
        let s = input_state(&r, &i[0], &at);
        assert!(
            s.text
                .starts_with("ketunkolo does not capture in 1 (open it there)"),
            "{s:?}"
        );
        assert!(s.tone.is_problem());
        // Unknown to the remote state (not answered yet): the stream's name for it.
        let (mut r, i) = state(PeerRelation::Measuring, RemoteAudio::Streaming);
        r.rigs.clear();
        r.inputs.clear();
        assert_eq!(input_state(&r, &i[0], &at).text, "waiting for ketunkolo");
    }

    #[test]
    fn the_connection_block_has_the_relation_the_link_and_the_audio() {
        let at = |_: WallNs| Some("19:20".to_owned());
        let (mut r, i) = state(PeerRelation::Agree, RemoteAudio::Streaming);
        r.rigs[0].clock.ppm = Some(0.0);
        r.rigs[0].clock.span = Some(Seconds(60.0));
        r.rigs[0].clock.last_change = Some(WallNs(1));
        r.inputs[0].late_frames_hour = 2;
        r.inputs[0].slips = 1;
        r.inputs[0].arrival_margin = Some(Seconds(0.0042));
        assert_eq!(
            rig_block(&r, &r.rigs[0], &i, &at),
            vec![
                "ketunkolo: clocks agree: 0.0 ppm over the last 60 s, both system clocks \
                 synced; a lock is not proven below 2 ppm",
                "last change 19:20",
                "link up, 3.1 Mbit/s · window 256 KiB",
                "10 ms link offset · 2 late frames in the last hour · 1 slips · arrival margin \
                 4.2 ms",
            ]
        );
    }
}
