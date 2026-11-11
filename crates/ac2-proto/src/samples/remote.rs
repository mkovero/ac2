//! Remote inputs: the paired rigs, a session's remote rigs and inputs, a source rig's state.

use crate::model::{
    Anchor, ArrivalReason, CalBasis, CalStatus, CantTellReason, Clocks, CrossRigTransfer,
    CrossTfState, DelaySlip, NotLockedReason, PeerClock, PeerRelation, Relock, RelockOutcome,
    RemoteAudio, RemoteInput, RemoteInputStatus, RemoteLink, RemoteRig, RemoteSide, RemoteState,
    RigPeer, SourceCurve, SourceInput, SourceSession, SourceState, StampMap, StopWhy,
    StreamIncarnation,
};
use crate::units::{DaemonIncarnation, DbSpl, MeasId, RigId, Seconds, SessionEpoch, WallNs};

use super::builders::{cal_entry, mic_curve_ref};
use super::state::{clocks, inputs};

/// ketunkolo, as pupu knows it.
pub(super) fn ketunkolo() -> RigId {
    RigId(0x6b65_7475_6e6b_6f6c)
}

pub(super) fn remote_input() -> RemoteInput {
    RemoteInput {
        slot: 0,
        rig: ketunkolo(),
        rig_name: "ketunkolo".into(),
        channel: 0,
    }
}

pub(super) fn rig_peer() -> RigPeer {
    RigPeer {
        rig: ketunkolo(),
        name: "ketunkolo".into(),
        ctrl: "tcp://192.168.9.20:47820".into(),
        data: "tcp://192.168.9.20:47821".into(),
        key: "rq:rM>}U?@Lns47E1%kR.o@n%FcmmsL/@{H8]yf7".into(),
    }
}

pub(super) fn stamp_map() -> StampMap {
    StampMap {
        epoch_stamp_ns: 812_345_678_901,
        system_ns: 1_790_000_000_123_456_789,
        ppm: 8.2,
    }
}

/// A source rig's clocks with its stamp map.
pub(super) fn source_clocks() -> Clocks {
    Clocks {
        stamp_map: Some(stamp_map()),
        ..clocks()
    }
}

/// What ketunkolo tells pupu.
pub fn source_state() -> SourceState {
    SourceState {
        rig: ketunkolo(),
        name: "ketunkolo".into(),
        clocks: source_clocks(),
        session: Some(SourceSession {
            incarnation: StreamIncarnation {
                daemon: DaemonIncarnation(0x1234),
                epoch: SessionEpoch(3),
            },
            sample_rate_hz: 96_000,
            buffer_frames: 256,
            replaying: false,
            capture_stamps: true,
            inputs: vec![SourceInput {
                channel: 1,
                setup: inputs().inputs.first().cloned(),
                cal_status: CalStatus::Verified {
                    calibrated_at: WallNs(1_789_000_000_000_000_000),
                    basis: CalBasis::Acoustic {
                        calibrator_level: DbSpl(94.0),
                    },
                },
                calibration: Some(cal_entry()),
                curve: Some(SourceCurve {
                    reference: mic_curve_ref(),
                    freq: vec![20.0, 1000.0, 20_000.0],
                    gain_db: vec![-0.5, 0.0, 1.5],
                }),
            }],
        }),
    }
}

fn input_status(slot: u16, audio: RemoteAudio, anchor: Option<Anchor>) -> RemoteInputStatus {
    RemoteInputStatus {
        slot,
        audio,
        anchor,
        late_frames: 0,
        late_frames_hour: 0,
        slips: 0,
        last_slip: None,
        arrival_margin: None,
        setup: None,
        cal_status: CalStatus::Uncalibrated,
        calibration: None,
        curve: None,
    }
}

/// pupu taking ketunkolo's mic, and every state an input, link and relation can be in.
pub fn remote_state() -> RemoteState {
    let rig = |link: RemoteLink, relation: PeerRelation| RemoteRig {
        rig: ketunkolo(),
        name: "ketunkolo".into(),
        link,
        clock: PeerClock {
            relation,
            ppm: Some(0.03),
            span: Some(Seconds(60.0)),
            last_change: Some(WallNs(1_790_000_000_000_000_000)),
        },
        bytes_per_s: 392_000.0,
        window: 262_144,
    };
    RemoteState {
        peers: vec![rig_peer()],
        rigs: vec![
            rig(RemoteLink::Up, PeerRelation::Agree),
            rig(RemoteLink::NotPaired, PeerRelation::Measuring),
            rig(
                RemoteLink::NotNetworked,
                PeerRelation::NotLocked {
                    reason: NotLockedReason::CrossRate {
                        ppm: 5.6,
                        over: Seconds(40.0),
                    },
                },
            ),
            rig(
                RemoteLink::Connecting,
                PeerRelation::NotLocked {
                    reason: NotLockedReason::OwnClock {
                        rig: ketunkolo(),
                        ppm: 5.6,
                    },
                },
            ),
            rig(
                RemoteLink::Unreachable {
                    since: WallNs(1_790_000_000_000_000_000),
                },
                PeerRelation::CantTell {
                    reason: CantTellReason::SystemClockUnsynced { rig: ketunkolo() },
                },
            ),
            rig(
                RemoteLink::NotAdmitted,
                PeerRelation::CantTell {
                    reason: CantTellReason::SystemClockNotStated { rig: ketunkolo() },
                },
            ),
            rig(
                RemoteLink::KeyChanged,
                PeerRelation::CantTell {
                    reason: CantTellReason::NotAgainstSystemClock { rig: ketunkolo() },
                },
            ),
            rig(
                RemoteLink::OtherRig { found: RigId(9) },
                PeerRelation::CantTell {
                    reason: CantTellReason::NoCaptureStamps,
                },
            ),
            rig(
                RemoteLink::Up,
                PeerRelation::CantTell {
                    reason: CantTellReason::ClocksDisagree {
                        by: Seconds(0.0032),
                    },
                },
            ),
            rig(
                RemoteLink::Up,
                PeerRelation::CantTell {
                    reason: CantTellReason::NoAudio,
                },
            ),
        ],
        inputs: vec![
            RemoteInputStatus {
                late_frames: 2,
                late_frames_hour: 1,
                slips: 3,
                last_slip: Some(-470),
                arrival_margin: Some(Seconds(0.0034)),
                setup: inputs().inputs.first().cloned(),
                cal_status: CalStatus::Verified {
                    calibrated_at: WallNs(1_789_000_000_000_000_000),
                    basis: CalBasis::Acoustic {
                        calibrator_level: DbSpl(94.0),
                    },
                },
                calibration: Some(cal_entry()),
                curve: Some(mic_curve_ref()),
                ..input_status(0, RemoteAudio::Streaming, Some(Anchor::SystemTime))
            },
            input_status(
                1,
                RemoteAudio::Starting,
                Some(Anchor::Arrival {
                    reason: ArrivalReason::SystemClock,
                }),
            ),
            input_status(
                2,
                RemoteAudio::Waiting,
                Some(Anchor::Arrival {
                    reason: ArrivalReason::NoCaptureStamps,
                }),
            ),
            input_status(
                3,
                RemoteAudio::OnThisRig,
                Some(Anchor::Arrival {
                    reason: ArrivalReason::NoStampMap,
                }),
            ),
            input_status(
                4,
                RemoteAudio::NoSession,
                Some(Anchor::Arrival {
                    reason: ArrivalReason::ClocksDisagree { by: Seconds(-0.01) },
                }),
            ),
            input_status(5, RemoteAudio::NotCaptured, None),
            input_status(6, RemoteAudio::Replaying, None),
            input_status(
                7,
                RemoteAudio::RateMismatch {
                    theirs: 48_000,
                    ours: 96_000,
                },
                None,
            ),
        ],
        link_offset: Some(Seconds(0.0101)),
        transfers: cross_rig_transfers(),
    }
}

/// Cross-rig transfer functions in every state, relock and cue.
fn cross_rig_transfers() -> Vec<CrossRigTransfer> {
    let t = |meas: u32, side, state, relock, slip, drift: Option<f64>, late| CrossRigTransfer {
        meas: MeasId(meas),
        rig: ketunkolo(),
        side,
        state,
        relock,
        slip,
        delay_drift: drift.map(Seconds),
        late,
    };
    let relock = |from, outcome| Relock {
        after: Seconds(190.0),
        from,
        outcome,
    };
    vec![
        t(
            3,
            RemoteSide::Measurement,
            CrossTfState::Running,
            Some(relock(
                StopWhy::NotLocked,
                RelockOutcome::Tracked {
                    delay: Seconds(0.01341),
                    was: Seconds(0.01238),
                },
            )),
            Some(DelaySlip {
                by: Seconds(-0.0049),
            }),
            Some(0.0003),
            true,
        ),
        t(
            4,
            RemoteSide::Reference,
            CrossTfState::Stopped {
                why: StopWhy::NotLocked,
                held: true,
            },
            None,
            None,
            None,
            false,
        ),
        t(
            5,
            RemoteSide::Measurement,
            CrossTfState::Stopped {
                why: StopWhy::CantTell,
                held: false,
            },
            Some(relock(StopWhy::CantTell, RelockOutcome::Ambiguous)),
            None,
            None,
            false,
        ),
        t(
            6,
            RemoteSide::Measurement,
            CrossTfState::Stopped {
                why: StopWhy::Measuring,
                held: false,
            },
            Some(relock(StopWhy::Measuring, RelockOutcome::FindAgain)),
            None,
            None,
            false,
        ),
        t(
            7,
            RemoteSide::Reference,
            CrossTfState::Running,
            Some(relock(StopWhy::NotLocked, RelockOutcome::Tracking)),
            None,
            None,
            false,
        ),
    ]
}
