//! Dynamics measurements, runs and traces.

use crate::model::*;
use crate::units::*;

use super::builders::edit;
use super::traces::trace_meta;

pub(super) fn dynamics_config() -> DynamicsConfig {
    DynamicsConfig {
        reference_input: 6,
        measurement_inputs: vec![4, 5],
        outputs: vec![2, 3, 4],
        carrier: Hz(1000.0),
        onset: DynamicsOnset::Zero,
        pilot: Dbfs(-33.0),
        bursts: vec![Dbfs(-8.0), Dbfs(0.0)],
        durations: vec![Seconds(0.001), Seconds(0.1)],
        rest_max: Seconds(30.0),
    }
}

/// A dynamics measurement: settings only, never running.
pub fn dynamics_measurement() -> Measurement {
    Measurement {
        id: MeasId(12),
        config: MeasConfig {
            scope: MeasScope::Session,
            name: "Studer VCA".into(),
            kind: MeasKind::Dynamics {
                config: dynamics_config(),
            },
        },
        config_rev: Rev(70),
        edit_rev: Rev(70),
        running: false,
        grid_id: None,
        colour: Some(ColourFamily::Neutral),
        layout: None,
        offset: Db(0.0),
        eq: Vec::new(),
    }
}

pub(super) fn dynamics_run() -> DynamicsRun {
    DynamicsRun {
        id: DynamicsId(2),
        meas: MeasId(12),
        owner: ClientId("alice".into()),
        name: "Run 1".into(),
        reference_input: 6,
        measurement_inputs: vec![4, 5],
        outputs: vec![2, 3, 4],
        carrier: Hz(1000.0),
        onset: DynamicsOnset::Peak,
        pilot: Dbfs(-33.0),
        bursts: vec![Dbfs(-8.0), Dbfs(0.0)],
        durations: vec![Seconds(0.001), Seconds(0.1)],
        rest_max: Seconds(30.0),
        rests_from: Some(TraceId(14)),
        longest: Seconds(124.1),
        status: DynamicsStatus::Done { trace: TraceId(15) },
        started_at: WallNs(1_790_000_000_000_000_000),
    }
}

pub(super) fn dynamics_failed() -> DynamicsRun {
    DynamicsRun {
        status: DynamicsStatus::Failed {
            reason: DynamicsFailure::PilotNotBelowThreshold,
            msg: "pilot is not below threshold".into(),
        },
        rests_from: None,
        ..dynamics_run()
    }
}

pub(super) fn dynamics_playing() -> DynamicsRun {
    DynamicsRun {
        status: DynamicsStatus::Playing {
            step: 3,
            steps: 4,
            phase: DynamicsPhase::Rest,
        },
        ..dynamics_run()
    }
}

pub(super) fn dynamics_meta() -> TraceMeta {
    TraceMeta {
        id: TraceId(15),
        edit: TraceEdit {
            owner: TraceOwner::Meas { meas: MeasId(12) },
            ..edit()
        },
        kind: TraceKind::Dynamics,
        source: TraceSource::Dynamics {
            meas: MeasId(12),
            meas_name: "Studer VCA".into(),
            run: DynamicsId(2),
            number: 1,
            epoch: StreamEpoch(2),
        },
        depth: None,
        lf_window: None,
        cal: CalState::Uncalibrated,
        ..trace_meta()
    }
}

fn harmonics(first: DynamicsLevel) -> DynamicsHarmonics {
    DynamicsHarmonics {
        h: [
            first,
            DynamicsLevel::BelowFloor,
            DynamicsLevel::CantTell,
            DynamicsLevel::BelowFloor,
        ],
        thd: first,
    }
}

pub fn dynamics_data() -> DynamicsData {
    let level = |v| DynamicsLevel::Db { value: Db(v) };
    let row = DynamicsRow {
        gr_end: Db(7.5),
        gr_settled: Some(Db(8.0)),
        settled: true,
        attack_tau: DynamicsTime::Seconds {
            value: Seconds(0.0018),
        },
        attack_2db: DynamicsTime::WithinPeriod,
        settle_0_5db: DynamicsTime::CantTell,
        overshoot: Some(DynamicsOvershoot {
            level: Db(3.25),
            at: Seconds(0.00025),
            reference: Db(0.125),
        }),
        release_tau: DynamicsTime::Seconds {
            value: Seconds(2.75),
        },
        recovery_2db: DynamicsTime::NoGainReduction,
        recovery_0_5db: DynamicsTime::Seconds {
            value: Seconds(9.5),
        },
        harmonics: DynamicsWindows {
            attack: harmonics(level(-41.5)),
            settled: harmonics(DynamicsLevel::BelowFloor),
            release: harmonics(level(-60.0)),
            rest: harmonics(DynamicsLevel::CantTell),
        },
        thump_start: DynamicsThump {
            peak: level(-38.0),
            rms: DynamicsLevel::BelowFloor,
        },
        thump_end: DynamicsThump {
            peak: DynamicsLevel::CantTell,
            rms: DynamicsLevel::CantTell,
        },
        drift: Db(-0.0625),
    };
    let h_db = [-40.5, f32::NAN, -70.0, f32::NAN];
    let mut edges = DynamicsEdges::default();
    edges.start.push(-0.05, 0.0, h_db);
    edges.start.push(0.00025, 4.5, h_db);
    edges.end.push(0.5, 1.25, h_db);
    DynamicsData {
        info: DynamicsInfo {
            sample_rate_hz: Hz(48_000.0),
            carrier: Hz(1000.0),
            period: 48,
            onset: DynamicsOnset::Peak,
            pilot: Dbfs(-33.0),
            bursts: vec![Dbfs(-8.0)],
            durations: vec![Seconds(0.001)],
            rest_max: Seconds(30.0),
            replayed: false,
            reference_input: 6,
            steps: vec![DynamicsStep {
                level: Dbfs(-8.0),
                burst: Seconds(0.001),
                rest: Seconds(4.5),
                recovered: true,
            }],
        },
        inputs: vec![DynamicsInput {
            input: 4,
            name: "VCA L".into(),
            delay: Seconds(0.0000625),
            g0: Db(-0.25),
            rows: vec![row],
        }],
        envelopes: vec![vec![edges]],
    }
}
