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
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};
use ac2_zmq::{Context, PublicKey};

use crate::autosave::Autosaver;
use crate::calstore::{self, CalStore};
use crate::config::{DedupLimits, ReplayLimits};
use crate::dedup::Dedup;
use crate::io::Interest;
use crate::jobs::{self, Captured, JobHandle, Probes, Seqs, block_index};
use crate::outbox::Outbox;
use crate::preview::Preview;
use crate::session::{Limits, Runtime};
use crate::state::Store;
use crate::stimulus::{LeaseGate, LeasedSource, SweepTrain};
use crate::sweep::Recording;
use crate::util::perr;

mod artnet;
mod autosave;
mod cal;
mod eq;
mod execute;
mod file_jobs;
mod files;
mod generator;
mod leq;
mod levels;
mod maths;
mod meas_jobs;
mod osc;
mod owners;
mod recording;
mod recovery;
mod request;
pub(crate) mod rig;
mod rig_meas;
mod session_ctl;
mod sweeps;
mod traces;
mod transfer;
mod validate;
mod working;

pub(crate) use artnet::check_addr as check_artnet_addr;
pub(crate) use osc::{Judged, has_cue};
pub(crate) use validate::validate_meas;
use validate::{
    MAX_CAL_UNSETTLED_DB, MAX_DELAY_S, check_delay, check_find, delay_samples, gen_err,
    lease_required, mutation_conflict, not_found, static_grid, upsert_inputs,
};

/// The authenticated peer of a network-mode (CURVE) connection, as the ZAP handler granted
/// it at the handshake.
pub(crate) struct CurvePeer {
    /// The authorized name the key had at the handshake.
    pub(crate) name: String,
    /// The key the connection authenticated with.
    pub(crate) key: PublicKey,
}

/// Everything that reaches the control thread.
pub(crate) enum ControlMsg {
    /// A ctrl request from the I/O thread.
    Request {
        routing_id: Vec<u8>,
        /// `None` on connections without ZAP (embedded, local).
        peer: Option<CurvePeer>,
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
    /// The `session.control_panel` call started under `token` answered (or is showing a
    /// modal panel).
    PanelShown {
        token: u64,
        result: Result<(), ProtoError>,
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
    /// An SPL meter's log file failed to be written (`Some`, the first failure of a run), or
    /// is written again or gone (`None`).
    LogWrite {
        meas: MeasId,
        log: ac2_proto::model::MeterLog,
        failure: Option<ac2_proto::model::LogWriteFailure>,
    },
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
        /// Every subject's judgement and level, for the OSC cues.
        judged: Vec<osc::Judged>,
    },
    /// The recording under `token` has more audio in its file.
    RecordingProgress { token: u64 },
    /// The recording under `token` ended by itself and is finalised.
    RecordingEnded { token: u64 },
    /// The network sockets are gone (ZAP handler exited); shut down.
    Fatal(String),
    /// The worker of file operation `token` finished its disk work.
    FileDone {
        token: u64,
        done: Box<file_jobs::FileDone>,
    },
    /// The autosave writer put the recovery copy kept under `token` in place on disk, or
    /// why it could not.
    RecoverInstalled {
        token: u64,
        result: Result<(), String>,
    },
    /// Orderly shutdown.
    Shutdown,
}

/// Construction parameters.
pub(crate) struct Setup {
    /// Offered backends, the default first.
    pub(crate) backends: Vec<Arc<dyn Backend>>,
    pub(crate) incarnation: DaemonIncarnation,
    /// This daemon's identity (`DaemonConfig::rig_id`) and name, told every client at hello.
    pub(crate) rig_id: ac2_proto::units::RigId,
    pub(crate) name: String,
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
    /// Where the rig's SPL meters keep their logs (`DaemonConfig::rig_logs`).
    pub(crate) rig_logs: Option<std::path::PathBuf>,
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
    pub(crate) systemd_notify: bool,
    /// State files found damaged before the control thread started (the server keys).
    pub(crate) damaged: Vec<crate::damage::Damage>,
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
    /// The measurement input's mic, without a curve: the sweep analysis works on the raw
    /// recordings, so no curve is in its columns.
    mic: Option<MicState>,
    /// The input's active mic curve, put on the trace after capture (as the live jobs on
    /// the input apply it), so the sweep reads what a live transfer of the input reads.
    mic_curve: Option<(
        Box<ac2_proto::model::TraceMicCurve>,
        ac2_core::mic_curve::Correction,
    )>,
}

pub(crate) struct Control {
    s: Setup,
    store: Store,
    cal: CalStore,
    dedup: Dedup,
    session: Option<Runtime>,
    /// The health meter keepalives carry.
    health: crate::health::Health,
    /// The open session's audio stopped and is being reopened; `session` is `None` meanwhile.
    audio_recovery: Option<recovery::Recovery>,
    /// What fed the spare inputs when the daemon last closed a running stream itself,
    /// connected again to each stream it opens: the operator's routing into spare ports
    /// would otherwise end with every reopen.
    spare_links: Vec<ac2_audio::SpareLink>,
    jobs: BTreeMap<MeasId, JobHandle>,
    /// How spatial averages reach their running members.
    probes: Arc<Probes>,
    timing_job: Option<JobHandle>,
    /// Capture-only meters of a device before a session opens on it.
    preview: Option<Preview>,
    /// The loopback detection running, by token.
    detecting: Option<(u64, PendingReply)>,
    /// Driver control panel calls under way, by token.
    panels: HashMap<u64, PendingReply>,
    /// Warnings the command executing now raised; the next reply carries them.
    reply_warnings: Vec<ac2_proto::Warning>,
    next_fanout_id: u64,
    next_meas: u32,
    out: Output,
    grids: HashMap<GridId, GridDef>,
    seqs: Arc<Seqs>,
    ka_seq: u64,
    pending_finds: HashMap<u64, PendingFind>,
    next_token: u64,
    next_ka: Instant,
    /// systemd's readiness and watchdog, pinged from this loop's keepalive tick so that a
    /// stuck loop stops the pings and gets the daemon restarted.
    notify: ac2_paths::notify::Notifier,
    /// State files found damaged, not yet in the rig entity.
    damaged: Vec<crate::damage::Damage>,
    traces: traces::TraceStore,
    sweep: Option<ActiveSweep>,
    next_sweep: u32,
    autosave: Option<Autosaver>,
    /// Load the autosave when the control thread starts.
    restore: bool,
    logs: SplLogs,
    /// The raw capture being written.
    recording: Option<recording::ActiveRecording>,
    rig: RigState,
    /// What the programme meters of the session and the preview read: `rig.levels`, the
    /// inputs and line calibrations (VU offers), the pairs, the resets.
    levels: crate::jobs::programme::SharedPolicy,
    /// The recording a replay session plays (`session.replay`).
    replay_backend: Option<Arc<ac2_audio::ReplayBackend>>,
    /// What the working state belongs to (`State.working`).
    working: working::Persist,
    /// The working state the last `file.new` / `file.load` replaced with unsaved changes.
    replaced_working: Option<working::Recover>,
    /// Session archives being uploaded or downloaded.
    transfers: transfer::Transfers,
    /// File operations whose disk work runs on a worker, by token.
    file_jobs: file_jobs::FileJobs,
}

/// The stimulus output: who may drive it, its gate, level and source, and the ceiling.
struct Output {
    lease: Option<Lease>,
    gate: Arc<LeaseGate>,
    level: Option<LevelControl>,
    source: Option<SourceKey>,
    /// The system max level a client last set (kept in the rig settings); `None`: never
    /// set, the bound applies.
    ceiling_set: Option<f64>,
}

/// The rig's persistent settings, its meters and the cue senders they drive.
struct RigState {
    /// Where the rig settings are kept.
    settings: crate::rig::RigStore,
    /// The rig's SPL meters and the monitor at start.
    meas: rig_meas::RigMeas,
    /// The OSC cues' states and their sender.
    osc: osc::Cues,
    /// The Art-Net universe the cue states make, and its sender.
    artnet: artnet::Universe,
}

/// The per-second SPL logs, by meter.
struct SplLogs {
    /// The log each SPL measurement is writing now, shared with its job.
    live: HashMap<MeasId, crate::leq_log::SharedLog>,
    /// The log `spl.log_new` ended last, per SPL meter.
    ended: HashMap<MeasId, crate::leq_log::LeqLog>,
}

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
            .with_calibrations(contents)
            .with_autosave(Autosave {
                state: status,
                saved_at: None,
                log_failures: Vec::new(),
            });
        let dedup = Dedup::new(s.dedup);
        let rigm = rig_meas::RigMeas::new(&saved, s.rig_logs.as_deref());
        Self {
            store,
            cal,
            dedup,
            session: None,
            health: crate::health::Health::new(),
            audio_recovery: None,
            spare_links: Vec::new(),
            jobs: BTreeMap::new(),
            probes: Arc::new(Probes::default()),
            timing_job: None,
            preview: None,
            detecting: None,
            panels: HashMap::new(),
            reply_warnings: Vec::new(),
            next_fanout_id: 1,
            next_meas: rigm.next_id(),
            out: Output {
                lease: None,
                gate: Arc::new(LeaseGate::new()),
                level: None,
                source: None,
                ceiling_set: saved.ceiling_dbfs,
            },
            grids: HashMap::new(),
            seqs: Arc::new(Seqs::default()),
            ka_seq: 0,
            pending_finds: HashMap::new(),
            next_token: 1,
            next_ka: Instant::now(),
            damaged,
            notify: if s.systemd_notify {
                ac2_paths::notify::Notifier::from_env()
            } else {
                ac2_paths::notify::Notifier::disabled()
            },
            traces: traces::TraceStore::default(),
            sweep: None,
            next_sweep: 1,
            autosave,
            restore,
            logs: SplLogs {
                live: HashMap::new(),
                ended: HashMap::new(),
            },
            recording: None,
            replay_backend: None,
            working: working::Persist::untitled(),
            replaced_working: None,
            transfers: transfer::Transfers::default(),
            file_jobs: file_jobs::FileJobs::default(),
            rig: RigState {
                settings: rig,
                meas: rigm,
                osc: osc::Cues::default(),
                artnet: artnet::Universe::default(),
            },
            levels: Default::default(),
            s,
        }
    }

    pub(crate) fn run(mut self, rx: &Receiver<ControlMsg>) {
        self.sync_levels_policy();
        self.load_rig_meas();
        self.start_autosave();
        self.start_rig_monitor();
        self.recover_recordings();
        self.publish_damage();
        // The sockets were bound before this thread started; the restore and the rig
        // monitor's start are done or under way.
        if let Some(e) = self.notify.ready() {
            tracing::warn!("systemd notification failed: {e}");
        }
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
                if let Some(e) = self.notify.ping(now) {
                    tracing::warn!("systemd watchdog ping failed: {e}");
                }
            }
            if self
                .audio_recovery
                .as_ref()
                .and_then(recovery::Recovery::due)
                .is_some_and(|d| d <= now)
            {
                self.start_attempt(None);
            }
            if self
                .audio_recovery
                .as_ref()
                .and_then(recovery::Recovery::probe_due)
                .is_some_and(|d| d <= now)
            {
                self.start_probe();
            }
            self.rig_tick(now);
            if self.preview.as_ref().is_some_and(|p| p.deadline <= now) {
                tracing::info!("preview not renewed: closing it");
                self.close_preview();
            }
            self.osc_sync();
            self.osc_tick(now);
            self.artnet_tick(now);
            let mut wake = self.next_ka;
            if let Some(t) = self.osc_wake() {
                wake = wake.min(t);
            }
            if let Some(t) = self.artnet_wake() {
                wake = wake.min(t);
            }
            if let Some(l) = &self.out.lease {
                wake = wake.min(l.deadline);
            }
            if let Some(p) = &self.preview {
                wake = wake.min(p.deadline);
            }
            if let Some(d) = self.autosave.as_ref().and_then(Autosaver::due) {
                wake = wake.min(d);
            }
            if let Some(d) = self
                .audio_recovery
                .as_ref()
                .and_then(recovery::Recovery::due)
            {
                wake = wake.min(d);
            }
            if let Some(d) = self.rig_due() {
                wake = wake.min(d);
            }
            if let Some(d) = self
                .audio_recovery
                .as_ref()
                .and_then(recovery::Recovery::probe_due)
            {
                wake = wake.min(d);
            }
            match rx.recv_timeout(wake.saturating_duration_since(Instant::now())) {
                Ok(ControlMsg::Request {
                    routing_id,
                    peer,
                    payload,
                }) => self.on_request(&routing_id, peer, &payload),
                Ok(ControlMsg::DeviceChanged { epoch }) => {
                    if self.session.as_ref().is_some_and(|r| r.epoch == epoch) {
                        tracing::warn!("device or configuration change: reopening the audio I/O");
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
                Ok(ControlMsg::PanelShown { token, result }) => {
                    if let Some(p) = self.panels.remove(&token) {
                        let r = result.map(|()| ReplyBody::Ack {
                            rev: self.store.rev(),
                        });
                        self.answer(&p.routing_id, &p.client, p.id, r, Instant::now());
                    }
                }
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
                Ok(ControlMsg::LogWrite { meas, log, failure }) => {
                    self.log_write(meas, log, failure);
                }
                Ok(ControlMsg::Leq {
                    meas,
                    epoch,
                    config_rev,
                    at,
                    judgements,
                    peak_judgements,
                    alarms,
                    judged,
                }) => self.leq_reported(
                    meas,
                    epoch,
                    config_rev,
                    at,
                    &judgements,
                    peak_judgements,
                    alarms,
                    judged,
                ),
                Ok(ControlMsg::RecordingProgress { token }) => self.recording_progress(token),
                Ok(ControlMsg::RecordingEnded { token }) => self.recording_ended(token),
                Ok(ControlMsg::FileDone { token, done }) => self.file_done(token, *done),
                Ok(ControlMsg::RecoverInstalled { token, result }) => {
                    self.recover_installed(token, result);
                }
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

    /// A state file found damaged (logged already); clients see it with the next
    /// [`Self::publish_damage`].
    pub(super) fn note_damage(&mut self, d: crate::damage::Damage) {
        self.damaged.push(d);
    }

    /// Puts the damaged files noted so far in the rig entity.
    fn publish_damage(&mut self) {
        if self.damaged.is_empty() {
            return;
        }
        let at = WallNs(crate::util::wall_ns());
        let found: Vec<_> = self.damaged.drain(..).map(|d| d.wire(at)).collect();
        self.commit_rig(|r| r.damaged_files.extend(found));
    }

    fn shutdown(&mut self) {
        tracing::info!("shutting down");
        if let Some(e) = self.notify.stopping() {
            tracing::warn!("systemd notification failed: {e}");
        }
        self.osc_shutdown();
        self.abort_sweep(SweepFailure::Stopped, "the daemon is shutting down");
        self.end_recording(ac2_proto::model::RecordingEnd::DaemonShutdown);
        self.close_preview();
        self.stop_output();
        self.stop_all_jobs();
        self.audio_recovery = None;
        if let Some(rt) = self.session.take() {
            recovery::close_bounded(rt, recovery::CLOSE_BOUND);
        }
        self.flush_file_jobs();
        self.flush_autosave();
        self.finish_rig_logs();
        self.s.outbox.stop();
    }

    // -- plumbing --------------------------------------------------------------------------

    fn commit(&mut self, change: Change) -> Rev {
        let saved = matches!(change, Change::Measurement(_) | Change::Trace(_));
        let levels = matches!(
            change,
            Change::Rig(_) | Change::Inputs(_) | Change::LineCalibration(_)
        );
        self.note_for_recording(&change);
        self.rig_meas_committing(&change);
        let ev = self.store.commit(change, Instant::now());
        if levels {
            self.sync_levels_policy();
        }
        match ac2_proto::encode_event(&ev) {
            Ok(b) => self.s.outbox.event(&b),
            Err(e) => tracing::error!("event not encodable: {e}"),
        }
        if saved {
            self.working_changed();
            self.autosave_changed();
        }
        ev.rev
    }

    fn epoch(&self) -> SessionEpoch {
        self.store.state().session.epoch
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
