//! `ac2 session salvage` of a session this machine can read: the measurements and stored
//! traces of a session of another format version, made again in the current measurements
//! and traces with the commands a client always uses. The old file is read here
//! ([`read`]); the daemon only ever reads its own version and checks each command as usual.
//!
//! Measurements first (math channels last: they name other measurements and traces), each
//! with its colour, layout and delay; then each trace file imported, given its old
//! properties and filed under its measurement; then the measurements that were running are
//! started where the open audio session captures their inputs. A summary names what was
//! restored, what was skipped and every setting not read as saved.

mod read;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ac2_client::{Client, expect_body};
use ac2_proto::model::{
    ImportFormat, ImportRole, MathConfig, MathExpr, MathReference, MeasConfig, MeasKind, Operand,
    PaneLayout, TraceEdit, TraceKind, TraceOwner,
};
use ac2_proto::units::{Blob, MeasId, TraceId};
use ac2_proto::{Command, ReplyBody};
use serde::Serialize;

use super::meas::meas_call;
use super::meas_config::uncaptured;
use super::state;
use crate::CliError;
use crate::output::{self, Out};

/// What a salvage did, item by item.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Report {
    /// The session as given.
    pub session: String,
    /// Its format version.
    pub version: Option<u64>,
    /// One per measurement, trace, SPL log and unreadable entry.
    pub items: Vec<Item>,
}

/// One row of the report.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Item {
    /// What it is.
    pub what: What,
    /// Its name in the old session.
    pub name: String,
    /// Its id now, when restored.
    pub id: Option<u32>,
    /// Restored or not.
    pub restored: bool,
    /// What was not restored as saved, or why it was skipped.
    pub notes: Vec<String>,
}

/// Kinds of report rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum What {
    Measurement,
    Trace,
    SplLog,
    Entry,
}

/// The session folder `arg` names on this machine, unpacking an `.ac2session` file into a
/// temporary folder; `None` when it is not here (a name or a path of the daemon's host).
pub(crate) fn local_session(arg: &str) -> Result<Option<Local>, CliError> {
    let p = Path::new(arg);
    if ac2_archive::is_archive_name(p) && p.is_file() {
        let bytes = std::fs::read(p)?;
        let dir = std::env::temp_dir().join(format!(
            "ac2-salvage-{}-{}",
            std::process::id(),
            crate::watch::now_wall().0
        ));
        let local = Local {
            dir: dir.clone(),
            temporary: true,
        };
        ac2_archive::unpack(&bytes, &dir).map_err(|e| CliError::Usage(format!("{arg}: {e}")))?;
        return Ok(Some(local));
    }
    if super::sessions::is_path(arg) && ac2_archive::is_session_dir(p) {
        return Ok(Some(Local {
            dir: p.to_owned(),
            temporary: false,
        }));
    }
    Ok(None)
}

/// A session folder read here.
#[derive(Debug)]
pub(crate) struct Local {
    dir: PathBuf,
    temporary: bool,
}

impl Drop for Local {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Restores `local` into the daemon's measurements and traces and prints the summary.
pub(crate) async fn run(
    c: &Client,
    shown: &str,
    local: &Local,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let plan = read::read(&local.dir).map_err(CliError::Usage)?;
    let report = restore(c, shown, &local.dir, plan).await?;
    out.emit(&report, || summary(&report))?;
    Ok(())
}

async fn restore(
    c: &Client,
    shown: &str,
    dir: &Path,
    plan: read::Plan,
) -> Result<Report, CliError> {
    let mut items = Vec::new();
    let mut meas_ids: HashMap<MeasId, MeasId> = HashMap::new();
    let mut trace_ids: HashMap<TraceId, TraceId> = HashMap::new();
    let mut running: Vec<(usize, MeasId, MeasKind)> = Vec::new();
    let mut layouts: Vec<(usize, MeasId, PaneLayout)> = Vec::new();

    let names: HashMap<MeasId, String> = plan
        .measurements
        .iter()
        .filter_map(|m| m.old_id.map(|id| (id, m.config.name.clone())))
        .collect();
    let (math, plain): (Vec<_>, Vec<_>) = plan
        .measurements
        .into_iter()
        .partition(|m| matches!(m.config.kind, MeasKind::Math { .. }));
    for m in plain {
        let config = m.config.clone();
        restore_meas(
            c,
            m,
            config,
            &mut items,
            &mut meas_ids,
            &mut running,
            &mut layouts,
        )
        .await;
    }

    for t in plan.traces {
        items.push(restore_trace(c, dir, t, &meas_ids, &mut trace_ids).await);
    }

    // A math channel may compute from another: make each whose operands exist, until a
    // round makes none.
    let mut pending = math;
    loop {
        let before = pending.len();
        let mut left = Vec::new();
        for m in pending {
            match remap_math(&m.config, &meas_ids, &trace_ids) {
                Ok(config) => {
                    restore_meas(
                        c,
                        m,
                        config,
                        &mut items,
                        &mut meas_ids,
                        &mut running,
                        &mut layouts,
                    )
                    .await;
                }
                Err(_) => left.push(m),
            }
        }
        pending = left;
        if pending.is_empty() || pending.len() == before {
            break;
        }
    }
    for m in pending {
        let reason = remap_math(&m.config, &meas_ids, &trace_ids)
            .err()
            .unwrap_or_default();
        let mut notes = m.notes;
        notes.insert(0, reason);
        items.push(Item {
            what: What::Measurement,
            name: m.config.name,
            id: None,
            restored: false,
            notes,
        });
    }
    for (i, id, mut l) in layouts {
        // Each pane's pick names the measurement by its id in the old session; one not
        // made again shows the layout's own measurement.
        for p in l.panes_mut() {
            p.meas = p.meas.and_then(|old| meas_ids.get(&old).copied());
        }
        if let Err(e) = c
            .call(Command::MeasLayout {
                meas: id,
                layout: Some(l),
            })
            .await
        {
            items[i].notes.push(format!("layout not set: {e}"));
        }
    }

    start_running(c, &mut items, running).await?;

    for l in plan.spl_logs {
        let name = l
            .meas
            .and_then(|old| names.get(&old).cloned())
            .unwrap_or_else(|| l.file.clone());
        items.push(Item {
            what: What::SplLog,
            name,
            id: None,
            restored: false,
            notes: vec![format!(
                "per-second Leq history is not restored: its Leq windows start again \
                     (the log stays in the old session, {})",
                l.file
            )],
        });
    }
    for u in plan.unread {
        items.push(Item {
            what: What::Entry,
            name: u.what,
            id: None,
            restored: false,
            notes: vec![u.reason],
        });
    }
    Ok(Report {
        session: shown.to_owned(),
        version: plan.version,
        items,
    })
}

async fn restore_meas(
    c: &Client,
    m: read::OldMeas,
    config: MeasConfig,
    items: &mut Vec<Item>,
    meas_ids: &mut HashMap<MeasId, MeasId>,
    running: &mut Vec<(usize, MeasId, MeasKind)>,
    layouts: &mut Vec<(usize, MeasId, PaneLayout)>,
) {
    let mut notes = m.notes;
    let name = config.name.clone();
    let kind = config.kind.clone();
    let created = match meas_call(c, Command::MeasCreate { config }).await {
        Ok(created) => created,
        Err(e) => {
            notes.insert(0, format!("not created: {e}"));
            items.push(Item {
                what: What::Measurement,
                name,
                id: None,
                restored: false,
                notes,
            });
            return;
        }
    };
    let id = created.id;
    if let Some(old) = m.old_id {
        meas_ids.insert(old, id);
    }
    if m.colour.is_some()
        && let Err(e) = c
            .call(Command::MeasColour {
                meas: id,
                colour: m.colour,
            })
            .await
    {
        notes.push(format!("colour not set: {e}"));
    }
    // Set once every measurement is made: its panes may be picked for one made later.
    if let Some(l) = m.layout {
        layouts.push((items.len(), id, l));
    }
    if let Some(d) = m.delay {
        if d.applied.0 != 0.0 {
            match c
                .call(Command::DelaySet {
                    meas: id,
                    delay: d.applied,
                })
                .await
            {
                // `delay.set` measures the offset from the arrival of the last insert; a
                // new measurement has none, so the whole delay reads as the offset.
                Ok(_) if d.nudged != d.applied => notes.push(format!(
                    "delay {} applied; its arrival ({}) is not kept: nudges count from 0",
                    ac2_scene::format::delay(d.applied.0),
                    ac2_scene::format::delay(d.applied.0 - d.nudged.0)
                )),
                Ok(_) => {}
                Err(e) => notes.push(format!(
                    "delay {} not applied: {e}",
                    ac2_scene::format::delay(d.applied.0)
                )),
            }
        }
        if d.tracking
            && let Err(e) = c
                .call(Command::DelayTrack {
                    meas: id,
                    enabled: true,
                })
                .await
        {
            notes.push(format!("delay tracking not enabled: {e}"));
        }
    }
    if m.running {
        running.push((items.len(), id, kind));
    }
    items.push(Item {
        what: What::Measurement,
        name,
        id: Some(id.0),
        restored: true,
        notes,
    });
}

/// Starts the measurements that were running, each only where the open audio session
/// captures every input it reads: a start the daemon would refuse is reported instead.
async fn start_running(
    c: &Client,
    items: &mut [Item],
    running: Vec<(usize, MeasId, MeasKind)>,
) -> Result<(), CliError> {
    if running.is_empty() {
        return Ok(());
    }
    let open = state(c).await?.session.open;
    for (row, id, kind) in running {
        let Some(item) = items.get_mut(row) else {
            continue;
        };
        if !kind.is_job() {
            continue;
        }
        let why = match &open {
            None => Some("no audio session is open".to_owned()),
            Some(o) => uncaptured(&kind, &o.config.input_channels)
                .err()
                .map(|e| e.to_string()),
        };
        if let Some(why) = why {
            item.notes.push(format!(
                "was running; left stopped: {why} (`ac2 meas start {}` once it is)",
                id.0
            ));
            continue;
        }
        if let Err(e) = c.call(Command::MeasStart { meas: id }).await {
            item.notes.push(format!("was running; start refused: {e}"));
        }
    }
    Ok(())
}

async fn restore_trace(
    c: &Client,
    dir: &Path,
    t: read::OldTrace,
    meas_ids: &HashMap<MeasId, MeasId>,
    trace_ids: &mut HashMap<TraceId, TraceId>,
) -> Item {
    let mut notes = t.notes;
    let item = |id: Option<TraceId>, notes: Vec<String>| Item {
        what: What::Trace,
        name: t.name.clone(),
        id: id.map(|i| i.0),
        restored: id.is_some(),
        notes,
    };
    let content = match std::fs::read(dir.join(&t.file)) {
        Ok(b) => b,
        Err(e) => {
            notes.insert(0, format!("{}: {e}", t.file));
            return item(None, notes);
        }
    };
    // A target stays a target; everything else comes back as a measured trace.
    let role = if ac2_traces::text::header_kind(&content) == Some(TraceKind::Target) {
        ImportRole::Target
    } else {
        ImportRole::Trace
    };
    let imported = c
        .call(Command::TraceImport {
            file_name: t.file.clone(),
            format: ImportFormat::Ac2Csv,
            role,
            ir: None,
            content: Blob(content),
        })
        .await
        .map_err(CliError::from)
        .and_then(|r| Ok(expect_body!("trace.import", r, ReplyBody::Trace(t) => t)?));
    let imported = match imported {
        Ok(m) => m,
        Err(e) => {
            notes.insert(0, format!("not imported: {e}"));
            return item(None, notes);
        }
    };
    if let Some(old) = t.old_id {
        trace_ids.insert(old, imported.id);
    }
    let mut edit_notes = Vec::new();
    let mut edit: TraceEdit = read::lenient(&t.edit, Some(&imported.edit), "", &mut edit_notes)
        .unwrap_or_else(|| {
            notes.push("its display properties were unreadable: the import's are kept".into());
            imported.edit.clone()
        });
    notes.extend(edit_notes.into_iter().filter(|n| !n.starts_with("`owner`")));
    edit.owner = match t.owner {
        TraceOwner::Meas { meas } => match meas_ids.get(&meas) {
            Some(&now) => TraceOwner::Meas { meas: now },
            None => {
                notes.push(format!(
                    "its measurement (old id {}) was not restored: filed under Imported",
                    meas.0
                ));
                TraceOwner::Imported
            }
        },
        TraceOwner::Imported => TraceOwner::Imported,
    };
    if edit != imported.edit
        && let Err(e) = c
            .call(Command::TraceUpdate {
                trace: imported.id,
                edit,
            })
            .await
    {
        notes.push(format!(
            "its name, display properties and measurement were not set: {e}"
        ));
    }
    if let Some(curve) = &t.mic_curve {
        notes.push(format!(
            "mic curve {curve} applied after capture is not applied again"
        ));
    }
    item(Some(imported.id), notes)
}

/// `config` with the old measurement and trace ids it names changed to the restored ones;
/// `Err` names the first it names that was not restored.
fn remap_math(
    config: &MeasConfig,
    meas: &HashMap<MeasId, MeasId>,
    traces: &HashMap<TraceId, TraceId>,
) -> Result<MeasConfig, String> {
    let MeasKind::Math { config: m } = &config.kind else {
        return Ok(config.clone());
    };
    let operand = |o: Operand| match o {
        Operand::Meas { meas: old } => meas
            .get(&old)
            .map(|&meas| Operand::Meas { meas })
            .ok_or_else(|| format!("computes from measurement (old id {}), not restored", old.0)),
        Operand::Trace { trace: old } => traces
            .get(&old)
            .map(|&trace| Operand::Trace { trace })
            .ok_or_else(|| format!("computes from trace (old id {}), not restored", old.0)),
    };
    let owner = match m.owner {
        TraceOwner::Meas { meas: old } => TraceOwner::Meas {
            meas: *meas
                .get(&old)
                .ok_or_else(|| format!("its measurement (old id {}) was not restored", old.0))?,
        },
        TraceOwner::Imported => TraceOwner::Imported,
    };
    let expr = match &m.expr {
        MathExpr::Binary { a, op, b } => MathExpr::Binary {
            a: operand(*a)?,
            op: *op,
            b: operand(*b)?,
        },
        MathExpr::Average { of, method } => MathExpr::Average {
            of: of.iter().map(|o| operand(*o)).collect::<Result<_, _>>()?,
            method: *method,
        },
    };
    let reference = match m.reference {
        MathReference::Operand { operand: o } => MathReference::Operand {
            operand: operand(o)?,
        },
        r @ MathReference::Fixed { .. } => r,
    };
    Ok(MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name: config.name.clone(),
        kind: MeasKind::Math {
            config: MathConfig {
                owner,
                expr,
                reference,
                ..m.clone()
            },
        },
    })
}

/// The summary: a table of every item, then each note under its item's name.
fn summary(r: &Report) -> String {
    let version = r
        .version
        .map_or_else(|| "unknown format".to_owned(), |v| format!("format {v}"));
    let mut t = output::table(&["item", "name", "now", "result"]);
    for i in &r.items {
        t.add_row([
            match i.what {
                What::Measurement => "measurement",
                What::Trace => "trace",
                What::SplLog => "SPL log",
                What::Entry => "entry",
            }
            .to_owned(),
            i.name.clone(),
            i.id.map_or_else(|| "–".to_owned(), |id| id.to_string()),
            match (i.restored, i.notes.is_empty()) {
                (true, true) => "restored",
                (true, false) => "restored, see notes",
                (false, _) => "skipped",
            }
            .to_owned(),
        ]);
    }
    let restored = r.items.iter().filter(|i| i.restored).count();
    let mut s = format!(
        "salvaged {} ({version}): {restored} restored, {} skipped\n{t}",
        r.session,
        r.items.len() - restored
    );
    let noted: Vec<&Item> = r.items.iter().filter(|i| !i.notes.is_empty()).collect();
    if !noted.is_empty() {
        s.push_str("\nnotes:");
        for i in noted {
            for n in &i.notes {
                s.push_str(&format!("\n  {}: {n}", i.name));
            }
        }
    }
    s
}
