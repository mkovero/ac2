//! What the tree asks before it changes what a measurement owns: the words for what it
//! owns, Delete's confirmation, its three choices for a measurement that owns traces, its
//! refusal, and the groups Move offers.

use ac2_proto::model::{MeasKind, Measurement, TraceOwner};

use super::{kind_name, kind_tag, state_word};
use crate::trace_list::DeleteConfirm;

/// What a measurement owns, for the question Delete asks: `3 traces`, `2 traces and 1 math
/// channel`.
pub fn owned_words(traces: usize, maths: usize) -> String {
    let t = match traces {
        0 => None,
        1 => Some("1 trace".to_owned()),
        n => Some(format!("{n} traces")),
    };
    let m = match maths {
        0 => None,
        1 => Some("1 math channel".to_owned()),
        n => Some(format!("{n} math channels")),
    };
    match (t, m) {
        (Some(t), Some(m)) => format!("{t} and {m}"),
        (Some(x), None) | (None, Some(x)) => x,
        (None, None) => "nothing".to_owned(),
    }
}

/// One answer of the question deleting a measurement that owns traces asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// `Keep them (move to Imported)`.
    pub label: String,
    /// Why it cannot be chosen (a math channel that stays would lose an operand).
    pub blocked: Option<String>,
}

/// The question Delete asks for a measurement that owns stored traces or math channels:
/// keep them (moved to the imported group), delete them too, or cancel. Asked every time;
/// Keep is the default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteChoices {
    /// `Delete measurement Main L?`
    pub title: String,
    /// `It has 3 traces.`, then what each answer does.
    pub lines: Vec<String>,
    /// Keep, Delete, Cancel, in that order.
    pub choices: Vec<Choice>,
    /// The choice Enter takes at first: Keep, or Cancel when Keep is blocked.
    pub default: usize,
    pub hint: String,
}

/// Index of "Keep them" in [`DeleteChoices::choices`].
pub const KEEP: usize = 0;
/// Index of "Delete them too".
pub const DELETE: usize = 1;
/// Index of "Cancel".
pub const CANCEL: usize = 2;

/// The question for measurement `m` owning `traces` stored traces and `maths` math
/// channels. `keep_blocked` / `delete_blocked`: the math channels that would lose an
/// operand with that answer (the daemon refuses it).
pub fn delete_choices(
    m: &Measurement,
    traces: usize,
    maths: usize,
    keep_blocked: &[String],
    delete_blocked: &[String],
) -> DeleteChoices {
    let name = &m.config.name;
    let them = if traces + maths == 1 { "it" } else { "them" };
    let (they, stay, go) = if traces + maths == 1 {
        ("it", "stays", "goes")
    } else {
        ("they", "stay", "go")
    };
    let why = |users: &[String]| {
        (!users.is_empty()).then(|| {
            format!(
                "the math channel{} {} would lose an operand",
                if users.len() == 1 { "" } else { "s" },
                users.join(", ")
            )
        })
    };
    let choices = vec![
        Choice {
            label: format!("Keep {them} (move to Imported)"),
            blocked: why(keep_blocked),
        },
        Choice {
            label: format!("Delete {them} too"),
            blocked: why(delete_blocked),
        },
        Choice {
            label: "Cancel".to_owned(),
            blocked: None,
        },
    ];
    let default = if choices[KEEP].blocked.is_none() {
        KEEP
    } else {
        CANCEL
    };
    DeleteChoices {
        title: format!("Delete measurement {name}?"),
        lines: vec![
            format!(
                "{} · {} · it has {}.",
                kind_name(&m.config.kind),
                state_word(m),
                owned_words(traces, maths)
            ),
            format!(
                "Keep: {they} {stay}, listed under Imported. Delete: {they} {go} with {name}; it \
                 cannot be undone."
            ),
        ],
        choices,
        default,
        hint: "←/→ choose · Enter confirms · Esc cancels".to_owned(),
    }
}

/// The question "Move to measurement…" asks: where a stored trace or a math channel is
/// filed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveChoices {
    /// `Move pre-EQ to…`
    pub title: String,
    pub lines: Vec<String>,
    /// Every measurement that can own (not a math channel), in tree order, then Imported;
    /// where it is filed now cannot be picked.
    pub choices: Vec<Choice>,
    /// Where each answer files it.
    pub owners: Vec<TraceOwner>,
    pub default: usize,
    pub hint: String,
}

/// The question for moving `name`, filed under `current` now.
pub fn move_choices(name: &str, current: TraceOwner, meas: &[&Measurement]) -> MoveChoices {
    let mut ms: Vec<&&Measurement> = meas
        .iter()
        .filter(|m| !matches!(m.config.kind, MeasKind::Math { .. }))
        .collect();
    ms.sort_by_key(|m| m.id);
    let mut choices = Vec::new();
    let mut owners = Vec::new();
    for m in ms {
        choices.push(Choice {
            label: format!("{}  {}", kind_tag(&m.config.kind), m.config.name),
            blocked: None,
        });
        owners.push(TraceOwner::Meas { meas: m.id });
    }
    choices.push(Choice {
        label: "Imported (under no measurement)".to_owned(),
        blocked: None,
    });
    owners.push(TraceOwner::Imported);
    for (c, o) in choices.iter_mut().zip(&owners) {
        if *o == current {
            c.blocked = Some(format!("{name} is filed there now"));
        }
    }
    let default = choices
        .iter()
        .position(|c| c.blocked.is_none())
        .unwrap_or(0);
    MoveChoices {
        title: format!("Move {name} to…"),
        lines: vec![
            "Only where it is listed changes: its curve, name and settings stay.".to_owned(),
        ],
        choices,
        owners,
        default,
        hint: "↑/↓ choose · Enter moves it · Esc cancels".to_owned(),
    }
}

/// What the confirmation before deleting measurement `m` says.
pub fn delete_confirm(m: &Measurement) -> DeleteConfirm {
    DeleteConfirm {
        title: format!("Delete measurement {}?", m.config.name),
        lines: vec![
            format!("{} · {}", kind_name(&m.config.kind), state_word(m)),
            "Its live curve and settings go; it owns no stored traces.".to_owned(),
        ],
        hint: "Delete or Backspace deletes it · Esc or N keeps it".to_owned(),
        refused: false,
    }
}

/// What shows in the confirmation's place when math channels `users` compute from `m`: the
/// daemon refuses deleting an operand, so the window says which channel to change first.
pub fn delete_refused(m: &Measurement, users: &[String]) -> DeleteConfirm {
    let name = &m.config.name;
    let (which, them, computes) = match users {
        [one] => (
            format!("the math channel {one}"),
            one.clone(),
            "it computes",
        ),
        _ => (
            format!("the math channels {}", users.join(", ")),
            "them".to_owned(),
            "they compute",
        ),
    };
    DeleteConfirm {
        title: format!("{name} cannot be deleted"),
        lines: vec![
            format!("{} · an operand of {which}", kind_name(&m.config.kind)),
            format!("Edit or delete {them} first: {computes} from {name}."),
        ],
        hint: "Enter or Esc closes".to_owned(),
        refused: true,
    }
}
