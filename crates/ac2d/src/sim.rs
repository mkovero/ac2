//! Room simulation on a worker thread (`sim.run`, `sim.fit`; `docs/design/spatial-walk.md`
//! §7): the plan as the simulator's room, a pass per request with its own small thread
//! pool, progress and the result back to the control thread as messages.

pub(crate) mod fit;
pub(crate) mod plan;
pub(crate) mod run;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use ac2_proto::model::{SimAbsorption, SimStatus};
use ac2_proto::units::{SimId, WalkId};
use ac2_roomsim::{Absorption, BAND_COUNT, PerBand};

use crate::control::ControlMsg;
use run::{Job, Pass, Reporter, SimEnd};

/// What a job does with its passes.
pub(crate) enum Mode {
    /// One pass with `scale` (a fit kept from earlier on the same plan) or the table.
    Run {
        scale: Option<PerBand>,
        absorption: SimAbsorption,
    },
    /// Fit to the walk's measured T20 per band.
    Fit {
        walk: WalkId,
        positions: u16,
        measured: [Option<f64>; BAND_COUNT],
    },
}

/// A finished job.
pub(crate) struct Done {
    pub(crate) pass: Pass,
    pub(crate) absorption: SimAbsorption,
    /// The fitted scale per band (a fit's), for later runs on the same plan.
    pub(crate) fitted: Option<PerBand>,
}

/// What a worker tells the control thread.
pub(crate) enum SimMsg {
    Progress(SimStatus),
    Ended(Box<Result<Done, SimEnd>>),
}

/// A running worker: raise `cancel` to stop it; `ended` disconnects when its thread exits.
pub(crate) struct Worker {
    pub(crate) cancel: Arc<AtomicBool>,
    ended: mpsc::Receiver<()>,
}

impl Worker {
    /// Stops the worker and waits up to `bound` for its thread to exit.
    pub(crate) fn stop(&self, bound: Duration) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.ended.recv_timeout(bound);
    }
}

/// Threads of a simulation's pool: half the cores, at most 8, so the audio path, the jobs
/// and the control thread keep theirs.
fn pool_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(2, std::num::NonZeroUsize::get)
        .div_euclid(2)
        .clamp(1, 8)
}

/// Starts `job` on its own thread and pool; messages carry `id`.
pub(crate) fn spawn(
    id: SimId,
    job: Job,
    mode: Mode,
    to: Sender<ControlMsg>,
) -> std::io::Result<Worker> {
    let cancel = Arc::new(AtomicBool::new(false));
    let (alive, ended) = mpsc::channel::<()>();
    let flag = cancel.clone();
    std::thread::Builder::new()
        .name("ac2d-sim".into())
        .spawn(move || {
            let _alive = alive;
            let mut send = |status: SimStatus| {
                let _ = to.send(ControlMsg::Sim {
                    id,
                    msg: SimMsg::Progress(status),
                });
            };
            let result = rayon::ThreadPoolBuilder::new()
                .num_threads(pool_threads())
                .thread_name(|i| format!("ac2d-sim-{i}"))
                .build()
                .map_err(|e| SimEnd::Failed(format!("no thread pool: {e}")))
                .and_then(|pool| pool.install(|| work(&job, mode, &flag, &mut send)));
            let _ = to.send(ControlMsg::Sim {
                id,
                msg: SimMsg::Ended(Box::new(result)),
            });
        })?;
    Ok(Worker { cancel, ended })
}

/// The job's passes, on the calling thread's pool.
pub(crate) fn work(
    job: &Job,
    mode: Mode,
    cancel: &AtomicBool,
    send: &mut dyn FnMut(SimStatus),
) -> Result<Done, SimEnd> {
    match mode {
        Mode::Run { scale, absorption } => {
            let a = scale.map_or(Absorption::Table, |scale| Absorption::Scaled { scale });
            let pass = run::pass(job, a, cancel, &mut Reporter::new(send, 0))?;
            Ok(Done {
                pass,
                absorption,
                fitted: None,
            })
        }
        Mode::Fit {
            walk,
            positions,
            measured,
        } => {
            let f = fit::fit(job, walk, positions, &measured, cancel, send)?;
            Ok(Done {
                pass: f.pass,
                absorption: f.absorption,
                fitted: Some(f.scale),
            })
        }
    }
}
