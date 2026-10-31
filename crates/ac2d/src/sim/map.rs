//! Coverage maps (`sim.map`): the direct and early energy of a plan's speakers on a grid over
//! its zones, image sources only. The early window is specular reflections, which image
//! sources give exactly; the late field is close to uniform over a room and would add the
//! same to every cell, so no rays are traced.

use std::sync::atomic::{AtomicBool, Ordering};

use ac2_proto::model::{PlanSpec, SimMap, SimMapCell, SimMapRequest, SimRefusal, SimStage};
use ac2_proto::units::{Hz, Seconds};
use ac2_roomsim::{
    Absorption, Band, Control, PerBand, Prism, Receiver, ReceiverResult, SimConfig, Source,
    simulate,
};
use rayon::prelude::*;

use super::run::{Reporter, SimEnd};

/// A map's grid: where each receiver of the job sits on it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Grid {
    pub(crate) spacing: f64,
    pub(crate) origin: [f64; 2],
    /// Column, row and position (plan x, y, the zone's ear height) of each cell.
    pub(crate) cells: Vec<(u16, u16, [f64; 3])>,
}

/// Receivers per call of the simulator: the batch is the unit of progress, of the pool's
/// work and of the cancel check.
const BATCH: usize = 256;
/// Batches run together on the pool between progress reports.
const BATCHES_PER_ROUND: usize = 8;

/// The cells of a `spacing` grid over `plan`'s zones: a cell is kept when its centre lies in
/// a zone (the first that holds it gives its ear height) and strictly inside the room.
pub(crate) fn grid(plan: &PlanSpec, prism: &Prism, spacing: f64) -> Result<Grid, SimRefusal> {
    let max = SimMapRequest::MAX_CELLS;
    let too_fine = |cells: f64| SimRefusal::MapTooFine {
        cells: cells.min(f64::from(u32::MAX)) as u32,
        max: max as u32,
    };
    let zones: Vec<_> = plan.zones.iter().filter(|z| z.outline.len() >= 3).collect();
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for v in zones.iter().flat_map(|z| &z.outline) {
        for k in 0..2 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    if !lo[0].is_finite() {
        return Err(SimRefusal::NoZones);
    }
    // Before walking the grid: the zones' area in cells says when it is far too fine.
    let area: f64 = zones.iter().map(|z| ac2_room::plan::area(&z.outline)).sum();
    let estimate = area / (spacing * spacing);
    if estimate > 4.0 * max as f64 {
        return Err(too_fine(estimate));
    }
    let cols = ((hi[0] - lo[0]) / spacing).ceil().max(1.0);
    let rows = ((hi[1] - lo[1]) / spacing).ceil().max(1.0);
    if cols > f64::from(u16::MAX) || rows > f64::from(u16::MAX) {
        return Err(too_fine(cols * rows));
    }
    let origin = [lo[0] + 0.5 * spacing, lo[1] + 0.5 * spacing];
    let mut cells = Vec::new();
    for row in 0..rows as u16 {
        for col in 0..cols as u16 {
            let p = [
                origin[0] + f64::from(col) * spacing,
                origin[1] + f64::from(row) * spacing,
            ];
            let Some(z) = zones
                .iter()
                .find(|z| ac2_room::plan::contains(&z.outline, p))
            else {
                continue;
            };
            let at = [p[0], p[1], z.ear_height];
            if prism.contains(at) {
                cells.push((col, row, at));
            }
        }
    }
    if cells.len() > max {
        return Err(too_fine(cells.len() as f64));
    }
    if cells.is_empty() {
        return Err(SimRefusal::NoZones);
    }
    Ok(Grid {
        spacing,
        origin,
        cells,
    })
}

/// The index of the band with nominal centre `hz`, if the simulator has it.
pub(crate) fn band_index(hz: Hz) -> Option<usize> {
    Band::ALL
        .iter()
        .find(|b| (b.nominal_hz() - hz.0).abs() < 0.5)
        .map(|b| b.index())
}

/// What a map job maps.
pub(crate) struct MapSpec {
    pub(crate) grid: Grid,
    /// Band index; `None`: every band summed.
    pub(crate) band: Option<usize>,
    pub(crate) band_hz: Option<Hz>,
    pub(crate) window: Seconds,
    pub(crate) image_order: u8,
}

/// The energy one receiver collects within `window` of its direct sound, in `band` or summed
/// over the bands. A direct sound the room hides (round a corner) starts the window where it
/// would have arrived.
fn early(r: &ReceiverResult, source: [f64; 3], c: f64, band: Option<usize>, window: f64) -> f64 {
    let t0 = r.arrivals.iter().find(|a| a.path.is_empty()).map_or_else(
        || {
            let d: f64 = (0..3).map(|k| (r.at[k] - source[k]).powi(2)).sum();
            d.sqrt() / c
        },
        |a| a.time_s,
    );
    r.arrivals
        .iter()
        .filter(|a| a.time_s <= t0 + window + 1e-9)
        .map(|a| match band {
            Some(b) => a.energy[b],
            None => a.energy.iter().sum(),
        })
        .sum()
}

/// The map of every source over the grid, summed: progress per round of batches (stage
/// image sources, `done` cells of every source).
pub(crate) fn map(
    prism: &Prism,
    sources: &[(String, [f64; 3])],
    spec: &MapSpec,
    scale: Option<PerBand>,
    cancel: &AtomicBool,
    progress: &mut Reporter<'_>,
) -> Result<SimMap, SimEnd> {
    let config = SimConfig {
        image_order: spec.image_order,
        rays_per_band: 0,
        // No rays: the echogram stays empty, one bin is all it needs.
        bin_s: 0.1,
        duration_s: Some(0.1),
        absorption: scale.map_or(Absorption::Table, |scale| Absorption::Scaled { scale }),
        ..SimConfig::default()
    };
    let c = prism.air().speed_of_sound();
    let receivers: Vec<Receiver> = spec
        .grid
        .cells
        .iter()
        .map(|&(_, _, at)| Receiver { at })
        .collect();
    let batches: Vec<&[Receiver]> = receivers.chunks(BATCH).collect();
    let total = (receivers.len() * sources.len()) as u64;
    let mut energy = vec![0.0_f64; receivers.len()];
    let mut done = 0u64;
    for (k, (_, at)) in sources.iter().enumerate() {
        progress.report(SimStage::ImageSources, done, total, k);
        for (round, group) in batches.chunks(BATCHES_PER_ROUND).enumerate() {
            let results: Vec<Vec<f64>> = group
                .par_iter()
                .map(|batch| {
                    let mut control = Control {
                        cancel: Some(cancel),
                        budget: None,
                        progress: None,
                    };
                    let sim = simulate(prism, &Source { at: *at }, batch, &config, &mut control)
                        .map_err(SimEnd::from)?;
                    Ok(sim
                        .receivers
                        .iter()
                        .map(|r| early(r, *at, c, spec.band, spec.window.0))
                        .collect())
                })
                .collect::<Result<_, SimEnd>>()?;
            if cancel.load(Ordering::Relaxed) {
                return Err(SimEnd::Cancelled);
            }
            let first = round * BATCHES_PER_ROUND * BATCH;
            for (i, e) in results.into_iter().flatten().enumerate() {
                energy[first + i] += e;
            }
            done += group.iter().map(|b| b.len() as u64).sum::<u64>();
            progress.report(SimStage::ImageSources, done, total, k);
        }
    }
    let loudest = energy.iter().copied().fold(0.0, f64::max);
    let cells = spec
        .grid
        .cells
        .iter()
        .zip(&energy)
        .map(|(&(col, row, _), &e)| SimMapCell {
            col,
            row,
            db: if loudest > 0.0 && e > 0.0 {
                ((10.0 * (e / loudest).log10()) as f32).max(SimMap::FLOOR_DB)
            } else {
                SimMap::FLOOR_DB
            },
        })
        .collect();
    Ok(SimMap {
        spacing: spec.grid.spacing,
        origin: spec.grid.origin,
        band: spec.band_hz,
        window: spec.window,
        cells,
    })
}

#[cfg(test)]
mod tests;
