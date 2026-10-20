//! Import of realistic analyzer exports, ac2 CSV round trips, averaging and math on stored
//! traces with analytic expectations, and session directory round trips.
#![allow(clippy::unwrap_used)]

use std::path::Path;

use ac2_proto::model::*;
use ac2_proto::spl_log::SplLogInfo;
use ac2_proto::units::*;
use ac2_proto::{GridDef, ImportProblem};
use ac2_traces::columns::{Columns, StoredTrace};
use ac2_traces::math::{self, Combine, Input, MathResult, trace_on_grid};
use ac2_traces::ops::{OpError, average};
use ac2_traces::session::{self, SavedDelay, SavedMeasurement, SavedSplLog, Session, SessionError};
use ac2_traces::text::{export_csv, import};

mod averaging;
mod core_proto;
mod import;
mod load_checks;
mod mic_curve;
mod sessions;
mod smoothing;
mod sweep;

use averaging::{binary, level_trace, third_octaves};
use sweep::sweep_trace;

fn fixture(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(p).unwrap()
}

fn grid() -> GridDef {
    GridDef::Log {
        ppo: 48,
        k_min: -240,
        k_max: 239,
    }
}

fn edit(name: &str) -> TraceEdit {
    TraceEdit {
        owner: ac2_proto::model::TraceOwner::Imported,
        name: name.into(),
        color: Rgb { r: 1, g: 2, b: 3 },
        visible: true,
        locked: false,
        order: 0,
        offset: Db(0.0),
        polarity: Polarity::Normal,
        delay_nudge: Seconds(0.0),
        slot: None,
        smoothing: None,
    }
}

fn meta(id: u32, source: TraceSource, delay: f64, kind: TraceKind, g: &GridDef) -> TraceMeta {
    TraceMeta {
        id: TraceId(id),
        edit: edit(&format!("t{id}")),
        kind,
        source,
        grid_id: g.id(),
        delay: Seconds(delay),
        depth: Some(DepthPolicy::EqualConfidence),
        cal: CalState::Uncalibrated,
        mic: None,
        mic_curve: None,
        created_at: WallNs(1_790_000_000_000_000_000),
    }
}

fn captured(epoch: u32) -> TraceSource {
    TraceSource::Captured {
        meas: MeasId(1),
        meas_name: "main".into(),
        epoch: SessionEpoch(epoch),
        at_sample: SampleIndex(48_000),
    }
}

/// A flat 0 dB path arriving `arrival` s late, captured with `inserted` s of reference
/// delay: stored phase = −360·f·(arrival − inserted).
fn delayed(id: u32, epoch: u32, arrival: f64, inserted: f64, coh: f32) -> StoredTrace {
    let g = grid();
    let f = g.centres();
    StoredTrace {
        meta: meta(id, captured(epoch), inserted, TraceKind::Transfer, &g),
        columns: Columns {
            mag_db: vec![0.0; f.len()],
            phase_deg: Some(
                f.iter()
                    .map(|f| {
                        ac2_traces::columns::wrap_deg(-360.0 * f * (arrival - inserted)) as f32
                    })
                    .collect(),
            ),
            coherence: Some(vec![coh; f.len()]),
        },
        grid: g,
        sweep: None,
        ir: None,
        mic_curve: None,
    }
}

fn col(g: &GridDef, hz: f64) -> usize {
    g.centres()
        .iter()
        .position(|f| (f - hz).abs() < 1e-6 * hz)
        .unwrap()
}
