//! The control thread: decodes requests, executes commands one at a time against the state
//! store (serial commits), runs the session and jobs, owns the stimulus lease and emits
//! keepalives.
//!
//! It is a plain thread fed by one channel. Commands are short (job threads do the DSP),
//! and a single consumer gives serial commits and a total event order for free; an async
//! runtime would add scheduling without adding concurrency that the state model allows.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use ac2_audio::{Backend, MaxLevel};
use ac2_core::generator::{
    BandLimit as CoreBandLimit, Generator as CoreGenerator, GeneratorConfig, LevelControl,
    Signal as CoreSignal, dbfs_to_rms,
};
use ac2_core::sweep::{SweepAnalysis, SweepError, SweepSpec};
use ac2_proto::event::{Change, Patch};
use ac2_proto::grid::{GridDef, GridId};
use ac2_proto::model::{
    Autosave, AutosaveState, CalState, DelayState, GenAction, GeneratorSettings, LoopbackDetection,
    MeasKind, Measurement, MicState, SweepConfig, SweepFailure, SweepRun, SweepStatus,
    TimingStatus, TraceKind, TraceMeta, TraceSource,
};
use ac2_proto::units::{
    ClientId, DaemonIncarnation, Dbfs, LeaseToken, MeasId, RequestId, Rev, Seconds, SessionEpoch,
    SweepId, WallNs,
};
use ac2_proto::{Command, ErrorCode, ErrorDetail, ProtoError, ReplyBody, Welcome};
use ac2_zmq::Context;

use crate::autosave::Autosaver;
use crate::calstore::{self, CalStore};
use crate::config::{DedupLimits, ReplayLimits};
use crate::dedup::Dedup;
use crate::io::Interest;
use crate::jobs::{self, JobCmd, JobHandle, Probes, Seqs, block_index};
use crate::outbox::Outbox;
use crate::preview::Preview;
use crate::session::{Limits, Runtime};
use crate::state::Store;
use crate::stimulus::{LeaseGate, LeasedSource, SweepTrain};
use crate::sweep::Recording;
use crate::util::{perr, perr_detail};

mod autosave;
mod cal;
mod files;
mod generator;
mod leq;
mod maths;
mod meas_jobs;
mod owners;
mod recording;
mod recovery;
mod request;
pub(crate) mod rig;
mod session_ctl;
mod sweeps;
mod traces;
mod validate;

use validate::{
    MAX_CAL_UNSETTLED_DB, MAX_DELAY_S, check_find, delay_samples, gen_err, lease_required,
    mutation_conflict, not_a_sweep, not_found, smoothing_only, spl_in_place, static_grid,
    upsert_inputs, validate_meas,
};

/// Everything that reaches the control thread.
pub(crate) enum ControlMsg {
    /// A ctrl request from the I/O thread.
    Request {
        routing_id: Vec<u8>,
        user_id: Option<String>,
        payload: Vec<u8>,
    },
    /// The stream of `epoch` reported a configuration change.
    DeviceChanged { epoch: SessionEpoch },
    /// The stream of `epoch` stopped: no audio since `since`, for `cause`.
    AudioStopped {
        epoch: SessionEpoch,
        since: WallNs,
        cause: ac2_proto::model::StopCause,
    },
    /// Reopen attempt `token` of a stopped session finished.
    Reopened {
        token: u64,
        result: Box<Result<Runtime, ProtoError>>,
    },
    /// Availability probe `token` of a stopped session's device answered.
    Probed {
        token: u64,
        presence: ac2_audio::Presence,
    },
    /// The timing monitor of `epoch` changed state.
    Timing {
        epoch: SessionEpoch,
        status: TimingStatus,
    },
    /// A `delay.find` started under `token` finished.
    DelayFound {
        token: u64,
        result: Box<Result<ac2_core::delay::FinderResult, String>>,
    },
    /// Delay tracking of `meas` agreed on a new arrival, in (fractional) samples.
    DelayTracked {
        meas: MeasId,
        epoch: SessionEpoch,
        samples: f64,
    },
    /// A `session.detect_loopback` started under `token` finished.
    LoopbackDetected {
        token: u64,
        result: Box<Result<LoopbackDetection, ProtoError>>,
    },
    /// Sweep `id` started playing repeat `repeat` (1-based).
    SweepProgress { id: SweepId, repeat: u8 },
    /// Sweep `id` has its whole recording, or lost audio while recording.
    SweepRecorded {
        id: SweepId,
        result: Box<Result<Recording, String>>,
    },
    /// The analysis of sweep `id` finished.
    SweepAnalysed {
        id: SweepId,
        result: Box<Result<SweepAnalysis, SweepError>>,
    },
    /// The autosave write finished: the time it was written, or why it failed.
    Autosaved { result: Box<Result<WallNs, String>> },
    /// An SPL meter's job judged its Leq windows after a second (configuration
    /// `config_rev`): a judgement changed, the windows changed or the log began.
    Leq {
        meas: MeasId,
        /// The log's epoch the job judged in.
        epoch: u64,
        config_rev: Rev,
        at: WallNs,
        judgements: Vec<ac2_proto::model::LeqJudgement>,
        /// The peak limits' judgements (`PeakQuantity::ALL` order).
        peak_judgements: [ac2_proto::model::LeqJudgement; 2],
        alarms: Vec<ac2_proto::model::LeqAlarm>,
    },
    /// The recording under `token` has more audio in its file.
    RecordingProgress { token: u64 },
    /// The recording under `token` ended by itself and is finalised.
    RecordingEnded { token: u64 },
    /// The network sockets are gone (ZAP handler exited); shut down.
    Fatal(String),
    /// Orderly shutdown.
    Shutdown,
}

/// Construction parameters.
pub(crate) struct Setup {
    /// Offered backends, the default first.
    pub(crate) backends: Vec<Arc<dyn Backend>>,
    pub(crate) incarnation: DaemonIncarnation,
    /// The system max level in force, dBFS RMS, and its sample-peak limit: start values
    /// (the bound); [`Control::new`] takes the rig settings' value.
    pub(crate) ceiling_dbfs: f64,
    pub(crate) max_level: MaxLevel,
    /// The hard upper bound of the system max level (`--max-level`) and its peak limit,
    /// which every stream opens with.
    pub(crate) ceiling_bound: f64,
    pub(crate) bound_level: MaxLevel,
    /// Rig settings file; `None`: in memory only.
    pub(crate) rig_settings: Option<std::path::PathBuf>,
    /// How the daemon serves clients (`server.*`).
    pub(crate) server: rig::ServerSetup,
    pub(crate) lease_expiry: Duration,
    pub(crate) keepalive: Duration,
    pub(crate) replay: ReplayLimits,
    pub(crate) dedup: DedupLimits,
    pub(crate) ctx: Context,
    pub(crate) endpoint: String,
    pub(crate) interest: Arc<Interest>,
    pub(crate) fps: u32,
    pub(crate) outbox: Outbox,
    pub(crate) to_self: Sender<ControlMsg>,
    /// Session directory for `file.*` by name.
    pub(crate) session_dir: std::path::PathBuf,
    /// Network mode: `file.*` accept names only, never paths.
    pub(crate) network: bool,
    pub(crate) cal_store: Option<std::path::PathBuf>,
    /// Autosave directory and whether to restore it at start.
    pub(crate) autosave: Option<crate::config::AutosaveConfig>,
    /// Where `rec.start` writes and `session.replay` finds recordings by name; `None`:
    /// this daemon does not record.
    pub(crate) recording_dir: Option<std::path::PathBuf>,
    /// Local time of day for the band meters' day and night limits.
    pub(crate) local_clock: crate::config::LocalClock,
}

impl Setup {
    /// The output path's limits a stream opens with now.
    pub(crate) fn limits(&self) -> Limits {
        Limits {
            open: self.bound_level,
            now: self.max_level,
        }
    }
}

/// A request answered when a worker thread reports back.
struct PendingReply {
    routing_id: Vec<u8>,
    client: ClientId,
    id: RequestId,
}

/// A `delay.find` running on a job thread; answered when the result arrives.
struct PendingFind {
    routing_id: Vec<u8>,
    client: ClientId,
    id: RequestId,
    meas: MeasId,
}

/// Where an applied delay came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DelaySource {
    /// `delay.insert` of the last finding.
    Insert,
    /// `delay.set`: a value the operator typed.
    Typed,
    /// `delay.nudge`: the operator moved the applied delay by a step; the finding it
    /// refines still stands.
    Nudge,
    /// Tracking agreed on a new delay.
    Tracking,
}

struct Lease {
    token: LeaseToken,
    owner: ClientId,
    deadline: Instant,
}

/// What the current generator source was built from: a change rebuilds it, a level change
/// alone ramps the running one.
#[derive(Clone, Copy, PartialEq)]
struct SourceKey {
    signal: ac2_proto::model::Signal,
    band: Option<ac2_proto::model::BandLimit>,
}

/// The `ir.capture` run in progress.
struct ActiveSweep {
    run: SweepRun,
    /// The measurement's name and the run's number within it, for the trace.
    meas_name: String,
    number: u32,
    spec: SweepSpec,
    /// The recorder; gone once the recording is in.
    job: Option<JobHandle>,
    epoch: SessionEpoch,
    mic: Option<MicState>,
}

pub(crate) struct Control {
    s: Setup,
    store: Store,
    cal: CalStore,
    dedup: Dedup,
    session: Option<Runtime>,
    /// The open session's audio stopped and is being reopened; `session` is `None` meanwhile.
    recovery: Option<recovery::Recovery>,
    jobs: BTreeMap<MeasId, JobHandle>,
    /// How spatial averages reach their running members.
    probes: Arc<Probes>,
    timing_job: Option<JobHandle>,
    /// Capture-only meters of a device before a session opens on it.
    preview: Option<Preview>,
    /// The loopback detection running, by token.
    detecting: Option<(u64, PendingReply)>,
    next_fanout_id: u64,
    next_meas: u32,
    lease: Option<Lease>,
    gate: Arc<LeaseGate>,
    level: Option<LevelControl>,
    source: Option<SourceKey>,
    grids: HashMap<GridId, GridDef>,
    seqs: Arc<Seqs>,
    ka_seq: u64,
    pending_finds: HashMap<u64, PendingFind>,
    next_token: u64,
    next_ka: Instant,
    traces: traces::TraceStore,
    sweep: Option<ActiveSweep>,
    next_sweep: u32,
    autosave: Option<Autosaver>,
    /// Load the autosave when the control thread starts.
    restore: bool,
    /// Per-second log of each SPL measurement, shared with its job.
    spl_logs: HashMap<MeasId, crate::leq_log::SharedLog>,
    /// The log `spl.log_new` ended last, per SPL meter.
    spl_prev_logs: HashMap<MeasId, crate::leq_log::LeqLog>,
    /// The raw capture being written.
    recording: Option<recording::ActiveRecording>,
    /// Where the rig settings are kept.
    rig: crate::rig::RigStore,
    /// The system max level a client last set (kept in the rig settings); `None`: never
    /// set, the bound applies.
    ceiling_set: Option<f64>,
    /// The recording a replay session plays (`session.replay`).
    replay_backend: Option<Arc<ac2_audio::ReplayBackend>>,
}

impl Control {
    pub(crate) fn new(s: Setup) -> Self {
        let (cal, contents) = match &s.cal_store {
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
        let (rig, saved) = match &s.rig_settings {
            Some(p) => crate::rig::RigStore::open(p),
            None => (
                crate::rig::RigStore::memory(),
                crate::rig::RigSettings::default(),
            ),
        };
        let mut s = s;
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
        let store = Store::new(Dbfs(s.ceiling_dbfs), Dbfs(s.ceiling_bound), s.replay)
            .with_outputs(saved.outputs)
            .with_calibrations(contents)
            .with_autosave(Autosave {
                state: status,
                saved_at: None,
            });
        let dedup = Dedup::new(s.dedup);
        Self {
            store,
            cal,
            dedup,
            session: None,
            recovery: None,
            jobs: BTreeMap::new(),
            probes: Arc::new(Probes::default()),
            timing_job: None,
            preview: None,
            detecting: None,
            next_fanout_id: 1,
            next_meas: 1,
            lease: None,
            gate: Arc::new(LeaseGate::new()),
            level: None,
            source: None,
            grids: HashMap::new(),
            seqs: Arc::new(Seqs::default()),
            ka_seq: 0,
            pending_finds: HashMap::new(),
            next_token: 1,
            next_ka: Instant::now(),
            traces: traces::TraceStore::default(),
            sweep: None,
            next_sweep: 1,
            autosave,
            restore,
            spl_logs: HashMap::new(),
            spl_prev_logs: HashMap::new(),
            recording: None,
            replay_backend: None,
            rig,
            ceiling_set: saved.ceiling_dbfs,
            s,
        }
    }

    pub(crate) fn run(mut self, rx: &Receiver<ControlMsg>) {
        self.start_autosave();
        self.recover_recordings();
        loop {
            let now = Instant::now();
            if self
                .autosave
                .as_ref()
                .and_then(Autosaver::due)
                .is_some_and(|d| d <= now)
            {
                self.autosave_write();
            }
            self.check_lease(now);
            self.check_muted();
            if now >= self.next_ka {
                self.send_ka();
                self.next_ka = now + self.s.keepalive;
            }
            if self
                .recovery
                .as_ref()
                .and_then(recovery::Recovery::due)
                .is_some_and(|d| d <= now)
            {
                self.start_attempt(None);
            }
            if self
                .recovery
                .as_ref()
                .and_then(recovery::Recovery::probe_due)
                .is_some_and(|d| d <= now)
            {
                self.start_probe();
            }
            if self.preview.as_ref().is_some_and(|p| p.deadline <= now) {
                tracing::info!("preview not renewed: closing it");
                self.close_preview();
            }
            let mut wake = self.next_ka;
            if let Some(l) = &self.lease {
                wake = wake.min(l.deadline);
            }
            if let Some(p) = &self.preview {
                wake = wake.min(p.deadline);
            }
            if let Some(d) = self.autosave.as_ref().and_then(Autosaver::due) {
                wake = wake.min(d);
            }
            if let Some(d) = self.recovery.as_ref().and_then(recovery::Recovery::due) {
                wake = wake.min(d);
            }
            if let Some(d) = self
                .recovery
                .as_ref()
                .and_then(recovery::Recovery::probe_due)
            {
                wake = wake.min(d);
            }
            match rx.recv_timeout(wake.saturating_duration_since(Instant::now())) {
                Ok(ControlMsg::Request {
                    routing_id,
                    user_id,
                    payload,
                }) => self.on_request(&routing_id, user_id, &payload),
                Ok(ControlMsg::DeviceChanged { epoch }) => {
                    if self.session.as_ref().is_some_and(|r| r.epoch == epoch) {
                        tracing::warn!("device or configuration change: reopening the session");
                        let routes = self
                            .session
                            .as_ref()
                            .map(|r| r.routes.clone())
                            .unwrap_or_default();
                        if let Err(e) = self.reopen(&routes) {
                            tracing::error!("reopen after device change failed: {}", e.msg);
                        }
                    }
                }
                Ok(ControlMsg::AudioStopped {
                    epoch,
                    since,
                    cause,
                }) => self.audio_stopped(epoch, since, cause),
                Ok(ControlMsg::Reopened { token, result }) => self.reopened(token, *result),
                Ok(ControlMsg::Probed { token, presence }) => self.probed(token, presence),
                Ok(ControlMsg::Timing { epoch, status }) => {
                    if self.session.as_ref().is_some_and(|r| r.epoch == epoch)
                        && self.store.state().timing != status
                    {
                        self.commit(Change::Timing(status));
                    }
                }
                Ok(ControlMsg::DelayFound { token, result }) => self.finish_find(token, *result),
                Ok(ControlMsg::LoopbackDetected { token, result }) => {
                    if let Some((t, p)) = self.detecting.take() {
                        if t == token {
                            let r = (*result).map(ReplyBody::LoopbackDetection);
                            self.answer(&p.routing_id, &p.client, p.id, r, Instant::now());
                        } else {
                            self.detecting = Some((t, p));
                        }
                    }
                }
                Ok(ControlMsg::DelayTracked {
                    meas,
                    epoch,
                    samples,
                }) => self.tracked(meas, epoch, samples),
                Ok(ControlMsg::SweepProgress { id, repeat }) => self.sweep_progress(id, repeat),
                Ok(ControlMsg::SweepRecorded { id, result }) => self.sweep_recorded(id, *result),
                Ok(ControlMsg::SweepAnalysed { id, result }) => self.sweep_analysed(id, *result),
                Ok(ControlMsg::Autosaved { result }) => self.autosaved(*result),
                Ok(ControlMsg::Leq {
                    meas,
                    epoch,
                    config_rev,
                    at,
                    judgements,
                    peak_judgements,
                    alarms,
                }) => self.leq_reported(
                    meas,
                    epoch,
                    config_rev,
                    at,
                    &judgements,
                    peak_judgements,
                    alarms,
                ),
                Ok(ControlMsg::RecordingProgress { token }) => self.recording_progress(token),
                Ok(ControlMsg::RecordingEnded { token }) => self.recording_ended(token),
                Ok(ControlMsg::Fatal(why)) => {
                    tracing::error!("fatal: {why}");
                    break;
                }
                Ok(ControlMsg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
        self.shutdown();
    }

    fn shutdown(&mut self) {
        tracing::info!("shutting down");
        self.abort_sweep(SweepFailure::Stopped, "the daemon is shutting down");
        self.end_recording(ac2_proto::model::RecordingEnd::DaemonShutdown);
        self.close_preview();
        self.stop_output();
        self.stop_all_jobs();
        self.recovery = None;
        if let Some(rt) = self.session.take() {
            recovery::close_bounded(rt, recovery::CLOSE_BOUND);
        }
        self.flush_autosave();
        self.s.outbox.stop();
    }

    // -- plumbing --------------------------------------------------------------------------

    fn commit(&mut self, change: Change) -> Rev {
        let saved = matches!(change, Change::Measurement(_) | Change::Trace(_));
        self.note_for_recording(&change);
        let ev = self.store.commit(change, Instant::now());
        match ac2_proto::encode_event(&ev) {
            Ok(b) => self.s.outbox.event(&b),
            Err(e) => tracing::error!("event not encodable: {e}"),
        }
        if saved {
            self.autosave_changed();
        }
        ev.rev
    }

    fn epoch(&self) -> SessionEpoch {
        self.store.state().session.epoch
    }

    fn execute(&mut self, client: &ClientId, cmd: Command) -> Result<ReplyBody, ProtoError> {
        let ack = |rev: Rev| Ok(ReplyBody::Ack { rev });
        match cmd {
            Command::Hello { client: software } => {
                tracing::info!("hello from {} ({software})", client.0);
                Ok(ReplyBody::Welcome(Welcome {
                    server: format!(
                        "ac2d {} (build {})",
                        env!("CARGO_PKG_VERSION"),
                        env!("AC2_BUILD_ID")
                    ),
                    client_id: client.clone(),
                    daemon_incarnation: self.s.incarnation,
                    session_epoch: self.epoch(),
                    rev: self.store.rev(),
                }))
            }
            Command::SessionDevices => Ok(ReplyBody::Backends(self.backend_infos())),
            Command::SessionPreview { backend, device } => self.session_preview(backend, device),
            Command::SessionPreviewStop => {
                self.close_preview();
                ack(self.store.rev())
            }
            Command::SessionDetectLoopback { .. } => Err(perr(
                ErrorCode::Internal,
                "session.detect_loopback is answered asynchronously",
            )),
            Command::SessionOpen { config } => self.session_open(client, config),
            Command::SessionClose => {
                self.session_close(client);
                ack(self.store.rev())
            }
            Command::SessionStatus => Ok(ReplyBody::Session(self.store.state().session.clone())),

            Command::GenAcquire { force } => self.gen_acquire(client, force),
            Command::GenSet {
                lease_token,
                desired,
            } => self.gen_set(client, lease_token, desired),
            Command::GenRefresh { lease_token } => {
                self.lease_check(client, lease_token)?;
                let deadline = Instant::now() + self.s.lease_expiry;
                if let Some(l) = &mut self.lease {
                    l.deadline = deadline;
                }
                if self.store.state().generator.firing {
                    self.gate.open_until(deadline);
                }
                Ok(ReplyBody::Lease(self.wire_lease(lease_token)))
            }
            Command::GenRelease { lease_token } => {
                self.lease_check(client, lease_token)?;
                self.abort_sweep(SweepFailure::Stopped, "the stimulus lease was released");
                self.stop_output();
                self.lease = None;
                let mut g = self.store.state().generator.clone();
                g.owner = None;
                g.armed = false;
                g.firing = false;
                self.audit(&mut g, GenAction::Release, Some(client));
                ack(self.commit(Change::Generator(g)))
            }
            Command::GenCeiling {
                ceiling,
                confirm_raise,
            } => self.gen_ceiling(client, ceiling, confirm_raise),
            Command::SessionOutputs { outputs } => self.session_outputs(outputs),
            Command::ServerInfo => Ok(ReplyBody::Server(self.server_info())),
            Command::ServerAuthorize { name, key } => self.server_authorize(client, &name, &key),
            Command::ServerRevoke { name } => self.server_revoke(client, &name),
            Command::GenStop => {
                self.abort_sweep(SweepFailure::Stopped, "the stimulus was stopped");
                self.stop_output();
                let mut g = self.store.state().generator.clone();
                g.armed = false;
                g.firing = false;
                self.audit(&mut g, GenAction::Stop, Some(client));
                ack(self.commit(Change::Generator(g)))
            }

            Command::MeasCreate { config } => {
                validate_meas(&config)?;
                self.check_meas_owner(None, &config.kind)?;
                let grid = self.grid_of(None, &config.kind)?;
                let id = MeasId(self.next_meas);
                self.next_meas += 1;
                let grid_id = grid.map(|g| self.register_grid(g));
                let delay = matches!(config.kind, MeasKind::Transfer { .. }).then(|| DelayState {
                    applied: Seconds(0.0),
                    applied_samples: 0.0,
                    nudged: Seconds(0.0),
                    nudged_samples: 0.0,
                    tracking: false,
                    awaiting_pick: false,
                    last_finding: None,
                });
                let m = Measurement {
                    id,
                    config,
                    config_rev: Rev(self.store.rev().0 + 1),
                    running: false,
                    frozen: false,
                    delay,
                    grid_id,
                };
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                self.ensure_spl_log(&m, None);
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasUpdate { meas, config } => {
                validate_meas(&config)?;
                let mut m = self.meas(meas)?.clone();
                self.check_meas_owner(Some(meas), &config.kind)?;
                self.check_operand_update(meas, &config.kind)?;
                if owners::owner_only(&m.config, &config) {
                    // Moving a math channel changes where it is listed, not what it
                    // computes: its job and its frames' config rev go on.
                    m.config = config;
                    self.commit(Change::Measurement(Patch::Set(m.clone())));
                    return Ok(ReplyBody::Measurement(m));
                }
                let grid = self.grid_of(Some(meas), &config.kind)?;
                let old_leq = match &m.config.kind {
                    MeasKind::Spl { config } => Some(config.leq.clone()),
                    _ => None,
                };
                if let Some(spl) = spl_in_place(&m.config.kind, &config.kind) {
                    m.config = config;
                    m.config_rev = Rev(self.store.rev().0 + 1);
                    if let Some(j) = self.jobs.get(&meas) {
                        j.send(JobCmd::Spl {
                            config: Box::new(spl),
                            rev: m.config_rev,
                        });
                    }
                    self.commit(Change::Measurement(Patch::Set(m.clone())));
                    self.ensure_spl_log(&m, old_leq.as_ref());
                    return Ok(ReplyBody::Measurement(m));
                }
                if let Some(change) = smoothing_only(&m.config.kind, &config.kind) {
                    // Display smoothing changes in place: averaging goes on, and the next
                    // frame carries the new setting under the new rev.
                    m.config = config;
                    m.config_rev = Rev(self.store.rev().0 + 1);
                    if let Some(j) = self.jobs.get(&meas) {
                        j.send(JobCmd::Smoothing {
                            change,
                            rev: m.config_rev,
                        });
                    }
                    self.commit(Change::Measurement(Patch::Set(m.clone())));
                    return Ok(ReplyBody::Measurement(m));
                }
                let was_transfer = matches!(m.config.kind, MeasKind::Transfer { .. });
                let is_transfer = matches!(config.kind, MeasKind::Transfer { .. });
                m.config = config;
                m.config_rev = Rev(self.store.rev().0 + 1);
                if !is_transfer {
                    m.delay = None;
                } else if !was_transfer {
                    m.delay = Some(DelayState {
                        applied: Seconds(0.0),
                        applied_samples: 0.0,
                        nudged: Seconds(0.0),
                        nudged_samples: 0.0,
                        tracking: false,
                        awaiting_pick: false,
                        last_finding: None,
                    });
                }
                m.grid_id = grid.map(|g| self.register_grid(g));
                // The log and the entity follow the new configuration before the job
                // starts, so a restarted meter reports against its new windows; the
                // measurement's own commit comes after, under the rev it names.
                self.ensure_spl_log(&m, old_leq.as_ref());
                m.config_rev = Rev(self.store.rev().0 + 1);
                if m.running && self.session.is_some() {
                    self.stop_job(meas);
                    if let Some(g) = self.start_job(&m)? {
                        m.grid_id = Some(self.register_grid(g));
                    }
                }
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasDelete { meas, traces } => self.meas_delete(meas, traces),
            Command::MeasStart { meas } => {
                let mut m = self.meas(meas)?.clone();
                not_a_sweep(&m)?;
                if !m.running
                    && self.session.is_some()
                    && let Some(g) = self.start_job(&m)?
                {
                    m.grid_id = Some(self.register_grid(g));
                }
                m.running = true;
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasStop { meas } => {
                let mut m = self.meas(meas)?.clone();
                not_a_sweep(&m)?;
                self.stop_job(meas);
                m.running = false;
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasFreeze { meas, frozen } => {
                let mut m = self.meas(meas)?.clone();
                not_a_sweep(&m)?;
                m.frozen = frozen;
                if let Some(j) = self.jobs.get(&meas) {
                    j.send(JobCmd::Freeze(frozen));
                }
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasReset { meas } => {
                not_a_sweep(self.meas(meas)?)?;
                if let MeasKind::Math { .. } = self.meas(meas)?.config.kind {
                    // Its operands hold the averages; resetting one is the operator's choice.
                    return Err(perr(
                        ErrorCode::Invalid,
                        "a math channel holds no averages of its own; reset its operands",
                    ));
                }
                if let Some(j) = self.jobs.get(&meas) {
                    j.send(JobCmd::Reset);
                }
                ack(self.store.rev())
            }

            Command::DelayFind { .. } => Err(perr(
                ErrorCode::Internal,
                "delay.find is answered asynchronously",
            )),
            Command::DelayInsert { meas, pick } => {
                let d = self.transfer_delay(meas)?;
                let Some(f) = &d.last_finding else {
                    return Err(perr(
                        ErrorCode::Invalid,
                        "no delay finding to insert; run delay.find or use delay.set",
                    ));
                };
                if let Some(reasons) = f.no_estimate() {
                    return Err(perr(
                        ErrorCode::Refused,
                        format!(
                            "the last finding has no estimate ({reasons:?}); find again or use delay.set"
                        ),
                    ));
                }
                let delay = f
                    .arrival(pick)
                    .ok_or_else(|| {
                        perr(
                            ErrorCode::NotFound,
                            "the last finding has no such arrival (ranked picks need an ambiguous finding)",
                        )
                    })?
                    .delay;
                self.set_delay(meas, delay, DelaySource::Insert)
            }
            Command::DelaySet { meas, delay } => self.set_delay(meas, delay, DelaySource::Typed),
            Command::DelayNudge { meas, by } => {
                let d = self.transfer_delay(meas)?;
                if !by.0.is_finite() {
                    return Err(perr(ErrorCode::Invalid, "the step must be finite"));
                }
                let delay = Seconds(d.applied.0 + by.0);
                self.set_delay(meas, delay, DelaySource::Nudge)
            }
            Command::DelayTrack { meas, enabled } => {
                self.transfer_delay(meas)?;
                let mut m = self.meas(meas)?.clone();
                if let Some(d) = &mut m.delay {
                    d.tracking = enabled;
                }
                if let Some(j) = self.jobs.get(&meas) {
                    j.send(JobCmd::Track { enabled });
                }
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }

            Command::TraceList => Ok(ReplyBody::Traces(self.store.state().traces.clone())),
            Command::TraceCapture { meas, name, slot } => self.trace_capture(meas, name, slot),
            Command::TraceGet { trace } => self.trace_get(trace),
            Command::TraceUpdate { trace, edit } => self.trace_update(trace, edit),
            Command::TraceDelete { trace } => self.trace_delete(trace),
            Command::TraceAverage {
                traces,
                method,
                reference,
                name,
            } => self.trace_average(&traces, method, reference, name),
            Command::TraceImport {
                file_name,
                format,
                role,
                content,
            } => self.trace_import(file_name, format, role, &content.0),
            Command::TraceExport { trace, format } => self.trace_export(trace, format),
            Command::TraceMicCurve { trace, curve } => self.trace_mic_curve(trace, curve),

            Command::CalSpl {
                input,
                mic,
                calibrator_level,
                calibrator_freq,
            } => self.cal_spl(input, mic, calibrator_level, calibrator_freq),
            Command::CalSplElectrical {
                input,
                mic,
                connection,
                volts,
                freq,
                mic_sensitivity,
                uncertainty,
                replace_acoustic,
            } => self.cal_spl_electrical(cal::ElectricalArgs {
                input,
                mic,
                connection,
                volts,
                freq,
                mic_sensitivity,
                uncertainty,
                replace_acoustic,
            }),
            Command::CalCurveImport {
                mic,
                label,
                file_name,
                content,
                input,
            } => self.cal_curve_import(mic, label, file_name, content, input),
            Command::CalCurveRename { curve, label } => self.cal_curve_rename(curve, label),
            Command::CalCurveDelete { curve } => self.cal_curve_delete(curve),
            Command::CalList => self.cal_list(),
            Command::CalDelete { key } => self.cal_delete(&key),
            Command::SessionInputs { inputs } => self.session_inputs(inputs),

            Command::SplLogGet {
                meas,
                log,
                from,
                max,
            } => self.spl_log_get(meas, log, from, max),
            Command::SplLogNew { meas } => self.spl_log_new(meas),
            Command::SplBandTransfer {
                meas,
                foh,
                at_place,
                background,
                place,
            } => self.spl_band_transfer(client, meas, &foh, &at_place, background.as_ref(), &place),
            Command::SplHistoryGet { meas, seconds } => self.spl_history_get(meas, seconds),
            Command::SplBandLogGet {
                meas,
                from,
                until,
                step,
            } => self.spl_band_log_get(meas, from, until, step),

            Command::SweepRun {
                lease_token,
                meas,
                name,
            } => self.sweep_run(client, lease_token, meas, name),

            Command::StateSnapshot => Ok(ReplyBody::Snapshot(Box::new(
                self.store.snapshot(self.s.incarnation),
            ))),
            Command::StateSince { rev } => match self.store.since(rev, Instant::now()) {
                Ok(evs) => Ok(ReplyBody::Events(evs)),
                Err(oldest) => Err(perr_detail(
                    ErrorCode::ResyncRequired,
                    format!(
                        "events after rev {} are no longer replayable; take a snapshot",
                        rev.0
                    ),
                    ErrorDetail::Resync { oldest },
                )),
            },
            Command::GridGet { grid_id } => self
                .grids
                .get(&grid_id)
                .cloned()
                .map(ReplyBody::Grid)
                .ok_or_else(|| perr(ErrorCode::NotFound, format!("no grid {grid_id}"))),
            Command::FileSave { session } => self.file_save(&session),
            Command::FileLoad { session } => self.file_load(client, &session),
            Command::FileList => self.file_list(),
            Command::RecStart { request } => self.rec_start(client, request),
            Command::RecStop => self.rec_stop(),
            Command::RecList => self.rec_list(),
            Command::SessionReplay { recording, pace } => {
                self.session_replay(client, &recording, pace)
            }
        }
    }

    fn meas(&self, id: MeasId) -> Result<&Measurement, ProtoError> {
        self.store
            .state()
            .measurements
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| not_found(id))
    }

    fn transfer_delay(&self, id: MeasId) -> Result<&DelayState, ProtoError> {
        self.meas(id)?.delay.as_ref().ok_or_else(|| {
            perr(
                ErrorCode::Invalid,
                format!("measurement {} is not a transfer function", id.0),
            )
        })
    }

    fn register_grid(&mut self, g: GridDef) -> GridId {
        let id = g.id();
        self.grids.entry(id).or_insert(g);
        id
    }
}
