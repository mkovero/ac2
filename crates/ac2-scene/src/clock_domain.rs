//! Which rigs of a list share a sample clock, as a tag on each rig's row and the words
//! behind it: `LOCKED`, `AGREE`, `FREE`, `UNKNOWN`.
//!
//! A transfer function between two rigs means something only when both count samples on
//! one clock. Three kinds of evidence say whether they do, strongest first:
//!
//! 1. the nominal sample rates: two rates are never one clock;
//! 2. a daemon's own verdict on the stream it takes from the other rig
//!    ([`PeerClock`], the *between rigs* line);
//! 3. each rig's sample clock against its own system clock, which compares the two rigs
//!    only while both system clocks are held to a common time closely enough.
//!
//! Neither 2 nor 3 can prove a lock: block stamps resolve about 2 ppm over a minute, and
//! two free crystals can sit that close. The daemon's verdict watches one stream over two
//! windows and holds a failure for a minute, so it earns `LOCKED`; agreement of two
//! separately measured figures earns only `AGREE`. Every tooltip states its basis.

use ac2_proto::model::{
    CantTellReason, ClockBasis, Clocks, NotLockedReason, PeerClock, PeerRelation, RemoteRig,
    SampleClock, SystemClock,
};
use ac2_proto::units::RigId;

use crate::audio::rate_text;
use crate::clock::system_clock_text;
use crate::format;

/// How far two sample clocks may read apart and still agree, ppm. Stamps resolve 0.34–0.40
/// ppm (one sd) over a minute, so the difference of two rigs' figures has an sd near 0.55
/// ppm and one clock read twice lands inside ±2 ppm; it is also the bound the daemon's own
/// between-rigs verdict uses, so both kinds of evidence mean the same by *agree*.
pub const AGREE_PPM: f64 = 2.0;

/// The kernel widens its error bound by this much per second without an update from the
/// discipline daemon (Linux `MAXFREQ`, the worst oscillator it assumes), s/s.
const BOUND_GROWTH: f64 = 500e-6;

/// What the app knows about one rig of the list.
#[derive(Debug, Clone, Copy)]
pub struct RigClockFacts<'a> {
    /// The rig's id, when it has stated one; what [`RemoteRig::rig`] of another rig is
    /// matched against.
    pub id: Option<RigId>,
    /// Its display name.
    pub name: &'a str,
    /// Its session's sample rate, Hz; `None` with no audio session.
    pub rate_hz: Option<u32>,
    /// Its clocks.
    pub clocks: &'a Clocks,
    /// The rigs its session takes inputs from, each with the daemon's verdict on the clocks
    /// between the two (`State.remote.rigs`); empty when it takes none or is not known.
    pub between: &'a [RemoteRig],
}

/// What a group's agreement rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// Daemons' verdicts on the streams between the group's rigs join all of it.
    BetweenRigs,
    /// At least one rig is joined only by its figure against its system clock.
    SystemClocks,
}

/// Where a rig stands among the rigs of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// No audio session: no sample clock to speak of.
    NoSession,
    /// The only rig with a session: nothing to compare with.
    NotCompared,
    /// One of a group of rigs whose clocks agree; `index` counts groups in list order of
    /// their first rig.
    Group {
        /// The group.
        index: usize,
        /// What the agreement rests on.
        basis: Basis,
    },
    /// Shown to differ from every other rig with a session.
    Own,
    /// The evidence does not say.
    CantTell,
}

/// One rig's row tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainTag {
    /// The tag: `LOCKED`, `LOCKED A`, `AGREE`, `FREE`, `UNKNOWN`; `None`
    /// where there is nothing to say (no session, or no other rig to compare with).
    pub tag: Option<String>,
    /// The tooltip: the other rigs of the group and the basis, or what differs, or what is
    /// missing. `None` exactly when `tag` is.
    pub tip: Option<String>,
    /// The same as a value.
    pub domain: Domain,
}

/// Whether two rigs count samples on one clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SameDomain {
    /// The evidence puts them in one group (see the group's [`Basis`]: agreement within
    /// [`AGREE_PPM`], never a proven lock).
    Yes,
    /// Shown to differ.
    No {
        /// What differs, as a sentence without a full stop.
        reason: String,
    },
    /// The evidence does not say.
    CantTell {
        /// What is missing.
        reason: String,
    },
}

/// The clock domains of a list of rigs: [`clock_domains`]' answer.
#[derive(Debug, Clone, PartialEq)]
pub struct ClockDomains {
    /// One tag per rig, in the order given.
    pub rigs: Vec<DomainTag>,
    names: Vec<String>,
    rates: Vec<Option<u32>>,
    pairs: Vec<Vec<Pair>>,
    /// Per rig: why a chain of agreements it belongs to makes no group.
    broken: Vec<Option<String>>,
}

/// What the evidence says of two rigs; ppm figures are the row rig against the column rig.
#[derive(Debug, Clone, PartialEq)]
enum Pair {
    /// One of them has no session (or it is the rig itself).
    Skip,
    Agree {
        /// By a daemon's between-rigs verdict.
        direct: bool,
        /// That verdict's figure: ppm, span s.
        figure: Option<(f64, f64)>,
    },
    Differ(Differ),
    CantTell(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
enum Differ {
    Rates,
    /// The daemon saw the two sample clocks run apart.
    Cross {
        ppm: f64,
        over: f64,
    },
    /// The daemon saw one rig's interface run off its own system clock.
    OwnClock {
        who: String,
        ppm: f64,
    },
    /// The two figures against each rig's system clock differ.
    BySystemClocks {
        ppm: f64,
    },
}

impl Pair {
    /// The same seen from the other rig.
    fn mirrored(&self) -> Self {
        match self {
            Self::Agree { direct, figure } => Self::Agree {
                direct: *direct,
                figure: figure.map(|(p, s)| (-p, s)),
            },
            Self::Differ(Differ::Cross { ppm, over }) => Self::Differ(Differ::Cross {
                ppm: -ppm,
                over: *over,
            }),
            Self::Differ(Differ::BySystemClocks { ppm }) => {
                Self::Differ(Differ::BySystemClocks { ppm: -ppm })
            }
            other => other.clone(),
        }
    }
}

/// The clock domains of `rigs`, one tag per rig.
///
/// Groups are rigs joined by agreement, direct or through other rigs, with no two of them
/// shown to differ. Where a chain of agreements holds a pair that differs (A and B within
/// 2 ppm, B and C within 2 ppm, A and C not), the chain is no group: its rigs fall back to
/// the groups the between-rigs verdicts alone make, and the rest read `UNKNOWN`.
///
/// ```
/// use ac2_proto::model::{ClockBasis, Clocks, DeviceClock, DeviceId, SampleClock, SystemClock};
/// use ac2_proto::units::Seconds;
/// use ac2_scene::clock_domain::{RigClockFacts, clock_domains};
///
/// let clocks = Clocks {
///     system: SystemClock::Synced { maxerror_us: 3027 },
///     capture: Some(DeviceClock {
///         device: DeviceId("system".into()),
///         rate: SampleClock::Measured {
///             ppm: 0.2,
///             span: Seconds(59.9),
///             against: ClockBasis::SystemClock,
///         },
///     }),
///     stamp_map: None,
/// };
/// let rig = |name, rate_hz| RigClockFacts {
///     id: None,
///     name,
///     rate_hz: Some(rate_hz),
///     clocks: &clocks,
///     between: &[],
/// };
/// let d = clock_domains(&[rig("pupu", 96_000), rig("ketunkolo", 96_000), rig("cm4", 48_000)]);
/// assert_eq!(d.rigs[0].tag.as_deref(), Some("AGREE"));
/// assert_eq!(d.rigs[2].tag.as_deref(), Some("FREE"));
/// assert_eq!(
///     d.rigs[2].tip.as_deref(),
///     Some("48 kHz here; pupu and ketunkolo at 96 kHz")
/// );
/// ```
pub fn clock_domains(rigs: &[RigClockFacts<'_>]) -> ClockDomains {
    let n = rigs.len();
    // Each pair is judged once, from its earlier rig, and mirrored for the later one, so the
    // two rigs can never read each other differently.
    let pairs: Vec<Vec<Pair>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| match i.cmp(&j) {
                    std::cmp::Ordering::Less => pair(rigs, i, j),
                    std::cmp::Ordering::Equal => Pair::Skip,
                    std::cmp::Ordering::Greater => pair(rigs, j, i).mirrored(),
                })
                .collect()
        })
        .collect();
    let active: Vec<usize> = (0..n).filter(|&i| rigs[i].rate_hz.is_some()).collect();
    let mut out = ClockDomains {
        rigs: rigs
            .iter()
            .map(|r| DomainTag {
                tag: None,
                tip: None,
                domain: match r.rate_hz {
                    Some(_) => Domain::NotCompared,
                    None => Domain::NoSession,
                },
            })
            .collect(),
        names: rigs.iter().map(|r| r.name.to_owned()).collect(),
        rates: rigs.iter().map(|r| r.rate_hz).collect(),
        pairs,
        broken: vec![None; n],
    };
    if active.len() < 2 {
        return out;
    }

    // Groups, each with its basis, in list order of their first rig.
    let mut groups: Vec<(Vec<usize>, Basis)> = Vec::new();
    for chain in components(&active, |a, b| {
        matches!(out.pairs[a][b], Pair::Agree { .. })
    }) {
        let by_verdicts =
            |a: usize, b: usize| matches!(out.pairs[a][b], Pair::Agree { direct: true, .. });
        let whole = components(&chain, by_verdicts);
        match first_differing(&out.pairs, &chain) {
            None if chain.len() >= 2 => {
                let basis = match whole.len() {
                    1 => Basis::BetweenRigs,
                    _ => Basis::SystemClocks,
                };
                groups.push((chain, basis));
            }
            None => {}
            Some((a, b)) => {
                let why = format!(
                    "the rates agree in a chain within {} ppm, but {}",
                    format::fixed(AGREE_PPM, 0),
                    out.sentence(a, b)
                );
                for part in whole {
                    if part.len() >= 2 && first_differing(&out.pairs, &part).is_none() {
                        groups.push((part, Basis::BetweenRigs));
                    } else {
                        for i in part {
                            out.broken[i] = Some(why.clone());
                        }
                    }
                }
            }
        }
    }
    groups.sort_by_key(|(members, _)| members[0]);

    for (index, (members, basis)) in groups.iter().enumerate() {
        let word = match basis {
            Basis::BetweenRigs => "LOCKED",
            Basis::SystemClocks => "AGREE",
        };
        let tag = match groups.len() {
            1 => word.to_owned(),
            _ => format!("{word} {}", group_letter(index)),
        };
        for &i in members {
            out.rigs[i] = DomainTag {
                tag: Some(tag.clone()),
                tip: Some(out.group_tip(i, members, *basis)),
                domain: Domain::Group {
                    index,
                    basis: *basis,
                },
            };
        }
    }
    for &i in &active {
        if matches!(out.rigs[i].domain, Domain::Group { .. }) {
            continue;
        }
        let others = || active.iter().copied().filter(|&j| j != i);
        let alone = others().all(|j| matches!(out.pairs[i][j], Pair::Differ(_)));
        out.rigs[i] = if alone {
            DomainTag {
                tag: Some("FREE".into()),
                tip: Some(out.own_tip(i, others())),
                domain: Domain::Own,
            }
        } else {
            DomainTag {
                tag: Some("UNKNOWN".into()),
                tip: Some(out.cant_tell_tip(i, others())),
                domain: Domain::CantTell,
            }
        };
    }
    out
}

impl ClockDomains {
    /// Whether rigs `a` and `b` (indices into the list given) count samples on one clock,
    /// for a measurement across the two. Panics on an index outside the list.
    pub fn same_domain(&self, a: usize, b: usize) -> SameDomain {
        if a == b {
            return SameDomain::Yes;
        }
        for i in [a, b] {
            if self.rigs[i].domain == Domain::NoSession {
                return SameDomain::CantTell {
                    reason: format!("{} has no audio session", self.names[i]),
                };
            }
        }
        if let (Domain::Group { index: x, .. }, Domain::Group { index: y, .. }) =
            (self.rigs[a].domain, self.rigs[b].domain)
            && x == y
        {
            return SameDomain::Yes;
        }
        match &self.pairs[a][b] {
            Pair::Differ(_) => SameDomain::No {
                reason: self.sentence(a, b),
            },
            Pair::CantTell(why) => SameDomain::CantTell {
                reason: why.join(" · "),
            },
            // Agreeing rigs outside one group: the chain they are in contradicts itself.
            Pair::Agree { .. } | Pair::Skip => SameDomain::CantTell {
                reason: self.broken[a]
                    .clone()
                    .or_else(|| self.broken[b].clone())
                    .unwrap_or_else(|| "nothing compares the two clocks".into()),
            },
        }
    }

    /// How rigs `a` and `b` differ, as a sentence: *cm4 runs +6.0 ppm against pupu (by each
    /// rig's system clock)*.
    fn sentence(&self, a: usize, b: usize) -> String {
        let (an, bn) = (&self.names[a], &self.names[b]);
        match &self.pairs[a][b] {
            Pair::Differ(Differ::Rates) => {
                format!("{an} runs at {}, {bn} at {}", self.rate(a), self.rate(b))
            }
            Pair::Differ(Differ::Cross { ppm, over }) => format!(
                "{an} runs {} ppm against {bn} over {} s, measured between the rigs",
                format::signed(*ppm, 1),
                format::fixed(*over, 0)
            ),
            Pair::Differ(Differ::OwnClock { who, ppm }) => format!(
                "{who}'s interface runs {} ppm off its system clock",
                format::signed(*ppm, 1)
            ),
            Pair::Differ(Differ::BySystemClocks { ppm }) => format!(
                "{an} runs {} ppm against {bn} (by each rig's system clock)",
                format::signed(*ppm, 1)
            ),
            _ => format!("{an} and {bn} are not shown to differ"),
        }
    }

    fn rate(&self, i: usize) -> String {
        self.rates[i].map_or_else(|| format::NO_VALUE.to_owned(), rate_text)
    }

    /// A group member's tooltip: the other rigs and the basis.
    fn group_tip(&self, i: usize, members: &[usize], basis: Basis) -> String {
        let others: Vec<usize> = members.iter().copied().filter(|&j| j != i).collect();
        let names = list(others.iter().map(|&j| self.names[j].as_str()));
        let tol = format::fixed(AGREE_PPM, 0);
        match basis {
            Basis::BetweenRigs => {
                // One other rig: the figure the verdict rests on reads beside it.
                let figure = match (others.as_slice(), &self.pairs[i][others[0]]) {
                    (
                        [_],
                        Pair::Agree {
                            figure: Some((ppm, span)),
                            ..
                        },
                    ) => format!(
                        " ({} ppm over the last {} s)",
                        format::signed(*ppm, 1),
                        format::fixed(*span, 0)
                    ),
                    _ => String::new(),
                };
                format!(
                    "clocks agree with {names}: measured between the rigs, within {tol} \
                     ppm{figure}; a lock is not proven below {tol} ppm"
                )
            }
            Basis::SystemClocks => {
                let mixed = members.iter().any(|&a| {
                    members
                        .iter()
                        .any(|&b| matches!(self.pairs[a][b], Pair::Agree { direct: true, .. }))
                });
                let by = if mixed {
                    "by each rig's system clock, or measured between the rigs"
                } else {
                    "by each rig's system clock"
                };
                format!(
                    "rates agree with {names} within {tol} ppm ({by}); two free clocks can \
                     meet that by chance"
                )
            }
        }
    }

    /// The tooltip of a rig that differs from every other: the rates first (rigs at one
    /// rate named together), then each other difference.
    fn own_tip(&self, i: usize, others: impl Iterator<Item = usize>) -> String {
        let mut by_rate: Vec<(u32, Vec<&str>)> = Vec::new();
        let mut rest = Vec::new();
        for j in others {
            match (&self.pairs[i][j], self.rates[j]) {
                (Pair::Differ(Differ::Rates), Some(hz)) => {
                    match by_rate.iter_mut().find(|(r, _)| *r == hz) {
                        Some((_, names)) => names.push(&self.names[j]),
                        None => by_rate.push((hz, vec![&self.names[j]])),
                    }
                }
                _ => push_new(&mut rest, self.sentence(i, j)),
            }
        }
        let mut parts = Vec::new();
        if !by_rate.is_empty() {
            let theirs: Vec<String> = by_rate
                .into_iter()
                .map(|(hz, names)| format!("{} at {}", list(names.into_iter()), rate_text(hz)))
                .collect();
            parts.push(format!("{} here; {}", self.rate(i), theirs.join(", ")));
        }
        parts.extend(rest);
        parts.join(" · ")
    }

    /// The tooltip of a rig the evidence cannot place, in the words the app's other texts use
    /// for it: *can't tell*, then each missing fact once.
    fn cant_tell_tip(&self, i: usize, others: impl Iterator<Item = usize>) -> String {
        let mut parts = Vec::new();
        if let Some(why) = &self.broken[i] {
            parts.push(why.clone());
        }
        for j in others {
            if let Pair::CantTell(why) = &self.pairs[i][j] {
                for w in why {
                    push_new(&mut parts, w.clone());
                }
            }
        }
        format!("can't tell: {}", parts.join(" · "))
    }
}

/// What the evidence says of rigs `i` and `j`, strongest evidence first.
fn pair(rigs: &[RigClockFacts<'_>], i: usize, j: usize) -> Pair {
    let (a, b) = (&rigs[i], &rigs[j]);
    let (Some(ra), Some(rb)) = (a.rate_hz, b.rate_hz) else {
        return Pair::Skip;
    };
    if ra != rb {
        return Pair::Differ(Differ::Rates);
    }
    // Each daemon's verdict on the other, as seen from `i`. One that differs outweighs one
    // that cannot tell, which outweighs one that agrees: the two daemons watch different
    // streams, and a difference seen on either is a difference.
    let verdicts = [verdict(a, b), verdict(b, a).map(|p| p.mirrored())];
    let rank = |p: &Pair| match p {
        Pair::Differ(_) => 0,
        Pair::CantTell(_) => 1,
        _ => 2,
    };
    if let Some(v) = verdicts.into_iter().flatten().min_by_key(rank) {
        return v;
    }
    match (own_rate(a), own_rate(b)) {
        (Ok(pa), Ok(pb)) if (pa - pb).abs() <= AGREE_PPM => Pair::Agree {
            direct: false,
            figure: None,
        },
        (Ok(pa), Ok(pb)) => Pair::Differ(Differ::BySystemClocks { ppm: pa - pb }),
        (ea, eb) => Pair::CantTell(ea.err().into_iter().chain(eb.err()).collect()),
    }
}

/// `by`'s daemon on the clocks between it and `other`; `None` when it takes no input from
/// that rig or is still measuring. Figures are `by` against `other`.
fn verdict(by: &RigClockFacts<'_>, other: &RigClockFacts<'_>) -> Option<Pair> {
    let id = other.id?;
    let clock: &PeerClock = &by.between.iter().find(|r| r.rig == id)?.clock;
    // A verdict names one of its two rigs: the remote one by its id, else the measuring one.
    let name = |rig: RigId| if rig == id { other.name } else { by.name };
    // The wire figure is the remote rig against the measuring one.
    Some(match &clock.relation {
        PeerRelation::Measuring => return None,
        PeerRelation::Agree => Pair::Agree {
            direct: true,
            figure: clock.ppm.zip(clock.span).map(|(p, s)| (-p, s.0)),
        },
        PeerRelation::NotLocked { reason } => Pair::Differ(match reason {
            NotLockedReason::CrossRate { ppm, over } => Differ::Cross {
                ppm: -ppm,
                over: over.0,
            },
            NotLockedReason::OwnClock { rig, ppm } => Differ::OwnClock {
                who: name(*rig).to_owned(),
                ppm: *ppm,
            },
        }),
        PeerRelation::CantTell { reason } => Pair::CantTell(vec![match reason {
            CantTellReason::NoAudio => format!("no audio between {} and {}", by.name, other.name),
            CantTellReason::SystemClockUnsynced { rig } => {
                format!("{}'s system clock is not synced", name(*rig))
            }
            CantTellReason::SystemClockNotStated { rig } => {
                format!("{}'s system clock does not say", name(*rig))
            }
            CantTellReason::NotAgainstSystemClock { rig } => format!(
                "{}'s interface is measured against its own crystal",
                name(*rig)
            ),
            CantTellReason::NoCaptureStamps => {
                format!("{} stamps no capture times", other.name)
            }
            CantTellReason::ClocksDisagree { by: d } => format!(
                "the system clocks of {} and {} disagree by {} ms",
                by.name,
                other.name,
                format::fixed(d.0.abs() * 1e3, 1)
            ),
        }]),
    })
}

/// A rig's sample clock against true time, ppm, as far as its system clock can carry it;
/// else the fact that is missing.
///
/// The figure is against the rig's system clock, so it compares with another rig's only
/// while both system clocks run at one rate over the window measured. The OS states no
/// rate error, only a bound on the offset, and the kernel widens that bound by 500 µs every
/// second it goes without an update from the discipline daemon. A bound under what one
/// window adds (30 ms for a minute) therefore shows the clock was steered within about the
/// window; a wider one allows that it ran on its crystal for all of it (629.5 ms: up to 21
/// minutes unsteered), which makes the figure one against that crystal: no evidence
/// between rigs. The bound also holds the distance to the time source, so a rig synced to a
/// far or poor source is refused as well; that errs to *can't tell*.
fn own_rate(r: &RigClockFacts<'_>) -> Result<f64, String> {
    let name = r.name;
    let (ppm, span) = match r.clocks.capture.as_ref().map(|c| c.rate) {
        None => return Err(format!("{name} reports no sample clock")),
        Some(SampleClock::Measuring) => {
            return Err(format!(
                "{name}'s sample clock is still being measured (less than a minute of audio)"
            ));
        }
        Some(SampleClock::Synthetic) => {
            return Err(format!("{name} plays a recording: no device clock"));
        }
        Some(SampleClock::Measured {
            against: ClockBasis::HostCrystal,
            ..
        }) => {
            return Err(format!(
                "{name}'s interface is measured against its own crystal"
            ));
        }
        Some(SampleClock::Measured {
            ppm,
            span,
            against: ClockBasis::SystemClock,
        }) => (ppm, span.0),
    };
    match r.clocks.system {
        SystemClock::Unsynced { .. } => Err(format!("{name}'s system clock is not synced")),
        SystemClock::NotStated => Err(format!("{name}'s system clock does not say")),
        SystemClock::Synced { maxerror_us }
            if f64::from(maxerror_us) * 1e-6 > span * BOUND_GROWTH =>
        {
            Err(format!(
                "{name}'s system clock is {}: too loose to compare rates between rigs",
                system_clock_text(&r.clocks.system)
            ))
        }
        SystemClock::Synced { .. } => Ok(ppm),
    }
}

/// `of` split into the sets `joined` connects, each in `of`'s order, by first member.
fn components(of: &[usize], joined: impl Fn(usize, usize) -> bool) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut seen = vec![false; of.len()];
    for start in 0..of.len() {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let mut part = vec![start];
        let mut next = 0;
        while next < part.len() {
            let at = part[next];
            next += 1;
            for k in 0..of.len() {
                if !seen[k] && joined(of[at], of[k]) {
                    seen[k] = true;
                    part.push(k);
                }
            }
        }
        part.sort_unstable();
        out.push(part.into_iter().map(|k| of[k]).collect());
    }
    out
}

/// The first two rigs of `set` shown to differ.
fn first_differing(pairs: &[Vec<Pair>], set: &[usize]) -> Option<(usize, usize)> {
    set.iter().enumerate().find_map(|(k, &a)| {
        set[k + 1..]
            .iter()
            .find(|&&b| matches!(pairs[a][b], Pair::Differ(_)))
            .map(|&b| (a, b))
    })
}

/// A group's letter: `A`… in list order; past `Z` its number.
fn group_letter(index: usize) -> String {
    match u8::try_from(index) {
        Ok(i) if i < 26 => char::from(b'A' + i).to_string(),
        _ => (index + 1).to_string(),
    }
}

/// `a`, `a and b`, `a, b and c`.
fn list<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let names: Vec<&str> = names.collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

fn push_new(to: &mut Vec<String>, s: String) {
    if !to.contains(&s) {
        to.push(s);
    }
}

#[cfg(test)]
mod tests;
