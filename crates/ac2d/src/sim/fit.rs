//! The absorption fit (`docs/design/spatial-walk.md` §7): one scale per octave band on
//! every surface's absorption, chosen so the simulated T20 at a sweep walk's positions
//! matches the T20 measured there.
//!
//! Rule: start from the scale at which Eyring's T of the room equals the measured T20
//! (fixed-point iterations on Eyring's formula, no simulation), then run simulation passes
//! with a fixed seed. After each pass, per band, a Newton step on log T against log scale:
//! `scale ← scale · (T_sim / T_meas)^(1/p)`, where T ∝ scale^(−p). The first step takes
//! p = 1 (T ∝ 1/absorption where surfaces dominate); later ones take p from the last two
//! passes, clamped to [`SLOPE_MIN`]…[`SLOPE_MAX`], because absorption clipped at 1, the air's
//! share and a field that is not diffuse all make T follow the scale less than inversely.
//! The scale is clamped to [`SCALE_MIN`]…[`SCALE_MAX`]. The fit stops when every fitted band
//! is within [`TOLERANCE`] or after [`MAX_PASSES`]; the last pass is the result. A band the
//! walk gives no T20 keeps the table's absorption (scale 1, assumed).

use std::sync::atomic::AtomicBool;

use ac2_proto::model::{SimAbsorption, SimFitBand, WalkRoom};
use ac2_proto::units::{Seconds, WalkId};
use ac2_roomsim::{Absorption, BAND_COUNT, Band, PerBand, Prism};

use super::run::{Job, Pass, Reporter, SimEnd, pass};

/// The fit stops once every fitted band's simulated T20 is this close to the measured one
/// (relative): about the spread of T20 between seats of one room.
pub(crate) const TOLERANCE: f64 = 0.03;
/// Simulation passes at most.
pub(crate) const MAX_PASSES: u8 = 6;
/// The least and greatest slope p of log T against log scale a step takes.
pub(crate) const SLOPE_MIN: f64 = 0.2;
pub(crate) const SLOPE_MAX: f64 = 1.5;
/// The least scale: below it a measured decay longer than the air alone allows is
/// being chased.
pub(crate) const SCALE_MIN: f64 = 0.05;
/// The greatest scale (the product with a coefficient is clipped to 1 by the simulator).
pub(crate) const SCALE_MAX: f64 = 20.0;

/// The walk's mean T20 per simulator band, matched by mid-band frequency.
pub(crate) fn measured_t20(room: &WalkRoom) -> [Option<f64>; BAND_COUNT] {
    Band::ALL.map(|b| {
        room.octave
            .iter()
            .find(|w| {
                w.centre
                    .is_some_and(|c| (c.0 / b.exact_hz() - 1.0).abs() < 0.03)
            })
            .and_then(|w| w.t20)
            .map(|m| m.mean)
            .filter(|t| t.is_finite() && *t > 0.0)
    })
}

/// The scale per band at which Eyring's T (with air) equals `measured`; 1 where there is
/// no measurement.
pub(crate) fn eyring_scale(prism: &Prism, measured: &[Option<f64>; BAND_COUNT]) -> PerBand {
    let mut scale = [1.0; BAND_COUNT];
    for _ in 0..40 {
        let t = prism.eyring_t(&scale, true);
        for b in 0..BAND_COUNT {
            if let Some(m) = measured[b] {
                scale[b] = (scale[b] * t[b] / m).clamp(SCALE_MIN, SCALE_MAX);
            }
        }
    }
    scale
}

/// Mean simulated T20 per band over the receivers of the first source that give it.
pub(crate) fn simulated_t20(p: &Pass) -> [Option<f64>; BAND_COUNT] {
    let Some(src) = p.sources.first() else {
        return [None; BAND_COUNT];
    };
    Band::ALL.map(|b| {
        let v: Vec<f64> = src
            .receivers
            .iter()
            .filter_map(|r| {
                r.room
                    .octave
                    .iter()
                    .find(|o| {
                        o.centre
                            .is_some_and(|c| (c.0 / b.exact_hz() - 1.0).abs() < 0.03)
                    })
                    .and_then(|o| o.t20.value())
            })
            .collect();
        (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
    })
}

/// The fit's result: the last pass and the absorption it was made with.
pub(crate) struct Fitted {
    pub(crate) pass: Pass,
    pub(crate) absorption: SimAbsorption,
    pub(crate) scale: PerBand,
}

/// Fits `job` (its first source, its receivers the walk's positions) to `measured`.
pub(crate) fn fit(
    job: &Job,
    walk: WalkId,
    positions: u16,
    measured: &[Option<f64>; BAND_COUNT],
    cancel: &AtomicBool,
    send: &mut dyn FnMut(ac2_proto::model::SimStatus),
) -> Result<Fitted, SimEnd> {
    let mut scale = eyring_scale(&job.prism, measured);
    // The previous pass's scale and simulated T20 per band, for the slope.
    let mut prev: [Option<(f64, f64)>; BAND_COUNT] = [None; BAND_COUNT];
    let mut k = 1;
    loop {
        let mut progress = Reporter::new(send, k);
        let p = pass(job, Absorption::Scaled { scale }, cancel, &mut progress)?;
        let sim = simulated_t20(&p);
        let close = |b: usize| match (sim[b], measured[b]) {
            (Some(s), Some(m)) => (s / m - 1.0).abs() <= TOLERANCE,
            _ => false,
        };
        let done = (0..BAND_COUNT).all(|b| measured[b].is_none() || close(b));
        if done || k >= MAX_PASSES {
            let bands = (0..BAND_COUNT)
                .map(|b| SimFitBand {
                    scale: measured[b].map(|_| scale[b]),
                    measured: measured[b].map(Seconds),
                    simulated: sim[b].map(Seconds),
                    converged: close(b),
                })
                .collect();
            return Ok(Fitted {
                pass: p,
                absorption: SimAbsorption::Fitted {
                    walk,
                    positions,
                    bands,
                },
                scale,
            });
        }
        for b in 0..BAND_COUNT {
            if let (Some(s), Some(m)) = (sim[b], measured[b]) {
                let slope = prev[b]
                    .filter(|(ps, _)| (scale[b] / ps).ln().abs() > 1e-3)
                    .map_or(1.0, |(ps, pt)| -(s / pt).ln() / (scale[b] / ps).ln())
                    .clamp(SLOPE_MIN, SLOPE_MAX);
                prev[b] = Some((scale[b], s));
                scale[b] = (scale[b] * (s / m).powf(1.0 / slope)).clamp(SCALE_MIN, SCALE_MAX);
            }
        }
        k += 1;
    }
}
