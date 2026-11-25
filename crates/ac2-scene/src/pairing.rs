//! The words of pairing two rigs on the app's Connection page: the *Paired rigs* rows, the
//! pick of a rig to pair with, the refusals and the toasts. A pairing has four halves, two
//! per direction: a rig takes another's inputs when it lists that rig (`rig.pair`) and
//! that rig has admitted it (`auth.admit_rig`). Each half is said as the daemons' state
//! shows it, and a half the app cannot see is said as not known, never as missing.

/// The Connection page's section of the rigs paired with the rig shown.
pub const SECTION: &str = "Paired rigs";

/// Whose the pairing is, beside the section's title: each rig's daemon keeps its half.
pub const WHOSE: &str = "both rigs — all clients";

/// The row that pairs the rig shown with another rig the app holds.
pub const PAIR_WITH: &str = "Pair with rig… (Enter)";

/// *Pair with rig…* with no other rig held: pairing needs both daemons.
pub const NO_OTHER_RIG: &str = "The app holds no other rig: connect to it first (Connect a rig… \
     above), then pair here";

/// *Pair with rig…* with every rig held already paired both ways.
pub const ALL_PAIRED: &str = "Every rig the app holds is paired with this rig both ways";

/// The pick's heading, above the rigs offered.
pub const PICK_HEADING: &str = "Pair this rig with (Enter pairs both ways · Esc closes):";

/// One half of a pairing as the daemons' state shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Half {
    Yes,
    No,
    /// The app cannot see this half: the listing or admitting rig is `side`, and `why` its
    /// state is not seen.
    Unknown {
        side: Side,
        why: Unseen,
    },
    /// The entry is there, but the key it pins cannot be checked against the one its rig
    /// serves under now: that rig is `side`, and `why` its key is not seen. An entry under
    /// a stale key is refused, so the half is not counted in place.
    Unchecked {
        side: Side,
        why: Unseen,
    },
}

/// Which rig's state a half that cannot be told waits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The rig shown.
    Here,
    /// The other rig.
    There,
}

/// Why the app cannot see a rig's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unseen {
    /// The app does not hold that rig: its daemon's state is not here.
    NotHeld,
    /// The app holds that rig but is not connected to it now: what it showed is stale.
    NotConnected,
    /// That rig has not answered yet.
    Asking,
    /// That rig did not answer `server.info` (the request failed).
    Unanswered,
    /// That rig serves only its own computer: its admitted rigs are not listed to the app.
    NetworkOff,
}

/// The four halves between the rig shown (*this rig*) and another rig.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Halves {
    /// This rig lists the other rig (it takes from it once admitted there).
    pub here_lists: Half,
    /// The other rig has admitted this rig.
    pub there_admits: Half,
    /// The other rig lists this rig.
    pub there_lists: Half,
    /// This rig has admitted the other rig.
    pub here_admits: Half,
}

impl Halves {
    /// All four halves seen and in place.
    pub fn both_ways(&self) -> bool {
        [
            &self.here_lists,
            &self.there_admits,
            &self.there_lists,
            &self.here_admits,
        ]
        .iter()
        .all(|h| **h == Half::Yes)
    }

    /// Some half is seen missing.
    pub fn missing(&self) -> bool {
        [
            &self.here_lists,
            &self.there_admits,
            &self.there_lists,
            &self.here_admits,
        ]
        .iter()
        .any(|h| **h == Half::No)
    }

    /// Some half is seen in place: something already links the two rigs.
    pub fn any(&self) -> bool {
        [
            &self.here_lists,
            &self.there_admits,
            &self.there_lists,
            &self.here_admits,
        ]
        .iter()
        .any(|h| **h == Half::Yes)
    }

    /// The halves that cannot be told (unseen, or there but unchecked), with whose state
    /// each waits on.
    fn unknown(&self) -> impl Iterator<Item = (Side, Unseen)> + '_ {
        [
            &self.here_lists,
            &self.there_admits,
            &self.there_lists,
            &self.here_admits,
        ]
        .into_iter()
        .filter_map(|h| match h {
            Half::Unknown { side, why } | Half::Unchecked { side, why } => Some((*side, *why)),
            _ => None,
        })
    }

    /// Whether `side`'s admissions are not seen.
    pub fn admissions_unseen(&self, side: Side) -> Option<Unseen> {
        let h = match side {
            Side::Here => &self.here_admits,
            Side::There => &self.there_admits,
        };
        match h {
            Half::Unknown { why, .. } => Some(*why),
            _ => None,
        }
    }
}

/// How one direction of a pairing stands: a rig takes the other's inputs when it lists that
/// rig and that rig has admitted it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    /// Both halves seen in place.
    Done,
    /// A half seen missing.
    Missing,
    /// Neither: a half cannot be told.
    Open,
}

fn way(a: &Half, b: &Half) -> Way {
    if *a == Half::No || *b == Half::No {
        Way::Missing
    } else if *a == Half::Yes && *b == Half::Yes {
        Way::Done
    } else {
        Way::Open
    }
}

/// What cannot be told among `halves`: plain `can't tell` when this rig's state is
/// wanting, `can't tell its side` when only the other rig's is.
fn cant(halves: &[&Half]) -> &'static str {
    let here = halves.iter().any(|h| {
        matches!(
            h,
            Half::Unknown {
                side: Side::Here,
                ..
            } | Half::Unchecked {
                side: Side::Here,
                ..
            }
        )
    });
    if here {
        "can't tell"
    } else {
        "can't tell its side"
    }
}

/// A *Paired rigs* row: the rig's name, then [`row_state`].
pub fn row(rig: &str, h: &Halves) -> String {
    format!("{rig} · {}", row_state(h))
}

/// A *Paired rigs* row's state, short enough to stand beside a long rig name; each half in
/// words is the row's [`tip`]. `paired both ways`; one direction in place and what the
/// other does (`this rig takes its inputs · not the other way`, `… · other way: can't
/// tell`); neither in place: `not paired · 1 of 4 halves in place`, or with a direction that
/// cannot be told how many halves are in place and what cannot be told. A half that cannot
/// be told is never counted missing: `can't tell` when this rig's state is wanting, `can't
/// tell its side` when only the other rig's is. Nothing seen missing and nothing in place
/// both ways: this rig's entries whose key cannot be checked, and what cannot be told.
pub fn row_state(h: &Halves) -> String {
    let all = [
        &h.here_lists,
        &h.there_admits,
        &h.there_lists,
        &h.here_admits,
    ];
    let takes = way(&h.here_lists, &h.there_admits);
    let gives = way(&h.there_lists, &h.here_admits);
    let other = |done: &str, a: &Half, b: &Half, rest: Way| match rest {
        Way::Missing => format!("{done} · not the other way"),
        _ => format!("{done} · other way: {}", cant(&[a, b])),
    };
    const TAKES: &str = "this rig takes its inputs";
    const GIVES: &str = "it takes this rig's inputs";
    let unknown = |x: &Half| matches!(x, Half::Unknown { .. } | Half::Unchecked { .. });
    let unchecked = |x: &Half| matches!(x, Half::Unchecked { .. });
    let count = || {
        let yes = all.iter().filter(|x| ***x == Half::Yes).count();
        let u = all.iter().filter(|x| unchecked(x)).count();
        if u == 0 {
            format!("{yes} of 4 halves in place")
        } else {
            format!("{yes} of 4 halves in place, {u} unchecked")
        }
    };
    match (takes, gives) {
        (Way::Done, Way::Done) => "paired both ways".into(),
        (Way::Done, rest) => other(TAKES, &h.there_lists, &h.here_admits, rest),
        (rest, Way::Done) => other(GIVES, &h.here_lists, &h.there_admits, rest),
        (Way::Missing, Way::Missing) if all.iter().any(|x| unknown(x)) => {
            format!("not paired · {} · {}", count(), cant(&all))
        }
        (Way::Missing, Way::Missing) => format!("not paired · {}", count()),
        (Way::Missing, Way::Open) | (Way::Open, Way::Missing) => {
            format!("{} · {}", count(), cant(&all))
        }
        (Way::Open, Way::Open) => {
            let mut parts: Vec<&str> = Vec::new();
            match (unchecked(&h.here_lists), unchecked(&h.here_admits)) {
                (true, true) => parts.push("listed and admitted by this rig"),
                (true, false) => parts.push("listed by this rig"),
                (false, true) => parts.push("admitted by this rig"),
                (false, false) => {}
            }
            if unchecked(&h.there_lists) {
                parts.push("lists this rig");
            }
            if unchecked(&h.there_admits) {
                parts.push("has admitted this rig");
            }
            parts.push(cant(&all));
            parts.join(" · ")
        }
    }
}

/// Why `side`'s state is not seen; `rig` is the other rig's name.
fn unseen(rig: &str, side: Side, u: Unseen) -> String {
    let who = match side {
        Side::Here => "this rig",
        Side::There => rig,
    };
    match u {
        Unseen::NotHeld => format!("the app does not hold {who}: connect to it to see"),
        Unseen::NotConnected => format!("the app is not connected to {who} now"),
        Unseen::Asking => format!("asking {who}…"),
        Unseen::Unanswered => format!("{who} did not answer the app"),
        Unseen::NetworkOff => format!("{who}'s network is off"),
    }
}

/// Enter on a row that is not paired both ways although no half is seen missing: what
/// cannot be told, and why.
pub fn cant_tell(rig: &str, h: &Halves) -> String {
    let mut why: Vec<String> = Vec::new();
    for (side, u) in h.unknown() {
        let w = unseen(rig, side, u);
        if !why.contains(&w) {
            why.push(w);
        }
    }
    format!(
        "can't tell whether {rig} is paired both ways: {}",
        why.join("; ")
    )
}

/// A *Paired rigs* row's hover: each half in words, then the clocks as this rig's daemon
/// hears them in `rig`'s levels (`clocks`: [`crate::standing::evidence_line`], `None` when it
/// lists no standing relation with it), then what the keys do there. `reachable`: the app
/// is connected to both rigs, so Enter can pair the missing halves.
pub fn tip(rig: &str, h: &Halves, clocks: Option<&str>, reachable: bool) -> String {
    let line = |half: &Half, yes: String, no: String, what: String| match half {
        Half::Yes => yes,
        Half::No => no,
        Half::Unknown { side, why } => {
            format!("  can't tell whether {what} ({})", unseen(rig, *side, *why))
        }
        Half::Unchecked { side, why } => {
            format!("{yes} (can't check its key: {})", unseen(rig, *side, *why))
        }
    };
    let mut lines = vec![
        format!("This rig takes {rig}'s inputs when both of these hold:"),
        line(
            &h.here_lists,
            format!("  this rig lists {rig} as paired"),
            format!("  this rig does not list {rig}"),
            format!("this rig lists {rig}"),
        ),
        line(
            &h.there_admits,
            format!("  {rig} has admitted this rig"),
            format!("  {rig} has not admitted this rig"),
            format!("{rig} has admitted this rig"),
        ),
        format!("{rig} takes this rig's inputs when both of these hold:"),
        line(
            &h.there_lists,
            format!("  {rig} lists this rig as paired"),
            format!("  {rig} does not list this rig"),
            format!("{rig} lists this rig"),
        ),
        line(
            &h.here_admits,
            format!("  this rig has admitted {rig}"),
            format!("  this rig has not admitted {rig}"),
            format!("this rig has admitted {rig}"),
        ),
    ];
    lines.extend(clocks.map(str::to_owned));
    lines.push(if h.both_ways() {
        "Delete twice unpairs both ways".into()
    } else if h.missing() && reachable {
        "Enter pairs the missing halves · Delete twice unpairs".into()
    } else {
        "Delete twice unpairs".into()
    });
    lines.join("\n")
}

/// A rig offered by *Pair with rig…*: `ketunkolo`, or `ketunkolo · pairs the missing
/// halves` when some half is already in place.
pub fn offer_row(rig: &str, h: &Halves) -> String {
    if h.any() {
        format!("{rig} · pairs the missing halves")
    } else {
        rig.to_owned()
    }
}

/// Enter on a half-paired rig the app does not hold: both daemons are needed.
pub fn not_held(rig: &str) -> String {
    format!("the app does not hold {rig}: connect to it first, then Enter pairs the missing halves")
}

/// Enter on a rig paired both ways: nothing to pair; what Delete does.
pub fn unpair_hint(rig: &str) -> String {
    format!("{rig} is paired both ways · Delete twice unpairs it")
}

/// Why rig `rig` cannot be paired with now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairRefusal {
    /// It has not answered `server.info` yet.
    Asking,
    /// The app cannot ask it (`server.info` failed).
    Unanswered(String),
    /// It listens on its own computer only: no other rig reaches it.
    LocalOnly,
    /// It already authorizes an app under the name the other rig would be admitted as.
    NameTaken(String),
    /// Rig `0` (by its name), which would list it, already lists another rig under the
    /// same name: a rig is listed by its name, so the two cannot both be.
    NamePaired(String),
    /// The app is not connected to it now: which halves it holds is not known, and its
    /// half could not be sent.
    NotConnected,
}

/// The words of a [`PairRefusal`] of rig `rig`: why, and what to do.
pub fn refusal(rig: &str, why: &PairRefusal) -> String {
    match why {
        PairRefusal::Asking => format!("asking {rig} for its key…"),
        PairRefusal::Unanswered(e) => format!("{rig}: cannot ask it ({e})"),
        PairRefusal::LocalOnly => format!(
            "{rig} serves only its own computer: turn its network on (Settings › Connection) \
             to pair it"
        ),
        PairRefusal::NameTaken(n) => {
            format!("{rig} already authorizes an app named {n}: revoke it there first (Connection)")
        }
        PairRefusal::NamePaired(taker) => format!(
            "{taker} already pairs another rig named {rig}: unpair that one on {taker} first \
             (Settings › Connection)"
        ),
        PairRefusal::NotConnected => {
            format!("the app is not connected to {rig} now: its paired rigs are not known")
        }
    }
}

/// The toast once rig `lister` lists rig `peer` as paired.
pub fn listed_toast(lister: &str, peer: &str) -> String {
    format!("{lister} lists {peer} as paired")
}

/// The fault once rig `lister` refused to list `peer` (the daemon's words follow).
pub fn not_listed(lister: &str, peer: &str) -> String {
    format!("{lister} did not list {peer} as paired")
}

/// The fault once rig `admitter` refused to admit `rig` (the daemon's words follow).
pub fn not_admitted(admitter: &str, rig: &str) -> String {
    format!("{admitter} did not admit {rig}")
}

/// The toast once rig `lister` no longer lists `peer`.
pub fn unlisted_toast(lister: &str, peer: &str) -> String {
    format!("{lister} no longer lists {peer} as paired")
}

/// The fault once rig `lister` refused to drop `peer` (the daemon's words follow).
pub fn not_unlisted(lister: &str, peer: &str) -> String {
    format!("{lister} still lists {peer}")
}

/// The toast once rig `admitter` revoked rig `rig`'s admission.
pub fn revoked_toast(admitter: &str, rig: &str) -> String {
    format!("{admitter} no longer admits {rig}")
}

/// The fault once rig `admitter` refused to revoke `rig` (the daemon's words follow).
pub fn not_revoked(admitter: &str, rig: &str) -> String {
    format!("{admitter} still admits {rig}")
}

/// Why the app cannot reach rig `rig`'s daemon to unpair its side.
fn unreached(rig: &str, why: Unseen) -> String {
    match why {
        Unseen::NotConnected => format!("the app is not connected to {rig} now"),
        _ => format!("the app does not hold {rig}"),
    }
}

/// Whether `side` admits the other rig, as a clause; `rig` is the other rig's name.
fn admits_clause(rig: &str, side: Side, why: Unseen) -> String {
    let (who, whom) = match side {
        Side::Here => ("this rig".to_owned(), rig.to_owned()),
        Side::There => (rig.to_owned(), "this rig".to_owned()),
    };
    format!(
        "can't tell whether {who} admits {whom} ({})",
        unseen(rig, side, why)
    )
}

/// The first Delete on a *Paired rigs* row. `unreached`: why the app cannot reach the
/// other rig, so only this rig's side is unpaired; `None`: both sides are. A stream there
/// cannot be seen, so the confirm says what revoking its admission here does to it. An
/// admission on a side whose admitted rigs are not seen is not revoked, so the confirm
/// promises no more than is sent.
pub fn unpair_confirm(rig: &str, unreached_why: Option<Unseen>, h: &Halves) -> String {
    let sides: &[Side] = match unreached_why {
        None => &[Side::Here, Side::There],
        Some(_) => &[Side::Here],
    };
    let unseen: Vec<String> = sides
        .iter()
        .filter_map(|s| {
            h.admissions_unseen(*s)
                .map(|why| admits_clause(rig, *s, why))
        })
        .collect();
    let stays = if unseen.is_empty() {
        String::new()
    } else {
        format!(
            ". {}: an admission not seen stays",
            upper_first(&unseen.join("; "))
        )
    };
    match unreached_why {
        None if unseen.is_empty() => format!("Delete again unpairs {rig} both ways"),
        None => format!("Delete again unpairs {rig} as far as the app sees{stays}"),
        Some(u) => format!(
            "Delete again unpairs this rig's side only ({}; its side stays). Can't tell \
             whether {rig}'s stream takes this rig's inputs: if it does, they stop{stays}",
            unreached(rig, u)
        ),
    }
}

/// After this rig's side was unpaired from a rig the app cannot reach: what remains.
pub fn unpair_rest(rig: &str, why: Unseen) -> String {
    format!(
        "{rig}'s side is unchanged ({}): connect to {rig}, show it and unpair this rig there \
         to finish",
        unreached(rig, why)
    )
}

/// Unpairing while `side`'s admitted rigs cannot be seen: its admission of the other rig,
/// if any, was not revoked.
pub fn unpair_admission_unseen(rig: &str, side: Side, why: Unseen) -> String {
    let there = match side {
        Side::Here => "",
        Side::There => " there",
    };
    format!(
        "{}: if it does, that stays; revoke it{there} under Authorized clients once seen",
        upper_first(&admits_clause(rig, side, why))
    )
}

fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Unpairing refused while the stream of rig `taker` takes inputs `inputs` from the
/// other rig: those go first.
pub fn unpair_inputs(taker: &str, inputs: &[String]) -> String {
    format!(
        "{} on {taker} come from that rig: remove them first (Settings › Audio I/O)",
        inputs.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn halves(a: Half, b: Half, c: Half, d: Half) -> Halves {
        Halves {
            here_lists: a,
            there_admits: b,
            there_lists: c,
            here_admits: d,
        }
    }

    #[test]
    fn a_row_says_both_ways_or_what_is_missing() {
        use Half::*;
        let all = halves(Yes, Yes, Yes, Yes);
        assert_eq!(row("ketunkolo", &all), "ketunkolo · paired both ways");
        assert!(tip("ketunkolo", &all, None, true).ends_with("Delete twice unpairs both ways"));
        let one = halves(Yes, No, Yes, Yes);
        assert_eq!(
            row("ketunkolo", &one),
            "ketunkolo · it takes this rig's inputs · not the other way"
        );
        assert!(
            tip("ketunkolo", &one, None, true).contains("  ketunkolo has not admitted this rig"),
            "{}",
            tip("ketunkolo", &one, None, true)
        );
        // Enter is promised only where it can pair the missing halves.
        assert!(
            tip("ketunkolo", &one, None, true)
                .ends_with("Enter pairs the missing halves · Delete twice unpairs")
        );
        assert!(tip("ketunkolo", &one, None, false).ends_with("\nDelete twice unpairs"));
        // The clocks the levels show read between the halves and the keys.
        let clocks = "clocks agree · +0.12 ppm over 60 s · marks for 5 min 12 s · measured \
                      from the rigs' level frames, no audio streamed";
        assert!(
            tip("ketunkolo", &all, Some(clocks), true).ends_with(&format!(
                "  this rig has admitted ketunkolo\n{clocks}\nDelete twice unpairs both ways"
            ))
        );
        let two = halves(No, Yes, Yes, No);
        assert_eq!(
            row("ketunkolo", &two),
            "ketunkolo · not paired · 2 of 4 halves in place"
        );
        assert_eq!(
            offer_row("ketunkolo", &two),
            "ketunkolo · pairs the missing halves"
        );
        assert_eq!(offer_row("ketunkolo", &halves(No, No, No, No)), "ketunkolo");
    }

    /// A half the app cannot see is said as not known, never as missing.
    #[test]
    fn an_unseen_half_is_not_claimed() {
        use Half::*;
        let there = Unknown {
            side: Side::There,
            why: Unseen::NotHeld,
        };
        let h = halves(Yes, there, there, Yes);
        assert_eq!(row("ketunkolo", &h), "ketunkolo · can't tell its side");
        let t = tip("ketunkolo", &h, None, false);
        assert!(t.ends_with("\nDelete twice unpairs"), "{t}");
        assert!(
            t.contains(
                "can't tell whether ketunkolo has admitted this rig (the app does not hold \
                 ketunkolo: connect to it to see)"
            ),
            "{t}"
        );
        assert!(!t.contains("has not admitted"), "{t}");
        assert!(!h.missing() && !h.both_ways() && h.any());
        assert_eq!(
            cant_tell("ketunkolo", &h),
            "can't tell whether ketunkolo is paired both ways: the app does not hold \
             ketunkolo: connect to it to see"
        );
    }

    /// A half of this rig that cannot be told is this rig's, not the other rig's.
    #[test]
    fn this_rigs_unseen_half_is_said_as_this_rigs() {
        use Half::*;
        let here = Unknown {
            side: Side::Here,
            why: Unseen::NetworkOff,
        };
        let h = halves(Yes, Yes, Yes, here);
        assert_eq!(
            row("ketunkolo", &h),
            "ketunkolo · this rig takes its inputs · other way: can't tell"
        );
        let t = tip("ketunkolo", &h, None, true);
        assert!(t.ends_with("\nDelete twice unpairs"), "{t}");
        assert!(
            t.contains(
                "can't tell whether this rig has admitted ketunkolo (this rig's network is off)"
            ),
            "{t}"
        );
        assert!(!t.contains("ketunkolo's network"), "{t}");
        assert_eq!(
            cant_tell("ketunkolo", &h),
            "can't tell whether ketunkolo is paired both ways: this rig's network is off"
        );
        // Its admissions unseen, the confirm does not promise both ways.
        assert_eq!(
            unpair_confirm("ketunkolo", None, &h),
            "Delete again unpairs ketunkolo as far as the app sees. Can't tell whether this \
             rig admits ketunkolo (this rig's network is off): an admission not seen stays"
        );
    }

    /// An entry in this rig's own state whose rig's key cannot be checked is said as there,
    /// with the key unchecked: never as unknown, never as in place.
    #[test]
    fn an_entry_whose_key_is_unchecked_is_said_as_there() {
        use Half::*;
        let key = Unchecked {
            side: Side::There,
            why: Unseen::NotHeld,
        };
        let unseen = Unknown {
            side: Side::There,
            why: Unseen::NotHeld,
        };
        let h = halves(key, unseen, unseen, key);
        assert!(!h.missing() && !h.both_ways());
        assert_eq!(
            row("ketunkolo", &h),
            "ketunkolo · listed and admitted by this rig · can't tell its side"
        );
        assert_eq!(
            row("ketunkolo", &halves(key, unseen, unseen, No)),
            "ketunkolo · 0 of 4 halves in place, 1 unchecked · can't tell its side"
        );
        assert_eq!(
            row("ketunkolo", &halves(key, unseen, unseen, unseen)),
            "ketunkolo · listed by this rig · can't tell its side"
        );
        let t = tip("ketunkolo", &h, None, false);
        assert!(
            t.contains(
                "  this rig lists ketunkolo as paired (can't check its key: the app does not \
                 hold ketunkolo: connect to it to see)"
            ),
            "{t}"
        );
        assert!(!t.contains("can't tell whether this rig lists"), "{t}");
    }

    /// The row says how the two directions stand in a few words, every class of halves:
    /// the rig's name and state together stay short beside a long name.
    #[test]
    fn a_row_says_how_the_directions_stand() {
        use Half::*;
        let there = Unknown {
            side: Side::There,
            why: Unseen::NotHeld,
        };
        let here = Unknown {
            side: Side::Here,
            why: Unseen::NetworkOff,
        };
        let key = Unchecked {
            side: Side::There,
            why: Unseen::NotConnected,
        };
        let cases = [
            (halves(Yes, Yes, Yes, Yes), "paired both ways"),
            // One direction in place, the other seen missing.
            (
                halves(Yes, Yes, No, Yes),
                "this rig takes its inputs · not the other way",
            ),
            (
                halves(Yes, Yes, No, No),
                "this rig takes its inputs · not the other way",
            ),
            (
                halves(No, Yes, Yes, Yes),
                "it takes this rig's inputs · not the other way",
            ),
            // One direction in place, the other cannot be told: whose state is wanting.
            (
                halves(Yes, Yes, there, Yes),
                "this rig takes its inputs · other way: can't tell its side",
            ),
            (
                halves(Yes, Yes, Yes, here),
                "this rig takes its inputs · other way: can't tell",
            ),
            (
                halves(Yes, there, Yes, Yes),
                "it takes this rig's inputs · other way: can't tell its side",
            ),
            (
                halves(key, Yes, Yes, Yes),
                "it takes this rig's inputs · other way: can't tell its side",
            ),
            // Neither direction in place.
            (
                halves(Yes, No, No, No),
                "not paired · 1 of 4 halves in place",
            ),
            (
                halves(No, No, No, No),
                "not paired · 0 of 4 halves in place",
            ),
            (
                halves(Yes, No, there, No),
                "not paired · 1 of 4 halves in place · can't tell its side",
            ),
            (
                halves(Yes, No, there, here),
                "1 of 4 halves in place · can't tell",
            ),
            (
                halves(Yes, No, there, there),
                "1 of 4 halves in place · can't tell its side",
            ),
            // Nothing seen missing, nothing in place both ways.
            (halves(Yes, there, there, Yes), "can't tell its side"),
            (
                halves(Yes, Yes, here, here),
                "this rig takes its inputs · other way: can't tell",
            ),
            (halves(here, there, there, here), "can't tell"),
            (
                halves(key, there, there, key),
                "listed and admitted by this rig · can't tell its side",
            ),
            (
                halves(Yes, key, key, Yes),
                "lists this rig · has admitted this rig · can't tell its side",
            ),
        ];
        for (h, want) in cases {
            assert_eq!(row_state(&h), want, "{h:?}");
            // Never more than a 25-character name leaves room for on the page.
            assert!(want.chars().count() <= 60, "{want}");
            assert_eq!(
                row("ac2 on ac2pi-0ab1c6", &h),
                format!("ac2 on ac2pi-0ab1c6 · {want}")
            );
        }
    }

    #[test]
    fn refusals_and_unpairing_say_what_to_do() {
        assert_eq!(
            refusal("ketunkolo", &PairRefusal::LocalOnly),
            "ketunkolo serves only its own computer: turn its network on (Settings › \
             Connection) to pair it"
        );
        assert!(NO_OTHER_RIG.contains("connect to it first"));
        assert_eq!(
            unpair_confirm(
                "ketunkolo",
                None,
                &halves(Half::Yes, Half::Yes, Half::Yes, Half::Yes)
            ),
            "Delete again unpairs ketunkolo both ways"
        );
        assert_eq!(
            unpair_confirm(
                "ketunkolo",
                Some(Unseen::NotConnected),
                &halves(Half::Yes, Half::Yes, Half::Yes, Half::Yes)
            ),
            "Delete again unpairs this rig's side only (the app is not connected to ketunkolo \
             now; its side stays). Can't tell whether ketunkolo's stream takes this rig's \
             inputs: if it does, they stop"
        );
        assert!(
            unpair_rest("ketunkolo", Unseen::NotHeld).contains("unpair this rig there to finish")
        );
        assert_eq!(
            unpair_admission_unseen("ketunkolo", Side::Here, Unseen::Unanswered),
            "Can't tell whether this rig admits ketunkolo (this rig did not answer the app): if \
             it does, that stays; revoke it under Authorized clients once seen"
        );
        assert_eq!(
            unpair_admission_unseen("ketunkolo", Side::There, Unseen::NetworkOff),
            "Can't tell whether ketunkolo admits this rig (ketunkolo's network is off): if it \
             does, that stays; revoke it there under Authorized clients once seen"
        );
        assert_eq!(
            unpair_inputs("pupu", &["ketunkolo in 1".into()]),
            "ketunkolo in 1 on pupu come from that rig: remove them first (Settings › Audio I/O)"
        );
    }
}
