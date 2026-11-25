//! What a session load checks before it replaces anything ([`check_session`]), and what the
//! autosave restore does with what it refuses ([`salvage`]): a load refuses the whole, a
//! restore only the items refused.

use std::collections::HashMap;

use ac2_proto::ProtoError;
use ac2_proto::grid::GridDef;
use ac2_proto::model::{MeasKind, TraceOwner};
use ac2_proto::units::MeasId;
use ac2_traces::meta;
use ac2_traces::session::{SavedMeasurement, Session as SessionData};

use super::{check_delay, maths, static_grid, validate_meas};
use crate::util::invalid;

/// Checks `data` before a load replaces anything with it: everything the live commands
/// would refuse is refused here too. Returns the grid of each measurement that has one before
/// a stream runs; a transfer math trace's is its operands' (checked against the loaded
/// measurements and traces).
pub(super) fn check_session(data: &SessionData) -> Result<HashMap<MeasId, GridDef>, ProtoError> {
    check_items(data).map_err(|(_, e)| e)
}

/// The bound on the stream epoch a loaded trace may record: it records one below it. A rig
/// opens one epoch per stream open or audio restart, so half the range is more than any
/// rig reaches, and the other half stays for the streams opened after a load.
pub(super) const MAX_LOADED_EPOCH: u32 = u32::MAX / 2;

/// One item of a session, by its index in its list: what a refusal is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Item {
    Meas(usize),
    Trace(usize),
    SplLog(usize),
    Plan(usize),
    Walk(usize),
}

/// [`check_session`], naming the item each refusal is about. Of two items with one id the
/// later is the refused one.
pub(super) fn check_items(
    data: &SessionData,
) -> Result<HashMap<MeasId, GridDef>, (Item, ProtoError)> {
    let mut grids: HashMap<MeasId, GridDef> = HashMap::new();
    for (i, sm) in data.measurements.iter().enumerate() {
        let item = |e| (Item::Meas(i), e);
        validate_meas(&sm.config).map_err(item)?;
        // A saved delay meets the limits `delay.set` and `delay.nudge` enforce.
        if let Some(d) = &sm.delay {
            check_delay(&format!("measurement {} saved delay", sm.id), d.applied).map_err(item)?;
            check_delay(&format!("measurement {} saved nudge", sm.id), d.nudged).map_err(item)?;
        }
        if data.measurements[..i].iter().any(|o| o.id == sm.id) {
            return Err(item(invalid(format!(
                "measurement {} appears twice in the session",
                sm.id
            ))));
        }
        let grid = match &sm.config.kind {
            MeasKind::Math { config } => {
                let meas = |id| {
                    data.measurements
                        .iter()
                        .find(|o: &&SavedMeasurement| o.id == id)
                        .map(|o| (o.config.name.as_str(), &o.config.kind))
                };
                let trace = |id| {
                    data.traces
                        .iter()
                        .find(|t: &&ac2_traces::StoredTrace| t.meta.id == id)
                        .map(|t| (&t.meta, t.grid.clone()))
                };
                let eq = |id, set| {
                    data.measurements
                        .iter()
                        .any(|o: &SavedMeasurement| o.id == id && o.eq.iter().any(|s| s.id == set))
                };
                maths::check_operands(
                    Some(sm.id),
                    config,
                    &maths::Lookup {
                        meas: &meas,
                        trace: &trace,
                        eq: &eq,
                        epoch: None,
                    },
                )
                .map_err(item)?
            }
            k => static_grid(k),
        };
        if let Some(g) = grid {
            grids.insert(sm.id, g);
        }
    }
    // EQ sets: ids unique in the session, each curve one of its traces.
    let mut set_ids = std::collections::HashSet::new();
    for (i, sm) in data.measurements.iter().enumerate() {
        for s in &sm.eq {
            if !set_ids.insert(s.id) {
                return Err((
                    Item::Meas(i),
                    invalid(format!("EQ set {} appears twice in the session", s.id.0)),
                ));
            }
            if s.body.sections.len() > ac2_proto::model::EqSet::MAX_SECTIONS
                || !data.traces.iter().any(|t| t.meta.id == s.trace)
            {
                return Err((
                    Item::Meas(i),
                    invalid(format!(
                        "EQ set {} of measurement {} is not one ac2d keeps",
                        s.id.0, sm.id
                    )),
                ));
            }
        }
    }
    // The session loaded opens in an epoch newer than every one its traces were captured
    // in, and every session after it in a newer one still: a recorded epoch in the upper
    // half of the range is no epoch this rig reached, and would leave too few after it.
    for (i, t) in data.traces.iter().enumerate() {
        if let Some(e) = t.meta.source.shared_epoch()
            && e.0 >= MAX_LOADED_EPOCH
        {
            return Err((
                Item::Trace(i),
                invalid(format!(
                    "trace {} records stream epoch {}, at or past the {MAX_LOADED_EPOCH} a \
                     session file can hold: the file is damaged",
                    t.meta.id, e.0
                )),
            ));
        }
    }
    // Every owner is a measurement of the session that can own (not a math trace).
    let owner_ok = |o: TraceOwner| match o.meas() {
        None => true,
        Some(id) => data
            .measurements
            .iter()
            .any(|m| m.id == id && !matches!(m.config.kind, MeasKind::Math { .. })),
    };
    let owners =
        data.traces
            .iter()
            .enumerate()
            .map(|(i, t)| {
                (
                    Item::Trace(i),
                    format!("trace {}", t.meta.id),
                    t.meta.edit.owner,
                )
            })
            .chain(data.measurements.iter().enumerate().filter_map(
                |(i, m)| match &m.config.kind {
                    MeasKind::Math { config } => {
                        Some((Item::Meas(i), format!("math trace {}", m.id), config.owner))
                    }
                    _ => None,
                },
            ));
    for (item, what, owner) in owners {
        if !owner_ok(owner) {
            return Err((
                item,
                invalid(format!(
                    "{what} is filed under a measurement the session does not hold"
                )),
            ));
        }
    }
    // An SPL log belongs to an SPL meter of the session, one log each, and holds rows the
    // meter could have written: a log is the record of a meter, never kept without one.
    for (li, l) in data.spl_logs.iter().enumerate() {
        let item = |e| (Item::SplLog(li), e);
        let id = l.info.meas;
        match data.measurements.iter().find(|m| m.id == id) {
            None => {
                return Err(item(invalid(format!(
                    "SPL log of measurement {id}, which the session does not hold"
                ))));
            }
            Some(m) if !matches!(m.config.kind, MeasKind::Spl { .. }) => {
                return Err(item(invalid(format!(
                    "SPL log of measurement {id}, which is not an SPL meter"
                ))));
            }
            Some(_) => {}
        }
        if data.spl_logs[..li].iter().any(|o| o.info.meas == id) {
            return Err(item(invalid(format!("measurement {id} has two SPL logs"))));
        }
        let row = |i: usize, e: &str| {
            item(invalid(format!(
                "SPL log of measurement {id}, row {}: {e}",
                i + 1
            )))
        };
        for (i, r) in l.rows.iter().enumerate() {
            ac2_proto::spl_log::check_row(r).map_err(|e| row(i, e))?;
        }
        for (i, r) in l.bands.iter().enumerate() {
            ac2_traces::band_log::check_row(r).map_err(|e| row(i, e))?;
        }
    }
    for (i, t) in data.traces.iter().enumerate() {
        let item = |e: String| (Item::Trace(i), invalid(format!("trace {}: {e}", t.meta.id)));
        // Data a crafted file could hold that no capture produces (columns off the grid,
        // non-finite values, an unbounded impulse response).
        ac2_traces::check::trace(t).map_err(item)?;
        // A saved edit meets the limits `trace.update` enforces.
        meta::check_values(&t.meta.edit).map_err(item)?;
        if data.traces[..i].iter().any(|o| o.meta.id == t.meta.id) {
            return Err((
                Item::Trace(i),
                invalid(format!("trace {} appears twice in the session", t.meta.id)),
            ));
        }
    }
    Ok(grids)
}

/// What [`salvage`] took out of a session.
pub(super) struct SetAside {
    /// The items taken out, as a session of their own (a walk's plan copied along).
    pub(super) data: SessionData,
    /// One line per item: which, and why.
    pub(super) reasons: Vec<String>,
}

/// Takes out of `data`, one at a time, each item a load would refuse, until the rest loads:
/// a state the daemon wrote that this build refuses (a check added since) costs those
/// items, not the session. A measurement goes as `meas.delete` keeping its traces: what it
/// owned stays, filed under no measurement, except its SPL log and EQ curves, which exist
/// only with it. What depended on a removed item (a math trace's operand, a walk's
/// measurement) is refused in turn and goes too.
pub(super) fn salvage(data: &mut SessionData) -> SetAside {
    let mut aside = SessionData {
        saved_at: data.saved_at,
        measurements: Vec::new(),
        spl_logs: Vec::new(),
        traces: Vec::new(),
        walks: Vec::new(),
        plans: Vec::new(),
        remote_inputs: data.remote_inputs.clone(),
    };
    let mut reasons = Vec::new();
    loop {
        let scope = data.measurements.iter().enumerate().find_map(|(i, m)| {
            super::rig_meas::check_session_scope(m)
                .err()
                .map(|e| (Item::Meas(i), e))
        });
        let refused = scope.or_else(|| {
            check_items(data).err().or_else(|| {
                let ids: Vec<MeasId> = data.measurements.iter().map(|m| m.id).collect();
                super::walks::check_walk_items(&data.walks, &data.plans, &ids).err()
            })
        });
        let Some((item, e)) = refused else {
            break;
        };
        let what = match item {
            Item::Meas(i) => {
                let sm = data.measurements.remove(i);
                let what = format!("measurement {} ({})", sm.id, sm.config.name);
                // Its EQ curves and its log go with it; a duplicate's id is the kept one's,
                // whose stay.
                if !data.measurements.iter().any(|m| m.id == sm.id) {
                    let curves: Vec<_> = sm.eq.iter().map(|s| s.trace).collect();
                    let (gone, kept) = std::mem::take(&mut data.traces)
                        .into_iter()
                        .partition(|t| curves.contains(&t.meta.id));
                    data.traces = kept;
                    aside.traces.extend(gone);
                    let (gone, kept) = std::mem::take(&mut data.spl_logs)
                        .into_iter()
                        .partition(|l| l.info.meas == sm.id);
                    data.spl_logs = kept;
                    aside.spl_logs.extend(gone);
                    let owner = TraceOwner::Meas { meas: sm.id };
                    for t in &mut data.traces {
                        if t.meta.edit.owner == owner {
                            t.meta.edit.owner = TraceOwner::Imported;
                        }
                    }
                    for m in &mut data.measurements {
                        if let MeasKind::Math { config } = &mut m.config.kind
                            && config.owner == owner
                        {
                            config.owner = TraceOwner::Imported;
                        }
                    }
                }
                aside.measurements.push(sm);
                what
            }
            Item::Trace(i) => {
                let t = data.traces.remove(i);
                let what = format!("trace {} ({})", t.meta.id, t.meta.edit.name);
                aside.traces.push(t);
                what
            }
            Item::SplLog(i) => {
                let l = data.spl_logs.remove(i);
                let what = format!("the SPL log of measurement {}", l.info.meas);
                aside.spl_logs.push(l);
                what
            }
            Item::Plan(i) => {
                let p = data.plans.remove(i);
                let what = format!("plan {}", p.name);
                aside.plans.push(p);
                what
            }
            Item::Walk(i) => {
                let w = data.walks.remove(i);
                let what = format!("walk {} ({})", w.id, w.name);
                if let Some(p) = data.plans.iter().find(|p| p.name == w.plan)
                    && !aside.plans.iter().any(|a| a.name == p.name)
                {
                    aside.plans.push(p.clone());
                }
                aside.walks.push(w);
                what
            }
        };
        reasons.push(format!("{what}: {}", e.msg));
    }
    SetAside {
        data: aside,
        reasons,
    }
}
