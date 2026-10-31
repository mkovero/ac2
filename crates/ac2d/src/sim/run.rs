//! One simulation job on the worker: a pass per source, each receiver's result in wire
//! form, its room parameters evaluated by ac2-core on the simulated impulse response.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ac2_core::rta::BandFraction;
use ac2_proto::model::{
    RoomAcoustics, SimAbsorption, SimArrival, SimEchogram, SimReceiver, SimSettings, SimSource,
    SimStage, SimState, SimStatus,
};
use ac2_proto::units::{Hz, Seconds};
use ac2_roomsim::{
    Absorption, BAND_COUNT, Band, Control, Echogram, PerBand, Prism, Progress, Receiver,
    ReceiverResult, SimConfig, SimError, Simulation, Source, Stage, simulate,
};
use rayon::prelude::*;

/// Sample rate of the impulse responses the room parameters are evaluated on: above twice
/// the 8 kHz band's top edge (11.3 kHz), as the simulator's synthesis requires.
const IR_RATE: f64 = 48_000.0;
/// Progress reaches the control thread at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// What a worker simulates: the room, the sources and receivers by name, how.
pub(crate) struct Job {
    pub(crate) prism: Prism,
    pub(crate) sources: Vec<(String, [f64; 3])>,
    pub(crate) receivers: Vec<(String, [f64; 3])>,
    pub(crate) settings: SimSettings,
}

/// A pass's results, before they become a [`SimState`].
pub(crate) struct Pass {
    pub(crate) sources: Vec<SimSource>,
    pub(crate) schroeder_hz: f64,
    pub(crate) eyring: PerBand,
    pub(crate) rays_per_band: u64,
    pub(crate) budget_exhausted: bool,
    pub(crate) lost_rays: u64,
}

impl Pass {
    /// What a map reports besides its grid: its sources (no receivers), and the room's
    /// Schroeder frequency and Eyring times with the absorption it was mapped with.
    pub(crate) fn of_map(job: &Job, scale: Option<PerBand>) -> Pass {
        let scale = scale.unwrap_or([1.0; BAND_COUNT]);
        let eyring = job.prism.eyring_t(&scale, true);
        let t_mid = 0.5 * (eyring[Band::Hz500.index()] + eyring[Band::Hz1k.index()]);
        Pass {
            sources: job
                .sources
                .iter()
                .map(|(name, at)| SimSource {
                    name: name.clone(),
                    at: *at,
                    receivers: Vec::new(),
                    room: None,
                })
                .collect(),
            schroeder_hz: 2000.0 * (t_mid / job.prism.volume()).sqrt(),
            eyring,
            rays_per_band: 0,
            budget_exhausted: false,
            lost_rays: 0,
        }
    }

    /// `state` done with this pass's results and `absorption`.
    pub(crate) fn into_state(self, state: &SimState, absorption: SimAbsorption) -> SimState {
        SimState {
            status: SimStatus::Done,
            absorption,
            sources: self.sources,
            schroeder: Some(Hz(self.schroeder_hz)),
            valid_above: Some(Hz(self.schroeder_hz)),
            eyring: self.eyring.iter().map(|&t| Seconds(t)).collect(),
            rays_per_band: self.rays_per_band,
            budget_exhausted: self.budget_exhausted,
            lost_rays: self.lost_rays,
            ..state.clone()
        }
    }
}

/// The octave bands of every per-band value, nominal.
pub(crate) fn bands() -> Vec<Hz> {
    Band::ALL.iter().map(|b| Hz(b.nominal_hz())).collect()
}

/// Reports progress as statuses, at most every [`PROGRESS_EVERY`] (a stage's start always).
pub(crate) struct Reporter<'a> {
    pub(crate) send: &'a mut dyn FnMut(SimStatus),
    pub(crate) pass: u8,
    last: Option<(Instant, SimStage)>,
}

impl<'a> Reporter<'a> {
    pub(crate) fn new(send: &'a mut dyn FnMut(SimStatus), pass: u8) -> Self {
        Reporter {
            send,
            pass,
            last: None,
        }
    }

    pub(crate) fn report(&mut self, stage: SimStage, done: u64, total: u64, source: usize) {
        let now = Instant::now();
        if self
            .last
            .is_some_and(|(at, s)| s == stage && now - at < PROGRESS_EVERY && done < total)
        {
            return;
        }
        self.last = Some((now, stage));
        (self.send)(SimStatus::Running {
            stage,
            done,
            total,
            source: u16::try_from(source).unwrap_or(u16::MAX),
            pass: self.pass,
        });
    }
}

/// One pass: every source at every receiver with `absorption`, each source's ray tracing
/// under its share of the budget.
pub(crate) fn pass(
    job: &Job,
    absorption: Absorption,
    cancel: &AtomicBool,
    progress: &mut Reporter<'_>,
) -> Result<Pass, SimEnd> {
    let receivers: Vec<Receiver> = job.receivers.iter().map(|r| Receiver { at: r.1 }).collect();
    let config = SimConfig {
        image_order: job.settings.image_order,
        rays_per_band: u64::from(job.settings.rays_per_band),
        seed: job.settings.seed,
        absorption,
        ..SimConfig::default()
    };
    let n = job.sources.len().max(1) as f64;
    let budget = Duration::from_secs_f64(job.settings.budget.0 / n);
    let mut sources = Vec::with_capacity(job.sources.len());
    let mut last: Option<Simulation> = None;
    let (mut rays, mut exhausted, mut lost) = (u64::MAX, false, 0);
    for (k, (name, at)) in job.sources.iter().enumerate() {
        let mut on_progress = |p: Progress| {
            let stage = match p.stage {
                Stage::ImageSources => SimStage::ImageSources,
                Stage::RayTracing => SimStage::RayTracing,
            };
            progress.report(stage, p.done, p.total, k);
        };
        let mut control = Control {
            cancel: Some(cancel),
            budget: Some(budget),
            progress: Some(&mut on_progress),
        };
        let sim = simulate(
            &job.prism,
            &Source { at: *at },
            &receivers,
            &config,
            &mut control,
        )
        .map_err(SimEnd::from)?;
        progress.report(SimStage::Analysis, 0, 1, k);
        let results: Vec<SimReceiver> = sim
            .receivers
            .par_iter()
            .zip(&job.receivers)
            .map(|(r, (label, _))| receiver(label, r, job.settings.seed))
            .collect::<Result<_, _>>()?;
        if cancel.load(Ordering::Relaxed) {
            return Err(SimEnd::Cancelled);
        }
        let rooms: Vec<&RoomAcoustics> = results.iter().map(|r| &r.room).collect();
        let room = crate::control::walk_room(&rooms);
        rays = rays.min(sim.rays_per_band);
        exhausted |= sim.budget_exhausted;
        lost += sim.lost_rays;
        sources.push(SimSource {
            name: name.clone(),
            at: *at,
            receivers: results,
            room,
        });
        last = Some(sim);
    }
    let sim = last.ok_or(SimEnd::Failed("no source".into()))?;
    Ok(Pass {
        sources,
        schroeder_hz: sim.schroeder_hz,
        eyring: sim.eyring_t_s,
        rays_per_band: rays,
        budget_exhausted: exhausted,
        lost_rays: lost,
    })
}

/// Why a job ended without a result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SimEnd {
    Cancelled,
    Failed(String),
}

impl From<SimError> for SimEnd {
    fn from(e: SimError) -> Self {
        match e {
            SimError::Cancelled => SimEnd::Cancelled,
            other => SimEnd::Failed(other.to_string()),
        }
    }
}

/// One receiver's result as the wire carries it.
fn receiver(label: &str, r: &ReceiverResult, seed: u64) -> Result<SimReceiver, SimEnd> {
    Ok(SimReceiver {
        label: label.to_owned(),
        at: r.at,
        arrivals: r
            .arrivals
            .iter()
            .take(SimReceiver::MAX_ARRIVALS)
            .map(|a| SimArrival {
                time: Seconds(a.time_s),
                energy: a.energy.iter().map(|&e| e as f32).collect(),
                path: a.path.iter().map(|&s| super::plan::surface(s)).collect(),
            })
            .collect(),
        late: summary(&r.late),
        room: room(r, seed)?,
    })
}

/// ISO 3382-1 parameters of the simulated impulse response, octave bands 125 Hz … 8 kHz (the
/// simulator's) and broadband, by the code that evaluates a measured one: like with like.
/// Each band is evaluated on the simulator's impulse response of that band alone (its energy
/// band-masked to its crossovers): an octave filter of ours over the whole response would
/// let a slow 125 Hz decay through its skirt and outlast a fast 1 kHz one. One-third octave
/// bands are left out: the simulation has no detail finer than an octave.
pub(crate) fn room(r: &ReceiverResult, seed: u64) -> Result<RoomAcoustics, SimEnd> {
    let failed = |e: ac2_roomsim::IrError| SimEnd::Failed(e.to_string());
    let ir = r.impulse_response(IR_RATE, seed).map_err(failed)?;
    let bands = r.band_impulse_responses(IR_RATE, seed).map_err(failed)?;
    let excited = (Band::Hz125.lower_hz() * 0.99, Band::Hz8k.upper_hz() * 1.01);
    let mut octave = Vec::with_capacity(BAND_COUNT);
    for b in Band::ALL {
        let f = b.exact_hz();
        octave.extend(
            ac2_core::room::analyse_bands(
                &bands[b.index()],
                IR_RATE,
                0.0,
                BandFraction::Octave,
                (f / 1.1, f * 1.1),
                excited,
            )
            .iter()
            .map(crate::sweep::room_band),
        );
    }
    Ok(RoomAcoustics {
        broadband: crate::sweep::room_band(&ac2_core::room::analyse_broadband(&ir, IR_RATE, 0.0)),
        octave,
        third: Vec::new(),
        span_end: Seconds(ir.len() as f64 / IR_RATE),
    })
}

/// The echogram in at most [`SimEchogram::MAX_BINS`] bins of whole simulator bins.
fn summary(e: &Echogram) -> SimEchogram {
    let per = e.len().div_ceil(SimEchogram::MAX_BINS).max(1);
    SimEchogram {
        bin: Seconds(e.bin_s * per as f64),
        energy: (0..BAND_COUNT)
            .map(|b| {
                e.bands[b]
                    .chunks(per)
                    .map(|c| c.iter().sum::<f64>() as f32)
                    .collect()
            })
            .collect(),
    }
}
