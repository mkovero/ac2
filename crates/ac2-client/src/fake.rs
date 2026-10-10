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
    /// Backends and their devices (`session.devices`).
    pub backends: Vec<BackendInfo>,
    /// The device `session.preview` last opened; `None` once stopped.
    pub preview: Option<(BackendKind, DeviceId)>,
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
        mics: vec![],
        inputs: vec![],
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
        },
        recording: None,
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

/// Band levels of a transfer source as the daemon takes them: typed levels, or a span of
/// the band rows a test gave the fake (`FakeDaemon::band_rows`).
fn fake_band_levels(
    src: &BandLevelSource,
    band_rows: &HashMap<MeasId, Vec<ac2_traces::band_log::BandLogRow>>,
) -> Result<[f64; BAND_COUNT], ProtoError> {
    match src {
        BandLevelSource::Levels { levels } => {
            if levels.len() != BAND_COUNT {
                return Err(err(
                    ErrorCode::Invalid,
                    format!("band levels are {BAND_COUNT} values, 20 Hz … 10 kHz"),
                ));
            }
            if levels.iter().flatten().any(|l| !l.0.is_finite()) {
                return Err(err(ErrorCode::Invalid, "a band level must be finite"));
            }
            let mut out = [f64::NAN; BAND_COUNT];
            for (o, l) in out.iter_mut().zip(levels) {
                if let Some(l) = l {
                    *o = l.0;
                }
            }
            Ok(out)
        }
        BandLevelSource::Log { meas, from, until } => {
            if until <= from {
                return Err(err(ErrorCode::Invalid, "the span ends before it starts"));
            }
            let rows = fake_band_rows_in(band_rows, *meas, *from, *until);
            ac2_traces::band_log::span_average(&rows)
                .levels()
                .map_err(|g| err(ErrorCode::Invalid, format!("SPL meter {meas}: {g}")))
        }
    }
}

fn fake_band_rows_in(
    band_rows: &HashMap<MeasId, Vec<ac2_traces::band_log::BandLogRow>>,
    meas: MeasId,
    from: WallNs,
    until: WallNs,
) -> Vec<ac2_traces::band_log::BandLogRow> {
    band_rows
        .get(&meas)
        .map(|r| {
            r.iter()
                .filter(|r| r.start >= from && r.start < until)
                .copied()
                .collect()
        })
        .unwrap_or_default()
}

fn fake_transfer_band(b: ac2_core::band_leq::BandTransfer) -> BandTransferBand {
    use ac2_core::band_leq::BandTransfer as T;
    match b {
        T::Unchecked { attenuation_db } => BandTransferBand::Unchecked {
            attenuation: Db(attenuation_db),
        },
        T::Clean { attenuation_db } => BandTransferBand::Clean {
            attenuation: Db(attenuation_db),
        },
        T::Corrected {
            attenuation_db,
            margin_db,
        } => BandTransferBand::Corrected {
            attenuation: Db(attenuation_db),
            margin: Db(margin_db),
        },
        T::Unusable { at_least_db } => BandTransferBand::Unusable {
            at_least: Db(at_least_db),
        },
        T::Missing => BandTransferBand::Missing,
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
            ka_paused: false,
            mute: false,
            drop_reply_once: HashSet::new(),
            executions: HashMap::new(),
            requests: vec![],
            grids: HashMap::new(),
            backends: fake_backends(),
            preview: None,
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

    /// Calibrating binds the input's mic name, as the daemon does; a new mic starts with
    /// no curve chosen.
    fn set_mic(&mut self, channel: u16, mic: &str) {
        let mut all = self.state.inputs.clone();
        match all.iter_mut().find(|i| i.channel == channel) {
            Some(i) if i.mic.as_deref() == Some(mic) => {}
            Some(i) => {
                i.mic = Some(mic.to_owned());
                i.curve = CurveChoice::NotChosen;
            }
            None => all.push(InputSetup {
                channel,
                mic: Some(mic.to_owned()),
                curve: CurveChoice::NotChosen,
            }),
        }
        self.set_inputs(all);
    }

    /// Commits the input setup (sorted, the only curve of a one-curve mic chosen where none
    /// is, as the daemon does) when it changed.
    fn set_inputs(&mut self, mut all: Vec<InputSetup>) -> Option<Rev> {
        all.sort_by_key(|i| i.channel);
        for r in &mut all {
            ac2_proto::cal::settle(r, &self.state.mics);
        }
        (all != self.state.inputs).then(|| self.commit(Change::Inputs(all)))
    }

    /// `cal.curve_import`, as the daemon (labels and stated sensitivity from the file).
    fn curve_import(
        &mut self,
        mic: String,
        label: Option<String>,
        file_name: &str,
        content: &[u8],
        input: Option<u16>,
    ) -> Result<ReplyBody, ProtoError> {
        ac2_proto::cal::check_mic_name(&mic).map_err(|e| err(ErrorCode::Invalid, e))?;
        let curve = ac2_core::mic_curve::MicCurve::parse(content).map_err(|e| ProtoError {
            code: ErrorCode::Invalid,
            msg: format!("mic curve file refused: {e}"),
            detail: Some(ErrorDetail::MicCurveFile {
                line: e.line().map(|l| u32::try_from(l).unwrap_or(u32::MAX)),
                reason: ac2_proto::MicCurveFileReason::TooFewPoints,
            }),
        })?;
        let info = ac2_core::mic_curve::file_info(content, file_name);
        let label =
            label.unwrap_or_else(|| ac2_proto::cal::default_label(info.angle_deg, file_name));
        ac2_proto::cal::check_label(&label).map_err(|e| err(ErrorCode::Invalid, e))?;
        let r = MicCurveRef {
            label: label.clone(),
            file_name: file_name.to_owned(),
            content_hash: "0000000000000000".into(),
            points: u32::try_from(curve.len()).unwrap_or(u32::MAX),
            f_lo: Hz(curve.f_lo()),
            f_hi: Hz(curve.f_hi()),
            imported_at: WallNs(1_790_000_000_000_000_000),
            stated_sensitivity: info.stated_sensitivity_mv_per_pa,
        };
        let mut m = ac2_proto::cal::mic(&self.state.mics, &mic)
            .cloned()
            .unwrap_or(Mic {
                name: mic.clone(),
                curves: vec![],
            });
        match m.curves.iter_mut().find(|c| c.label == label) {
            Some(c) => *c = r,
            None => m.curves.push(r),
        }
        self.traces.curve_points.insert(
            MicCurveId {
                mic: mic.clone(),
                label: label.clone(),
            },
            curve
                .freqs()
                .iter()
                .zip(curve.gains())
                .map(|(f, g)| [*f, *g])
                .collect(),
        );
        self.commit(Change::Mic(Patch::Set(m.clone())));
        if let Some(i) = input {
            self.set_mic(i, &mic);
            if m.curves.len() == 1 {
                let mut all = self.state.inputs.clone();
                if let Some(row) = all.iter_mut().find(|r| r.channel == i) {
                    row.curve = CurveChoice::Curve { label };
                }
                self.set_inputs(all);
            }
        }
        Ok(ReplyBody::Mic(m))
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

    fn put_meas(&mut self, m: Measurement) -> ReplyBody {
        self.commit(Change::Measurement(Patch::Set(m.clone())));
        self.spl_log_for(&m);
        ReplyBody::Measurement(m)
    }

    /// The `spl_log` entity of an SPL meter follows its windows, as the daemon's does: a
    /// window configured as before keeps its state; others start unjudged (the fake has no
    /// calibration path for SPL, so a limit reads "not calibrated" until a test commits
    /// states of its own).
    fn spl_log_for(&mut self, m: &Measurement) {
        let MeasKind::Spl { config } = &m.config.kind else {
            if self.state.spl_logs.iter().any(|l| l.meas == m.id) {
                self.commit(Change::SplLog(Patch::Deleted(m.id)));
            }
            return;
        };
        let prev = self.state.spl_logs.iter().find(|l| l.meas == m.id).cloned();
        let now = WallNs(self.now_ns());
        let windows = config
            .leq
            .windows
            .iter()
            .map(|w| {
                prev.as_ref()
                    .and_then(|p| {
                        p.windows
                            .iter()
                            .find(|s| s.duration == w.duration && s.weighting == w.weighting)
                    })
                    .copied()
                    .filter(|s| (s.judgement == LeqJudgement::NoLimit) == w.limit.is_none())
                    .unwrap_or(LeqWindowState {
                        duration: w.duration,
                        weighting: w.weighting,
                        judgement: if w.limit.is_some() {
                            LeqJudgement::NotCalibrated
                        } else {
                            LeqJudgement::NoLimit
                        },
                        since: now,
                    })
            })
            .collect();
        let peak = |q: ac2_proto::model::PeakQuantity| {
            let judgement = if config.leq.peaks.get(q).is_some() {
                LeqJudgement::NotCalibrated
            } else {
                LeqJudgement::NoLimit
            };
            prev.as_ref()
                .map(|p| p.peaks.get(q))
                .filter(|s| s.judgement == judgement)
                .unwrap_or(ac2_proto::model::LeqPeakState {
                    judgement,
                    since: now,
                })
        };
        let peaks = ac2_proto::model::PeakStates {
            lcpeak: peak(ac2_proto::model::PeakQuantity::LcPeak),
            lafmax: peak(ac2_proto::model::PeakQuantity::LafMax),
        };
        let l = SplLog {
            meas: m.id,
            started_at: prev.as_ref().and_then(|p| p.started_at),
            windows,
            peaks,
            alarms: prev.map(|p| p.alarms).unwrap_or_default(),
        };
        if self.state.spl_logs.iter().find(|x| x.meas == m.id) != Some(&l) {
            self.commit(Change::SplLog(Patch::Set(l)));
        }
    }

    /// `invalid` unless `meas` is an SPL meter.
    fn spl_meter(&self, meas: MeasId) -> Result<Measurement, ProtoError> {
        let m = self.meas(meas)?;
        if !matches!(m.config.kind, MeasKind::Spl { .. }) {
            return Err(ProtoError {
                code: ErrorCode::Invalid,
                msg: format!("measurement {meas} is not an SPL meter"),
                detail: None,
            });
        }
        Ok(m)
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
            C::SessionDevices => ReplyBody::Backends(self.backends.clone()),
            C::SessionPreview { backend, device } => {
                let channels = self
                    .backends
                    .iter()
                    .flat_map(|b| &b.devices)
                    .find(|d| d.backend == backend && d.id == device)
                    .and_then(|d| d.input.as_ref())
                    .map(|i| i.max_channels)
                    .ok_or_else(|| err(ErrorCode::NotFound, "no such device"))?;
                self.preview = Some((backend, device.clone()));
                ReplyBody::Preview(Preview {
                    backend,
                    device,
                    channels,
                    sample_rate_hz: 48_000,
                    expires_in_ms: 5000,
                })
            }
            C::SessionPreviewStop => {
                self.preview = None;
                ReplyBody::Ack { rev: self.rev }
            }
            C::SessionDetectLoopback {
                lease_token,
                backend,
                input_device,
                output_device,
                output,
                level,
            } => {
                self.expire_lease();
                self.check_lease(lease_token)?;
                let Some(level) = level else {
                    return Err(err(ErrorCode::Refused, "no level"));
                };
                if level.0 > self.state.generator.ceiling.0 {
                    return Err(err(ErrorCode::Refused, "level above ceiling"));
                }
                self.preview = None;
                ReplyBody::LoopbackDetection(LoopbackDetection {
                    backend,
                    input_device,
                    output_device,
                    output,
                    level,
                    ..self.detection.clone()
                })
            }
            C::SessionOpen { config } => {
                let dev = |s: &DeviceSelector| match s {
                    DeviceSelector::Default => DeviceId("fake:loop".into()),
                    DeviceSelector::Id { id } => id.clone(),
                };
                self.preview = None;
                let s = Session {
                    epoch: SessionEpoch(self.state.session.epoch.0 + 1),
                    open: Some(OpenSession {
                        backend: config.backend.unwrap_or(BackendKind::Fake),
                        input_device: dev(&config.input_device),
                        output_device: dev(&config.output_device),
                        sample_rate_hz: config.sample_rate_hz.unwrap_or(48_000),
                        buffer_frames: config.buffer_frames.unwrap_or(256),
                        clock: ClockRelation::SingleCallback,
                        opened_at: WallNs(self.now_ns()),
                        config,
                        replay: None,
                    }),
                    stopped: None,
                };
                self.commit(Change::Session(s.clone()));
                ReplyBody::Session(s)
            }
            C::SessionClose => {
                let s = Session {
                    epoch: SessionEpoch(self.state.session.epoch.0 + 1),
                    open: None,
                    stopped: None,
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
            C::GenCeiling {
                ceiling,
                confirm_raise,
            } => {
                let g = &self.state.generator;
                if !ceiling.0.is_finite() || ceiling.0 > g.ceiling_bound.0 {
                    return Err(err(ErrorCode::Invalid, "above the bound"));
                }
                let action = if ceiling.0 > g.ceiling.0 {
                    if !confirm_raise {
                        return Err(err(ErrorCode::Refused, "raising needs a confirmation"));
                    }
                    if g.armed || g.firing {
                        return Err(err(ErrorCode::Refused, "armed: stop first"));
                    }
                    GenAction::CeilingRaised
                } else {
                    let above = g.settings.as_ref().is_some_and(|s| s.level.0 > ceiling.0);
                    if above && (g.armed || g.firing) {
                        self.state.generator.armed = false;
                        self.state.generator.firing = false;
                    }
                    GenAction::CeilingLowered
                };
                self.state.generator.ceiling = ceiling;
                self.generator_changed(action, Some(client.clone()));
                ReplyBody::Generator(self.state.generator.clone())
            }
            C::SessionOutputs { outputs } => {
                for o in &outputs {
                    if let Some(l) = &o.label {
                        check_output_label(l).map_err(|e| err(ErrorCode::Invalid, e))?;
                    }
                }
                let mut all: Vec<OutputSetup> = self
                    .state
                    .outputs
                    .iter()
                    .filter(|c| outputs.iter().all(|r| r.channel != c.channel))
                    .cloned()
                    .collect();
                all.extend(outputs.into_iter().filter(|o| o.label.is_some()));
                all.sort_by_key(|o| o.channel);
                if all != self.state.outputs {
                    self.commit(Change::Outputs(all));
                }
                ReplyBody::Outputs(self.state.outputs.clone())
            }
            C::ServerInfo => ReplyBody::Server(ServerInfo {
                mode: ServerMode::Embedded,
                recording_dir: None,
            }),
            C::ServerAuthorize { .. } | C::ServerRevoke { .. } => {
                return Err(err(
                    ErrorCode::Unsupported,
                    "the fake daemon is not in network mode",
                ));
            }
            C::MeasCreate { config } => {
                let id = MeasId(self.next_id);
                self.next_id += 1;
                let (delay, grid_id) = match &config.kind {
                    MeasKind::Transfer { config } => {
                        let g = GridDef::Log {
                            ppo: config.grid().ppo,
                            k_min: config.grid().k_min,
                            k_max: config.grid().k_max,
                        };
                        let gid = g.id();
                        self.grids.insert(gid, g);
                        (
                            Some(DelayState {
                                applied: Seconds(0.0),
                                applied_samples: 0.0,
                                nudged: Seconds(0.0),
                                nudged_samples: 0.0,
                                tracking: false,
                                awaiting_pick: false,
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
            C::MeasDelete { meas, traces } => {
                self.meas(meas)?;
                // What it owns: kept under the imported group, or deleted with it.
                let owner = TraceOwner::Meas { meas };
                let owned: Vec<TraceMeta> = self
                    .state
                    .traces
                    .iter()
                    .filter(|t| t.edit.owner == owner)
                    .cloned()
                    .collect();
                let maths: Vec<Measurement> = self
                    .state
                    .measurements
                    .iter()
                    .filter(|m| matches!(&m.config.kind, MeasKind::Math { config } if config.owner == owner))
                    .cloned()
                    .collect();
                match traces {
                    OwnedTraces::Keep => {
                        for mut t in owned {
                            t.edit.owner = TraceOwner::Imported;
                            self.commit(Change::Trace(Patch::Set(t)));
                        }
                        for mut m in maths {
                            if let MeasKind::Math { config } = &mut m.config.kind {
                                config.owner = TraceOwner::Imported;
                            }
                            self.commit(Change::Measurement(Patch::Set(m)));
                        }
                    }
                    OwnedTraces::Delete => {
                        for m in maths {
                            self.commit(Change::Measurement(Patch::Deleted(m.id)));
                        }
                        for t in owned {
                            self.traces.data.remove(&t.id);
                            self.traces.sweeps.remove(&t.id);
                            self.commit(Change::Trace(Patch::Deleted(t.id)));
                        }
                    }
                }
                if self.state.spl_logs.iter().any(|l| l.meas == meas) {
                    self.commit(Change::SplLog(Patch::Deleted(meas)));
                }
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
            C::MeasReset { meas } => {
                self.meas(meas)?;
                ReplyBody::Ack { rev: self.rev }
            }
            C::DelayFind {
                meas,
                band,
                observation,
            } => {
                let mut m = self.meas(meas)?;
                self.last_find = Some((band, observation));
                let f = finding(self.finding, band, self.now_ns());
                let st = m
                    .delay
                    .as_mut()
                    .ok_or_else(|| err(ErrorCode::Invalid, "not a transfer measurement"))?;
                st.awaiting_pick = matches!(f.outcome, DelayOutcome::Ambiguous { .. });
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
                let arrival = m.delay.as_ref().map(|d| d.applied.0 - d.nudged.0);
                set_delay(&mut m, delay)?;
                if let Some(d) = &mut m.delay {
                    // A typed value keeps the arrival and moves the live curve alone, as a
                    // step does (as the daemon does).
                    if let Some(a) = arrival {
                        d.nudged = Seconds(delay.0 - a);
                        d.nudged_samples = d.nudged.0 * 48_000.0;
                    }
                    // An explicit value supersedes the last finding (as the daemon does).
                    d.last_finding = None;
                }
                self.put_meas(m)
            }
            C::DelayNudge { meas, by } => {
                let mut m = self.meas(meas)?;
                let st = m
                    .delay
                    .as_mut()
                    .ok_or_else(|| err(ErrorCode::Invalid, "not a transfer measurement"))?;
                // A step from the arrival: the daemon keeps what nudges added apart.
                st.awaiting_pick = false;
                st.applied = Seconds(st.applied.0 + by.0);
                st.applied_samples = st.applied.0 * 48_000.0;
                st.nudged = Seconds(st.nudged.0 + by.0);
                st.nudged_samples = st.nudged.0 * 48_000.0;
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
                content,
            } => self.trace_import(file_name, format, role, &content.0)?,
            C::TraceExport { trace, .. } => self.trace_export(trace)?,
            C::TraceMicCurve { trace, curve } => self.trace_mic_curve(trace, curve)?,
            C::FileSave { session } => self.file_save(&session)?,
            C::FileLoad { session } => self.file_load(client, &session)?,
            C::FileList => self.file_list()?,
            C::RecStart { request } => self.rec_start(client, request)?,
            C::RecStop => self.rec_stop()?,
            C::RecList => ReplyBody::Recordings(Vec::new()),
            C::SessionReplay { .. } => {
                return Err(err(
                    ErrorCode::Unsupported,
                    "the fake daemon has no recordings to replay",
                ));
            }
            C::CalSpl {
                input,
                mic,
                calibrator_level,
                calibrator_freq,
            } => {
                let key = fake_cal_key(input, &mic)?;
                let e = CalEntry {
                    spl: SplCal {
                        sensitivity: Db(calibrator_level.0 + 20.0),
                        method: ac2_proto::model::CalMethod::Acoustic { calibrator_level },
                        freq: calibrator_freq,
                        measured: Dbfs(-20.0),
                        calibrated_at: WallNs(1_790_000_000_000_000_000),
                    },
                    key,
                };
                self.commit(Change::Calibration(Patch::Set(e.clone())));
                self.set_mic(input, &mic);
                ReplyBody::Calibration(e)
            }
            // The fake input reads −20 dBFS, as for `cal.spl`.
            C::CalSplElectrical {
                input,
                mic,
                connection,
                volts,
                freq,
                mic_sensitivity,
                uncertainty,
                replace_acoustic,
            } => {
                use ac2_proto::cal;
                use ac2_proto::model::{CalMethod, SensitivitySource};
                let key = fake_cal_key(input, &mic)?;
                let (s, from) = match mic_sensitivity {
                    Some(s) => (s, SensitivitySource::Typed),
                    None => match cal::data_sheet(&self.state.mics, &mic) {
                        cal::DataSheet::One(s, from) => (s, from),
                        _ => {
                            return Err(err(
                                ErrorCode::Invalid,
                                "no mic sensitivity given and no data-sheet value",
                            ));
                        }
                    },
                };
                if !replace_acoustic
                    && self
                        .state
                        .calibrations
                        .iter()
                        .any(|e| e.key == key && matches!(e.spl.method, CalMethod::Acoustic { .. }))
                {
                    return Err(err(ErrorCode::Refused, "input has an acoustic calibration"));
                }
                let measured = -20.0;
                let full_scale = cal::full_scale_volts(volts.0, measured);
                let e = CalEntry {
                    spl: SplCal {
                        sensitivity: Db(cal::electrical_sensitivity_db(full_scale, s.0)),
                        method: CalMethod::Electrical {
                            connection,
                            volts,
                            full_scale: ac2_proto::units::Volts(full_scale),
                            mic_sensitivity: s,
                            mic_sensitivity_from: from,
                            uncertainty: uncertainty.unwrap_or(cal::DEFAULT_ELECTRICAL_UNCERTAINTY),
                        },
                        freq,
                        measured: Dbfs(measured),
                        calibrated_at: WallNs(1_790_000_000_000_000_000),
                    },
                    key,
                };
                self.commit(Change::Calibration(Patch::Set(e.clone())));
                self.set_mic(input, &mic);
                ReplyBody::Calibration(e)
            }
            C::CalCurveImport {
                mic,
                label,
                file_name,
                content,
                input,
            } => self.curve_import(mic, label, &file_name, &content.0, input)?,
            C::CalCurveRename { curve, label } => {
                ac2_proto::cal::check_label(&label).map_err(|e| err(ErrorCode::Invalid, e))?;
                let Some(mut m) = ac2_proto::cal::mic(&self.state.mics, &curve.mic).cloned() else {
                    return Err(err(ErrorCode::NotFound, "no such mic"));
                };
                if label != curve.label && m.curves.iter().any(|c| c.label == label) {
                    return Err(err(ErrorCode::Invalid, "label taken"));
                }
                let Some(c) = m.curves.iter_mut().find(|c| c.label == curve.label) else {
                    return Err(err(ErrorCode::NotFound, "no such curve"));
                };
                c.label.clone_from(&label);
                if let Some(p) = self.traces.curve_points.remove(&curve) {
                    self.traces.curve_points.insert(
                        MicCurveId {
                            mic: curve.mic.clone(),
                            label: label.clone(),
                        },
                        p,
                    );
                }
                self.commit(Change::Mic(Patch::Set(m.clone())));
                let old = CurveChoice::Curve {
                    label: curve.label.clone(),
                };
                let mut all = self.state.inputs.clone();
                for r in &mut all {
                    if r.mic.as_deref() == Some(curve.mic.as_str()) && r.curve == old {
                        r.curve = CurveChoice::Curve {
                            label: label.clone(),
                        };
                    }
                }
                self.set_inputs(all);
                ReplyBody::Mic(m)
            }
            C::CalCurveDelete { curve } => {
                let Some(mut m) = ac2_proto::cal::mic(&self.state.mics, &curve.mic).cloned() else {
                    return Err(err(ErrorCode::NotFound, "no such mic"));
                };
                let n = m.curves.len();
                m.curves.retain(|c| c.label != curve.label);
                if m.curves.len() == n {
                    return Err(err(ErrorCode::NotFound, "no such curve"));
                }
                self.traces.curve_points.remove(&curve);
                let rev = if m.curves.is_empty() {
                    self.commit(Change::Mic(Patch::Deleted(m.name)))
                } else {
                    self.commit(Change::Mic(Patch::Set(m)))
                };
                let all = self.state.inputs.clone();
                let rev = self.set_inputs(all).unwrap_or(rev);
                ReplyBody::Ack { rev }
            }
            C::CalList => ReplyBody::Calibrations {
                calibrations: self.state.calibrations.clone(),
                mics: self.state.mics.clone(),
            },
            C::CalDelete { key } => {
                if !self.state.calibrations.iter().any(|e| e.key == key) {
                    return Err(err(ErrorCode::NotFound, "no such calibration"));
                }
                let rev = self.commit(Change::Calibration(Patch::Deleted(key)));
                ReplyBody::Ack { rev }
            }
            C::SessionInputs { inputs } => {
                for r in &inputs {
                    if let (Some(m), CurveChoice::Curve { label }) = (&r.mic, &r.curve)
                        && !self.state.inputs.contains(r)
                        && ac2_proto::cal::curve(&self.state.mics, m, label).is_none()
                    {
                        return Err(err(
                            ErrorCode::Invalid,
                            format!("no curve {label:?} stored for {m}"),
                        ));
                    }
                }
                let mut all: Vec<InputSetup> = self
                    .state
                    .inputs
                    .iter()
                    .filter(|c| inputs.iter().all(|r| r.channel != c.channel))
                    .cloned()
                    .collect();
                all.extend(inputs);
                self.set_inputs(all);
                ReplyBody::Inputs(self.state.inputs.clone())
            }
            C::SplLogGet {
                meas,
                log,
                from,
                max,
            } => {
                self.spl_meter(meas)?;
                let rows = match log {
                    SplLogWhich::Current => self.spl_rows.get(&meas).cloned().unwrap_or_default(),
                    SplLogWhich::Previous => {
                        self.spl_prev_rows
                            .get(&meas)
                            .cloned()
                            .ok_or_else(|| ProtoError {
                                code: ErrorCode::NotFound,
                                msg: format!("SPL meter {meas} has no previous log"),
                                detail: None,
                            })?
                    }
                };
                let total = rows.len() as u64;
                let from = from.min(total);
                let n = max.min(SplLogPage::MAX_ROWS) as usize;
                ReplyBody::SplLogPage(SplLogPage {
                    meas,
                    from,
                    total,
                    rows: rows.into_iter().skip(from as usize).take(n).collect(),
                })
            }
            C::SplHistoryGet { meas, .. } => {
                // The fake computes no windows: its meters have no history to replay.
                let m = self.spl_meter(meas)?;
                let windows = match &m.config.kind {
                    MeasKind::Spl { config } => config.leq.windows.clone(),
                    _ => Vec::new(),
                };
                let n = windows.len();
                ReplyBody::SplHistory(Box::new(SplHistory {
                    meas,
                    windows,
                    scale: LevelScale::Dbfs,
                    at: Vec::new(),
                    leq: vec![Vec::new(); n],
                    over: vec![Vec::new(); n],
                }))
            }
            C::SplBandLogGet {
                meas,
                from,
                until,
                step,
            } => {
                self.spl_meter(meas)?;
                let rows = fake_band_rows_in(&self.band_rows, meas, from, until);
                ac2_traces::band_log::span_reply(meas, from, until, step, &rows)
                    .map(|r| ReplyBody::SplBandLog(Box::new(r)))
                    .map_err(|m| err(ErrorCode::Invalid, m))?
            }
            C::SplLogNew { meas } => {
                let m = self.spl_meter(meas)?;
                let rows = self.spl_rows.remove(&meas).unwrap_or_default();
                self.spl_prev_rows.insert(meas, rows);
                // The entity starts over as for a new meter: initial states, no alarms.
                if self.state.spl_logs.iter().any(|l| l.meas == meas) {
                    self.commit(Change::SplLog(Patch::Deleted(meas)));
                }
                self.spl_log_for(&m);
                ReplyBody::Ack { rev: self.rev }
            }
            C::SplBandTransfer {
                meas,
                foh,
                at_place,
                background,
                place,
            } => {
                BandTransferSet::check_place(&place).map_err(|e| err(ErrorCode::Invalid, e))?;
                let mut m = self.spl_meter(meas)?;
                let MeasKind::Spl { config } = &mut m.config.kind else {
                    unreachable!("spl_meter checked the kind");
                };
                let Some(bands) = &mut config.bands else {
                    return Err(err(
                        ErrorCode::Invalid,
                        format!("SPL meter {meas} has no band meter: enable it first"),
                    ));
                };
                if let Some(e) = ac2_proto::model::overlapping_spans(
                    &foh,
                    &at_place,
                    background.as_ref(),
                    &place,
                    |id| {
                        self.meas(id)
                            .map_or_else(|_| format!("SPL meter {id}"), |m| m.config.name)
                    },
                ) {
                    return Err(err(ErrorCode::Invalid, e));
                }
                let foh = fake_band_levels(&foh, &self.band_rows)?;
                let at_place = fake_band_levels(&at_place, &self.band_rows)?;
                let background = background
                    .as_ref()
                    .map(|b| fake_band_levels(b, &self.band_rows))
                    .transpose()?;
                let t = ac2_core::band_leq::Transfer::measure(&foh, &at_place, background.as_ref());
                bands.transfer = Some(BandTransferSet {
                    place,
                    measured_at: WallNs(self.now_ns()),
                    origin: ac2_proto::model::TransferOrigin::Measured,
                    bands: t.bands().map(fake_transfer_band),
                });
                m.config_rev = Rev(self.rev.0 + 1);
                self.put_meas(m)
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

/// An operator-set delay (insert or typed), which also resolves an ambiguous finding.
/// Set as a new arrival (nothing nudged from it); a typed value then restores its arrival.
fn set_delay(m: &mut Measurement, d: Seconds) -> Result<(), ProtoError> {
    let st = m
        .delay
        .as_mut()
        .ok_or_else(|| err(ErrorCode::Invalid, "not a transfer measurement"))?;
    st.awaiting_pick = false;
    st.applied = d;
    st.applied_samples = d.0 * 48_000.0;
    st.nudged = Seconds(0.0);
    st.nudged_samples = 0.0;
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
