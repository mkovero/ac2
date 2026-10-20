//! The measurement tree beside the panes (`docs/design/measurement-tree.md`): each
//! measurement with what it owns under it — its live curve, its stored traces, the math
//! channels made on it — then the imported group; whether the keys act on a row; and what
//! Delete asks (the confirmation, the three choices for a measurement that owns traces, or
//! the refusal).

use std::collections::BTreeSet;

use ac2_proto::model::{
    CueState, MeasKind, Measurement, Resolution, Rig, SpecAveraging, SpecResolution, SweepRun,
    TraceMeta, TraceOwner, TraceSource,
};
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

/// `running`, `stopped`.
pub fn state_word(m: &Measurement) -> &'static str {
    if m.running { "running" } else { "stopped" }
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
    /// `TF  Main L` over `running · delay 12.34 ms (+0.02 ms from arrival) · tracking`.
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
        // One delay and how far it is from the measured arrival, as the legend and the
        // delay toasts say it. Distance stays in the transfer legend's reference line.
        text.push_str(&format!(
            " · {}",
            format::meas_delay(d.applied.0, d.nudged.0)
        ));
    }
    if let Some(d) = &m.delay {
        if d.tracking && d.awaiting_pick {
            text.push_str(" · tracking paused");
        } else if d.tracking {
            text.push_str(" · tracking");
        }
    }
    // A resolution other than the default is said: it changes what the curve can show.
    let resolution = match &m.config.kind {
        MeasKind::Transfer { config } => Some(config.resolution),
        MeasKind::Sweep { config } => Some(config.resolution),
        _ => None,
    };
    if let Some(r) = resolution.filter(|r| *r != Resolution::default()) {
        text.push_str(&format!(" · resolution {}", format::resolution(r)));
    }
    match &m.config.kind {
        MeasKind::Transfer { config } if config.smoothing.is_some() => {
            text.push_str(&format!(" · {}", format::smoothing(config.smoothing)));
        }
        MeasKind::Spectrum { config } => {
            // One FFT is the default and its size is in the dialog; multi-resolution
            // changes how each part of the curve behaves, so the row says it.
            if matches!(config.resolution, SpecResolution::Multi { .. }) {
                text.push_str(&format!(
                    " · {}",
                    format::spec_resolution(config.resolution)
                ));
            }
            if let Some(l) = format::spec_level_shown(config.resolution, config.level) {
                text.push_str(&format!(" · {l}"));
            }
            if config.averaging != SpecAveraging::Off {
                text.push_str(&format!(
                    " · avg {}",
                    format::spec_averaging(config.averaging)
                ));
            }
            if let Some(f) = config.smoothing {
                text.push_str(&format!(" · smoothed {}", format::octave_fraction(f)));
            }
        }
        MeasKind::Rta { config } if config.averaging != SpecAveraging::Off => {
            text.push_str(&format!(
                " · avg {}",
                format::spec_averaging(config.averaging)
            ));
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
    // Last and quiet: selecting it brings its own panes, which the operator should
    // expect before the screen changes under them.
    if let Some(l) = &m.layout {
        text.push_str(&format!(" · {}", format::own_panes(l.panes().len())));
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
    /// The rig group's header, over the meters the rig keeps across session loads: opens
    /// the Rig monitor settings.
    Rig,
    /// The header of the session's measurements, under the rig group.
    Session,
}

impl TreeKey {
    /// A header that stands for no measurement or curve: nothing to select, drag or show.
    pub fn is_heading(self) -> bool {
        matches!(self, TreeKey::Imported | TreeKey::Rig | TreeKey::Session)
    }
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
    /// A meter's OSC cue state ([`crate::osc::meter_state`]), shown as a chip.
    pub chip: Option<CueState>,
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
    /// The keys act on the selected measurement through its live-curve row (Tab or a click
    /// picked that row, not the header): the row is marked, its header outlined.
    pub live_selected: bool,
    /// The latest sweep run (a sweep measurement's header says it plays).
    pub sweep: Option<&'a SweepRun>,
    /// Rows compared (live curves, math channels, stored traces): marked
    /// [`crate::trace::COMPARED`], as the legend tags their curves.
    pub compared: Vec<TreeKey>,
    /// The rig entity: which meters' cues are in which state, where cues go.
    pub rig: Option<&'a Rig>,
}

/// The group a trace is listed in: its owner, or the imported group when its owner is gone.
pub fn group_of(t: &TraceMeta, meas: &[&Measurement]) -> TraceOwner {
    match t.edit.owner {
        TraceOwner::Meas { meas: id } if meas.iter().any(|m| m.id == id) => t.edit.owner,
        _ => TraceOwner::Imported,
    }
}

/// The group a math channel is listed in (`None`: not a math channel).
pub(crate) fn math_group(m: &Measurement, meas: &[&Measurement]) -> Option<TraceOwner> {
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

/// The group a measurement is listed in: its own for one with a live curve or runs, its
/// owner's for a math channel (Imported once that owner is gone).
pub fn meas_group(m: &Measurement, meas: &[&Measurement]) -> TraceOwner {
    math_group(m, meas).unwrap_or(TraceOwner::Meas { meas: m.id })
}

/// What a pane says while it draws no stored trace of its measurement's group but `n` shown
/// traces of a kind it draws wait under Imported: a pane draws only its measurement's group
/// (and what is compared from other measurements), so they show once moved under one. `move_key` is the chord of "Move to measurement…".
pub fn imported_hint(n: usize, move_key: Option<&str>) -> String {
    let what = if n == 1 {
        "1 trace under Imported is".to_owned()
    } else {
        format!("{n} traces under Imported are")
    };
    let how = move_key.map_or_else(
        || "Move to measurement…".to_owned(),
        |k| format!("Move to measurement… ({k})"),
    );
    format!("{what} not on this pane — select it, then {how}")
}

/// What a compare toggle says: `Spectrum 2 compared into Spectrum 1 (cmp)`, or that it is
/// off there. A hidden curve is compared but not drawn until shown.
pub fn compare_toast(name: &str, target: &str, on: bool, shown: bool) -> String {
    let tag = crate::trace::COMPARED;
    match (on, shown) {
        (false, _) => format!("{name}: no longer compared into {target}"),
        (true, true) => format!("{name} compared into {target} ({tag})"),
        (true, false) => {
            format!("{name} compared into {target} ({tag}) — hidden: A shows it")
        }
    }
}

/// A measurement C offers to compare a curve into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompareTarget {
    pub name: String,
    /// A pane on screen shows it.
    pub shown: bool,
    /// The curve is compared into it already.
    pub on: bool,
}

/// The question C asks: which measurement's panes draw the curve. A tick marks those it is
/// compared into; Enter adds or removes the highlighted one and the list stays open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompareChoices {
    /// `Compare Spectrum 2 with…`
    pub title: String,
    pub lines: Vec<String>,
    /// One per target, in the order given (those on screen first).
    pub choices: Vec<Choice>,
    pub hint: String,
}

/// Mark of a target the curve is compared into.
pub const COMPARE_TICK: &str = "\u{2713}";

/// The compare chooser for curve `name` over `targets` (on screen first).
pub fn compare_choices(name: &str, targets: &[CompareTarget]) -> CompareChoices {
    CompareChoices {
        title: format!("Compare {name} with…"),
        lines: vec![format!(
            "{name} is drawn over the panes of each measurement ticked, under its own curves."
        )],
        choices: targets
            .iter()
            .map(|t| Choice {
                label: format!(
                    "{} {}{}",
                    if t.on { COMPARE_TICK } else { " " },
                    t.name,
                    if t.shown { " · on screen" } else { "" }
                ),
                blocked: None,
            })
            .collect(),
        hint: "Enter adds or removes · Esc closes".to_owned(),
    }
}

/// What C says when no measurement's panes could draw curve `name` (`what`: `spectrum or
/// RTA`, `transfer`, `sweep`).
pub fn compare_no_target(name: &str, what: &str) -> String {
    format!("{name}: no other {what} measurement to compare it with — make one first")
}

/// What C on a stored trace under Imported says: compare draws over a measurement's
/// panes, and an import is no measurement's until moved under one.
pub fn imported_compare_hint(name: &str, move_key: Option<&str>) -> String {
    let how = move_key.map_or_else(
        || "Move to measurement…".to_owned(),
        |k| format!("Move to measurement… ({k})"),
    );
    format!("{name} is under Imported: it is no measurement's to compare — select it, then {how}")
}

/// What C on an SPL meter says: a meter is a number, compared by seeing two side by side.
pub fn meter_compare_hint(name: &str) -> String {
    format!(
        "{name}: meters are not compared — add a pane and drag the meter onto it to see two side by side"
    )
}

/// The groups in tree order: the measurements that are not math channels — the rig's
/// first, then the session's, each by id — then the imported group.
pub fn group_order(meas: &[&Measurement]) -> Vec<TraceOwner> {
    let mut ids: Vec<(bool, MeasId)> = meas
        .iter()
        .filter(|m| !matches!(m.config.kind, MeasKind::Math { .. }))
        .map(|m| (!crate::osc::on_rig(m), m.id))
        .collect();
    ids.sort();
    ids.into_iter()
        .map(|(_, id)| id)
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
    let live_selected = input.live_selected && !input.keys_on_trace;
    let mark = |id: MeasId| match (
        input.selected == Some(id),
        input.keys_on_trace || live_selected,
    ) {
        (false, _) => Mark::None,
        (true, true) => Mark::Selected,
        (true, false) => Mark::Active,
    };
    let mut rows = Vec::new();
    let rig_meters = meas.iter().filter(|m| crate::osc::on_rig(m)).count();
    let (mut rig_said, mut session_said) = (false, false);
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
                mark: if live_selected && input.selected == Some(m.id) {
                    Mark::Active
                } else {
                    Mark::None
                },
                dot: Some((item.color, !item.hidden)),
                chip: None,
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
                chip: None,
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
            let state = if m.running {
                "math (live)"
            } else {
                "math · stopped"
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
                chip: None,
            });
        }
        for c in &mut children {
            if input.compared.contains(&c.key) {
                mark_compared(c);
            }
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
                let chip = input.rig.and_then(|r| crate::osc::meter_state(r, m.id));
                let describe = match chip {
                    Some(c) => format!(
                        "{} {}: {detail}, OSC cue {}",
                        kind_name(&m.config.kind),
                        m.config.name,
                        crate::osc::state_word(c)
                    ),
                    None => format!("{} {}: {detail}", kind_name(&m.config.kind), m.config.name),
                };
                TreeRow {
                    key: TreeKey::Meas(m.id),
                    depth: 0,
                    last: false,
                    name: format!("{}  {}", kind_tag(&m.config.kind), m.config.name),
                    describe,
                    details: vec![detail],
                    collapsed: Some(folded),
                    hidden,
                    mark: mark(m.id),
                    dot: None,
                    chip,
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
                    chip: None,
                }
            }
        };
        let rig_group = header_item.is_some_and(|i| crate::osc::on_rig(i.meas));
        if rig_group && !rig_said {
            rig_said = true;
            rows.push(rig_header(rig_meters, input.rig));
        } else if !rig_group && rig_said && !session_said {
            session_said = true;
            rows.push(heading_row(
                TreeKey::Session,
                crate::osc::SESSION,
                vec!["saved and loaded with the session".into()],
                "Session: the measurements saved and loaded with the session".into(),
            ));
        }
        rows.push(header);
        if !folded {
            rows.extend(children);
        }
    }
    rows
}

/// The rig group's header: its meters and where their cues go.
fn rig_header(meters: usize, rig: Option<&Rig>) -> TreeRow {
    let rig = rig.cloned().unwrap_or_default();
    let details = crate::osc::rig_details(meters, &rig);
    let describe = format!(
        "Rig: meters kept by the rig across session loads, {}; opens the Rig monitor settings",
        crate::osc::status_line(&rig)
    );
    heading_row(TreeKey::Rig, crate::osc::RIG, details, describe)
}

/// A header that stands for no measurement: the rig's, the session's.
fn heading_row(key: TreeKey, name: &str, details: Vec<String>, describe: String) -> TreeRow {
    TreeRow {
        key,
        depth: 0,
        last: false,
        name: name.to_owned(),
        details,
        collapsed: None,
        hidden: false,
        mark: Mark::None,
        dot: None,
        chip: None,
        describe,
    }
}

/// A compared row says so the way the legend does: ` · cmp` after each detail line.
fn mark_compared(r: &mut TreeRow) {
    let tag = crate::trace::COMPARED;
    for d in &mut r.details {
        d.push_str(&format!(" · {tag}"));
    }
    if r.details.is_empty() {
        r.details.push(tag.to_owned());
    }
    r.describe.push_str(&format!(", compared ({tag})"));
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

#[cfg(test)]
pub(crate) mod tests;
