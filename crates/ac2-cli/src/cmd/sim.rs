//! `ac2 sim run|fit|status|cancel`: the daemon simulates; this names the plan's speakers
//! and the receivers, follows the progress and prints the result as the scene words it.

use ac2_client::{Client, expect_body};
use ac2_proto::model::{
    MarkerKind, PlanSpec, SimFitRequest, SimReceivers, SimRequest, SimState, State,
};
use ac2_proto::{Command, ReplyBody};
use ac2_scene::format;
use ac2_scene::room::sim;

use super::walk::find_walk;
use super::{connect, state};
use crate::CliError;
use crate::args::{Cli, SimCmd, SimFitArgs, SimRunArgs};
use crate::output::Out;
use crate::watch::quit_signal;

pub(crate) async fn run(cli: &Cli, cmd: &SimCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let detach = match cmd {
        SimCmd::Run(a) => a.detach,
        SimCmd::Fit(a) => a.detach,
        SimCmd::Status | SimCmd::Cancel => true,
    };
    let c = connect(cli, !detach).await?;
    let s = state(&c).await?;
    let started = match cmd {
        SimCmd::Run(a) => {
            let request = run_request(&s, a)?;
            sim_call(&c, Command::SimRun { request }).await?
        }
        SimCmd::Fit(a) => {
            let request = fit_request(&s, a)?;
            sim_call(&c, Command::SimFit { request }).await?
        }
        SimCmd::Status => s
            .sim
            .clone()
            .ok_or_else(|| CliError::Usage("no simulation (ac2 sim run)".into()))?,
        SimCmd::Cancel => sim_call(&c, Command::SimCancel).await?,
    };
    let done = if detach {
        started
    } else {
        follow(&c, started, out).await?
    };
    out.emit(&done, || sim_text(&done))?;
    Ok(())
}

fn run_request(s: &State, a: &SimRunArgs) -> Result<SimRequest, CliError> {
    let plan = find_plan(s, &a.plan)?;
    let sources = if a.sources.is_empty() {
        speakers(plan)
    } else {
        a.sources.clone()
    };
    if sources.is_empty() {
        return Err(CliError::Usage(format!(
            "plan {} has no speaker marker to simulate",
            plan.name
        )));
    }
    let receivers = match &a.walk {
        Some(w) => SimReceivers::Walk {
            walk: find_walk(s, Some(w))?.id,
        },
        None => SimReceivers::Points {
            points: a.point_specs(),
        },
    };
    Ok(SimRequest {
        plan: plan.name.clone(),
        sources,
        receivers,
        settings: a.settings.settings(),
    })
}

fn fit_request(s: &State, a: &SimFitArgs) -> Result<SimFitRequest, CliError> {
    let w = find_walk(s, a.walk.as_ref())?;
    let source = match &a.source {
        Some(n) => n.clone(),
        None => {
            let plan = find_plan(s, &w.plan)?;
            match speakers(plan).as_slice() {
                [one] => one.clone(),
                [] => {
                    return Err(CliError::Usage(format!(
                        "plan {} has no speaker marker",
                        plan.name
                    )));
                }
                _ => {
                    return Err(CliError::Usage(format!(
                        "plan {} has several speakers: name the walk's with --source",
                        plan.name
                    )));
                }
            }
        }
    };
    Ok(SimFitRequest {
        walk: w.id,
        source,
        settings: a.settings.settings(),
    })
}

/// The session's copy of a plan, else the library's (as the daemon looks it up).
fn find_plan<'s>(s: &'s State, name: &str) -> Result<&'s PlanSpec, CliError> {
    s.walk_plans
        .iter()
        .chain(&s.plans)
        .find(|p| p.name == name)
        .ok_or_else(|| CliError::Usage(format!("no plan named {name:?} (ac2 plan list)")))
}

fn speakers(p: &PlanSpec) -> Vec<String> {
    p.markers
        .iter()
        .filter(|m| m.kind == MarkerKind::Speaker)
        .map(|m| m.name.clone())
        .collect()
}

async fn sim_call(c: &Client, cmd: Command) -> Result<SimState, CliError> {
    let op = cmd.name();
    let r = c.call(cmd).await?;
    Ok(expect_body!(op, r, ReplyBody::Sim(s) => s)?)
}

/// Waits for simulation `s` to end, a line per change of its progress text; a quit signal
/// cancels it.
async fn follow(c: &Client, s: SimState, out: &mut Out<'_>) -> Result<SimState, CliError> {
    let mut watch = c.watch();
    let quit = quit_signal();
    tokio::pin!(quit);
    let mut last = String::new();
    let mut current = s;
    loop {
        if !current.active() {
            return Ok(current);
        }
        let line = sim::status_text(&current);
        if line != last && !out.json {
            writeln!(out.w, "{line}").map_err(CliError::Io)?;
            last = line;
        }
        tokio::select! {
            _ = &mut quit => {
                return sim_call(c, Command::SimCancel).await;
            }
            silent = c.await_dead() => return Err(crate::watch::dead(silent)),
            changed = watch.changed() => {
                if changed.is_err() {
                    return Err(CliError::Refused("the connection to the daemon closed".into()));
                }
                let view = watch.borrow_and_update().clone();
                if let Some(now) = view.state.as_ref().and_then(|st| st.sim.clone())
                    && now.id == current.id
                {
                    current = now;
                }
            }
        }
    }
}

/// The simulation as text: what it is, for a fit the scale per band, per source the room
/// table (its legend carries the labels).
pub(crate) fn sim_text(s: &SimState) -> String {
    let what = match &s.kind {
        ac2_proto::model::SimKind::Run => "run".to_owned(),
        ac2_proto::model::SimKind::Fit { walk } => format!("fit to walk {}", walk.0),
    };
    let mut t = format!(
        "simulation {} ({what}, plan {})  {}\n",
        s.id.0,
        s.plan,
        sim::status_text(s)
    );
    if s.active() || s.sources.is_empty() {
        return t;
    }
    if let ac2_proto::model::SimAbsorption::Fitted { bands, .. } = &s.absorption {
        t.push_str("band     scale  T20 measured  T20 simulated\n");
        for (f, b) in s.bands.iter().zip(bands) {
            let secs = |v: Option<ac2_proto::units::Seconds>| {
                v.map_or_else(|| "—".to_owned(), |v| format!("{:.2} s", v.0))
            };
            t.push_str(&format!(
                "{:<7}  {:>5}  {:>12}  {:>13}{}\n",
                format::freq_tick(f.0),
                b.scale
                    .map_or_else(|| "assumed".to_owned(), |v| format!("{v:.2}")),
                secs(b.measured),
                secs(b.simulated),
                if b.scale.is_some() && !b.converged {
                    "  not within the tolerance"
                } else {
                    ""
                }
            ));
        }
    }
    for k in 0..s.sources.len() {
        if let Some(table) = sim::room_table(s, k) {
            t.push_str(&table.text());
        }
    }
    if s.lost_rays > 0 {
        t.push_str(&format!(
            "{} rays left the room through a gap in its surfaces\n",
            s.lost_rays
        ));
    }
    t
}
