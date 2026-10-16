//! The rig's own SPL meters (`MeasScope::Rig`) and the monitor at start.
//!
//! A rig meter watches a noise limit for the rig, not for a session: whichever client loads
//! whatever session, its limits must stay in force and its log must go on. So:
//! - its configuration (with its id) lives in the rig settings (`rig.json`), written before
//!   each change is committed; session files and the autosave never hold it;
//! - `file.load`, `file.new`, salvage and the autosave restore replace the session's
//!   measurements only: a rig meter keeps running, keeps its log, its Leq history and its
//!   judgements;
//! - its log lives in a directory of its own ([`crate::autosave::RigLogs`]), not in the
//!   autosave a load replaces, and a restarted daemon carries on from it;
//! - session and rig meters share one id counter, so a meter keeps its id when it moves
//!   between them; a loaded session file (made elsewhere, numbered from 1) has the ids a
//!   rig meter holds renumbered past the counter ([`remap_ids`]).
//!
//! Traces are always the session's: a trace filed under a rig meter is saved in the
//! session as an import (no owner), so that the file holds nothing that names a
//! measurement it does not have; a load replaces such traces like any other. A math
//! channel filed under a rig meter is saved the same way.
//!
//! The monitor at start: with `rig.monitor_start` on, each successful `session.open` while
//! rig meters exist is recorded in the rig settings, and a restarted daemon reopens that
//! device capture-only (no output channels, no loopback), so the rig meters meter with no
//! client connected. With no output stream the generator cannot be routed anywhere
//! (`gen.set` refuses a session without outputs) and it starts disarmed as always, so
//! nothing can be emitted.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::time::Instant;

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    MathExpr, MeasKind, MeasScope, Measurement, Operand, Rig, SessionConfig, TraceOwner,
    TraceSource,
};
use ac2_proto::units::{MeasId, Rev, WallNs};
use ac2_proto::{Command, ErrorCode, ErrorDetail, ProtoError, ReplyBody, Warning};
use ac2_traces::session::{SavedMeasurement, Session as SessionData, SplLogOnDisk};

use super::{Control, recovery};
use crate::autosave::RigLogs;
use crate::rig::RigSettings;
use crate::util::{perr, perr_detail, wall_ns};

/// The control thread's rig meter state.
pub(crate) struct RigMeas {
    /// What the rig settings hold of the rig's meters, by id.
    saved: Vec<SavedMeasurement>,
    /// The audio configuration the monitor at start reopens.
    session: Option<SessionConfig>,
    /// Their logs on disk; `None`: in memory only.
    logs: Option<RigLogs>,
    /// The next attempt of the monitor at start to open the audio, and its number.
    retry: Option<(Instant, u32)>,
}

impl RigMeas {
    /// What the rig settings hold, with the logs kept in `logs_dir`.
    pub(crate) fn new(s: &RigSettings, logs_dir: Option<&Path>) -> Self {
        let logs = logs_dir.and_then(|d| match RigLogs::spawn(d.to_owned()) {
            Ok(l) => Some(l),
            Err(e) => {
                tracing::error!(
                    "rig SPL meters' logs in {} not kept: cannot start their thread: {e}",
                    d.display()
                );
                None
            }
        });
        let mut saved = s.measurements.clone();
        saved.sort_by_key(|m| m.id);
        Self {
            saved,
            session: s.monitor_session.clone(),
            logs,
            retry: None,
        }
    }

    /// The first id the session's counter may hand out.
    pub(crate) fn next_id(&self) -> u32 {
        self.saved
            .iter()
            .map(|m| m.id.0.saturating_add(1))
            .max()
            .unwrap_or(1)
    }
}

/// A measurement as a session file or the rig settings keep it.
pub(super) fn saved_of(m: &Measurement) -> SavedMeasurement {
    SavedMeasurement {
        id: m.id,
        config: m.config.clone(),
        running: m.running,
        delay: m.delay.as_ref().map(|d| ac2_traces::session::SavedDelay {
            applied: d.applied,
            nudged: d.nudged,
            tracking: d.tracking,
        }),
        colour: m.colour,
        layout: m.layout.clone(),
    }
}

/// A `rig.*` command made on rig settings that have changed since.
/// `cfg` without outputs: the monitor at start opens the capture side only.
fn capture_only(cfg: &SessionConfig) -> SessionConfig {
    SessionConfig {
        output_channels: 0,
        loopback: None,
        ..cfg.clone()
    }
}

impl Control {
    /// Whether `id` is one of the rig's meters.
    pub(super) fn is_rig(&self, id: MeasId) -> bool {
        self.meas_scope(id) == MeasScope::Rig
    }

    /// Whose measurement `id` is (an unknown id: the session's).
    pub(super) fn meas_scope(&self, id: MeasId) -> MeasScope {
        self.store
            .state()
            .measurements
            .iter()
            .find(|m| m.id == id)
            .map_or(MeasScope::Session, |m| m.config.scope)
    }

    /// Whether the rig has meters.
    fn has_rig_meters(&self) -> bool {
        !self.rigm.saved.is_empty()
    }

    /// The rig meters' ids, ascending.
    fn rig_meter_ids(&self) -> Vec<MeasId> {
        self.rigm.saved.iter().map(|m| m.id).collect()
    }

    /// `o` as a session file keeps it: a rig meter is no owner the file could name.
    pub(super) fn session_owner(&self, o: TraceOwner) -> TraceOwner {
        match o.meas() {
            Some(id) if self.is_rig(id) => TraceOwner::Imported,
            _ => o,
        }
    }

    /// Every rig setting as it stands, to persist with one of them changed.
    pub(super) fn rig_settings_now(&self) -> RigSettings {
        RigSettings {
            ceiling_dbfs: self.ceiling_set,
            outputs: self.store.state().outputs.clone(),
            measurements: self.rigm.saved.clone(),
            monitor_at_start: self.store.state().rig.monitor_at_start,
            monitor_session: self.rigm.session.clone(),
            osc_target: self.store.state().rig.osc_target,
        }
    }

    /// `conflict` unless `base` is the rig's rev (`osc.target`, `rig.monitor_start`).
    pub(super) fn rig_base(&self, base: Rev) -> Result<(), ProtoError> {
        let rev = self.store.state().rig.rev;
        if base == rev {
            return Ok(());
        }
        Err(perr_detail(
            ErrorCode::Conflict,
            format!(
                "the rig settings changed since (now at rev {}); re-read them",
                rev.0
            ),
            ErrorDetail::Conflict { rev },
        ))
    }

    /// Commits the rig entity changed by `f`, under a new `rig.rev`.
    pub(super) fn commit_rig(&mut self, f: impl FnOnce(&mut Rig)) -> Rev {
        let mut rig = self.store.state().rig.clone();
        f(&mut rig);
        rig.rev = Rev(self.store.rev().0 + 1);
        self.commit(Change::Rig(rig))
    }

    /// Refuses a change to a rig meter when the rig settings cannot be written (an
    /// unreadable file is never overwritten): a meter that would quietly come back
    /// different after a restart is worse than a refused change.
    pub(super) fn rig_preflight(&self, cmd: &Command) -> Result<(), ProtoError> {
        let touches = match cmd {
            Command::MeasCreate { config } => config.scope == MeasScope::Rig,
            Command::MeasUpdate { meas, config, .. } => {
                config.scope == MeasScope::Rig || self.is_rig(*meas)
            }
            Command::MeasColour { meas, .. }
            | Command::MeasLayout { meas, .. }
            | Command::MeasDelete { meas, .. }
            | Command::MeasStart { meas }
            | Command::MeasStop { meas } => self.is_rig(*meas),
            _ => false,
        };
        if touches { self.rig.check() } else { Ok(()) }
    }

    /// Called with every change before it is committed: a change to what the rig settings
    /// hold of a rig meter is written there first.
    pub(super) fn rig_meas_committing(&mut self, change: &Change) {
        let Change::Measurement(p) = change else {
            return;
        };
        let mut next = self.rigm.saved.clone();
        match p {
            Patch::Set(m) => {
                next.retain(|s| s.id != m.id);
                if m.config.scope == MeasScope::Rig {
                    next.push(saved_of(m));
                    next.sort_by_key(|s| s.id);
                }
            }
            Patch::Deleted(id) => next.retain(|s| s.id != *id),
        }
        if next == self.rigm.saved {
            return;
        }
        let s = RigSettings {
            measurements: next.clone(),
            ..self.rig_settings_now()
        };
        if let Err(e) = self.rig.persist(&s) {
            // The commands checked the store is writable first; this is a failed write.
            tracing::error!("rig SPL meters not saved: {}", e.msg);
        }
        self.rigm.saved = next;
    }

    /// Prepares a session about to replace the working one: refuses a rig meter in it and
    /// renumbers the measurements whose ids rig meters hold.
    pub(super) fn prepare_load(&mut self, data: &mut SessionData) -> Result<(), ProtoError> {
        if let Some(m) = data
            .measurements
            .iter()
            .find(|m| m.config.scope == MeasScope::Rig)
        {
            return Err(perr(
                ErrorCode::Invalid,
                format!(
                    "measurement {} is a rig meter: a session holds session measurements only",
                    m.id
                ),
            ));
        }
        let taken: BTreeSet<MeasId> = self.rigm.saved.iter().map(|m| m.id).collect();
        let map = remap_ids(data, &taken, &mut self.next_meas);
        for (from, to) in &map {
            tracing::info!(
                "session load: measurement {from} is numbered {to} (a rig meter holds {from})"
            );
        }
        Ok(())
    }

    /// Daemon start, before the autosave restore: the rig meters from the rig settings,
    /// with their logs from the rig log directory.
    pub(super) fn load_rig_meas(&mut self) {
        if !self.has_rig_meters() {
            return;
        }
        let saved = self.rigm.saved.clone();
        // Out while the meters load: their logs are handed over once, with where their
        // files stand.
        let mut logs = self.rigm.logs.take();
        let read = logs.as_ref().and_then(RigLogs::read);
        let (mut spl_logs, resume): (Vec<_>, Vec<SplLogOnDisk>) =
            read.map(|r| (r.data.spl_logs, r.logs)).unwrap_or_default();
        spl_logs.retain(|l| saved.iter().any(|m| m.id == l.info.meas));
        let mut data = SessionData {
            saved_at: WallNs(wall_ns()),
            measurements: saved.clone(),
            spl_logs,
            traces: Vec::new(),
        };
        // The logs a load would refuse are not read back; the meters start new ones.
        let mut grids = match super::files::check_session(&data) {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("rig SPL meters' logs not read back: {}", e.msg);
                data.spl_logs.clear();
                super::files::check_session(&data).unwrap_or_default()
            }
        };
        for l in data.spl_logs {
            self.set_spl_log(
                l.info.meas,
                crate::leq_log::LeqLog::from_rows(l.rows, l.bands),
            );
        }
        for sm in saved {
            let grid_id = grids.remove(&sm.id).map(|g| self.register_grid(g));
            let mut m = Measurement {
                id: sm.id,
                running: sm.running && sm.config.kind.is_job(),
                config: sm.config,
                config_rev: Rev(0),
                delay: None,
                grid_id,
                colour: sm.colour.filter(|c| c.is_valid()),
                layout: sm.layout.filter(|l| l.check().is_ok()),
            };
            self.ensure_spl_log(&m, None);
            m.config_rev = Rev(self.store.rev().0 + 1);
            self.commit(Change::Measurement(Patch::Set(m)));
        }
        let n = self.rigm.saved.len();
        let dir = logs
            .as_ref()
            .map_or_else(|| "-".into(), |l| l.dir().display().to_string());
        tracing::info!("{n} rig SPL meter(s) loaded from the rig settings (logs in {dir})");
        let (_, fp) = self.rig_logs_manifest();
        let specs = self.spl_log_specs(MeasScope::Rig, &resume);
        if let Some(l) = logs.as_mut() {
            l.restored(fp, specs);
        }
        self.rigm.logs = logs;
    }

    /// The rig log directory's manifest as of now, and its fingerprint.
    fn rig_logs_manifest(&self) -> (SessionData, crate::autosave::Fingerprint) {
        let data = SessionData {
            saved_at: WallNs(wall_ns()),
            measurements: self.rigm.saved.clone(),
            spl_logs: Vec::new(),
            traces: Vec::new(),
        };
        let fp = (
            self.rigm.saved.clone(),
            Vec::new(),
            self.spl_log_ids(MeasScope::Rig),
        );
        (data, fp)
    }

    /// The measurements or their logs changed: the rig log directory follows.
    pub(super) fn rig_logs_changed(&mut self) {
        if self.rigm.logs.is_none() {
            return;
        }
        let (data, fp) = self.rig_logs_manifest();
        let specs = self.spl_log_specs(MeasScope::Rig, &[]);
        if let Some(l) = self.rigm.logs.as_mut() {
            l.changed(data, fp, specs);
        }
    }

    /// Shutdown: the rig meters' logs reach the disk.
    pub(super) fn finish_rig_logs(&mut self) {
        self.rig_logs_changed();
        if let Some(l) = self.rigm.logs.as_mut() {
            l.finish();
        }
    }

    /// `rig.monitor_start`.
    pub(super) fn rig_monitor_start(
        &mut self,
        base: Rev,
        on: bool,
    ) -> Result<ReplyBody, ProtoError> {
        self.rig_base(base)?;
        if self.store.state().rig.monitor_at_start == on {
            return Ok(ReplyBody::Ack {
                rev: self.store.rev(),
            });
        }
        // Turned on with the audio open: that is the device to come back to.
        let session = match self.open_config() {
            Some(c) if on && self.has_rig_meters() => Some(c),
            _ => self.rigm.session.clone(),
        };
        self.rig.persist(&RigSettings {
            monitor_at_start: on,
            monitor_session: session.clone(),
            ..self.rig_settings_now()
        })?;
        self.rigm.session = session;
        let rev = self.commit_rig(|r| r.monitor_at_start = on);
        tracing::info!(
            target: "ac2d::audit",
            "monitor at start {}",
            if on { "on" } else { "off" }
        );
        Ok(ReplyBody::Ack { rev })
    }

    /// The configuration the open session was opened with by a client (never a replay).
    fn open_config(&self) -> Option<SessionConfig> {
        self.session
            .as_ref()
            .filter(|rt| rt.open.replay.is_none())
            .map(|rt| rt.open.config.clone())
    }

    /// `session.open`: warns when rig meters move to another device, records the
    /// configuration for the monitor at start.
    pub(super) fn rig_session_open(
        &mut self,
        client: &ac2_proto::units::ClientId,
        config: SessionConfig,
    ) -> Result<ReplyBody, ProtoError> {
        let moved = self.has_rig_meters()
            && self.open_config().is_some_and(|old| {
                old.backend != config.backend || old.input_device != config.input_device
            });
        let r = self.session_open(client, config.clone())?;
        if moved {
            tracing::warn!(
                "session.open on another device while {} rig SPL meter(s) run: they meter its \
                 inputs from now on",
                self.rigm.saved.len()
            );
            self.reply_warnings.push(Warning::RigMonitorMoved {
                meas: self.rig_meter_ids(),
            });
        }
        self.rigm.retry = None;
        if self.store.state().rig.monitor_at_start
            && self.has_rig_meters()
            && self.rigm.session.as_ref() != Some(&config)
        {
            let s = RigSettings {
                monitor_session: Some(config.clone()),
                ..self.rig_settings_now()
            };
            match self.rig.persist(&s) {
                Ok(()) => self.rigm.session = Some(config),
                Err(e) => tracing::warn!("monitor at start: session not recorded: {}", e.msg),
            }
        }
        Ok(r)
    }

    /// `session.close` is about to end the audio the rig meters meter.
    pub(super) fn rig_session_closing(&mut self) {
        self.rigm.retry = None;
        if self.has_rig_meters() && (self.session.is_some() || self.recovery.is_some()) {
            tracing::warn!(
                "session.close: {} rig SPL meter(s) stop metering until a session opens again",
                self.rigm.saved.len()
            );
            self.reply_warnings.push(Warning::RigMonitorBlind {
                meas: self.rig_meter_ids(),
            });
        }
    }

    /// Daemon start, after the autosave restore: with the monitor at start on, reopens the
    /// recorded device capture-only.
    pub(super) fn start_rig_monitor(&mut self) {
        if !self.store.state().rig.monitor_at_start {
            return;
        }
        if !self.has_rig_meters() {
            tracing::info!("monitor at start: no rig SPL meters; the audio stays closed");
            return;
        }
        if self.rigm.session.is_none() {
            tracing::info!(
                "monitor at start: no session recorded yet (one is, at the next session.open); \
                 the audio stays closed"
            );
            return;
        }
        self.monitor_open(1);
    }

    /// Attempt `attempt` of the monitor at start to open the audio.
    fn monitor_open(&mut self, attempt: u32) {
        self.rigm.retry = None;
        let Some(cfg) = self.rigm.session.as_ref().map(capture_only) else {
            return;
        };
        let daemon = ac2_proto::units::ClientId("ac2d".into());
        match self.open_session(&daemon, cfg, None) {
            Ok(s) => {
                let dev = s
                    .open
                    .as_ref()
                    .map_or_else(String::new, |o| format!("{:?}", o.input_device));
                tracing::info!(
                    target: "ac2d::audit",
                    "monitor at start: audio opened capture-only on {dev} (no outputs, the \
                     generator disarmed); {} rig SPL meter(s) metering",
                    self.rigm.saved.len()
                );
            }
            Err(e) => {
                let wait = recovery::backoff(attempt);
                tracing::warn!(
                    "monitor at start: attempt {attempt} to open the audio failed: {}; next in \
                     {} s",
                    e.msg,
                    wait.as_secs()
                );
                self.rigm.retry = Some((Instant::now() + wait, attempt + 1));
            }
        }
    }

    /// When the control loop must wake for the monitor at start.
    pub(super) fn rig_due(&self) -> Option<Instant> {
        self.rigm.retry.map(|(at, _)| at)
    }

    /// The monitor at start's next attempt, when due and no session came meanwhile.
    pub(super) fn rig_tick(&mut self, now: Instant) {
        let Some((at, attempt)) = self.rigm.retry else {
            return;
        };
        if self.session.is_some() || self.recovery.is_some() {
            self.rigm.retry = None;
        } else if now >= at {
            self.monitor_open(attempt);
        }
    }
}

/// Renumbers the measurements of `data` whose ids are `taken`, from `next` on (past every
/// id taken or in `data`), and every reference to them. Returns what moved where.
pub(super) fn remap_ids(
    data: &mut SessionData,
    taken: &BTreeSet<MeasId>,
    next: &mut u32,
) -> Vec<(MeasId, MeasId)> {
    let ids: BTreeSet<MeasId> = data.measurements.iter().map(|m| m.id).collect();
    let mut map = HashMap::new();
    for id in ids.iter().filter(|i| taken.contains(i)) {
        loop {
            let c = MeasId(*next);
            *next = next.saturating_add(1);
            if !taken.contains(&c) && !ids.contains(&c) {
                map.insert(*id, c);
                break;
            }
        }
    }
    if map.is_empty() {
        return Vec::new();
    }
    let re = |id: &mut MeasId| {
        if let Some(n) = map.get(id) {
            *id = *n;
        }
    };
    let owner = |o: &mut TraceOwner| {
        if let TraceOwner::Meas { meas } = o {
            re(meas);
        }
    };
    let expr = |e: &mut MathExpr| {
        let ops: Vec<&mut Operand> = match e {
            MathExpr::Binary { a, b, .. } => vec![a, b],
            MathExpr::Average { of, .. } => of.iter_mut().collect(),
        };
        for o in ops {
            if let Operand::Meas { meas } = o {
                re(meas);
            }
        }
    };
    for m in &mut data.measurements {
        re(&mut m.id);
        if let MeasKind::Math { config } = &mut m.config.kind {
            owner(&mut config.owner);
            expr(&mut config.expr);
        }
    }
    for t in &mut data.traces {
        owner(&mut t.meta.edit.owner);
        match &mut t.meta.source {
            TraceSource::Captured { meas, .. } | TraceSource::Sweep { meas, .. } => re(meas),
            TraceSource::Math { meas, expr: e, .. } => {
                re(meas);
                expr(e);
            }
            _ => {}
        }
    }
    for l in &mut data.spl_logs {
        re(&mut l.info.meas);
    }
    let mut moved: Vec<_> = map.into_iter().collect();
    moved.sort();
    moved
}
