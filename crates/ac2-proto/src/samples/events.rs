//! Every event.

use super::builders::*;
use super::state::*;
use super::traces::*;
use crate::event::{Change, Event, Patch};
use crate::model::*;
use crate::units::*;

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
        ev(73, Change::LineCalibration(Patch::Set(line_cal_entry()))),
        ev(74, Change::LineCalibration(Patch::Deleted(line_cal_key()))),
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
                log_failures: vec![LogWriteFailure {
                    meas: MeasId(4),
                    log: MeterLog::Bands,
                    reason: "disk full".into(),
                    since: WallNs(1_790_000_030_000_000_000),
                }],
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
        ev(
            67,
            Change::Working(WorkingSession {
                origin: None,
                dirty: false,
                saved_at: None,
                recover: None,
            }),
        ),
        ev(68, Change::Rig(rig())),
        ev(69, Change::Measurement(Patch::Set(align_measurement()))),
        ev(
            70,
            Change::Measurement(Patch::Set(super::eq::eq_measurement())),
        ),
        ev(71, Change::Trace(Patch::Set(super::eq::eq_trace_meta()))),
        ev(72, Change::Trace(Patch::Set(super::eq::eq_capture_meta()))),
        ev(
            73,
            Change::Measurement(Patch::Set(super::eq::eq_prediction())),
        ),
        ev(80, Change::Plan(Patch::Set(plan()))),
        ev(81, Change::Plan(Patch::Deleted("Club".into()))),
        ev(82, Change::Walk(Patch::Set(walk()))),
        ev(83, Change::Walk(Patch::Deleted(WalkId(2)))),
        ev(84, Change::WalkPlan(Patch::Set(plan()))),
        ev(85, Change::WalkPlan(Patch::Deleted("Club".into()))),
    ]
}
