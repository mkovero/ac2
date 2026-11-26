//! The control thread: decodes requests, executes commands one at a time against the state
//! store (serial commits), runs the stream and jobs, owns the stimulus lease and emits
//! keepalives.
//!
//! It is a plain thread fed by one channel. Commands are short (job threads do the DSP),
//! and a single consumer gives serial commits and a total event order for free; an async
//! runtime would add scheduling without adding concurrency that the state model allows.

use std::collections::{BTreeMap, HashMap, HashSet};
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
    CalState, GenAction, GeneratorSettings, LoopbackDetection, MeasKind, Measurement, MicState,
    SweepConfig, SweepFailure, SweepRun, SweepStatus, TimingStatus, TraceKind, TraceMeta,
    TraceSource,
};
use ac2_proto::units::{
    DaemonIncarnation, Dbfs, LeaseToken, MeasId, RequestId, Rev, Seconds, StreamEpoch, SweepId,
    WallNs,
};
use ac2_proto::{ProtoError, ReplyBody};
use ac2_zmq::Context;

use crate::autosave::Autosaver;
use crate::calstore::CalStore;
use crate::config::{DedupLimits, ReplayLimits};
use crate::dedup::Dedup;
use crate::io::Interest;
use crate::jobs::{self, Captured, JobHandle, Probes, Seqs, block_index};
use crate::outbox::Outbox;
use crate::preview::Preview;
use crate::route::{Peer, Route};
use crate::state::Store;
use crate::stimulus::{LeaseGate, LeasedSource, SweepTrain};
use crate::stream::{Limits, Runtime};
use crate::sweep::Recording;

mod artnet;
mod autosave;
mod cal;
mod clocks;
mod cross_rig;
mod delays;
mod dynamics;
mod eq;
mod execute;
mod file_jobs;
mod file_ops;
mod files;
mod generator;
mod ids;
mod kept;
mod leq;
mod levels;
mod load_check;
mod maths;
mod meas_jobs;
pub(crate) mod network;
mod osc;
mod owners;
mod recording;
mod recovery;
mod redact;
mod remote;
mod request;
pub(crate) mod rig;
mod rig_meas;
mod roles;
mod sacn;
mod salvage_logs;
mod sim;
mod standing;
mod startup;
mod stream_ctl;
mod sweeps;
mod traces;
mod transfer;
mod validate;
mod walk_mics;
mod walk_steps;
mod walk_sweep;
mod walks;
mod working;

pub(crate) use crate::dmx::sacn::check_target as check_sacn_target;
pub(crate) use artnet::check_addr as check_artnet_addr;
pub(crate) use osc::{Judged, has_cue};
pub(crate) use remote::check_peers;
pub use transfer::{FreeSpace, TransferLimits};
pub(crate) use validate::validate_meas;
use validate::{
    MAX_CAL_UNSETTLED_DB, MAX_DELAY_S, check_delay, check_find, delay_samples, gen_err,
    lease_required, not_found, static_grid, upsert_inputs,
};
pub(crate) use walk_sweep::walk_room;

/// Everything that reaches the control thread.
pub(crate) enum ControlMsg {
    /// A ctrl request from the I/O thread, with the pair it came in on.
    Request {
        route: Route,
        peer: Peer,
        payload: Vec<u8>,
    },
    /// The I/O thread closed network pair `epoch` (its ZAP handler died).
    NetFailed {
        epoch: crate::route::NetEpoch,
        reason: crate::network::NetReason,
    },
    /// The stream of `epoch` reported a configuration change.
    DeviceChanged { epoch: StreamEpoch },
    /// The stream of `epoch` stopped: no audio since `since`, for `cause`.
    AudioStopped {
        epoch: StreamEpoch,
        since: WallNs,
        cause: ac2_proto::model::StopCause,
    },
    /// Reopen attempt `token` of a stopped stream finished.
    Reopened {
        token: u64,
        result: Box<Result<Runtime, ProtoError>>,
    },
    /// Availability probe `token` of a stopped stream's device answered.
    Probed {
        token: u64,
        presence: ac2_audio::Presence,
    },
    /// The fan-out of `epoch` measured the capture clock.
    SampleClock {
        epoch: StreamEpoch,
        rate: ac2_proto::model::SampleClock,
        stamp_map: Option<ac2_proto::model::StampMap>,
    },
    /// A link to another rig, or the joined feed, reports.
    Remote(crate::remote::RemoteMsg),
    /// The timing monitor of `epoch` changed state.
    Timing {
        epoch: StreamEpoch,
        status: TimingStatus,
    },
    /// A `delay.find` started under `token` finished.
    DelayFound {
        token: u64,
        result: Box<Result<crate::jobs::finder::Found, String>>,
    },
    /// Delay tracking of `meas` agreed on a new arrival, in (fractional) samples.
    DelayTracked {
        meas: MeasId,
        epoch: StreamEpoch,
        samples: f64,
    },
    /// A transfer function between rigs stopped, relocked, slipped or raised a cue.
    CrossRig {
        meas: MeasId,
        epoch: StreamEpoch,
        event: crate::jobs::cross_rig::CrossEvent,
    },
    /// The `stream.devices` enumeration started under `token` finished.
    Devices {
        token: u64,
        backends: Vec<ac2_proto::model::BackendInfo>,
    },
    /// The preview opening under `token` opened or failed (its result waits in its
    /// channel).
    PreviewOpened { token: u64 },
    /// The `stream.control_panel` call started under `token` answered (or is showing a
    /// modal panel).
    PanelShown {
        token: u64,
        result: Result<(), ProtoError>,
    },
    /// A `stream.detect_loopback` started under `token` finished.
    LoopbackDetected {
        token: u64,
        result: Box<Result<LoopbackDetection, ProtoError>>,
    },
    /// The burst of detection `token` started (`true`) or has played out (`false`).
    LoopbackBurst { token: u64, sounding: bool },
    /// Sweep `id` started playing repeat `repeat` (1-based).
    SweepProgress { id: SweepId, repeat: u8 },
    /// Sweep `id` has its whole recording, or lost audio while recording.
    SweepRecorded {
        id: SweepId,
        result: Box<Result<Recording, String>>,
    },
    /// The analysis of sweep `id` finished: one result per measurement input, in the run's
    /// order.
    SweepAnalysed {
        id: SweepId,
        result: Vec<Result<SweepAnalysis, SweepError>>,
    },
    /// Dynamics run `id` moved on in its programme.
    DynamicsProgress {
        id: ac2_proto::units::DynamicsId,
        phase: ac2_core::dynamics::Phase,
    },
    /// Dynamics run `id` has its whole analysis, or ended without one.
    DynamicsDone {
        id: ac2_proto::units::DynamicsId,
        result: Box<Result<ac2_core::dynamics::DynamicsResult, jobs::dynamics::Failure>>,
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
    /// The simulation worker of run `id` reports progress or its end.
    Sim {
        id: ac2_proto::units::SimId,
        msg: crate::sim::SimMsg,
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
    pub(crate) server: rig::Server,
    pub(crate) lease_expiry: Duration,
    pub(crate) transfers: TransferLimits,
    pub(crate) keepalive: Duration,
    pub(crate) replay: ReplayLimits,
    pub(crate) dedup: DedupLimits,
    pub(crate) ctx: Context,
    pub(crate) endpoint: String,
    pub(crate) interest: Arc<Interest>,
    pub(crate) fps: u32,
    /// Frames from the jobs to the I/O thread.
    pub(crate) frames: crate::outbox::FrameTx,
    pub(crate) outbox: Outbox,
    pub(crate) to_self: Sender<ControlMsg>,
    /// Session directory for `file.*` by name.
    pub(crate) session_dir: std::path::PathBuf,
    pub(crate) cal_store: Option<std::path::PathBuf>,
    /// Autosave directory and whether to restore it at start.
    pub(crate) autosave: Option<crate::config::AutosaveConfig>,
    /// Where `rec.start` writes and `stream.replay` finds recordings by name; `None`:
    /// this daemon does not record.
    pub(crate) recording_dir: Option<std::path::PathBuf>,
    /// The plan library's directory; `None`: plans in memory only.
    pub(crate) plan_dir: Option<std::path::PathBuf>,
    /// Local time of day for the band meters' day and night limits.
    pub(crate) local_clock: crate::config::LocalClock,
    pub(crate) system_clock: crate::config::SystemClockSource,
    pub(crate) systemd_notify: bool,
    /// The DMX outputs' UDP addresses.
    pub(crate) dmx: crate::dmx::DmxPorts,
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
    route: Route,
    client: Peer,
    id: RequestId,
}

/// A loopback detection running on its own thread; answered when the result arrives.
struct PendingDetect {
    token: u64,
    reply: PendingReply,
    cut: crate::detect::Cut,
    /// The burst's level and output, for the generator to show while it sounds.
    level: Dbfs,
    output: u16,
    /// The generator shows the burst firing.
    sounding: bool,
}

/// A `delay.find` running on a job thread; answered when the result arrives.
struct PendingFind {
    route: Route,
    client: Peer,
    id: RequestId,
    meas: MeasId,
}

/// The replies held until a worker thread reports back, by the token its work was given.
#[derive(Default)]
struct Pending {
    /// The loopback detection running.
    detecting: Option<PendingDetect>,
    /// Driver control panel calls under way, by token.
    panels: HashMap<u64, PendingReply>,
    /// The `delay.find` runs under way, by token.
    finds: HashMap<u64, PendingFind>,
    /// The `stream.devices` enumeration under way, by token, and every request waiting
    /// for it: a driver that hangs in its listing holds one thread, however often asked.
    devices: Option<(u64, Vec<PendingReply>)>,
    /// The preview opening on its own thread.
    preview: Option<PreviewOpening>,
}

/// A `stream.preview` whose device opens on its own thread: every request for the same
/// preview meanwhile is answered with it.
struct PreviewOpening {
    token: u64,
    key: PreviewKey,
    waiters: Vec<PendingReply>,
    /// Stopped (or replaced) before it opened: closed as soon as it has.
    stopped: bool,
    result: std::sync::mpsc::Receiver<Result<crate::preview::Preview, ProtoError>>,
}

/// What a preview is of: a request for another one is a new preview.
#[derive(PartialEq)]
struct PreviewKey {
    backend: ac2_proto::model::BackendKind,
    device: ac2_proto::model::DeviceId,
    inputs: Vec<ac2_proto::model::PreviewInput>,
    pairs: Vec<ac2_proto::model::Pair>,
}

/// The keepalive this loop publishes.
struct Keepalive {
    /// The sequence number of the last one sent.
    seq: u64,
    /// When the next one is due.
    due: Instant,
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
    owner: Peer,
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
    epoch: StreamEpoch,
    /// One per measurement input, in the run's order.
    inputs: Vec<SweepInput>,
    /// The walk the run records for, and where.
    walk: Option<walk_sweep::WalkSweepAt>,
}

/// A measurement input of the sweep in progress, as its trace will carry it.
struct SweepInput {
    /// The trace's name: the run's, with the input's name when the run has several.
    name: String,
    /// The input's mic, without a curve: the sweep analysis works on the raw recordings, so
    /// no curve is in its columns.
    mic: Option<MicState>,
    /// The input's active mic curve, put on the trace after capture (as the live jobs on
    /// the input apply it), so the sweep reads what a live transfer of the input reads.
    mic_curve: Option<(
        Box<ac2_proto::model::TraceMicCurve>,
        ac2_core::mic_curve::Correction,
    )>,
    /// Where the input's mic stood, when the run records for a walk.
    position: Option<ac2_proto::model::TracePosition>,
}

pub(crate) struct Control {
    s: Setup,
    store: Store,
    cal: CalStore,
    dedup: Dedup,
    stream: Option<Runtime>,
    /// The health meter keepalives carry.
    health: crate::health::Health,
    /// The open stream's audio stopped and is being reopened; `stream` is `None` meanwhile.
    audio_recovery: Option<recovery::Recovery>,
    /// What fed the spare inputs when the daemon last closed a running stream itself,
    /// connected again to each stream it opens: the operator's routing into spare ports
    /// would otherwise end with every reopen.
    spare_links: Vec<ac2_audio::SpareLink>,
    jobs: BTreeMap<MeasId, JobHandle>,
    /// What each stopped measurement's job left for `trace.capture`.
    kept: kept::Kept,
    /// How spatial averages reach their running members.
    probes: Arc<Probes>,
    timing_job: Option<JobHandle>,
    /// Capture-only meters of a device before a stream opens on it.
    preview: Option<Preview>,
    /// The replies held until a worker reports back.
    pending: Pending,
    /// Warnings the command executing now raised; the next reply carries them.
    reply_warnings: Vec<ac2_proto::Warning>,
    /// The next id of each kind this thread hands out.
    ids: ids::Ids,
    out: Output,
    grids: HashMap<GridId, GridDef>,
    seqs: Arc<Seqs>,
    ka: Keepalive,
    /// The clocks entity's sources.
    clocks: clocks::ClockWatch,
    /// Refused requests already warned about.
    refusals: crate::refusals::Refusals,
    /// systemd's readiness and watchdog, pinged from this loop's keepalive tick so that a
    /// stuck loop stops the pings and gets the daemon restarted.
    notify: ac2_paths::notify::Notifier,
    /// State files found damaged, not yet in the rig entity.
    damaged: Vec<crate::damage::Damage>,
    traces: traces::TraceStore,
    sweep: Option<ActiveSweep>,
    dynamics: Option<dynamics::ActiveDynamics>,
    /// Walk measurements of a loaded session whose first run still owes the restart a
    /// move gives: their delay and average belong to the mic's placement when it was saved.
    walk_resume: HashSet<MeasId>,
    autosave: Option<Autosaver>,
    /// Load the autosave when the control thread starts.
    restore: bool,
    logs: SplLogs,
    /// The raw capture being written.
    recording: Option<recording::ActiveRecording>,
    rig: RigState,
    /// What the programme meters of the stream and the preview read: `rig.levels`, the
    /// inputs and line calibrations (VU offers), the pairs, the resets.
    levels: crate::jobs::programme::SharedPolicy,
    /// The recording a replay stream plays (`stream.replay`).
    replay_backend: Option<Arc<ac2_audio::ReplayBackend>>,
    /// The replayed recording's remote-channel marks, for every open of `replay_backend`.
    replay_marks: Option<Arc<crate::remote::replay::ReplayMarks>>,
    /// What the working state belongs to (`State.working`).
    working: working::Persist,
    /// The working state the last `file.new` / `file.load` replaced with unsaved changes.
    replaced_working: Option<working::Recover>,
    /// Session archives being uploaded or downloaded.
    transfers: transfer::Transfers,
    /// File operations whose disk work runs on a worker, by token.
    file_jobs: file_jobs::FileJobs,
    /// The room simulation running and the fit kept for later runs.
    sims: sim::Sims,
    /// The links to the paired rigs and what they report.
    remote: remote::RemoteCtl,
    /// Each measurement's next run number (none held: 1).
    next_run: HashMap<MeasId, u32>,
}

/// The stimulus output: who may drive it, its gate, level and source, and the ceiling.
struct Output {
    lease: Option<Lease>,
    gate: Arc<LeaseGate>,
    level: Option<LevelControl>,
    source: Option<SourceKey>,
    /// The firing sine the spectrum jobs were last told (Hz).
    tone: Option<f64>,
}

/// The rig's persistent settings, its meters and the cue senders they drive.
struct RigState {
    /// Where the rig settings are kept.
    settings: crate::rig::RigStore,
    /// The rig's SPL meters and the monitor at start.
    meas: rig_meas::RigMeas,
    /// The OSC cues' states and their sender.
    osc: osc::Cues,
    /// The DMX universe the cue states make, and its Art-Net and sACN outputs.
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
    pub(crate) fn run(mut self, rx: &Receiver<ControlMsg>) -> crate::Ended {
        self.sync_levels_policy();
        self.load_rig_meas();
        self.start_autosave();
        self.start_links();
        self.start_rig_monitor();
        self.recover_recordings();
        self.publish_damage();
        // The sockets were bound before this thread started; the restore and the rig
        // monitor's start are done or under way.
        if let Some(e) = self.notify.ready() {
            tracing::warn!("systemd notification failed: {e}");
        }
        let ended = loop {
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
            self.transfers.tick(now);
            self.check_dynamics_deadline(now);
            if now >= self.ka.due {
                self.send_ka();
                self.ka.due = now + self.s.keepalive;
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
            self.clocks_tick(now);
            self.remote_sync();
            if self.preview.as_ref().is_some_and(|p| p.deadline <= now) {
                tracing::info!("preview not renewed: closing it");
                self.close_running_preview();
            }
            self.osc_sync();
            self.osc_tick(now);
            self.artnet_tick(now);
            self.s.server.network.tick(&self.s.outbox, now);
            self.server_sync();
            let mut wake = self.ka.due.min(self.clocks_due());
            if let Some(t) = self.osc_wake() {
                wake = wake.min(t);
            }
            if let Some(t) = self.artnet_wake() {
                wake = wake.min(t);
            }
            if let Some(d) = self.s.server.network.due() {
                wake = wake.min(d);
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
                    route,
                    peer,
                    payload,
                }) => self.on_request(&route, peer, &payload),
                Ok(ControlMsg::NetFailed { epoch, reason }) => {
                    if self.s.server.network.failed(epoch, reason, Instant::now()) {
                        self.drop_network_lease();
                    }
                    self.server_sync();
                }
                Ok(ControlMsg::DeviceChanged { epoch }) => {
                    if self.stream.as_ref().is_some_and(|r| r.epoch == epoch) {
                        tracing::warn!("device or configuration change: reopening the audio I/O");
                        let routes = self
                            .stream
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
                Ok(ControlMsg::SampleClock {
                    epoch,
                    rate,
                    stamp_map,
                }) => self.sample_clock(epoch, rate, stamp_map),
                Ok(ControlMsg::Remote(m)) => self.remote_msg(m),
                Ok(ControlMsg::Timing { epoch, status }) => {
                    if self.stream.as_ref().is_some_and(|r| r.epoch == epoch)
                        && self.store.state().timing != status
                    {
                        self.commit(Change::Timing(status));
                    }
                }
                Ok(ControlMsg::DelayFound { token, result }) => self.finish_find(token, *result),
                Ok(ControlMsg::Devices { token, backends }) => {
                    if let Some((_, waiters)) = self.pending.devices.take_if(|(t, _)| *t == token) {
                        let now = Instant::now();
                        for p in waiters {
                            let r = self
                                .still_operator(&p.client, "stream.devices")
                                .map(|()| ReplyBody::Backends(backends.clone()));
                            self.answer(&p.route, &p.client, p.id, r, now);
                        }
                    }
                }
                Ok(ControlMsg::PreviewOpened { token }) => self.preview_opened(token),
                Ok(ControlMsg::PanelShown { token, result }) => {
                    if let Some(p) = self.pending.panels.remove(&token) {
                        let r = self
                            .still_operator(&p.client, "stream.control_panel")
                            .and(result)
                            .map(|()| ReplyBody::Ack {
                                rev: self.store.rev(),
                            });
                        self.answer(&p.route, &p.client, p.id, r, Instant::now());
                    }
                }
                Ok(ControlMsg::LoopbackBurst { token, sounding }) => {
                    self.loopback_burst(token, sounding);
                }
                Ok(ControlMsg::LoopbackDetected { token, result }) => {
                    self.loopback_burst(token, false);
                    if let Some(d) = self.pending.detecting.take_if(|d| d.token == token) {
                        let p = d.reply;
                        let r = self
                            .still_operator(&p.client, "stream.detect_loopback")
                            .and_then(|()| (*result).map(ReplyBody::LoopbackDetection));
                        self.answer(&p.route, &p.client, p.id, r, Instant::now());
                    }
                }
                Ok(ControlMsg::DelayTracked {
                    meas,
                    epoch,
                    samples,
                }) => self.tracked(meas, epoch, samples),
                Ok(ControlMsg::CrossRig { meas, epoch, event }) => {
                    self.cross_event(meas, epoch, event);
                }
                Ok(ControlMsg::SweepProgress { id, repeat }) => self.sweep_progress(id, repeat),
                Ok(ControlMsg::SweepRecorded { id, result }) => self.sweep_recorded(id, *result),
                Ok(ControlMsg::SweepAnalysed { id, result }) => self.sweep_analysed(id, result),
                Ok(ControlMsg::DynamicsProgress { id, phase }) => self.dynamics_progress(id, phase),
                Ok(ControlMsg::DynamicsDone { id, result }) => self.dynamics_done(id, *result),
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
                Ok(ControlMsg::Sim { id, msg }) => self.sim_message(id, msg),
                Ok(ControlMsg::RecoverInstalled { token, result }) => {
                    self.recover_installed(token, result);
                }
                Ok(ControlMsg::Fatal(why)) => {
                    tracing::error!("fatal: {why}");
                    break crate::Ended::Fatal(why);
                }
                Ok(ControlMsg::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                    break crate::Ended::Stopped;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        };
        self.shutdown();
        ended
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
        self.abort_runs(SweepFailure::Stopped, "the daemon is shutting down");
        self.shutdown_sim();
        self.end_recording(ac2_proto::model::RecordingEnd::DaemonShutdown);
        // A preview still opening past the wait is dropped when its open returns.
        let _ = self.close_preview();
        self.stop_output();
        self.stop_all_jobs();
        self.audio_recovery = None;
        if let Some(rt) = self.stream.take() {
            recovery::close_bounded(rt, recovery::CLOSE_BOUND);
        }
        self.flush_file_jobs();
        self.flush_autosave();
        self.finish_rig_logs();
        self.s.server.network.shutdown();
        self.s.outbox.stop();
    }

    // -- plumbing --------------------------------------------------------------------------

    fn commit(&mut self, change: Change) -> Rev {
        let saved = matches!(
            change,
            Change::Measurement(_)
                | Change::Delay(_)
                | Change::Trace(_)
                | Change::Walk(_)
                | Change::WalkPlan(_)
        );
        let generator = matches!(change, Change::Generator(_));
        let levels = matches!(
            change,
            Change::Rig(_) | Change::Inputs(_) | Change::LineCalibration(_)
        );
        self.rig_meas_committing(&change);
        self.runs_committing(&change);
        let sim_stops = self.sim_stopped_by(&change);
        let ev = self.store.commit(change, Instant::now());
        // After the commit: the timeline keeps the measurement as committed, `edit_rev`
        // included.
        self.note_for_recording(&ev.change);
        if levels {
            self.sync_levels_policy();
        }
        if generator {
            self.sync_tone();
        }
        match ac2_proto::encode_event(&ev) {
            Ok(b) => {
                if b.len() > ac2_proto::event::MAX_EVENT_BYTES {
                    // A client drops it unread; the I/O thread owes every connection that
                    // takes events `resync_required` instead, sent ahead of its next event,
                    // credit or keepalive, and the snapshot that brings holds the value.
                    // Nor can a `state.since` carry it.
                    self.store.not_replayable(ev.rev);
                    tracing::warn!(
                        "event rev {} ({}) is {} bytes, over the {}-byte event limit: not \
                         sent; event connections are sent resync_required with their next \
                         message",
                        ev.rev.0,
                        ev.change.kind(),
                        b.len(),
                        ac2_proto::event::MAX_EVENT_BYTES
                    );
                }
                // The network pair is sent its own copy where the change names host paths.
                let net = redact::change(&ev.change).and_then(|change| {
                    let mut n = ev.clone();
                    n.change = change;
                    ac2_proto::encode_event(&n)
                        .inspect_err(|e| tracing::error!("event not encodable: {e}"))
                        .ok()
                });
                self.s.outbox.event(&b, net.as_deref());
            }
            Err(e) => tracing::error!("event not encodable: {e}"),
        }
        if saved {
            self.working_changed();
            self.autosave_changed();
        }
        self.sim_after(sim_stops);
        ev.rev
    }

    fn epoch(&self) -> StreamEpoch {
        self.store.state().stream.epoch
    }

    fn meas(&self, id: MeasId) -> Result<&Measurement, ProtoError> {
        self.store
            .state()
            .measurements
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| not_found(id))
    }

    /// Commits `m` as its measurement's new state and answers with it as stored: the commit
    /// stamped its `edit_rev`, which the caller's next `meas.update` names as its base.
    fn reply_meas(&mut self, m: Measurement) -> ReplyBody {
        ReplyBody::Measurement(self.commit_meas_read(m))
    }

    /// Commits `m` and reads it back as stored, `edit_rev` included.
    fn commit_meas_read(&mut self, m: Measurement) -> Measurement {
        let id = m.id;
        self.commit(Change::Measurement(Patch::Set(m)));
        self.store
            .state()
            .measurements
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .expect("a measurement just committed is in the state")
    }

    fn register_grid(&mut self, g: GridDef) -> GridId {
        let id = g.id();
        self.grids.entry(id).or_insert(g);
        id
    }
}
