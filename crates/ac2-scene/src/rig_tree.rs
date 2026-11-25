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
    pub pairing: Option<&'a Pairing>,
}

/// A rig's clock domain among the rigs listed ([`crate::clock_domain::clock_domains`]): the
/// tag on its heading and the words behind it, which the heading's tooltip carries since a
/// tag alone states no basis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockTag<'a> {
    /// `AGREE`, `FREE`, `TIMING`, `UNKNOWN`.
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

/// A pairing as the rig shown's daemon reports it: its words ([`Pairing::text`]), and
/// whether a heading has to say them beside its rig's rate and clock tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pairing {
    /// Paired and nothing more reported: a rig listed under the rig shown is paired anyway.
    Plain,
    /// Its audio is refused for the rate (*cm4 runs 48 kHz, this rig 96 kHz*): the two
    /// headings' rates already say it.
    RateMismatch(String),
    /// Anything else the rig shown reports: a link not up, audio not streaming, a clock
    /// warning, streaming.
    Said(String),
    /// No input taken from it, and the link its levels come over not up: a few words that
    /// say whose link it is (*link from pupu: unreachable*), and the sentence for the
    /// tooltip ([`between_rigs::link_down_text`]).
    Link {
        /// The few words, for the heading's line.
        short: String,
        /// The sentence.
        full: String,
    },
}

impl Pairing {
    /// The pairing in full, as the heading's tooltip says it.
    pub fn text(&self) -> &str {
        match self {
            Pairing::Plain => PAIRED,
            Pairing::RateMismatch(t) | Pairing::Said(t) => t,
            Pairing::Link { full, .. } => full,
        }
    }

    /// The pairing as a heading's line says it, where a sentence has no room.
    pub fn line(&self) -> &str {
        match self {
            Pairing::Link { short, .. } => short,
            other => other.text(),
        }
    }

    /// The link from rig `from` (the rig shown) to rig `to` not up (`link`), as the rig
    /// shown's daemon reports it with the levels; `None` while it is up.
    pub fn link(from: &str, to: &str, link: &RemoteLink) -> Option<Self> {
        Some(Pairing::Link {
            short: format!("link from {from}: {}", crate::standing::link_short(link)),
            full: between_rigs::link_down_text(to, link)?,
        })
    }

    /// What the heading's second line says of it after the rate and the clock tag: only
    /// what nothing else on screen says.
    pub fn on_heading(&self) -> Option<&str> {
        match self {
            Pairing::Plain | Pairing::RateMismatch(_) => None,
            Pairing::Said(t) | Pairing::Link { short: t, .. } => Some(t),
        }
    }
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
) -> Pairing {
    let Some(remote) = r.rigs.iter().find(|x| x.rig == rig) else {
        return Pairing::Plain;
    };
    if remote.link != RemoteLink::Up {
        return Pairing::Said(between_rigs::link_text(remote));
    }
    let mut audio = inputs.iter().filter(|i| i.rig == rig).filter_map(|i| {
        let st = r.inputs.iter().find(|s| s.slot == i.slot)?;
        Some((st.audio, i.channel))
    });
    match audio.clone().find(|(a, _)| *a != RemoteAudio::Streaming) {
        Some((a @ RemoteAudio::RateMismatch { .. }, channel)) => {
            Pairing::RateMismatch(between_rigs::audio_text(a, &remote.name, channel))
        }
        Some((a, channel)) => Pairing::Said(between_rigs::audio_text(a, &remote.name, channel)),
        // Audio streams: the clocks between the two rigs are what is left to say. A
        // warning leads, so the narrowest list still shows it.
        None if audio.next().is_some() => {
            Pairing::Said(match (crate::cross_rig::clock_words(r, remote), clocks) {
                ((words, true), _) => format!("{words} · {PAIRED} · streaming"),
                ((words, false), ClockVerdict::InWords) => {
                    format!("{PAIRED} · streaming · {words}")
                }
                ((_, false), ClockVerdict::OnTag) => format!("{PAIRED} · streaming"),
            })
        }
        None => Pairing::Plain,
    }
}

/// The heading's second line at its narrowest: its session's rate and its clock tag,
/// `48 kHz FREE`. Between two rigs' headings these say what a pairing refused for the rate
/// would.
fn audio_line(root: &RigRoot<'_>) -> String {
    match root.clock {
        Some(c) => format!("{} {}", root.audio.text(), c.tag),
        None => root.audio.text(),
    }
}

/// A paired rig's pairing as its hover's first line: `Paired · streaming · clocks agree`,
/// `Paired rig: connecting`.
fn paired_words(pairing: &str) -> String {
    match pairing.strip_prefix(PAIRED) {
        Some("") => "Paired rig".to_owned(),
        Some(rest) => format!("Paired{rest}"),
        None => format!("Paired rig: {pairing}"),
    }
}

/// Rig `root`'s heading, with its fold arrow: its name, and under it its rate and clock tag
/// and what else the list cannot show of it.
pub fn rig_heading(root: &RigRoot<'_>) -> TreeRow {
    let name = root.name;
    let audio = audio_line(root);
    // The rate and tag lead every line, the pairing next: a narrower list drops the counts
    // first and the pairing's words last.
    let said = root.pairing.and_then(Pairing::on_heading);
    let lead = |rest: &[&str]| {
        let mut parts = vec![audio.as_str()];
        parts.extend(said);
        parts.extend(rest);
        parts.join(" · ")
    };
    let whose = match root.pairing {
        None => "The rig shown".to_owned(),
        Some(p) => paired_words(p.text()),
    };
    let (mut details, describe) = if root.folded {
        // The counts read as the Sessions dialog says a session's.
        let all = crate::sessions::counts(root.measurements, root.traces);
        let meas = all.split(" · ").next().unwrap_or_default().to_owned();
        (
            vec![
                lead(&[&all, "folded"]),
                lead(&[&meas, "folded"]),
                lead(&[]),
                audio.clone(),
            ],
            format!("{whose} · folded: {all}"),
        )
    } else {
        (vec![lead(&[]), audio.clone()], whose)
    };
    details.dedup();
    let describe = match root.clock {
        Some(c) => format!("{describe}\n{}: {}", c.tag, c.tip),
        None => describe,
    };
    let mut row = heading_row(TreeKey::RigName(root.rig), name, details, describe);
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

/// What the rig shown's daemon hears of a paired rig in its levels
/// (`State.remote.standing`) while the link between the two is up: its session, and the
/// clock tag that rests on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heard<'a> {
    /// Its session's rate, or [`RigAudio::NoSession`].
    pub audio: RigAudio,
    /// Its clock domain's tag; `None`: no tag.
    pub clock: Option<ClockTag<'a>>,
}

/// The heading of rig `name`, paired with the rig shown (`pairing`: [`pairing_text`]) and
/// not read by this app: the pairing is the rig shown's to report, the measurements are
/// not, so none is listed and the heading has no fold. `heard`: its rate and tag lead the
/// line, as on a heading the app reads, and the narrowest line keeps them.
pub fn rig_away_heading(
    rig: RigId,
    name: &str,
    pairing: &Pairing,
    away: Away,
    heard: Option<Heard<'_>>,
) -> TreeRow {
    let (said, pairing) = (pairing.line(), pairing.text());
    let lead = heard.map(|h| match h.clock {
        Some(c) => format!("{} {}", h.audio.text(), c.tag),
        None => h.audio.text(),
    });
    let line = |rest: &[&str]| {
        let mut parts: Vec<&str> = lead.iter().map(String::as_str).collect();
        parts.push(said);
        parts.extend(rest);
        parts.join(" · ")
    };
    let (mut details, why) = match away {
        Away::NotConnected => (
            vec![
                line(&["this app is not connected to it"]),
                line(&["not connected"]),
            ],
            "Not connected: add it from the rig drop-down".to_owned(),
        ),
        Away::Unreachable => (
            vec![line(&[crate::chrome::RIG_UNREACHABLE])],
            "Its measurements show once it is reachable".to_owned(),
        ),
    };
    details.extend(lead.clone());
    let mut describe = format!("{}\n{why}", paired_words(pairing));
    if let Some(c) = heard.and_then(|h| h.clock) {
        describe.push_str(&format!("\n{}: {}", c.tag, c.tip));
    }
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
            pairing: (!shown).then_some(&Pairing::Plain),
        }
    }

    /// The rig shown and a rig paired with it: each is a root over its own rows, a level in,
    /// its session's rate under its name; the folded one lists only its heading, saying how
    /// many measurements and traces it hides. Plain *paired* is not said: a rig listed under
    /// the rig shown is paired with it.
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
                "▾ ketunkolo · 96 kHz",
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
                "▸ pupu · 48 kHz · 4 measurements · 5 traces · folded",
            ]
        );
        let pupu = list.last().expect("pupu");
        assert_eq!(pupu.key, TreeKey::RigName(RigId(2)));
        assert_eq!(pupu.name, "pupu");
        assert_eq!(
            pupu.details,
            [
                "48 kHz · 4 measurements · 5 traces · folded",
                "48 kHz · 4 measurements · folded",
                "48 kHz"
            ]
        );
        assert_eq!(
            pupu.describe,
            "Paired rig · folded: 4 measurements · 5 traces"
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
            [
                "96 kHz · 1 measurement · 1 trace · folded",
                "96 kHz · 1 measurement · folded",
                "96 kHz"
            ]
        );
        assert_eq!(
            rig_heading(&one).describe,
            "The rig shown · folded: 1 measurement · 1 trace"
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
            standing: Vec::new(),
        };
        let inputs = vec![RemoteInput {
            slot: 0,
            rig,
            rig_name: "cm4".into(),
            channel: 0,
        }];
        (state, inputs)
    }

    /// A rig's name stands alone on its heading's first line; the second says its session's
    /// rate, or what the top bar says of a rig without one, and its clock tag after the rate.
    /// The rig shown says nothing more: where new measurements and the keys go is its
    /// tooltip's.
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
        let lines = |audio, clock| {
            let h = head(audio, clock);
            (h.name, h.details)
        };
        let says = |line: &str| ("pupu".to_owned(), vec![line.to_owned()]);
        assert_eq!(lines(RigAudio::Rate(96_000), None), says("96 kHz"));
        assert_eq!(lines(RigAudio::Rate(44_100), None), says("44.1 kHz"));
        assert_eq!(lines(RigAudio::NoSession, None), says("no session"));
        assert_eq!(lines(RigAudio::NoAudio, None), says("no audio I/O"));
        assert_eq!(
            lines(RigAudio::Rate(96_000), Some("FREE")),
            says("96 kHz FREE")
        );
        // The tag's basis is in the heading's tooltip, after what the heading is.
        let tagged = head(RigAudio::Rate(48_000), Some("AGREE")).describe;
        assert_eq!(
            tagged,
            "The rig shown\nAGREE: with cm4, by the rate between the two rigs"
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
            Pairing::Plain
        );
        assert_eq!(Pairing::Plain.text(), "paired");
        let pairing = |link, audio| {
            let (r, inputs) = remote(link, audio);
            pairing_text(&r, &inputs, rig, words)
        };
        let text = |link, audio| pairing(link, audio).text().to_owned();
        // With a clock tag on the heading the tag is the verdict: the pairing gives none.
        let (r, inputs) = remote(RemoteLink::Up, Some(RemoteAudio::Streaming));
        assert_eq!(
            pairing_text(&r, &inputs, rig, ClockVerdict::OnTag),
            Pairing::Said("paired · streaming".into())
        );
        assert_eq!(
            text(RemoteLink::Up, Some(RemoteAudio::Streaming)),
            "paired · streaming · measuring clocks…"
        );
        // Told apart from the rest: the two headings' rates say it.
        assert_eq!(
            pairing(
                RemoteLink::Up,
                Some(RemoteAudio::RateMismatch {
                    theirs: 48_000,
                    ours: 96_000
                })
            ),
            Pairing::RateMismatch("cm4 runs 48 kHz, this rig 96 kHz".into())
        );
        assert_eq!(text(RemoteLink::Connecting, None), "connecting");
        assert_eq!(
            text(RemoteLink::NotAdmitted, None),
            "cm4 has not admitted this rig"
        );
        // A rig that turned its network off says so on the heading's second line.
        assert_eq!(
            pairing(RemoteLink::NetworkOff, None).on_heading(),
            Some("cm4 turned its network off; not heard since")
        );
        assert_eq!(pairing(RemoteLink::Up, None), Pairing::Plain);
    }

    /// A paired rig unfolded at another rate: its rate and tag are all of its second line,
    /// the rate-refused pairing, the click and the drag its tooltip's.
    #[test]
    fn a_paired_rig_at_another_rate_says_its_rate_and_tag() {
        let refused = Pairing::RateMismatch("cm4 runs 48 kHz, this rig 96 kHz".into());
        let free = ClockTag {
            tag: "FREE",
            tip: "no clock in common with ketunkolo",
        };
        let h = rig_heading(&RigRoot {
            pairing: Some(&refused),
            clock: Some(free),
            ..root("cm4", false, false)
        });
        assert_eq!(h.name, "cm4");
        assert_eq!(h.details, ["48 kHz FREE"]);
        assert_eq!(
            h.describe,
            "Paired rig: cm4 runs 48 kHz, this rig 96 kHz\nFREE: no clock in common with ketunkolo"
        );
    }

    /// What the rig shown reports beyond the rates follows the rate and tag, a warning as
    /// loud as the rest of the line; narrower, the counts go first, the words next, the rate
    /// and tag never.
    #[test]
    fn a_paired_rigs_heading_says_what_else_the_pairing_reports() {
        let warned = Pairing::Said("CLOCKS NOT LOCKED +5.6 ppm · paired · streaming".into());
        let unfolded = rig_heading(&RigRoot {
            pairing: Some(&warned),
            ..root("cm4", false, false)
        });
        assert_eq!(
            unfolded.details,
            [
                "48 kHz · CLOCKS NOT LOCKED +5.6 ppm · paired · streaming",
                "48 kHz"
            ]
        );
        let late = Pairing::Said("cm4 has not admitted this rig".into());
        let folded = rig_heading(&RigRoot {
            pairing: Some(&late),
            ..root("cm4", false, true)
        });
        assert_eq!(
            folded.details,
            [
                "48 kHz · cm4 has not admitted this rig · 4 measurements · 5 traces · folded",
                "48 kHz · cm4 has not admitted this rig · 4 measurements · folded",
                "48 kHz · cm4 has not admitted this rig",
                "48 kHz"
            ]
        );
    }

    /// A paired rig this app does not read: its heading alone, with the pairing and why no
    /// measurement is listed; nothing to fold.
    #[test]
    fn a_paired_rig_not_connected_is_a_heading_alone() {
        let h = rig_away_heading(
            RigId(2),
            "cm4",
            &Pairing::Said("connecting".into()),
            Away::NotConnected,
            None,
        );
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
            "Paired rig: connecting\nNot connected: add it from the rig drop-down"
        );
        let h = rig_away_heading(RigId(2), "cm4", &Pairing::Plain, Away::Unreachable, None);
        assert_eq!(h.details, ["paired · unreachable · retrying"]);
        assert_eq!(
            h.describe,
            "Paired rig\nIts measurements show once it is reachable"
        );
    }

    /// A paired rig the app does not read leads with what the rig shown's daemon hears in
    /// its levels: the rate and tag, which the narrowest line keeps.
    #[test]
    fn a_paired_rig_not_read_says_its_rate_and_tag_from_its_levels() {
        let timing = ClockTag {
            tag: "TIMING",
            tip: "can't tell: measuring clocks… · from the rigs' level frames",
        };
        let heard = Heard {
            audio: RigAudio::Rate(96_000),
            clock: Some(timing),
        };
        let h = rig_away_heading(
            RigId(2),
            "ketunkolo",
            &Pairing::Plain,
            Away::NotConnected,
            Some(heard),
        );
        assert_eq!(
            h.details,
            [
                "96 kHz TIMING · paired · this app is not connected to it",
                "96 kHz TIMING · paired · not connected",
                "96 kHz TIMING",
            ]
        );
        assert!(
            h.describe
                .ends_with("\nTIMING: can't tell: measuring clocks… · from the rigs' level frames")
        );
        let idle = Heard {
            audio: RigAudio::NoSession,
            clock: None,
        };
        let h = rig_away_heading(
            RigId(2),
            "ketunkolo",
            &Pairing::Plain,
            Away::Unreachable,
            Some(idle),
        );
        assert_eq!(
            h.details,
            ["no session · paired · unreachable · retrying", "no session"]
        );
        // The link between the daemons down: its words, nothing heard.
        let h = rig_away_heading(
            RigId(2),
            "ketunkolo",
            &Pairing::Said("unreachable".into()),
            Away::NotConnected,
            None,
        );
        assert_eq!(
            h.details,
            [
                "unreachable · this app is not connected to it",
                "unreachable · not connected"
            ]
        );
    }
}
