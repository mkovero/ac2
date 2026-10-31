//! The session, its meters and logs, and the full state.

use super::builders::*;
use super::traces::*;
use crate::model::*;
use crate::units::*;

/// A network-mode daemon's `server.info`.
pub fn server_info() -> ServerInfo {
    ServerInfo {
        mode: ServerMode::Network {
            ctrl: "tcp://0.0.0.0:47820".into(),
            data: "tcp://0.0.0.0:47821".into(),
            server_key: "rq:rM>}U?@Lns47E1%kR.o@n%FcmmsL/@{H8]yf7".into(),
            fingerprint: "SHA256:3f1c 9a2e 77b0 51d4".into(),
            advertised_as: Some("foh-rig".into()),
            authorized: vec![AuthorizedClient {
                name: "laptop".into(),
                key: "Yne@$w-vo<fVvi]a<NY6T1ed:M$fCG*[IaLV{hID".into(),
                fingerprint: "SHA256:b2aa 0c3d 9e41 7f60".into(),
            }],
            refused: vec![
                RefusedKey {
                    key: Some("D:)Q[IlAW!ahhC2ac:9*A}h:p?([4%wOTJ%JR%cs".into()),
                    fingerprint: Some("SHA256:51e0 c2b9 aa13 0d77".into()),
                    address: "192.168.1.40".into(),
                    count: 12,
                    last_at: WallNs(1_790_000_000_000_000_000),
                },
                RefusedKey {
                    key: None,
                    fingerprint: None,
                    address: "192.168.1.41".into(),
                    count: 1,
                    last_at: WallNs(1_790_000_000_000_000_000),
                },
            ],
        },
        recording_dir: Some("/home/fohtech/.local/share/ac2/recordings".into()),
    }
}

pub(super) fn inputs() -> InputSetups {
    InputSetups {
        inputs: vec![
            InputSetup {
                channel: 1,
                kind: InputKind::Mic {
                    mic: Some("M30 #1234".into()),
                    curve: CurveChoice::Curve {
                        label: "0°".into()
                    },
                },
                pair: None,
            },
            InputSetup {
                channel: 2,
                kind: InputKind::Mic {
                    mic: None,
                    curve: CurveChoice::NotChosen,
                },
                pair: None,
            },
            InputSetup {
                channel: 3,
                kind: InputKind::Mic {
                    mic: Some("ECM".into()),
                    curve: CurveChoice::Off,
                },
                pair: None,
            },
            InputSetup {
                channel: 4,
                kind: InputKind::Line,
                pair: Some(PairSide::Left),
            },
            InputSetup {
                channel: 5,
                kind: InputKind::Line,
                pair: Some(PairSide::Right),
            },
            InputSetup {
                channel: 6,
                kind: InputKind::Digital,
                pair: None,
            },
        ],
        pairs: vec![Pair {
            left: 4,
            right: 5,
            name: Some("Main".into()),
        }],
    }
}

pub(super) fn session() -> Session {
    Session {
        epoch: SessionEpoch(2),
        open: Some(OpenSession {
            config: session_config(),
            backend: BackendKind::CoreAudio,
            input_device: DeviceId("hw:UMC1820".into()),
            output_device: DeviceId("hw:UMC1820".into()),
            sample_rate_hz: 48_000,
            buffer_frames: 256,
            clock: ClockRelation::SingleCallback,
            opened_at: WallNs(1_789_999_000_000_000_000),
            replay: None,
            clock_source: Some("Internal".into()),
            driver_change: Some(DriverChange {
                from_rate_hz: 44_100,
                from_buffer_frames: 512,
            }),
            driver_reset: Some(DriverReset {
                reason: DriverResetReason::ResyncRequest,
                output_stopped: true,
            }),
            stated_latency: Some(StatedLatency {
                input_frames: 288,
                output_frames: 320,
                driver_buffering: Some(DriverBuffering {
                    input_frames: 32,
                    output_frames: 64,
                }),
            }),
            spare_ports: vec![],
        }),
        stopped: Some(AudioStopped {
            since: WallNs(1_789_999_500_000_000_000),
            cause: StopCause::NotDelivering { after_ms: 1000 },
            recovery: Recovery::Waiting {
                attempt: 3,
                error: "No JACK server: start JACK".into(),
                next_at: WallNs(1_789_999_508_000_000_000),
            },
        }),
    }
}

/// A session replaying a recording.
pub(super) fn replay_session() -> Session {
    let mut s = session();
    if let Some(o) = &mut s.open {
        o.backend = BackendKind::Replay;
        o.config.backend = Some(BackendKind::Replay);
        o.config.output_channels = 0;
        o.config.loopback = None;
        o.replay = Some(ReplayInfo {
            name: "soundcheck".into(),
            path: "/home/op/.local/share/ac2/recordings/soundcheck.wav".into(),
            frames: 480_000,
            end_sample: SampleIndex(481_000),
            pace: ReplayPace::Fast,
            recorded_start_sample: SampleIndex(96_000),
            recorded_at: WallNs(1_789_999_100_000_000_000),
        });
    }
    s
}

/// A recording that ended at its duration limit.
pub fn recording_run() -> RecordingRun {
    RecordingRun {
        name: "soundcheck".into(),
        path: "/home/op/.local/share/ac2/recordings/soundcheck.wav".into(),
        inputs: vec![0, 1],
        sample_rate_hz: 48_000,
        session_epoch: SessionEpoch(2),
        start_sample: SampleIndex(96_000),
        started_at: WallNs(1_789_999_100_000_000_000),
        started_by: ClientId("alice".into()),
        frames: 480_000,
        bytes: 3_840_116,
        discontinuities: 1,
        max_duration: Seconds(10.0),
        max_bytes: Some(1 << 30),
        status: RecordingStatus::Ended {
            reason: RecordingEnd::DurationLimit,
        },
    }
}

pub(super) fn recording_file() -> RecordingFile {
    RecordingFile {
        name: "soundcheck".into(),
        path: "/home/op/.local/share/ac2/recordings/soundcheck.wav".into(),
        sample_rate_hz: 48_000,
        inputs: vec![0, 1],
        frames: 480_000,
        started_at: WallNs(1_789_999_100_000_000_000),
        discontinuities: 1,
        end: Some(RecordingEnd::WriteFailed {
            msg: "No space left on device (os error 28)".into(),
        }),
    }
}

pub(super) fn timing() -> TimingStatus {
    TimingStatus {
        epoch: 3,
        state: TimingState::Locked {
            offset: Samples(312),
        },
        last_lock: Some(LastLock {
            epoch: 3,
            offset: Samples(312),
            at_sample: SampleIndex(96_000),
            at: WallNs(1_790_000_000_000_000_000),
        }),
        drift: Some(Drift {
            ppm: 0.4,
            span: Seconds(30.0),
            warning: false,
            at: WallNs(1_790_000_000_500_000_000),
        }),
        internal_reference: true,
    }
}

/// An SPL meter with two Leq windows, one limited.
pub fn spl_config() -> SplConfig {
    SplConfig {
        cue: Some(Box::new(light_cue())),
        input: 1,
        weighting: Weighting::A,
        time_weighting: TimeWeighting::Fast,
        peak_weighting: PeakWeighting::C,
        leq: LeqConfig {
            windows: vec![
                LeqWindow::minutes(1),
                LeqWindow {
                    cue: None,
                    duration: Seconds(1800.0),
                    weighting: Weighting::A,
                    limit: Some(DbSpl(99.0)),
                    warn_margin: Db(3.0),
                },
            ],
            horizon: Seconds(60.0),
            peaks: PeakLimits {
                lcpeak: Some(PeakLimit {
                    cue: None,
                    limit: DbSpl(135.0),
                    warn_margin: Db(3.0),
                }),
                lafmax: None,
            },
        },
        position: Some(PositionCorrection {
            level: Db(2.5),
            peak: Db(1.5),
        }),
        bands: Some(Box::new(band_leq_config())),
    }
}

/// A band meter on the `finland-545-lf` limits with a transfer of every band status and a
/// §13 impulse correction in force.
pub fn band_leq_config() -> BandLeqConfig {
    let mut bands = [BandTransferBand::Missing; BAND_COUNT];
    bands[0] = BandTransferBand::Unchecked {
        attenuation: Db(20.0),
    };
    bands[1] = BandTransferBand::Clean {
        attenuation: Db(25.5),
    };
    bands[2] = BandTransferBand::Corrected {
        attenuation: Db(30.0),
        margin: Db(5.5),
    };
    bands[3] = BandTransferBand::Unusable { at_least: Db(35.0) };
    let mut cfg = BandLeqPreset::Finland545Lf.apply(None);
    cfg.windows.push(BandWindow {
        cue: Some(Box::new(light_cue())),
        band: Hz(1000.0),
        duration: Seconds(900.0),
        weighting: Weighting::A,
        limit: Some(DbSpl(30.5)),
        day_offset: None,
        warn_margin: Db(2.0),
    });
    BandLeqConfig {
        correction: BandCorrection {
            impulse: ImpulseCorrection::Plus5,
            tonal: TonalCorrection::None,
        },
        transfer: Some(BandTransferSet {
            place: "flat 4 bedroom".into(),
            measured_at: WallNs(1_789_500_000_000_000_000),
            origin: TransferOrigin::Measured,
            bands,
        }),
        ..cfg
    }
}

/// A running SPL meter.
pub fn spl_measurement() -> Measurement {
    Measurement {
        id: MeasId(4),
        config: MeasConfig {
            scope: crate::model::MeasScope::Session,
            name: "FOH SPL".into(),
            kind: MeasKind::Spl {
                config: spl_config(),
            },
        },
        config_rev: Rev(40),
        running: true,
        delay: None,
        grid_id: None,
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: Db(0.0),
        eq: Vec::new(),
    }
}

pub(super) fn spl_log() -> SplLog {
    SplLog {
        meas: MeasId(4),
        started_at: Some(WallNs(1_790_000_000_000_000_000)),
        windows: vec![
            LeqWindowState {
                duration: Seconds(60.0),
                weighting: Weighting::A,
                judgement: LeqJudgement::NoLimit,
                since: WallNs(1_790_000_000_000_000_000),
            },
            LeqWindowState {
                duration: Seconds(1800.0),
                weighting: Weighting::A,
                judgement: LeqJudgement::Over,
                since: WallNs(1_790_000_600_000_000_000),
            },
        ],
        peaks: PeakStates {
            lcpeak: LeqPeakState {
                judgement: LeqJudgement::Near,
                since: WallNs(1_790_000_500_000_000_000),
            },
            lafmax: LeqPeakState {
                judgement: LeqJudgement::NoLimit,
                since: WallNs(1_790_000_000_000_000_000),
            },
        },
        alarms: vec![
            LeqAlarm {
                at: WallNs(1_790_000_600_000_000_000),
                subject: AlarmSubject::Window {
                    duration: Seconds(1800.0),
                    weighting: Weighting::A,
                },
                kind: LeqAlarmKind::Over,
                level: DbSpl(99.25),
                limit: DbSpl(99.0),
                position: Some(Db(2.5)),
            },
            LeqAlarm {
                at: WallNs(1_790_000_610_000_000_000),
                subject: AlarmSubject::Peak {
                    quantity: PeakQuantity::LcPeak,
                },
                kind: LeqAlarmKind::Recovered,
                level: DbSpl(133.5),
                limit: DbSpl(135.0),
                position: None,
            },
            LeqAlarm {
                at: WallNs(1_790_000_620_000_000_000),
                subject: AlarmSubject::Band {
                    duration: Seconds(3600.0),
                    weighting: Weighting::Z,
                    nominal: Hz(63.0),
                },
                kind: LeqAlarmKind::Over,
                level: DbSpl(82.5),
                limit: DbSpl(80.0),
                position: None,
            },
            LeqAlarm {
                at: WallNs(1_790_000_630_000_000_000),
                subject: AlarmSubject::Predicted,
                kind: LeqAlarmKind::Over,
                level: DbSpl(26.5),
                limit: DbSpl(25.0),
                position: None,
            },
        ],
    }
}

pub(super) fn spl_history() -> SplHistory {
    SplHistory {
        meas: MeasId(4),
        windows: vec![
            LeqWindow::minutes(1),
            LeqWindow {
                limit: Some(DbSpl(99.0)),
                ..LeqWindow::minutes(30)
            },
        ],
        scale: LevelScale::DbSpl,
        step_s: 1,
        log_start: Some(WallNs(1_790_000_000_000_000_000)),
        log_until: Some(WallNs(1_790_000_122_000_000_000)),
        at: vec![
            WallNs(1_790_000_121_000_000_000),
            WallNs(1_790_000_122_000_000_000),
        ],
        leq: vec![vec![98.5, 99.75], vec![99.0, 99.25]],
        over: vec![vec![false, false], vec![false, true]],
    }
}

pub(super) fn spl_band_log() -> SplBandLog {
    let mut levels = [None; BAND_COUNT];
    levels[0] = Some(71.25);
    levels[5] = Some(80.5);
    let mut average = [None; BAND_COUNT];
    average[0] = Some(DbSpl(70.75));
    average[5] = Some(DbSpl(80.0));
    SplBandLog {
        meas: MeasId(4),
        from: WallNs(1_789_500_000_000_000_000),
        until: WallNs(1_789_500_030_000_000_000),
        step: Some(10),
        average: BandLogAverage {
            seconds: 30,
            measured: Seconds(29.5),
            uncalibrated: 0,
            levels: Some(average),
        },
        rows: vec![BandLogSecond {
            start: WallNs(1_789_500_000_000_000_000),
            measured: Seconds(1.0),
            period: BandPeriod::Night,
            correction: Db(5.0),
            sensitivity: Some(Db(120.0)),
            levels,
        }],
    }
}

pub(super) fn spl_log_page() -> SplLogPage {
    SplLogPage {
        meas: MeasId(4),
        from: 120,
        total: 122,
        rows: vec![
            SplLogRow {
                start: WallNs(1_790_000_120_000_000_000),
                measured: Seconds(1.0),
                laeq: Dbfs(-26.5),
                lceq: Dbfs(-24.25),
                lzeq: Dbfs(-23.0),
                lcpeak: Dbfs(-8.5),
                lafmax: Dbfs(-18.25),
                sensitivity: Some(Db(120.0)),
                position: Some(PositionCorrection {
                    level: Db(2.5),
                    peak: Db(1.5),
                }),
            },
            SplLogRow {
                start: WallNs(1_790_000_121_000_000_000),
                measured: Seconds(0.5),
                laeq: Dbfs(f64::NEG_INFINITY),
                lceq: Dbfs(-90.0),
                lzeq: Dbfs(-80.0),
                lcpeak: Dbfs(-70.0),
                lafmax: Dbfs(f64::NEG_INFINITY),
                sensitivity: None,
                position: None,
            },
        ],
    }
}

/// A full state.
pub fn state() -> State {
    State {
        session: session(),
        measurements: vec![measurement(), spectrum_measurement()],
        traces: vec![trace_meta(), spectrum_trace_meta()],
        generator: generator(),
        calibrations: vec![cal_entry(), electrical_cal_entry()],
        line_calibrations: vec![line_cal_entry()],
        mics: vec![mic()],
        inputs: inputs().inputs,
        pairs: inputs().pairs,
        outputs: outputs(),
        spl_logs: vec![spl_log()],
        timing: timing(),
        sweep: Some(sweep_run()),
        autosave: Autosave {
            state: AutosaveState::Saved,
            saved_at: Some(WallNs(1_790_000_000_000_000_000)),

            log_failures: Vec::new(),
        },
        recording: Some(RecordingRun {
            status: RecordingStatus::Recording,
            ..recording_run()
        }),
        working: working(),
        rig: rig(),
        plans: vec![plan()],
        walks: vec![walk(), sweep_walk()],
        walk_plans: vec![plan()],
        sim: Some(super::sim::sim_state()),
    }
}

/// A room plan: a hall with its stalls, FOH and a speaker.
pub fn plan() -> PlanSpec {
    PlanSpec {
        name: "Hall".into(),
        outline: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 30.0], [0.0, 30.0]],
        ceiling: Some(9.5),
        zones: vec![ZoneSpec {
            name: "Stalls".into(),
            outline: vec![[2.0, 8.0], [18.0, 8.0], [18.0, 28.0], [2.0, 28.0]],
            ear_height: 1.2,
            weight: None,
            seats: Some(600),
        }],
        markers: vec![
            Marker {
                name: "FOH".into(),
                kind: MarkerKind::Foh,
                at: [10.0, 20.0, 1.5],
                heading_deg: Some(-90.0),
            },
            Marker {
                name: "Main L".into(),
                kind: MarkerKind::Speaker,
                at: [4.0, 1.0, 6.0],
                heading_deg: Some(90.0),
            },
        ],
        surfaces: vec![SurfaceTag {
            surface: Surface::Wall { edge: 2 },
            material: "brick".into(),
        }],
    }
}

/// A walk of measurements 1 and 2 (two mics) through three points of [`plan`]: step 0 put
/// mic 1 at P1 (captured) and mic 2 at P2 (skipped); at step 1, mic 1 at P3.
pub fn walk() -> WalkState {
    WalkState {
        id: WalkId(1),
        name: "Mains".into(),
        plan: "Hall".into(),
        kind: WalkKind::LiveTransfer,
        mics: vec![
            WalkMic {
                meas: MeasId(1),
                input: 1,
            },
            WalkMic {
                meas: MeasId(2),
                input: 2,
            },
        ],
        points: vec![
            WalkPoint {
                label: "P1".into(),
                at: [5.0, 12.0, 1.2],
                step: 0,
                mic: WalkMic {
                    meas: MeasId(1),
                    input: 1,
                },
                state: PointState::Captured {
                    trace: TraceId(7),
                    context: CaptureContext {
                        at: WallNs(1_761_400_000_000_000_000),
                        mic: Some(MicState {
                            name: "Vilppu".into(),
                            curve: None,
                        }),
                        eq_in: vec![EqMark {
                            set: EqSetId(2),
                            rev: 3,
                        }],
                    },
                },
            },
            WalkPoint {
                label: "P2".into(),
                at: [12.5, 15.0, 1.2],
                step: 0,
                mic: WalkMic {
                    meas: MeasId(2),
                    input: 2,
                },
                state: PointState::Skipped,
            },
            WalkPoint {
                label: "P3".into(),
                at: [8.0, 24.0, 1.2],
                step: 1,
                mic: WalkMic {
                    meas: MeasId(1),
                    input: 1,
                },
                state: PointState::Pending,
            },
        ],
        step: Some(1),
        average: None,
        room: None,
    }
}

/// A sweep walk of sweep measurement 4 recording inputs 2 and 3, both points of step 0
/// captured and averaged; its room parameters over the two positions.
pub fn sweep_walk() -> WalkState {
    let mic = |input| WalkMic {
        meas: MeasId(4),
        input,
    };
    let captured = |trace, input| PointState::Captured {
        trace: TraceId(trace),
        context: CaptureContext {
            at: WallNs(1_761_400_000_000_000_000),
            mic: Some(MicState {
                name: format!("M30 #{input}"),
                curve: None,
            }),
            eq_in: Vec::new(),
        },
    };
    let mean = |mean, min, max, n| Some(PositionMean { mean, min, max, n });
    WalkState {
        id: WalkId(2),
        name: "Room".into(),
        plan: "Hall".into(),
        kind: WalkKind::Sweep,
        mics: vec![mic(1), mic(2)],
        points: vec![
            WalkPoint {
                label: "P1".into(),
                at: [5.0, 12.0, 1.2],
                step: 0,
                mic: mic(1),
                state: captured(11, 1),
            },
            WalkPoint {
                label: "P2".into(),
                at: [12.5, 15.0, 1.2],
                step: 0,
                mic: mic(2),
                state: captured(12, 2),
            },
        ],
        step: None,
        average: Some(MeasId(5)),
        room: Some(WalkRoom {
            positions: 2,
            sources: 1,
            broadband: WalkRoomBand {
                centre: None,
                edt: mean(0.9, 0.85, 0.95, 2),
                t20: mean(1.1, 1.05, 1.15, 2),
                c80: mean(2.5, 2.0, 3.0, 2),
                d50: mean(0.45, 0.4, 0.5, 2),
            },
            octave: vec![WalkRoomBand {
                centre: Some(Hz(125.0)),
                edt: mean(1.2, 1.2, 1.2, 1),
                t20: None,
                c80: mean(-1.0, -1.5, -0.5, 2),
                d50: mean(0.3, 0.25, 0.35, 2),
            }],
            third: Vec::new(),
        }),
    }
}

/// Where [`walk`]'s first capture stood.
pub fn trace_position() -> TracePosition {
    TracePosition {
        label: "P1".into(),
        at: [5.0, 12.0, 1.2],
        plan: "Hall".into(),
        walk: WalkId(1),
    }
}

/// An OSC message.
pub fn osc_message(path: &str, args: Vec<OscArg>, append_level: bool) -> OscMessage {
    OscMessage {
        path: OscPath::new(path).expect("valid sample path"),
        args,
        append_level,
    }
}

/// A DMX level list from `channel=value` text.
pub fn dmx(text: &str) -> Vec<DmxSet> {
    parse_dmx_list(text).expect("valid sample levels")
}

/// The usual red-light cue: OSC to a stage display and levels on a lamp's channels.
pub fn light_cue() -> Cue {
    Cue {
        ok: Some(CueAction {
            osc: Some(osc_message("/light/red", vec![OscArg::Int(0)], false)),
            dmx: dmx("12=0 13=255"),
        }),
        near: Some(CueAction {
            osc: Some(osc_message(
                "/light/red",
                vec![OscArg::Float(0.5), OscArg::Str("near".into())],
                false,
            )),
            dmx: vec![],
        }),
        over: Some(CueAction {
            osc: Some(osc_message("/light/red", vec![OscArg::Int(1)], true)),
            dmx: dmx("12=255 14=40"),
        }),
        fault: Some(CueAction {
            osc: None,
            dmx: dmx("512=255"),
        }),
    }
}

/// The rig entity.
pub fn rig() -> Rig {
    Rig {
        rev: Rev(38),
        osc_target: Some("192.168.9.20:9000".parse().expect("valid sample address")),
        artnet_target: Some(ArtnetTarget {
            addr: "192.168.9.255:6454".parse().expect("valid sample address"),
            universe: PortAddress::from_parts(0, 0, 1).expect("valid sample universe"),
        }),
        artnet_fade: ArtnetFade::new(1.5).expect("valid sample fade"),
        monitor_at_start: true,
        levels: LevelsSettings {
            ballistic: Ballistic::Type1,
            correlation_window: CorrelationWindow::Short,
            over_threshold: Dbtp(-1.0),
            correlation_gate: Dbfs(-50.0),
            loudness_target: Lufs(-16.0),
        },
        osc: OutputStatus {
            sent: 12,
            last_error: Some("Connection refused (os error 111)".into()),
        },
        artnet: OutputStatus {
            sent: 340,
            last_error: None,
        },
        dmx: dmx("12=255 14=40"),
        cues: vec![
            RigCue {
                meas: MeasId(4),
                subject: CueSubject::Meter,
                state: CueState::Over,
            },
            RigCue {
                meas: MeasId(4),
                subject: CueSubject::Window {
                    duration: Seconds(1800.0),
                    weighting: Weighting::A,
                },
                state: CueState::Near,
            },
        ],
        damaged_files: vec![DamagedFile {
            kind: StateFile::RigSettings,
            path: "/home/op/.config/ac2/rig.json".into(),
            set_aside_as: Some("/home/op/.config/ac2/rig.json.damaged-1790000000".into()),
            reason: "EOF while parsing an object at line 3 column 0; restored the previous \
                     copy"
                .into(),
            at: WallNs(1_790_000_000_000_000_000),
        }],
    }
}
