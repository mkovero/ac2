//! What a pane says when its own measurement has nothing to draw. A pane never borrows
//! another measurement's curve or reading to fill the space: it names its own and says why
//! it is empty, in the words the IR view uses ([`crate::ir::missing_text`]).

/// Why a pane's own measurement draws nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// Hidden in this app (it keeps measuring); the key shows it.
    Hidden,
    /// Stopped before its first frame arrived; the key starts it.
    Stopped,
    /// Running: its first `what` is on its way, and `after` says when, if it takes long.
    NotYet {
        what: &'static str,
        after: Option<&'static str>,
    },
    /// An SPL meter without a band meter, in the band view.
    NoBandMeter,
    /// A sweep measurement none of whose runs has been played: the key arms one and Enter
    /// (the stimulus's own key in every keymap) fires it.
    NoRun,
    /// The pane has no measurement of the `kind` its view draws (`transfer`); the key
    /// makes one. It has no name to say.
    NoMeasurement { kind: &'static str },
}

/// The text for measurement `name`; `key` shows a hidden one, starts a stopped one or
/// makes the one that is missing.
pub fn missing_text(name: &str, why: Missing, key: &str) -> String {
    match why {
        Missing::Hidden => format!("{name} hidden — {key} shows it"),
        Missing::Stopped => format!("{name} stopped — {key} starts it"),
        Missing::NotYet { what, after: None } => format!("{name}: no {what} yet"),
        Missing::NotYet {
            what,
            after: Some(after),
        } => format!("{name}: no {what} yet: {after}"),
        Missing::NoRun => format!("{name}: no run yet — {key} arms the sweep, Enter fires it"),
        Missing::NoBandMeter => format!(
            "{name}: no band meter — turn it on in the SPL / Leq settings (or `ac2 spl bands set`)"
        ),
        Missing::NoMeasurement { kind } => format!("no {kind} measurement — {key} makes one"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_names_its_own_measurement() {
        assert_eq!(
            missing_text("FOH SPL", Missing::Hidden, "A"),
            "FOH SPL hidden — A shows it"
        );
        assert_eq!(
            missing_text("FOH SPL", Missing::Stopped, "S"),
            "FOH SPL stopped — S starts it"
        );
        assert_eq!(
            missing_text(
                "FOH SPL",
                Missing::NotYet {
                    what: "SPL frame",
                    after: None
                },
                "S"
            ),
            "FOH SPL: no SPL frame yet"
        );
        assert_eq!(
            missing_text(
                "FOH SPL",
                Missing::NotYet {
                    what: "Leq windows",
                    after: Some("they show once the meter has measured a second")
                },
                "S"
            ),
            "FOH SPL: no Leq windows yet: they show once the meter has measured a second"
        );
        assert_eq!(
            missing_text("Genelec 1 m", Missing::NoRun, "Space"),
            "Genelec 1 m: no run yet — Space arms the sweep, Enter fires it"
        );
        assert_eq!(
            missing_text("", Missing::NoMeasurement { kind: "transfer" }, "Ctrl+M"),
            "no transfer measurement — Ctrl+M makes one"
        );
    }
}
