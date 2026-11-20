//! A dynamics trace in an ac2 CSV: the run's settings as one JSON header line
//! (`# dynamics_info: {…}`, [`DynamicsInfo`]), each measured input's facts as another
//! (`# dynamics_inputs: [{…}]`), then two tables, each announced by a header line with its row
//! count:
//!
//! ```text
//! # dynamics_steps: {"rows":18}
//! input,step,level_dbfs,burst_s,rest_s,recovered,gr_end_db,gr_settled_db,settled,…,drift_db
//! 5,1,-8,0.001,4.25,true,0.5,8,true,0.0018,within_period,…,-0.0625
//! # dynamics_envelopes: {"rows":5210}
//! input,step,edge,t_s,gr_db,h2_db,h3_db,h4_db,h5_db
//! 5,1,start,-0.05,0,nan,nan,nan,nan
//! ```
//!
//! Inputs and steps are 1-based. A time reads seconds, `within_period` (one carrier period or
//! less), `cant_tell` or `no_gain_reduction`; a level reads dB, `below_floor` or `cant_tell`;
//! an overshoot or settled gain reduction the level has none of reads `cant_tell`. Numbers
//! are written in their shortest exact form, so the export re-imports as the trace it was.
//!
//! A dynamics trace has no frequency columns. Every trace sits on a grid, so it sits on the
//! one log-grid column nearest its carrier, a gap.

use std::fmt::Write as _;

use ac2_proto::GridDef;
use ac2_proto::ImportProblem;
use ac2_proto::model::{
    DynamicsData, DynamicsEdge, DynamicsHarmonics, DynamicsInfo, DynamicsInput, DynamicsLevel,
    DynamicsOvershoot, DynamicsPoint, DynamicsRow, DynamicsThump, DynamicsTime, DynamicsWindows,
};
use ac2_proto::units::{Db, Hz, Seconds};
use serde::{Deserialize, Serialize};

use super::table::number;
use super::{ImportError, fail};
use crate::columns::Columns;

/// Points per octave of the log grid a dynamics trace sits on.
const GRID_PPO: u32 = 48;

/// Most envelope rows an import accepts: 128 steps × 8 inputs × 6000 points.
pub const MAX_ENVELOPE_ROWS: usize = 6_144_000;

/// The grid of a dynamics trace: the one log-grid column nearest its carrier.
pub fn grid(carrier: Hz) -> GridDef {
    let k = (f64::from(GRID_PPO) * (carrier.0 / 1000.0).log2()).round() as i32;
    GridDef::Log {
        ppo: GRID_PPO,
        k_min: k,
        k_max: k,
    }
}

/// The columns of a dynamics trace: its one column, a gap.
pub fn columns() -> Columns {
    Columns {
        mag_db: vec![f32::NAN],
        phase_deg: None,
        coherence: None,
    }
}

/// A measured input's facts, as the `# dynamics_inputs:` line holds them.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputFacts {
    input: u16,
    name: String,
    delay: Seconds,
    g0: Db,
}

const WINDOWS: [&str; 4] = ["attack", "settled", "release", "rest"];

fn step_header() -> String {
    let mut h = String::from(
        "input,step,level_dbfs,burst_s,rest_s,recovered,gr_end_db,gr_settled_db,settled,\
         attack_tau_s,attack_2db_s,settle_0_5db_s,overshoot_db,overshoot_t_s,overshoot_ref_db,\
         release_tau_s,recovery_2db_s,recovery_0_5db_s",
    );
    for w in WINDOWS {
        for k in 2..=5 {
            let _ = write!(h, ",{w}_h{k}_db");
        }
        let _ = write!(h, ",{w}_thd_db");
    }
    h.push_str(
        ",thump_start_peak_db,thump_start_rms_db,thump_end_peak_db,thump_end_rms_db,drift_db",
    );
    h
}

const ENVELOPE_HEADER: &str = "input,step,edge,t_s,gr_db,h2_db,h3_db,h4_db,h5_db";

const CANT_TELL: &str = "cant_tell";

fn time_text(t: DynamicsTime) -> String {
    match t {
        DynamicsTime::Seconds { value } => format!("{}", value.0),
        DynamicsTime::WithinPeriod => "within_period".into(),
        DynamicsTime::CantTell => CANT_TELL.into(),
        DynamicsTime::NoGainReduction => "no_gain_reduction".into(),
    }
}

fn level_text(l: DynamicsLevel) -> String {
    match l {
        DynamicsLevel::Db { value } => format!("{}", value.0),
        DynamicsLevel::BelowFloor => "below_floor".into(),
        DynamicsLevel::CantTell => CANT_TELL.into(),
    }
}

fn num32(v: f32) -> String {
    if v.is_nan() {
        "nan".into()
    } else {
        format!("{v}")
    }
}

/// The dynamics part of an export: header lines, then both tables.
pub(super) fn write(out: &mut String, d: &DynamicsData) {
    let facts: Vec<InputFacts> = d
        .inputs
        .iter()
        .map(|i| InputFacts {
            input: i.input,
            name: i.name.clone(),
            delay: i.delay,
            g0: i.g0,
        })
        .collect();
    let _ = writeln!(out, "# dynamics_info: {}", json(&d.info));
    let _ = writeln!(out, "# dynamics_inputs: {}", json(&facts));
    let rows: usize = d.inputs.iter().map(|i| i.rows.len()).sum();
    let _ = writeln!(out, "# dynamics_steps: {{\"rows\":{rows}}}");
    out.push_str(&step_header());
    out.push('\n');
    for i in &d.inputs {
        for (s, r) in i.rows.iter().enumerate() {
            let step = d.info.steps.get(s);
            let _ = write!(
                out,
                "{},{},{},{},{},{},{},{},{}",
                u32::from(i.input) + 1,
                s + 1,
                step.map_or(f64::NAN, |x| x.level.0),
                step.map_or(f64::NAN, |x| x.burst.0),
                step.map_or(f64::NAN, |x| x.rest.0),
                step.is_some_and(|x| x.recovered),
                r.gr_end.0,
                r.gr_settled
                    .map_or_else(|| CANT_TELL.into(), |g| format!("{}", g.0)),
                r.settled,
            );
            for t in [r.attack_tau, r.attack_2db, r.settle_0_5db] {
                let _ = write!(out, ",{}", time_text(t));
            }
            match r.overshoot {
                Some(o) => {
                    let _ = write!(out, ",{},{},{}", o.level.0, o.at.0, o.reference.0);
                }
                None => {
                    let _ = write!(out, ",{CANT_TELL},{CANT_TELL},{CANT_TELL}");
                }
            }
            for t in [r.release_tau, r.recovery_2db, r.recovery_0_5db] {
                let _ = write!(out, ",{}", time_text(t));
            }
            let w = &r.harmonics;
            for h in [w.attack, w.settled, w.release, w.rest] {
                for l in h.h.iter().chain(std::iter::once(&h.thd)) {
                    let _ = write!(out, ",{}", level_text(*l));
                }
            }
            for t in [r.thump_start, r.thump_end] {
                let _ = write!(out, ",{},{}", level_text(t.peak), level_text(t.rms));
            }
            let _ = writeln!(out, ",{}", r.drift.0);
        }
    }
    let points: usize = d
        .inputs
        .iter()
        .flat_map(|i| i.envelopes.iter())
        .map(Vec::len)
        .sum();
    let _ = writeln!(out, "# dynamics_envelopes: {{\"rows\":{points}}}");
    out.push_str(ENVELOPE_HEADER);
    out.push('\n');
    for i in &d.inputs {
        for (s, env) in i.envelopes.iter().enumerate() {
            for p in env {
                let _ = writeln!(
                    out,
                    "{},{},{},{},{},{},{},{},{}",
                    u32::from(i.input) + 1,
                    s + 1,
                    match p.edge {
                        DynamicsEdge::Start => "start",
                        DynamicsEdge::End => "end",
                    },
                    num32(p.t),
                    num32(p.gr_db),
                    num32(p.h_db[0]),
                    num32(p.h_db[1]),
                    num32(p.h_db[2]),
                    num32(p.h_db[3]),
                );
            }
        }
    }
}

fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Announce {
    rows: usize,
}

fn bad(no: usize, what: &str, e: &dyn std::fmt::Display) -> ImportError {
    fail(Some(no), ImportProblem::BadHeader, format!("{what}: {e}"))
}

fn cell_err(no: usize, col: &str, v: &str) -> ImportError {
    fail(
        Some(no),
        ImportProblem::BadNumber,
        format!("{col}: {v:?} is not a value of that column"),
    )
}

fn parse_time(no: usize, col: &str, v: &str) -> Result<DynamicsTime, ImportError> {
    Ok(match v {
        "within_period" => DynamicsTime::WithinPeriod,
        CANT_TELL => DynamicsTime::CantTell,
        "no_gain_reduction" => DynamicsTime::NoGainReduction,
        _ => DynamicsTime::Seconds {
            value: Seconds(finite(no, col, v)?),
        },
    })
}

fn parse_level(no: usize, col: &str, v: &str) -> Result<DynamicsLevel, ImportError> {
    Ok(match v {
        "below_floor" => DynamicsLevel::BelowFloor,
        CANT_TELL => DynamicsLevel::CantTell,
        _ => DynamicsLevel::Db {
            value: Db(finite(no, col, v)?),
        },
    })
}

fn finite(no: usize, col: &str, v: &str) -> Result<f64, ImportError> {
    number(v)
        .filter(|x| x.is_finite())
        .ok_or_else(|| cell_err(no, col, v))
}

fn flag(no: usize, col: &str, v: &str) -> Result<bool, ImportError> {
    match v {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(cell_err(no, col, v)),
    }
}

/// A table's rows, each with its line number and its cells.
type Rows<'a> = Vec<(usize, Vec<&'a str>)>;

/// The table announced at `lines[at]` (header line next, then its rows): its rows split
/// into cells, after checking the column header.
fn table<'a>(
    lines: &[(usize, &'a str)],
    at: usize,
    header: &str,
    max: usize,
) -> Result<(Rows<'a>, usize), ImportError> {
    let (no, l) = lines[at];
    let v = l.split_once(": ").map_or("", |(_, v)| v);
    let a: Announce = serde_json::from_str(v).map_err(|e| bad(no, "table", &e))?;
    if a.rows > max {
        return Err(fail(
            Some(no),
            ImportProblem::TooManyRows,
            format!("{} rows (at most {max})", a.rows),
        ));
    }
    let Some(&(hno, h)) = lines.get(at + 1) else {
        return Err(fail(
            Some(no),
            ImportProblem::NoData,
            "table header missing",
        ));
    };
    if h.trim() != header {
        return Err(bad(hno, "table header", &format!("{:?}", h.trim())));
    }
    let width = header.split(',').count();
    let mut rows = Vec::with_capacity(a.rows);
    for k in 0..a.rows {
        let Some(&(rno, r)) = lines.get(at + 2 + k) else {
            return Err(fail(
                Some(hno),
                ImportProblem::NoData,
                format!("{} rows announced, {k} found", a.rows),
            ));
        };
        let cells: Vec<&str> = r.trim().split(',').collect();
        if cells.len() != width {
            return Err(fail(
                Some(rno),
                ImportProblem::ColumnCount,
                format!("{} columns, the header names {width}", cells.len()),
            ));
        }
        rows.push((rno, cells));
    }
    Ok((rows, at + 2 + a.rows))
}

fn index(no: usize, col: &str, v: &str, n: usize) -> Result<usize, ImportError> {
    v.parse::<usize>()
        .ok()
        .filter(|i| (1..=n).contains(i))
        .map(|i| i - 1)
        .ok_or_else(|| cell_err(no, col, v))
}

/// Reads the dynamics part of an export (module docs).
pub(super) fn read(lines: &[(usize, &str)]) -> Result<DynamicsData, ImportError> {
    let mut info: Option<DynamicsInfo> = None;
    let mut facts: Option<Vec<InputFacts>> = None;
    let mut steps_at = None;
    let mut env_at = None;
    for (k, &(no, l)) in lines.iter().enumerate() {
        let l = l.trim();
        if let Some(v) = l.strip_prefix("# dynamics_info: ") {
            info = Some(serde_json::from_str(v).map_err(|e| bad(no, "dynamics_info", &e))?);
        } else if let Some(v) = l.strip_prefix("# dynamics_inputs: ") {
            facts = Some(serde_json::from_str(v).map_err(|e| bad(no, "dynamics_inputs", &e))?);
        } else if l.starts_with("# dynamics_steps: ") {
            steps_at = Some(k);
        } else if l.starts_with("# dynamics_envelopes: ") {
            env_at = Some(k);
        }
    }
    let missing = |what: &str| fail(None, ImportProblem::BadHeader, format!("no {what} line"));
    let info = info.ok_or_else(|| missing("dynamics_info"))?;
    let facts = facts.ok_or_else(|| missing("dynamics_inputs"))?;
    let steps_at = steps_at.ok_or_else(|| missing("dynamics_steps"))?;
    let env_at = env_at.ok_or_else(|| missing("dynamics_envelopes"))?;
    crate::check::dynamics_info(&info).map_err(|e| fail(None, ImportProblem::BadHeader, e))?;
    if facts.is_empty() || facts.len() > crate::check::DYNAMICS_MAX_INPUTS {
        return Err(fail(
            None,
            ImportProblem::BadHeader,
            "dynamics_inputs: 1 … 8 inputs",
        ));
    }
    let n_steps = info.steps.len();
    let n_inputs = facts.len();
    let mut inputs: Vec<DynamicsInput> = facts
        .into_iter()
        .map(|f| DynamicsInput {
            input: f.input,
            name: f.name,
            delay: f.delay,
            g0: f.g0,
            rows: Vec::with_capacity(n_steps),
            envelopes: vec![Vec::new(); n_steps],
        })
        .collect();
    let header = step_header();
    let (rows, _) = table(lines, steps_at, &header, n_steps * n_inputs)?;
    let cols: Vec<&str> = header.split(',').collect();
    for (no, c) in rows {
        let col = |i: usize| (cols[i], c[i]);
        let (n, v) = col(0);
        let input_no = v
            .parse::<u32>()
            .ok()
            .and_then(|x| x.checked_sub(1))
            .and_then(|x| u16::try_from(x).ok())
            .ok_or_else(|| cell_err(no, n, v))?;
        let Some(inp) = inputs.iter_mut().find(|i| i.input == input_no) else {
            return Err(cell_err(no, n, v));
        };
        let (n, v) = col(1);
        let step = index(no, n, v, n_steps)?;
        if step != inp.rows.len() {
            return Err(fail(
                Some(no),
                ImportProblem::BadNumber,
                format!("step {}: steps of an input run 1, 2 … in order", step + 1),
            ));
        }
        let t = |i: usize| parse_time(no, cols[i], c[i]);
        let lv = |i: usize| parse_level(no, cols[i], c[i]);
        let f = |i: usize| finite(no, cols[i], c[i]);
        let harmonics = |base: usize| -> Result<DynamicsHarmonics, ImportError> {
            Ok(DynamicsHarmonics {
                h: [lv(base)?, lv(base + 1)?, lv(base + 2)?, lv(base + 3)?],
                thd: lv(base + 4)?,
            })
        };
        let overshoot = if c[12] == CANT_TELL && c[13] == CANT_TELL && c[14] == CANT_TELL {
            None
        } else {
            Some(DynamicsOvershoot {
                level: Db(f(12)?),
                at: Seconds(f(13)?),
                reference: Db(f(14)?),
            })
        };
        let row = DynamicsRow {
            gr_end: Db(f(6)?),
            gr_settled: if c[7] == CANT_TELL {
                None
            } else {
                Some(Db(f(7)?))
            },
            settled: flag(no, cols[8], c[8])?,
            attack_tau: t(9)?,
            attack_2db: t(10)?,
            settle_0_5db: t(11)?,
            overshoot,
            release_tau: t(15)?,
            recovery_2db: t(16)?,
            recovery_0_5db: t(17)?,
            harmonics: DynamicsWindows {
                attack: harmonics(18)?,
                settled: harmonics(23)?,
                release: harmonics(28)?,
                rest: harmonics(33)?,
            },
            thump_start: DynamicsThump {
                peak: lv(38)?,
                rms: lv(39)?,
            },
            thump_end: DynamicsThump {
                peak: lv(40)?,
                rms: lv(41)?,
            },
            drift: Db(f(42)?),
        };
        inp.rows.push(row);
    }
    if inputs.iter().any(|i| i.rows.len() != n_steps) {
        return Err(fail(
            None,
            ImportProblem::NoData,
            "every input needs one row per step",
        ));
    }
    let (rows, _) = table(lines, env_at, ENVELOPE_HEADER, MAX_ENVELOPE_ROWS)?;
    for (no, c) in rows {
        let input_no = c[0]
            .parse::<u32>()
            .ok()
            .and_then(|x| x.checked_sub(1))
            .and_then(|x| u16::try_from(x).ok())
            .ok_or_else(|| cell_err(no, "input", c[0]))?;
        let Some(inp) = inputs.iter_mut().find(|i| i.input == input_no) else {
            return Err(cell_err(no, "input", c[0]));
        };
        let step = index(no, "step", c[1], n_steps)?;
        let edge = match c[2] {
            "start" => DynamicsEdge::Start,
            "end" => DynamicsEdge::End,
            v => return Err(cell_err(no, "edge", v)),
        };
        // Parsed as f32 directly: the export wrote each value's shortest f32 form.
        let v32 = |i: usize, finite_only: bool| -> Result<f32, ImportError> {
            let x = if c[i] == "nan" {
                f32::NAN
            } else {
                c[i].parse::<f32>()
                    .ok()
                    .filter(|x| x.is_finite())
                    .ok_or_else(|| cell_err(no, "envelope", c[i]))?
            };
            if finite_only && x.is_nan() {
                return Err(cell_err(no, "envelope", c[i]));
            }
            Ok(x)
        };
        inp.envelopes[step].push(DynamicsPoint {
            edge,
            t: v32(3, true)?,
            gr_db: v32(4, false)?,
            h_db: [
                v32(5, false)?,
                v32(6, false)?,
                v32(7, false)?,
                v32(8, false)?,
            ],
        });
    }
    let d = DynamicsData { info, inputs };
    crate::check::dynamics(&d).map_err(|e| fail(None, ImportProblem::BadHeader, e))?;
    Ok(d)
}
