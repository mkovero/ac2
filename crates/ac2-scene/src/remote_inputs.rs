//! The words of remote inputs on the app's Audio I/O and Connection pages
//! (`docs/design/remote-inputs.md` §5): a remote input is named by its rig and that rig's
//! input, so the same input reads the same in the add dialog, its row, the channel pickers
//! and the toasts.

use crate::meter::InputRole;

/// The Audio I/O row that adds one.
pub const ADD: &str = "Add an input from another rig…";

/// The Audio I/O section the remote inputs are listed in, each under its rig's heading.
pub const SECTION: &str = "Inputs from other rigs";

/// The Connection page's section on the clocks and links to the remote rigs.
pub const BETWEEN_RIGS: &str = "Between rigs";

/// The Connection page's list of the rigs that may take this rig's inputs.
pub const ADMITTED: &str = "Rigs admitted to this rig";

/// The add dialog with no other rig held by the app.
pub const NO_OTHER_RIG: &str =
    "The app holds no other rig: add one first (Connection › Add a rig), then pick its input here.";

/// The add dialog on a rig without an open session.
pub const NO_SESSION_HERE: &str =
    "Open this rig's audio I/O first (Enter on this page): a remote input joins its session.";

/// What a remote input is called: its rig's mic name, if any, then the rig and that rig's
/// input number: `ketunkolo in 1`, `MM1 34804 · ketunkolo in 1`.
pub fn remote_name(rig: &str, channel: u16, mic: Option<&str>) -> String {
    let at = format!("{rig} in {}", u32::from(channel) + 1);
    match mic.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => format!("{m} · {at}"),
        None => at,
    }
}

/// An input of another rig as that rig's row says it, without the rig: `in 1 · mic MM1
/// 34804`, `in 3 · line capture_3`, `in 2`.
pub fn source_row(channel: u16, role: Option<InputRole>, name: Option<&str>) -> String {
    let mut s = format!("in {}", u32::from(channel) + 1);
    if let Some(r) = role {
        s.push_str(" · ");
        s.push_str(r.name());
    }
    if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
        s.push_str(if role.is_some() { " " } else { " · " });
        s.push_str(n);
    }
    s
}

/// A row of the add dialog: `ketunkolo · in 1 · mic MM1 34804`, `… · added` when this
/// rig's session has it already.
pub fn pick_row(rig: &str, row: &str, added: bool) -> String {
    if added {
        format!("{rig} · {row} · added")
    } else {
        format!("{rig} · {row}")
    }
}

/// Why rig `rig`'s inputs cannot be taken, as its heading in the add dialog says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceRefusal {
    /// It has not answered `server.info` yet.
    Asking,
    /// It listens on its own computer only: no other rig reaches it.
    LocalOnly,
    /// It has no session: nothing captured to give.
    NoSession,
    /// Its session replays a recording: no device clock to pair with.
    Replaying,
    /// It already authorizes an app under the name this rig would be admitted as.
    NameTaken(String),
    /// The app cannot ask it (`server.info` failed).
    Unanswered(String),
}

/// The add dialog's line for a rig whose inputs cannot be taken.
pub fn source_refusal(rig: &str, why: &SourceRefusal) -> String {
    match why {
        SourceRefusal::Asking => format!("{rig}: asking…"),
        SourceRefusal::LocalOnly => format!(
            "{rig} serves only this computer: start its ac2d in network mode to give its inputs"
        ),
        SourceRefusal::NoSession => {
            format!("{rig} has no session: open its audio I/O there first")
        }
        SourceRefusal::Replaying => format!("{rig} is replaying a recording"),
        SourceRefusal::NameTaken(n) => {
            format!("{rig} already authorizes an app named {n}: revoke it there first (Connection)")
        }
        SourceRefusal::Unanswered(e) => format!("{rig}: cannot ask it ({e})"),
    }
}

/// The add dialog on a rig that listens on its own computer only: it has no key to show
/// another rig.
pub fn this_rig_local(rig: &str) -> String {
    format!(
        "{rig} listens on this computer only: another rig's input needs its ac2d in network \
         mode"
    )
}

/// The add dialog while this rig has not answered `server.info` yet.
pub fn asking_this_rig(rig: &str) -> String {
    format!("asking {rig} for its key…")
}

/// A change on a remote input refused: its setup and calibration are its own rig's.
pub fn set_on(rig: &str) -> String {
    format!("set on {rig}: show {rig} (the rig drop-down) to change it")
}

/// The toast on rig `source` once it admitted rig `taker`.
pub fn admitted_toast(source: &str, taker: &str) -> String {
    format!("{source} admitted {taker}: it may take {source}'s inputs")
}

/// The toast once the session reopened with remote input `name`.
pub fn added_toast(name: &str) -> String {
    format!("{name} added to the audio I/O")
}

/// The first Delete on a remote input's row.
pub fn remove_confirm(name: &str) -> String {
    format!("Delete again removes {name} (the audio I/O reopens)")
}

/// The toast once the session reopened without remote input `name`.
pub fn removed_toast(name: &str) -> String {
    format!("{name} removed from the audio I/O")
}

/// A row of the rigs admitted to this rig: `pupu · may take this rig's inputs`.
pub fn admitted_row(name: &str) -> String {
    format!("{name} · may take this rig's inputs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_input_is_named_by_its_rig_and_that_rigs_input() {
        assert_eq!(remote_name("ketunkolo", 0, None), "ketunkolo in 1");
        assert_eq!(
            remote_name("ketunkolo", 0, Some("MM1 34804")),
            "MM1 34804 · ketunkolo in 1"
        );
        let row = source_row(0, Some(InputRole::Mic), Some("MM1 34804"));
        assert_eq!(row, "in 1 · mic MM1 34804");
        assert_eq!(
            pick_row("ketunkolo", &row, false),
            "ketunkolo · in 1 · mic MM1 34804"
        );
        assert_eq!(source_row(1, None, None), "in 2");
        assert_eq!(source_row(2, None, Some("capture_3")), "in 3 · capture_3");
        assert!(pick_row("ketunkolo", "in 2", true).ends_with("· added"));
    }

    #[test]
    fn refusals_say_why_and_what_to_do() {
        assert_eq!(
            source_refusal("ketunkolo", &SourceRefusal::LocalOnly),
            "ketunkolo serves only this computer: start its ac2d in network mode to give its \
             inputs"
        );
        assert!(set_on("ketunkolo").starts_with("set on ketunkolo"));
        assert!(this_rig_local("pupu").contains("network mode"));
    }
}
