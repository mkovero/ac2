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

pub(super) fn inputs() -> Vec<InputSetup> {
    vec![
        InputSetup {
            channel: 1,
            mic: Some("M30 #1234".into()),
            curve: CurveChoice::Curve {
                label: "0°".into()
            },
        },
        InputSetup {
            channel: 2,
            mic: None,
            curve: CurveChoice::NotChosen,
        },
        InputSetup {
            channel: 3,
            mic: Some("ECM".into()),
            curve: CurveChoice::Off,
        },
    ]
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
        input: 1,
        weighting: Weighting::A,
        time_weighting: TimeWeighting::Fast,
        peak_weighting: PeakWeighting::C,
        leq: LeqConfig {
            windows: vec![
                LeqWindow::minutes(1),
                LeqWindow {
                    duration: Seconds(1800.0),
                    weighting: Weighting::A,
                    limit: Some(DbSpl(99.0)),
                    warn_margin: Db(3.0),
                },
            ],
            horizon: Seconds(60.0),
            peaks: PeakLimits {
                lcpeak: Some(PeakLimit {
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
        mics: vec![mic()],
        inputs: inputs(),
        outputs: outputs(),
        spl_logs: vec![spl_log()],
        timing: timing(),
        sweep: Some(sweep_run()),
        autosave: Autosave {
            state: AutosaveState::Saved,
            saved_at: Some(WallNs(1_790_000_000_000_000_000)),
        },
        recording: Some(RecordingRun {
            status: RecordingStatus::Recording,
            ..recording_run()
        }),
        working: working(),
    }
}
