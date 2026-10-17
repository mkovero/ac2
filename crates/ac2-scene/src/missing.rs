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
}

/// The text for measurement `name`; `key` shows a hidden one or starts a stopped one.
pub fn missing_text(name: &str, why: Missing, key: &str) -> String {
    match why {
        Missing::Hidden => format!("{name} hidden — {key} shows it"),
        Missing::Stopped => format!("{name} stopped — {key} starts it"),
        Missing::NotYet { what, after: None } => format!("{name}: no {what} yet"),
        Missing::NotYet {
            what,
            after: Some(after),
        } => format!("{name}: no {what} yet: {after}"),
        Missing::NoBandMeter => format!(
            "{name}: no band meter — turn it on in the SPL / Leq settings (or `ac2 spl bands set`)"
        ),
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
    }
}
