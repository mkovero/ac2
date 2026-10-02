//! One deterministic example of every message variant.
//!
//! Used by round-trip tests, the documentation parity test and the cross-language
//! fixtures (`tools/protocol/fixtures.py` builds the same frames in Python).

use crate::ctrl::{
    Command, ErrorCode, ErrorDetail, ImportProblem, MicCurveFileReason, ProtoError, ReplyBody,
    Welcome,
};
use crate::event::{Change, Event, Patch, StateSnapshot};
use crate::frame::{
    ClipFlags, Frame, FrameData, FrameStamp, GenSummary, IrFrame, IrMeta, KaMeta, LevelsFrame,
    LevelsMeta, PreviewLevelsFrame, PreviewLevelsMeta, ProtectionFlags, RtaFrame, RtaMeta,
    SessionLevelsFrame, SpecFrame, SpecMeta, SplFrame, SplMeta, TfFrame, TfMeta, TimingMeta,
    TimingWindow, ValidityMask,
};
use crate::grid::GridDef;
use crate::model::*;
use crate::units::*;

/// A log grid of 480 columns (10 octaves at 48 points per octave).
pub fn log_grid() -> GridDef {
    GridDef::Log {
        ppo: 48,
        k_min: -240,
        k_max: 239,
    }
}

/// One grid of each type.
pub fn grids() -> Vec<GridDef> {
    vec![
        log_grid(),
        GridDef::IecBands {
            fraction: BandFraction::Third,
            centres: vec![Hz(19.952_623_149_688_797), Hz(1000.0), Hz(20_000.0)],
        },
        GridDef::Linear {
            fs: Hz(48_000.0),
            n: 65_536,
        },
    ]
}

fn token() -> LeaseToken {
    LeaseToken(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210)
}

fn session_config() -> SessionConfig {
    SessionConfig {
        backend: Some(BackendKind::Cpal),
        input_device: DeviceSelector::Id {
            id: DeviceId("hw:UMC1820".into()),
        },
        output_device: DeviceSelector::Default,
        input_channels: vec![0, 1, 2],
        output_channels: 2,
        sample_rate_hz: Some(48_000),
        buffer_frames: None,
        loopback: Some(LoopbackRoute {
            output: 1,
            input: 0,
        }),
    }
}

fn settings() -> GeneratorSettings {
    GeneratorSettings {
        signal: Signal::PeriodicPink {
            period: Samples(131_072),
        },
        level: Dbfs(-20.0),
        band: Some(BandLimit {
            highpass: Some(Hz(30.0)),
            lowpass: None,
            order: FilterOrder::Fourth,
        }),
        outputs: vec![0, 1],
    }
}

fn sweep() -> EssSpec {
    EssSpec {
        start: Hz(20.0),
        end: Hz(20_000.0),
        duration: Seconds(5.0),
        fade_in: Seconds(0.01),
        fade_out: Seconds(0.01),
    }
}

fn meas_config() -> MeasConfig {
    MeasConfig {
        name: "Main L".into(),
        kind: MeasKind::Transfer {
            config: TransferConfig {
                reference_input: 0,
                measurement_input: 1,
                averaging: TfAveraging::Fifo { blocks: 8 },
                grid: LogGridSpec {
                    ppo: 48,
                    k_min: -240,
                    k_max: 239,
                },
                smoothing: Some(Smoothing {
                    fraction: SmoothingFraction::Sixth,
                    mode: SmoothingMode::Power,
                }),
                depth: DepthPolicy::FastLf {
                    max_settle_s: Seconds(1.0),
                },
            },
        },
    }
}

fn cal_key() -> CalKey {
    CalKey {
        device: DeviceId("hw:UMC1820".into()),
        channel: 1,
        mic: "M30 #1234".into(),
    }
}

fn edit() -> TraceEdit {
    TraceEdit {
        name: "Main L before EQ".into(),
        color: Rgb {
            r: 230,
            g: 120,
            b: 20,
        },
        visible: true,
        locked: false,
        order: 2,
        offset: Db(-3.0),
        polarity: Polarity::Inverted,
        delay_nudge: Seconds(0.000_25),
        slot: Some(3),
        smoothing: Some(Smoothing {
            fraction: SmoothingFraction::Twelfth,
            mode: SmoothingMode::Complex,
        }),
    }
}

/// One example of every [`Command`] variant.
pub fn commands() -> Vec<Command> {
    vec![
        Command::Hello {
            client: "ac2-cli 0.0.0".into(),
        },
        Command::SessionDevices,
        Command::SessionOpen {
            config: session_config(),
        },
        Command::SessionClose,
        Command::SessionStatus,
        Command::GenAcquire { force: false },
        Command::GenSet {
            lease_token: token(),
            desired: GeneratorDesired {
                settings: settings(),
                armed: true,
                firing: false,
            },
        },
        Command::GenRefresh {
            lease_token: token(),
        },
        Command::GenRelease {
            lease_token: token(),
        },
        Command::GenStop,
        Command::MeasCreate {
            config: meas_config(),
        },
        Command::MeasUpdate {
            meas: MeasId(3),
            config: MeasConfig {
                name: "RTA".into(),
                kind: MeasKind::Rta {
                    config: RtaConfig {
                        input: 2,
                        fraction: BandFraction::Third,
                        f_lo: Hz(20.0),
                        f_hi: Hz(20_000.0),
                        weighting: Weighting::Z,
                        averaging: SpecAveraging::Exponential {
                            time_constant: Seconds(1.0),
                        },
                    },
                },
            },
        },
        Command::MeasDelete { meas: MeasId(3) },
        Command::MeasStart { meas: MeasId(1) },
        Command::MeasStop { meas: MeasId(1) },
        Command::MeasFreeze {
            meas: MeasId(1),
            frozen: true,
        },
        Command::MeasReset { meas: MeasId(1) },
        Command::DelayFind {
            meas: MeasId(1),
            band: FinderBand::Sub,
            observation: Some(Seconds(8.0)),
        },
        Command::DelayInsert {
            meas: MeasId(1),
            pick: DelayPick::Ranked { index: 1 },
        },
        Command::DelaySet {
            meas: MeasId(1),
            delay: Seconds(0.012_5),
        },
        Command::DelayTrack {
            meas: MeasId(1),
            enabled: true,
        },
        Command::TraceCapture {
            meas: MeasId(1),
            name: "Main L".into(),
            slot: Some(3),
        },
        Command::TraceList,
        Command::TraceGet { trace: TraceId(7) },
        Command::TraceUpdate {
            trace: TraceId(7),
            edit: edit(),
        },
        Command::TraceDelete { trace: TraceId(7) },
        Command::TraceAverage {
            traces: vec![TraceId(7), TraceId(8)],
            method: AverageMethod::CoherenceWeighted,
            reference: DelayReference::Trace { trace: TraceId(7) },
            name: "avg".into(),
        },
        Command::TraceMath {
            a: TraceId(7),
            b: TraceId(8),
            op: MathOp::ComplexDivision,
            name: "L/R".into(),
        },
        Command::TraceImport {
            file_name: "sub.txt".into(),
            format: ImportFormat::AnalyzerText,
            role: ImportRole::Target,
            content: Blob(b"20 -3.0 10\n".to_vec()),
        },
        Command::TraceExport {
            trace: TraceId(7),
            format: ExportFormat::Ac2Csv,
        },
        Command::CalSpl {
            input: 1,
            mic: "M30 #1234".into(),
            calibrator_level: DbSpl(94.0),
            calibrator_freq: Hz(1000.0),
        },
        Command::CalMicCurve {
            input: 1,
            mic: "M30 #1234".into(),
            action: MicCurveAction::Import {
                file_name: "M30-1234.frd".into(),
                content: Blob(b"20 -0.5\n20000 1.5\n".to_vec()),
            },
        },
        Command::CalList,
        Command::SplLogStart {
            meas: MeasId(4),
            interval: Seconds(1.0),
        },
        Command::SplLogStop { meas: MeasId(4) },
        Command::IrCapture {
            lease_token: token(),
            input: 1,
            sweep: sweep(),
            name: "room".into(),
        },
        Command::StateSnapshot,
        Command::StateSince { rev: Rev(41) },
        Command::GridGet {
            grid_id: log_grid().id(),
        },
        Command::FileSave {
            session: SessionRef::Name {
                name: "friday show".into(),
            },
        },
        Command::FileLoad {
            session: SessionRef::Path {
                path: "/tmp/show".into(),
            },
        },
        Command::FileList,
        Command::SessionInputs { inputs: inputs() },
        Command::CalDelete {
            key: cal_key(),
            part: CalPart::Sensitivity,
        },
        Command::SessionPreview {
            backend: BackendKind::Jack,
            device: DeviceId("jack".into()),
        },
        Command::SessionPreviewStop,
        Command::SessionDetectLoopback {
            lease_token: token(),
            backend: BackendKind::Jack,
            device: DeviceId("jack".into()),
            output: 0,
            level: Some(Dbfs(-30.0)),
        },
    ]
}

fn measurement() -> Measurement {
    Measurement {
        id: MeasId(1),
        config: meas_config(),
        config_rev: Rev(40),
        running: true,
        frozen: false,
        delay: Some(DelayState {
            applied: Seconds(0.0125),
            applied_samples: Samples(600),
            tracking: true,
            awaiting_pick: false,
            last_finding: Some(finding()),
        }),
        grid_id: Some(log_grid().id()),
    }
}

fn arrival(delay_samples: f64, level: f64) -> DelayArrival {
    DelayArrival {
        delay: Seconds(delay_samples / 48_000.0),
        delay_samples,
        level: Db(level),
        phase: Degrees(-12.5),
        uncertainty_samples: 0.25,
        misfit: 0.125,
        refined: true,
    }
}

fn confidence() -> DelayConfidence {
    DelayConfidence {
        psr_db: Some(Db(18.5)),
        psr_acq_db: Some(Db(14.0)),
        band_snr_db: Some(Db(21.25)),
        excited_fraction: Some(0.875),
        uncertainty_samples: Some(0.25),
        pulse_width_samples: Some(9.5),
        period: None,
    }
}

/// An ambiguous finding (two near-equal arrivals).
fn finding() -> DelayFinding {
    DelayFinding {
        outcome: DelayOutcome::Ambiguous {
            reasons: vec![
                AmbiguityReason::BorderlineLevel,
                AmbiguityReason::MergedLobe,
            ],
            ranked: vec![arrival(600.0, -1.25), arrival(628.5, 0.0)],
            strongest: arrival(628.5, 0.0),
        },
        confidence: confidence(),
        band: DelayBand::Custom {
            lo_hz: Hz(80.0),
            hi_hz: Hz(800.0),
        },
        observation: Seconds(0.5),
        candidates: vec![arrival(600.0, -1.25), arrival(628.5, 0.0)],
        found_at: WallNs(1_790_000_000_000_000_000),
    }
}

fn accepted_finding() -> DelayFinding {
    DelayFinding {
        outcome: DelayOutcome::Accepted {
            first: arrival(600.0, -6.0),
            strongest: arrival(628.5, 0.0),
        },
        confidence: confidence(),
        band: DelayBand::Full,
        observation: Seconds(0.25),
        candidates: vec![arrival(600.0, -6.0), arrival(628.5, 0.0)],
        found_at: WallNs(1_790_000_000_000_000_000),
    }
}

fn refused_finding() -> DelayFinding {
    DelayFinding {
        outcome: DelayOutcome::NoEstimate {
            reasons: vec![
                NoEstimateReason::LowPsr,
                NoEstimateReason::PeriodicExcitation {
                    period: Samples(131_072),
                },
            ],
        },
        confidence: DelayConfidence {
            psr_db: Some(Db(3.0)),
            psr_acq_db: None,
            band_snr_db: None,
            excited_fraction: Some(0.5),
            uncertainty_samples: None,
            pulse_width_samples: None,
            period: Some(Samples(131_072)),
        },
        band: DelayBand::Sub,
        observation: Seconds(4.0),
        candidates: vec![],
        found_at: WallNs(1_790_000_000_000_000_000),
    }
}

fn trace_meta() -> TraceMeta {
    TraceMeta {
        id: TraceId(7),
        edit: edit(),
        kind: TraceKind::Transfer,
        source: TraceSource::Captured {
            meas: MeasId(1),
            meas_name: "Main L".into(),
            epoch: SessionEpoch(2),
            at_sample: SampleIndex(4_800_000),
        },
        grid_id: log_grid().id(),
        delay: Seconds(0.0125),
        depth: Some(DepthPolicy::EqualConfidence),
        cal: CalState::Calibrated {
            key: cal_key(),
            sensitivity: Db(120.5),
            calibrated_at: WallNs(1_789_000_000_000_000_000),
        },
        mic: Some(MicState {
            name: "M30 #1234".into(),
            curve: Some("M30-1234".into()),
        }),
        created_at: WallNs(1_790_000_000_000_000_000),
    }
}

fn session_file() -> SessionFile {
    SessionFile {
        name: "friday show".into(),
        path: "/home/fohtech/.local/share/ac2/sessions/friday show".into(),
        saved_at: WallNs(1_790_000_000_000_000_000),
        measurements: 2,
        traces: 5,
    }
}

fn generator() -> Generator {
    Generator {
        owner: Some(ClientId("alice".into())),
        armed: true,
        firing: true,
        settings: Some(settings()),
        ceiling: Dbfs(-6.0),
        last_action: Some(GenAudit {
            action: GenAction::Fire,
            client: Some(ClientId("alice".into())),
            at: WallNs(1_790_000_000_000_000_000),
        }),
    }
}

fn cal_entry() -> CalEntry {
    CalEntry {
        key: cal_key(),
        spl: Some(SplCal {
            sensitivity: Db(120.5),
            calibrator_level: DbSpl(94.0),
            calibrator_freq: Hz(1000.0),
            measured: Dbfs(-26.5),
            calibrated_at: WallNs(1_789_000_000_000_000_000),
        }),
        mic_curve: Some(MicCurveRef {
            name: "M30-1234".into(),
            file_name: "M30-1234.frd".into(),
            content_hash: "af63bd4c8601b7df".into(),
            points: 2,
            f_lo: Hz(20.0),
            f_hi: Hz(20_000.0),
            imported_at: WallNs(1_788_000_000_000_000_000),
        }),
    }
}

fn inputs() -> Vec<InputSetup> {
    vec![
        InputSetup {
            channel: 1,
            mic: Some("M30 #1234".into()),
            mic_curve: true,
        },
        InputSetup {
            channel: 2,
            mic: None,
            mic_curve: false,
        },
    ]
}

fn session() -> Session {
    Session {
        epoch: SessionEpoch(2),
        open: Some(OpenSession {
            config: session_config(),
            backend: BackendKind::Cpal,
            input_device: DeviceId("hw:UMC1820".into()),
            output_device: DeviceId("hw:UMC1820".into()),
            sample_rate_hz: 48_000,
            buffer_frames: 256,
            clock: ClockRelation::SingleCallback,
            opened_at: WallNs(1_789_999_000_000_000_000),
        }),
    }
}

fn timing() -> TimingStatus {
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
        }),
        internal_reference: true,
    }
}

fn spl_log() -> SplLog {
    SplLog {
        meas: MeasId(4),
        running: true,
        interval: Seconds(1.0),
        started_at: Some(WallNs(1_790_000_000_000_000_000)),
    }
}

/// A full state.
pub fn state() -> State {
    State {
        session: session(),
        measurements: vec![measurement()],
        traces: vec![trace_meta()],
        generator: generator(),
        calibrations: vec![cal_entry()],
        inputs: inputs(),
        spl_logs: vec![spl_log()],
        timing: timing(),
    }
}

/// One example of every [`Change`] variant (and both [`Patch`] arms).
pub fn events() -> Vec<Event> {
    let ev = |rev, change| Event {
        rev: Rev(rev),
        change,
    };
    vec![
        ev(42, Change::Session(session())),
        ev(43, Change::Measurement(Patch::Set(measurement()))),
        ev(44, Change::Measurement(Patch::Deleted(MeasId(3)))),
        ev(45, Change::Trace(Patch::Set(trace_meta()))),
        ev(46, Change::Trace(Patch::Deleted(TraceId(8)))),
        ev(47, Change::Generator(generator())),
        ev(48, Change::Calibration(Patch::Set(cal_entry()))),
        ev(49, Change::Calibration(Patch::Deleted(cal_key()))),
        ev(50, Change::Inputs(inputs())),
        ev(52, Change::SplLog(Patch::Set(spl_log()))),
        ev(53, Change::SplLog(Patch::Deleted(MeasId(4)))),
        ev(54, Change::Timing(timing())),
    ]
}

/// One example of every [`ReplyBody`] variant, plus an error.
pub fn replies() -> Vec<Result<ReplyBody, ProtoError>> {
    vec![
        Ok(ReplyBody::Ack { rev: Rev(42) }),
        Ok(ReplyBody::Welcome(Welcome {
            server: "ac2d 0.0.0".into(),
            client_id: ClientId("alice".into()),
            daemon_incarnation: DaemonIncarnation(0x5eed_5eed_5eed_5eed),
            session_epoch: SessionEpoch(2),
            rev: Rev(42),
        })),
        Ok(ReplyBody::Backends(vec![
            BackendInfo {
                kind: BackendKind::Jack,
                description: "JACK audio server".into(),
                availability: Availability::Available,
                devices: vec![DeviceInfo {
                    backend: BackendKind::Jack,
                    host: "jack".into(),
                    id: DeviceId("jack".into()),
                    name: "JACK server".into(),
                    input: Some(DirectionInfo {
                        max_channels: 2,
                        rates_hz: vec![RangeU32 {
                            min: 48_000,
                            max: 48_000,
                        }],
                        buffer_frames: Some(RangeU32 { min: 256, max: 256 }),
                        default_rate_hz: Some(48_000),
                        default_buffer_frames: Some(256),
                        channel_names: Some(vec!["capture_1".into(), "capture_2".into()]),
                    }),
                    output: None,
                    duplex_clock: ClockRelation::SingleCallback,
                    index: IndexExactness::Exact,
                    notes: vec![],
                }],
            },
            BackendInfo {
                kind: BackendKind::Cpal,
                description: "system audio".into(),
                availability: Availability::Unavailable {
                    reason: "no devices".into(),
                },
                devices: vec![],
            },
        ])),
        Ok(ReplyBody::Preview(Preview {
            backend: BackendKind::Jack,
            device: DeviceId("jack".into()),
            channels: 2,
            sample_rate_hz: 48_000,
            expires_in_ms: 5000,
        })),
        Ok(ReplyBody::LoopbackDetection(LoopbackDetection {
            backend: BackendKind::Jack,
            device: DeviceId("jack".into()),
            output: 0,
            level: Dbfs(-30.0),
            ranked: vec![
                LoopbackCandidate {
                    input: 0,
                    delay: Seconds(32.0 / 48_000.0),
                    delay_samples: Samples(32),
                    correlation: 0.999,
                    gain: Some(Db(-0.5)),
                },
                LoopbackCandidate {
                    input: 1,
                    delay: Seconds(0.0),
                    delay_samples: Samples(0),
                    correlation: 0.0,
                    gain: None,
                },
            ],
            loopback: Some(0),
            clock: ClockRelation::SingleCallback,
        })),
        Ok(ReplyBody::Session(session())),
        Ok(ReplyBody::Lease(Lease {
            lease_token: token(),
            expires_in_ms: 1500,
        })),
        Ok(ReplyBody::Generator(generator())),
        Ok(ReplyBody::Measurement(measurement())),
        Ok(ReplyBody::DelayFinding(finding())),
        Ok(ReplyBody::DelayFinding(accepted_finding())),
        Ok(ReplyBody::DelayFinding(refused_finding())),
        Ok(ReplyBody::Trace(trace_meta())),
        Ok(ReplyBody::Traces(vec![trace_meta()])),
        Ok(ReplyBody::TraceData(TraceData {
            meta: trace_meta(),
            mag_db: vec![0.0, -3.0, f32::NAN],
            phase_deg: Some(vec![0.0, 45.0, f32::NAN]),
            coherence: None,
        })),
        Ok(ReplyBody::Export {
            file_name: "main-l.csv".into(),
            content: Blob(b"freq_hz,mag_db\n1000,0\n".to_vec()),
        }),
        Ok(ReplyBody::Calibration(cal_entry())),
        Ok(ReplyBody::Calibrations(vec![cal_entry()])),
        Ok(ReplyBody::Inputs(inputs())),
        Ok(ReplyBody::Calibration(CalEntry {
            key: cal_key(),
            spl: None,
            mic_curve: None,
        })),
        Ok(ReplyBody::SplLog(spl_log())),
        Ok(ReplyBody::Snapshot(Box::new(StateSnapshot {
            state: state(),
            rev: Rev(42),
            daemon_incarnation: DaemonIncarnation(0x5eed_5eed_5eed_5eed),
            session_epoch: SessionEpoch(2),
        }))),
        Ok(ReplyBody::Events(events())),
        Ok(ReplyBody::Grid(grids().remove(1))),
        Ok(ReplyBody::SessionFile(session_file())),
        Ok(ReplyBody::Sessions(vec![session_file()])),
        Err(ProtoError {
            code: ErrorCode::Conflict,
            msg: "state moved".into(),
            detail: Some(ErrorDetail::Conflict { rev: Rev(50) }),
        }),
        Err(ProtoError {
            code: ErrorCode::LeaseHeld,
            msg: "held by bob".into(),
            detail: Some(ErrorDetail::LeaseHeld {
                owner: ClientId("bob".into()),
            }),
        }),
        Err(ProtoError {
            code: ErrorCode::ResyncRequired,
            msg: "gap expired".into(),
            detail: Some(ErrorDetail::Resync { oldest: Rev(100) }),
        }),
        Err(ProtoError {
            code: ErrorCode::Invalid,
            msg: "line 12: not a number".into(),
            detail: Some(ErrorDetail::Import {
                line: Some(12),
                problem: ImportProblem::BadNumber,
            }),
        }),
        Err(ProtoError {
            code: ErrorCode::Unsupported,
            msg: "session format 2".into(),
            detail: Some(ErrorDetail::SessionVersion {
                found: 2,
                supported: 1,
            }),
        }),
        Err(ProtoError {
            code: ErrorCode::Invalid,
            msg: "line 7: frequency is not above the previous line's".into(),
            detail: Some(ErrorDetail::MicCurveFile {
                line: Some(7),
                reason: MicCurveFileReason::NotAscending,
            }),
        }),
        Err(ProtoError {
            code: ErrorCode::Refused,
            msg: "calibration store is unreadable".into(),
            detail: Some(ErrorDetail::CalStore {
                path: "/home/op/.config/ac2/calibrations.json".into(),
                reason: "expected value at line 1 column 1".into(),
            }),
        }),
        Err(ProtoError {
            code: ErrorCode::NotFound,
            msg: "no trace 9".into(),
            detail: None,
        }),
    ]
}

/// The stamp the sample frames carry (with the kind's grid).
pub fn stamp(grid: Option<GridDef>) -> FrameStamp {
    FrameStamp {
        seq: 1234,
        audio_sample: SampleIndex(48_000 * 3600),
        session_epoch: SessionEpoch(2),
        daemon_incarnation: DaemonIncarnation(0x5eed_5eed_5eed_5eed),
        config_rev: Rev(40),
        config_applied_at: SampleIndex(48_000 * 3500),
        capture_wall_ns: WallNs(1_790_000_000_123_456_789),
        grid_id: grid.map(|g| g.id()),
        protection: ProtectionFlags::WEAK_REFERENCE,
    }
}

/// A 480-column TF frame. Columns 0..4 are thinned and 470.. out of band (NaN). Values
/// are exactly representable so other languages can rebuild them bit for bit.
pub fn tf_frame(eff_avg: bool) -> Frame {
    let n = 480;
    let valid = |i: usize| (4..470).contains(&i);
    let val = |i: usize, v: f32| if valid(i) { v } else { f32::NAN };
    Frame {
        stamp: stamp(Some(log_grid())),
        data: FrameData::Tf(TfFrame {
            meas: MeasId(1),
            meta: TfMeta {
                delay: Seconds(0.0125),
                frozen: false,
                smoothing: Some(Smoothing {
                    fraction: SmoothingFraction::Sixth,
                    mode: SmoothingMode::Power,
                }),
                mic_curve: true,
            },
            mag: (0..n).map(|i| val(i, -6.0 + i as f32 * 0.031_25)).collect(),
            phase: (0..n)
                .map(|i| val(i, ((i * 15) % 720) as f32 * 0.5 - 180.0))
                .collect(),
            coh: (0..n).map(|i| val(i, (i % 33) as f32 / 32.0)).collect(),
            eff_avg: eff_avg.then(|| (0..n).map(|i| 8.0 + (i % 5) as f32).collect()),
            validity: (0..n)
                .map(|i| {
                    if i < 4 {
                        ValidityMask::THINNED
                    } else if i >= 470 {
                        ValidityMask::OUT_OF_BAND
                    } else {
                        ValidityMask::NONE
                    }
                })
                .collect(),
        }),
    }
}

/// One frame of every kind.
pub fn frames() -> Vec<Frame> {
    let mut g = grids();
    let linear = g.remove(2);
    let bands = g.remove(1);
    vec![
        tf_frame(true),
        Frame {
            stamp: stamp(None),
            data: FrameData::Ir(IrFrame {
                meas: MeasId(1),
                meta: IrMeta {
                    sample_rate: Hz(48_000.0),
                    t0: Seconds(-0.005),
                    dt: Seconds(1.0 / 12_000.0),
                    inserted_delay: Seconds(0.0125),
                },
                linear: vec![0.0, 0.5, -1.0, 0.25, f32::MIN_POSITIVE, -0.0],
                etc: Some(vec![-60.0, -6.0, 0.0, -12.0, -200.0, -200.0]),
            }),
        },
        Frame {
            stamp: stamp(Some(bands.clone())),
            data: FrameData::Rta(RtaFrame {
                meas: MeasId(3),
                meta: RtaMeta {
                    fraction: BandFraction::Third,
                    weighting: Weighting::Z,
                    scale: LevelScale::DbSpl,
                    cal: CalStatus::Verified {
                        calibrated_at: WallNs(1_789_000_000_000_000_000),
                    },
                    mic_curve: true,
                },
                level: vec![f32::NAN, 74.5, 61.25],
                validity: vec![
                    ValidityMask::INSUFFICIENT_RESOLUTION,
                    ValidityMask::NONE,
                    ValidityMask::NONE,
                ],
            }),
        },
        Frame {
            stamp: stamp(Some(linear)),
            data: FrameData::Spec(SpecFrame {
                meas: MeasId(5),
                meta: SpecMeta {
                    window: Window::Hann,
                    scale: LevelScale::Dbfs,
                    cal: CalStatus::Uncalibrated,
                    mic_curve: false,
                },
                level: vec![-120.0, -20.0, f32::INFINITY, f32::NEG_INFINITY],
                validity: vec![ValidityMask::NONE; 4],
            }),
        },
        Frame {
            stamp: stamp(None),
            data: FrameData::Spl(SplFrame {
                meas: MeasId(4),
                meta: SplMeta {
                    scale: LevelScale::DbSpl,
                    weighting: Weighting::A,
                    time_weighting: TimeWeighting::Fast,
                    peak_weighting: PeakWeighting::C,
                    level: 94.1,
                    lmax: 101.3,
                    lmin: 40.2,
                    leq: 92.0,
                    lpeak: 112.7,
                    duration: Seconds(60.0),
                    cal: CalStatus::OtherMicOrInput {
                        calibrated_at: WallNs(1_789_000_000_000_000_000),
                    },
                    mic_curve: true,
                },
            }),
        },
        Frame {
            stamp: stamp(None),
            data: FrameData::Levels(LevelsFrame {
                meas: MeasId(1),
                meta: LevelsMeta {
                    channels: vec![0, 1],
                },
                peak: vec![-0.1, -18.0],
                rms: vec![-12.0, -30.5],
                clip: vec![ClipFlags::CLIP.with(ClipFlags::HELD), ClipFlags::NONE],
            }),
        },
        Frame {
            stamp: FrameStamp {
                protection: ProtectionFlags::NONE,
                ..stamp(None)
            },
            data: FrameData::SessionLevels(SessionLevelsFrame {
                meta: LevelsMeta {
                    channels: vec![0, 1, 2],
                },
                peak: vec![-6.0, -0.0625, f32::NEG_INFINITY],
                rms: vec![-18.5, -3.25, f32::NEG_INFINITY],
                clip: vec![ClipFlags::NONE, ClipFlags::HELD, ClipFlags::NONE],
            }),
        },
        Frame {
            stamp: FrameStamp {
                protection: ProtectionFlags::NONE,
                ..stamp(None)
            },
            data: FrameData::PreviewLevels(PreviewLevelsFrame {
                meta: PreviewLevelsMeta {
                    backend: BackendKind::Jack,
                    device: DeviceId("jack".into()),
                    channels: vec![0, 1],
                },
                peak: vec![-12.0, -40.5],
                rms: vec![-20.0, -52.25],
                clip: vec![ClipFlags::NONE, ClipFlags::CLIP],
            }),
        },
        Frame {
            stamp: stamp(None),
            data: FrameData::Timing(TimingMeta {
                status: timing(),
                window: Some(TimingWindow {
                    capture_start: SampleIndex(96_000),
                    offset: Some(Samples(312)),
                    psr: Some(Db(24.0)),
                    loopback: Dbfs(-20.5),
                    stimulus: Dbfs(-20.0),
                }),
            }),
        },
        Frame {
            stamp: FrameStamp {
                grid_id: None,
                protection: ProtectionFlags::NONE,
                ..stamp(None)
            },
            data: FrameData::Ka(KaMeta {
                rev: Rev(40),
                daemon_wall_ns: WallNs(1_790_000_000_123_456_789),
                timing: TimingState::Locked {
                    offset: Samples(312),
                },
                generator: GenSummary {
                    owner: Some(ClientId("alice".into())),
                    armed: true,
                    firing: true,
                },
            }),
        },
    ]
}
