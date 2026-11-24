//! A rig for the app's pictures without a daemon (`ac2-ui` links none): a scene a real
//! `ac2d` recorded ([`crate::recorded::scene`]) fed to the app through `Msg::Conn` as its
//! link would deliver it — the hello, the mirror of the recorded state, the device list,
//! every stored trace's data — and frames of shapes of our own on its measurements. The
//! frames are display material only (shapes chosen to be recognisable on the recorded
//! grids), never a reference for any measurement.
//!
//! A test makes a [`Rig`] and opens the app with [`Rig::options`]-less options (no
//! target); the UI harness's waits call [`pump`] before each pass, which feeds every rig
//! alive on this thread.
#![allow(dead_code, clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_client::data::{Latest, TopicFrame};
use ac2_client::{MirrorView, Phase};
use ac2_proto::frame::{
    BinLayout, BinStage, BinsFrame, BinsMeta, DaemonHealth, GenSummary, IrFrame, IrMeta, MathState,
    OperandState, OperandStatus, ProtectionFlags, SpecFrame, SpecMeta, SplFrame, SplMeta, TfFrame,
    TfMeta, ValidityMask,
};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Frame, FrameData, FrameStamp, GridDef, GridId};
use ac2_ui::App;
use ac2_ui::conn::{ConnEvent, DataSnapshot};
use ac2_ui::state::Msg;

/// The rig's identity in its hello: the simulated rig as `ac2-e2e` started it.
pub const RIG_ID: RigId = RigId(0x51_6d);
pub const RIG_NAME: &str = "Simulated rig";
const INCARNATION: DaemonIncarnation = DaemonIncarnation(1);
const REV: Rev = Rev(100);

/// When the rig's SPL meter started: 2026-10-03 19:02:00 UTC (the screenshots show it in
/// UTC, on a day before any run of them: with its date).
pub const SPL_SINCE: WallNs = WallNs(1_791_054_120_000_000_000);

thread_local! {
    /// The rigs [`pump`] feeds.
    static RIGS: RefCell<Vec<Weak<RefCell<Inner>>>> = const { RefCell::new(Vec::new()) };
}

pub struct Rig {
    inner: Rc<RefCell<Inner>>,
}

struct Inner {
    state: Arc<State>,
    grids: BTreeMap<GridId, Arc<GridDef>>,
    traces: Vec<TraceData>,
    /// The transfer grid (Main L's) and the spectrum grid (Mic 1 FFT's).
    tf_grid: GridDef,
    spec_grid: GridDef,
    protection: ProtectionFlags,
    health: DaemonHealth,
    /// The hello and the first mirror were delivered.
    connected: bool,
    seq: u64,
}

impl Rig {
    /// The session open with Main L, Delay tower, Mic 1 FFT and FOH SPL running.
    pub fn start() -> Self {
        Self::scene("base")
    }

    /// Scene `name` of `fixtures/daemon/rig_<name>.json` (`ac2-e2e`'s `recorded/scenes.rs`
    /// builds each).
    pub fn scene(name: &str) -> Self {
        let s = crate::recorded::scene(name);
        let grids: BTreeMap<GridId, Arc<GridDef>> = s
            .grids
            .iter()
            .map(|g| (g.id(), Arc::new(g.clone())))
            .collect();
        let grid_of = |meas: &str| {
            let id = s
                .state
                .measurements
                .iter()
                .find(|m| m.config.name == meas)
                .and_then(|m| m.grid_id)
                .unwrap_or_else(|| panic!("{meas} has no grid"));
            (*grids[&id]).clone()
        };
        let inner = Inner {
            tf_grid: grid_of("Main L"),
            spec_grid: grid_of("Mic 1 FFT"),
            state: Arc::new(s.state),
            grids,
            traces: s.traces,
            protection: ProtectionFlags::NONE,
            health: DaemonHealth {
                host: "rig".into(),
                cpu: None,
                audio: None,
            },
            connected: false,
            seq: 0,
        };
        let inner = Rc::new(RefCell::new(inner));
        RIGS.with(|r| r.borrow_mut().push(Rc::downgrade(&inner)));
        Self { inner }
    }

    /// Every transfer frame from now on carries `flags` (NO REFERENCE: nothing on the
    /// reference input).
    pub fn set_tf_protection(&self, flags: ProtectionFlags) {
        self.inner.borrow_mut().protection = flags;
    }

    /// The health figures the keepalives carry from now on.
    pub fn set_health(&self, health: DaemonHealth) {
        self.inner.borrow_mut().health = health;
    }

    /// Measurement `name`'s peak mode (P) in the state the app is about to mirror: the rig
    /// answers no command, so a test sets it before the app's first pass.
    pub fn set_peak(&self, name: &str, mode: PeakMode) {
        let mut inner = self.inner.borrow_mut();
        assert!(!inner.connected, "the app has the state already");
        let state = Arc::make_mut(&mut inner.state);
        let m = state
            .measurements
            .iter_mut()
            .find(|m| m.config.name == name)
            .unwrap_or_else(|| panic!("no measurement {name}"));
        *m.config
            .kind
            .peak_mut()
            .unwrap_or_else(|| panic!("{name} takes no peak mode")) = mode;
    }

    /// The rigs the rig lists as paired, in the state the app is about to mirror.
    pub fn set_peers(&self, peers: Vec<RigPeer>) {
        let mut inner = self.inner.borrow_mut();
        assert!(!inner.connected, "the app has the state already");
        Arc::make_mut(&mut inner.state).remote.peers = peers;
    }

    /// Transfer measurement `name` at the `bins` resolution, unsmoothed, in the state the app
    /// is about to mirror: its `tf` frames state the 48 kHz standard ladder, and the rig
    /// sends each stage's bins the app asks for.
    pub fn set_bins(&self, name: &str) {
        let mut inner = self.inner.borrow_mut();
        assert!(!inner.connected, "the app has the state already");
        let state = Arc::make_mut(&mut inner.state);
        let m = state
            .measurements
            .iter_mut()
            .find(|m| m.config.name == name)
            .unwrap_or_else(|| panic!("no measurement {name}"));
        let MeasKind::Transfer { config } = &mut m.config.kind else {
            panic!("{name} is no transfer");
        };
        config.resolution = TransferResolution::Bins;
        // A smoothed curve keeps its grid: the bins are drawn unsmoothed only.
        config.smoothing = None;
    }

    /// The recorded state.
    pub fn state(&self) -> Arc<State> {
        Arc::clone(&self.inner.borrow().state)
    }

    /// The id of the measurement named `name`.
    pub fn id(&self, name: &str) -> MeasId {
        id_in(&self.inner.borrow().state, name)
    }

    /// The transfer grid the pictures' transfer curves are drawn on.
    pub fn tf_grid(&self) -> GridDef {
        self.inner.borrow().tf_grid.clone()
    }
}

fn id_in(s: &State, name: &str) -> MeasId {
    s.measurements
        .iter()
        .find(|m| m.config.name == name)
        .map(|m| m.id)
        .unwrap_or_else(|| panic!("no measurement {name}"))
}

/// Feeds every rig alive on this thread to `app`: the hello, device list and traces once,
/// then each call a keepalive's worth of mirror and a round of frames, as the link
/// delivers them (what the app's panes subscribe to only).
pub fn pump(app: &mut App) {
    let rigs: Vec<Rc<RefCell<Inner>>> =
        RIGS.with(|r| r.borrow().iter().filter_map(Weak::upgrade).collect());
    for rig in rigs {
        rig.borrow_mut().feed(app);
    }
}

impl Inner {
    fn feed(&mut self, app: &mut App) {
        let conn = |app: &mut App, e: ConnEvent| {
            let rig = app.state.rig.id;
            app.dispatch(Msg::Conn(rig, Box::new(e)));
        };
        let now = Instant::now();
        if !self.connected {
            conn(
                app,
                ConnEvent::Connected {
                    target: "recorded rig".into(),
                    server: "ac2d 0.0.0 (build test)".into(),
                    client_id: ClientId("ac2-ui test".into()),
                    rig_id: RIG_ID,
                    name: RIG_NAME.into(),
                },
            );
        }
        conn(app, ConnEvent::Mirror(Arc::new(self.mirror(now))));
        if !self.connected {
            self.connected = true;
            conn(app, ConnEvent::Devices(Ok(crate::recorded::backends())));
            for t in &self.traces {
                let grid = Arc::clone(&self.grids[&t.meta.grid_id]);
                conn(app, ConnEvent::Trace(Arc::new(t.clone()), grid));
            }
        }
        let wanted = app.state.wanted_topics();
        self.seq += 1;
        let mut latest = Latest {
            responding: true,
            ..Latest::default()
        };
        for (data, grid) in self.frames() {
            let topic = data.topic();
            if !wanted.contains(&topic) {
                continue;
            }
            // Protection describes a transfer measurement's inputs.
            let protection = match &data {
                FrameData::Tf(_) => self.protection,
                _ => ProtectionFlags::NONE,
            };
            let frame = Frame {
                stamp: self.stamp(grid, protection),
                data,
            };
            latest.frames.insert(
                topic.to_string().into(),
                TopicFrame {
                    topic,
                    frame: Arc::new(frame),
                    received: now,
                    since_new: Duration::ZERO,
                    age: Some(0.0),
                    stale: false,
                },
            );
        }
        let grids = latest
            .grid_ids()
            .into_iter()
            .map(|g| (g, Arc::clone(&self.grids[&g])))
            .collect();
        conn(
            app,
            ConnEvent::Data(Arc::new(DataSnapshot {
                latest,
                grids,
                drained: now,
            })),
        );
    }

    fn mirror(&self, now: Instant) -> MirrorView {
        let g = &self.state.generator;
        MirrorView {
            client_id: Some(ClientId("ac2-ui test".into())),
            phase: Phase::Live,
            incarnation: Some(INCARNATION),
            session_epoch: Some(self.state.session.epoch),
            rev: REV,
            state: Some(Arc::clone(&self.state)),
            last_ka: Some(now),
            connected: now,
            generator: Some(GenSummary {
                owner: g.owner.clone(),
                owner_local: g.owner_local,
                armed: g.armed,
                firing: g.firing,
            }),
            timing: Some(self.state.timing.state),
            health: Some(self.health.clone()),
            ka_rev: Some(REV),
            clock_offset_ns: Some(0),
            snapshots: 1,
            since_requests: 0,
            incarnation_changes: 0,
        }
    }

    fn stamp(&self, grid_id: Option<GridId>, protection: ProtectionFlags) -> FrameStamp {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        FrameStamp {
            seq: self.seq,
            audio_sample: SampleIndex(self.seq * 4800),
            session_epoch: self.state.session.epoch,
            daemon_incarnation: INCARNATION,
            config_rev: REV,
            config_applied_at: SampleIndex(0),
            capture_wall_ns: WallNs(now),
            grid_id,
            protection,
        }
    }

    /// One round of frames: each measurement the pictures know by name, with a shape of
    /// its own.
    fn frames(&self) -> Vec<(FrameData, Option<GridId>)> {
        let tf = self.tf_grid.id();
        let spec = self.spec_grid.id();
        let s = &self.state;
        let mut out = Vec::new();
        for m in s.measurements.iter().filter(|m| m.running) {
            let id = m.id.0;
            match m.config.name.as_str() {
                "Main L" => {
                    let mut f = self.tf(id, 0.0, 0.0, 2000.0);
                    if at_bins(m) {
                        f.meta.smoothing = None;
                        f.meta.bins = Some(Box::new(ladder()));
                        f.meta.unresolved = Some(ladder_unresolved());
                        out.extend(
                            (0..LADDER.len())
                                .map(|b| (FrameData::Bins(stage_frame(&f, b, 2000.0)), None)),
                        );
                    }
                    out.push((FrameData::Tf(f), Some(tf)));
                    out.push((FrameData::Ir(ir_frame(id)), None));
                }
                "Delay tower" => {
                    out.push((FrameData::Tf(self.tf(id, -4.0, 0.000_35, 900.0)), Some(tf)));
                }
                "Mic 1 FFT" => out.push((FrameData::Spec(self.spec(id)), Some(spec))),
                "FOH SPL" => out.push((FrameData::Spl(spl_frame(id)), None)),
                "Audience" => out.push((FrameData::Tf(self.average(m)), Some(tf))),
                "Mic 1 − Mic 2" => out.push((FrameData::Spec(self.difference(m)), Some(spec))),
                "Delay tower ↔ Main L" => {
                    out.push((FrameData::Tf(self.alignment(m)), Some(tf)));
                }
                "Main L after EQ 1" => out.push((FrameData::Tf(self.after_eq(m)), Some(tf))),
                _ => {}
            }
        }
        out
    }

    fn tf(&self, meas: u32, gain: f64, tau: f64, bump_hz: f64) -> TfFrame {
        let mut f = tf_frame(&self.tf_grid, meas, gain, tau, bump_hz);
        f.meta.delay = self
            .state
            .measurements
            .iter()
            .find(|m| m.id.0 == meas)
            .and_then(|m| m.delay.as_ref())
            .map_or(Seconds(0.0), |d| d.applied);
        f
    }

    fn spec(&self, meas: u32) -> SpecFrame {
        let mut f = spec_frame(&self.spec_grid, meas);
        if let Some(MeasKind::Spectrum { config }) = self
            .state
            .measurements
            .iter()
            .find(|m| m.id.0 == meas)
            .map(|m| &m.config.kind)
        {
            f.meta.smoothing = config.smoothing;
        }
        f
    }

    /// The power average's frame: its operands as the daemon says them, a stopped one left
    /// out.
    fn average(&self, m: &Measurement) -> TfFrame {
        let mut f = tf_frame(&self.tf_grid, m.id.0, -1.5, 0.0, 1300.0);
        f.meta.math = Some(Box::new(MathState {
            range: None,
            operands: self.operands(m),
            phase: PhaseBasis::SharedTimeBase,
            align: None,
        }));
        f
    }

    /// The level difference of two spectra: a gently rising few dB.
    fn difference(&self, m: &Measurement) -> SpecFrame {
        let mut f = spec_frame(&self.spec_grid, m.id.0);
        f.level = self
            .spec_grid
            .centres()
            .iter()
            .map(|x| (2.0 * (x.max(20.0) / 1000.0).log2()).clamp(-6.0, 6.0) as f32)
            .collect();
        f.meta.math = Some(Box::new(MathState {
            range: None,
            operands: self.operands(m),
            phase: PhaseBasis::NoPhase,
            align: None,
        }));
        f
    }

    /// Each operand of math trace `m`, included unless its measurement is stopped.
    fn operands(&self, m: &Measurement) -> Vec<OperandState> {
        let MeasKind::Math { config } = &m.config.kind else {
            panic!("{} is not math", m.config.name)
        };
        config
            .expr
            .operands()
            .into_iter()
            .map(|operand| {
                let running = match operand {
                    Operand::Meas { meas } => self
                        .state
                        .measurements
                        .iter()
                        .any(|x| x.id == meas && x.running),
                    _ => true,
                };
                OperandState {
                    operand,
                    status: if running {
                        OperandStatus::Included
                    } else {
                        OperandStatus::Stopped
                    },
                }
            })
            .collect()
    }

    /// The alignment of Delay tower and Main L in 60–120 Hz: Main L 4.21 ms late, the fit's
    /// curves where both play (30–400 Hz).
    fn alignment(&self, m: &Measurement) -> TfFrame {
        let freqs = self.tf_grid.centres();
        let tau = 0.00421;
        let both = |f: f64| (30.0..=400.0).contains(&f);
        let sum = |deg: f64| (20.0 * (deg.to_radians() / 2.0).cos().abs().log10()).max(-30.0);
        let before: Vec<f64> = freqs
            .iter()
            .map(|f| wrap(-360.0 * f * tau + 20.0))
            .collect();
        let after: Vec<f64> = freqs.iter().map(|f| 14.0 * (f / 25.0).ln().sin()).collect();
        let only = |v: &[f64], g: &dyn Fn(f64) -> f64| -> Vec<f32> {
            freqs
                .iter()
                .zip(v)
                .map(|(f, x)| if both(*f) { g(*x) as f32 } else { f32::NAN })
                .collect()
        };
        let curves = AlignCurves {
            phase_before: only(&before, &|x| x),
            phase_after: only(&after, &|x| x),
            sum_before: only(&before, &sum),
            sum_after: only(&after, &sum),
        };
        let candidate = |ms: f64, inverted: bool, res: f64, score: f64| AlignCandidate {
            delay: Seconds(ms / 1000.0),
            delayed: AlignSide::A,
            inverted,
            residual_deg: res,
            score,
        };
        let fit = AlignFit {
            best: candidate(4.21, false, 11.0, 0.98),
            other_polarity: Some(candidate(9.8, true, 41.0, 0.71)),
            runner_up: None,
            before: Summation {
                mean_db: -7.9,
                worst_db: -21.0,
            },
            after: Summation {
                mean_db: -0.3,
                worst_db: -1.2,
            },
            columns: 24,
        };
        let mut f = tf_frame(&self.tf_grid, m.id.0, 0.0, 0.0, 90.0);
        f.coh = vec![f32::NAN; f.coh.len()];
        f.meta.math = Some(Box::new(MathState {
            range: None,
            operands: self.operands(m),
            phase: PhaseBasis::SharedTimeBase,
            align: Some(Box::new(AlignState {
                outcome: AlignOutcome::Fit { fit },
            })),
        }));
        f.align = Some(curves);
        f
    }

    /// Main L's curve through its EQ set: the set's filter curve as the daemon stored it
    /// (the dashed curve the pictures show) added to Main L's shape.
    fn after_eq(&self, m: &Measurement) -> TfFrame {
        let curve = self
            .traces
            .iter()
            .find(|t| matches!(t.meta.source, TraceSource::Eq { .. }))
            .expect("the EQ set's curve");
        let main = id_in(&self.state, "Main L");
        let mut f = self.tf(main.0, 0.0, 0.0, 2000.0);
        f.meas = m.id;
        let phase = curve.phase_deg.as_deref().unwrap_or(&[]);
        for (i, (mag, ph)) in f.mag.iter_mut().zip(f.phase.iter_mut()).enumerate() {
            *mag += curve.mag_db.get(i).copied().unwrap_or(0.0);
            *ph = wrap(f64::from(*ph) + f64::from(phase.get(i).copied().unwrap_or(0.0))) as f32;
        }
        f
    }
}

fn wrap(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

/// The pictures' transfer magnitude at `f`: a bump at `bump_hz`, rolled off at both ends.
fn shape_db(f: f64, gain: f64, bump_hz: f64) -> f64 {
    let x = (f / bump_hz).log2();
    gain + 5.0 * (-x * x * 2.0).exp()
        - 12.0 * (60.0 / f).powi(4).min(1.0)
        - 6.0 * (f / 16_000.0).powi(2).min(1.0)
}

/// The 48 kHz standard ladder: per stage its bin spacing, first bin, bins, decimation and
/// hop.
const LADDER: [(f64, u32, u32, u32, i64); 3] = [
    (48_000.0 / 4096.0, 88, 1756, 1, 2048),
    (48_000.0 / 16_384.0, 88, 189, 4, 4096),
    (48_000.0 / 49_152.0, 1, 207, 12, 6144),
];

fn at_bins(m: &Measurement) -> bool {
    matches!(&m.config.kind, MeasKind::Transfer { config }
        if config.resolution == TransferResolution::Bins)
}

fn ladder() -> BinLayout {
    let range = |lo, hi| FreqRange {
        lo: Hz(lo),
        hi: Hz(hi),
    };
    BinLayout {
        stages: LADDER
            .iter()
            .map(|&(bin_hz, first_bin, bins, ..)| BinStage {
                first_bin,
                bins,
                bin_hz: Hz(bin_hz),
            })
            .collect(),
        blends: vec![range(811.0, 1031.25), range(202.2, 257.8)],
    }
}

/// Where the 1/96 columns are coarser than the ladder's bins: up to 138.5 bins of each stage.
fn ladder_unresolved() -> Vec<BinRange> {
    [
        (0.5, 135.3, LADDER[2].0),
        (202.2, 405.8, LADDER[1].0),
        (811.0, 1623.0, LADDER[0].0),
    ]
    .into_iter()
    .map(|(lo, hi, b)| BinRange {
        lo: Hz(lo),
        hi: Hz(hi),
        bin_hz: Hz(b),
    })
    .collect()
}

/// Stage `b`'s bins beside `tf`: its shape with a reflection's comb (a 1.5 dB ripple every
/// 150 Hz) that the 1/96 grid, wider than a period up high, averages away.
fn stage_frame(tf: &TfFrame, b: usize, bump_hz: f64) -> BinsFrame {
    let (bin_hz, first_bin, bins, factor, hop) = LADDER[b];
    let freqs: Vec<f64> = (first_bin..first_bin + bins)
        .map(|k| f64::from(k) * bin_hz)
        .collect();
    BinsFrame {
        meas: tf.meas,
        meta: BinsMeta {
            stage: b as u8,
            factor,
            rate: Hz(48_000.0 / f64::from(factor)),
            bin_hz: Hz(bin_hz),
            first_bin,
            hop: Samples(hop),
            delay: tf.meta.delay,
            nudged: tf.meta.nudged,
            mic_curve: false,
            mic_range: None,
            blocks: 8,
            eff_avg: 8.0,
        },
        mag: freqs
            .iter()
            .map(|f| {
                let comb = 1.5 * (2.0 * std::f64::consts::PI * f / 150.0).cos();
                (shape_db(*f, 0.0, bump_hz) + comb) as f32
            })
            .collect(),
        phase: freqs
            .iter()
            .map(|f| wrap(-40.0 * (bump_hz / f).min(3.0)) as f32)
            .collect(),
        coh: vec![0.97; freqs.len()],
    }
}

fn tf_frame(grid: &GridDef, meas: u32, gain: f64, tau: f64, bump_hz: f64) -> TfFrame {
    let freqs = grid.centres();
    let mag = freqs
        .iter()
        .map(|f| shape_db(*f, gain, bump_hz) as f32)
        .collect();
    let phase = freqs
        .iter()
        .map(|f| wrap(-360.0 * f * tau - 40.0 * (bump_hz / f).min(3.0)) as f32)
        .collect();
    let coh = freqs
        .iter()
        .map(|f| (1.0 - (35.0 / f).powi(2)).clamp(0.05, 0.99) as f32)
        .collect();
    TfFrame {
        range: None,
        meas: MeasId(meas),
        meta: TfMeta {
            delay: Seconds(0.0125),
            nudged: Seconds(0.0),
            smoothing: Some(Smoothing {
                fraction: SmoothingFraction::Sixth,
                mode: SmoothingMode::MagnitudePhase,
            }),
            mic_curve: false,
            mic_range: None,
            math: None,
            unresolved: None,
            bins: None,
            settling: None,
            filled: 1.0,
        },
        mag,
        phase,
        coh,
        validity: vec![ValidityMask::NONE; freqs.len()],
        align: None,
    }
}

fn ir_frame(meas: u32) -> IrFrame {
    let dt = 1.0 / 48_000.0;
    let n = 960;
    let linear = (0..n)
        .map(|i| {
            let t = (i as f64 - 96.0) * dt;
            if t < 0.0 {
                0.0
            } else {
                (0.8 * (-t / 0.004).exp() * (2.0 * std::f64::consts::PI * 900.0 * t).cos()
                    + 0.25 * (-(t - 0.006).powi(2) / 2e-8).exp()) as f32
            }
        })
        .collect();
    IrFrame {
        meas: MeasId(meas),
        meta: IrMeta {
            sample_rate: Hz(48_000.0),
            t0: Seconds(-96.0 * dt),
            dt: Seconds(dt),
            inserted_delay: Seconds(0.0125),
        },
        linear,
        etc: None,
    }
}

/// Pink-ish noise falling 3 dB an octave with a tone at 1 kHz.
fn spec_frame(grid: &GridDef, meas: u32) -> SpecFrame {
    let freqs = grid.centres();
    let level = freqs
        .iter()
        .map(|f| {
            let f = f.max(5.0);
            let tone = if (f - 1000.0).abs() < 6.0 { 30.0 } else { 0.0 };
            (-62.0 - 3.0 * (f / 1000.0).log2() + tone) as f32
        })
        .collect();
    SpecFrame {
        meas: MeasId(meas),
        meta: SpecMeta {
            window: Window::Hann,
            scale: LevelScale::Dbfs,
            level: SpecLevel::Tone,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            mic_range: None,
            smoothing: None,
            math: None,
            bands: None,
            harmonics: None,
        },
        level,
    }
}

/// The meter has run since [`SPL_SINCE`], as a real one runs since its start: its interval
/// grows, the heading's time stays put.
fn spl_frame(meas: u32) -> SplFrame {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    SplFrame {
        meas: MeasId(meas),
        meta: SplMeta {
            scale: LevelScale::Dbfs,
            weighting: Weighting::A,
            time_weighting: TimeWeighting::Fast,
            peak_weighting: PeakWeighting::C,
            level: -23.4,
            lmax: -18.2,
            lmin: -31.0,
            leq: -24.1,
            lpeak: -9.6,
            duration: Seconds(now.saturating_sub(SPL_SINCE.0) as f64 / 1e9),
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            position: None,
        },
    }
}
