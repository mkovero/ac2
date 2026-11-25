//! This app's stimulus settings beside the signal rows of Settings › Audio I/O (whether
//! transfers stop with the stimulus, the stimulus level), the level prompt's title and the
//! toasts those settings lead to.

use ac2_proto::units::Dbfs;

use crate::format;

/// A row of this app's stimulus settings, after the signal rows and directly above the
/// system max level, so the level and its maximum are set in one place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StimRow {
    /// Whether this app's stimulus stop stops the transfers measuring against it: ←/→.
    StopTransfers,
    /// The stimulus level of the rig shown, typed.
    Level,
}

impl StimRow {
    /// The rows in page order.
    pub const ALL: [StimRow; 2] = [StimRow::StopTransfers, StimRow::Level];

    pub fn title(self) -> &'static str {
        match self {
            StimRow::StopTransfers => "Stop transfers with the stimulus",
            StimRow::Level => "Stimulus level",
        }
    }

    /// The hint line under the row.
    pub fn hint(self) -> &'static str {
        match self {
            StimRow::StopTransfers => {
                "On: when this app stops its stimulus while it plays, the transfer \
                 measurements measuring against the stimulus loopback stop with it; one on \
                 another reference keeps running."
            }
            StimRow::Level => {
                "Type a level in dBFS, at most the max level below, then Enter; L types it \
                 from anywhere."
            }
        }
    }
}

/// The transfer row's value.
pub fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// The level row's value: `−20.0 dBFS`, `not set`.
pub fn level_value(level: Option<Dbfs>) -> String {
    level.map_or_else(|| "not set".to_owned(), |l| format::dbfs(l.0))
}

/// The level row focused: the level set, then the one being typed.
pub fn level_typing(level: Option<Dbfs>, typed: &str) -> String {
    if typed.is_empty() {
        format!("{}   new level: ▏", level_value(level))
    } else {
        format!("{}   new level: {typed}▏ dBFS", level_value(level))
    }
}

/// The level prompt's title with the max level it may reach: `Stimulus level (dBFS, max
/// −40.0)`; without a rig to ask, `Stimulus level (dBFS)`.
pub fn level_prompt_title(max: Option<Dbfs>) -> String {
    match max {
        Some(m) => format!("Stimulus level (dBFS, max {})", format::fixed(m.0, 1)),
        None => "Stimulus level (dBFS)".to_owned(),
    }
}

/// A level above the max level, refused.
pub fn above_max(max: Dbfs) -> String {
    format!("above the max level {}", format::dbfs(max.0))
}

/// A level step that cannot go higher.
pub fn at_max(max: f64) -> String {
    format!("at the max level {}", format::dbfs(max))
}

/// What this app's stop did: `stimulus stopped · TF 1, TF 2 stopped`.
pub fn stopped_with(transfers: &[String]) -> String {
    if transfers.is_empty() {
        "stimulus stopped".to_owned()
    } else {
        format!("stimulus stopped · {} stopped", transfers.join(", "))
    }
}

/// The toast after the system max level was changed from Settings: a raise adds how the
/// new room is used, `system max level raised to −20.0 dBFS · L sets the stimulus level`.
pub fn max_changed(from: Dbfs, to: Dbfs, level_key: &str) -> String {
    let said = crate::rig::ceiling_changed(from, to);
    if to.0 > from.0 {
        format!("{said} · {level_key} sets the stimulus level")
    } else {
        said
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_read_like_their_neighbours() {
        assert_eq!(
            StimRow::ALL.map(StimRow::title),
            ["Stop transfers with the stimulus", "Stimulus level"]
        );
        assert_eq!((on_off(true), on_off(false)), ("on", "off"));
        assert_eq!(level_value(None), "not set");
        assert_eq!(level_value(Some(Dbfs(-20.0))), "−20.0 dBFS");
        assert_eq!(level_typing(None, ""), "not set   new level: ▏");
        assert_eq!(
            level_typing(Some(Dbfs(-20.0)), "-30"),
            "−20.0 dBFS   new level: -30▏ dBFS"
        );
        assert!(StimRow::StopTransfers.hint().contains("stimulus loopback"));
    }

    #[test]
    fn the_prompt_states_the_max() {
        assert_eq!(
            level_prompt_title(Some(Dbfs(-40.0))),
            "Stimulus level (dBFS, max −40.0)"
        );
        assert_eq!(level_prompt_title(None), "Stimulus level (dBFS)");
        assert_eq!(above_max(Dbfs(-40.0)), "above the max level −40.0 dBFS");
        assert_eq!(at_max(-40.0), "at the max level −40.0 dBFS");
    }

    #[test]
    fn the_stop_names_the_transfers_it_stopped() {
        assert_eq!(stopped_with(&[]), "stimulus stopped");
        assert_eq!(
            stopped_with(&["TF 1".into(), "TF 2".into()]),
            "stimulus stopped · TF 1, TF 2 stopped"
        );
    }

    #[test]
    fn a_raise_says_how_to_use_it() {
        assert_eq!(
            max_changed(Dbfs(-60.0), Dbfs(-40.0), "L"),
            "system max level raised to −40.0 dBFS · L sets the stimulus level"
        );
        assert_eq!(
            max_changed(Dbfs(-40.0), Dbfs(-60.0), "L"),
            "system max level lowered to −60.0 dBFS"
        );
    }
}
