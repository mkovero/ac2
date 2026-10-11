//! Delay findings, stored traces, sweep results and session files.

use crate::model::*;
use crate::units::*;

use super::builders::*;

pub(super) fn arrival(delay_samples: f64, level: f64) -> DelayArrival {
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

pub(super) fn confidence() -> DelayConfidence {
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
pub(super) fn finding() -> DelayFinding {
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

pub(super) fn accepted_finding() -> DelayFinding {
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

pub(super) fn refused_finding() -> DelayFinding {
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

pub(super) fn trace_meta() -> TraceMeta {
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
pub(super) fn imported_trace_meta() -> TraceMeta {
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
            notes: vec![ImportNote::SweepWithoutAnalysis, ImportNote::IrTruncated],
            ir_file: Some(Box::new(IrFile {
                sample_rate: Hz(48_000.0),
                channels: 2,
                channel: 1,
                frames: 200_000,
                kept: TraceIr::MAX_POINTS,
                first: 1_200,
                peak: Seconds(0.026),
                import: IrImport::default(),
                unresolved: Unresolved {
                    resolution: Resolution::FortyEighth,
                    ranges: Vec::new(),
                },
            })),
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

pub(super) fn sweep_run() -> SweepRun {
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
        lf_harmonics: LfHarmonics::Standard,
        resolution: Resolution::FortyEighth,
        status: SweepStatus::Done { trace: TraceId(9) },
        started_at: WallNs(1_790_000_000_000_000_000),
    }
}

pub(super) fn sweep_meta() -> TraceMeta {
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
            lf_harmonics: LfHarmonics::Fine,
            reference_input: 1,
            measurement_input: 0,
        },
        depth: None,
        cal: CalState::Uncalibrated,
        ..trace_meta()
    }
}

pub(super) fn sweep_data() -> SweepData {
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
        ir: TraceIr {
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
        unresolved: Some(SweepUnresolved {
            response: Unresolved {
                resolution: Resolution::FortyEighth,
                ranges: Vec::new(),
            },
            harmonics: Unresolved {
                resolution: Resolution::FortyEighth,
                ranges: vec![FreqRange {
                    lo: Hz(20.0),
                    hi: Hz(212.5),
                }],
            },
        }),
    }
}

pub(super) fn room_band(centre: Option<Hz>) -> RoomBand {
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

pub(super) fn session_file() -> SessionFile {
    SessionFile {
        name: "friday show".into(),
        path: "/home/fohtech/.local/share/ac2/sessions/friday show".into(),
        saved_at: WallNs(1_790_000_000_000_000_000),
        measurements: 2,
        traces: 5,
    }
}
