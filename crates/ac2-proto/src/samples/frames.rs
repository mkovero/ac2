//! Every frame kind.

use super::builders::*;
use super::state::*;
use crate::frame::{
    AudioHealth, BandLeqFrame, BandLeqMeta, BandWindowState, ClipFlags, DaemonHealth, Frame,
    FrameData, FrameStamp, GenSummary, IrFrame, IrMeta, KaMeta, LeqFlags, LeqFrame, LeqMeta,
    LeqPeak, LeqRun, LevelsFrame, LevelsMeta, MathState, OperandState, OperandStatus,
    PreviewLevelsFrame, PreviewLevelsMeta, ProtectionFlags, RtaFrame, RtaMeta, SessionLevelsFrame,
    SpecBand, SpecBands, SpecFrame, SpecMeta, SplFrame, SplMeta, TfFrame, TfMeta, TimingMeta,
    TimingWindow, ValidityMask,
};
use crate::grid::GridDef;
use crate::model::*;
use crate::units::*;

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
                smoothing: Some(Smoothing {
                    fraction: SmoothingFraction::Sixth,
                    mode: SmoothingMode::Magnitude,
                }),
                mic_curve: true,
                unresolved: Some(Unresolved {
                    resolution: Resolution::NinetySixth,
                    ranges: vec![
                        FreqRange {
                            lo: Hz(31.25),
                            hi: Hz(135.5),
                        },
                        FreqRange {
                            lo: Hz(1625.0),
                            hi: Hz(3250.0),
                        },
                    ],
                }),
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
            // On the wire's grids (hundredths of a dB and degree, ten-thousandths of γ²), so
            // they decode to themselves.
            mag: (0..n)
                .map(|i| val(i, (3 * i as i32 - 600) as f32 / 100.0))
                .collect(),
            phase: (0..n)
                .map(|i| val(i, ((i * 15) % 720) as f32 * 0.5 - 180.0))
                .collect(),
            coh: (0..n)
                .map(|i| val(i, ((i % 41) * 250) as f32 / 10_000.0))
                .collect(),
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
                linear: vec![0.0, 0.5, -1.0, 0.3, 1.0e-3, -0.0],
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
                    level: SpecLevel::Density,
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
                    bands: Some(Box::new(SpecBands {
                        bands: vec![
                            SpecBand {
                                window: Seconds(0.6826666666666666),
                                hop: Seconds(0.08533333333333333),
                                sole: Hz(202.875),
                                age: Some(Seconds(0.0)),
                            },
                            SpecBand {
                                window: Seconds(0.021333333333333333),
                                hop: Seconds(0.010666666666666666),
                                sole: Hz(6492.0),
                                age: None,
                            },
                        ],
                        unresolved: Unresolved {
                            resolution: Resolution::FortyEighth,
                            ranges: vec![FreqRange {
                                lo: Hz(7.8125),
                                hi: Hz(101.5),
                            }],
                        },
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
                health: DaemonHealth {
                    host: "pupu".into(),
                    cpu: Some(0.25),
                    audio: Some(AudioHealth {
                        load: Some(0.125),
                        peak: Some(0.5),
                        session_load: None,
                        session_peak: Some(0.75),
                        xruns: 3,
                    }),
                },
            }),
        },
    ]
}

/// A band meter's frame at night, three windows (50 and 63 Hz LZeq 60 min, 1 kHz LAeq 15
/// min): 63 Hz over and cannot recover, 50 Hz on course, 1 kHz under its limit.
pub(super) fn band_leq_frame() -> BandLeqFrame {
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
            windows: vec![
                BandWindowState {
                    band: Hz(50.0),
                    duration: Seconds(3600.0),
                    weighting: Weighting::Z,
                    elapsed: Seconds(1800.0),
                    measured: Seconds(1790.0),
                    period: BandPeriod::Night,
                    period_after_horizon: BandPeriod::Night,
                },
                BandWindowState {
                    band: Hz(63.0),
                    duration: Seconds(3600.0),
                    weighting: Weighting::Z,
                    elapsed: Seconds(1800.0),
                    measured: Seconds(1790.0),
                    period: BandPeriod::Night,
                    period_after_horizon: BandPeriod::Night,
                },
                BandWindowState {
                    band: Hz(1000.0),
                    duration: Seconds(900.0),
                    weighting: Weighting::A,
                    elapsed: Seconds(900.0),
                    measured: Seconds(900.0),
                    period: BandPeriod::Night,
                    period_after_horizon: BandPeriod::Night,
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
        leq: vec![58.0, 62.5, 40.0],
        limit: vec![60.0, 58.0, 46.5],
        allowed: vec![59.5, nan, 49.0],
        recover: vec![nan, 412.0, nan],
        flags: vec![
            judged.with(LeqFlags::NEAR).with(LeqFlags::ON_COURSE),
            judged.with(LeqFlags::OVER).with(LeqFlags::CANNOT_RECOVER),
            judged,
        ],
    }
}
