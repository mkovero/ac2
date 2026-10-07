//! The measurement tree beside the panes (`docs/design/measurement-tree.md`): each
//! measurement with what it owns under it — its live curve, its stored traces, the math
//! channels made on it — then the imported group; whether the keys act on a row; and what
//! Delete asks (the confirmation, the three choices for a measurement that owns traces, or
//! the refusal).

use std::collections::BTreeSet;

use ac2_proto::model::{MeasKind, Measurement, SweepRun, TraceMeta, TraceOwner, TraceSource};
use ac2_proto::units::{MeasId, TraceId};

use crate::format;
use crate::primitives::Color;
use crate::trace_list::{self, DeleteConfirm, TraceItem};

/// The short tag a row starts with: `TF`, `FFT`, `RTA`, `SPL`, `MATH`, `SWEEP`.
pub fn kind_tag(k: &MeasKind) -> &'static str {
    match k {
        MeasKind::Transfer { .. } => "TF",
        MeasKind::Spectrum { .. } => "FFT",
        MeasKind::Rta { .. } => "RTA",
        MeasKind::Spl { .. } => "SPL",
        MeasKind::Math { .. } => "MATH",
        MeasKind::Sweep { .. } => "SWEEP",
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
        MeasKind::Sweep { .. } => "sweep measurement",
    }
}

/// Whether a measurement of kind `k` has a live curve of its own in the tree (an SPL meter
/// shows a reading, a sweep measurement only its runs).
pub fn has_live_curve(k: &MeasKind) -> bool {
    matches!(
        k,
        MeasKind::Transfer { .. } | MeasKind::Spectrum { .. } | MeasKind::Rta { .. }
    )
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
    /// The colour its curve is drawn in, in every pane and legend: its row's dot has it.
    pub color: Color,
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

/// A sweep measurement's state: `playing 1/2`, `analysing`, `2 runs`, `no runs yet`.
fn sweep_state(m: &Measurement, runs: usize, sweep: Option<&SweepRun>) -> String {
    match sweep.filter(|r| r.meas == m.id && r.active()) {
        Some(SweepRun {
            status: ac2_proto::model::SweepStatus::Playing { repeat },
            repeats,
            ..
        }) => format!("playing {repeat}/{repeats}"),
        Some(_) => "analysing".to_owned(),
        None => match runs {
            0 => "no runs yet".to_owned(),
            1 => "1 run".to_owned(),
            n => format!("{n} runs"),
        },
    }
}

/// What a tree row stands for (what a click on it selects).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKey {
    /// A measurement's header: selects the measurement; its arrow folds the group.
    Meas(MeasId),
    /// A measurement's live curve: selects the measurement.
    Live(MeasId),
    /// A stored trace.
    Trace(TraceId),
    /// A math channel under the measurement it was made on: selects the math channel.
    Math(MeasId),
    /// The imported group's header.
    Imported,
}

/// One row of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeRow {
    pub key: TreeKey,
    /// 0: a group's header; 1: a row under it.
    pub depth: u8,
    /// The last row of its group (`└`, else `├`).
    pub last: bool,
    /// A header: `TF  Main L`; under it: the curve's name (`Main L (live)`, `pre-EQ`).
    pub name: String,
    /// The line under the name, longest first: the view shows the first that fits.
    pub details: Vec<String>,
    /// A header: its group folded (its rows not listed). `None` for rows under a header.
    pub collapsed: Option<bool>,
    /// Drawn dimmed: the curve (or every curve of the group) is hidden in the panes.
    pub hidden: bool,
    pub mark: Mark,
    /// The colour of the curve the row stands for (its live curve, a stored trace or sweep
    /// run, a math channel's result), as the panes draw it, and whether it is shown: filled
    /// when shown, a ring when hidden, and a click on it toggles that. Headers have none:
    /// they stand for a group, not a curve. A curve no visible pane draws right now keeps
    /// its colour, so it reads the same when its pane comes back.
    pub dot: Option<(Color, bool)>,
    /// Everything about the row in one sentence (the tooltip, and what a screen reader says).
    pub describe: String,
}

/// What the tree is built from.
#[derive(Debug)]
pub struct TreeInput<'a> {
    /// Every measurement, math channels included.
    pub meas: Vec<MeasItem<'a>>,
    pub traces: Vec<TraceItem<'a>>,
    /// Folded groups.
    pub collapsed: &'a BTreeSet<TraceOwner>,
    pub selected: Option<MeasId>,
    pub selected_trace: Option<TraceId>,
    /// The keys act on the selected stored trace (it was selected after the measurement).
    pub keys_on_trace: bool,
    /// The latest sweep run (a sweep measurement's header says it plays).
    pub sweep: Option<&'a SweepRun>,
}

/// The group a trace is listed in: its owner, or the imported group when its owner is gone.
pub fn group_of(t: &TraceMeta, meas: &[&Measurement]) -> TraceOwner {
    match t.edit.owner {
        TraceOwner::Meas { meas: id } if meas.iter().any(|m| m.id == id) => t.edit.owner,
        _ => TraceOwner::Imported,
    }
}

/// The group a math channel is listed in (`None`: not a math channel).
fn math_group(m: &Measurement, meas: &[&Measurement]) -> Option<TraceOwner> {
    let MeasKind::Math { config } = &m.config.kind else {
        return None;
    };
    Some(match config.owner {
        TraceOwner::Meas { meas: id }
            if meas
                .iter()
                .any(|x| x.id == id && !matches!(x.config.kind, MeasKind::Math { .. })) =>
        {
            config.owner
        }
        _ => TraceOwner::Imported,
    })
}

/// The groups in tree order: the measurements that are not math channels by id, then the
/// imported group.
pub fn group_order(meas: &[&Measurement]) -> Vec<TraceOwner> {
    let mut ids: Vec<MeasId> = meas
        .iter()
        .filter(|m| !matches!(m.config.kind, MeasKind::Math { .. }))
        .map(|m| m.id)
        .collect();
    ids.sort();
    ids.into_iter()
        .map(|meas| TraceOwner::Meas { meas })
        .chain(std::iter::once(TraceOwner::Imported))
        .collect()
}

/// Stored traces in tree order (group by group, each in [`trace_list::sort_key`] order): the
/// order V steps through and pane legends follow.
pub fn trace_order<'t>(meas: &[&Measurement], traces: &[&'t TraceMeta]) -> Vec<&'t TraceMeta> {
    let mut out = Vec::with_capacity(traces.len());
    for g in group_order(meas) {
        let mut v: Vec<&TraceMeta> = traces
            .iter()
            .copied()
            .filter(|t| group_of(t, meas) == g)
            .collect();
        v.sort_by_key(|t| trace_list::sort_key(t));
        out.extend(v);
    }
    out
}

/// The tree's rows, top to bottom.
pub fn tree_rows(input: &TreeInput<'_>) -> Vec<TreeRow> {
    let meas: Vec<&Measurement> = input.meas.iter().map(|i| i.meas).collect();
    let trace_rows = trace_list::trace_rows(
        &input.traces,
        input.selected_trace.filter(|_| input.keys_on_trace),
    );
    let mark = |id: MeasId| match (input.selected == Some(id), input.keys_on_trace) {
        (false, _) => Mark::None,
        (true, true) => Mark::Selected,
        (true, false) => Mark::Active,
    };
    let mut rows = Vec::new();
    for g in group_order(&meas) {
        let mut children: Vec<TreeRow> = Vec::new();
        let header_item = g
            .meas()
            .and_then(|id| input.meas.iter().find(|i| i.meas.id == id));
        if let Some(item) = header_item.filter(|i| has_live_curve(&i.meas.config.kind)) {
            let m = item.meas;
            let state = if item.hidden {
                "live · hidden"
            } else {
                "live"
            };
            children.push(TreeRow {
                key: TreeKey::Live(m.id),
                depth: 1,
                last: false,
                name: format!("{} (live)", m.config.name),
                details: vec![state.to_owned()],
                collapsed: None,
                hidden: item.hidden,
                mark: Mark::None,
                dot: Some((item.color, !item.hidden)),
                describe: format!("{}: the live curve, {state}", m.config.name),
            });
        }
        let owned: Vec<&TraceMeta> = input
            .traces
            .iter()
            .map(|i| i.meta)
            .filter(|t| group_of(t, &meas) == g)
            .collect();
        for t in trace_order(&meas, &owned) {
            let Some(r) = trace_rows.iter().find(|r| r.id == t.id) else {
                continue;
            };
            children.push(TreeRow {
                key: TreeKey::Trace(r.id),
                depth: 1,
                last: false,
                name: r.name.clone(),
                details: r.details.clone(),
                collapsed: None,
                hidden: !r.shown,
                mark: if r.selected { Mark::Active } else { Mark::None },
                dot: Some((r.color, r.shown)),
                describe: r.describe.clone(),
            });
        }
        let mut maths: Vec<&MeasItem<'_>> = input
            .meas
            .iter()
            .filter(|i| math_group(i.meas, &meas) == Some(g))
            .collect();
        maths.sort_by_key(|i| i.meas.id);
        for item in maths {
            let m = item.meas;
            let state = match (m.running, m.frozen) {
                (_, true) => "math · frozen",
                (true, false) => "math (live)",
                (false, false) => "math · stopped",
            };
            let mut long = state.to_owned();
            if item.hidden {
                long.push_str(" · hidden");
            }
            let mut details = Vec::new();
            if let Some(e) = item.expression.as_ref().filter(|e| **e != m.config.name) {
                details.push(format!("{long} · {e}"));
            }
            details.push(long.clone());
            if long != state {
                details.push(state.to_owned());
            }
            children.push(TreeRow {
                key: TreeKey::Math(m.id),
                depth: 1,
                last: false,
                name: m.config.name.clone(),
                describe: format!("{}: {}", m.config.name, details[0]),
                details,
                collapsed: None,
                hidden: item.hidden,
                mark: mark(m.id),
                dot: Some((item.color, !item.hidden)),
            });
        }
        if let Some(c) = children.last_mut() {
            c.last = true;
        }
        let folded = input.collapsed.contains(&g);
        let n = children.len();
        let header = match header_item {
            Some(item) => {
                let m = item.meas;
                let runs = owned
                    .iter()
                    .filter(
                        |t| matches!(&t.source, TraceSource::Sweep { meas, .. } if *meas == m.id),
                    )
                    .count();
                let mut detail = match &m.config.kind {
                    MeasKind::Sweep { .. } => sweep_state(m, runs, input.sweep),
                    _ => meas_row(item, input.selected, !input.keys_on_trace)
                        .text
                        .split_once('\n')
                        .map_or_else(String::new, |(_, d)| d.trim().to_owned()),
                };
                if folded && n > 0 {
                    detail.push_str(&format!(" · {n} folded"));
                }
                let hidden = item.hidden
                    || (!has_live_curve(&m.config.kind)
                        && n > 0
                        && children.iter().all(|c| c.hidden));
                TreeRow {
                    key: TreeKey::Meas(m.id),
                    depth: 0,
                    last: false,
                    name: format!("{}  {}", kind_tag(&m.config.kind), m.config.name),
                    describe: format!("{} {}: {detail}", kind_name(&m.config.kind), m.config.name),
                    details: vec![detail],
                    collapsed: Some(folded),
                    hidden,
                    mark: mark(m.id),
                    dot: None,
                }
            }
            None => {
                // The imported group is listed only when it holds something.
                if n == 0 {
                    continue;
                }
                let what = if n == 1 {
                    "1 trace".to_owned()
                } else {
                    format!("{n} traces")
                };
                let detail = if folded {
                    format!("{what} · folded")
                } else {
                    what
                };
                TreeRow {
                    key: TreeKey::Imported,
                    depth: 0,
                    last: false,
                    name: "Imported".to_owned(),
                    describe: format!("Imported: {detail}, under no measurement"),
                    details: vec![detail],
                    collapsed: Some(folded),
                    hidden: false,
                    mark: Mark::None,
                    dot: None,
                }
            }
        };
        rows.push(header);
        if !folded {
            rows.extend(children);
        }
    }
    rows
}

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
            color: Color::from_rgba8([0, 0, u8::try_from(m.id.0).unwrap_or(0), 255]),
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
                "Its live curve and settings go; it owns no stored traces."
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

    fn with_kind(id: u32, name: &str, kind: MeasKind, running: bool) -> Measurement {
        Measurement {
            id: MeasId(id),
            config: MeasConfig {
                name: name.into(),
                kind,
            },
            running,
            ..tf(id, name)
        }
    }

    fn sweep_config() -> SweepConfig {
        SweepConfig {
            reference_input: 0,
            measurement_input: 1,
            outputs: vec![0],
            level: Dbfs(-50.0),
            sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
            repeats: 1,
            gate: None,
            tail: None,
        }
    }

    fn trace(id: u32, name: &str, owner: TraceOwner, source: TraceSource) -> TraceMeta {
        TraceMeta {
            id: TraceId(id),
            edit: TraceEdit {
                name: name.into(),
                color: Rgb { r: 1, g: 2, b: 3 },
                visible: true,
                locked: false,
                order: id,
                offset: Db(0.0),
                polarity: Polarity::Normal,
                delay_nudge: Seconds(0.0),
                slot: None,
                smoothing: None,
                owner,
            },
            kind: if matches!(source, TraceSource::Sweep { .. }) {
                TraceKind::Sweep
            } else {
                TraceKind::Transfer
            },
            source,
            grid_id: ac2_proto::GridId(1),
            delay: Seconds(0.0),
            depth: None,
            cal: CalState::Uncalibrated,
            mic: None,
            mic_curve: None,
            created_at: WallNs(0),
        }
    }

    fn captured() -> TraceSource {
        TraceSource::Captured {
            meas: MeasId(1),
            meas_name: "Main L".into(),
            epoch: SessionEpoch(1),
            at_sample: SampleIndex(0),
        }
    }

    fn run(number: u32) -> TraceSource {
        TraceSource::Sweep {
            meas: MeasId(2),
            meas_name: "Genelec 1 m".into(),
            run: SweepId(number),
            number,
            epoch: SessionEpoch(1),
            sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
            level: Dbfs(-50.0),
            repeats: 1,
            reference_input: 0,
            measurement_input: 1,
        }
    }

    /// The operator's example: a transfer function with two captures and math made on it, a
    /// sweep measurement with two runs, an SPL meter, an imported file.
    struct Rig {
        meas: Vec<Measurement>,
        traces: Vec<TraceMeta>,
    }

    fn rig() -> Rig {
        let main = TraceOwner::Meas { meas: MeasId(1) };
        let math = MathConfig::of(
            main,
            MathDomain::Transfer,
            MathExpr::Binary {
                a: Operand::Trace { trace: TraceId(1) },
                op: MathOp::Divide,
                b: Operand::Trace { trace: TraceId(2) },
            },
        );
        Rig {
            meas: vec![
                tf(1, "Main L"),
                with_kind(
                    2,
                    "Genelec 1 m",
                    MeasKind::Sweep {
                        config: sweep_config(),
                    },
                    false,
                ),
                with_kind(
                    3,
                    "FOH SPL",
                    MeasKind::Spl {
                        config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
                    },
                    true,
                ),
                with_kind(4, "pre ÷ post", MeasKind::Math { config: math }, true),
            ],
            traces: vec![
                trace(1, "pre-EQ", main, captured()),
                trace(2, "post-EQ", main, captured()),
                trace(3, "Run 1", TraceOwner::Meas { meas: MeasId(2) }, run(1)),
                trace(4, "Run 2", TraceOwner::Meas { meas: MeasId(2) }, run(2)),
                trace(
                    5,
                    "1083 94cm",
                    TraceOwner::Imported,
                    TraceSource::Imported {
                        file_name: "1083 94cm.txt".into(),
                        format: ImportFormat::AnalyzerText,
                        notes: vec![],
                    },
                ),
            ],
        }
    }

    fn rows(
        r: &Rig,
        collapsed: &BTreeSet<TraceOwner>,
        selected: Option<MeasId>,
        trace: Option<TraceId>,
    ) -> Vec<TreeRow> {
        tree_rows(&TreeInput {
            meas: r.meas.iter().map(item).collect(),
            traces: r
                .traces
                .iter()
                .map(|meta| TraceItem {
                    meta,
                    has_data: true,
                })
                .collect(),
            collapsed,
            selected,
            selected_trace: trace,
            keys_on_trace: trace.is_some(),
            sweep: None,
        })
    }

    #[test]
    fn the_tree_lists_each_measurement_with_what_it_owns() {
        let r = rig();
        let none = BTreeSet::new();
        let t = rows(&r, &none, Some(MeasId(1)), None);
        let lines: Vec<String> = t
            .iter()
            .map(|row| {
                let lead = match (row.depth, row.last) {
                    (0, _) => "",
                    (_, false) => "  ├ ",
                    (_, true) => "  └ ",
                };
                format!("{lead}{} · {}", row.name, row.details[0])
            })
            .collect();
        assert_eq!(
            lines,
            [
                "TF  Main L · running",
                "  ├ Main L (live) · live",
                "  ├ pre-EQ · capture",
                "  ├ post-EQ · capture",
                "  └ pre ÷ post · math (live)",
                "SWEEP  Genelec 1 m · 2 runs",
                "  ├ Run 1 · sweep run · 3 s −50.0 dBFS",
                "  └ Run 2 · sweep run · 3 s −50.0 dBFS",
                "SPL  FOH SPL · running",
                "Imported · 1 trace",
                "  └ 1083 94cm · imported",
            ]
        );
        assert_eq!(t[0].mark, Mark::Active);
        assert_eq!(t[0].collapsed, Some(false));
        assert_eq!(t[1].key, TreeKey::Live(MeasId(1)));
        assert_eq!(t[4].key, TreeKey::Math(MeasId(4)));
        // Every row with a curve has a dot in its curve's colour, filled while shown; a
        // header stands for a group and has none.
        let colour = |id: u32| item(r.meas.iter().find(|m| m.id == MeasId(id)).expect("m")).color;
        assert_eq!(t[1].dot, Some((colour(1), true)), "the live curve");
        assert_eq!(t[2].dot, Some((Color::from_rgba8([1, 2, 3, 255]), true)));
        assert_eq!(t[4].dot, Some((colour(4), true)), "the math result");
        for row in &t {
            assert_eq!(row.dot.is_none(), row.depth == 0, "{row:?}");
        }
        // The order V steps through: group by group.
        let metas: Vec<&TraceMeta> = r.traces.iter().rev().collect();
        let ms: Vec<&Measurement> = r.meas.iter().collect();
        let order: Vec<u32> = trace_order(&ms, &metas).iter().map(|t| t.id.0).collect();
        assert_eq!(order, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_trace_selected_after_the_measurement_has_the_keys() {
        let r = rig();
        let none = BTreeSet::new();
        let t = rows(&r, &none, Some(MeasId(1)), Some(TraceId(3)));
        assert_eq!(t[0].mark, Mark::Selected, "outlined: a trace has the keys");
        let run1 = t
            .iter()
            .find(|x| x.key == TreeKey::Trace(TraceId(3)))
            .expect("row");
        assert_eq!(run1.mark, Mark::Active);
        // A math channel selected is marked like a measurement.
        let t = rows(&r, &none, Some(MeasId(4)), None);
        let m = t
            .iter()
            .find(|x| x.key == TreeKey::Math(MeasId(4)))
            .expect("row");
        assert_eq!(m.mark, Mark::Active);
        assert_eq!(t[0].mark, Mark::None);
    }

    #[test]
    fn folded_groups_hide_their_rows_and_say_how_many() {
        let r = rig();
        let folded: BTreeSet<TraceOwner> =
            [TraceOwner::Meas { meas: MeasId(1) }, TraceOwner::Imported].into();
        let t = rows(&r, &folded, None, None);
        let names: Vec<(&str, &str)> = t
            .iter()
            .map(|x| (x.name.as_str(), x.details[0].as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("TF  Main L", "running · 4 folded"),
                ("SWEEP  Genelec 1 m", "2 runs"),
                ("Run 1", "sweep run · 3 s −50.0 dBFS"),
                ("Run 2", "sweep run · 3 s −50.0 dBFS"),
                ("SPL  FOH SPL", "running"),
                ("Imported", "1 trace · folded"),
            ]
        );
        assert_eq!(t[0].collapsed, Some(true));
    }

    #[test]
    fn hidden_curves_read_hidden_and_a_sweep_says_it_plays() {
        let mut r = rig();
        for t in &mut r.traces {
            if t.edit.owner == (TraceOwner::Meas { meas: MeasId(2) }) {
                t.edit.visible = false;
            }
        }
        let none = BTreeSet::new();
        let items: Vec<MeasItem<'_>> = r
            .meas
            .iter()
            .map(|m| MeasItem {
                hidden: m.id == MeasId(1),
                ..item(m)
            })
            .collect();
        let playing = SweepRun {
            id: SweepId(9),
            meas: MeasId(2),
            owner: ClientId("ui".into()),
            name: "Run 3".into(),
            reference_input: 0,
            measurement_input: 1,
            outputs: vec![0],
            level: Dbfs(-50.0),
            sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
            sweep_duration: Seconds(3.0),
            post_roll: Seconds(1.0),
            repeats: 2,
            gate: None,
            status: SweepStatus::Playing { repeat: 1 },
            started_at: WallNs(0),
        };
        let t = tree_rows(&TreeInput {
            meas: items,
            traces: r
                .traces
                .iter()
                .map(|meta| TraceItem {
                    meta,
                    has_data: true,
                })
                .collect(),
            collapsed: &none,
            selected: None,
            selected_trace: None,
            keys_on_trace: false,
            sweep: Some(&playing),
        });
        assert_eq!(t[0].details[0], "running · hidden");
        assert!(t[0].hidden && t[1].hidden);
        assert_eq!(t[1].details[0], "live · hidden");
        assert_eq!(t[1].dot, Some((item(&r.meas[0]).color, false)), "a ring");
        let sweep = t
            .iter()
            .find(|x| x.key == TreeKey::Meas(MeasId(2)))
            .expect("row");
        assert_eq!(sweep.details[0], "playing 1/2");
        assert!(sweep.hidden, "every run of it is hidden");
    }

    #[test]
    fn deleting_a_measurement_with_traces_asks_keep_delete_or_cancel() {
        let m = tf(1, "Main L");
        let c = delete_choices(&m, 3, 0, &[], &[]);
        assert_eq!(c.title, "Delete measurement Main L?");
        assert_eq!(
            c.lines,
            [
                "transfer function · running · it has 3 traces.",
                "Keep: they stay, listed under Imported. Delete: they go with Main L; it cannot be \
                 undone."
            ]
        );
        let labels: Vec<&str> = c.choices.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Keep them (move to Imported)", "Delete them too", "Cancel"]
        );
        assert_eq!(c.default, KEEP);
        assert_eq!(c.hint, "←/→ choose · Enter confirms · Esc cancels");
        let c = delete_choices(&m, 1, 1, &["pre ÷ Main L".into()], &[]);
        assert_eq!(
            c.lines[0],
            "transfer function · running · it has 1 trace and 1 math channel."
        );
        assert_eq!(
            c.choices[KEEP].blocked.as_deref(),
            Some("the math channel pre ÷ Main L would lose an operand")
        );
        assert_eq!(c.default, CANCEL, "Keep is refused: Enter cancels");
        assert_eq!(owned_words(0, 2), "2 math channels");
    }
}
