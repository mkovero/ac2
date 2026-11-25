//! The Dynamics pane (`docs/design/dynamics.md`, "App: the Dynamics pane"): every measured
//! input of every dynamics trace on the pane is a curve; Steps and Table compare two of them,
//! A and B, step by step; Envelopes and Release draw them all. Every number and word the
//! pane shows is made here, from the daemon's step table and envelopes; the UI only says
//! which curves the pane holds and what is picked.
//!
//! A difference is shown only where both values are numbers, the two programmes match, and
//! neither curve's step started from a moved baseline: anything else reads "can't compare"
//! rather than a number that compares unlike things.

use ac2_proto::model::{
    DynamicsEnvelope, DynamicsInfo, DynamicsLevel, DynamicsRow, DynamicsTable, DynamicsTime,
};
use ac2_proto::units::{RigId, TraceId};

pub use ac2_proto::model::layout::{DynamicsMetric, DynamicsStepPick, DynamicsView};

use crate::format::{MINUS, fixed, signed};
use crate::primitives::Color;

mod draw;
mod plots;
#[cfg(test)]
mod tests;

pub use draw::{DynamicsFrame, DynamicsScene, curve_steps, dynamics_scene};

/// One curve: a measured input of a dynamics trace, on the pane's rig (`rig` `None`) or
/// another's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CurveId {
    pub rig: Option<RigId>,
    pub trace: TraceId,
    /// The device input, zero-based.
    pub input: u16,
}

/// Curve A or B as the pane keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// A: the first input of the first trace. B: that trace's second input, else the first
    /// input of the next trace, else none.
    Default,
    /// None (B only).
    Off,
    Curve(CurveId),
}

/// What a Dynamics pane shows of its curves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DynamicsModes {
    pub view: DynamicsView,
    pub a: Side,
    pub b: Side,
    /// `None`: the middle level and the longest burst of curve A.
    pub step: Option<DynamicsStepPick>,
    pub metric: DynamicsMetric,
    /// Envelopes draw THD instead of gain reduction (`D`).
    pub thd: bool,
    /// Release draws every level of curve A (`E`).
    pub all_levels: bool,
}

// The picked step is finite (the pane's layout check refuses anything else), so equality
// is an equivalence.
impl Eq for DynamicsModes {}

impl Default for DynamicsModes {
    fn default() -> Self {
        Self {
            view: DynamicsView::Steps,
            a: Side::Default,
            b: Side::Default,
            step: None,
            metric: DynamicsMetric::GrEnd,
            thd: false,
            all_levels: false,
        }
    }
}

/// One curve as the pane draws it.
#[derive(Clone, Debug)]
pub struct DynCurve<'a> {
    pub id: CurveId,
    /// `VCA L · Run 1` (with the measurement's name first when the pane holds several).
    pub name: String,
    pub color: Color,
    pub table: &'a DynamicsTable,
    /// The picked step's envelopes, once fetched.
    pub envelope: Option<&'a DynamicsEnvelope>,
}

impl DynCurve<'_> {
    /// Its row index in the table's inputs.
    fn input_index(&self) -> Option<usize> {
        self.table
            .inputs
            .iter()
            .position(|i| i.input == self.id.input)
    }

    /// Its rows, one per step.
    pub fn rows(&self) -> &[DynamicsRow] {
        self.input_index()
            .map_or(&[], |i| self.table.inputs[i].rows.as_slice())
    }

    pub fn info(&self) -> &DynamicsInfo {
        &self.table.info
    }
}

/// A curve's name: `<input name> · <run>`, the measurement's name first when the pane holds
/// curves of more than one measurement.
pub fn curve_name(input: &str, run: &str, meas: Option<&str>) -> String {
    match meas {
        Some(m) => format!("{m} · {input} · {run}"),
        None => format!("{input} · {run}"),
    }
}

/// The curves of `table` (one per measured input), in input order.
pub fn curve_ids(rig: Option<RigId>, trace: TraceId, table: &DynamicsTable) -> Vec<CurveId> {
    table
        .inputs
        .iter()
        .map(|i| CurveId {
            rig,
            trace,
            input: i.input,
        })
        .collect()
}

/// A and B among `curves` (indices): a side naming a curve the pane no longer holds falls
/// back to the default, and B is never A.
pub fn resolve(modes: &DynamicsModes, curves: &[CurveId]) -> (Option<usize>, Option<usize>) {
    let find = |id: CurveId| curves.iter().position(|c| *c == id);
    let a = match modes.a {
        Side::Curve(id) => find(id).or(if curves.is_empty() { None } else { Some(0) }),
        _ => (!curves.is_empty()).then_some(0),
    };
    let Some(a) = a else {
        return (None, None);
    };
    let default_b = || {
        let first = curves[a];
        curves
            .iter()
            .position(|c| c.trace == first.trace && c.rig == first.rig && *c != first)
            .or_else(|| {
                curves
                    .iter()
                    .position(|c| (c.trace, c.rig) != (first.trace, first.rig))
            })
    };
    let b = match modes.b {
        Side::Off => None,
        Side::Default => default_b(),
        Side::Curve(id) => find(id).or_else(default_b),
    };
    (Some(a), b.filter(|b| *b != a))
}

/// The side after `side` among `curves` (`B` steps A, `Shift+B` steps B), B passing through
/// none after the last curve.
pub fn cycle(side: Side, current: Option<usize>, curves: &[CurveId], is_b: bool) -> Side {
    if curves.is_empty() {
        return side;
    }
    let next = match (side, current) {
        (Side::Off, _) | (_, None) => Some(0),
        (_, Some(i)) if i + 1 < curves.len() => Some(i + 1),
        _ if is_b => None,
        _ => Some(0),
    };
    next.map_or(Side::Off, |i| Side::Curve(curves[i]))
}

/// How two curves' programmes compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    Same,
    /// Same programme, but some step's rest differs: the next step started elsewhere.
    RestsDiffer,
    /// Different carrier, onset, pilot, levels, lengths or sample rate.
    Differ,
}

pub const PROGRAMMES_DIFFER: &str = "programmes differ";
pub const RESTS_DIFFER: &str = "rests differ";
pub const CANT_COMPARE: &str = "can't compare";
pub const NO_SUCH_STEP: &str = "no such step";
pub const CANT_TELL: &str = "can't tell";
pub const WITHIN_PERIOD: &str = "≤ 1 period";
pub const BELOW_FLOOR: &str = "≤ floor";
pub const NO_GR: &str = "no gain reduction";
pub const NOT_RECOVERED: &str = "not recovered";

/// A run's progress for the pane's title: `step 3 of 24 · burst`; the linearity check
/// before the first step is step 0.
pub fn progress_text(step: u32, steps: u32, phase: ac2_proto::model::DynamicsPhase) -> String {
    use ac2_proto::model::DynamicsPhase as P;
    let phase = match phase {
        P::Waiting => "waiting",
        P::Check => "check",
        P::Burst => "burst",
        P::Rest => "rest",
    };
    if step == 0 {
        format!("{phase} · {steps} steps")
    } else {
        format!("step {step} of {steps} · {phase}")
    }
}

/// The question at arm when the measurement has runs: adapt the rests, or replay a run's
/// so two runs play the same programme (`runs`: the runs' names, newest first).
pub fn rests_question(meas: &str, runs: &[&str]) -> (String, Vec<crate::meas_list::Choice>) {
    let mut choices = vec![crate::meas_list::Choice {
        label: "Adapt the rests (each rest ends once the processor has recovered)".into(),
        blocked: None,
    }];
    choices.extend(runs.iter().map(|r| crate::meas_list::Choice {
        label: format!("Replay the rests of {r} (the same programme, to compare with it)"),
        blocked: None,
    }));
    (format!("Arm a run of {meas}"), choices)
}

/// The toast once a run is armed: its loudest level and how long it can take.
pub fn armed_text(meas: &str, top: &str, rests_from: Option<&str>) -> String {
    let rests = rests_from.map_or_else(String::new, |r| format!(" · rests of {r}"));
    format!("armed: a run of {meas} up to {top}{rests} · Enter plays it · Esc stops")
}

/// What an empty Dynamics pane says: with a dynamics measurement on it, the key that runs
/// it (`run`); without one, how to get one (`new`).
pub fn missing_text(meas: Option<&str>, run: &str, new: &str) -> String {
    match meas {
        Some(m) => format!("no run of {m} yet: {run} runs it"),
        None => format!("no dynamics run on this pane: {new} makes a dynamics measurement"),
    }
}

/// Whether `a` and `b` compare step by step.
pub fn programmes(a: &DynamicsInfo, b: &DynamicsInfo) -> Match {
    let near = |x: f64, y: f64| (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0);
    let same = near(a.sample_rate_hz.0, b.sample_rate_hz.0)
        && near(a.carrier.0, b.carrier.0)
        && a.period == b.period
        && a.onset == b.onset
        && near(a.pilot.0, b.pilot.0)
        && a.bursts.len() == b.bursts.len()
        && a.bursts.iter().zip(&b.bursts).all(|(x, y)| near(x.0, y.0))
        && a.durations.len() == b.durations.len()
        && a.durations
            .iter()
            .zip(&b.durations)
            .all(|(x, y)| near(x.0, y.0))
        && a.steps.len() == b.steps.len();
    if !same {
        return Match::Differ;
    }
    let sample = 1.0 / a.sample_rate_hz.0;
    if a.steps
        .iter()
        .zip(&b.steps)
        .any(|(x, y)| (x.rest.0 - y.rest.0).abs() > sample)
    {
        Match::RestsDiffer
    } else {
        Match::Same
    }
}

/// How far a curve's step may be from the pick and still be it.
const PICK_DB: f64 = 0.5;
const PICK_RATIO: f64 = 0.10;

/// The step of `info` nearest `pick`, if one is within 0.5 dB and 10 %.
pub fn step_of(info: &DynamicsInfo, pick: DynamicsStepPick) -> Option<usize> {
    info.steps
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            (s.level.0 - pick.level.0).abs() <= PICK_DB
                && (s.burst.0 - pick.burst.0).abs() <= PICK_RATIO * pick.burst.0
        })
        .min_by(|(_, x), (_, y)| {
            let d = |s: &ac2_proto::model::DynamicsStep| {
                (s.level.0 - pick.level.0).abs() + (s.burst.0 / pick.burst.0).ln().abs()
            };
            d(x).total_cmp(&d(y))
        })
        .map(|(i, _)| i)
}

fn sorted(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(f64::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    v
}

fn levels(info: &DynamicsInfo) -> Vec<f64> {
    sorted(info.bursts.iter().map(|b| b.0).collect())
}

fn lengths(info: &DynamicsInfo) -> Vec<f64> {
    sorted(info.durations.iter().map(|d| d.0).collect())
}

/// The default pick: the middle level (the lower of two), the longest burst.
pub fn default_pick(info: &DynamicsInfo) -> Option<DynamicsStepPick> {
    let l = levels(info);
    let d = lengths(info);
    Some(DynamicsStepPick {
        level: ac2_proto::units::Dbfs(*l.get(l.len().checked_sub(1)? / 2)?),
        burst: ac2_proto::units::Seconds(*d.last()?),
    })
}

/// The pick in force: the pane's, else the default of `info`.
pub fn pick(modes: &DynamicsModes, info: &DynamicsInfo) -> Option<DynamicsStepPick> {
    modes.step.or_else(|| default_pick(info))
}

/// `pick` moved `levels` levels up (negative: down) and `lengths` lengths longer among the
/// steps of `info`, stopping at either end.
pub fn step_pick(
    info: &DynamicsInfo,
    pick: DynamicsStepPick,
    levels_by: i32,
    lengths_by: i32,
) -> DynamicsStepPick {
    let nearest = |v: &[f64], x: f64, log: bool| {
        (0..v.len())
            .min_by(|&i, &j| {
                let d = |k: usize| {
                    if log {
                        (v[k] / x).ln().abs()
                    } else {
                        (v[k] - x).abs()
                    }
                };
                d(i).total_cmp(&d(j))
            })
            .unwrap_or(0)
    };
    let moved = |v: &[f64], x: f64, by: i32, log: bool| {
        if v.is_empty() {
            return x;
        }
        let i = nearest(v, x, log) as i64 + i64::from(by);
        v[i.clamp(0, v.len() as i64 - 1) as usize]
    };
    DynamicsStepPick {
        level: ac2_proto::units::Dbfs(moved(&levels(info), pick.level.0, levels_by, false)),
        burst: ac2_proto::units::Seconds(moved(&lengths(info), pick.burst.0, lengths_by, true)),
    }
}

/// A level as the pick shows it: `−8 dBFS`, `−7.5 dBFS`.
pub fn level_text(dbfs: f64) -> String {
    let d = if (dbfs - dbfs.round()).abs() < 1e-6 {
        0
    } else {
        1
    };
    format!("{} dBFS", fixed(dbfs, d))
}

/// A burst length: `1 ms`, `300 ms`, `10 s`, `0.5 ms`.
pub fn length_text(s: f64) -> String {
    let trim = |v: f64| {
        let t = format!("{v:.2}");
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    if s < 1.0 {
        format!("{} ms", trim(s * 1e3))
    } else {
        format!("{} s", trim(s))
    }
}

/// `−8 dBFS · 1 s`.
pub fn pick_text(p: DynamicsStepPick) -> String {
    format!("{} · {}", level_text(p.level.0), length_text(p.burst.0))
}

/// A measured time: `0.25 ms`, `12.3 ms`, `2.75 s`.
pub fn time_text(s: f64) -> String {
    if s.abs() < 0.01 {
        format!("{} ms", fixed(s * 1e3, 2))
    } else if s.abs() < 1.0 {
        format!("{} ms", fixed(s * 1e3, 1))
    } else if s.abs() < 10.0 {
        format!("{} s", fixed(s, 2))
    } else {
        format!("{} s", fixed(s, 1))
    }
}

/// The metric's name in the readout and caption.
pub fn metric_name(m: DynamicsMetric) -> &'static str {
    match m {
        DynamicsMetric::GrEnd => "GR end",
        DynamicsMetric::AttackTau => "attack τ",
        DynamicsMetric::Attack2Db => "attack 2 dB",
        DynamicsMetric::Settle05Db => "settle 0.5 dB",
        DynamicsMetric::Overshoot => "overshoot",
        DynamicsMetric::ReleaseTau => "release τ",
        DynamicsMetric::Recovery2Db => "recovery 2 dB",
        DynamicsMetric::Recovery05Db => "recovery 0.5 dB",
        DynamicsMetric::ThdAttack => "THD attack",
        DynamicsMetric::ThdSettled => "THD settled",
        DynamicsMetric::ThdRelease => "THD release",
        DynamicsMetric::ThumpStart => "thump start",
        DynamicsMetric::ThumpEnd => "thump end",
        DynamicsMetric::Drift => "drift",
    }
}

/// Its short heading in the Table.
pub fn metric_heading(m: DynamicsMetric) -> &'static str {
    match m {
        DynamicsMetric::GrEnd => "GR end",
        DynamicsMetric::AttackTau => "atk τ",
        DynamicsMetric::Attack2Db => "atk 2dB",
        DynamicsMetric::Settle05Db => "set 0.5",
        DynamicsMetric::Overshoot => "ovs",
        DynamicsMetric::ReleaseTau => "rel τ",
        DynamicsMetric::Recovery2Db => "rec 2dB",
        DynamicsMetric::Recovery05Db => "rec 0.5",
        DynamicsMetric::ThdAttack => "THD atk",
        DynamicsMetric::ThdSettled => "THD set",
        DynamicsMetric::ThdRelease => "THD rel",
        DynamicsMetric::ThumpStart => "thump st",
        DynamicsMetric::ThumpEnd => "thump end",
        DynamicsMetric::Drift => "drift",
    }
}

/// What a quantity is measured in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Gain reduction, overshoot, drift.
    Db,
    /// A level re the carrier (THD, thump): only a number above its floor.
    DbRe,
    Seconds,
}

/// One cell's value: a number, or what the analysis said instead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Num(f64, Unit),
    Word(&'static str),
}

fn time(t: DynamicsTime) -> Value {
    match t {
        DynamicsTime::Seconds { value } => Value::Num(value.0, Unit::Seconds),
        DynamicsTime::WithinPeriod => Value::Word(WITHIN_PERIOD),
        DynamicsTime::CantTell => Value::Word(CANT_TELL),
        DynamicsTime::NoGainReduction => Value::Word(NO_GR),
    }
}

fn level(l: DynamicsLevel) -> Value {
    match l {
        DynamicsLevel::Db { value } => Value::Num(value.0, Unit::DbRe),
        DynamicsLevel::BelowFloor => Value::Word(BELOW_FLOOR),
        DynamicsLevel::CantTell => Value::Word(CANT_TELL),
    }
}

/// Metric `m` of `row`.
pub fn value(row: &DynamicsRow, m: DynamicsMetric) -> Value {
    match m {
        DynamicsMetric::GrEnd => Value::Num(row.gr_end.0, Unit::Db),
        DynamicsMetric::AttackTau => time(row.attack_tau),
        DynamicsMetric::Attack2Db => time(row.attack_2db),
        DynamicsMetric::Settle05Db => time(row.settle_0_5db),
        DynamicsMetric::Overshoot => row
            .overshoot
            .map_or(Value::Word(CANT_TELL), |o| Value::Num(o.level.0, Unit::Db)),
        DynamicsMetric::ReleaseTau => time(row.release_tau),
        DynamicsMetric::Recovery2Db => time(row.recovery_2db),
        DynamicsMetric::Recovery05Db => time(row.recovery_0_5db),
        DynamicsMetric::ThdAttack => level(row.harmonics.attack.thd),
        DynamicsMetric::ThdSettled => level(row.harmonics.settled.thd),
        DynamicsMetric::ThdRelease => level(row.harmonics.release.thd),
        DynamicsMetric::ThumpStart => level(row.thump_start.peak),
        DynamicsMetric::ThumpEnd => level(row.thump_end.peak),
        DynamicsMetric::Drift => Value::Num(row.drift.0, Unit::Db),
    }
}

/// A value as a cell prints it.
pub fn value_text(v: Value, m: DynamicsMetric) -> String {
    match v {
        Value::Word(w) => w.to_string(),
        Value::Num(x, Unit::Seconds) => time_text(x),
        Value::Num(x, _) if m == DynamicsMetric::Drift => format!("{} dB", signed(x, 2)),
        Value::Num(x, _) => format!("{} dB", fixed(x, 1)),
    }
}

/// A − B of one metric.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Diff {
    /// The difference, and whether it is beyond the metric's tolerance.
    Num(f64, Unit, bool),
    CantCompare,
}

/// Starting tolerances of A − B (`docs/design/dynamics.md`, Steps): until a bench repeat run
/// measures the run-to-run spread of each metric.
pub const TOL_GR_DB: f64 = 0.3;
pub const TOL_TIME_RATIO: f64 = 0.25;
pub const TOL_LEVEL_DB: f64 = 3.0;

/// A − B, `period` the carrier period in seconds.
pub fn diff(a: Value, b: Value, period: f64) -> Diff {
    let (Value::Num(x, unit), Value::Num(y, _)) = (a, b) else {
        return Diff::CantCompare;
    };
    let d = x - y;
    let tol = match unit {
        Unit::Db => TOL_GR_DB,
        Unit::DbRe => TOL_LEVEL_DB,
        Unit::Seconds => (TOL_TIME_RATIO * x.abs().max(y.abs())).max(period),
    };
    Diff::Num(d, unit, d.abs() > tol)
}

pub fn diff_text(d: Diff) -> String {
    match d {
        Diff::CantCompare => CANT_COMPARE.to_string(),
        Diff::Num(x, Unit::Seconds, _) => {
            let t = time_text(x.abs());
            if x < 0.0 {
                format!("{MINUS}{t}")
            } else {
                format!("+{t}")
            }
        }
        Diff::Num(x, _, _) => format!("{} dB", signed(x, 2)),
    }
}

/// Where a step's rest left the processor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Baseline {
    /// Within what its release leaves.
    Steady,
    /// Moved by this drift, dB.
    Moved(f64),
    /// The rest reached its longest without recovering.
    NotRecovered,
    /// No time constant at the level to judge by.
    CantTell,
}

/// The residual a rest that ends when the gain moves under `FLAT_DB` over
/// `FLAT_WINDOW_SECONDS` leaves on an exponential-in-dB tail, per second of the tail's
/// time constant.
pub const RESIDUAL_DB_PER_S: f64 = 0.05;
/// Above twice the expected residual plus this, a step's baseline moved.
pub const MOVED_MARGIN_DB: f64 = 0.1;
/// Gain reduction from which recovery to 0.5 dB gives the tail's time constant: nearer
/// 0.5 dB the logarithm goes to zero.
pub const TAIL_MIN_GR_DB: f64 = 1.0;

/// The tail time constant of each level of `info`'s steps, in step order: the longest of
/// every step's release τ and recovery-to-0.5 dB over `ln(GR end / 0.5 dB)` at that level.
fn tail_taus(info: &DynamicsInfo, rows: &[DynamicsRow]) -> Vec<Option<f64>> {
    let at_level = |l: f64| {
        info.steps
            .iter()
            .zip(rows)
            .filter(|(s, _)| (s.level.0 - l).abs() < 1e-9)
            .flat_map(|(_, r)| {
                let release = match r.release_tau {
                    DynamicsTime::Seconds { value } => Some(value.0),
                    _ => None,
                };
                let recovery = match r.recovery_0_5db {
                    DynamicsTime::Seconds { value } if r.gr_end.0 >= TAIL_MIN_GR_DB => {
                        Some(value.0 / (r.gr_end.0 / 0.5).ln())
                    }
                    _ => None,
                };
                [release, recovery]
            })
            .flatten()
            .reduce(f64::max)
    };
    info.steps.iter().map(|s| at_level(s.level.0)).collect()
}

/// Each step's baseline check.
pub fn baselines(info: &DynamicsInfo, rows: &[DynamicsRow]) -> Vec<Baseline> {
    let taus = tail_taus(info, rows);
    info.steps
        .iter()
        .zip(rows)
        .zip(taus)
        .map(|((s, r), tau)| {
            if !s.recovered {
                return Baseline::NotRecovered;
            }
            let Some(tau) = tau else {
                return Baseline::CantTell;
            };
            let limit = 2.0 * RESIDUAL_DB_PER_S * tau + MOVED_MARGIN_DB;
            if r.drift.0.abs() > limit {
                Baseline::Moved(r.drift.0)
            } else {
                Baseline::Steady
            }
        })
        .collect()
}

/// Steps whose start is flagged: a moved step, and the one after it (it started from the
/// moved baseline).
pub fn flagged(b: &[Baseline]) -> Vec<bool> {
    (0..b.len())
        .map(|i| {
            matches!(b[i], Baseline::Moved(_)) || (i > 0 && matches!(b[i - 1], Baseline::Moved(_)))
        })
        .collect()
}

/// The readout's words for a step's baseline.
pub fn baseline_text(b: Baseline) -> String {
    match b {
        Baseline::Steady => "baseline held".to_string(),
        Baseline::Moved(d) => format!("baseline moved {} dB", signed(d, 2)),
        Baseline::NotRecovered => NOT_RECOVERED.to_string(),
        Baseline::CantTell => format!("baseline {CANT_TELL}"),
    }
}

/// Total of the H2…H5 levels (dB re the carrier); NaN where every one is blind.
pub fn thd_db(h: [f32; 4]) -> f32 {
    let p: f32 = h
        .iter()
        .filter(|v| v.is_finite())
        .map(|v| 10f32.powf(v / 10.0))
        .sum();
    if h.iter().any(|v| v.is_finite()) {
        10.0 * p.log10()
    } else {
        f32::NAN
    }
}
