//! What a capture into a slot says: one line for every measurement the focused pane drew,
//! those stored first, then each that had nothing to store and why.

/// How one measurement's capture ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome<'a> {
    /// Stored as a trace in the slot.
    Captured,
    /// The measurement has no result to store (not measured since its start or change).
    NoResult,
    /// Refused for another reason, in the daemon's words.
    Failed(&'a str),
}

/// The toast of a capture into slot `slot` of the measurements named in `results`, in the
/// pane's order: `slot 1: TF 1, TF 2 captured · TF 3: no result yet`.
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
        match o {
            Outcome::Captured => {}
            Outcome::NoResult => s.push_str(&format!(" · {name}: no result yet")),
            Outcome::Failed(e) => s.push_str(&format!(" · {name}: {e}")),
        }
    }
    s
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
                &[("TF 1", Captured), ("TF 2", Captured), ("TF 3", NoResult)]
            ),
            "slot 1: TF 1, TF 2 captured · TF 3: no result yet"
        );
        assert_eq!(
            slot_toast(4, &[("Main", Captured)]),
            "slot 4: Main captured"
        );
        assert_eq!(
            slot_toast(2, &[("TF 1", NoResult), ("Sub", Failed("not connected"))]),
            "slot 2: nothing captured · TF 1: no result yet · Sub: not connected"
        );
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
