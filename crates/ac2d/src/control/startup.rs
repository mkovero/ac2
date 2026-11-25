//! The control thread as it starts: the calibration, rig and autosave stores opened, the
//! system max level settled from the saved ceiling and its bound, and the state store seeded
//! from what was saved.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use ac2_proto::model::{Autosave, AutosaveState};
use ac2_proto::units::Dbfs;

use super::{
    Control, Keepalive, Output, Pending, RigState, Setup, SplLogs, artnet, clocks, file_jobs, ids,
    kept, osc, remote, rig_meas, sim, traces, transfer, walks, working,
};
use crate::autosave::Autosaver;
use crate::calstore::{self, CalStore};
use crate::dedup::Dedup;
use crate::jobs::{Probes, Seqs};
use crate::state::Store;
use crate::stimulus::LeaseGate;

impl Control {
    pub(crate) fn new(s: Setup) -> Self {
        let (mut cal, contents) = match &s.cal_store {
            Some(p) => CalStore::open(p),
            None => (CalStore::memory(), calstore::Contents::default()),
        };
        let (autosave, status) = match &s.autosave {
            None => (None, AutosaveState::Off),
            Some(c) => match crate::autosave::Writer::spawn(c.dir.clone(), s.to_self.clone()) {
                Ok(w) => (Some(Autosaver::new(c.dir.clone(), w)), AutosaveState::Saved),
                Err(e) => {
                    tracing::error!("autosave disabled: cannot start its thread: {e}");
                    (
                        None,
                        AutosaveState::Failed {
                            reason: format!("cannot start the autosave thread: {e}"),
                        },
                    )
                }
            },
        };
        let restore = s.autosave.as_ref().is_some_and(|c| c.restore);
        let (mut rig, saved) = match &s.rig_settings {
            Some(p) => crate::rig::RigStore::open(p),
            None => (
                crate::rig::RigStore::memory(),
                crate::rig::RigSettings::default(),
            ),
        };
        let mut s = s;
        let mut damaged = std::mem::take(&mut s.damaged);
        damaged.extend(cal.take_damage());
        damaged.extend(rig.take_damage());
        let ceiling = crate::rig::start_ceiling(saved.ceiling_dbfs, s.ceiling_bound);
        match crate::stimulus::peak_limit(ceiling) {
            Ok(m) => {
                s.ceiling_dbfs = ceiling;
                s.max_level = m;
            }
            // Unreachable for a checked file; the bound stays in force.
            Err(e) => tracing::error!("system max level {ceiling} dBFS: {e}"),
        }
        if s.ceiling_dbfs < s.ceiling_bound {
            tracing::info!(
                target: "ac2d::audit",
                "system max level {:.1} dBFS (bound {:.1} dBFS, --max-level)",
                s.ceiling_dbfs,
                s.ceiling_bound
            );
        }
        // Every part of the rig settings is in the state before anything commits: a write of
        // the file is made from the state, so a part held back until later would be written
        // as empty by any write in between (a start's own restore writes one).
        let store = Store::new(Dbfs(s.ceiling_dbfs), Dbfs(s.ceiling_bound), s.replay)
            .with_outputs(saved.outputs.clone())
            .with_rig(ac2_proto::model::Rig {
                monitor_at_start: saved.monitor_at_start,
                osc_target: saved.osc_target,
                artnet_target: saved.artnet_target,
                artnet_fade: saved.artnet_fade,
                levels: saved.levels,
                ..Default::default()
            })
            .with_peers(saved.peers.clone())
            .with_calibrations(contents)
            .with_plans(
                s.plan_dir
                    .as_deref()
                    .map(walks::load_plans)
                    .unwrap_or_default(),
            )
            .with_autosave(Autosave {
                state: status,
                saved_at: None,
                log_failures: Vec::new(),
            })
            .with_server(s.server.network.summary());
        let dedup = Dedup::new(s.dedup);
        let rigm = rig_meas::RigMeas::new(&saved, s.rig_logs.as_deref());
        let stored_ceiling = saved.ceiling_dbfs;
        let mut c = Self {
            store,
            cal,
            dedup,
            session: None,
            health: crate::health::Health::new(),
            audio_recovery: None,
            spare_links: Vec::new(),
            jobs: BTreeMap::new(),
            kept: kept::Kept::default(),
            probes: Arc::new(Probes::default()),
            timing_job: None,
            preview: None,
            pending: Pending::default(),
            reply_warnings: Vec::new(),
            ids: ids::Ids::new(rigm.next_id()),
            out: Output {
                lease: None,
                gate: Arc::new(LeaseGate::new()),
                level: None,
                source: None,
                tone: None,
            },
            grids: HashMap::new(),
            seqs: Arc::new(Seqs::default()),
            ka: Keepalive {
                seq: 0,
                due: Instant::now(),
            },
            clocks: clocks::ClockWatch::new(s.system_clock.read()),
            refusals: crate::refusals::Refusals::default(),
            damaged,
            notify: if s.systemd_notify {
                ac2_paths::notify::Notifier::from_env()
            } else {
                ac2_paths::notify::Notifier::disabled()
            },
            traces: traces::TraceStore::default(),
            sweep: None,
            dynamics: None,
            walk_resume: HashSet::new(),
            autosave,
            restore,
            logs: SplLogs {
                live: HashMap::new(),
                ended: HashMap::new(),
            },
            recording: None,
            replay_backend: None,
            replay_marks: None,
            working: working::Persist::untitled(),
            replaced_working: None,
            transfers: transfer::Transfers::default(),
            file_jobs: file_jobs::FileJobs::default(),
            sims: sim::Sims::default(),
            remote: remote::RemoteCtl::new(),
            rig: RigState {
                settings: rig,
                meas: rigm,
                osc: osc::Cues::default(),
                artnet: artnet::Universe::default(),
            },
            levels: Default::default(),
            s,
        };
        c.store_start_ceiling(stored_ceiling);
        c
    }

    /// Stores the level the daemon starts with when the file holds another (none, or one
    /// above a lowered bound). The level that protects the speakers then only ever rises by
    /// a typed, confirmed `gen.ceiling`: a later start with a higher `--max-level` reads
    /// this level back instead of taking its own bound, and lowering then raising the bound
    /// does not bring an older, higher level back.
    fn store_start_ceiling(&mut self, stored: Option<f64>) {
        let now = self.s.ceiling_dbfs;
        // A file read whole that holds the level in force needs nothing. Anything else
        // (no file, one set aside or recovered from its previous copy) is written now: after
        // the start the file always holds what is in force, so the next start cannot find
        // it missing and take a higher bound.
        if self.rig.settings.stored_ceiling() == Some(now) {
            return;
        }
        if self.rig.settings.check().is_err() {
            // Unreadable: never written, and said so at open.
            return;
        }
        let s = crate::rig::RigSettings {
            ceiling_dbfs: Some(now),
            ..self.rig_settings_now()
        };
        match self.rig.settings.persist(&s) {
            Ok(()) => {}
            Err(e) => {
                let then = match stored {
                    Some(s) if s > now => format!(
                        "the rig settings file still holds {s:.1} dBFS, which a later start \
                         with a higher --max-level takes"
                    ),
                    _ => "a later start finds no level and takes its --max-level bound".into(),
                };
                tracing::error!(
                    "system max level {now:.1} dBFS not stored ({}): {then}",
                    e.msg
                );
            }
        }
    }
}
