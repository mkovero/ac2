//! What a capture into a slot says: one line for every measurement the focused pane drew,
//! those stored first, then each that had nothing to store and why.

/// How one curve's capture ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome<'a> {
    /// Stored as a trace in the slot.
    Captured,
    /// Not stored, in the daemon's words (`TF 3: no result to capture yet`) or the link's.
    NotStored(&'a str),
}

/// The toast of a capture into slot `slot` of the curves named in `results`, in the pane's
/// order: `slot 1: TF 1, TF 2 captured · TF 3: no result to capture yet`. A reason that
/// already starts with the curve's name is not named twice.
pub fn slot_toast(slot: u8, results: &[(&str, Outcome<'_>)]) -> String {
    let done: Vec<&str> = results
        .iter()
        .filter(|(_, o)| *o == Outcome::Captured)
        .map(|(n, _)| *n)
        .collect();
    let mut s = if done.is_empty() {
        format!("slot {slot}: nothing captured")
    } else {
        format!("slot {slot}: {} captured", done.join(", "))
    };
    for (name, o) in results {
        if let Outcome::NotStored(why) = o {
            let why = why
                .strip_prefix(*name)
                .and_then(|r| r.strip_prefix(": "))
                .unwrap_or(why);
            s.push_str(&format!(" · {name}: {why}"));
        }
    }
    s
}

/// A capture key pressed again before the daemon answered the slot's previous capture.
pub fn still_capturing(slot: u8) -> String {
    format!("slot {slot}: still capturing")
}

/// A show/hide-slot key on a pane none of whose curves has a trace in the slot, while
/// other panes' do: those are left as they are.
pub fn nothing_of_pane(slot: u8) -> String {
    format!("slot {slot}: nothing of this pane")
}

/// A capture key on a pane that draws no live curve to store; `to_it` says how to put a
/// measurement on the pane, `new` how to make one.
pub fn nothing_to_capture(to_it: &str, new: &str) -> String {
    format!(
        "this pane shows no transfer, spectrum, RTA or math measurement to capture ({to_it}; \
         {new} makes one)"
    )
}

/// A show/hide-slot key on a pane with no trace in that slot: `key` the capture key of the
/// slot when it has one, else `palette` (how the palette reaches the capture).
pub fn slot_empty(slot: u8, key: Option<&str>, palette: &str) -> String {
    match key {
        Some(k) => format!("slot {slot} is empty ({k} captures into it)"),
        None => format!("slot {slot} is empty ({palette})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_what_was_stored_and_what_was_not() {
        use Outcome::*;
        assert_eq!(
            slot_toast(
                1,
                &[
                    ("TF 1", Captured),
                    ("TF 2", Captured),
                    ("TF 3", NotStored("TF 3: no result to capture yet"))
                ]
            ),
            "slot 1: TF 1, TF 2 captured · TF 3: no result to capture yet"
        );
        assert_eq!(
            slot_toast(4, &[("Main", Captured)]),
            "slot 4: Main captured"
        );
        assert_eq!(
            slot_toast(
                2,
                &[
                    (
                        "TF 1",
                        NotStored(
                            "TF 1: no result to capture (it has not measured with its \
                             current settings)"
                        )
                    ),
                    ("Sub", NotStored("client is closed"))
                ]
            ),
            "slot 2: nothing captured · TF 1: no result to capture (it has not measured \
             with its current settings) · Sub: client is closed"
        );
        assert_eq!(still_capturing(3), "slot 3: still capturing");
        assert_eq!(nothing_of_pane(5), "slot 5: nothing of this pane");
    }

    #[test]
    fn an_empty_slot_says_how_to_fill_it() {
        assert_eq!(
            slot_empty(3, Some("Ctrl+3"), "palette: Capture pane to slot 3"),
            "slot 3 is empty (Ctrl+3 captures into it)"
        );
        assert_eq!(
            slot_empty(3, None, "palette: X"),
            "slot 3 is empty (palette: X)"
        );
    }

    #[test]
    fn says_how_to_get_something_to_capture() {
        assert_eq!(
            nothing_to_capture("drag one onto it", "N"),
            "this pane shows no transfer, spectrum, RTA or math measurement to capture \
             (drag one onto it; N makes one)"
        );
    }
}
