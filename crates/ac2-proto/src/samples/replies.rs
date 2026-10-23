//! Every reply.

use super::builders::*;
use super::events::events;
use super::state::*;
use super::traces::*;
use crate::ctrl::{
    ErrorCode, ErrorDetail, ImportProblem, MicCurveFileReason, ProtoError, Reply, ReplyBody,
    Warning, Welcome,
};
use crate::event::StateSnapshot;
use crate::model::*;
use crate::units::*;

/// One example of every [`Warning`] variant.
pub fn warnings() -> Vec<Warning> {
    vec![
        Warning::RigMonitorBlind {
            meas: vec![MeasId(3), MeasId(7)],
        },
        Warning::RigMonitorMoved {
            meas: vec![MeasId(3)],
        },
    ]
}

/// Sample reply `i` around `result`; the first carries every [`Warning`].
pub fn reply(i: usize, result: Result<ReplyBody, ProtoError>) -> Reply {
    let r = Reply::new(RequestId(i as u64), result);
    if i == 0 {
        r.with_warnings(warnings())
    } else {
        r
    }
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
                        system_default: true,
                    }),
                    output: None,
                    duplex_clock: ClockRelation::SingleCallback,
                    index: IndexExactness::Exact,
                    clock_sources: vec![],
                    notes: vec![],
                }],
            },
            BackendInfo {
                kind: BackendKind::CoreAudio,
                description: "Core Audio".into(),
                availability: Availability::Unavailable {
                    reason: "no devices".into(),
                },
                devices: vec![],
            },
            BackendInfo {
                kind: BackendKind::Asio,
                description: "ASIO drivers".into(),
                availability: Availability::Available,
                devices: vec![DeviceInfo {
                    backend: BackendKind::Asio,
                    host: "asio".into(),
                    id: DeviceId("Fireface USB".into()),
                    name: "Fireface USB".into(),
                    input: Some(DirectionInfo {
                        max_channels: 2,
                        rates_hz: vec![
                            RangeU32 {
                                min: 44_100,
                                max: 44_100,
                            },
                            RangeU32 {
                                min: 48_000,
                                max: 48_000,
                            },
                        ],
                        buffer_frames: Some(RangeU32 { min: 32, max: 2048 }),
                        default_rate_hz: Some(48_000),
                        default_buffer_frames: Some(256),
                        channel_names: Some(vec!["Mic/Line 1".into(), "Mic/Line 2".into()]),
                        system_default: true,
                    }),
                    output: Some(DirectionInfo {
                        max_channels: 2,
                        rates_hz: vec![RangeU32 {
                            min: 48_000,
                            max: 48_000,
                        }],
                        buffer_frames: Some(RangeU32 { min: 32, max: 2048 }),
                        default_rate_hz: Some(48_000),
                        default_buffer_frames: Some(256),
                        channel_names: Some(vec!["Analog 1".into(), "Analog 2".into()]),
                        system_default: true,
                    }),
                    duplex_clock: ClockRelation::SingleCallback,
                    index: IndexExactness::Exact,
                    clock_sources: vec![
                        ClockSource {
                            name: "Internal".into(),
                            current: false,
                        },
                        ClockSource {
                            name: "Word Clock".into(),
                            current: true,
                        },
                    ],
                    notes: vec!["buffer 32–2048 frames in powers of two".into()],
                }],
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
            input_device: DeviceId("jack".into()),
            output_device: DeviceId("jack".into()),
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
        Ok(ReplyBody::Traces(vec![
            trace_meta(),
            imported_trace_meta(),
            generated_trace_meta(),
        ])),
        Ok(ReplyBody::TraceData(Box::new(TraceData {
            meta: trace_meta(),
            mag_db: vec![0.0, -3.0, f32::NAN],
            phase_deg: Some(vec![0.0, 45.0, f32::NAN]),
            coherence: None,
            sweep: None,
            ir: Some(TransferIr {
                sample_rate: Hz(48_000.0),
                ir: TraceIr {
                    t0: Seconds(-2.0 / 48_000.0),
                    dt: Seconds(1.0 / 48_000.0),
                    linear: vec![0.0, -0.125, 0.75, 0.25],
                    etc_db: vec![-200.0, -12.0, -2.5, -9.0],
                },
            }),
        }))),
        Ok(ReplyBody::TraceData(Box::new(TraceData {
            meta: sweep_meta(),
            mag_db: vec![-6.0, -6.5, f32::NAN],
            phase_deg: Some(vec![10.0, -20.0, f32::NAN]),
            coherence: None,
            sweep: Some(sweep_data()),
            ir: None,
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
        Ok(ReplyBody::LineCalibration(line_cal_entry())),
        Ok(ReplyBody::Calibrations {
            calibrations: vec![cal_entry()],
            line_calibrations: vec![line_cal_entry()],
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
        Ok(ReplyBody::Transfer(Transfer {
            transfer: TransferId(4),
            file_name: "friday show.ac2session".into(),
            bytes: 1_234_567,
            sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
            chunk_bytes: 1 << 20,
        })),
        Ok(ReplyBody::Chunk(Blob(vec![0x1f, 0x8b]))),
        Ok(ReplyBody::Salvage(Salvage {
            version: Some(19),
            imported: vec![trace_meta()],
            refused: vec![SalvageRefusal {
                file: "traces/9-0a1b2c3d4e5f6071.csv".into(),
                reason: "ac2 trace CSV version 3; this build reads version 4".into(),
            }],
        })),
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
            code: ErrorCode::Conflict,
            msg: "unsaved changes".into(),
            detail: Some(ErrorDetail::UnsavedChanges {
                origin: Some(SessionOrigin::Path {
                    path: "/srv/shows/friday".into(),
                }),
            }),
        }),
        Err(ProtoError {
            code: ErrorCode::Conflict,
            msg: "session exists".into(),
            detail: Some(ErrorDetail::SessionExists {
                name: "friday show".into(),
                saved_at: WallNs(1_790_000_000_000_000_000),
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
