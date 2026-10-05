//! The control thread: decodes requests, executes commands one at a time against the state
//! store (serial commits), runs the session and jobs, owns the stimulus lease and emits
//! keepalives.
//!
//! It is a plain thread fed by one channel. Commands are short (job threads do the DSP),
//! and a single consumer gives serial commits and a total event order for free; an async
//! runtime would add scheduling without adding concurrency that the state model allows.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use ac2_audio::{Backend, Gain, MaxLevel};
use ac2_core::delay::FinderResult;
use ac2_core::generator::{
    BandLimit as CoreBandLimit, Generator as CoreGenerator, GeneratorConfig, GeneratorError,
    LevelControl, Signal as CoreSignal, dbfs_to_rms,
};
use ac2_core::sweep::{SweepAnalysis, SweepError, SweepSpec};
use ac2_proto::event::{Change, Patch};
use ac2_proto::frame::{Frame, FrameData, FrameStamp, GenSummary, KaMeta, ProtectionFlags};
use ac2_proto::grid::{GridDef, GridId};
use ac2_proto::model::{
    Autosave, AutosaveState, Availability, BackendInfo, CalState, DelayOutcome, DelayState,
    FinderBand, GenAction, GenAudit, Generator, GeneratorDesired, GeneratorSettings, InputSetup,
    Lease as WireLease, LoopbackDetection, MeasConfig, MeasKind, Measurement, MicState, Session,
    SessionConfig, SweepFailure, SweepInputs, SweepRequest, SweepRun, SweepStatus, TimingStatus,
    TraceKind, TraceMeta, TraceSource,
};
use ac2_proto::topic::Topic;
use ac2_proto::units::{
    ClientId, DaemonIncarnation, Dbfs, LeaseToken, MeasId, RequestId, Rev, SampleIndex, Seconds,
    SessionEpoch, SweepId, WallNs,
};
use ac2_proto::{
    Command, ErrorCode, ErrorDetail, PROTO_VERSION, ProtoError, Reply, ReplyBody, Welcome,
    decode_request, encode_reply, peek_envelope,
};
use ac2_zmq::Context;

use crate::autosave::Autosaver;
use crate::calstore::{self, CalStore, InputCal};
use crate::config::{DedupLimits, ReplayLimits};
use crate::conv;
use crate::dedup::Dedup;
use crate::io::Interest;
use crate::jobs::{self, Analysis, JobCmd, JobEnv, JobHandle, Seqs, SmoothingChange, block_index};
use crate::outbox::Outbox;
use crate::preview::Preview;
use crate::session::{self, Runtime};
use crate::state::Store;
use crate::stimulus::{LeaseGate, LeasedSource, SweepTrain};
use crate::sweep::Recording;
use crate::util::{hex, perr, perr_detail, random_u64, random_u128, wall_ns};

mod autosave;
mod cal;
mod files;
mod leq;
mod sweeps;
mod traces;

/// Everything that reaches the control thread.
pub(crate) enum ControlMsg {
    /// A ctrl request from the I/O thread.
    Request {
        routing_id: Vec<u8>,
        user_id: Option<String>,
        payload: Vec<u8>,
    },
    /// The stream of `epoch` reported a configuration change or ended.
    DeviceChanged { epoch: SessionEpoch },
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
    /// Delay tracking of `meas` agreed on a new delay.
    DelayTracked {
        meas: MeasId,
        epoch: SessionEpoch,
        samples: i64,
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
        alarms: Vec<ac2_proto::model::LeqAlarm>,
    },
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
    pub(crate) ceiling_dbfs: f64,
    pub(crate) max_level: MaxLevel,
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
    jobs: BTreeMap<MeasId, JobHandle>,
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
}

const MAX_DELAY_S: f64 = 10.0;

/// A delay in samples at `fs`, fractions kept. Snapped to a millionth of a sample: a delay
/// given in seconds or built from fractional steps lands within float rounding of the value
/// meant, and a whole-sample delay must stay exactly whole (the engine then applies no phase
/// rotation at all). A millionth of a sample is 0.0002° at 20 kHz and 48 kHz.
fn delay_samples(seconds: f64, fs: f64) -> f64 {
    (seconds * fs * 1e6).round() / 1e6
}
/// Largest difference between the fast and slow input mean squares a calibration accepts,
/// dB.
const MAX_CAL_UNSETTLED_DB: f64 = 0.05;

/// Refuses a `delay.find` the finder could not run as asked: band edges or observation out
/// of range for `fs`. Sub-band observations are the operator's 2 / 4 / 8 s choice (D2).
fn check_find(band: conv::FindBand, observation: Option<f64>, fs: f64) -> Result<(), ProtoError> {
    use ac2_core::delay::{Band, BandClass, FinderConfig};
    let inv = |m: String| Err(perr(ErrorCode::Invalid, m));
    let core_band = match band {
        conv::FindBand::Auto => Band::FullRange,
        conv::FindBand::Band(b) => b,
    };
    let mut cfg = FinderConfig::new(fs, core_band);
    cfg.observation_s = observation;
    if let Err(e) = cfg.validate() {
        return inv(format!("delay finder: {e}"));
    }
    if let Some(s) = observation {
        if s > jobs::finder::MAX_OBSERVATION_S {
            return inv(format!(
                "observation {s} s is longer than {} s",
                jobs::finder::MAX_OBSERVATION_S
            ));
        }
        let sub = match band {
            conv::FindBand::Auto => false,
            conv::FindBand::Band(b) => b.class() == BandClass::Sub,
        };
        if sub && ![2.0, 4.0, 8.0].contains(&s) {
            return inv(format!(
                "sub-band observation must be 2, 4 or 8 s, not {s} s"
            ));
        }
    }
    Ok(())
}

/// `current` with `rows` replacing the rows of their channels, sorted by channel.
fn upsert_inputs(current: &[InputSetup], rows: Vec<InputSetup>) -> Vec<InputSetup> {
    let mut out: Vec<InputSetup> = current
        .iter()
        .filter(|c| rows.iter().all(|r| r.channel != c.channel))
        .cloned()
        .collect();
    out.extend(rows);
    out.sort_by_key(|i| i.channel);
    out
}

fn mutation_conflict(rev: Rev) -> ProtoError {
    perr_detail(
        ErrorCode::Conflict,
        format!("state has moved on to rev {}", rev.0),
        ErrorDetail::Conflict { rev },
    )
}

fn not_found(meas: MeasId) -> ProtoError {
    perr(ErrorCode::NotFound, format!("no measurement {}", meas.0))
}

fn lease_required() -> ProtoError {
    perr(
        ErrorCode::LeaseRequired,
        "this command needs the stimulus lease; the token is missing, stale or expired",
    )
}

fn gen_err(e: GeneratorError) -> ProtoError {
    let code = match e {
        GeneratorError::WouldClip { .. } | GeneratorError::AboveCeiling { .. } => {
            ErrorCode::Refused
        }
        _ => ErrorCode::Invalid,
    };
    perr(code, e.to_string())
}

/// Checks a measurement configuration without a session.
fn validate_meas(c: &MeasConfig) -> Result<(), ProtoError> {
    let inv = |m: &str| Err(perr(ErrorCode::Invalid, m.to_owned()));
    match &c.kind {
        MeasKind::Transfer { config } => {
            if config.reference_input == config.measurement_input {
                return inv("reference and measurement must be different inputs");
            }
            let g = config.grid;
            if g.ppo == 0 || g.ppo > 96 || g.k_min > g.k_max {
                return inv("invalid grid");
            }
            if i64::from(g.k_max) - i64::from(g.k_min) + 1 > i64::from(ac2_proto::frame::MAX_N) {
                return inv("grid too large");
            }
            if conv::tf_averaging(config.averaging).is_none() {
                return inv("invalid averaging");
            }
            if conv::depth(config.depth).is_none() {
                return inv("fast_lf max_settle_s must be a positive time");
            }
        }
        MeasKind::Spectrum { config } => {
            if !config.fft_len.is_power_of_two() || !(64..=65536).contains(&config.fft_len) {
                return inv("fft_len must be a power of two in 64..=65536");
            }
            if conv::spec_averaging(config.averaging).is_none() {
                return inv("invalid averaging");
            }
        }
        MeasKind::Rta { config } => {
            if !(config.f_lo.0.is_finite()
                && config.f_hi.0.is_finite()
                && config.f_lo.0 > 0.0
                && config.f_hi.0 > config.f_lo.0)
            {
                return inv("f_lo must be positive and below f_hi");
            }
            if conv::spec_averaging(config.averaging).is_none() {
                return inv("invalid averaging");
            }
        }
        MeasKind::Spl { config } => {
            config
                .leq
                .check()
                .map_err(|m| perr(ErrorCode::Invalid, m))?;
        }
    }
    Ok(())
}

/// The new configuration when `new` is `old` on the same input (an SPL meter): its
/// weightings and Leq windows change in place — the meter runs every weighting all along, so
/// its interval, its log and its windows carry on.
fn spl_in_place(old: &MeasKind, new: &MeasKind) -> Option<ac2_proto::model::SplConfig> {
    match (old, new) {
        (MeasKind::Spl { config: a }, MeasKind::Spl { config: b }) if a.input == b.input => {
            Some(b.clone())
        }
        _ => None,
    }
}

/// The new smoothing when `new` is `old` with only the display smoothing changed (a
/// transfer or spectrum measurement); such an update is applied in place instead of
/// restarting the job.
fn smoothing_only(old: &MeasKind, new: &MeasKind) -> Option<SmoothingChange> {
    match (old, new) {
        (MeasKind::Transfer { config: a }, MeasKind::Transfer { config: b })
            if ac2_proto::model::TransferConfig {
                smoothing: b.smoothing,
                ..a.clone()
            } == *b =>
        {
            Some(SmoothingChange::Transfer(b.smoothing))
        }
        (MeasKind::Spectrum { config: a }, MeasKind::Spectrum { config: b })
            if ac2_proto::model::SpectrumConfig {
                smoothing: b.smoothing,
                ..a.clone()
            } == *b =>
        {
            Some(SmoothingChange::Spectrum(b.smoothing))
        }
        _ => None,
    }
}

/// Grid of a transfer measurement; known without a session.
fn static_grid(kind: &MeasKind) -> Option<GridDef> {
    match kind {
        MeasKind::Transfer { config } => Some(GridDef::Log {
            ppo: config.grid.ppo,
            k_min: config.grid.k_min,
            k_max: config.grid.k_max,
        }),
        _ => None,
    }
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
        let store = Store::new(Dbfs(s.ceiling_dbfs), s.replay)
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
            jobs: BTreeMap::new(),
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
            s,
        }
    }

    pub(crate) fn run(mut self, rx: &Receiver<ControlMsg>) {
        self.start_autosave();
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
                    alarms,
                }) => self.leq_reported(meas, epoch, config_rev, at, &judgements, alarms),
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
        self.close_preview();
        self.stop_output();
        self.stop_all_jobs();
        if let Some(rt) = self.session.take() {
            rt.close();
        }
        self.flush_autosave();
        self.s.outbox.stop();
    }

    // -- plumbing --------------------------------------------------------------------------

    fn commit(&mut self, change: Change) -> Rev {
        let saved = matches!(change, Change::Measurement(_) | Change::Trace(_));
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

    fn send_ka(&mut self) {
        // Keepalives tell subscribers the daemon is alive; with nobody subscribed there is
        // no one to tell, and skipping them spares the I/O thread a wakeup each.
        if !self.s.interest.wants(&Topic::Ka.to_bytes()) {
            return;
        }
        self.ka_seq += 1;
        let st = self.store.state();
        let now = wall_ns();
        let latest = self
            .session
            .as_ref()
            .map_or(0u64, |r| r.fanout.latest.load(Ordering::Acquire));
        let frame = Frame {
            stamp: FrameStamp {
                seq: self.ka_seq,
                audio_sample: SampleIndex(latest.saturating_sub(1)),
                session_epoch: st.session.epoch,
                daemon_incarnation: self.s.incarnation,
                config_rev: self.store.rev(),
                config_applied_at: SampleIndex(0),
                capture_wall_ns: WallNs(now),
                grid_id: None,
                protection: ProtectionFlags::NONE,
            },
            data: FrameData::Ka(KaMeta {
                rev: self.store.rev(),
                daemon_wall_ns: WallNs(now),
                timing: st.timing.state,
                generator: GenSummary {
                    owner: st.generator.owner.clone(),
                    armed: st.generator.armed,
                    firing: st.generator.firing,
                },
            }),
        };
        match ac2_proto::encode_frame(&frame) {
            Ok(parts) => self.s.outbox.ka(parts),
            Err(e) => tracing::error!("keepalive not encodable: {e}"),
        }
    }

    fn on_request(&mut self, routing_id: &[u8], user_id: Option<String>, payload: &[u8]) {
        let now = Instant::now();
        let env = match peek_envelope(payload) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("undecodable ctrl message dropped: {e}");
                return;
            }
        };
        let id = env.id.unwrap_or(RequestId(0));
        if env.v != Some(PROTO_VERSION) {
            tracing::warn!("refused a client at protocol version {:?}", env.v);
            self.send_reply(routing_id, &Reply::version_refusal(id, env.v));
            return;
        }
        let req = match decode_request(payload) {
            Ok(r) => r,
            Err(e) => {
                self.send_reply(
                    routing_id,
                    &Reply::new(id, Err(perr(ErrorCode::Invalid, e.to_string()))),
                );
                return;
            }
        };
        let client = ClientId(user_id.unwrap_or_else(|| format!("local-{}", hex(routing_id))));
        if let Some(stored) = self.dedup.get(&client, req.id, now) {
            let stored = stored.to_vec();
            tracing::debug!("{} retried request {}; stored reply", client.0, req.id.0);
            self.s.outbox.reply(routing_id, &stored);
            return;
        }
        let result = match req.expect_rev {
            Some(r) if req.cmd.is_mutation() && r != self.store.rev() => {
                Err(mutation_conflict(self.store.rev()))
            }
            _ => match req.cmd {
                // The finder runs for a while on the job thread; the reply follows its result
                // and other clients are served meanwhile.
                // The burst and its capture take about a second on their own thread; the
                // reply follows the result.
                Command::SessionDetectLoopback {
                    lease_token,
                    backend,
                    device,
                    output,
                    level,
                } => {
                    match self.start_detect(&client, lease_token, backend, device, output, level) {
                        Ok(token) => {
                            self.detecting = Some((
                                token,
                                PendingReply {
                                    routing_id: routing_id.to_vec(),
                                    client,
                                    id: req.id,
                                },
                            ));
                            return;
                        }
                        Err(e) => Err(e),
                    }
                }
                Command::DelayFind {
                    meas,
                    band,
                    observation,
                } => match self.start_find(meas, band, observation) {
                    Ok(token) => {
                        self.pending_finds.insert(
                            token,
                            PendingFind {
                                routing_id: routing_id.to_vec(),
                                client,
                                id: req.id,
                                meas,
                            },
                        );
                        return;
                    }
                    Err(e) => Err(e),
                },
                cmd => self.execute(&client, cmd),
            },
        };
        self.answer(routing_id, &client, req.id, result, now);
    }

    /// Sends a reply and remembers it for retries of `id`.
    fn answer(
        &mut self,
        routing_id: &[u8],
        client: &ClientId,
        id: RequestId,
        result: Result<ReplyBody, ProtoError>,
        now: Instant,
    ) {
        let reply = Reply::new(id, result);
        let bytes = match encode_reply(&reply) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("reply not encodable: {e}");
                match encode_reply(&Reply::new(
                    id,
                    Err(perr(ErrorCode::Internal, e.to_string())),
                )) {
                    Ok(b) => b,
                    Err(_) => return,
                }
            }
        };
        self.dedup.insert(client, id, bytes.clone(), now);
        self.s.outbox.reply(routing_id, &bytes);
    }

    fn send_reply(&self, routing_id: &[u8], r: &Reply) {
        match encode_reply(r) {
            Ok(b) => self.s.outbox.reply(routing_id, &b),
            Err(e) => tracing::error!("reply not encodable: {e}"),
        }
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
                let id = MeasId(self.next_meas);
                self.next_meas += 1;
                let grid_id = static_grid(&config.kind).map(|g| self.register_grid(g));
                let delay = matches!(config.kind, MeasKind::Transfer { .. }).then(|| DelayState {
                    applied: Seconds(0.0),
                    applied_samples: 0.0,
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
                        tracking: false,
                        awaiting_pick: false,
                        last_finding: None,
                    });
                }
                m.grid_id = static_grid(&m.config.kind).map(|g| self.register_grid(g));
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
            Command::MeasDelete { meas } => {
                self.meas(meas)?;
                self.stop_job(meas);
                self.s
                    .outbox
                    .clear(&ac2_proto::Subscription::Meas(meas).prefix());
                self.drop_spl_log(meas);
                ack(self.commit(Change::Measurement(Patch::Deleted(meas))))
            }
            Command::MeasStart { meas } => {
                let mut m = self.meas(meas)?.clone();
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
                self.stop_job(meas);
                m.running = false;
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasFreeze { meas, frozen } => {
                let mut m = self.meas(meas)?.clone();
                m.frozen = frozen;
                if let Some(j) = self.jobs.get(&meas) {
                    j.send(JobCmd::Freeze(frozen));
                }
                self.commit(Change::Measurement(Patch::Set(m.clone())));
                Ok(ReplyBody::Measurement(m))
            }
            Command::MeasReset { meas } => {
                self.meas(meas)?;
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
            Command::TraceMath { a, b, op, name } => self.trace_math(a, b, op, name),
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
            Command::SplHistoryGet { meas, seconds } => self.spl_history_get(meas, seconds),

            Command::IrCapture {
                lease_token,
                request,
                name,
            } => self.ir_capture(client, lease_token, request, name),

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

    // -- session ---------------------------------------------------------------------------

    fn session_open(
        &mut self,
        client: &ClientId,
        config: SessionConfig,
    ) -> Result<ReplyBody, ProtoError> {
        session::validate(&config)?;
        let backend = self.backend_for(config.backend)?;
        // The preview may hold the very device the session is about to open.
        self.close_preview();
        if self.session.is_some() {
            self.session_close(client);
        }
        let epoch = SessionEpoch(self.epoch().0 + 1);
        let rt = match Runtime::open(
            &*backend,
            &config,
            &[],
            self.s.max_level,
            epoch,
            self.s.to_self.clone(),
            self.s.fps,
        ) {
            Ok(rt) => rt,
            Err(e) => {
                tracing::warn!("session open failed: {}", e.msg);
                return Err(e);
            }
        };
        let s = Session {
            epoch,
            open: Some(rt.open.clone()),
        };
        self.session = Some(rt);
        self.commit(Change::Session(s.clone()));
        self.after_open();
        Ok(ReplyBody::Session(s))
    }

    /// Starts jobs and re-derives sample-rate dependent state for a freshly opened stream.
    fn after_open(&mut self) {
        let Some(fs) = self.session.as_ref().map(|r| f64::from(r.sample_rate)) else {
            return;
        };
        let ms: Vec<Measurement> = self.store.state().measurements.clone();
        for mut m in ms {
            let mut changed = false;
            if let Some(d) = &mut m.delay {
                let samples = delay_samples(d.applied.0, fs);
                if samples != d.applied_samples {
                    d.applied_samples = samples;
                    d.applied = Seconds(samples / fs);
                    m.config_rev = Rev(self.store.rev().0 + 1);
                    changed = true;
                }
            }
            if m.running {
                match self.start_job(&m) {
                    Ok(Some(g)) => {
                        let id = self.register_grid(g);
                        if m.grid_id != Some(id) {
                            m.grid_id = Some(id);
                            changed = true;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!("measurement {} not started: {}", m.id.0, e.msg),
                }
            }
            if changed {
                self.commit(Change::Measurement(Patch::Set(m)));
            }
        }
        self.start_timing_job();
        self.start_session_levels();
    }

    fn session_close(&mut self, client: &ClientId) {
        self.abort_sweep(SweepFailure::SessionClosed, "the audio session closed");
        self.stop_all_jobs();
        let g = self.store.state().generator.clone();
        if g.armed || g.firing {
            self.stop_output();
            let mut g = g;
            g.armed = false;
            g.firing = false;
            self.audit(&mut g, GenAction::Stop, Some(client));
            self.commit(Change::Generator(g));
        } else {
            self.stop_output();
        }
        if let Some(rt) = self.session.take() {
            rt.close();
            self.s.outbox.clear(b"d/");
            self.s.outbox.clear(b"timing");
            self.s.outbox.clear(b"session/levels");
            let epoch = SessionEpoch(self.epoch().0 + 1);
            self.commit(Change::Session(Session { epoch, open: None }));
        }
    }

    /// Reopens the stream with the same configuration and the given generator routes: a
    /// device change is a configuration change, so it starts a new session epoch (5b).
    fn reopen(&mut self, routes: &[u16]) -> Result<(), ProtoError> {
        let epoch = SessionEpoch(self.epoch().0 + 1);
        self.reopen_at(routes, epoch)
    }

    /// [`Self::reopen`] into a given (newer) epoch.
    fn reopen_at(&mut self, routes: &[u16], epoch: SessionEpoch) -> Result<(), ProtoError> {
        let Some(rt) = self.session.take() else {
            return Err(perr(ErrorCode::Invalid, "no open session"));
        };
        let config = rt.open.config.clone();
        let backend = self.backend_for(Some(rt.open.backend))?;
        self.abort_sweep(SweepFailure::SessionClosed, "the audio session reopened");
        self.stop_all_jobs();
        self.level = None;
        self.source = None;
        rt.close();
        self.s.outbox.clear(b"d/");
        self.s.outbox.clear(b"timing");
        self.s.outbox.clear(b"session/levels");
        match Runtime::open(
            &*backend,
            &config,
            routes,
            self.s.max_level,
            epoch,
            self.s.to_self.clone(),
            self.s.fps,
        ) {
            Ok(rt) => {
                let s = Session {
                    epoch,
                    open: Some(rt.open.clone()),
                };
                self.session = Some(rt);
                self.commit(Change::Session(s));
                self.after_open();
                Ok(())
            }
            Err(e) => {
                self.commit(Change::Session(Session { epoch, open: None }));
                let g = self.store.state().generator.clone();
                if g.armed || g.firing {
                    let mut g = g;
                    g.armed = false;
                    g.firing = false;
                    self.audit(&mut g, GenAction::Stop, None);
                    self.commit(Change::Generator(g));
                }
                Err(e)
            }
        }
    }

    // -- backends, preview, loopback detection ----------------------------------------------

    /// The backend `kind` names; `None` = the default one.
    fn backend_for(
        &self,
        kind: Option<ac2_proto::model::BackendKind>,
    ) -> Result<Arc<dyn Backend>, ProtoError> {
        match kind {
            None => self.s.backends.first().cloned(),
            Some(k) => self
                .s
                .backends
                .iter()
                .find(|b| conv::backend_kind(b.kind()) == k)
                .cloned(),
        }
        .ok_or_else(|| {
            perr(
                ErrorCode::NotFound,
                format!("this daemon offers no {kind:?} backend"),
            )
        })
    }

    fn backend_infos(&self) -> Vec<BackendInfo> {
        self.s
            .backends
            .iter()
            .map(|b| {
                let (availability, devices) = match b.enumerate() {
                    Ok(d) => (
                        Availability::Available,
                        d.iter().map(conv::device_info).collect(),
                    ),
                    Err(ac2_audio::AudioError::Unavailable { reason, .. }) => (
                        Availability::Unavailable {
                            reason: reason.to_string(),
                        },
                        Vec::new(),
                    ),
                    Err(e) => (
                        Availability::Unavailable {
                            reason: e.to_string(),
                        },
                        Vec::new(),
                    ),
                };
                BackendInfo {
                    kind: conv::backend_kind(b.kind()),
                    description: crate::backend::describe(b.kind()),
                    availability,
                    devices,
                }
            })
            .collect()
    }

    fn close_preview(&mut self) {
        if let Some(p) = self.preview.take() {
            p.close();
            self.s.outbox.clear(b"session/preview");
        }
    }

    fn session_preview(
        &mut self,
        kind: ac2_proto::model::BackendKind,
        device: ac2_proto::model::DeviceId,
    ) -> Result<ReplyBody, ProtoError> {
        if self.detecting.is_some() {
            // The burst holds the device; a preview asked for before the detection started
            // must not reopen it under the burst.
            return Err(perr(
                ErrorCode::Refused,
                "a loopback detection holds the device; the meters return with its result",
            ));
        }
        if let Some(p) = &mut self.preview
            && p.backend == kind
            && p.device == device
        {
            match p.dead(Instant::now()) {
                None => {
                    p.renew();
                    return Ok(ReplyBody::Preview(p.wire()));
                }
                Some(why) => {
                    tracing::warn!("preview of {kind:?} {:?}: {why}; reopening it", device.0);
                }
            }
        }
        let backend = self.backend_for(Some(kind))?;
        self.close_preview();
        let p = Preview::open(
            &*backend,
            kind,
            device,
            self.s.max_level,
            self.env_at(self.epoch()),
        )?;
        let wire = p.wire();
        self.preview = Some(p);
        Ok(ReplyBody::Preview(wire))
    }

    /// Checks a `session.detect_loopback` and starts it on its own thread.
    fn start_detect(
        &mut self,
        client: &ClientId,
        token: LeaseToken,
        kind: ac2_proto::model::BackendKind,
        device: ac2_proto::model::DeviceId,
        output: u16,
        level: Option<Dbfs>,
    ) -> Result<u64, ProtoError> {
        self.check_lease(Instant::now());
        self.lease_check(client, token)?;
        let Some(level) = level else {
            return Err(perr(
                ErrorCode::Refused,
                "type the burst level: loopback detection has no default level",
            ));
        };
        if !level.0.is_finite() {
            return Err(perr(ErrorCode::Invalid, "level must be finite"));
        }
        if level.0 > self.s.ceiling_dbfs {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "{:.1} dBFS is above the global maximum {:.1} dBFS",
                    level.0, self.s.ceiling_dbfs
                ),
            ));
        }
        let g = &self.store.state().generator;
        if g.armed || g.firing {
            return Err(perr(
                ErrorCode::Refused,
                "the stimulus is armed: stop it before detecting the loopback",
            ));
        }
        if self.detecting.is_some() {
            return Err(perr(
                ErrorCode::Refused,
                "a loopback detection is already running",
            ));
        }
        let backend = self.backend_for(Some(kind))?;
        self.close_preview();
        let token = self.next_token;
        self.next_token += 1;
        let req = crate::detect::DetectRequest {
            backend,
            kind,
            device,
            output,
            level_dbfs: level.0,
            ceiling_dbfs: self.s.ceiling_dbfs,
            max_level: self.s.max_level,
        };
        tracing::info!(
            target: "ac2d::audit",
            "loopback detection on output {} by {}",
            output + 1,
            client.0
        );
        let to = self.s.to_self.clone();
        std::thread::Builder::new()
            .name("ac2d-detect".into())
            .spawn(move || {
                let result = crate::detect::run(&req);
                let _ = to.send(ControlMsg::LoopbackDetected {
                    token,
                    result: Box::new(result),
                });
            })
            .map_err(|e| perr(ErrorCode::Internal, format!("cannot start detection: {e}")))?;
        Ok(token)
    }

    // -- jobs ------------------------------------------------------------------------------

    fn job_env(&self, rt: &Runtime) -> JobEnv {
        self.env_at(rt.epoch)
    }

    fn env_at(&self, epoch: SessionEpoch) -> JobEnv {
        JobEnv {
            ctx: self.s.ctx.clone(),
            endpoint: self.s.endpoint.clone(),
            incarnation: self.s.incarnation,
            epoch,
            seqs: Arc::clone(&self.seqs),
            interest: Arc::clone(&self.s.interest),
            fps: self.s.fps,
        }
    }

    /// The calibration and mic curve a job on `input` of the open session uses (Q7 §3).
    fn input_cal(&self, rt: &Runtime, input: u16) -> InputCal {
        calstore::resolve(self.store.state(), &self.cal, &rt.open.input_device, input)
    }

    /// Hands every running job its input's current calibration and mic curve.
    fn refresh_cal(&self) {
        let Some(rt) = self.session.as_ref() else {
            return;
        };
        for m in &self.store.state().measurements {
            let Some(job) = self.jobs.get(&m.id) else {
                continue;
            };
            let input = match &m.config.kind {
                MeasKind::Spectrum { config } => config.input,
                MeasKind::Rta { config } => config.input,
                MeasKind::Spl { config } => config.input,
                MeasKind::Transfer { config } => config.measurement_input,
            };
            job.send(JobCmd::Cal(Box::new(self.input_cal(rt, input))));
        }
    }

    /// Starts the job of `m` on the open session; returns its grid when it has one.
    fn start_job(&mut self, m: &Measurement) -> Result<Option<GridDef>, ProtoError> {
        if self.session.is_none() {
            return Ok(None);
        }
        let mut leq = matches!(m.config.kind, MeasKind::Spl { .. }).then(|| self.leq_setup(m.id));
        // A spectrum publishes display columns but captures every bin: `trace.capture`
        // looks the capture's grid up by id.
        if let (MeasKind::Spectrum { config }, Some(rt)) = (&m.config.kind, &self.session) {
            let g = jobs::spectrum::capture_grid(config, rt.sample_rate);
            self.register_grid(g);
        }
        let Some(rt) = self.session.as_ref() else {
            return Ok(None);
        };
        let fs = rt.sample_rate;
        let idx = |input: u16| {
            block_index(&rt.input_map, input).ok_or_else(|| {
                perr(
                    ErrorCode::Invalid,
                    format!("input {input} is not captured by the session"),
                )
            })
        };
        let inv = |e: String| perr(ErrorCode::Invalid, e);
        let (analysis, grid): (Box<dyn Analysis>, Option<GridDef>) = match &m.config.kind {
            MeasKind::Transfer { config } => {
                let d = m.delay.clone().unwrap_or(DelayState {
                    applied: Seconds(0.0),
                    applied_samples: 0.0,
                    tracking: false,
                    awaiting_pick: false,
                    last_finding: None,
                });
                let a = jobs::transfer::Transfer::new(
                    m.id,
                    config.clone(),
                    fs,
                    idx(config.reference_input)?,
                    idx(config.measurement_input)?,
                    d.applied_samples,
                    d.applied.0,
                    m.frozen,
                    m.config_rev,
                    d.tracking,
                    d.awaiting_pick,
                    rt.epoch,
                    self.s.to_self.clone(),
                    self.input_cal(rt, config.measurement_input)
                        .correction
                        .as_deref(),
                )
                .map_err(inv)?;
                (Box::new(a), static_grid(&m.config.kind))
            }
            MeasKind::Spectrum { config } => {
                let a = jobs::spectrum::Spectrum::new(
                    m.id,
                    config.clone(),
                    fs,
                    idx(config.input)?,
                    self.input_cal(rt, config.input),
                    m.frozen,
                    m.config_rev,
                )
                .map_err(inv)?;
                (Box::new(a), Some(jobs::spectrum::grid(config, fs)))
            }
            MeasKind::Rta { config } => {
                let bank = jobs::rta::bank(config, fs).map_err(inv)?;
                let g = jobs::rta::grid(config, &bank);
                let a = jobs::rta::Rta::new(
                    m.id,
                    config.clone(),
                    fs,
                    idx(config.input)?,
                    self.input_cal(rt, config.input),
                    m.frozen,
                    m.config_rev,
                )
                .map_err(inv)?;
                (Box::new(a), Some(g))
            }
            MeasKind::Spl { config } => {
                let (input, cal) = (idx(config.input)?, self.input_cal(rt, config.input));
                let config = config.clone();
                let leq = leq
                    .take()
                    .ok_or_else(|| perr(ErrorCode::Internal, "SPL meter without its log"))?;
                let a =
                    jobs::spl::Spl::new(m.id, config, fs, input, cal, m.frozen, m.config_rev, leq)
                        .map_err(inv)?;
                (Box::new(a), None)
            }
        };
        let fid = self.next_fanout_id;
        self.next_fanout_id += 1;
        let (handle, tx) = jobs::spawn(
            format!("ac2d-meas-{}", m.id.0),
            self.job_env(rt),
            fid,
            analysis,
        )
        .map_err(|e| perr(ErrorCode::Internal, format!("cannot start job: {e}")))?;
        rt.fanout.attach(fid, tx);
        if let Some(old) = self.jobs.insert(m.id, handle)
            && let Some(rt) = &self.session
        {
            rt.fanout.detach(old.fanout_id);
        }
        tracing::info!("measurement {} running", m.id.0);
        Ok(grid)
    }

    fn stop_job(&mut self, id: MeasId) {
        let orphaned: Vec<u64> = self
            .pending_finds
            .iter()
            .filter(|(_, p)| p.meas == id)
            .map(|(t, _)| *t)
            .collect();
        for t in orphaned {
            if let Some(p) = self.pending_finds.remove(&t) {
                let e = perr(
                    ErrorCode::Invalid,
                    "the measurement stopped during delay.find",
                );
                self.answer(&p.routing_id, &p.client, p.id, Err(e), Instant::now());
            }
        }
        if let Some(h) = self.jobs.remove(&id) {
            if let Some(rt) = &self.session {
                rt.fanout.detach(h.fanout_id);
            }
            drop(h);
        }
    }

    fn stop_all_jobs(&mut self) {
        let ids: Vec<MeasId> = self.jobs.keys().copied().collect();
        for id in ids {
            self.stop_job(id);
        }
        if let Some(h) = self.timing_job.take() {
            if let Some(rt) = &self.session {
                rt.fanout.detach(h.fanout_id);
            }
            drop(h);
        }
    }

    /// Starts the session input meters; the fan-out publishes them, they stop with it.
    fn start_session_levels(&self) {
        let Some(rt) = self.session.as_ref() else {
            return;
        };
        rt.fanout
            .start_levels(self.job_env(rt), rt.input_map.clone(), self.store.rev());
    }

    fn start_timing_job(&mut self) {
        let Some(rt) = self.session.as_ref() else {
            return;
        };
        let Some(lb) = rt.open.config.loopback else {
            return;
        };
        let (Some(history), Some(idx)) = (rt.history.clone(), block_index(&rt.input_map, lb.input))
        else {
            return;
        };
        let a = jobs::timing::Timing::new(
            rt.sample_rate,
            idx,
            history,
            self.s.to_self.clone(),
            rt.epoch,
            self.store.state().timing,
        );
        let fid = self.next_fanout_id;
        self.next_fanout_id += 1;
        match jobs::spawn("ac2d-timing".into(), self.job_env(rt), fid, Box::new(a)) {
            Ok((h, tx)) => {
                rt.fanout.attach(fid, tx);
                self.timing_job = Some(h);
            }
            Err(e) => tracing::error!("cannot start the timing monitor: {e}"),
        }
    }

    /// Applies `delay`. An explicit operator value drops the last finding: it no longer
    /// describes the applied delay, and a refusal must not keep showing as the reason there
    /// is no delay. The operator's insert or value resolves an ambiguous finding, so tracking
    /// resumes (decision 1c); a delay tracking moved changes neither.
    fn set_delay(
        &mut self,
        meas: MeasId,
        delay: Seconds,
        source: DelaySource,
    ) -> Result<ReplyBody, ProtoError> {
        self.transfer_delay(meas)?;
        if !(delay.0.is_finite() && delay.0.abs() <= MAX_DELAY_S) {
            return Err(perr(
                ErrorCode::Invalid,
                format!("delay must be finite and within ±{MAX_DELAY_S} s"),
            ));
        }
        let Some(fs) = self.session.as_ref().map(|r| f64::from(r.sample_rate)) else {
            return Err(perr(
                ErrorCode::Invalid,
                "no open session: the delay in samples depends on its sample rate",
            ));
        };
        let samples = delay_samples(delay.0, fs);
        let mut m = self.meas(meas)?.clone();
        let rev = Rev(self.store.rev().0 + 1);
        let operator = source != DelaySource::Tracking;
        if let Some(d) = &mut m.delay {
            d.applied = Seconds(samples / fs);
            d.applied_samples = samples;
            if source == DelaySource::Typed {
                d.last_finding = None;
            }
            if operator {
                d.awaiting_pick = false;
            }
        }
        m.config_rev = rev;
        if let Some(j) = self.jobs.get(&meas) {
            j.send(JobCmd::SetDelay {
                samples,
                seconds: samples / fs,
                rev,
                resume: operator,
            });
        }
        self.commit(Change::Measurement(Patch::Set(m.clone())));
        Ok(ReplyBody::Measurement(m))
    }

    fn start_find(
        &mut self,
        meas: MeasId,
        band: FinderBand,
        observation: Option<Seconds>,
    ) -> Result<u64, ProtoError> {
        self.transfer_delay(meas)?;
        let job = self.jobs.get(&meas).ok_or_else(|| {
            perr(
                ErrorCode::Invalid,
                "the measurement is not running; the finder needs live audio",
            )
        })?;
        let fs = self
            .session
            .as_ref()
            .map(|r| f64::from(r.sample_rate))
            .ok_or_else(|| perr(ErrorCode::Invalid, "no open session"))?;
        let band = conv::finder_band(band);
        check_find(band, observation.map(|o| o.0), fs)?;
        let token = self.next_token;
        self.next_token += 1;
        job.send(JobCmd::Find {
            token,
            band,
            observation: observation.map(|o| o.0),
        });
        Ok(token)
    }

    fn finish_find(&mut self, token: u64, result: Result<FinderResult, String>) {
        let Some(p) = self.pending_finds.remove(&token) else {
            return;
        };
        let fs = self.session.as_ref().map(|r| f64::from(r.sample_rate));
        let reply = match (result, fs) {
            (Err(e), _) => Err(perr(ErrorCode::Invalid, format!("delay finder: {e}"))),
            (Ok(_), None) => Err(perr(ErrorCode::Invalid, "the session closed")),
            (Ok(r), Some(fs)) => {
                let f = conv::delay_finding(&r, fs, WallNs(wall_ns()));
                match self.meas(p.meas).cloned() {
                    Err(e) => Err(e),
                    Ok(mut m) => {
                        if let Some(d) = &mut m.delay {
                            // The job paused tracking on an ambiguous result (1c).
                            d.awaiting_pick = matches!(f.outcome, DelayOutcome::Ambiguous { .. });
                            d.last_finding = Some(f.clone());
                        }
                        self.commit(Change::Measurement(Patch::Set(m)));
                        Ok(ReplyBody::DelayFinding(f))
                    }
                }
            }
        };
        self.answer(&p.routing_id, &p.client, p.id, reply, Instant::now());
    }

    fn tracked(&mut self, meas: MeasId, epoch: SessionEpoch, samples: i64) {
        let Some(fs) = self
            .session
            .as_ref()
            .filter(|r| r.epoch == epoch)
            .map(|r| f64::from(r.sample_rate))
        else {
            return;
        };
        let Ok(m) = self.meas(meas) else {
            return;
        };
        let Some(d) = &m.delay else {
            return;
        };
        if d.tracking
            && d.applied_samples.round() as i64 != samples
            && let Err(e) =
                self.set_delay(meas, Seconds(samples as f64 / fs), DelaySource::Tracking)
        {
            tracing::warn!("tracked delay not applied: {}", e.msg);
        }
    }

    // -- generator (Q6) --------------------------------------------------------------------

    fn audit(&self, g: &mut Generator, action: GenAction, client: Option<&ClientId>) {
        tracing::info!(
            target: "ac2d::audit",
            "generator {action:?} by {}",
            client.map_or("daemon", |c| c.0.as_str())
        );
        g.last_action = Some(GenAudit {
            action,
            client: client.cloned(),
            at: WallNs(wall_ns()),
        });
    }

    fn wire_lease(&self, token: LeaseToken) -> WireLease {
        WireLease {
            lease_token: token,
            expires_in_ms: u32::try_from(self.s.lease_expiry.as_millis()).unwrap_or(u32::MAX),
        }
    }

    fn lease_check(&self, client: &ClientId, token: LeaseToken) -> Result<(), ProtoError> {
        match &self.lease {
            Some(l) if l.token == token && l.owner == *client && l.deadline > Instant::now() => {
                Ok(())
            }
            _ => Err(lease_required()),
        }
    }

    /// Fades the output out (the gate makes the source fade on its own as well).
    fn stop_output(&mut self) {
        self.gate.close();
        if let Some(rt) = &self.session {
            rt.gen_handle.stop();
        }
        self.level = None;
        self.source = None;
    }

    fn check_lease(&mut self, now: Instant) {
        if self.lease.as_ref().is_some_and(|l| l.deadline <= now) {
            let owner = self.lease.take().map(|l| l.owner);
            tracing::warn!(
                "stimulus lease of {} expired: output muted",
                owner.as_ref().map_or("?", |o| o.0.as_str())
            );
            self.abort_sweep(SweepFailure::LeaseExpired, "the stimulus lease expired");
            self.stop_output();
            let mut g = self.store.state().generator.clone();
            g.owner = None;
            g.armed = false;
            g.firing = false;
            self.audit(&mut g, GenAction::Expiry, owner.as_ref());
            self.commit(Change::Generator(g));
        }
    }

    /// The output path muted itself on an expired gate: whatever the control side believed,
    /// the stimulus is silent, so the state must not say firing. Disarms with an expiry
    /// audit, and the lease goes with it.
    fn check_muted(&mut self) {
        if !self.gate.take_tripped() || !self.store.state().generator.firing {
            return;
        }
        let owner = self.lease.take().map(|l| l.owner);
        tracing::warn!(
            "stimulus lease of {} expired: output muted",
            owner.as_ref().map_or("?", |o| o.0.as_str())
        );
        self.abort_sweep(SweepFailure::LeaseExpired, "the stimulus lease expired");
        self.stop_output();
        let mut g = self.store.state().generator.clone();
        g.owner = None;
        g.armed = false;
        g.firing = false;
        self.audit(&mut g, GenAction::Expiry, owner.as_ref());
        self.commit(Change::Generator(g));
    }

    fn gen_acquire(&mut self, client: &ClientId, force: bool) -> Result<ReplyBody, ProtoError> {
        self.check_lease(Instant::now());
        let mut g = self.store.state().generator.clone();
        let action = match &self.lease {
            Some(l) if l.owner != *client => {
                if !force {
                    return Err(perr_detail(
                        ErrorCode::LeaseHeld,
                        format!("{} holds the stimulus lease", l.owner.0),
                        ErrorDetail::LeaseHeld {
                            owner: l.owner.clone(),
                        },
                    ));
                }
                // Takeover stops and disarms first; the new owner arms and fires explicitly.
                self.abort_sweep(
                    SweepFailure::Stopped,
                    "another client took the stimulus over",
                );
                self.stop_output();
                g.armed = false;
                g.firing = false;
                GenAction::Force
            }
            _ => GenAction::Acquire,
        };
        let token = LeaseToken(random_u128());
        let deadline = Instant::now() + self.s.lease_expiry;
        self.lease = Some(Lease {
            token,
            owner: client.clone(),
            deadline,
        });
        if g.firing {
            self.gate.open_until(deadline);
        }
        g.owner = Some(client.clone());
        self.audit(&mut g, action, Some(client));
        self.commit(Change::Generator(g));
        Ok(ReplyBody::Lease(self.wire_lease(token)))
    }

    fn gen_set(
        &mut self,
        client: &ClientId,
        token: LeaseToken,
        desired: GeneratorDesired,
    ) -> Result<ReplyBody, ProtoError> {
        self.lease_check(client, token)?;
        let st = &desired.settings;
        if desired.firing && !desired.armed {
            return Err(perr(ErrorCode::Refused, "firing requires armed"));
        }
        if !st.level.0.is_finite() {
            return Err(perr(ErrorCode::Invalid, "level must be finite"));
        }
        if st.level.0 > self.s.ceiling_dbfs {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "{:.1} dBFS is above the global maximum {:.1} dBFS",
                    st.level.0, self.s.ceiling_dbfs
                ),
            ));
        }
        for (i, o) in st.outputs.iter().enumerate() {
            if st.outputs[..i].contains(o) {
                return Err(perr(ErrorCode::Invalid, format!("output {o} listed twice")));
            }
        }
        if desired.armed && st.outputs.is_empty() {
            return Err(perr(ErrorCode::Invalid, "no output channels"));
        }
        let signal =
            conv::signal(st.signal).ok_or_else(|| perr(ErrorCode::Invalid, "invalid signal"))?;
        if let Some(rt) = &self.session
            && let Some(o) = st.outputs.iter().find(|o| **o >= rt.output_channels)
        {
            return Err(perr(
                ErrorCode::Invalid,
                format!("output {o} is not an output of the session"),
            ));
        }
        if desired.firing && self.session.is_none() {
            return Err(perr(ErrorCode::Refused, "no open session to emit on"));
        }
        if desired.firing && self.detecting.is_some() {
            return Err(perr(
                ErrorCode::Refused,
                "a loopback detection is playing its burst; fire once it is done",
            ));
        }
        if self
            .sweep
            .as_ref()
            .is_some_and(|s| matches!(s.run.status, SweepStatus::Playing { .. }))
        {
            return Err(perr(
                ErrorCode::Refused,
                "a sweep is playing: stop it (gen.stop) or let it finish",
            ));
        }

        let deadline = Instant::now() + self.s.lease_expiry;
        if let Some(l) = &mut self.lease {
            l.deadline = deadline;
        }
        let prev = self.store.state().generator.clone();

        // The stream carries every session output: arming routes the generator (and
        // connects the chosen outputs) without reopening it, so the session, its jobs and
        // every port connection stay as they are.
        if desired.armed
            && let Some(rt) = self.session.as_mut()
        {
            rt.set_routes(&st.outputs)?;
        }

        if desired.firing {
            let key = SourceKey {
                signal: st.signal,
                band: st.band,
            };
            let rt = self
                .session
                .as_mut()
                .ok_or_else(|| perr(ErrorCode::Refused, "no session"))?;
            if self.source != Some(key) || self.level.is_none() {
                let g = CoreGenerator::new(&GeneratorConfig {
                    signal,
                    sample_rate: f64::from(rt.sample_rate),
                    seed: random_u64(),
                    band: conv::band_limit(st.band),
                    level_dbfs: st.level.0,
                    ceiling_dbfs: self.s.ceiling_dbfs,
                })
                .map_err(gen_err)?;
                let peak = dbfs_to_rms(st.level.0) * g.crest_factor();
                if peak > f64::from(self.s.max_level.linear()) * (1.0 + 1e-9) {
                    return Err(perr(
                        ErrorCode::Refused,
                        "the signal's peak would exceed the output limit",
                    ));
                }
                let lc = g.level_control();
                // A fresh gate state for the new source; the old one fades on its own.
                self.gate.open_until(deadline);
                rt.gen_handle
                    .set_source(Box::new(LeasedSource::new(
                        Box::new(g),
                        Arc::clone(&self.gate),
                    )))
                    .map_err(|e| perr(ErrorCode::Internal, e.to_string()))?;
                self.level = Some(lc);
                self.source = Some(key);
            } else if let Some(lc) = &self.level {
                lc.set_level_dbfs(st.level.0).map_err(gen_err)?;
                self.gate.open_until(deadline);
            }
            rt.gen_handle.set_gain(Gain::UNITY);
            rt.gen_handle.start();
        } else {
            self.stop_output();
        }

        let mut g = self.store.state().generator.clone();
        let action = if desired.firing && !prev.firing {
            GenAction::Fire
        } else if desired.armed && !prev.armed {
            GenAction::Arm
        } else {
            GenAction::Set
        };
        g.armed = desired.armed;
        g.firing = desired.firing;
        g.settings = Some(desired.settings.clone());
        self.audit(&mut g, action, Some(client));
        self.commit(Change::Generator(g.clone()));
        Ok(ReplyBody::Generator(g))
    }
}
