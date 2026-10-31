//! `ac2 plan new|list|show|import|export` and
//! `ac2 walk new|status|capture|next|prev|skip|redo|swap`.
//! The daemon keeps the library and the walk; this suggests the positions
//! (`ac2_room::pattern`) and words what the walk holds.

use ac2_client::{Client, expect_body};
use ac2_proto::model::{
    MeasKind, PlanSpec, PointState, State, WalkKind, WalkPointSpec, WalkState, ZoneSpec,
};
use ac2_proto::units::MeasId;
use ac2_proto::{Command, ReplyBody};
use ac2_room::pattern::{Preset, suggest};
use ac2_scene::room::BandSet;

use super::{connect, find_meas, state};
use crate::CliError;
use crate::args::{Cli, MeasRef, PatternArg, PlanCmd, PlanNew, WalkCmd, WalkNew};
use crate::output::Out;

/// A zone this far in from the walls keeps the presets' wall distance.
const ZONE_INSET_M: f64 = 1.0;

pub(crate) async fn plan(cli: &Cli, cmd: &PlanCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    match cmd {
        PlanCmd::New(n) => {
            let p = save(&c, rectangle(n)?).await?;
            out.emit(&p, || plan_text(&p))?;
        }
        PlanCmd::List => {
            let op = "plan.list";
            let r = c.call(Command::PlanList).await?;
            let plans = expect_body!(op, r, ReplyBody::Plans(p) => p)?;
            out.emit(&plans, || {
                if plans.is_empty() {
                    return "no plans (ac2 plan new or ac2 plan import)".into();
                }
                plans.iter().map(plan_line).collect::<Vec<_>>().join("\n")
            })?;
        }
        PlanCmd::Show { plan } => {
            let p = library_plan(&c, plan).await?;
            out.emit(&p, || plan_text(&p))?;
        }
        PlanCmd::Import { file, name } => {
            let bytes = std::fs::read(file).map_err(CliError::Io)?;
            let mut p: PlanSpec = serde_json::from_slice(&bytes)
                .map_err(|e| CliError::Usage(format!("{}: no plan: {e}", file.display())))?;
            if let Some(n) = name {
                p.name.clone_from(n);
            }
            let p = save(&c, p).await?;
            out.emit(&p, || plan_text(&p))?;
        }
        PlanCmd::Export { plan, file } => {
            let p = library_plan(&c, plan).await?;
            let text = serde_json::to_string_pretty(&p)
                .map_err(|e| CliError::Io(std::io::Error::other(e)))?;
            match file {
                Some(f) => {
                    std::fs::write(f, format!("{text}\n")).map_err(CliError::Io)?;
                    out.emit(&serde_json::json!({ "plan": p.name, "file": f }), || {
                        format!("plan {} written to {}", p.name, f.display())
                    })?;
                }
                None => out.emit(&p, || text)?,
            }
        }
    }
    Ok(())
}

pub(crate) async fn walk(cli: &Cli, cmd: &WalkCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    let w = match cmd {
        WalkCmd::New(n) => new_walk(&c, &s, n, out).await?,
        WalkCmd::Status { walk } => find_walk(&s, walk.as_ref())?.clone(),
        WalkCmd::Capture { walk } => {
            let w = find_walk(&s, walk.as_ref())?;
            if w.kind == WalkKind::Sweep {
                let m = w.mics.first().map_or(0, |m| m.meas.0);
                return Err(CliError::Usage(format!(
                    "the walk {} captures by running its sweep: ac2 sweep run {m}",
                    w.name
                )));
            }
            walk_call(&c, Command::WalkCapture { walk: w.id }).await?
        }
        WalkCmd::Next { walk } | WalkCmd::Prev { walk } => {
            let w = find_walk(&s, walk.as_ref())?;
            let n = w.steps().max(1);
            let step = match (cmd, w.step) {
                (WalkCmd::Next { .. }, Some(i)) => (i + 1) % n,
                (WalkCmd::Next { .. }, None) => 0,
                (_, Some(i)) => (i + n - 1) % n,
                (_, None) => n - 1,
            };
            walk_call(&c, Command::WalkGoto { walk: w.id, step }).await?
        }
        WalkCmd::Skip { point, walk } => {
            let w = find_walk(&s, walk.as_ref())?;
            let points: Vec<u16> = match point {
                Some(p) => vec![find_point(w, p)?],
                None => w.standing().into_iter().map(index).collect(),
            };
            if points.is_empty() {
                return Err(CliError::Usage(format!(
                    "no mic stands anywhere in the walk {}: name the point",
                    w.name
                )));
            }
            let mut last = w.clone();
            for point in points {
                last = walk_call(&c, Command::WalkSkip { walk: w.id, point }).await?;
            }
            last
        }
        WalkCmd::Redo { point, walk } => {
            let w = find_walk(&s, walk.as_ref())?;
            let point = find_point(w, point)?;
            walk_call(&c, Command::WalkRedo { walk: w.id, point }).await?
        }
        WalkCmd::Swap { points, walk } => {
            let w = find_walk(&s, walk.as_ref())?;
            let points = points
                .iter()
                .map(|p| find_point(w, p))
                .collect::<Result<_, _>>()?;
            walk_call(&c, Command::WalkSwap { walk: w.id, points }).await?
        }
    };
    let s = state(&c).await?;
    out.emit(&w, || walk_text(&w, &s))?;
    Ok(())
}

async fn new_walk(
    c: &Client,
    s: &State,
    n: &WalkNew,
    out: &mut Out<'_>,
) -> Result<WalkState, CliError> {
    let found = n
        .meas
        .iter()
        .map(|m| find_meas(s, m))
        .collect::<Result<Vec<_>, _>>()?;
    // One sweep measurement walks as a sweep walk (its inputs are the mics); transfers live.
    let kind = match found.as_slice() {
        [m] if matches!(m.config.kind, MeasKind::Sweep { .. }) => WalkKind::Sweep,
        _ => WalkKind::LiveTransfer,
    };
    let meas: Vec<MeasId> = found.iter().map(|m| m.id).collect();
    let plan = s
        .walk_plans
        .iter()
        .chain(&s.plans)
        .find(|p| p.name == n.plan)
        .ok_or_else(|| CliError::Usage(format!("no plan named {:?} (ac2 plan list)", n.plan)))?;
    let zones: Vec<usize> = if n.zones.is_empty() {
        (0..plan.zones.len()).collect()
    } else {
        n.zones
            .iter()
            .map(|z| {
                plan.zones.iter().position(|x| &x.name == z).ok_or_else(|| {
                    CliError::Usage(format!("plan {} has no zone named {z:?}", plan.name))
                })
            })
            .collect::<Result<_, _>>()?
    };
    let preset = match n.preset {
        PatternArg::Tuning => Preset::tuning(),
        PatternArg::Iso3382 => Preset::iso3382(plan, n.rt.unwrap_or(0.0))
            .map_err(|e| CliError::Usage(e.to_string()))?,
    };
    let count = match n.points.or_else(|| preset.default_n(plan, &zones)) {
        Some(k) => k,
        None => {
            return Err(CliError::Usage(
                "the zones' seats are not set: give the number of positions (--points)".into(),
            ));
        }
    };
    let suggestion = suggest(plan, &zones, count, &preset, n.seed)
        .map_err(|e| CliError::Usage(format!("no positions: {e}")))?;
    let points = suggestion
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| WalkPointSpec {
            label: format!("P{}", i + 1),
            at: p.at,
        })
        .collect();
    let w = walk_call(
        c,
        Command::WalkNew {
            name: n.name.clone(),
            plan: plan.name.clone(),
            kind,
            meas,
            points,
        },
    )
    .await?;
    // A rule the plan did not let the suggestion keep is the operator's to know.
    out.warnings
        .extend(suggestion.relaxed.iter().map(|r| format!("relaxed: {r}")));
    Ok(w)
}

async fn walk_call(c: &Client, cmd: Command) -> Result<WalkState, CliError> {
    let op = cmd.name();
    let r = c.call(cmd).await?;
    Ok(expect_body!(op, r, ReplyBody::Walk(w) => w)?)
}

async fn save(c: &Client, plan: PlanSpec) -> Result<PlanSpec, CliError> {
    let op = "plan.save";
    let r = c.call(Command::PlanSave { plan }).await?;
    Ok(expect_body!(op, r, ReplyBody::Plan(p) => p)?)
}

async fn library_plan(c: &Client, name: &str) -> Result<PlanSpec, CliError> {
    let op = "plan.list";
    let r = c.call(Command::PlanList).await?;
    let plans = expect_body!(op, r, ReplyBody::Plans(p) => p)?;
    plans
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| CliError::Usage(format!("no plan named {name:?} (ac2 plan list)")))
}

/// The room `n` describes: a counter-clockwise rectangle, its audience zone inset from the
/// walls.
fn rectangle(n: &PlanNew) -> Result<PlanSpec, CliError> {
    let (w, d) = (n.width, n.depth);
    if !(w.is_finite() && d.is_finite() && w > 2.0 * ZONE_INSET_M && d > 2.0 * ZONE_INSET_M) {
        return Err(CliError::Usage(format!(
            "a room more than {} m wide and deep",
            2.0 * ZONE_INSET_M
        )));
    }
    let rect = |x0: f64, y0: f64, x1: f64, y1: f64| vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let i = ZONE_INSET_M;
    Ok(PlanSpec {
        name: n.name.clone(),
        outline: rect(0.0, 0.0, w, d),
        ceiling: n.ceiling,
        zones: vec![ZoneSpec {
            name: "Audience".into(),
            outline: rect(i, i, w - i, d - i),
            ear_height: n.ear_height,
            weight: None,
            seats: n.seats,
        }],
        markers: Vec::new(),
        surfaces: Vec::new(),
    })
}

/// The walk `r` names, or the session's only one.
pub(crate) fn find_walk<'s>(s: &'s State, r: Option<&MeasRef>) -> Result<&'s WalkState, CliError> {
    match r {
        Some(r) => super::resolve(&s.walks, r, "walk", |w| w.id.0, |w| w.name.as_str()),
        None => match s.walks.as_slice() {
            [w] => Ok(w),
            [] => Err(CliError::Usage("no walk (ac2 walk new)".into())),
            _ => Err(CliError::Usage(format!(
                "{} walks: name one with --walk",
                s.walks.len()
            ))),
        },
    }
}

/// A point by label, else by its number (1 the first).
fn find_point(w: &WalkState, p: &str) -> Result<u16, CliError> {
    if let Some(i) = w.points.iter().position(|x| x.label == p) {
        return Ok(index(i));
    }
    match p.parse::<usize>() {
        Ok(k) if (1..=w.points.len()).contains(&k) => Ok(index(k - 1)),
        _ => Err(CliError::Usage(format!(
            "the walk {} has no point {p:?}",
            w.name
        ))),
    }
}

fn index(i: usize) -> u16 {
    u16::try_from(i).unwrap_or(u16::MAX)
}

fn plan_line(p: &PlanSpec) -> String {
    let ceiling = p
        .ceiling
        .map_or_else(|| "outdoors".to_owned(), |c| format!("ceiling {c:.1} m"));
    format!(
        "{}  {:.0} m², {ceiling}, {} zone(s), {} marker(s)",
        p.name,
        ac2_room::plan::area(&p.outline).abs(),
        p.zones.len(),
        p.markers.len()
    )
}

fn plan_text(p: &PlanSpec) -> String {
    let mut t = format!("{}\n", plan_line(p));
    for z in &p.zones {
        let seats = z.seats.map_or_else(String::new, |s| format!(", {s} seats"));
        t.push_str(&format!(
            "  zone {}  {:.0} m², ear height {:.2} m{seats}\n",
            z.name,
            ac2_room::plan::area(&z.outline).abs(),
            z.ear_height
        ));
    }
    for m in &p.markers {
        t.push_str(&format!(
            "  {:?} {}  ({:.1}, {:.1}, {:.1} m)\n",
            m.kind, m.name, m.at[0], m.at[1], m.at[2]
        ));
    }
    t
}

/// The walk as the walk pane words it: *2 OF 8 POSITIONS*, the points the mics are at, each
/// point's state and (with several mics) the measurement standing or that stood there.
fn walk_text(w: &WalkState, s: &State) -> String {
    let count = |f: fn(&PointState) -> bool| w.points.iter().filter(|p| f(&p.state)).count();
    let captured = count(|p| matches!(p, PointState::Captured { .. }));
    let skipped = count(|p| matches!(p, PointState::Skipped));
    let average = w
        .average
        .and_then(|a| s.measurements.iter().find(|m| m.id == a))
        .map_or_else(
            || "no average".to_owned(),
            |m| format!("average: {}", m.config.name),
        );
    let meas_name = |id| {
        s.measurements
            .iter()
            .find(|m| m.id == id)
            .map_or_else(|| format!("measurement {id}"), |m| m.config.name.clone())
    };
    let mut t = format!(
        "{} (walk {}, plan {})  {captured} of {} positions{}  {average}\n",
        w.name,
        w.id.0,
        w.plan,
        w.points.len(),
        if skipped > 0 {
            format!(", {skipped} skipped")
        } else {
            String::new()
        }
    );
    let standing = w.standing();
    for (i, p) in w.points.iter().enumerate() {
        let here = if standing.contains(&i) { ">" } else { " " };
        let state = match &p.state {
            PointState::Pending => "pending".to_owned(),
            PointState::Skipped => "skipped".to_owned(),
            PointState::Captured { trace, .. } => {
                let unsettled = s
                    .traces
                    .iter()
                    .any(|x| x.id == *trace && x.captured_unsettled);
                if unsettled {
                    "captured, unsettled".to_owned()
                } else {
                    "captured".to_owned()
                }
            }
        };
        let mic = if w.mics.len() > 1 {
            format!(
                "  step {}  {} in {}",
                p.step + 1,
                meas_name(p.mic.meas),
                p.mic.input + 1
            )
        } else {
            String::new()
        };
        t.push_str(&format!(
            "{here} {:<6} ({:5.1}, {:5.1}, {:.2} m)  {state}{mic}\n",
            p.label, p.at[0], p.at[1], p.at[2]
        ));
    }
    if w.step.is_none() {
        t.push_str("every point captured or skipped\n");
    }
    if let Some(r) = &w.room {
        t.push_str(&ac2_scene::room::walk_room_table(r, BandSet::Octave).text());
    }
    t
}
