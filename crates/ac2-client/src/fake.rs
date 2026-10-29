//! An in-process fake daemon for tests (feature `test-support`).
//!
//! It speaks the real wire protocol over real sockets (ROUTER + XPUB on `tcp://127.0.0.1`),
//! built only from `ac2-proto` and `ac2-zmq`: hello, request-id dedup, `expect_rev`,
//! serial commits with events on `evt`, a replay buffer with eviction, keepalives,
//! `grid.get`, and the stimulus lease with expiry. Hooks let a test drop events, drop one
//! reply, pause keepalives, restart the "daemon" and publish arbitrary frames.
//!
//! It never produces audio; generator state is bookkeeping only.

#![allow(
    clippy::unwrap_used,
    reason = "test support: a poisoned lock is a test failure"
)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::frame::{DaemonHealth, GenSummary, KaMeta, LoudnessSource, ProtectionFlags};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{
    Change, Command, ErrorCode, ErrorDetail, Event, Frame, FrameData, FrameStamp, GridDef, GridId,
    Patch, ProtoError, Reply, ReplyBody, StateSnapshot, decode_request, encode_event_message,
    encode_frame, encode_reply, peek_envelope,
};
use ac2_zmq::{Context, PollItem, Socket, SocketType, poll};

use crate::endpoint::Endpoints;

#[path = "fake_cal.rs"]
mod cal;
#[path = "fake_files.rs"]
mod files;
#[path = "fake_generator.rs"]
mod generator;
#[path = "fake_levels.rs"]
pub mod levels;
#[path = "fake_meas.rs"]
mod meas;
#[path = "fake_session.rs"]
mod session;
#[path = "fake_spl.rs"]
mod spl;
#[path = "fake_sweeps.rs"]
mod sweeps;
#[path = "fake_traces.rs"]
mod traces;
use crate::io::wall_ns;
use crate::mirror::apply_change;

/// What the fake's `delay.find` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FakeFinding {
    /// First arrival 12.5 ms, strongest 12.7 ms.
    #[default]
    Accepted,
    /// Three ranked arrivals (12.5, 12.7, 13.4 ms); the strongest is 12.7 ms.
    Ambiguous,
    /// Refused: low PSR and low band SNR.
    NoEstimate,
}

/// Fake daemon settings.
#[derive(Debug, Clone)]
pub struct FakeOptions {
    /// Keepalive period (the real daemon: 250 ms).
    pub ka_interval: Duration,
    /// Lease expiry after the last refresh (the real daemon: 1.5 s).
    pub lease_expiry: Duration,
    /// `welcome.server`.
    pub server: String,
    /// `welcome.rig_id`; kept across [`Shared::restart`], as a daemon keeps its file. Two
    /// fakes with different ids are two rigs.
    pub rig_id: RigId,
    /// `welcome.name`.
    pub name: String,
    /// Added to the daemon's wall clock (tests the client's offset estimate).
    pub clock_skew_ns: i64,
    /// Session directory for `file.*`; `None` answers them `unsupported`.
    pub session_dir: Option<std::path::PathBuf>,
}

impl Default for FakeOptions {
    fn default() -> Self {
        Self {
            ka_interval: Duration::from_millis(100),
            lease_expiry: Duration::from_millis(1500),
            server: "ac2d 0.0.0 (build fake)".into(),
            rig_id: RigId(0xfa4e),
            name: "fake rig".into(),
            clock_skew_ns: 0,
            session_dir: None,
        }
    }
}

#[derive(Debug, Clone)]
struct LeaseSlot {
    token: LeaseToken,
    owner: ClientId,
    deadline: Instant,
}

/// Mutable daemon state, shared between the test and the fake's thread.
#[derive(Debug)]
pub struct Shared {
    /// Incarnation.
    pub incarnation: DaemonIncarnation,
    /// Rev.
    pub rev: Rev,
    /// State.
    pub state: State,
    /// Every committed event of this incarnation, in order.
    pub replay: Vec<Event>,
    /// `state.since(r)` with r below this answers `resync_required`.
    pub replay_floor: Rev,
    /// Publish committed events on `evt`.
    pub publish_events: bool,
    /// A command is editing the measurements and traces: its edits are unsaved changes.
    /// Off for a load or new replacing them, and for changes a test commits directly.
    pub(crate) marking: bool,
    /// Keepalives paused.
    pub ka_paused: bool,
    /// The health figures keepalives carry.
    pub health: DaemonHealth,
    /// Ctrl requests are read and ignored (no execution, no reply).
    pub mute: bool,
    /// Ops whose next reply is swallowed after executing (the client must retry the id).
    pub drop_reply_once: HashSet<&'static str>,
    /// Executions per op (retries answered from the dedup store do not count).
    pub executions: HashMap<&'static str, u32>,
    /// Every request received, as (op, id), including retries.
    pub requests: Vec<(&'static str, u64)>,
    /// Grids known to `grid.get`.
    pub grids: HashMap<GridId, GridDef>,
    /// Backends and their devices (`session.devices`).
    pub backends: Vec<BackendInfo>,
    /// The device `session.preview` last opened; `None` once stopped.
    pub preview: Option<(BackendKind, DeviceId)>,
    /// What the preview declared, for its meter frames; `None` once stopped.
    pub preview_decl: Option<levels::PreviewDecl>,
    /// The sample peak each input reads in meter frames; absent: the default.
    pub input_peak: HashMap<u16, f32>,
    /// The overs count and CLIP each input reads in meter frames; absent: none.
    pub input_overs: HashMap<u16, (u32, bool)>,
    /// The max holds of the programme meters, by input.
    pub levels_max: HashMap<u16, f32>,
    /// When each loudness source started and was last reset, by meter.
    pub loudness_starts: HashMap<(levels::Meter, LoudnessSource), levels::SourceStart>,
    /// Sequence of the meter frames.
    pub meter_seq: u64,
    /// Publish meter frames (`session/levels`, `session/preview`, the goniometers) while a
    /// session or a preview is open. Off unless a test asks: a UI test that opens a session
    /// sees no meter bars it did not publish itself.
    pub meters: bool,
    /// What `session.detect_loopback` answers (output, level and device are the request's).
    pub detection: LoopbackDetection,
    /// Lease refreshes accepted.
    pub refreshes: u32,
    /// Lease expiries.
    pub expiries: u32,
    /// What `delay.find` answers.
    pub finding: FakeFinding,
    /// Band and observation of the last `delay.find`.
    pub last_find: Option<(FinderBand, Option<Seconds>)>,
    /// Per-second log rows `spl.log_get` serves, per SPL meter (a test fills them).
    pub spl_rows: HashMap<MeasId, Vec<SplLogRow>>,
    /// The rows `spl.log_new` ended last, per SPL meter (`spl.log_get` of the previous log).
    pub spl_prev_rows: HashMap<MeasId, Vec<SplLogRow>>,
    /// Per-second band rows of each SPL meter's band log (a test fills them): what
    /// `spl.band_log_get` and a `spl.band_transfer` span read; the fake runs no band meter.
    pub band_rows: HashMap<MeasId, Vec<ac2_traces::band_log::BandLogRow>>,
    traces: traces::FakeTraces,
    lease: Option<LeaseSlot>,
    dedup: HashMap<(Vec<u8>, u64), Vec<u8>>,
    outbox: Vec<Vec<Vec<u8>>>,
    next_id: u32,
    next_token: u128,
    ka_seq: u64,
    opts: FakeOptions,
}

/// A minimal empty state.
pub fn empty_state() -> State {
    State {
        session: Session {
            epoch: SessionEpoch(1),
            open: None,
            stopped: None,
        },
        measurements: vec![],
        traces: vec![],
        generator: Generator {
            owner: None,
            armed: false,
            firing: false,
            settings: None,
            ceiling: Dbfs(-6.0),
            ceiling_bound: Dbfs(-6.0),
            last_action: None,
        },
        calibrations: vec![],
        line_calibrations: vec![],
        mics: vec![],
        inputs: vec![],
        pairs: vec![],
        outputs: vec![],
        spl_logs: vec![],
        timing: TimingStatus {
            epoch: 0,
            state: TimingState::NoStimulus,
            last_lock: None,
            drift: None,
            internal_reference: false,
        },
        sweep: None,
        autosave: Autosave {
            state: AutosaveState::Off,
            saved_at: None,
            log_failures: Vec::new(),
        },
        recording: None,
        working: WorkingSession {
            origin: None,
            dirty: false,
            saved_at: None,
            recover: None,
        },
        rig: Rig::default(),
        plans: vec![],
        walks: vec![],
        walk_plans: vec![],
    }
}

/// The fake's backends: the simulated rig (4 in, 2 out, inputs named like `ac2d`'s rig) and
/// a JACK backend whose server is not running.
pub fn fake_backends() -> Vec<BackendInfo> {
    let names = |n: &[&str]| Some(n.iter().map(|s| (*s).to_owned()).collect());
    let dir = |ch, channel_names| DirectionInfo {
        max_channels: ch,
        rates_hz: vec![RangeU32 {
            min: 48_000,
            max: 48_000,
        }],
        buffer_frames: Some(RangeU32 { min: 256, max: 256 }),
        default_rate_hz: Some(48_000),
        default_buffer_frames: Some(256),
        channel_names,
        system_default: true,
        spare_channels: 0,
    };
    vec![
        BackendInfo {
            kind: BackendKind::Fake,
            description: "Simulated rig (no audio): out 1 returns on in 1 (loop) and in 2 (room)"
                .into(),
            availability: Availability::Available,
            devices: vec![DeviceInfo {
                backend: BackendKind::Fake,
                host: "fake".into(),
                id: DeviceId("fake:loop".into()),
                name: "Fake loopback".into(),
                input: Some(dir(
                    4,
                    names(&["Loop return", "Room mic", "Line 3", "Line 4"]),
                )),
                output: Some(dir(2, names(&["Out 1 (speaker + loop)", "Out 2"]))),
                duplex_clock: ClockRelation::SingleCallback,
                index: IndexExactness::Exact,
                clock_sources: vec![],
                notes: vec![],
            }],
        },
        BackendInfo {
            kind: BackendKind::Jack,
            description: "JACK audio server".into(),
            availability: Availability::Unavailable {
                reason: "JACK server not running".into(),
            },
            devices: vec![],
        },
    ]
}

/// What the fake's `session.detect_loopback` answers by default: input 1 is the loopback.
pub fn fake_detection() -> LoopbackDetection {
    let c = |input, samples: i64, correlation, gain: Option<f64>| LoopbackCandidate {
        input,
        delay: Seconds(samples as f64 / 48_000.0),
        delay_samples: Samples(samples),
        correlation,
        gain: gain.map(Db),
    };
    LoopbackDetection {
        backend: BackendKind::Fake,
        input_device: DeviceId("fake:loop".into()),
        output_device: DeviceId("fake:loop".into()),
        output: 0,
        level: Dbfs(-30.0),
        ranked: vec![
            c(0, 32, 0.9998, Some(0.0)),
            c(1, 272, 0.9991, Some(-6.0)),
            c(2, 0, 0.0, None),
            c(3, 0, 0.0, None),
        ],
        loopback: Some(0),
        clock: ClockRelation::SingleCallback,
    }
}

/// A stale `base`: someone changed the entity since; the current rev rides along.
pub(super) fn conflict(rev: Rev) -> ProtoError {
    ProtoError {
        code: ErrorCode::Conflict,
        msg: "changed since; re-read it".into(),
        detail: Some(ErrorDetail::Conflict { rev }),
    }
}

fn err(code: ErrorCode, msg: impl Into<String>) -> ProtoError {
    ProtoError {
        code,
        msg: msg.into(),
        detail: None,
    }
}

impl Shared {
    fn new(opts: FakeOptions, incarnation: u64) -> Self {
        Self {
            incarnation: DaemonIncarnation(incarnation),
            rev: Rev(0),
            state: empty_state(),
            replay: vec![],
            replay_floor: Rev(0),
            publish_events: true,
            marking: false,
            ka_paused: false,
            health: DaemonHealth {
                host: "fake".into(),
                cpu: None,
                audio: None,
            },
            mute: false,
            drop_reply_once: HashSet::new(),
            executions: HashMap::new(),
            requests: vec![],
            grids: HashMap::new(),
            backends: fake_backends(),
            preview: None,
            preview_decl: None,
            input_peak: HashMap::new(),
            input_overs: HashMap::new(),
            levels_max: HashMap::new(),
            loudness_starts: HashMap::new(),
            meter_seq: 0,
            meters: false,
            detection: fake_detection(),
            refreshes: 0,
            expiries: 0,
            finding: FakeFinding::default(),
            last_find: None,
            spl_rows: HashMap::new(),
            spl_prev_rows: HashMap::new(),
            band_rows: HashMap::new(),
            traces: traces::FakeTraces::default(),
            lease: None,
            dedup: HashMap::new(),
            outbox: vec![],
            next_id: 1,
            next_token: 0x1000,
            ka_seq: 0,
            opts,
        }
    }

    fn now_ns(&self) -> u64 {
        (wall_ns() + i128::from(self.opts.clock_skew_ns)).max(0) as u64
    }

    /// A frame stamp of the current incarnation and epoch, captured now.
    pub fn stamp(&self, seq: u64, grid_id: Option<GridId>) -> FrameStamp {
        FrameStamp {
            seq,
            audio_sample: SampleIndex(seq * 4800),
            session_epoch: self.state.session.epoch,
            daemon_incarnation: self.incarnation,
            config_rev: self.rev,
            config_applied_at: SampleIndex(0),
            capture_wall_ns: WallNs(self.now_ns()),
            grid_id,
            protection: ProtectionFlags::NONE,
        }
    }

    /// Commits a change: bumps rev, applies it, records it for replay and (unless
    /// `publish_events` is off) publishes it on `evt`.
    pub fn commit(&mut self, change: Change) -> Rev {
        let edits = matches!(change, Change::Measurement(_) | Change::Trace(_));
        let rev = self.commit_one(change);
        // The fake's unsaved changes: any edit by a command since the last save or load.
        if edits && self.marking && !self.state.working.dirty {
            let mut w = self.state.working.clone();
            w.dirty = true;
            return self.commit_one(Change::Working(w));
        }
        rev
    }

    fn commit_one(&mut self, change: Change) -> Rev {
        self.rev = Rev(self.rev.0 + 1);
        apply_change(&mut self.state, change.clone());
        let ev = Event {
            rev: self.rev,
            change,
        };
        if self.publish_events
            && let Ok(parts) = encode_event_message(&ev)
        {
            self.outbox.push(parts);
        }
        self.replay.push(ev);
        self.rev
    }

    /// Publishes a frame.
    pub fn publish(&mut self, frame: &Frame) {
        if let Ok(parts) = encode_frame(frame) {
            self.outbox.push(parts);
        }
    }

    /// Simulates a restart: new incarnation, fresh state, empty replay and dedup, no lease.
    pub fn restart(&mut self, incarnation: u64) {
        let opts = self.opts.clone();
        let backends = std::mem::take(&mut self.backends);
        *self = Self::new(opts, incarnation);
        self.backends = backends;
    }

    /// Evicts the whole replay buffer: every `state.since` below the current rev answers
    /// `resync_required`.
    pub fn evict_replay(&mut self) {
        self.replay_floor = self.rev;
    }

    fn generator_changed(&mut self, action: GenAction, client: Option<ClientId>) {
        let at = WallNs(self.now_ns());
        let mut g = self.state.generator.clone();
        g.last_action = Some(GenAudit { action, client, at });
        self.commit(Change::Generator(g));
    }

    fn check_lease(&self, token: LeaseToken) -> Result<(), ProtoError> {
        match &self.lease {
            Some(l) if l.token == token && l.deadline > Instant::now() => Ok(()),
            _ => Err(err(ErrorCode::LeaseRequired, "lease not held")),
        }
    }

    fn refresh(&mut self) {
        if let Some(l) = self.lease.as_mut() {
            l.deadline = Instant::now() + self.opts.lease_expiry;
        }
    }

    fn expire_lease(&mut self) {
        if self
            .lease
            .as_ref()
            .is_some_and(|l| l.deadline <= Instant::now())
        {
            let owner = self.lease.take().map(|l| l.owner);
            self.expiries += 1;
            self.state.generator.owner = None;
            self.state.generator.armed = false;
            self.state.generator.firing = false;
            self.generator_changed(GenAction::Expiry, owner);
        }
    }

    fn meas(&self, id: MeasId) -> Result<Measurement, ProtoError> {
        self.state
            .measurements
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no measurement {id}")))
    }

    /// Rig commands: settings are kept (rev-guarded); nothing is sent.
    fn rig_cmd(&mut self, c: Command) -> Result<ReplyBody, ProtoError> {
        let mut rig = self.state.rig.clone();
        let base = match &c {
            Command::OscTarget { base, .. }
            | Command::ArtnetTarget { base, .. }
            | Command::ArtnetFade { base, .. }
            | Command::RigMonitorStart { base, .. } => *base,
            _ => rig.rev,
        };
        if base != rig.rev {
            return Err(conflict(rig.rev));
        }
        match c {
            Command::OscTarget { addr, .. } => rig.osc_target = addr,
            Command::ArtnetTarget { target, .. } => rig.artnet_target = target,
            Command::ArtnetFade { seconds, .. } => {
                rig.artnet_fade = ac2_proto::model::ArtnetFade::new(seconds.0)
                    .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
            }
            Command::RigMonitorStart { on, .. } => rig.monitor_at_start = on,
            Command::OscTest { .. } => {
                if rig.osc_target.is_none() {
                    return Err(err(ErrorCode::Invalid, "no OSC target"));
                }
                rig.osc.sent += 1;
            }
            Command::ArtnetTest { .. } => {
                if rig.artnet_target.is_none() {
                    return Err(err(ErrorCode::Invalid, "no Art-Net target"));
                }
                rig.artnet.sent += 1;
            }
            _ => unreachable!("rig_cmd is called with rig commands only"),
        }
        rig.rev = Rev(self.rev.0 + 1);
        self.commit(Change::Rig(rig));
        Ok(ReplyBody::Ack { rev: self.rev })
    }

    fn put_meas(&mut self, m: Measurement) -> ReplyBody {
        self.commit(Change::Measurement(Patch::Set(m.clone())));
        self.spl_log_for(&m);
        ReplyBody::Measurement(m)
    }

    fn execute(&mut self, client: &ClientId, cmd: Command) -> Result<ReplyBody, ProtoError> {
        use Command as C;
        Ok(match cmd {
            c @ (C::Hello { .. }
            | C::SessionDevices
            | C::SessionPreview { .. }
            | C::SessionPreviewStop
            | C::SessionDetectLoopback { .. }
            | C::SessionOpen { .. }
            | C::SessionClose
            | C::SessionStatus
            | C::SessionOutputs { .. }
            | C::ServerInfo
            | C::ServerAuthorize { .. }
            | C::ServerRevoke { .. }
            | C::SessionReplay { .. }
            | C::SessionInputs { .. }
            | C::SessionControlPanel { .. }) => self.session_cmd(client, c)?,
            c @ (C::GenAcquire { .. }
            | C::GenSet { .. }
            | C::GenRefresh { .. }
            | C::GenRelease { .. }
            | C::GenStop
            | C::GenCeiling { .. }) => self.generator_cmd(client, c)?,
            c @ (C::OscTarget { .. }
            | C::OscTest { .. }
            | C::ArtnetTarget { .. }
            | C::ArtnetFade { .. }
            | C::ArtnetTest { .. }
            | C::RigMonitorStart { .. }) => self.rig_cmd(c)?,
            c @ (C::LevelsSet { .. } | C::LevelsReset { .. }) => self.levels_cmd(c)?,
            c @ (C::MeasCreate { .. }
            | C::MeasUpdate { .. }
            | C::MeasColour { .. }
            | C::MeasLayout { .. }
            | C::MeasCompare { .. }
            | C::MeasOffset { .. }
            | C::MeasDelete { .. }
            | C::MeasStart { .. }
            | C::MeasStop { .. }
            | C::MeasReset { .. }
            | C::DelayFind { .. }
            | C::DelayInsert { .. }
            | C::DelaySet { .. }
            | C::DelayNudge { .. }
            | C::DelayTrack { .. }) => self.meas_cmd(c)?,
            C::TraceCapture { meas, name, slot } => self.trace_capture(meas, name, slot)?,
            C::TraceList => ReplyBody::Traces(self.state.traces.clone()),
            C::TraceGet { trace } => self.trace_get(trace)?,
            C::SweepRun {
                lease_token,
                meas,
                name,
            } => {
                self.expire_lease();
                self.check_lease(lease_token)?;
                self.refresh();
                self.sweep_run(client, meas, name)?
            }
            C::TraceUpdate { trace, edit } => self.trace_update(trace, edit)?,
            C::TraceDelete { trace } => self.trace_delete(trace)?,
            C::TraceAverage {
                traces,
                method,
                reference,
                name,
            } => self.trace_average(traces, method, reference, name)?,
            C::TraceImport {
                file_name,
                format,
                role,
                ir,
                content,
                owner,
            } => self.trace_import(file_name, format, role, ir, &content.0, owner)?,
            C::TraceTarget { curve, name, owner } => self.trace_target(curve, name, owner)?,
            C::TraceExport { trace, format } => self.trace_export(trace, format)?,
            C::TraceMicCurve { trace, curve } => self.trace_mic_curve(trace, curve)?,
            C::FileSave { session, .. } => self.file_save(&session)?,
            C::FileLoad { session, discard } => {
                self.guard_unsaved(discard)?;
                self.file_load(client, &session)?
            }
            C::FileNew { discard } => self.file_new(discard)?,
            C::FileRecover
            | C::FileSalvage { .. }
            | C::FileUploadBegin { .. }
            | C::FileUploadChunk { .. }
            | C::FileImport { .. }
            | C::FileDownloadBegin { .. }
            | C::FileDownloadChunk { .. }
            | C::FileDownloadEnd { .. } => {
                return Err(err(
                    ErrorCode::Unsupported,
                    "the fake keeps sessions by name and path only",
                ));
            }
            C::MeasEq { .. } => {
                // EQ sets are evaluated by the daemon (ac2_core::eq); the fake has no DSP.
                return Err(err(
                    ErrorCode::Unsupported,
                    "the fake evaluates no EQ: run ac2d on the fake backend",
                ));
            }
            C::PlanSave { .. }
            | C::PlanList
            | C::PlanDelete { .. }
            | C::WalkNew { .. }
            | C::WalkUpdate { .. }
            | C::WalkGoto { .. }
            | C::WalkCapture { .. }
            | C::WalkSkip { .. }
            | C::WalkRedo { .. }
            | C::WalkSwap { .. }
            | C::WalkDelete { .. } => {
                // Plans are validated and walks captured by the daemon (ac2-room, its
                // measurements' averages).
                return Err(err(
                    ErrorCode::Unsupported,
                    "the fake keeps no plans or walks: run ac2d on the fake backend",
                ));
            }
            C::FileList => self.file_list()?,
            C::RecStart { request } => self.rec_start(client, request)?,
            C::RecStop => self.rec_stop()?,
            C::RecList => ReplyBody::Recordings(Vec::new()),
            c @ (C::CalSpl { .. }
            | C::CalSplElectrical { .. }
            | C::CalCurveImport { .. }
            | C::CalCurveRename { .. }
            | C::CalCurveDelete { .. }
            | C::CalLine { .. }
            | C::CalList
            | C::CalDelete { .. }) => self.cal_cmd(c)?,
            c @ (C::SplLogGet { .. }
            | C::SplHistoryGet { .. }
            | C::SplBandLogGet { .. }
            | C::SplLogNew { .. }
            | C::SplBandTransfer { .. }) => self.spl_cmd(c)?,
            C::StateSnapshot => ReplyBody::Snapshot(Box::new(StateSnapshot {
                state: self.state.clone(),
                rev: self.rev,
                daemon_incarnation: self.incarnation,
                session_epoch: self.state.session.epoch,
            })),
            C::StateSince { rev } => {
                if rev < self.replay_floor {
                    return Err(ProtoError {
                        code: ErrorCode::ResyncRequired,
                        msg: "gap expired".into(),
                        detail: Some(ErrorDetail::Resync {
                            oldest: self.replay_floor,
                        }),
                    });
                }
                ReplyBody::Events(
                    self.replay
                        .iter()
                        .filter(|e| e.rev > rev)
                        .cloned()
                        .collect(),
                )
            }
            C::GridGet { grid_id } => ReplyBody::Grid(
                self.grids
                    .get(&grid_id)
                    .cloned()
                    .ok_or_else(|| err(ErrorCode::NotFound, format!("no grid {grid_id}")))?,
            ),
        })
    }
}

/// The running fake daemon. Dropping it stops the thread.
#[derive(Debug)]
pub struct FakeDaemon {
    endpoints: Endpoints,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeDaemon {
    /// Starts a fake daemon on loopback TCP with `opts`.
    pub fn start(opts: FakeOptions) -> Result<Self, ac2_zmq::Error> {
        let ctx = Context::new()?;
        let router = ctx.socket(SocketType::Router)?;
        router.set_router_mandatory(true)?;
        router.bind("tcp://127.0.0.1:*")?;
        let xpub = ctx.socket(SocketType::XPub)?;
        xpub.set_xpub_verbose(true)?;
        xpub.bind("tcp://127.0.0.1:*")?;
        let endpoints = Endpoints {
            ctrl: router.last_endpoint()?,
            data: xpub.last_endpoint()?,
        };
        let shared = Arc::new(Mutex::new(Shared::new(opts.clone(), 0x5eed_0001)));
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, st2) = (shared.clone(), stop.clone());
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("fake-ac2d".into())
            .spawn(move || {
                let _ = ready_tx.send(());
                run(router, xpub, s2, st2, opts.ka_interval);
            })
            .map_err(|e| ac2_zmq::Error::Spawn(e.to_string()))?;
        let _ = ready_rx.recv();
        Ok(Self {
            endpoints,
            shared,
            stop,
            thread: Some(thread),
        })
    }

    /// Endpoints to connect a client to.
    pub fn endpoints(&self) -> Endpoints {
        self.endpoints.clone()
    }

    /// Locks the daemon state.
    pub fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap()
    }

    /// Executions of `op` so far.
    pub fn executions(&self, op: &str) -> u32 {
        self.lock().executions.get(op).copied().unwrap_or(0)
    }

    /// Requests of `op` received so far, retries included.
    pub fn received(&self, op: &str) -> usize {
        self.lock()
            .requests
            .iter()
            .filter(|(o, _)| *o == op)
            .count()
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(
    router: Socket,
    xpub: Socket,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    ka_interval: Duration,
) {
    let mut next_ka = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let mut items = [PollItem::readable(&router), PollItem::readable(&xpub)];
        if poll(&mut items, Some(Duration::from_millis(5))).is_err() {
            return;
        }
        while let Ok(Some(_)) = xpub.try_recv() {}
        while let Ok(Some(m)) = router.try_recv() {
            let frames = m.into_frames();
            let [rid, body] = frames.as_slice() else {
                continue;
            };
            let reply = handle(&shared, rid, body);
            if let Some(reply) = reply {
                let _ = router.send(&[rid.as_slice(), reply.as_slice()]);
            }
        }
        let mut s = shared.lock().unwrap();
        s.expire_lease();
        if Instant::now() >= next_ka {
            next_ka = Instant::now() + ka_interval;
            if s.meters {
                s.publish_levels();
            }
            if !s.ka_paused {
                s.ka_seq += 1;
                let mut stamp = s.stamp(s.ka_seq, None);
                stamp.audio_sample = SampleIndex(0);
                let g = &s.state.generator;
                let ka = Frame {
                    data: FrameData::Ka(KaMeta {
                        rev: s.rev,
                        daemon_wall_ns: stamp.capture_wall_ns,
                        timing: s.state.timing.state,
                        generator: GenSummary {
                            owner: g.owner.clone(),
                            armed: g.armed,
                            firing: g.firing,
                        },
                        health: s.health.clone(),
                    }),
                    stamp,
                };
                s.publish(&ka);
            }
        }
        for parts in std::mem::take(&mut s.outbox) {
            let _ = xpub.send(&parts);
        }
    }
}

fn handle(shared: &Mutex<Shared>, rid: &[u8], body: &[u8]) -> Option<Vec<u8>> {
    let mut s = shared.lock().unwrap();
    if s.mute {
        return None;
    }
    let req = match decode_request(body) {
        Ok(r) => r,
        Err(_) => {
            let env = peek_envelope(body).ok()?;
            let id = env.id.unwrap_or(RequestId(0));
            return encode_reply(&Reply::version_refusal(id, env.v)).ok();
        }
    };
    let op = req.cmd.name();
    s.requests.push((op, req.id.0));
    let key = (rid.to_vec(), req.id.0);
    if let Some(stored) = s.dedup.get(&key) {
        return Some(stored.clone());
    }
    // Like a real restart (a new ROUTER assigns new routing ids), a new incarnation binds a
    // new identity to the same connection.
    let client = ClientId(format!(
        "local-{:04x}-{}",
        s.incarnation.0 & 0xffff,
        rid.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ));
    let result = match req.expect_rev {
        Some(r) if req.cmd.is_mutation() && s.rev > r => Err(ProtoError {
            code: ErrorCode::Conflict,
            msg: format!("state is at rev {}", s.rev),
            detail: Some(ErrorDetail::Conflict { rev: s.rev }),
        }),
        _ => {
            *s.executions.entry(op).or_insert(0) += 1;
            s.marking = true;
            let r = s.execute(&client, req.cmd);
            s.marking = false;
            r
        }
    };
    let bytes = encode_reply(&Reply::new(req.id, result)).ok()?;
    s.dedup.insert(key, bytes.clone());
    if s.drop_reply_once.remove(op) {
        return None;
    }
    Some(bytes)
}
