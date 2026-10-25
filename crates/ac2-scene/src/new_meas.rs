//! The New measurement chooser: every kind of measurement the app can make, in words, so
//! the kinds that have no key of their own are found without knowing the palette's words;
//! at its foot the Programme meter, which meters inputs without making a measurement.

use crate::meas_list::Choice;

/// A kind of measurement the chooser offers, in its rows' order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NewKind {
    Transfer,
    Spectrum,
    Rta,
    Spl,
    Sweep,
    Math,
    Alignment,
    Programme,
}

impl NewKind {
    /// The rows, top to bottom: the live two-channel measurement first, then one input's,
    /// then what is played and what is computed from the others; last the meter, which is
    /// not a measurement.
    pub const ALL: [NewKind; 8] = [
        NewKind::Transfer,
        NewKind::Spectrum,
        NewKind::Rta,
        NewKind::Spl,
        NewKind::Sweep,
        NewKind::Math,
        NewKind::Alignment,
        NewKind::Programme,
    ];

    /// The kind's name, as the palette's `New …` entries and the forms' titles say it.
    pub fn name(self) -> &'static str {
        match self {
            NewKind::Transfer => "Transfer",
            NewKind::Spectrum => "Spectrum",
            NewKind::Rta => "RTA",
            NewKind::Spl => "SPL meter",
            NewKind::Sweep => "Sweep",
            NewKind::Math => "Math channel",
            NewKind::Alignment => "Alignment",
            NewKind::Programme => "Programme meter",
        }
    }

    /// What it measures, in one line.
    pub fn what(self) -> &'static str {
        match self {
            NewKind::Transfer => "reference vs measurement: magnitude, phase, coherence, IR",
            NewKind::Spectrum => "one input's level at every frequency (FFT)",
            NewKind::Rta => "one input's level in fractional-octave bands",
            NewKind::Spl => "sound level: A / C / Z weighting, Leq windows and limits",
            NewKind::Sweep => "a played sweep: response and harmonic distortion",
            NewKind::Math => "A ÷ × + − B of two curves, or the average of several",
            NewKind::Alignment => {
                "delay and polarity of two systems in their overlap (sub + main, delay tower)"
            }
            NewKind::Programme => {
                "meters inputs live in a new pane (levels, correlation); adds no measurement"
            }
        }
    }

    /// Its row: `Sweep — a played sweep: … · Shift+S` (`key`: its own key, if bound).
    pub fn row(self, key: Option<&str>) -> String {
        let mut s = format!("{} — {}", self.name(), self.what());
        if let Some(k) = key {
            s.push_str("  ·  ");
            s.push_str(k);
        }
        s
    }
}

/// The chooser's text: one row per kind ([`NewKind::ALL`]'s order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewChoices {
    pub title: String,
    pub lines: Vec<String>,
    pub choices: Vec<Choice>,
    /// The row Enter takes at first.
    pub default: usize,
    pub hint: String,
}

/// The chooser, `preselect` highlighted (unless it cannot be made now: then the first that
/// can). `key` gives a kind's own key; `blocked`, why it cannot be made now.
pub fn new_choices(
    preselect: NewKind,
    key: impl Fn(NewKind) -> Option<String>,
    blocked: impl Fn(NewKind) -> Option<String>,
) -> NewChoices {
    let choices: Vec<Choice> = NewKind::ALL
        .iter()
        .map(|k| Choice {
            label: k.row(key(*k).as_deref()),
            blocked: blocked(*k),
        })
        .collect();
    let at = NewKind::ALL
        .iter()
        .position(|k| *k == preselect)
        .unwrap_or(0);
    let default = if choices[at].blocked.is_none() {
        at
    } else {
        choices
            .iter()
            .position(|c| c.blocked.is_none())
            .unwrap_or(at)
    };
    NewChoices {
        title: TITLE.to_owned(),
        lines: vec![
            "What to measure or meter (its settings come next): it opens in its own pane."
                .to_owned(),
        ],
        choices,
        default,
        hint: "↑/↓ choose · Enter opens its settings · Esc cancels".to_owned(),
    }
}

/// The chooser's title, and the palette entry and the pane list's row that open it.
pub const TITLE: &str = "New measurement";

/// The pane list's last row: `New measurement…  Ctrl+M`.
pub fn menu_row(key: Option<&str>) -> String {
    match key {
        Some(k) => format!("{TITLE}…  {k}"),
        None => format!("{TITLE}…"),
    }
}

/// The measurement list with nothing in it: how to make the first one (`reach`: the
/// chooser's key, or its palette entry).
pub fn empty_list(reach: &str) -> String {
    format!("none — {reach}: {TITLE}…")
}

/// Q on the only pane: it stays, and what does bring more to the screen (`split`: the
/// split key, if bound; `reach`: the chooser's key, or its palette entry).
pub fn last_pane_stays(split: Option<&str>, reach: &str) -> String {
    match split {
        Some(n) => format!("the last pane stays: {n} splits it · {reach}: {TITLE}…"),
        None => format!("the last pane stays · {reach}: {TITLE}…"),
    }
}

/// The empty transfer pane on a session without measurements (`reach`: the chooser's key,
/// else the palette's): where every kind is made.
pub fn no_measurements(reach: &str) -> String {
    let kinds: Vec<String> = NewKind::ALL
        .iter()
        // The meter makes no measurement.
        .filter(|k| **k != NewKind::Programme)
        .map(|k| match k {
            // Lower case mid-sentence; RTA and SPL stay capitals.
            NewKind::Rta | NewKind::Spl => k.name().to_owned(),
            _ => k.name().to_lowercase(),
        })
        .collect();
    format!(
        "No measurements — {reach} → {TITLE}… ({})",
        kinds.join(", ")
    )
}

/// A new transfer's default name: what it compares, `Reference → M30 FOH` (`mic`: the
/// measurement input's name), numbered ` 2`, ` 3` … past the names already `taken`; `TF n`
/// when the input has no name.
pub fn transfer_name(mic: Option<&str>, taken: &[&str], n: usize) -> String {
    let Some(mic) = mic.map(str::trim).filter(|m| !m.is_empty()) else {
        return format!("TF {n}");
    };
    let base = format!("Reference → {mic}");
    if !taken.contains(&base.as_str()) {
        return base;
    }
    (2..)
        .map(|i| format!("{base} {i}"))
        .find(|c| !taken.contains(&c.as_str()))
        .unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transfer_is_named_after_what_it_compares() {
        assert_eq!(
            transfer_name(Some("M30 FOH"), &[], 1),
            "Reference → M30 FOH"
        );
        assert_eq!(
            transfer_name(Some("M30 FOH"), &["Reference → M30 FOH", "TF 1"], 3),
            "Reference → M30 FOH 2"
        );
        assert_eq!(
            transfer_name(
                Some("M30 FOH"),
                &["Reference → M30 FOH", "Reference → M30 FOH 2"],
                3
            ),
            "Reference → M30 FOH 3"
        );
        assert_eq!(transfer_name(None, &[], 2), "TF 2");
        assert_eq!(transfer_name(Some(" "), &[], 1), "TF 1");
    }

    #[test]
    fn rows_name_every_kind_with_its_key_and_reason() {
        let q = new_choices(
            NewKind::Spectrum,
            |k| match k {
                NewKind::Sweep => Some("Shift+S".to_owned()),
                NewKind::Alignment => Some("Alt+M".to_owned()),
                _ => None,
            },
            |k| (k == NewKind::Math).then(|| "no curves".to_owned()),
        );
        let labels: Vec<&str> = q.choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Transfer — reference vs measurement: magnitude, phase, coherence, IR",
                "Spectrum — one input's level at every frequency (FFT)",
                "RTA — one input's level in fractional-octave bands",
                "SPL meter — sound level: A / C / Z weighting, Leq windows and limits",
                "Sweep — a played sweep: response and harmonic distortion  ·  Shift+S",
                "Math channel — A ÷ × + − B of two curves, or the average of several",
                "Alignment — delay and polarity of two systems in their overlap (sub + main, \
                 delay tower)  ·  Alt+M",
                "Programme meter — meters inputs live in a new pane (levels, correlation); adds no \
                 measurement",
            ]
        );
        assert_eq!(q.default, 1);
        assert_eq!(q.choices[5].blocked.as_deref(), Some("no curves"));
        assert_eq!(q.title, "New measurement");
    }

    #[test]
    fn alignment_and_the_meter_say_why_they_are_blocked() {
        let q = new_choices(
            NewKind::Programme,
            |_| None,
            |k| match k {
                NewKind::Alignment => Some("an alignment needs two transfer curves".to_owned()),
                NewKind::Programme => Some("no inputs in the session to meter".to_owned()),
                _ => None,
            },
        );
        assert_eq!(
            q.choices[6].blocked.as_deref(),
            Some("an alignment needs two transfer curves")
        );
        assert_eq!(
            q.choices[7].blocked.as_deref(),
            Some("no inputs in the session to meter")
        );
        // The meter preselected but blocked: the first row that can be made.
        assert_eq!(q.default, 0);
        let open = new_choices(NewKind::Programme, |_| None, |_| None);
        assert_eq!(open.default, 7);
        assert_eq!(
            open.hint,
            "↑/↓ choose · Enter opens its settings · Esc cancels"
        );
    }

    #[test]
    fn a_blocked_preselection_yields_to_the_first_that_can_be_made() {
        let q = new_choices(
            NewKind::Math,
            |_| None,
            |k| (k == NewKind::Math).then(|| "no".to_owned()),
        );
        assert_eq!(q.default, 0);
        assert_eq!(menu_row(Some("Ctrl+M")), "New measurement…  Ctrl+M");
        assert_eq!(empty_list("Ctrl+M"), "none — Ctrl+M: New measurement…");
        assert_eq!(
            last_pane_stays(Some("N"), "Ctrl+M"),
            "the last pane stays: N splits it · Ctrl+M: New measurement…"
        );
        assert_eq!(
            last_pane_stays(None, "Ctrl+M"),
            "the last pane stays · Ctrl+M: New measurement…"
        );
        assert_eq!(
            no_measurements("Ctrl+M"),
            "No measurements — Ctrl+M → New measurement… (transfer, spectrum, RTA, SPL meter, \
             sweep, math channel, alignment)"
        );
    }
}
