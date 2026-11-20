//! Pacing of the step analyses: each runs a few samples per sample fed, so no block carries
//! a whole step, and the run's last block finishes what is left.

use super::step::StepJob;
use super::*;

/// Samples of step analysis per input done for each sample fed, while steps wait. A step's
/// analysis is about two passes over its audio, so this finishes a step in about half its
/// length of what follows, while no single block carries more than a few blocks' worth of
/// work: a step is up to 40 s of audio, and analysing it at once holds the caller for about
/// a second at 96 kHz on a desktop, longer on a small computer, and with other jobs
/// competing that outlasts the capture queue.
pub(in crate::dynamics) const WORK_PER_SAMPLE: u64 = 4;

/// The step being analysed: its index, the input in progress, and the inputs done.
#[derive(Debug)]
pub(super) struct Job {
    step: usize,
    input: usize,
    run: StepJob,
    parts: Vec<Partial>,
}

impl Analyser {
    /// Runs the step analyses whose audio is in, `fed × inputs × WORK_PER_SAMPLE` samples
    /// of them, or all of them once the run's audio is in: nothing more arrives to
    /// spread the rest over, and the result is due.
    pub(super) fn work(&mut self, fed: u64) {
        #[cfg(test)]
        {
            self.last_work = 0;
        }
        let Some(cal) = self.cal.clone() else {
            return;
        };
        let inputs = self.store.inputs.len();
        let mut budget = if self.queued == self.prog.steps() {
            u64::MAX
        } else {
            fed * inputs as u64 * WORK_PER_SAMPLE
        };
        let ctx = Ctx {
            mx: &self.mx,
            fs: self.fs,
            carrier_hz: self.prog.carrier_hz(),
        };
        while budget > 0 {
            if self.job.is_none() {
                if self.analysed == self.queued {
                    break;
                }
                let i = self.analysed;
                let f = self.steps[i];
                let (Some(e), Some(end)) = (f.e, f.end) else {
                    return;
                };
                let b = Bounds { s: f.s, e, end };
                let r = self.store.reference();
                let y = self.store.input(0, cal.shift[0]);
                self.job = Some(Job {
                    step: i,
                    input: 0,
                    run: StepJob::new(&ctx, r, y, cal.g0[0], b),
                    parts: Vec::with_capacity(inputs),
                });
            }
            let Some(job) = self.job.as_mut() else {
                break;
            };
            let r = self.store.reference();
            let y = self.store.input(job.input, cal.shift[job.input]);
            #[cfg(test)]
            let before = budget;
            let part = job.run.run(&ctx, r, y, &mut budget);
            #[cfg(test)]
            {
                self.last_work += before - budget;
            }
            let Some(part) = part else {
                continue;
            };
            let b = part.bounds;
            job.parts.push(part);
            job.input += 1;
            if job.input < inputs {
                let k = job.input;
                let y = self.store.input(k, cal.shift[k]);
                job.run = StepJob::new(&ctx, r, y, cal.g0[k], b);
                continue;
            }
            let Some(job) = self.job.take() else {
                break;
            };
            let i = job.step;
            #[cfg(test)]
            for (x, part) in self.thump_excess.iter_mut().zip(&job.parts) {
                x.push(part.thump_excess);
            }
            for (lvl, part) in self.level.iter_mut().zip(job.parts) {
                lvl.push(part);
            }
            let f = self.steps[i];
            self.infos.push(StepInfo {
                level_dbfs: self.prog.step_level_dbfs(i),
                burst_samples: b.e - f.s,
                rest_samples: b.end - b.e,
                recovered: f.recovered.unwrap_or(false),
            });
            if self.prog.step(i).1 + 1 == self.prog.burst_periods.len() {
                for (res, lvl) in self.results.iter_mut().zip(self.level.iter_mut()) {
                    for (row, env) in step::finish_level(std::mem::take(lvl), self.fs, self.p) {
                        res.rows.push(row);
                        res.envelopes.push(env);
                    }
                }
            }
            self.analysed = i + 1;
            if self.analysed == self.prog.steps() {
                self.done = true;
                self.store.trim(self.store.end());
                return;
            }
            let keep = ctx.pre() + ctx.margin() + cal.shift_abs() + self.p;
            self.store.trim(b.end.saturating_sub(keep));
        }
    }
}
