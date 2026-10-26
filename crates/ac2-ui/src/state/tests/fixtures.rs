//! Fixtures for the state tests: the session dialog and its devices, sweeps, the mic
//! calibration views and SPL meters with their Leq data. `pub(super)`: every area of
//! `state::tests` reaches them through `super::*`.

use super::*;

pub(super) fn no_session_state() -> State {
    let mut s = empty_state();
    s.session.open = None;
    s
}

pub(super) fn backends(fake_names: bool) -> Vec<BackendInfo> {
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
        spare_channels: 0,
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
        clock_sources: vec![],
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

/// The session model of Settings (its Audio I/O page).
pub(super) fn dialog(t: &T) -> &SessionDialog {
    match t.st.overlay.settings() {
        Some(s) => &s.session,
        None => panic!("no Settings: {:?}", t.st.overlay),
    }
}

pub(super) fn form(t: &T) -> &crate::forms::Form {
    match &t.st.overlay {
        Overlay::Form(f) => f,
        other => panic!("no dialog open: {other:?}"),
    }
}

pub(super) fn created(r: &[Request]) -> Option<&MeasConfig> {
    r.iter().find_map(|r| match r {
        Request::CreateMeas { config } => Some(config),
        _ => None,
    })
}

pub(super) fn opened(r: &[Request]) -> Option<(&SessionConfig, &[InputSetup])> {
    r.iter().find_map(|r| match r {
        Request::OpenSession { config, inputs, .. } => Some((config, inputs.as_slice())),
        _ => None,
    })
}

/// Focuses the session dialog row `row` with ↓ from the top.
/// Moves to `row` by keys: Ctrl+PageUp / PageDown to the Audio I/O page, then ↓.
pub(super) fn focus(t: &mut T, row: Row) {
    let page = crate::settings::Page::AudioIo;
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
pub(super) fn open_dialog(t: &mut T, list: Vec<BackendInfo>) -> Vec<Request> {
    t.conn(mirror(no_session_state()));
    let mut r = t.type_key("Shift+O", "O");
    r.extend(t.conn(ConnEvent::Devices(Ok(list))));
    r
}

pub(super) fn sweep_run(status: SweepStatus) -> SweepRun {
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

pub(super) fn sweep_meta(id: u32) -> TraceMeta {
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

pub(super) fn sweep_data(id: u32) -> (Arc<TraceData>, Arc<GridDef>) {
    let grid = GridDef::Log {
        ppo: 12,
        k_min: -60,
        k_max: 50,
    };
    let n = grid.centres().len();
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
            reference_harmonics: None,
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
pub(super) fn mm1_state(curve: CurveChoice) -> State {
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.backend = BackendKind::Fake;
    }
    s.mics = vec![mm1()];
    s.inputs = vec![InputSetup {
        channel: 1,
        kind: InputKind::Mic {
            mic: Some("MM1 34804".into()),
            curve,
        },
        pair: None,
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

pub(super) fn cal_view(t: &T) -> &crate::cal_view::CalView {
    match t.st.overlay.cal() {
        Some(v) => v,
        None => panic!("no calibrations page: {:?}", t.st.overlay),
    }
}

pub(super) fn spl_meter() -> MeasKind {
    MeasKind::Spl {
        config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
    }
}

pub(super) fn with_spl() -> State {
    let mut s = daemon_state();
    s.measurements.push(meas(4, "FOH SPL", spl_meter()));
    s
}

/// A `leq` frame of meter 4 with `seq`, as the link delivers it.
pub(super) fn leq_data(
    seq: u64,
    at_s: u64,
    leq: f32,
    flags: ac2_proto::frame::LeqFlags,
) -> ConnEvent {
    leq_data_logged(seq, seq, at_s, leq, flags)
}

/// A `leq` frame of meter 4 with `seq`, of a log of `logged` rows.
pub(super) fn leq_data_logged(
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
