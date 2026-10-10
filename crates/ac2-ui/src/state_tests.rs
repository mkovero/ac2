//! Reducer tests: keyboard flows without a window or a daemon. This file holds the shared
//! helpers; the tests live in one `state_<area>_tests.rs` file per area.

use std::sync::Arc;
use std::time::Instant;

use ac2_client::fake::empty_state;
use ac2_client::{MirrorView, Phase};
use ac2_proto::Command;
use ac2_proto::model::*;
use ac2_proto::units::*;

use super::*;
use crate::conn::{ConnEvent, Request, StimEvent};
use crate::keys::{Chord, Keymap};
use crate::session_dialog::{InputRole, RoleKey, Row, SessionDialog};

fn meas(id: u32, name: &str, kind: MeasKind) -> Measurement {
    let delay = matches!(kind, MeasKind::Transfer { .. }).then(|| DelayState {
        applied: Seconds(0.0125),
        applied_samples: 600.0,
        nudged: Seconds(0.0),
        nudged_samples: 0.0,
        tracking: false,
        awaiting_pick: false,
        last_finding: None,
    });
    Measurement {
        id: MeasId(id),
        config: MeasConfig {
            name: name.into(),
            kind,
        },
        config_rev: Rev(1),
        running: true,
        delay,
        grid_id: None,
    }
}

fn transfer() -> MeasKind {
    MeasKind::Transfer {
        config: TransferConfig {
            reference_input: 0,
            measurement_input: 1,
            averaging: TfAveraging::Exponential {
                time_constant: Seconds(1.0),
            },
            resolution: Resolution::FortyEighth,
            smoothing: None,
            depth: ac2_proto::model::DepthPolicy::EqualConfidence,
        },
    }
}

fn spectrum() -> MeasKind {
    MeasKind::Spectrum {
        config: SpectrumConfig {
            input: 1,
            fft_len: 8192,
            window: Window::Hann,
            averaging: SpecAveraging::Off,
            smoothing: None,
        },
    }
}

fn daemon_state() -> State {
    let mut s = empty_state();
    s.measurements = vec![meas(2, "Sub", spectrum()), meas(1, "Main L", transfer())];
    s.session.open = Some(OpenSession {
        config: SessionConfig {
            backend: None,
            input_device: DeviceSelector::Default,
            output_device: DeviceSelector::Default,
            input_channels: vec![0, 1],
            output_channels: 2,
            sample_rate_hz: Some(48_000),
            buffer_frames: Some(256),
            loopback: None,
        },
        backend: BackendKind::Cpal,
        input_device: DeviceId("fake:loop".into()),
        output_device: DeviceId("fake:loop".into()),
        sample_rate_hz: 48_000,
        buffer_frames: 256,
        clock: ClockRelation::SingleCallback,
        opened_at: WallNs(0),
        replay: None,
    });
    s
}

fn mirror(state: State) -> ConnEvent {
    mirror_of(state, 1, Some("c1"))
}

fn mirror_of(state: State, incarnation: u64, me: Option<&str>) -> ConnEvent {
    ConnEvent::Mirror(Arc::new(MirrorView {
        client_id: me.map(|m| ClientId(m.into())),
        phase: Phase::Live,
        incarnation: Some(DaemonIncarnation(incarnation)),
        session_epoch: Some(SessionEpoch(2)),
        rev: Rev(10),
        state: Some(Arc::new(state)),
        last_ka: Some(Instant::now()),
        generator: None,
        timing: None,
        ka_rev: Some(Rev(10)),
        clock_offset_ns: Some(0),
        snapshots: 1,
        since_requests: 0,
        incarnation_changes: 0,
    }))
}

struct T {
    st: AppState,
    keys: Keymap,
}

/// Four panes the way most tests want them: transfer across the top, spectrum, a transfer
/// pane in its impulse-response view and SPL side by side below it; Alt+1..4 focus them in
/// that order.
fn grid() -> Layout {
    let leaf = |n| Box::new(PaneNode::Leaf(PaneId(n)));
    let root = PaneNode::Split {
        axis: Axis::Column,
        ratio: 0.62,
        a: leaf(1),
        b: Box::new(PaneNode::Split {
            axis: Axis::Row,
            ratio: 1.0 / 3.0,
            a: leaf(2),
            b: Box::new(PaneNode::Split {
                axis: Axis::Row,
                ratio: 0.5,
                a: leaf(3),
                b: leaf(4),
            }),
        }),
    };
    let mut ir = View::of(PaneKind::Transfer);
    ir.modes.transfer = TransferView::Ir;
    let views = [
        View::of(PaneKind::Transfer),
        View::of(PaneKind::Spectrum),
        ir,
        View::of(PaneKind::Spl),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, v)| (PaneId(i as u32 + 1), v))
    .collect();
    Layout::of(root, views, PaneId(1))
}

impl T {
    /// The four-pane grid on the test daemon.
    fn new() -> Self {
        let mut t = Self::fresh();
        t.st.layout = grid();
        t.st.pane_auto = false;
        t.connect(daemon_state());
        t
    }

    /// The app as first started, with no layout saved, before a daemon is known.
    fn fresh() -> Self {
        // Texts the tests compare name keys the PC way on every OS (macOS would print `⇧H`).
        crate::keys::set_label_style(crate::keys::LabelStyle::Pc);
        Self {
            st: AppState::default(),
            keys: Keymap::default(),
        }
    }

    fn connect(&mut self, state: State) {
        self.conn(ConnEvent::Connected {
            target: "local daemon".into(),
            server: "ac2d test".into(),
            client_id: ClientId("c1".into()),
        });
        self.conn(mirror(state));
    }

    /// The pane showing `kind` (the lead one when several do).
    fn pane(&self, kind: PaneKind) -> PaneId {
        self.st.layout.lead(kind).expect("a pane of the kind")
    }

    /// The pane in a transfer pane's IR view (the grid's third).
    fn ir_pane(&self) -> PaneId {
        let l = &self.st.layout;
        l.panes()
            .into_iter()
            .find(|id| l.view(*id).is_some_and(|v| v.shows_ir()))
            .expect("a transfer pane in its IR view")
    }

    /// The focus on the grid's IR view without a message.
    fn put_ir(&mut self) {
        let id = self.ir_pane();
        self.st.layout.set_focus(id);
    }

    /// What the focused pane shows.
    fn focus_kind(&self) -> PaneKind {
        self.st.layout.focus_kind()
    }

    /// The focused pane turned into `kind` through its menu.
    fn show(&mut self, kind: PaneKind) {
        let f = self.st.layout.focus;
        self.st
            .update(Msg::PanePick(f, PaneMenuRow::Kind(kind)), &self.keys);
    }

    /// The focus set on the pane of `kind` without a message; with none, one added as
    /// [`T::go`] does.
    fn put(&mut self, kind: PaneKind) {
        match self.st.layout.lead(kind) {
            Some(id) => self.st.layout.set_focus(id),
            None => self.go(kind),
        }
    }

    /// What the panes drawn now show, in reading order.
    fn visible(&self) -> Vec<PaneKind> {
        let l = &self.st.layout;
        self.st
            .visible_panes()
            .into_iter()
            .map(|id| l.kind(id))
            .collect()
    }

    /// Whether a pane shows `kind`.
    fn shown(&self, kind: PaneKind) -> bool {
        self.st.layout.lead(kind).is_some()
    }

    /// The pane of `kind` focused as a click would; with none, the last pane in reading
    /// order split and the new half turned into it (so on the grid Alt+5 reaches it).
    fn go(&mut self, kind: PaneKind) {
        if let Some(id) = self.st.layout.lead(kind) {
            self.st.update(Msg::FocusPane(id), &self.keys);
            return;
        }
        let last = *self.st.layout.root.reading_order().last().expect("a pane");
        self.st.update(Msg::FocusPane(last), &self.keys);
        self.key("N");
        self.show(kind);
    }

    fn disconnected() -> Self {
        let mut t = Self::fresh();
        t.st.layout = grid();
        t.st.pane_auto = false;
        t
    }

    fn conn(&mut self, e: ConnEvent) -> Vec<Request> {
        self.st.update(Msg::Conn(Box::new(e)), &self.keys)
    }

    fn key(&mut self, s: &str) -> Vec<Request> {
        let c = Chord::parse(s).expect(s);
        self.st.update(Msg::Key(c), &self.keys)
    }

    /// A key press followed by the text it types, as egui delivers it.
    fn type_key(&mut self, s: &str, text: &str) -> Vec<Request> {
        let mut r = self.key(s);
        r.extend(self.st.update(Msg::Text(text.into()), &self.keys));
        r
    }

    fn text(&mut self, s: &str) {
        self.st.update(Msg::Text(s.into()), &self.keys);
    }

    fn last_toast(&self) -> &str {
        self.st.toasts.last().map_or("", |t| t.text.as_str())
    }
}

fn set_level(r: &Request) -> Option<(f64, bool, bool)> {
    match r {
        Request::StimSet(d) => Some((d.settings.level.0, d.armed, d.firing)),
        _ => None,
    }
}

fn rta() -> MeasKind {
    MeasKind::Rta {
        config: RtaConfig::on_input(1, BandFraction::Third),
    }
}

/// Two transfer measurements, a spectrum and an RTA.
fn four() -> State {
    let mut s = daemon_state();
    s.measurements.push(meas(3, "Delay tower", transfer()));
    s.measurements.push(meas(4, "Room", rta()));
    s
}

fn smoothing(f: SmoothingFraction, mode: SmoothingMode) -> Option<Smoothing> {
    Some(Smoothing { fraction: f, mode })
}

/// The TraceUpdate a reply carries, with its toast text.
fn trace_update(r: &[Request]) -> (TraceId, TraceEdit, String) {
    match r {
        [
            Request::Call {
                cmd: Command::TraceUpdate { trace, edit },
                what,
            },
        ] => (*trace, edit.clone(), what.clone()),
        other => panic!("{other:?}"),
    }
}

/// A capture filed under Main L (1), as the daemon files captures: the transfer pane draws
/// it while it shows Main L.
fn captured(id: u32, slot: Option<u8>, epoch: u32) -> TraceMeta {
    let mut t = stored(id, slot, epoch);
    t.edit.owner = TraceOwner::Meas { meas: MeasId(1) };
    t
}

/// A stored transfer trace under Imported: the transfer pane leaves it out ([`captured`] is
/// one it draws while it shows Main L).
fn stored(id: u32, slot: Option<u8>, epoch: u32) -> TraceMeta {
    TraceMeta {
        id: TraceId(id),
        edit: TraceEdit {
            owner: ac2_proto::model::TraceOwner::Imported,
            name: format!("t{id}"),
            color: Rgb { r: 1, g: 2, b: 3 },
            visible: true,
            locked: false,
            order: id,
            offset: Db(0.0),
            polarity: Polarity::Normal,
            delay_nudge: Seconds(0.0),
            slot,
            smoothing: None,
        },
        kind: TraceKind::Transfer,
        source: TraceSource::Captured {
            meas: MeasId(1),
            meas_name: "Main L".into(),
            epoch: SessionEpoch(epoch),
            at_sample: SampleIndex(0),
        },
        grid_id: ac2_proto::GridId(1),
        delay: Seconds(0.0),
        depth: Some(DepthPolicy::EqualConfidence),
        cal: CalState::Uncalibrated,
        mic: None,
        mic_curve: None,
        created_at: WallNs(0),
    }
}

fn curve_ref(label: &str) -> MicCurveRef {
    MicCurveRef {
        label: label.into(),
        file_name: format!("{label}.txt"),
        content_hash: "0".into(),
        points: 2,
        f_lo: Hz(20.0),
        f_hi: Hz(20_000.0),
        imported_at: WallNs(0),
        stated_sensitivity: Some(15.0),
    }
}

/// MM1 34804 with its 0° and 90° curves.
fn mm1() -> Mic {
    Mic {
        name: "MM1 34804".into(),
        curves: vec![curve_ref("0°"), curve_ref("90°")],
    }
}

fn with_traces(traces: Vec<TraceMeta>) -> ConnEvent {
    let mut s = daemon_state();
    s.traces = traces;
    mirror(s)
}

impl T {
    fn tick(&mut self, now_s: f64) {
        let keys = self.keys.clone();
        self.st.update(Msg::Tick { now_s, dt_s: 0.016 }, &keys);
    }
}

fn arrival(ms: f64, level: f64) -> DelayArrival {
    DelayArrival {
        delay: Seconds(ms / 1000.0),
        delay_samples: ms * 48.0,
        level: Db(level),
        phase: Degrees(0.0),
        uncertainty_samples: 0.2,
        misfit: 0.0,
        refined: true,
    }
}

fn finding(outcome: DelayOutcome) -> Box<DelayFinding> {
    Box::new(DelayFinding {
        outcome,
        confidence: DelayConfidence {
            psr_db: Some(Db(20.0)),
            psr_acq_db: None,
            band_snr_db: Some(Db(25.0)),
            excited_fraction: Some(1.0),
            uncertainty_samples: Some(0.2),
            pulse_width_samples: None,
            period: None,
        },
        band: DelayBand::Full,
        observation: Seconds(0.25),
        candidates: vec![],
        found_at: WallNs(0),
    })
}

fn ambiguous() -> Box<DelayFinding> {
    finding(DelayOutcome::Ambiguous {
        reasons: vec![AmbiguityReason::BorderlineLevel],
        ranked: vec![
            arrival(12.5, -11.5),
            arrival(12.7, 0.0),
            arrival(13.4, -6.0),
        ],
        strongest: arrival(12.7, 0.0),
    })
}

fn found(t: &mut T, pick: DelayPick, f: Box<DelayFinding>) -> Vec<Request> {
    t.conn(ConnEvent::DelayFound {
        meas: MeasId(1),
        pick,
        finding: f,
    })
}

fn inserted(r: &[Request]) -> Option<DelayPick> {
    match r {
        [
            Request::Call {
                cmd:
                    Command::DelayInsert {
                        meas: MeasId(1),
                        pick,
                    },
                ..
            },
        ] => Some(*pick),
        _ => None,
    }
}

fn prompt_text(t: &mut T, c: CommandId, text: &str) -> Vec<Request> {
    t.st.update(Msg::Command(c), &t.keys);
    if let Overlay::Prompt(p) = &mut t.st.overlay {
        p.text.clear();
    }
    t.text(text);
    t.key("Enter")
}

fn no_session_state() -> State {
    let mut s = empty_state();
    s.session.open = None;
    s
}

fn backends(fake_names: bool) -> Vec<BackendInfo> {
    let dir = |ch, names: Option<Vec<&str>>| DirectionInfo {
        max_channels: ch,
        rates_hz: vec![RangeU32 {
            min: 48_000,
            max: 48_000,
        }],
        buffer_frames: Some(RangeU32 { min: 256, max: 256 }),
        default_rate_hz: Some(48_000),
        default_buffer_frames: Some(256),
        channel_names: names.map(|n| n.into_iter().map(String::from).collect()),
        system_default: true,
    };
    let dev = |backend, id: &str, names: bool| DeviceInfo {
        backend,
        host: "test".into(),
        id: DeviceId(id.into()),
        name: id.into(),
        input: Some(dir(
            4,
            names.then(|| vec!["Loop return", "Room mic", "Line 3", "Line 4"]),
        )),
        output: Some(dir(2, None)),
        duplex_clock: ClockRelation::SingleCallback,
        index: IndexExactness::Exact,
        notes: vec![],
    };
    vec![
        BackendInfo {
            kind: BackendKind::Fake,
            description: "Simulated rig".into(),
            availability: Availability::Available,
            devices: vec![dev(BackendKind::Fake, "fake:loop", fake_names)],
        },
        BackendInfo {
            kind: BackendKind::Jack,
            description: "JACK".into(),
            availability: Availability::Unavailable {
                reason: "JACK server not running".into(),
            },
            devices: vec![],
        },
    ]
}

/// The session model of Settings (its Inputs & outputs and Audio pages).
fn dialog(t: &T) -> &SessionDialog {
    match t.st.overlay.settings() {
        Some(s) => &s.session,
        None => panic!("no Settings: {:?}", t.st.overlay),
    }
}

fn form(t: &T) -> &crate::forms::Form {
    match &t.st.overlay {
        Overlay::Form(f) => f,
        other => panic!("no dialog open: {other:?}"),
    }
}

fn created(r: &[Request]) -> Option<&MeasConfig> {
    r.iter().find_map(|r| match r {
        Request::CreateMeas { config } => Some(config),
        _ => None,
    })
}

fn opened(r: &[Request]) -> Option<(&SessionConfig, &[InputSetup], &[MeasConfig])> {
    r.iter().find_map(|r| match r {
        Request::OpenSession {
            config,
            inputs,
            transfers,
            ..
        } => Some((config, inputs.as_slice(), transfers.as_slice())),
        _ => None,
    })
}

/// Focuses the session dialog row `row` with ↓ from the top.
/// Moves to `row` by keys: Ctrl+PageUp / PageDown to its page of Settings, then ↓.
fn focus(t: &mut T, row: Row) {
    let page = match row {
        Row::Input(_) | Row::Output(_) => crate::settings::Page::Io,
        _ => crate::settings::Page::Audio,
    };
    for _ in 0..8 {
        if t.st.overlay.settings().is_some_and(|s| s.page == page) {
            break;
        }
        t.key("Ctrl+PageDown");
    }
    for _ in 0..64 {
        if dialog(t).focus == row && t.st.overlay.settings().is_some_and(|s| !s.on_ceiling) {
            return;
        }
        t.key("Down");
    }
    panic!("row {row:?} not reached");
}

/// Shift+O on a daemon without a session, the simulated rig listed.
fn open_dialog(t: &mut T, list: Vec<BackendInfo>) -> Vec<Request> {
    t.conn(mirror(no_session_state()));
    let mut r = t.type_key("Shift+O", "O");
    r.extend(t.conn(ConnEvent::Devices(Ok(list))));
    r
}

fn sweep_run(status: SweepStatus) -> SweepRun {
    SweepRun {
        id: SweepId(4),
        meas: MeasId(5),
        owner: ClientId("c1".into()),
        name: "Run 1".into(),
        reference_input: 0,
        measurement_input: 1,
        outputs: vec![0],
        level: Dbfs(-50.0),
        sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
        sweep_duration: Seconds(3.1),
        post_roll: Seconds(1.0),
        repeats: 1,
        gate: None,
        lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
        resolution: ac2_proto::model::Resolution::FortyEighth,
        status,
        started_at: WallNs(0),
    }
}

fn sweep_meta(id: u32) -> TraceMeta {
    TraceMeta {
        kind: TraceKind::Sweep,
        edit: TraceEdit {
            owner: TraceOwner::Meas { meas: MeasId(5) },
            ..stored(id, None, 2).edit
        },
        source: TraceSource::Sweep {
            meas: MeasId(5),
            meas_name: "Sweep 1".into(),
            number: 1,
            run: SweepId(4),
            epoch: SessionEpoch(2),
            sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
            level: Dbfs(-50.0),
            repeats: 1,
            lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
            reference_input: 0,
            measurement_input: 1,
        },
        depth: None,
        ..stored(id, None, 2)
    }
}

fn sweep_data(id: u32) -> (Arc<TraceData>, Arc<GridDef>) {
    let grid = GridDef::Log {
        ppo: 12,
        k_min: -60,
        k_max: 50,
    };
    let n = ac2_scene::grid::column_frequencies(&grid).len();
    let curve = |l: f32| DistortionCurve {
        level_db: vec![l; n],
        floor_db: vec![-80.0; n],
    };
    let data = TraceData {
        meta: sweep_meta(id),
        mag_db: vec![-6.0; n],
        phase_deg: Some(vec![0.0; n]),
        coherence: None,
        ir: None,
        sweep: Some(SweepData {
            unresolved: None,
            harmonics: vec![HarmonicCurve {
                order: 2,
                curve: curve(-40.0),
            }],
            thd: curve(-40.0),
            ir: TraceIr {
                t0: Seconds(-0.1),
                dt: Seconds(0.001),
                linear: vec![0.0; 200],
                etc_db: vec![-90.0; 200],
            },
            info: SweepInfo {
                sample_rate: Hz(48_000.0),
                rate: Seconds(0.45),
                duration: Seconds(3.1),
                repeats: 1,
                arrival: Seconds(0.003),
                reference_level: Db(0.0),
                window_pre: Seconds(0.008),
                window_post: Seconds(0.09),
                gate_pre: Seconds(0.03),
                gate: Seconds(0.9),
                floor_margin: Db(6.0),
                clipped: false,
            },
            room: None,
        }),
    };
    (Arc::new(data), Arc::new(grid))
}

/// The daemon state with MM1 34804 (0° / 90°) on input 2 choosing `curve`, a sensitivity
/// calibration of it, the session on the simulated rig.
fn mm1_state(curve: CurveChoice) -> State {
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.backend = BackendKind::Fake;
    }
    s.mics = vec![mm1()];
    s.inputs = vec![InputSetup {
        channel: 1,
        mic: Some("MM1 34804".into()),
        curve,
    }];
    s.calibrations = vec![CalEntry {
        key: CalKey {
            device: DeviceId("fake:loop".into()),
            channel: 1,
            mic: "MM1 34804".into(),
        },
        spl: SplCal {
            sensitivity: Db(130.0),
            method: ac2_proto::model::CalMethod::Acoustic {
                calibrator_level: DbSpl(94.0),
            },
            freq: Hz(1000.0),
            measured: Dbfs(-36.0),
            calibrated_at: WallNs(0),
        },
    }];
    s
}

fn cal_view(t: &T) -> &crate::cal_view::CalView {
    match t.st.overlay.cal() {
        Some(v) => v,
        None => panic!("no calibrations page: {:?}", t.st.overlay),
    }
}

fn spl_meter() -> MeasKind {
    MeasKind::Spl {
        config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
    }
}

fn with_spl() -> State {
    let mut s = daemon_state();
    s.measurements.push(meas(4, "FOH SPL", spl_meter()));
    s
}

/// A `leq` frame of meter 4 with `seq`, as the link delivers it.
fn leq_data(seq: u64, at_s: u64, leq: f32, flags: ac2_proto::frame::LeqFlags) -> ConnEvent {
    leq_data_logged(seq, seq, at_s, leq, flags)
}

/// A `leq` frame of meter 4 with `seq`, of a log of `logged` rows.
fn leq_data_logged(
    seq: u64,
    logged: u64,
    at_s: u64,
    leq: f32,
    flags: ac2_proto::frame::LeqFlags,
) -> ConnEvent {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{Frame, FrameData, LeqFrame, LeqMeta, LeqRun};
    let n = 5;
    let data = FrameData::Leq(Box::new(LeqFrame {
        meas: MeasId(4),
        meta: LeqMeta {
            scale: LevelScale::DbSpl,
            cal: CalStatus::Verified {
                calibrated_at: WallNs(1),
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0),
                },
            },
            mic_curve: false,
            horizon: Seconds(60.0),
            logged,
            // A log of `logged` seconds up to `at_s`.
            run: Some(LeqRun {
                started_at: WallNs(at_s.saturating_sub(logged) * 1_000_000_000),
                until: WallNs(at_s * 1_000_000_000),
                measured: Seconds(logged as f64),
                gaps: Seconds(0.0),
                trimmed: false,
                laeq: f64::from(leq),
                lceq: f64::from(leq) + 3.0,
                lzeq: f64::from(leq) + 5.0,
            }),
            lcpeak: None,
            lafmax: None,
            position: None,
        },
        leq: vec![leq; n],
        elapsed: vec![60.0; n],
        measured: vec![60.0; n],
        allowed: vec![f32::NAN; n],
        recover: vec![f32::NAN; n],
        least: vec![leq; n],
        over_in: vec![f32::NAN; n],
        flags: vec![flags; n],
    }));
    let mut stamp = ac2_proto::samples::stamp(None);
    stamp.seq = seq;
    stamp.capture_wall_ns = WallNs(at_s * 1_000_000_000);
    let f = TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame { stamp, data }),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    latest.frames.insert(f.topic.to_string().into(), f);
    ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: Default::default(),
        drained: Instant::now(),
    }))
}

#[path = "state_display_tests.rs"]
mod display;

#[path = "state_overlay_tests.rs"]
mod overlay;

#[path = "state_ir_tests.rs"]
mod ir_nav_tests;

#[path = "state_settings_tests.rs"]
mod settings;

/// The measurement tree as the operator sees it: Main L with two captures and a math
/// channel made on it, the spectrum, an import; captures stay under Main L.
fn tree_state() -> State {
    let mut s = daemon_state();
    let main = TraceOwner::Meas { meas: MeasId(1) };
    let capture = |id: u32, name: &str| {
        let mut t = stored(id, None, 2);
        t.edit.name = name.into();
        t.edit.owner = main;
        t
    };
    let mut imported = stored(5, None, 2);
    imported.edit.name = "1083 94cm".into();
    s.traces = vec![capture(3, "pre-EQ"), capture(4, "post-EQ"), imported];
    s.measurements.push(meas(
        6,
        "pre ÷ post",
        MeasKind::Math {
            config: MathConfig::of(
                main,
                MathDomain::Transfer,
                MathExpr::Binary {
                    a: Operand::Trace { trace: TraceId(3) },
                    op: MathOp::Divide,
                    b: Operand::Trace { trace: TraceId(4) },
                },
            ),
        },
    ));
    s
}

#[path = "state_stimulus_tests.rs"]
mod stimulus;

#[path = "state_traces_tests.rs"]
mod traces;

#[path = "state_inputs_tests.rs"]
mod inputs;

#[path = "state_session_tests.rs"]
mod session;

#[path = "state_sweep_tests.rs"]
mod sweep;

#[path = "state_leq_tests.rs"]
mod leq;

#[path = "state_layout_tests.rs"]
mod layout;

#[path = "state_tf_group_tests.rs"]
mod tf_group;

#[path = "state_panes_tests.rs"]
mod panes;
