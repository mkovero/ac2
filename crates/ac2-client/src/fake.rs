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

use ac2_proto::frame::{GenSummary, KaMeta, ProtectionFlags};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{
    Change, Command, ErrorCode, ErrorDetail, Event, Frame, FrameData, FrameStamp, GridDef, GridId,
    Patch, ProtoError, Reply, ReplyBody, StateSnapshot, Welcome, decode_request,
    encode_event_message, encode_frame, encode_reply, peek_envelope,
};
use ac2_zmq::{Context, PollItem, Socket, SocketType, poll};

use crate::endpoint::Endpoints;

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
    /// Keepalives paused.
    pub ka_paused: bool,
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
    /// Devices.
    pub devices: Vec<DeviceInfo>,
    /// Lease refreshes accepted.
    pub refreshes: u32,
    /// Lease expiries.
    pub expiries: u32,
    /// What `delay.find` answers.
    pub finding: FakeFinding,
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
        },
        measurements: vec![],
        traces: vec![],
        generator: Generator {
            owner: None,
            armed: false,
            firing: false,
            settings: None,
            ceiling: Dbfs(-6.0),
            last_action: None,
        },
        calibrations: vec![],
        inputs: vec![],
        spl_logs: vec![],
        timing: TimingStatus {
            epoch: 0,
            state: TimingState::NoStimulus,
            last_lock: None,
            drift: None,
            internal_reference: false,
        },
    }
}

fn fake_devices() -> Vec<DeviceInfo> {
    let dir = |ch| DirectionInfo {
        max_channels: ch,
        rates_hz: vec![RangeU32 {
            min: 48_000,
            max: 48_000,
        }],
        buffer_frames: Some(RangeU32 { min: 256, max: 256 }),
        default_rate_hz: Some(48_000),
    };
    vec![DeviceInfo {
        backend: BackendKind::Fake,
        host: "fake".into(),
        id: DeviceId("fake:loop".into()),
        name: "Fake loopback".into(),
        input: Some(dir(4)),
        output: Some(dir(2)),
        duplex_clock: ClockRelation::SingleCallback,
        index: IndexExactness::Exact,
        notes: vec![],
    }]
}

/// The fake's calibration key: its one device.
fn fake_cal_key(input: u16, mic: &str) -> Result<CalKey, ProtoError> {
    if mic.is_empty() {
        return Err(err(ErrorCode::Invalid, "mic name is required"));
    }
    Ok(CalKey {
        device: DeviceId("fake:loop".into()),
        channel: input,
        mic: mic.to_owned(),
    })
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
            ka_paused: false,
            mute: false,
            drop_reply_once: HashSet::new(),
            executions: HashMap::new(),
            requests: vec![],
            grids: HashMap::new(),
            devices: fake_devices(),
            refreshes: 0,
            expiries: 0,
            finding: FakeFinding::default(),
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

    /// Calibrating binds the input's mic name, as the daemon does.
    fn set_mic(&mut self, channel: u16, mic: &str) {
        let mut all = self.state.inputs.clone();
        match all.iter_mut().find(|i| i.channel == channel) {
            Some(i) if i.mic.as_deref() == Some(mic) => return,
            Some(i) => i.mic = Some(mic.to_owned()),
            None => all.push(InputSetup {
                channel,
                mic: Some(mic.to_owned()),
                mic_curve: true,
            }),
        }
        all.sort_by_key(|i| i.channel);
        self.commit(Change::Inputs(all));
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
        let devices = std::mem::take(&mut self.devices);
        *self = Self::new(opts, incarnation);
        self.devices = devices;
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

    fn put_meas(&mut self, m: Measurement) -> ReplyBody {
        self.commit(Change::Measurement(Patch::Set(m.clone())));
        ReplyBody::Measurement(m)
    }

    fn execute(&mut self, client: &ClientId, cmd: Command) -> Result<ReplyBody, ProtoError> {
        use Command as C;
        Ok(match cmd {
            C::Hello { .. } => ReplyBody::Welcome(Welcome {
                server: self.opts.server.clone(),
                client_id: client.clone(),
                daemon_incarnation: self.incarnation,
                session_epoch: self.state.session.epoch,
                rev: self.rev,
            }),
            C::SessionDevices => ReplyBody::Devices(self.devices.clone()),
            C::SessionOpen { config } => {
                let dev = |s: &DeviceSelector| match s {
                    DeviceSelector::Default => DeviceId("fake:loop".into()),
                    DeviceSelector::Id { id } => id.clone(),
                };
                let s = Session {
                    epoch: SessionEpoch(self.state.session.epoch.0 + 1),
                    open: Some(OpenSession {
                        input_device: dev(&config.input_device),
                        output_device: dev(&config.output_device),
                        sample_rate_hz: config.sample_rate_hz.unwrap_or(48_000),
                        buffer_frames: config.buffer_frames.unwrap_or(256),
                        clock: ClockRelation::SingleCallback,
                        opened_at: WallNs(self.now_ns()),
                        config,
                    }),
                };
                self.commit(Change::Session(s.clone()));
                ReplyBody::Session(s)
            }
            C::SessionClose => {
                let s = Session {
                    epoch: SessionEpoch(self.state.session.epoch.0 + 1),
                    open: None,
                };
                let rev = self.commit(Change::Session(s));
                ReplyBody::Ack { rev }
            }
            C::SessionStatus => ReplyBody::Session(self.state.session.clone()),
            C::GenAcquire { force } => {
                self.expire_lease();
                if let Some(l) = &self.lease
                    && &l.owner != client
                    && !force
                {
                    return Err(ProtoError {
                        code: ErrorCode::LeaseHeld,
                        msg: format!("held by {}", l.owner.0),
                        detail: Some(ErrorDetail::LeaseHeld {
                            owner: l.owner.clone(),
                        }),
                    });
                }
                let forced = self.lease.is_some();
                self.next_token += 1;
                let token = LeaseToken(self.next_token);
                self.lease = Some(LeaseSlot {
                    token,
                    owner: client.clone(),
                    deadline: Instant::now() + self.opts.lease_expiry,
                });
                self.state.generator.owner = Some(client.clone());
                self.state.generator.armed = false;
                self.state.generator.firing = false;
                let action = if forced {
                    GenAction::Force
                } else {
                    GenAction::Acquire
                };
                self.generator_changed(action, Some(client.clone()));
                ReplyBody::Lease(Lease {
                    lease_token: token,
                    expires_in_ms: self.opts.lease_expiry.as_millis() as u32,
                })
            }
            C::GenSet {
                lease_token,
                desired,
            } => {
                self.expire_lease();
                self.check_lease(lease_token)?;
                if desired.firing && !desired.armed {
                    return Err(err(ErrorCode::Refused, "firing requires armed"));
                }
                if desired.settings.level.0 > self.state.generator.ceiling.0 {
                    return Err(err(ErrorCode::Refused, "level above ceiling"));
                }
                self.refresh();
                let action = if desired.firing {
                    GenAction::Fire
                } else if desired.armed {
                    GenAction::Arm
                } else {
                    GenAction::Set
                };
                self.state.generator.armed = desired.armed;
                self.state.generator.firing = desired.firing;
                self.state.generator.settings = Some(desired.settings);
                self.generator_changed(action, Some(client.clone()));
                ReplyBody::Generator(self.state.generator.clone())
            }
            C::GenRefresh { lease_token } => {
                self.expire_lease();
                self.check_lease(lease_token)?;
                self.refresh();
                self.refreshes += 1;
                ReplyBody::Lease(Lease {
                    lease_token,
                    expires_in_ms: self.opts.lease_expiry.as_millis() as u32,
                })
            }
            C::GenRelease { lease_token } => {
                self.expire_lease();
                self.check_lease(lease_token)?;
                self.lease = None;
                self.state.generator.owner = None;
                self.state.generator.armed = false;
                self.state.generator.firing = false;
                self.generator_changed(GenAction::Release, Some(client.clone()));
                ReplyBody::Ack { rev: self.rev }
            }
            C::GenStop => {
                self.state.generator.armed = false;
                self.state.generator.firing = false;
                self.generator_changed(GenAction::Stop, Some(client.clone()));
                ReplyBody::Ack { rev: self.rev }
            }
            C::MeasCreate { config } => {
                let id = MeasId(self.next_id);
                self.next_id += 1;
                let (delay, grid_id) = match &config.kind {
                    MeasKind::Transfer { config } => {
                        let g = GridDef::Log {
                            ppo: config.grid.ppo,
                            k_min: config.grid.k_min,
                            k_max: config.grid.k_max,
                        };
                        let gid = g.id();
                        self.grids.insert(gid, g);
                        (
                            Some(DelayState {
                                applied: Seconds(0.0),
                                applied_samples: Samples(0),
                                tracking: false,
                                last_finding: None,
                            }),
                            Some(gid),
                        )
                    }
                    _ => (None, None),
                };
                let m = Measurement {
                    id,
                    config,
                    config_rev: Rev(self.rev.0 + 1),
                    running: false,
                    frozen: false,
                    delay,
                    grid_id,
                };
                self.put_meas(m)
            }
            C::MeasUpdate { meas, config } => {
                let mut m = self.meas(meas)?;
                m.config = config;
                m.config_rev = Rev(self.rev.0 + 1);
                self.put_meas(m)
            }
            C::MeasDelete { meas } => {
                self.meas(meas)?;
                let rev = self.commit(Change::Measurement(Patch::Deleted(meas)));
                ReplyBody::Ack { rev }
            }
            C::MeasStart { meas } => {
                let mut m = self.meas(meas)?;
                m.running = true;
                self.put_meas(m)
            }
            C::MeasStop { meas } => {
                let mut m = self.meas(meas)?;
                m.running = false;
                self.put_meas(m)
            }
            C::MeasFreeze { meas, frozen } => {
                let mut m = self.meas(meas)?;
                m.frozen = frozen;
                self.put_meas(m)
            }
            C::MeasReset { meas } => {
                self.meas(meas)?;
                ReplyBody::Ack { rev: self.rev }
            }
            C::DelayFind { meas, band, .. } => {
                let mut m = self.meas(meas)?;
                let f = finding(self.finding, band, self.now_ns());
                let st = m
                    .delay
                    .as_mut()
                    .ok_or_else(|| err(ErrorCode::Invalid, "not a transfer measurement"))?;
                st.last_finding = Some(f.clone());
                self.commit(Change::Measurement(Patch::Set(m)));
                ReplyBody::DelayFinding(f)
            }
            C::DelayInsert { meas, pick } => {
                let mut m = self.meas(meas)?;
                let f = m
                    .delay
                    .as_ref()
                    .and_then(|d| d.last_finding.clone())
                    .ok_or_else(|| err(ErrorCode::Invalid, "no delay finding to insert"))?;
                if let Some(r) = f.no_estimate() {
                    return Err(err(ErrorCode::Refused, format!("no estimate: {r:?}")));
                }
                let d = f
                    .arrival(pick)
                    .ok_or_else(|| err(ErrorCode::NotFound, "no such arrival"))?
                    .delay;
                set_delay(&mut m, d)?;
                self.put_meas(m)
            }
            C::DelaySet { meas, delay } => {
                let mut m = self.meas(meas)?;
                set_delay(&mut m, delay)?;
                // An explicit value supersedes the last finding (as the daemon does).
                if let Some(d) = &mut m.delay {
                    d.last_finding = None;
                }
                self.put_meas(m)
            }
            C::DelayTrack { meas, enabled } => {
                let mut m = self.meas(meas)?;
                let d = m
                    .delay
                    .as_mut()
                    .ok_or_else(|| err(ErrorCode::Invalid, "not a transfer measurement"))?;
                d.tracking = enabled;
                self.put_meas(m)
            }
            C::TraceCapture { meas, name, slot } => self.trace_capture(meas, name, slot)?,
            C::TraceList => ReplyBody::Traces(self.state.traces.clone()),
            C::TraceGet { trace } => self.trace_get(trace)?,
            C::TraceUpdate { trace, edit } => self.trace_update(trace, edit)?,
            C::TraceDelete { trace } => self.trace_delete(trace)?,
            C::TraceAverage {
                traces,
                method,
                reference,
                name,
            } => self.trace_average(traces, method, reference, name)?,
            C::TraceMath { a, b, op, name } => self.trace_math(a, b, op, name)?,
            C::TraceImport {
                file_name,
                format,
                role,
                content,
            } => self.trace_import(file_name, format, role, &content.0)?,
            C::TraceExport { trace, .. } => self.trace_export(trace)?,
            C::FileSave { session } => self.file_save(&session)?,
            C::FileLoad { session } => self.file_load(client, &session)?,
            C::FileList => self.file_list()?,
            C::CalSpl {
                input,
                mic,
                calibrator_level,
                calibrator_freq,
            } => {
                let key = fake_cal_key(input, &mic)?;
                let prev = self.state.calibrations.iter().find(|e| e.key == key);
                let e = CalEntry {
                    spl: Some(SplCal {
                        sensitivity: Db(calibrator_level.0 + 20.0),
                        calibrator_level,
                        calibrator_freq,
                        measured: Dbfs(-20.0),
                        calibrated_at: WallNs(1_790_000_000_000_000_000),
                    }),
                    mic_curve: prev.and_then(|p| p.mic_curve.clone()),
                    key,
                };
                self.commit(Change::Calibration(Patch::Set(e.clone())));
                self.set_mic(input, &mic);
                ReplyBody::Calibration(e)
            }
            C::CalMicCurve { input, mic, action } => {
                let key = fake_cal_key(input, &mic)?;
                let prev = self
                    .state
                    .calibrations
                    .iter()
                    .find(|e| e.key == key)
                    .cloned();
                match action {
                    MicCurveAction::Import { file_name, content } => {
                        // Data lines only; the real parser lives in ac2-core.
                        let points = String::from_utf8_lossy(&content.0)
                            .lines()
                            .filter(|l| {
                                let mut f = l.split_whitespace().map(str::parse::<f64>);
                                matches!((f.next(), f.next()), (Some(Ok(_)), Some(Ok(_))))
                            })
                            .count();
                        if points < 2 {
                            return Err(ProtoError {
                                code: ErrorCode::Invalid,
                                msg: format!("mic curve file refused: {points} data lines"),
                                detail: Some(ErrorDetail::MicCurveFile {
                                    line: None,
                                    reason: ac2_proto::MicCurveFileReason::TooFewPoints,
                                }),
                            });
                        }
                        let e = CalEntry {
                            spl: prev.and_then(|p| p.spl),
                            mic_curve: Some(MicCurveRef {
                                name: file_name
                                    .rsplit_once('.')
                                    .map_or(file_name.as_str(), |(s, _)| s)
                                    .to_owned(),
                                file_name: file_name.clone(),
                                content_hash: "0000000000000000".into(),
                                points: u32::try_from(points).unwrap_or(u32::MAX),
                                f_lo: Hz(20.0),
                                f_hi: Hz(20_000.0),
                                imported_at: WallNs(1_790_000_000_000_000_000),
                            }),
                            key,
                        };
                        self.commit(Change::Calibration(Patch::Set(e.clone())));
                        self.set_mic(input, &mic);
                        ReplyBody::Calibration(e)
                    }
                    MicCurveAction::Clear => {
                        let Some(p) = prev.filter(|p| p.mic_curve.is_some()) else {
                            return Err(err(ErrorCode::NotFound, "no mic curve"));
                        };
                        let rev = if p.spl.is_some() {
                            self.commit(Change::Calibration(Patch::Set(CalEntry {
                                mic_curve: None,
                                ..p
                            })))
                        } else {
                            self.commit(Change::Calibration(Patch::Deleted(p.key)))
                        };
                        ReplyBody::Ack { rev }
                    }
                }
            }
            C::CalList => ReplyBody::Calibrations(self.state.calibrations.clone()),
            C::SessionInputs { inputs } => {
                let mut all: Vec<InputSetup> = self
                    .state
                    .inputs
                    .iter()
                    .filter(|c| inputs.iter().all(|r| r.channel != c.channel))
                    .cloned()
                    .collect();
                all.extend(inputs);
                all.sort_by_key(|i| i.channel);
                self.commit(Change::Inputs(all.clone()));
                ReplyBody::Inputs(all)
            }
            C::SplLogStart { meas, interval } => {
                let l = SplLog {
                    meas,
                    running: true,
                    interval,
                    started_at: Some(WallNs(self.now_ns())),
                };
                self.commit(Change::SplLog(Patch::Set(l.clone())));
                ReplyBody::SplLog(l)
            }
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
            other => {
                return Err(err(
                    ErrorCode::Unsupported,
                    format!("{} is not faked", other.name()),
                ));
            }
        })
    }
}

fn arrival(ms: f64, level: f64) -> DelayArrival {
    DelayArrival {
        delay: Seconds(ms / 1000.0),
        delay_samples: ms * 48.0,
        level: Db(level),
        phase: Degrees(0.0),
        uncertainty_samples: 0.2,
        misfit: 0.01,
        refined: true,
    }
}

fn finding(kind: FakeFinding, band: FinderBand, now: u64) -> DelayFinding {
    let band = match band {
        FinderBand::Full | FinderBand::Auto => DelayBand::Full,
        FinderBand::Mid => DelayBand::Mid,
        FinderBand::Sub => DelayBand::Sub,
        FinderBand::Custom { lo_hz, hi_hz } => DelayBand::Custom { lo_hz, hi_hz },
    };
    let confidence = DelayConfidence {
        psr_db: Some(Db(24.0)),
        psr_acq_db: Some(Db(20.0)),
        band_snr_db: Some(Db(30.0)),
        excited_fraction: Some(1.0),
        uncertainty_samples: Some(0.2),
        pulse_width_samples: Some(4.0),
        period: None,
    };
    let (outcome, candidates, confidence) = match kind {
        FakeFinding::Accepted => (
            DelayOutcome::Accepted {
                first: arrival(12.5, -2.5),
                strongest: arrival(12.7, 0.0),
            },
            vec![arrival(12.5, -2.5), arrival(12.7, 0.0)],
            confidence,
        ),
        FakeFinding::Ambiguous => (
            DelayOutcome::Ambiguous {
                reasons: vec![AmbiguityReason::BorderlineLevel],
                ranked: vec![
                    arrival(12.5, -11.5),
                    arrival(12.7, 0.0),
                    arrival(13.4, -6.0),
                ],
                strongest: arrival(12.7, 0.0),
            },
            vec![
                arrival(12.5, -11.5),
                arrival(12.7, 0.0),
                arrival(13.4, -6.0),
            ],
            confidence,
        ),
        FakeFinding::NoEstimate => (
            DelayOutcome::NoEstimate {
                reasons: vec![NoEstimateReason::LowPsr, NoEstimateReason::LowBandSnr],
            },
            vec![],
            DelayConfidence {
                psr_db: Some(Db(4.0)),
                band_snr_db: Some(Db(2.0)),
                uncertainty_samples: None,
                ..confidence
            },
        ),
    };
    DelayFinding {
        outcome,
        confidence,
        band,
        observation: Seconds(0.25),
        candidates,
        found_at: WallNs(now),
    }
}

fn set_delay(m: &mut Measurement, d: Seconds) -> Result<(), ProtoError> {
    let st = m
        .delay
        .as_mut()
        .ok_or_else(|| err(ErrorCode::Invalid, "not a transfer measurement"))?;
    st.applied = d;
    st.applied_samples = Samples((d.0 * 48_000.0).round() as i64);
    Ok(())
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
            s.execute(&client, req.cmd)
        }
    };
    let bytes = encode_reply(&Reply::new(req.id, result)).ok()?;
    s.dedup.insert(key, bytes.clone());
    if s.drop_reply_once.remove(op) {
        return None;
    }
    Some(bytes)
}
