//! Sweep measurement (`ir.capture`, `docs/design/sweep-distortion.md`): what a run needs
//! between the control thread, the recorder job and the analysis thread, and the conversion
//! of the core analysis into a stored trace.

use ac2_core::grid::LogGrid;
use ac2_core::room::{BandMetrics, Metric, Refusal};
use ac2_core::sweep::{
    DEFAULT_MAX_ORDER, FLOOR_MARGIN_DB, SweepAnalysis, SweepError, SweepSpec, SweepTiming,
};
use ac2_proto::model::{
    DistortionCurve, EssSpec, HarmonicCurve, RoomAcoustics, RoomBand, RoomRefusal, RoomValue,
    SweepData, SweepFailure, SweepInfo, SweepIr,
};
use ac2_proto::units::{Db, Hz, Seconds};
use ac2_proto::{ErrorCode, GridDef, ProtoError};
use ac2_traces::columns::Columns;

use crate::conv;
use crate::util::perr;

/// Points per octave of a sweep trace's grid (the transfer function's default).
pub(crate) const PPO: u32 = 48;
/// Recorded before the first sweep can have arrived: the output → input latency allowance,
/// seconds. A longer latency cuts the last sweep's silence and the analysis says so.
pub(crate) const LEAD_S: f64 = 1.0;

/// Both inputs of one run, sample-aligned.
#[derive(Debug)]
pub(crate) struct Recording {
    pub(crate) reference: Vec<f32>,
    pub(crate) measurement: Vec<f32>,
}

/// The analysis setup of a request at `fs`.
pub(crate) fn spec(
    sweep: EssSpec,
    level_dbfs: f64,
    gate: Option<Seconds>,
    tail: Option<Seconds>,
    fs: f64,
) -> Result<(SweepSpec, SweepTiming), ProtoError> {
    let ess = conv::ess(sweep);
    if !(ess.start_hz.is_finite() && ess.end_hz.is_finite() && ess.start_hz > 0.0)
        || ess.end_hz <= ess.start_hz
    {
        return Err(perr(
            ErrorCode::Invalid,
            "the sweep must run up from a positive start frequency",
        ));
    }
    let spec = SweepSpec {
        ess,
        level_dbfs,
        sample_rate: fs,
        max_order: DEFAULT_MAX_ORDER,
        gate_s: gate.map(|g| g.0),
        tail_s: tail.map(|t| t.0),
        grid: LogGrid::covering(PPO, ess.start_hz, ess.end_hz),
        lf_harmonics: ac2_core::sweep::LfHarmonics::Standard,
    };
    let timing = SweepTiming::new(&spec).map_err(|e| perr(ErrorCode::Invalid, e.to_string()))?;
    Ok((spec, timing))
}

/// The grid of a sweep trace analysed with `spec`.
pub(crate) fn grid(spec: &SweepSpec) -> GridDef {
    GridDef::Log {
        ppo: spec.grid.ppo,
        k_min: spec.grid.k_min,
        k_max: spec.grid.k_max,
    }
}

/// Why an analysis failed, as a run's failure.
pub(crate) fn failure(e: &SweepError) -> SweepFailure {
    match e {
        SweepError::NoReference { .. } => SweepFailure::NoReference,
        _ => SweepFailure::Analysis,
    }
}

fn f32s(v: &[f64]) -> Vec<f32> {
    v.iter().map(|x| *x as f32).collect()
}

/// A finite value for the session's JSON sidecar (an IR value is never NaN; a defensive
/// floor keeps one from making the session unreadable).
fn finite(v: &[f64], floor: f32) -> Vec<f32> {
    v.iter()
        .map(|x| if x.is_finite() { *x as f32 } else { floor })
        .collect()
}

/// The trace columns and sweep data of an analysis.
pub(crate) fn trace_data(a: &SweepAnalysis) -> (Columns, SweepData) {
    let columns = Columns {
        mag_db: f32s(&a.magnitude_db),
        phase_deg: Some(f32s(&a.phase_deg)),
        coherence: None,
    };
    let curve = |level: &[f64], floor: &[f64]| DistortionCurve {
        level_db: f32s(level),
        floor_db: f32s(floor),
    };
    let data = SweepData {
        harmonics: a
            .harmonics
            .iter()
            .map(|h| HarmonicCurve {
                order: h.order,
                curve: curve(&h.level_db, &h.floor_db),
            })
            .collect(),
        thd: curve(&a.thd_db, &a.thd_floor_db),
        ir: SweepIr {
            t0: Seconds(a.ir_t0_s),
            dt: Seconds(a.ir_dt_s),
            linear: finite(&a.ir, 0.0),
            etc_db: finite(&a.ir_etc_db, ac2_core::ir_view::FLOOR_DB as f32),
        },
        info: SweepInfo {
            sample_rate: Hz(a.sample_rate),
            rate: Seconds(a.rate_s),
            duration: Seconds(a.duration_s),
            repeats: u8::try_from(a.repeats).unwrap_or(u8::MAX),
            arrival: Seconds(a.arrival_s),
            reference_level: Db(a.reference_db),
            window_pre: Seconds(a.harmonic_window_s.0),
            window_post: Seconds(a.harmonic_window_s.1),
            gate_pre: Seconds(a.linear_window_s.0),
            gate: Seconds(a.linear_window_s.1),
            floor_margin: Db(FLOOR_MARGIN_DB),
            clipped: a.clipped,
        },
        room: Some(room(a)),
    };
    (columns, data)
}

/// A finite number for the wire and the session's JSON (a value is never infinite; a
/// defensive refusal keeps one from making a session unreadable).
fn room_value(m: Metric) -> RoomValue {
    match m {
        Ok(value) if value.is_finite() => RoomValue::Value { value },
        Ok(_) => RoomValue::Refused {
            reason: RoomRefusal::NoDecay,
        },
        Err(Refusal::NoDecay) => RoomValue::Refused {
            reason: RoomRefusal::NoDecay,
        },
        Err(Refusal::InsufficientRange {
            range_db,
            needed_db,
        }) => RoomValue::Refused {
            reason: RoomRefusal::InsufficientRange {
                range: Db(range_db.clamp(-999.0, 999.0)),
                needed: Db(needed_db),
            },
        },
        Err(Refusal::FilterLimited { bandwidth_decay }) => RoomValue::Refused {
            reason: RoomRefusal::FilterLimited {
                bandwidth_decay: if bandwidth_decay.is_finite() {
                    bandwidth_decay
                } else {
                    0.0
                },
            },
        },
    }
}

fn room_band(b: &BandMetrics) -> RoomBand {
    RoomBand {
        centre: b.centre_hz.map(Hz),
        onset: Seconds(b.onset_s),
        truncation: Seconds(b.truncation_s),
        decay_range: b
            .decay_range_db
            .filter(|r| r.is_finite())
            .map(|r| Db(r.clamp(-999.0, 999.0))),
        edt: room_value(b.edt_s),
        t20: room_value(b.t20_s),
        t30: room_value(b.t30_s),
        c50: room_value(b.c50_db),
        c80: room_value(b.c80_db),
        d50: room_value(b.d50),
        curvature: b.curvature_pct.filter(|c| c.is_finite()),
    }
}

/// The room parameters of an analysis.
pub(crate) fn room(a: &SweepAnalysis) -> RoomAcoustics {
    RoomAcoustics {
        broadband: room_band(&a.room.broadband),
        octave: a.room.octave.iter().map(room_band).collect(),
        third: a.room.third.iter().map(room_band).collect(),
        span_end: Seconds(a.room_end_s),
    }
}

/// Runs the analysis of `rec`.
pub(crate) fn analyse(
    spec: &SweepSpec,
    rec: &Recording,
    repeats: u8,
) -> Result<SweepAnalysis, SweepError> {
    let r: Vec<f64> = rec.reference.iter().map(|v| f64::from(*v)).collect();
    let m: Vec<f64> = rec.measurement.iter().map(|v| f64::from(*v)).collect();
    ac2_core::sweep::analyse_recording(spec, &r, &m, usize::from(repeats))
}
