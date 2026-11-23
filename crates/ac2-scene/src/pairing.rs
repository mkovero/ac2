//! The words of pairing two rigs on the app's Connection page: the *Paired rigs* rows, the
//! pick of a rig to pair with, the refusals and the toasts. A pairing has four halves, two
//! per direction: a rig takes another's inputs when it lists that rig (`rig.peers`) and
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Half {
    Yes,
    No,
    /// The app cannot see this half.
    Unknown(Unseen),
}

/// Why the app cannot see the other rig's halves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unseen {
    /// The app does not hold that rig: its daemon's state is not here.
    NotHeld,
    /// That rig has not answered yet.
    Asking,
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

    /// The halves the shown rig's own state shows: something links the two rigs.
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
}

/// A *Paired rigs* row: `ketunkolo · paired both ways`, else what is missing
/// (`ketunkolo · has not admitted this rig`), else what cannot be told.
pub fn row(rig: &str, h: &Halves) -> String {
    if h.both_ways() {
        return format!("{rig} · paired both ways");
    }
    let missing: Vec<&str> = [
        (&h.here_lists, "not listed by this rig"),
        (&h.there_admits, "has not admitted this rig"),
        (&h.there_lists, "does not list this rig"),
        (&h.here_admits, "not admitted by this rig"),
    ]
    .into_iter()
    .filter(|(half, _)| **half == Half::No)
    .map(|(_, w)| w)
    .collect();
    if missing.is_empty() {
        format!("{rig} · its side: can't tell")
    } else {
        format!("{rig} · {}", missing.join(" · "))
    }
}

fn unseen(rig: &str, u: Unseen) -> String {
    match u {
        Unseen::NotHeld => format!("the app does not hold {rig}: connect to it to see"),
        Unseen::Asking => format!("asking {rig}…"),
        Unseen::NetworkOff => format!("{rig}'s network is off"),
    }
}

/// A *Paired rigs* row's hover: each half in words, then what the keys do there.
pub fn tip(rig: &str, h: &Halves) -> String {
    let line = |half: &Half, yes: String, no: String, what: String| match half {
        Half::Yes => yes,
        Half::No => no,
        Half::Unknown(u) => format!("can't tell whether {what} ({})", unseen(rig, *u)),
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
    if h.both_ways() {
        lines.push("Delete twice unpairs both ways".into());
    } else {
        lines.push("Enter pairs both ways again · Delete twice unpairs".into());
    }
    lines.join("\n")
}

/// A rig offered by *Pair with rig…*: `ketunkolo`, or `ketunkolo · mends a half` when
/// some half is already in place.
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
    /// The app is not connected to it now: its paired rigs are not known, and a pairing
    /// replaces that list.
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

/// The first Delete on a *Paired rigs* row; `held`: the app holds that rig, so both
/// sides are unpaired, else this rig's side only.
pub fn unpair_confirm(rig: &str, held: bool) -> String {
    if held {
        format!("Delete again unpairs {rig} both ways")
    } else {
        format!(
            "Delete again unpairs this rig's side only: the app does not hold {rig}, whose \
             side stays"
        )
    }
}

/// After this rig's side was unpaired from a rig the app does not hold: what remains.
pub fn unpair_rest(rig: &str) -> String {
    format!(
        "{rig}'s side is unchanged (the app does not hold it): connect to {rig}, show it and \
         unpair this rig there to finish"
    )
}

/// Unpairing refused while the session of rig `taker` takes inputs `inputs` from the
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
        assert!(tip("ketunkolo", &all).ends_with("Delete twice unpairs both ways"));
        let one = halves(Yes, No, Yes, Yes);
        assert_eq!(
            row("ketunkolo", &one),
            "ketunkolo · has not admitted this rig"
        );
        assert!(
            tip("ketunkolo", &one).contains("  ketunkolo has not admitted this rig"),
            "{}",
            tip("ketunkolo", &one)
        );
        let two = halves(No, Yes, Yes, No);
        assert_eq!(
            row("ketunkolo", &two),
            "ketunkolo · not listed by this rig · not admitted by this rig"
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
        let h = halves(Yes, Unknown(Unseen::NotHeld), Unknown(Unseen::NotHeld), Yes);
        assert_eq!(row("ketunkolo", &h), "ketunkolo · its side: can't tell");
        let t = tip("ketunkolo", &h);
        assert!(
            t.contains(
                "can't tell whether ketunkolo has admitted this rig (the app does not hold \
                 ketunkolo: connect to it to see)"
            ),
            "{t}"
        );
        assert!(!t.contains("has not admitted"), "{t}");
        assert!(!h.missing() && !h.both_ways() && h.any());
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
            unpair_confirm("ketunkolo", true),
            "Delete again unpairs ketunkolo both ways"
        );
        assert!(unpair_rest("ketunkolo").contains("unpair this rig there to finish"));
        assert_eq!(
            unpair_inputs("pupu", &["ketunkolo in 1".into()]),
            "ketunkolo in 1 on pupu come from that rig: remove them first (Settings › Audio I/O)"
        );
    }
}
