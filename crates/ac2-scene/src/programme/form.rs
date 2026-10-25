//! The programme meter dialog's words: what a Programme pane meters (a pair, or up to eight
//! inputs as bars side by side) and how the rig's programme meters read.

pub const NEW_TITLE: &str = "New programme meter";
pub const EDIT_TITLE: &str = "Edit programme meter";

/// What Enter does in each.
pub const NEW_SUBMIT: &str = "Enter opens it in a new pane beside the focused one";
pub const EDIT_SUBMIT: &str = "Enter applies to this pane";

/// The heading over what the pane meters: the pane's own.
pub const WHAT_HEADING: &str = "What to meter";
/// The heading over the rig's settings (the whose words follow it).
pub const HOW_HEADING: &str = "How it reads";

/// The field that picks a pair or the inputs.
pub const METER: &str = "Meter";
/// The option of that field that meters inputs, one bar each.
pub const INPUTS_OPTION: &str = "inputs side by side";
pub const PAIR_HINT: &str = "bars L and R, goniometer, correlation, loudness";
pub const INPUTS_HINT: &str = "one bar per input, up to 8";

/// A bar of the inputs: `Bar 1`.
pub fn bar_label(i: usize) -> String {
    format!("Bar {}", i + 1)
}

/// A bar after the first with no input.
pub const NO_BAR: &str = "none";
pub const FIRST_BAR_HINT: &str = "by name; mics show levels, no loudness";
pub const MORE_BAR_HINT: &str = "← past the first input: none";

/// Why the chooser's row or the dialog cannot meter.
pub const NO_INPUTS: &str = "no inputs in the session to meter";
pub const CHOOSE_FIRST: &str = "choose the input of bar 1";
pub const BARS_DISTINCT: &str = "each bar meters another input, 8 at most";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_count_from_one() {
        assert_eq!(bar_label(0), "Bar 1");
        assert_eq!(bar_label(7), "Bar 8");
    }
}
