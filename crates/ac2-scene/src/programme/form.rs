//! The programme meter dialog's words: what a Programme pane meters (a pair, or up to eight
//! inputs side by side) and how the rig's programme meters read. The rows name inputs: under
//! a loudness ballistic the pane's bars follow from them by source, a pair reading one bar.

pub const NEW_TITLE: &str = "New programme meter";
pub const EDIT_TITLE: &str = "Edit programme meter";

/// What Enter does in each.
pub const NEW_SUBMIT: &str =
    "Enter opens it beside the focused one: meters, and a goniometer pane for a pair";
pub const EDIT_SUBMIT: &str = "Enter applies to this pane";

/// The heading over what the pane meters: the pane's own.
pub const WHAT_HEADING: &str = "What to meter";
/// The heading over the rig's settings (the whose words follow it).
pub const HOW_HEADING: &str = "How it reads";
/// The heading over the app's own display settings: the Settings page's name, since the
/// goniometer's fade is this app's and not the rig's.
pub const DISPLAY_HEADING: &str = "Display";
/// The ember decay row's label: Settings › Display's.
pub const EMBER_TITLE: &str = "Ember decay";
/// Under the ember decay row.
pub const EMBER_NOTE: &str = "how long a goniometer point glows; also Settings › Display";

/// The field that picks a pair or the inputs.
pub const METER: &str = "Meter";
/// The option of that field that meters inputs side by side.
pub const INPUTS_OPTION: &str = "inputs side by side";
pub const PAIR_HINT: &str =
    "L and R, loudness (line or digital); goniometer and correlation in their own pane";
pub const INPUTS_HINT: &str = "one row per input, up to 8";

/// A row of the inputs: `Input 1`.
pub fn bar_label(i: usize) -> String {
    format!("Input {}", i + 1)
}

/// A row after the first with no input.
pub const NO_BAR: &str = "none";
pub const FIRST_BAR_HINT: &str = "by name; mics show levels, no loudness";
pub const MORE_BAR_HINT: &str = "← past the first input: none";

/// Why the chooser's row or the dialog cannot meter.
pub const NO_INPUTS: &str = "no inputs in the session to meter";
pub const CHOOSE_FIRST: &str = "choose input 1";
pub const BARS_DISTINCT: &str = "each row names another input, 8 at most";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_count_from_one() {
        assert_eq!(bar_label(0), "Input 1");
        assert_eq!(bar_label(7), "Input 8");
    }
}
