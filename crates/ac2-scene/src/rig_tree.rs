//! The measurement list as a tree rooted at the rig shown and the rigs its daemon is paired
//! with: each rig's heading, and under it everything that lives on that rig (its Rig monitor
//! meters, its session's measurements with their rows, its imported traces). A rig's branch
//! folds as a measurement's group does; folded, its heading says what it hides. A rig the
//! app holds that the rig shown is not paired with has no root here: listing it would read
//! as a link between the two daemons that does not exist.

use crate::between_rigs;
use crate::meas_list::{TreeKey, TreeRow, heading_row};
use ac2_proto::model::{RemoteAudio, RemoteInput, RemoteLink, RemoteState};
use ac2_proto::units::RigId;

/// A rig with no measurements and no audio I/O open, as the top bar says it.
pub const NO_SESSION: &str = "no session";

/// A rig whose measurements have no audio I/O open, as the top bar says it.
pub const NO_AUDIO: &str = "no audio I/O";

/// A pairing its rig reports nothing wrong with.
pub const PAIRED: &str = "paired";

/// What a rig's audio session runs at, beside its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigAudio {
    /// Its session's sample rate, Hz.
    Rate(u32),
    /// No audio I/O open and nothing measured.
    NoSession,
    /// Measurements, and no audio I/O open.
    NoAudio,
}

impl RigAudio {
    /// `96 kHz`, `44.1 kHz`: the rate as the top bar and the lines between rigs say it,
    /// since a pairing refused for the rate reads beside it on the same heading.
    pub fn text(self) -> String {
        match self {
            RigAudio::Rate(hz) => crate::audio::rate_text(hz),
            RigAudio::NoSession => NO_SESSION.to_owned(),
            RigAudio::NoAudio => NO_AUDIO.to_owned(),
        }
    }
}

/// One rig's root in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RigRoot<'a> {
    pub rig: RigId,
    /// As the drop-down names it: `pupu`, `ketunkolo`.
    pub name: &'a str,
    /// The rig the drop-down shows, where new measurements and the keys go.
    pub shown: bool,
    /// Its branch folded: only the heading is listed.
    pub folded: bool,
    /// Every measurement on the rig, its meters and math traces included.
    pub measurements: u32,
    /// Every stored trace on the rig.
    pub traces: u32,
    /// Its audio session, said beside the name.
    pub audio: RigAudio,
    /// Its clock domain's tag, said after the rate; `None`: no tag.
    pub clock: Option<ClockTag<'a>>,
    /// A paired rig: the pairing as the rig shown reports it ([`pairing_text`]). `None` for
    /// the rig shown.
    pub pairing: Option<&'a str>,
}

/// A rig's clock domain among the rigs listed ([`crate::clock_domain::clock_domains`]): the
/// tag on its heading and the words behind it, which the heading's tooltip carries since a
/// tag alone states no basis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockTag<'a> {
    /// `AGREE`, `FREE`, `UNKNOWN`.
    pub tag: &'a str,
    /// What the tag rests on.
    pub tip: &'a str,
}

/// Where a heading says how the clocks between its rig and the rig shown stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockVerdict {
    /// In the pairing's words: the heading carries no clock tag.
    InWords,
    /// On the heading's clock tag ([`ClockTag`]), judged on more evidence than the rig
    /// shown's daemon has (the other daemon's verdict, each system clock). The pairing
    /// then gives no second verdict that could read otherwise; a warning is still said,
    /// since it is why a measurement stopped, and a rig warned of is never in the tag's
    /// group with the rig shown.
    OnTag,
}

/// The pairing with `rig` as the rig shown's daemon reports it (`r`, with its session's
/// remote inputs `inputs`), in the words the lines between rigs use: a link that is not up,
/// else the first of that rig's inputs whose audio does not stream (*cm4 runs 48 kHz, this
/// rig 96 kHz*), else that its audio streams and, as `clocks` has it, how the clocks
/// between the two stand (*paired · streaming · clocks agree*, *CLOCKS NOT LOCKED +5.6 ppm
/// · paired · streaming*). A daemon says nothing of a paired rig its session takes no
/// input from: that one reads *paired*, no more.
pub fn pairing_text(
    r: &RemoteState,
    inputs: &[RemoteInput],
    rig: RigId,
    clocks: ClockVerdict,
) -> String {
    let Some(remote) = r.rigs.iter().find(|x| x.rig == rig) else {
        return PAIRED.to_owned();
    };
    if remote.link != RemoteLink::Up {
        return between_rigs::link_text(remote);
    }
    let mut audio = inputs.iter().filter(|i| i.rig == rig).filter_map(|i| {
        let st = r.inputs.iter().find(|s| s.slot == i.slot)?;
        Some((st.audio, i.channel))
    });
    match audio.clone().find(|(a, _)| *a != RemoteAudio::Streaming) {
        Some((a, channel)) => between_rigs::audio_text(a, &remote.name, channel),
        // Audio streams: the clocks between the two rigs are what is left to say. A
        // warning leads, so the narrowest list still shows it.
        None if audio.next().is_some() => {
            match (crate::cross_rig::clock_words(r, remote), clocks) {
                ((words, true), _) => format!("{words} · {PAIRED} · streaming"),
                ((words, false), ClockVerdict::InWords) => {
                    format!("{PAIRED} · streaming · {words}")
                }
                ((_, false), ClockVerdict::OnTag) => format!("{PAIRED} · streaming"),
            }
        }
        None => PAIRED.to_owned(),
    }
}

/// The heading's first line: the rig's name, its session's rate and its clock tag.
fn name_line(root: &RigRoot<'_>) -> String {
    let mut parts = vec![root.name.to_owned(), root.audio.text()];
    parts.extend(root.clock.map(|c| c.tag.to_owned()));
    parts.join(" · ")
}

/// Rig `root`'s heading, with its fold arrow.
pub fn rig_heading(root: &RigRoot<'_>) -> TreeRow {
    let name = root.name;
    // A paired rig's pairing leads every line: the narrowest list still says it.
    let lead = |rest: &str| match root.pairing {
        Some(p) => format!("{p} · {rest}"),
        None => rest.to_owned(),
    };
    let whose = match root.pairing {
        None => "the rig shown".to_owned(),
        Some(p) => format!("another rig, {p}"),
    };
    let (mut details, describe) = if root.folded {
        // The counts read as the Sessions dialog says a session's.
        let all = crate::sessions::counts(root.measurements, root.traces);
        let meas = all.split(" · ").next().unwrap_or_default().to_owned();
        (
            vec![
                lead(&format!("{all} · folded")),
                lead(&format!("{meas} · folded")),
            ],
            format!("{name}: {whose}, folded: {all} under it"),
        )
    } else if let Some(p) = root.pairing {
        (
            vec![
                lead("click shows this rig; its rows drag onto panes"),
                p.to_owned(),
            ],
            format!("{name}: {whose}; a click shows it, its rows drag onto panes as they are"),
        )
    } else {
        (
            vec!["shown: new measurements and the keys go here".to_owned()],
            format!("{name}: {whose}; new measurements and the keys go to it"),
        )
    };
    details.dedup();
    let describe = match root.clock {
        Some(c) => format!("{describe}\nclock {}: {}", c.tag, c.tip),
        None => describe,
    };
    let mut row = heading_row(
        TreeKey::RigName(root.rig),
        &name_line(root),
        details,
        describe,
    );
    row.collapsed = Some(root.folded);
    row
}

/// Why a paired rig's heading has no rows under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Away {
    /// This app holds no link to it: only the rig shown's daemon does.
    NotConnected,
    /// This app's link to it is down.
    Unreachable,
}

/// The heading of rig `name`, paired with the rig shown (`pairing`: [`pairing_text`]) and
/// not read by this app: the pairing is the rig shown's to report, the measurements are
/// not, so none is listed and the heading has no fold.
pub fn rig_away_heading(rig: RigId, name: &str, pairing: &str, away: Away) -> TreeRow {
    let (details, why) = match away {
        Away::NotConnected => (
            vec![
                format!("{pairing} · this app is not connected to it"),
                format!("{pairing} · not connected"),
            ],
            format!(
                "this app is not connected to it, so its measurements are not listed: \
                 {} in the rig drop-down adds it",
                crate::chrome::RIG_CONNECT
            ),
        ),
        Away::Unreachable => (
            vec![format!("{pairing} · {}", crate::chrome::RIG_UNREACHABLE)],
            format!(
                "{} from this app, so its measurements are not listed",
                crate::chrome::RIG_UNREACHABLE
            ),
        ),
    };
    let describe = format!("{name}: another rig, {pairing}; {why}");
    heading_row(TreeKey::RigName(rig), name, details, describe)
}

/// Rig `root`'s branch of the list: its heading, then (unfolded) its rows `rows`
/// ([`crate::meas_list::tree_rows`]) a level under it.
pub fn rig_branch(root: &RigRoot<'_>, rows: Vec<TreeRow>) -> Vec<TreeRow> {
    let mut v = vec![rig_heading(root)];
    if !root.folded {
        v.extend(rows.into_iter().map(|mut r| {
            r.depth += 1;
            r
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meas_list::tests::{rig, rows};
    use std::collections::BTreeSet;

    /// The rows as the list reads, each indented two spaces a level, a row under a header
    /// with its tree line.
    fn text(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                let fold = match r.collapsed {
                    Some(true) => "▸ ",
                    Some(false) => "▾ ",
                    None if r.key.is_heading() => "  ",
                    None if r.last => "└ ",
                    None => "├ ",
                };
                let pad = "  ".repeat(usize::from(r.depth));
                format!("{pad}{fold}{} · {}", r.name, r.details[0])
            })
            .collect()
    }

    fn root(name: &str, shown: bool, folded: bool) -> RigRoot<'_> {
        RigRoot {
            rig: RigId(if shown { 1 } else { 2 }),
            name,
            shown,
            folded,
            measurements: 4,
            traces: 5,
            audio: RigAudio::Rate(if shown { 96_000 } else { 48_000 }),
            clock: None,
            pairing: (!shown).then_some(PAIRED),
        }
    }

    /// The rig shown and a rig paired with it: each is a root over its own rows, a level in,
    /// its session's rate beside its name; the folded one lists only its heading, saying the
    /// pairing and how many measurements and traces it hides.
    #[test]
    fn each_rig_is_a_root_and_a_folded_one_says_what_it_hides() {
        let r = rig();
        let none = BTreeSet::new();
        let mut list = rig_branch(&root("ketunkolo", true, false), rows(&r, &none, None, None));
        list.extend(rig_branch(
            &root("pupu", false, true),
            rows(&r, &none, None, None),
        ));
        assert_eq!(
            text(&list),
            [
                "▾ ketunkolo · 96 kHz · shown: new measurements and the keys go here",
                "  ▾ TF  Main L · running",
                "    ├ Main L (live) · live",
                "    ├ pre-EQ · capture",
                "    ├ post-EQ · capture",
                "    └ pre ÷ post · math (live)",
                "  ▾ SWEEP  Genelec 1 m · 2 runs",
                "    ├ Run 1 · sweep run · 3 s −50.0 dBFS",
                "    └ Run 2 · sweep run · 3 s −50.0 dBFS",
                "  ▾ SPL  FOH SPL · running",
                "  ▾ Imported · 1 trace",
                "    └ 1083 94cm · imported",
                "▸ pupu · 48 kHz · paired · 4 measurements · 5 traces · folded",
            ]
        );
        let pupu = list.last().expect("pupu");
        assert_eq!(pupu.key, TreeKey::RigName(RigId(2)));
        assert_eq!(
            pupu.details.last().map(String::as_str),
            Some("paired · 4 measurements · folded")
        );
        assert_eq!(
            pupu.describe,
            "pupu: another rig, paired, folded: 4 measurements · 5 traces under it"
        );
        assert!(pupu.key.is_heading(), "a root is no curve: not dragged");
    }

    /// One of each: the counts read as the Sessions dialog's.
    #[test]
    fn a_folded_rig_counts_in_the_singular() {
        let one = RigRoot {
            measurements: 1,
            traces: 1,
            ..root("pupu", true, true)
        };
        assert_eq!(
            rig_heading(&one).details,
            ["1 measurement · 1 trace · folded", "1 measurement · folded"]
        );
        assert_eq!(
            rig_heading(&one).describe,
            "pupu: the rig shown, folded: 1 measurement · 1 trace under it"
        );
    }

    fn remote(link: RemoteLink, audio: Option<RemoteAudio>) -> (RemoteState, Vec<RemoteInput>) {
        use ac2_proto::model::{CalStatus, PeerClock, PeerRelation, RemoteInputStatus, RemoteRig};
        let rig = RigId(2);
        let state = RemoteState {
            peers: Vec::new(),
            rigs: vec![RemoteRig {
                rig,
                name: "cm4".into(),
                link,
                clock: PeerClock {
                    relation: PeerRelation::Measuring,
                    ppm: None,
                    span: None,
                    last_change: None,
                },
                bytes_per_s: 0.0,
                window: 0,
            }],
            inputs: audio
                .into_iter()
                .map(|audio| RemoteInputStatus {
                    slot: 0,
                    audio,
                    anchor: None,
                    late_frames: 0,
                    late_frames_hour: 0,
                    slips: 0,
                    last_slip: None,
                    arrival_margin: None,
                    setup: None,
                    cal_status: CalStatus::Uncalibrated,
                    calibration: None,
                    curve: None,
                })
                .collect(),
            link_offset: None,
            transfers: Vec::new(),
        };
        let inputs = vec![RemoteInput {
            slot: 0,
            rig,
            rig_name: "cm4".into(),
            channel: 0,
        }];
        (state, inputs)
    }

    /// A rig beside its name says its session's rate, or what the top bar says of a rig
    /// without one; a clock tag follows the rate.
    #[test]
    fn a_heading_says_the_rigs_rate_beside_its_name() {
        let head = |audio, clock: Option<&'static str>| {
            rig_heading(&RigRoot {
                audio,
                clock: clock.map(|tag| ClockTag {
                    tag,
                    tip: "with cm4, by the rate between the two rigs",
                }),
                ..root("pupu", true, false)
            })
        };
        let name = |audio, clock| head(audio, clock).name;
        assert_eq!(name(RigAudio::Rate(96_000), None), "pupu · 96 kHz");
        assert_eq!(name(RigAudio::Rate(44_100), None), "pupu · 44.1 kHz");
        assert_eq!(name(RigAudio::NoSession, None), "pupu · no session");
        assert_eq!(name(RigAudio::NoAudio, None), "pupu · no audio I/O");
        assert_eq!(
            name(RigAudio::Rate(48_000), Some("AGREE")),
            "pupu · 48 kHz · AGREE"
        );
        // The tag's basis is in the heading's tooltip, after what the heading is.
        let tagged = head(RigAudio::Rate(48_000), Some("AGREE")).describe;
        assert_eq!(
            tagged,
            "pupu: the rig shown; new measurements and the keys go to it\n\
             clock AGREE: with cm4, by the rate between the two rigs"
        );
        assert!(
            !head(RigAudio::Rate(48_000), None)
                .describe
                .contains("clock")
        );
    }

    /// The pairing reads as the rig shown's daemon reports it, in the words of the lines
    /// between rigs; with no input taken from the rig the daemon reports nothing more than
    /// the pairing.
    #[test]
    fn a_pairing_reads_as_the_rig_shown_reports_it() {
        let rig = RigId(2);
        let words = ClockVerdict::InWords;
        assert_eq!(
            pairing_text(&RemoteState::default(), &[], rig, words),
            "paired"
        );
        let text = |link, audio| {
            let (r, inputs) = remote(link, audio);
            pairing_text(&r, &inputs, rig, words)
        };
        // With a clock tag on the heading the tag is the verdict: the pairing gives none.
        let (r, inputs) = remote(RemoteLink::Up, Some(RemoteAudio::Streaming));
        assert_eq!(
            pairing_text(&r, &inputs, rig, ClockVerdict::OnTag),
            "paired · streaming"
        );
        assert_eq!(
            text(RemoteLink::Up, Some(RemoteAudio::Streaming)),
            "paired · streaming · measuring clocks…"
        );
        assert_eq!(
            text(
                RemoteLink::Up,
                Some(RemoteAudio::RateMismatch {
                    theirs: 48_000,
                    ours: 96_000
                })
            ),
            "cm4 runs 48 kHz, this rig 96 kHz"
        );
        assert_eq!(text(RemoteLink::Connecting, None), "connecting");
        assert_eq!(
            text(RemoteLink::NotAdmitted, None),
            "cm4 has not admitted this rig"
        );
        assert_eq!(text(RemoteLink::Up, None), "paired");
    }

    /// A paired rig unfolded: the pairing leads its line and is all of the narrowest one.
    #[test]
    fn a_paired_rigs_heading_says_the_pairing() {
        let h = rig_heading(&RigRoot {
            pairing: Some("cm4 runs 48 kHz, this rig 96 kHz"),
            ..root("cm4", false, false)
        });
        assert_eq!(h.name, "cm4 · 48 kHz");
        assert_eq!(
            h.details,
            [
                "cm4 runs 48 kHz, this rig 96 kHz · click shows this rig; its rows drag onto \
                 panes",
                "cm4 runs 48 kHz, this rig 96 kHz"
            ]
        );
        assert_eq!(
            h.describe,
            "cm4: another rig, cm4 runs 48 kHz, this rig 96 kHz; a click shows it, its rows \
             drag onto panes as they are"
        );
    }

    /// A paired rig this app does not read: its heading alone, with the pairing and why no
    /// measurement is listed; nothing to fold.
    #[test]
    fn a_paired_rig_not_connected_is_a_heading_alone() {
        let h = rig_away_heading(RigId(2), "cm4", "connecting", Away::NotConnected);
        assert_eq!((h.name.as_str(), h.collapsed), ("cm4", None));
        assert_eq!(
            h.details,
            [
                "connecting · this app is not connected to it",
                "connecting · not connected"
            ]
        );
        assert_eq!(
            h.describe,
            "cm4: another rig, connecting; this app is not connected to it, so its \
             measurements are not listed: Connect a rig… in the rig drop-down adds it"
        );
        let h = rig_away_heading(RigId(2), "cm4", PAIRED, Away::Unreachable);
        assert_eq!(h.details, ["paired · unreachable · retrying"]);
        assert_eq!(
            h.describe,
            "cm4: another rig, paired; unreachable · retrying from this app, so its \
             measurements are not listed"
        );
    }
}
