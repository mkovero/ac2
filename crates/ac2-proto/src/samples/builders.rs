//! Small values the messages are built from: grids, configs, measurements, generator,
//! calibration and mic curves.

use crate::grid::GridDef;
use crate::model::*;
use crate::units::*;

use super::traces::*;

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

pub(super) fn token() -> LeaseToken {
    LeaseToken(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210)
}

pub(super) fn session_config() -> SessionConfig {
    SessionConfig {
        backend: Some(BackendKind::CoreAudio),
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

pub(super) fn settings() -> GeneratorSettings {
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

pub(super) fn sweep() -> EssSpec {
    EssSpec {
        start: Hz(20.0),
        end: Hz(20_000.0),
        duration: Seconds(5.0),
        fade_in: Seconds(0.01),
        fade_out: Seconds(0.01),
    }
}

pub(super) fn meas_config() -> MeasConfig {
    MeasConfig {
        scope: crate::model::MeasScope::Session,
        name: "Main L".into(),
        kind: MeasKind::Transfer {
            config: TransferConfig {
                reference_input: 0,
                measurement_input: 1,
                averaging: TfAveraging::Fifo { blocks: 8 },
                resolution: Resolution::NinetySixth,
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

pub(super) fn cal_key() -> CalKey {
    CalKey {
        device: DeviceId("hw:UMC1820".into()),
        channel: 1,
        mic: "M30 #1234".into(),
    }
}

pub(super) fn edit() -> TraceEdit {
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
pub(super) fn sweep_config() -> SweepConfig {
    SweepConfig {
        reference_input: 1,
        measurement_input: 0,
        outputs: vec![0, 1],
        level: Dbfs(-50.0),
        sweep: sweep(),
        repeats: 2,
        gate: Some(Seconds(0.005)),
        tail: Some(Seconds(3.0)),
        lf_harmonics: LfHarmonics::Fine,
        resolution: Resolution::TwentyFourth,
    }
}

/// A transfer pane beside an SPL meter, the transfer one in its phase view with its level
/// axis zoomed and its cursor values dragged to the bottom-left at twice the text size, the
/// meter picked for measurement 2.
pub fn pane_layout() -> PaneLayout {
    use crate::model::layout::*;
    let pane = |kind, transfer_view| LayoutPane {
        kind,
        transfer_view,
        spectrum_view: SpectrumView::Spectrograph,
        spl_view: SplView::MeterLeq,
        sweep_view: SweepView::Response,
        chrome: PlotChrome::Full,
        meas: None,
        levels: LayoutLevels::default(),
        plates: LayoutPlates::default(),
    };
    PaneLayout::Split {
        axis: SplitAxis::Row,
        ratio: 0.625,
        a: Box::new(PaneLayout::Pane {
            pane: Box::new(LayoutPane {
                levels: LayoutLevels {
                    transfer: Some(LevelRange {
                        lo: -24.0,
                        hi: 12.0,
                    }),
                    ..LayoutLevels::default()
                },
                plates: LayoutPlates {
                    readout: LayoutPlate {
                        at: Some([0.0, 1.0]),
                        scale: 2.0,
                    },
                    ..LayoutPlates::default()
                },
                ..pane(PaneKind::Transfer, TransferView::Phase)
            }),
        }),
        b: Box::new(PaneLayout::Pane {
            pane: Box::new(LayoutPane {
                chrome: PlotChrome::NoGrid,
                meas: Some(MeasId(2)),
                ..pane(PaneKind::Spl, TransferView::Magnitude)
            }),
        }),
    }
}

/// The working session of [`state`](fn@super::state).
pub fn working() -> WorkingSession {
    WorkingSession {
        origin: Some(SessionOrigin::Name {
            name: "friday show".into(),
        }),
        dirty: true,
        saved_at: Some(WallNs(1_789_990_000_000_000_000)),
        recover: Some(Recoverable {
            replaced_at: WallNs(1_789_995_000_000_000_000),
            origin: Some(SessionOrigin::Client {
                path: "/run/media/foh/STICK/thursday.ac2session".into(),
            }),
            measurements: 3,
            traces: 12,
        }),
    }
}

/// A sweep measurement: settings only, never running.
pub fn sweep_measurement() -> Measurement {
    Measurement {
        id: MeasId(7),
        config: MeasConfig {
            scope: crate::model::MeasScope::Session,
            name: "Genelec 1 m".into(),
            kind: MeasKind::Sweep {
                config: sweep_config(),
            },
        },
        config_rev: Rev(66),
        running: false,
        delay: None,
        grid_id: None,
        colour: Some(ColourFamily::Neutral),
        layout: None,
        compared: Vec::new(),
        offset: Db(0.0),
        eq: Vec::new(),
    }
}

/// A math channel: the coherence-weighted average of two live transfer measurements and a
/// stored trace.
pub fn math_measurement() -> Measurement {
    Measurement {
        id: MeasId(6),
        config: MeasConfig {
            scope: crate::model::MeasScope::Session,
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
        delay: None,
        grid_id: Some(log_grid().id()),
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: Db(0.0),
        eq: Vec::new(),
    }
}

/// An alignment: the live main against a stored sub capture, 60–120 Hz.
pub(super) fn align_measurement() -> Measurement {
    let mut m = math_measurement();
    m.id = MeasId(9);
    m.config.name = "Sub ↔ Main".into();
    m.config.kind = MeasKind::Math {
        config: MathConfig::of(
            TraceOwner::Meas { meas: MeasId(1) },
            MathDomain::Transfer,
            MathExpr::Align {
                a: Operand::Trace { trace: TraceId(7) },
                b: Operand::Meas { meas: MeasId(1) },
                band: AlignBand {
                    lo: Hz(60.0),
                    hi: Hz(120.0),
                },
                max_delay: ALIGN_MAX_DELAY_DEFAULT,
            },
        ),
    };
    m
}

/// A capture of the math channel `Main L ÷ Sub` (a live measurement over a stored trace).
pub(super) fn spectrum_trace_meta() -> TraceMeta {
    TraceMeta {
        id: TraceId(11),
        kind: TraceKind::Spectrum {
            scale: LevelScale::DbSpl,
            level: SpecLevel::Density,
        },
        delay: Seconds(0.0),
        depth: None,
        ..trace_meta()
    }
}

pub(super) fn math_trace_meta() -> TraceMeta {
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

pub(super) fn measurement() -> Measurement {
    Measurement {
        id: MeasId(1),
        config: meas_config(),
        config_rev: Rev(40),
        running: true,
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
        colour: Some(ColourFamily::Hue { index: 5 }),
        layout: Some(pane_layout()),
        compared: Vec::new(),
        offset: Db(0.0),
        eq: Vec::new(),
    }
}

pub(super) fn spectrum_measurement() -> Measurement {
    Measurement {
        id: MeasId(4),
        config: MeasConfig {
            scope: crate::model::MeasScope::Session,
            name: "Spectrum".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig {
                    input: 1,
                    resolution: SpecResolution::MULTI,
                    window: Window::Hann,
                    averaging: SpecAveraging::Fifo { frames: 4 },
                    smoothing: None,
                    level: SpecLevel::Density,
                    display: crate::model::SpectrumDisplay {
                        style: crate::model::SpectrumStyle::Line,
                        ..Default::default()
                    },
                },
            },
        },
        config_rev: Rev(41),
        running: true,
        delay: None,
        grid_id: None,
        colour: None,
        layout: None,
        compared: vec![crate::model::Compared::Meas { meas: MeasId(1) }],
        offset: Db(-6.5),
        eq: Vec::new(),
    }
}

pub(super) fn generator() -> Generator {
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

pub(super) fn cal_entry() -> CalEntry {
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

pub(super) fn electrical_cal_entry() -> CalEntry {
    CalEntry {
        key: CalKey {
            channel: 2,
            ..cal_key()
        },
        spl: SplCal {
            sensitivity: Db(135.31),
            method: CalMethod::Electrical {
                connection: ElectricalConnection::InLine,
                volts: Volts(0.015),
                full_scale: Volts(1.5),
                mic_sensitivity: MvPerPa(15.0),
                mic_sensitivity_from: SensitivitySource::DataSheet {
                    label: "0°".into(),
                    file_name: "449350_34804_0Grad.txt".into(),
                },
                loading: Some(MicLoading {
                    mic_impedance: Ohms(330.0),
                    input_impedance: Ohms(2000.0),
                }),
                uncertainty: Db(1.0),
            },
            freq: Hz(1000.0),
            measured: Dbfs(-40.0),
            calibrated_at: WallNs(1_789_000_000_000_000_000),
        },
    }
}

pub(super) fn electrical_basis() -> CalBasis {
    CalBasis::Electrical {
        connection: ElectricalConnection::Injected,
        mic_sensitivity: MvPerPa(15.0),
        data_sheet: false,
        loading: Some(MicLoading {
            mic_impedance: Ohms(330.0),
            input_impedance: Ohms(2000.0),
        }),
        uncertainty: Db(1.0),
    }
}

pub(super) fn curve_id() -> MicCurveId {
    MicCurveId {
        mic: "M30 #1234".into(),
        label: "0°".into(),
    }
}

pub(super) fn mic() -> Mic {
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

pub(super) fn mic_curve_ref() -> MicCurveRef {
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

pub(super) fn outputs() -> Vec<OutputSetup> {
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
