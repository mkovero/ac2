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
    BandLeqFrame, BandLeqMeta, BandWindowState, ClipFlags, Frame, FrameData, FrameStamp,
    GenSummary, IrFrame, IrMeta, KaMeta, LeqFlags, LeqFrame, LeqMeta, LeqPeak, LeqRun, LevelsFrame,
    LevelsMeta, MathState, OperandState, OperandStatus, PreviewLevelsFrame, PreviewLevelsMeta,
    ProtectionFlags, RtaFrame, RtaMeta, SessionLevelsFrame, SpecFrame, SpecMeta, SplFrame, SplMeta,
    TfFrame, TfMeta, TimingMeta, TimingWindow, ValidityMask,
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
        GridDef::LogBins {
            fs: Hz(48_000.0),
            n: 65_536,
            ppo: 96,
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
                    mode: SmoothingMode::Magnitude,
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
            mode: SmoothingMode::MagnitudePhase,
        }),
        owner: TraceOwner::Meas { meas: MeasId(1) },
    }
}

/// A sweep measurement's settings.
fn sweep_config() -> SweepConfig {
    SweepConfig {
        reference_input: 1,
        measurement_input: 0,
        outputs: vec![0, 1],
        level: Dbfs(-50.0),
        sweep: sweep(),
        repeats: 2,
        gate: Some(Seconds(0.005)),
        tail: Some(Seconds(3.0)),
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
        Command::MeasDelete {
            meas: MeasId(3),
            traces: OwnedTraces::Keep,
        },
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
        Command::CalCurveImport {
            mic: "M30 #1234".into(),
            label: Some("0°".into()),
            file_name: "M30-1234.frd".into(),
            content: Blob(b"20 -0.5\n20000 1.5\n".to_vec()),
            input: Some(1),
        },
        Command::CalList,
        Command::SplLogGet {
            meas: MeasId(4),
            log: SplLogWhich::Previous,
            from: 120,
            max: 3600,
        },
        Command::SweepRun {
            lease_token: token(),
            meas: MeasId(7),
            name: Some("1083 sweep".into()),
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
        Command::CalDelete { key: cal_key() },
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
        Command::TraceMicCurve {
            trace: TraceId(8),
            curve: Some(curve_id()),
        },
        Command::CalCurveRename {
            curve: curve_id(),
            label: "on axis".into(),
        },
        Command::CalCurveDelete { curve: curve_id() },
        Command::SplLogNew { meas: MeasId(4) },
        Command::CalSplElectrical {
            input: 1,
            mic: "M30 #1234".into(),
            connection: ElectricalConnection::InLine,
            volts: Volts(0.015),
            freq: Hz(1000.0),
            mic_sensitivity: Some(MvPerPa(15.0)),
            uncertainty: None,
            replace_acoustic: false,
        },
        Command::SplHistoryGet {
            meas: MeasId(4),
            seconds: 14_400,
        },
        Command::DelayNudge {
            meas: MeasId(1),
            by: Seconds(-0.25 / 48_000.0),
        },
        Command::RecStart {
            request: RecordRequest {
                inputs: vec![0, 1],
                name: Some("soundcheck".into()),
                max_duration: Seconds(600.0),
                max_bytes: None,
            },
        },
        Command::RecStop,
        Command::RecList,
        Command::SessionReplay {
            recording: RecordingRef::Name {
                name: "soundcheck".into(),
            },
            pace: ReplayPace::Fast,
        },
        Command::GenCeiling {
            ceiling: Dbfs(-40.0),
            confirm_raise: true,
        },
        Command::SessionOutputs {
            outputs: vec![
                OutputSetup {
                    channel: 0,
                    label: Some("Main L".into()),
                },
                OutputSetup {
                    channel: 1,
                    label: None,
                },
            ],
        },
        Command::ServerInfo,
        Command::ServerAuthorize {
            name: "laptop".into(),
            key: "Yne@$w-vo<fVvi]a<NY6T1ed:M$fCG*[IaLV{hID".into(),
        },
        Command::ServerRevoke {
            name: "laptop".into(),
        },
        Command::SplBandTransfer {
            meas: MeasId(4),
            foh: BandLevelSource::Log {
                meas: MeasId(4),
                from: WallNs(1_789_500_000_000_000_000),
                until: WallNs(1_789_500_030_000_000_000),
            },
            at_place: BandLevelSource::Levels {
                levels: (0..BAND_COUNT)
                    .map(|i| (i < 3).then_some(DbSpl(50.0 - i as f64)))
                    .collect(),
            },
            background: None,
            place: "flat 4 bedroom".into(),
        },
        Command::SplBandLogGet {
            meas: MeasId(4),
            from: WallNs(1_789_500_000_000_000_000),
            until: WallNs(1_789_500_030_000_000_000),
            step: Some(10),
        },
    ]
}

/// A sweep measurement: settings only, never running.
pub fn sweep_measurement() -> Measurement {
    Measurement {
        id: MeasId(7),
        config: MeasConfig {
            name: "Genelec 1 m".into(),
            kind: MeasKind::Sweep {
                config: sweep_config(),
            },
        },
        config_rev: Rev(66),
        running: false,
        frozen: false,
        delay: None,
        grid_id: None,
    }
}

/// A math channel: the coherence-weighted average of two live transfer measurements and a
/// stored trace.
pub fn math_measurement() -> Measurement {
    Measurement {
        id: MeasId(6),
        config: MeasConfig {
            name: "FOH average".into(),
            kind: MeasKind::Math {
                config: MathConfig {
                    owner: TraceOwner::Meas { meas: MeasId(1) },
                    domain: MathDomain::Transfer,
                    expr: MathExpr::Average {
                        of: vec![
                            Operand::Meas { meas: MeasId(1) },
                            Operand::Meas { meas: MeasId(2) },
                            Operand::Trace { trace: TraceId(7) },
                        ],
                        method: AverageMethod::CoherenceWeighted,
                    },
                    reference: MathReference::Operand {
                        operand: Operand::Meas { meas: MeasId(2) },
                    },
                    smoothing: Some(Smoothing {
                        fraction: SmoothingFraction::Third,
                        mode: SmoothingMode::MagnitudePhase,
                    }),
                },
            },
        },
        config_rev: Rev(62),
        running: true,
        frozen: false,
        delay: None,
        grid_id: Some(log_grid().id()),
    }
}

/// A capture of the math channel `Main L ÷ Sub` (a live measurement over a stored trace).
fn math_trace_meta() -> TraceMeta {
    TraceMeta {
        id: TraceId(10),
        source: TraceSource::Math {
            meas: MeasId(6),
            meas_name: "Main L ÷ Sub".into(),
            epoch: SessionEpoch(2),
            at_sample: SampleIndex(4_800_000),
            expr: MathExpr::Binary {
                a: Operand::Meas { meas: MeasId(1) },
                op: MathOp::Divide,
                b: Operand::Trace { trace: TraceId(8) },
            },
            operands: vec![
                NamedOperand {
                    operand: Operand::Meas { meas: MeasId(1) },
                    name: "Main L".into(),
                },
                NamedOperand {
                    operand: Operand::Trace { trace: TraceId(8) },
                    name: "Sub".into(),
                },
            ],
            phase: PhaseBasis::OwnAlignments,
        },
        delay: Seconds(0.0),
        depth: None,
        cal: CalState::Uncalibrated,
        mic: None,
        ..trace_meta()
    }
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
            applied_samples: 600.25,
            nudged: Seconds(0.25 / 48_000.0),
            nudged_samples: 0.25,
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
            curve: Some(mic_curve_ref()),
        }),
        mic_curve: None,
        created_at: WallNs(1_790_000_000_000_000_000),
    }
}

/// An imported trace with a mic curve applied afterwards.
fn imported_trace_meta() -> TraceMeta {
    TraceMeta {
        id: TraceId(8),
        edit: TraceEdit {
            owner: TraceOwner::Imported,
            ..edit()
        },
        kind: TraceKind::Transfer,
        source: TraceSource::Imported {
            file_name: "sweep1.csv".into(),
            format: ImportFormat::Ac2Csv,
            notes: vec![ImportNote::SweepWithoutAnalysis],
        },
        delay: Seconds(0.003_25),
        depth: None,
        cal: CalState::Uncalibrated,
        mic: None,
        mic_curve: Some(Box::new(TraceMicCurve {
            mic: "M30 #1234".into(),
            curve: mic_curve_ref(),
            f_norm: Hz(1000.0),
        })),
        ..trace_meta()
    }
}

fn sweep_run() -> SweepRun {
    SweepRun {
        id: SweepId(3),
        meas: MeasId(7),
        owner: ClientId("alice".into()),
        name: "1083 sweep".into(),
        reference_input: 1,
        measurement_input: 0,
        outputs: vec![0, 1],
        level: Dbfs(-50.0),
        sweep: sweep(),
        sweep_duration: Seconds(4.75),
        post_roll: Seconds(1.0),
        repeats: 2,
        gate: None,
        status: SweepStatus::Done { trace: TraceId(9) },
        started_at: WallNs(1_790_000_000_000_000_000),
    }
}

fn sweep_meta() -> TraceMeta {
    TraceMeta {
        id: TraceId(9),
        edit: TraceEdit {
            owner: TraceOwner::Meas { meas: MeasId(7) },
            ..edit()
        },
        kind: TraceKind::Sweep,
        source: TraceSource::Sweep {
            meas: MeasId(7),
            meas_name: "Genelec 1 m".into(),
            run: SweepId(3),
            number: 2,
            epoch: SessionEpoch(2),
            sweep: sweep(),
            level: Dbfs(-50.0),
            repeats: 2,
            reference_input: 1,
            measurement_input: 0,
        },
        depth: None,
        cal: CalState::Uncalibrated,
        ..trace_meta()
    }
}

fn sweep_data() -> SweepData {
    let curve = |l: f32| DistortionCurve {
        level_db: vec![l, l - 6.0, f32::NAN],
        floor_db: vec![-80.0, -78.0, f32::NAN],
    };
    SweepData {
        harmonics: vec![
            HarmonicCurve {
                order: 2,
                curve: curve(-40.0),
            },
            HarmonicCurve {
                order: 3,
                curve: curve(-50.0),
            },
        ],
        thd: curve(-39.5),
        ir: SweepIr {
            t0: Seconds(-0.75),
            dt: Seconds(1.0 / 48_000.0),
            linear: vec![0.0, 0.5, -0.25],
            etc_db: vec![-200.0, -6.0, -12.0],
        },
        info: SweepInfo {
            sample_rate: Hz(48_000.0),
            rate: Seconds(0.6875),
            duration: Seconds(4.75),
            repeats: 2,
            arrival: Seconds(0.003_3),
            reference_level: Db(2.25),
            window_pre: Seconds(0.0125),
            window_post: Seconds(0.1375),
            gate_pre: Seconds(0.047_5),
            gate: Seconds(0.875),
            floor_margin: Db(6.0),
            clipped: false,
        },
        room: Some(RoomAcoustics {
            broadband: room_band(None),
            octave: vec![room_band(Some(Hz(1000.0)))],
            third: Vec::new(),
            span_end: Seconds(0.98),
        }),
    }
}

fn room_band(centre: Option<Hz>) -> RoomBand {
    RoomBand {
        centre,
        onset: Seconds(0.0),
        truncation: Seconds(0.75),
        decay_range: Some(Db(52.5)),
        edt: RoomValue::Value { value: 0.875 },
        t20: RoomValue::Value { value: 0.9 },
        t30: RoomValue::Refused {
            reason: RoomRefusal::InsufficientRange {
                range: Db(42.5),
                needed: Db(45.0),
            },
        },
        c50: RoomValue::Value { value: -1.5 },
        c80: RoomValue::Value { value: 2.25 },
        d50: RoomValue::Value { value: 0.4375 },
        curvature: None,
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
        ceiling_bound: Dbfs(-3.0),
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
        spl: SplCal {
            sensitivity: Db(120.5),
            method: CalMethod::Acoustic {
                calibrator_level: DbSpl(94.0),
            },
            freq: Hz(1000.0),
            measured: Dbfs(-26.5),
            calibrated_at: WallNs(1_789_000_000_000_000_000),
        },
    }
}

fn electrical_cal_entry() -> CalEntry {
    CalEntry {
        key: CalKey {
            channel: 2,
            ..cal_key()
        },
        spl: SplCal {
            sensitivity: Db(133.98),
            method: CalMethod::Electrical {
                connection: ElectricalConnection::InLine,
                volts: Volts(0.015),
                full_scale: Volts(1.5),
                mic_sensitivity: MvPerPa(15.0),
                mic_sensitivity_from: SensitivitySource::DataSheet {
                    label: "0°".into(),
                    file_name: "449350_34804_0Grad.txt".into(),
                },
                uncertainty: Db(1.0),
            },
            freq: Hz(1000.0),
            measured: Dbfs(-40.0),
            calibrated_at: WallNs(1_789_000_000_000_000_000),
        },
    }
}

fn electrical_basis() -> CalBasis {
    CalBasis::Electrical {
        connection: ElectricalConnection::Injected,
        mic_sensitivity: MvPerPa(15.0),
        data_sheet: false,
        uncertainty: Db(1.0),
    }
}

fn curve_id() -> MicCurveId {
    MicCurveId {
        mic: "M30 #1234".into(),
        label: "0°".into(),
    }
}

fn mic() -> Mic {
    Mic {
        name: "M30 #1234".into(),
        curves: vec![
            mic_curve_ref(),
            MicCurveRef {
                label: "90°".into(),
                file_name: "M30-1234-90.frd".into(),
                content_hash: "0123456789abcdef".into(),
                stated_sensitivity: Some(15.0),
                ..mic_curve_ref()
            },
        ],
    }
}

fn mic_curve_ref() -> MicCurveRef {
    MicCurveRef {
        label: "0°".into(),
        file_name: "M30-1234.frd".into(),
        content_hash: "af63bd4c8601b7df".into(),
        points: 2,
        f_lo: Hz(20.0),
        f_hi: Hz(20_000.0),
        imported_at: WallNs(1_788_000_000_000_000_000),
        stated_sensitivity: None,
    }
}

fn outputs() -> Vec<OutputSetup> {
    vec![
        OutputSetup {
            channel: 0,
            label: Some("Main L".into()),
        },
        OutputSetup {
            channel: 3,
            label: Some("Sub".into()),
        },
    ]
}

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

fn inputs() -> Vec<InputSetup> {
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
            replay: None,
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
fn replay_session() -> Session {
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

fn recording_file() -> RecordingFile {
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
    let mut limits = [None; BAND_COUNT];
    limits[5] = Some(DbSpl(48.0));
    limits[17] = Some(DbSpl(30.5));
    cfg.windows.push(BandWindow {
        duration: Seconds(900.0),
        weighting: Weighting::A,
        limits: BandLimitSet::Always { limits },
        warn_margin: Db(2.0),
    });
    cfg.bands.push(Hz(1000.0));
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
        frozen: false,
        delay: None,
        grid_id: None,
    }
}

fn spl_log() -> SplLog {
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

fn spl_history() -> SplHistory {
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

fn spl_band_log() -> SplBandLog {
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

fn spl_log_page() -> SplLogPage {
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
        measurements: vec![measurement()],
        traces: vec![trace_meta()],
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
        ev(51, Change::Mic(Patch::Set(mic()))),
        ev(52, Change::SplLog(Patch::Set(spl_log()))),
        ev(53, Change::SplLog(Patch::Deleted(MeasId(4)))),
        ev(54, Change::Timing(timing())),
        ev(55, Change::Sweep(sweep_run())),
        ev(56, Change::Trace(Patch::Set(sweep_meta()))),
        ev(
            57,
            Change::Autosave(Autosave {
                state: AutosaveState::Failed {
                    reason: "No space left on device (os error 28)".into(),
                },
                saved_at: Some(WallNs(1_790_000_000_000_000_000)),
            }),
        ),
        ev(58, Change::Mic(Patch::Deleted("ECM".into()))),
        ev(59, Change::Measurement(Patch::Set(spl_measurement()))),
        ev(60, Change::Recording(recording_run())),
        ev(61, Change::Session(replay_session())),
        ev(62, Change::Measurement(Patch::Set(math_measurement()))),
        ev(63, Change::Trace(Patch::Set(math_trace_meta()))),
        ev(64, Change::Outputs(outputs())),
        ev(
            65,
            Change::Generator(Generator {
                owner: None,
                armed: false,
                firing: false,
                ceiling: Dbfs(-40.0),
                last_action: Some(GenAudit {
                    action: GenAction::CeilingRaised,
                    client: Some(ClientId("laptop".into())),
                    at: WallNs(1_790_000_000_000_000_000),
                }),
                ..generator()
            }),
        ),
        ev(66, Change::Measurement(Patch::Set(sweep_measurement()))),
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
        Ok(ReplyBody::Traces(vec![trace_meta(), imported_trace_meta()])),
        Ok(ReplyBody::TraceData(Box::new(TraceData {
            meta: trace_meta(),
            mag_db: vec![0.0, -3.0, f32::NAN],
            phase_deg: Some(vec![0.0, 45.0, f32::NAN]),
            coherence: None,
            sweep: None,
        }))),
        Ok(ReplyBody::TraceData(Box::new(TraceData {
            meta: sweep_meta(),
            mag_db: vec![-6.0, -6.5, f32::NAN],
            phase_deg: Some(vec![10.0, -20.0, f32::NAN]),
            coherence: None,
            sweep: Some(sweep_data()),
        }))),
        Ok(ReplyBody::Sweep(SweepRun {
            status: SweepStatus::Playing { repeat: 1 },
            ..sweep_run()
        })),
        Ok(ReplyBody::Export {
            file_name: "main-l.csv".into(),
            content: Blob(b"freq_hz,mag_db\n1000,0\n".to_vec()),
        }),
        Ok(ReplyBody::Calibration(cal_entry())),
        Ok(ReplyBody::Calibration(electrical_cal_entry())),
        Ok(ReplyBody::Calibrations {
            calibrations: vec![cal_entry()],
            mics: vec![mic()],
        }),
        Ok(ReplyBody::Inputs(inputs())),
        Ok(ReplyBody::Mic(mic())),
        Ok(ReplyBody::SplLogPage(spl_log_page())),
        Ok(ReplyBody::SplHistory(Box::new(spl_history()))),
        Ok(ReplyBody::SplBandLog(Box::new(spl_band_log()))),
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
        Ok(ReplyBody::Recording(recording_run())),
        Ok(ReplyBody::Recordings(vec![recording_file()])),
        Ok(ReplyBody::Outputs(outputs())),
        Ok(ReplyBody::Server(server_info())),
        Ok(ReplyBody::Server(ServerInfo {
            mode: ServerMode::Local {
                ctrl: "ipc:///run/user/1000/ac2/ctrl.sock".into(),
            },
            recording_dir: None,
        })),
        Ok(ReplyBody::Server(ServerInfo {
            mode: ServerMode::Embedded,
            recording_dir: None,
        })),
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

/// A 480-column TF frame of a math channel averaging four operands, two of them included.
/// Columns 0..4 are thinned and 470.. out of band (NaN). Values are exactly representable
/// so other languages can rebuild them bit for bit.
pub fn tf_frame() -> Frame {
    let n = 480;
    let valid = |i: usize| (4..470).contains(&i);
    let val = |i: usize, v: f32| if valid(i) { v } else { f32::NAN };
    Frame {
        stamp: stamp(Some(log_grid())),
        data: FrameData::Tf(TfFrame {
            meas: MeasId(1),
            meta: TfMeta {
                delay: Seconds(0.0125),
                nudged: Seconds(0.0),
                frozen: false,
                smoothing: Some(Smoothing {
                    fraction: SmoothingFraction::Sixth,
                    mode: SmoothingMode::Magnitude,
                }),
                mic_curve: true,
                math: Some(Box::new(MathState {
                    operands: vec![
                        OperandState {
                            operand: Operand::Meas { meas: MeasId(2) },
                            status: OperandStatus::Included,
                        },
                        OperandState {
                            operand: Operand::Trace { trace: TraceId(7) },
                            status: OperandStatus::Included,
                        },
                        OperandState {
                            operand: Operand::Meas { meas: MeasId(4) },
                            status: OperandStatus::Refused {
                                protection: ProtectionFlags::NO_SIGNAL,
                            },
                        },
                        OperandState {
                            operand: Operand::Meas { meas: MeasId(5) },
                            status: OperandStatus::Stopped,
                        },
                    ],
                    phase: PhaseBasis::SharedTimeBase,
                })),
            },
            mag: (0..n).map(|i| val(i, -6.0 + i as f32 * 0.031_25)).collect(),
            phase: (0..n)
                .map(|i| val(i, ((i * 15) % 720) as f32 * 0.5 - 180.0))
                .collect(),
            coh: (0..n).map(|i| val(i, (i % 33) as f32 / 32.0)).collect(),
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
    let log_bins = g.remove(3);
    let bands = g.remove(1);
    vec![
        tf_frame(),
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
                        basis: CalBasis::Acoustic {
                            calibrator_level: DbSpl(94.0),
                        },
                    },
                    mic_curve: true,
                    math: None,
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
            stamp: stamp(Some(log_bins)),
            data: FrameData::Spec(SpecFrame {
                meas: MeasId(5),
                meta: SpecMeta {
                    window: Window::Hann,
                    scale: LevelScale::Dbfs,
                    cal: CalStatus::Uncalibrated,
                    mic_curve: false,
                    smoothing: Some(SmoothingFraction::Sixth),
                    math: Some(Box::new(MathState {
                        operands: vec![
                            OperandState {
                                operand: Operand::Meas { meas: MeasId(3) },
                                status: OperandStatus::Included,
                            },
                            OperandState {
                                operand: Operand::Trace { trace: TraceId(8) },
                                status: OperandStatus::Mismatch,
                            },
                        ],
                        phase: PhaseBasis::NoPhase,
                    })),
                },
                level: vec![-120.0, -20.0, f32::INFINITY, f32::NAN],
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
                        basis: CalBasis::Acoustic {
                            calibrator_level: DbSpl(114.0),
                        },
                    },
                    mic_curve: true,
                    position: Some(PositionCorrection {
                        level: Db(2.5),
                        peak: Db(1.5),
                    }),
                },
            }),
        },
        Frame {
            stamp: stamp(None),
            data: FrameData::Leq(Box::new(LeqFrame {
                meas: MeasId(4),
                meta: LeqMeta {
                    scale: LevelScale::DbSpl,
                    cal: CalStatus::Verified {
                        calibrated_at: WallNs(1_789_000_000_000_000_000),
                        basis: electrical_basis(),
                    },
                    mic_curve: false,
                    horizon: Seconds(60.0),
                    logged: 1800,
                    run: Some(LeqRun {
                        started_at: WallNs(1_790_000_000_000_000_000),
                        until: WallNs(1_790_001_810_000_000_000),
                        measured: Seconds(1790.0),
                        gaps: Seconds(20.0),
                        trimmed: false,
                        laeq: 97.8,
                        lceq: 110.25,
                        lzeq: 112.5,
                    }),
                    lcpeak: Some(LeqPeak {
                        level: 133.5,
                        judgement: LeqJudgement::Near,
                    }),
                    lafmax: None,
                    position: Some(PositionCorrection {
                        level: Db(2.5),
                        peak: Db(1.5),
                    }),
                },
                leq: vec![96.5, 99.25, 101.5],
                elapsed: vec![60.0, 1800.0, 600.0],
                measured: vec![60.0, 1790.0, 600.0],
                allowed: vec![f32::NAN, f32::NAN, 99.5],
                recover: vec![f32::NAN, 412.0, f32::NAN],
                least: vec![96.5, 99.25, 93.75],
                over_in: vec![f32::NAN, f32::NAN, 1948.5],
                flags: vec![
                    LeqFlags::NONE,
                    LeqFlags::LIMIT
                        .with(LeqFlags::JUDGED)
                        .with(LeqFlags::OVER)
                        .with(LeqFlags::CANNOT_RECOVER)
                        .with(LeqFlags::INCOMPLETE),
                    LeqFlags::LIMIT
                        .with(LeqFlags::JUDGED)
                        .with(LeqFlags::NEAR)
                        .with(LeqFlags::ON_COURSE),
                ],
            })),
        },
        Frame {
            stamp: stamp(None),
            data: FrameData::BandLeq(Box::new(band_leq_frame())),
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

/// A band meter's frame at night, two windows (LZeq 60 min, LAeq 15 min) on three bands
/// (50, 63 Hz, 1 kHz): 63 Hz over and cannot recover in the first, 50 Hz on course, 1 kHz
/// without a limit there.
fn band_leq_frame() -> BandLeqFrame {
    let nan = f32::NAN;
    let judged = LeqFlags::LIMIT.with(LeqFlags::JUDGED);
    BandLeqFrame {
        meas: MeasId(4),
        meta: BandLeqMeta {
            scale: LevelScale::DbSpl,
            cal: CalStatus::Verified {
                calibrated_at: WallNs(1_789_000_000_000_000_000),
                basis: electrical_basis(),
            },
            mic_curve: true,
            horizon: Seconds(60.0),
            correction: Db(5.0),
            limits_from: BandLimitPlace::Transferred,
            bands: vec![4, 5, 17],
            windows: vec![
                BandWindowState {
                    duration: Seconds(3600.0),
                    weighting: Weighting::Z,
                    elapsed: Seconds(1800.0),
                    measured: Seconds(1790.0),
                    period: BandPeriod::Night,
                    period_after_horizon: BandPeriod::Night,
                    worst: Some(1),
                },
                BandWindowState {
                    duration: Seconds(900.0),
                    weighting: Weighting::A,
                    elapsed: Seconds(900.0),
                    measured: Seconds(900.0),
                    period: BandPeriod::Night,
                    period_after_horizon: BandPeriod::Night,
                    worst: Some(1),
                },
            ],
            predicted: Some(PredictedLeq {
                duration: Seconds(3600.0),
                estimate: 23.5,
                at_most: 27.25,
                limit: Some(DbSpl(25.0)),
                judgement: LeqJudgement::Near,
            }),
        },
        leq: vec![58.0, 62.5, 41.0, 31.5, 36.25, 40.0],
        limit: vec![60.0, 58.0, nan, nan, 74.0, 46.5],
        allowed: vec![59.5, nan, nan, nan, 80.0, 49.0],
        recover: vec![nan, 412.0, nan, nan, nan, nan],
        flags: vec![
            judged.with(LeqFlags::NEAR).with(LeqFlags::ON_COURSE),
            judged.with(LeqFlags::OVER).with(LeqFlags::CANNOT_RECOVER),
            LeqFlags::NONE,
            LeqFlags::NONE,
            judged,
            judged,
        ],
    }
}
