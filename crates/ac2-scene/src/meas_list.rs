//! The list of live measurements beside the panes, and what deleting one asks: each
//! measurement's row (its kind, name, state and display edits), whether the keys act on it,
//! and the confirmation (or the refusal) Delete shows.

use ac2_proto::model::{MeasKind, Measurement};
use ac2_proto::units::MeasId;

use crate::format;
use crate::trace_list::DeleteConfirm;

/// The short tag a row starts with: `TF`, `FFT`, `RTA`, `SPL`, `MATH`.
pub fn kind_tag(k: &MeasKind) -> &'static str {
    match k {
        MeasKind::Transfer { .. } => "TF",
        MeasKind::Spectrum { .. } => "FFT",
        MeasKind::Rta { .. } => "RTA",
        MeasKind::Spl { .. } => "SPL",
        MeasKind::Math { .. } => "MATH",
    }
}

/// What a measurement is, in words: `transfer function`, `math channel` …
pub fn kind_name(k: &MeasKind) -> &'static str {
    match k {
        MeasKind::Transfer { .. } => "transfer function",
        MeasKind::Spectrum { .. } => "spectrum",
        MeasKind::Rta { .. } => "RTA",
        MeasKind::Spl { .. } => "SPL meter",
        MeasKind::Math { .. } => "math channel",
    }
}

/// `running`, `stopped`, `frozen`.
pub fn state_word(m: &Measurement) -> &'static str {
    match (m.running, m.frozen) {
        (_, true) => "frozen",
        (true, false) => "running",
        (false, false) => "stopped",
    }
}

/// One measurement as the list gets it: the measurement and this app's display of it.
#[derive(Clone, Debug)]
pub struct MeasItem<'a> {
    pub meas: &'a Measurement,
    /// A math channel's expression by its operands' names ([`crate::math::expression`]).
    pub expression: Option<String>,
    /// Display offset, dB (this app's).
    pub offset_db: f64,
    /// Drawn inverted (this app's).
    pub inverted: bool,
    /// Its live curves are hidden in this app (it keeps measuring).
    pub hidden: bool,
}

/// What the list highlights on a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    None,
    /// The selected measurement, while a stored trace selected after it has the keys.
    Selected,
    /// The selected measurement, and the keys act on it.
    Active,
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct MeasRow {
    pub id: MeasId,
    /// `TF  Main L` over `running · 12.34 ms · tracking`.
    pub text: String,
    pub hidden: bool,
    pub mark: Mark,
}

/// The row of `item`. `selected`: the selected measurement; `active`: the keys act on the
/// measurement (no stored trace was selected after it).
pub fn meas_row(item: &MeasItem<'_>, selected: Option<MeasId>, active: bool) -> MeasRow {
    let m = item.meas;
    let mut text = format!(
        "{}  {}\n     {}",
        kind_tag(&m.config.kind),
        m.config.name,
        state_word(m)
    );
    // Right after the state: a curve missing from the panes must not read as a fault.
    if item.hidden {
        text.push_str(" · hidden");
    }
    if let Some(d) = &m.delay {
        // Distance stays in the transfer legend's reference line.
        text.push_str(&format!(" · {}", format::delay(d.applied.0)));
        if d.tracking && d.awaiting_pick {
            text.push_str(" · tracking paused");
        } else if d.tracking {
            text.push_str(" · tracking");
        }
    }
    match &m.config.kind {
        MeasKind::Transfer { config } if config.smoothing.is_some() => {
            text.push_str(&format!(" · {}", format::smoothing(config.smoothing)));
        }
        MeasKind::Spectrum { config } => {
            if let Some(f) = config.smoothing {
                text.push_str(&format!(" · smoothed {}", format::octave_fraction(f)));
            }
        }
        _ => {}
    }
    if let Some(e) = item.expression.as_ref().filter(|e| **e != m.config.name) {
        text.push_str(&format!(" · {e}"));
    }
    if item.inverted {
        text.push_str(" · inv");
    }
    if item.offset_db != 0.0 {
        text.push_str(&format!(" · {}", format::db_readout(item.offset_db)));
    }
    let mark = match (selected == Some(m.id), active) {
        (false, _) => Mark::None,
        (true, false) => Mark::Selected,
        (true, true) => Mark::Active,
    };
    MeasRow {
        id: m.id,
        text,
        hidden: item.hidden,
        mark,
    }
}

/// What the confirmation before deleting measurement `m` says.
pub fn delete_confirm(m: &Measurement) -> DeleteConfirm {
    DeleteConfirm {
        title: format!("Delete measurement {}?", m.config.name),
        lines: vec![
            format!("{} · {}", kind_name(&m.config.kind), state_word(m)),
            "Its live curve and settings go; captured traces stay.".to_owned(),
        ],
        hint: "Delete, Backspace or Enter deletes it · Esc or N keeps it".to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::*;
    use ac2_proto::units::*;

    fn tf(id: u32, name: &str) -> Measurement {
        Measurement {
            id: MeasId(id),
            config: MeasConfig {
                name: name.into(),
                kind: MeasKind::Transfer {
                    config: TransferConfig {
                        reference_input: 0,
                        measurement_input: 1,
                        averaging: TfAveraging::Exponential {
                            time_constant: Seconds(1.0),
                        },
                        grid: LogGridSpec {
                            ppo: 48,
                            k_min: -240,
                            k_max: 216,
                        },
                        smoothing: None,
                        depth: DepthPolicy::EqualConfidence,
                    },
                },
            },
            config_rev: Rev(1),
            running: true,
            frozen: false,
            delay: None,
            grid_id: None,
        }
    }

    fn item(m: &Measurement) -> MeasItem<'_> {
        MeasItem {
            meas: m,
            expression: None,
            offset_db: 0.0,
            inverted: false,
            hidden: false,
        }
    }

    #[test]
    fn rows_name_the_state_and_say_hidden() {
        let m = tf(2, "TF 2");
        let r = meas_row(&item(&m), Some(MeasId(2)), true);
        assert_eq!(r.text, "TF  TF 2\n     running");
        assert_eq!(r.mark, Mark::Active);
        let hidden = MeasItem {
            hidden: true,
            offset_db: 3.0,
            inverted: true,
            ..item(&m)
        };
        let r = meas_row(&hidden, Some(MeasId(2)), false);
        assert_eq!(r.text, "TF  TF 2\n     running · hidden · inv · +3.0 dB");
        assert!(r.hidden);
        assert_eq!(r.mark, Mark::Selected, "a stored trace has the keys");
        assert_eq!(meas_row(&item(&m), Some(MeasId(1)), true).mark, Mark::None);
    }

    #[test]
    fn delete_asks_and_a_math_operand_refuses() {
        let m = tf(2, "TF 2");
        let c = delete_confirm(&m);
        assert_eq!(c.title, "Delete measurement TF 2?");
        assert_eq!(
            c.lines,
            [
                "transfer function · running",
                "Its live curve and settings go; captured traces stay."
            ]
        );
        assert_eq!(
            c.hint,
            "Delete, Backspace or Enter deletes it · Esc or N keeps it"
        );
        assert!(!c.refused);
        let r = delete_refused(&m, &["Avg".to_owned()]);
        assert_eq!(r.title, "TF 2 cannot be deleted");
        assert_eq!(
            r.lines,
            [
                "transfer function · an operand of the math channel Avg",
                "Edit or delete Avg first: it computes from TF 2."
            ]
        );
        assert_eq!(r.hint, "Enter or Esc closes");
        assert!(r.refused);
        let r = delete_refused(&m, &["Avg".to_owned(), "Sum".to_owned()]);
        assert_eq!(
            r.lines[0],
            "transfer function · an operand of the math channels Avg, Sum"
        );
        assert_eq!(
            r.lines[1],
            "Edit or delete them first: they compute from TF 2."
        );
    }
}
