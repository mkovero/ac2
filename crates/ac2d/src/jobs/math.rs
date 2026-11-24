//! Math trace job: evaluates a math trace's expression over its operands' current
//! results and publishes the result as a `tf`, `spec` or `rta` stream
//! (`docs/design/math-traces.md`).
//!
//! The job analyses no audio itself. The capture fan-out still feeds it every hand-off, so
//! it publishes on the same clock as its live operands, its frames carry the session's
//! sample index, and it goes STALE exactly when they do. A frame is due when a live operand
//! has a new result (its job's result generation, read through its probe on each hand-off)
//! or a command changed the math trace; a math trace of stored traces alone only refreshes, as any
//! unchanged result does. Whenever a frame is due it asks
//! every live operand for its current unsmoothed result (the same request `trace.capture`
//! makes), so the result is formed once per published frame from the operands' newest
//! state, never from frames they happened to publish. Stored operands are held as their
//! columns on the result's grid, prepared when the job starts.
//!
//! The mathematics is [`ac2_traces::math`]'s, shared with every test: live operands run in
//! the session epoch, so they share one time base with each other and with the captures of
//! that epoch.

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use ac2_core::spectrum::{column_max, column_mean};
use ac2_proto::frame::{BinsFrame, Frame, FrameData, ProtectionFlags};
use ac2_proto::grid::{GridDef, GridId};
use ac2_proto::model::{LevelScale, MathConfig, Operand, SpecLevel};
use ac2_proto::topic::Topic;
use ac2_proto::units::{MeasId, Rev, SessionEpoch};
use ac2_traces::columns::Columns;

use super::bins::OperandBins;
use super::{
    Analysis, Due, Emitter, Flush, JobCmd, JobCtx, Pace, Probes, SmoothingChange, StampArgs,
};
use crate::fanout::Block;

pub(crate) use combine::{combine, is_filter, needs_shared_time_base};

/// Longest the job waits for its live operands' answers before it publishes without the
/// ones that have not answered. A running job answers after at most one drain of its
/// queued audio, a small fraction of this.
const OPERAND_WAIT: Duration = Duration::from_millis(250);

/// A stored operand as the job holds it: its columns on the result's grid.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Held {
    /// Columns as measured (an applied mic curve baked in), on the result's grid.
    pub(crate) columns: Columns,
    /// The delay its phase is referred to.
    pub(crate) delay: f64,
    /// The epoch whose time base its phase is in, if any.
    pub(crate) time_base: Option<SessionEpoch>,
    /// Level scale (spectrum, RTA).
    pub(crate) scale: Option<LevelScale>,
    /// Tone level or density (spectrum).
    pub(crate) level: Option<SpecLevel>,
    /// A mic curve is in its columns.
    pub(crate) mic_curve: bool,
}

/// A live operand's request in flight (`Ok`), or a stored operand's columns (`Err`).
type Pending = Result<Option<Asked>, Arc<Held>>;

/// A live operand's request: for its result alone, or with its stage bins.
enum Asked {
    Result(Receiver<Option<Frame>>),
    Bins(Receiver<Option<OperandBins>>),
}

/// What one operand gave the math.
#[derive(Debug)]
pub(crate) enum Answer {
    /// A live operand without a running job.
    Stopped,
    /// A live operand running, but without an answer in time, or nothing formed yet.
    NoResult,
    /// A live operand's current result.
    Result(Box<Frame>),
    /// A stored operand.
    Stored(Arc<Held>),
}

/// The grids a math trace works on.
#[derive(Debug, Clone)]
pub(crate) struct Grids {
    /// Where the operands are combined and the capture is: a transfer's log grid, a
    /// spectrum's every bin, an RTA's bands.
    pub(crate) grid: GridDef,
    /// Where live frames go out, when that differs: a spectrum's display columns, each the
    /// highest value among its bins (as a spectrum publishes).
    pub(crate) display: Option<(GridDef, Vec<u32>)>,
}

pub(crate) struct MathJob {
    meas: MeasId,
    cfg: MathConfig,
    grids: Grids,
    grid_id: GridId,
    display_id: Option<GridId>,
    freqs: Vec<f64>,
    epoch: SessionEpoch,
    probes: Arc<Probes>,
    /// Every operand, in expression order: `None` for a live one (asked when due).
    stored: Vec<Option<Arc<Held>>>,
    config_rev: Rev,
    applied_at: u64,
    apply_pending: bool,
    end: Option<u64>,
    wall: u64,
    generation: u64,
    /// When the live operands' results make a new result here ([`pace::ResultPace`]).
    result_pace: pace::ResultPace,
    /// Each operand's state as [`pace::ResultPace::observe`] takes it, kept to reuse.
    operand_states: Vec<Option<Option<u64>>>,
    pace: Pace,
    /// Newest inserted delay seen per operand: the reference while that operand is left out.
    seen_delay: Vec<Option<f64>>,
    /// The `bins/<meas>/<stage>` streams of the layout the newest result states.
    stages: stages::Streams,
}

impl MathJob {
    pub(crate) fn new(
        ctx: JobCtx<MathConfig>,
        grids: Grids,
        stored: Vec<Option<Arc<Held>>>,
        probes: Arc<Probes>,
    ) -> Self {
        let JobCtx {
            meas,
            cfg,
            config_rev,
            epoch,
        } = ctx;
        let seen_delay = stored.iter().map(|s| s.as_ref().map(|h| h.delay)).collect();
        Self {
            meas,
            freqs: grids.grid.centres(),
            grid_id: grids.grid.id(),
            display_id: grids.display.as_ref().map(|(g, _)| g.id()),
            grids,
            cfg,
            epoch,
            probes,
            stored,
            config_rev,
            applied_at: 0,
            apply_pending: true,
            end: None,
            wall: 0,
            generation: 0,
            result_pace: pace::ResultPace::default(),
            operand_states: Vec::new(),
            pace: Pace::new(Duration::ZERO),
            seen_delay,
            stages: stages::Streams::new(meas),
        }
    }

    /// Each operand's state for [`pace::ResultPace::observe`], in expression order: a live
    /// one's result count while it runs. Stored operands never change.
    fn read_operand_states(&mut self) {
        self.operand_states.clear();
        for (o, s) in self.cfg.expr.operands().iter().zip(&self.stored) {
            self.operand_states.push(match (o, s) {
                (Operand::Meas { meas }, None) => Some(self.probes.get(*meas).map(|p| p.results())),
                _ => None,
            });
        }
    }

    /// Every operand's current result, with each live one's stage bins when `bins` (`None`
    /// where it has none): all requests to live operands go out first, so they form their
    /// results in parallel and the wait is the slowest one's, not the sum.
    fn ask_operands(&self, bins: bool) -> (Vec<Answer>, Vec<Option<Vec<BinsFrame>>>) {
        let operands = self.cfg.expr.operands();
        let pending: Vec<Pending> = operands
            .iter()
            .zip(&self.stored)
            .map(|(o, s)| match (o, s) {
                (_, Some(h)) => Err(Arc::clone(h)),
                (Operand::Meas { meas }, None) => Ok(self.probes.get(*meas).and_then(|p| {
                    if bins {
                        p.request_bins().map(Asked::Bins)
                    } else {
                        p.request().map(Asked::Result)
                    }
                })),
                (Operand::Trace { .. } | Operand::Eq { .. }, None) => Ok(None),
            })
            .collect();
        let deadline = Instant::now() + OPERAND_WAIT;
        pending
            .into_iter()
            .map(|p| {
                let wait = || deadline.saturating_duration_since(Instant::now());
                match p {
                    Err(h) => (Answer::Stored(h), None),
                    Ok(None) => (Answer::Stopped, None),
                    Ok(Some(Asked::Result(rx))) => match rx.recv_timeout(wait()) {
                        Ok(Some(f)) => (Answer::Result(Box::new(f)), None),
                        Ok(None) | Err(_) => (Answer::NoResult, None),
                    },
                    Ok(Some(Asked::Bins(rx))) => match rx.recv_timeout(wait()) {
                        Ok(Some(b)) => (Answer::Result(Box::new(b.tf)), b.stages),
                        Ok(None) | Err(_) => (Answer::NoResult, None),
                    },
                }
            })
            .unzip()
    }

    /// The current result before display smoothing, on the combining grid, formed from the
    /// operands now.
    fn current(&mut self) -> FrameData {
        self.current_with(&[]).0
    }

    /// [`MathJob::current`], with the frames of stages `wanted` when the result states a
    /// layout: each combined per bin from the operands' own bins of the same answers.
    fn current_with(&mut self, wanted: &[usize]) -> (FrameData, Vec<BinsFrame>) {
        let (answers, operand_bins) = self.ask_operands(!wanted.is_empty());
        for (seen, a) in self.seen_delay.iter_mut().zip(&answers) {
            if let Answer::Result(f) = a
                && let FrameData::Tf(tf) = &f.data
            {
                *seen = Some(tf.meta.delay.0);
            }
        }
        let mut c = combine(
            self.meas,
            &self.cfg,
            &self.grids.grid,
            &self.freqs,
            self.epoch,
            &self.seen_delay,
            &answers,
        );
        let frames = match &c {
            FrameData::Tf(tf) if !wanted.is_empty() => stages::frames(
                &self.cfg,
                tf,
                &answers,
                &operand_bins,
                &self.seen_delay,
                self.epoch,
                wanted,
            ),
            _ => Vec::new(),
        };
        match &mut c {
            FrameData::Tf(f) => f.meta.smoothing = self.cfg.smoothing,
            FrameData::Spec(f) => f.meta.smoothing = self.cfg.smoothing.map(|s| s.fraction),
            _ => {}
        }
        (c, frames)
    }

    fn stamp(&self, end: u64) -> StampArgs {
        StampArgs {
            audio_sample: end.saturating_sub(1),
            config_rev: self.config_rev,
            applied_at: self.applied_at,
            wall_ns: self.wall,
            grid_id: Some(self.grid_id),
            protection: ProtectionFlags::NONE,
        }
    }

    /// `c` as it goes out live: with the display smoothing applied, and a spectrum gathered
    /// into its display columns.
    fn shown(&self, c: FrameData) -> FrameData {
        let smooth = |mag: Vec<f32>, phase: Option<Vec<f32>>, coh: Option<Vec<f32>>| {
            let cols = Columns {
                mag_db: mag,
                phase_deg: phase,
                coherence: coh,
            };
            match self.cfg.smoothing {
                Some(s) => ac2_traces::smooth::smooth(&self.grids.grid, &cols, s),
                None => cols,
            }
        };
        match c {
            FrameData::Tf(mut f) => {
                let sm = smooth(f.mag, Some(f.phase), Some(f.coh));
                f.mag = sm.mag_db;
                f.phase = sm.phase_deg.unwrap_or_default();
                f.coh = sm.coherence.unwrap_or_default();
                if let Some(r) = f.range.as_mut() {
                    r.lo = smooth(std::mem::take(&mut r.lo), None, None).mag_db;
                    r.hi = smooth(std::mem::take(&mut r.hi), None, None).mag_db;
                }
                FrameData::Tf(f)
            }
            FrameData::Spec(mut f) => {
                let level = smooth(f.level, None, None).mag_db;
                f.level = match &self.grids.display {
                    // Gathered as the spectrum's own frame is: a density by the mean
                    // power over the column (its level for a noise whatever the column
                    // width), a tone level by its highest bin.
                    Some((_, first)) if f.meta.level == SpecLevel::Density => {
                        let v: Vec<f64> = level
                            .iter()
                            .map(|x| 10f64.powf(f64::from(*x) / 10.0))
                            .collect();
                        column_mean(&v, first)
                            .map(|x| (10.0 * x.log10()) as f32)
                            .collect()
                    }
                    Some((_, first)) => {
                        let v: Vec<f64> = level.iter().map(|x| f64::from(*x)).collect();
                        column_max(&v, first).map(|x| x as f32).collect()
                    }
                    None => level,
                };
                FrameData::Spec(f)
            }
            other => other,
        }
    }
}

impl Analysis for MathJob {
    fn result_generation(&self) -> Option<u64> {
        Some(self.generation)
    }

    fn push(&mut self, b: &Block) {
        if self.apply_pending {
            self.applied_at = b.start_sample;
            self.apply_pending = false;
        }
        // The result changes only with a live operand's: a math trace of stored traces is the
        // same until a command changes it (the pace refreshes it for the client), and one of
        // live operands forms a new result at their rate ([`pace`]), not on every hand-off.
        self.read_operand_states();
        if self
            .result_pace
            .observe(b.end_sample(), &self.operand_states)
        {
            self.generation += 1;
        }
        self.end = Some(b.end_sample());
        self.wall = b.wall_ns;
    }

    fn command(&mut self, c: JobCmd) {
        self.generation += 1;
        match c {
            JobCmd::Smoothing {
                change: SmoothingChange::Transfer(smoothing),
                rev,
            } => {
                self.cfg.smoothing = smoothing;
                self.config_rev = rev;
                self.apply_pending = true;
            }
            // A math trace holds no audio state of its own: delay, reset, calibration and
            // the rest belong to its operands.
            JobCmd::Find { .. }
            | JobCmd::Track { .. }
            | JobCmd::GeneratorEdge(_)
            | JobCmd::Tone(_)
            | JobCmd::SetDelay { .. }
            | JobCmd::Reset
            | JobCmd::Smoothing { .. }
            | JobCmd::Spl { .. }
            | JobCmd::Cal(_)
            | JobCmd::Clocks(_) => {}
        }
    }

    fn emit(&mut self, e: &Emitter) -> Flush {
        let Some(end) = self.end else {
            return Flush::Done;
        };
        let stamp = StampArgs {
            grid_id: Some(self.display_id.unwrap_or(self.grid_id)),
            ..self.stamp(end)
        };
        let topic = Topic::Data {
            meas: self.meas,
            stream: self.cfg.domain.stream(),
        };
        let now = Instant::now();
        let due = self.pace.due(e, topic, self.generation, &stamp, now);
        // Stage frames carry the stamp of the result formed beside them, as a transfer's do.
        let bins_stamp = StampArgs {
            grid_id: None,
            ..stamp
        };
        let (wanted, stages_flush) = self.stages.due(e, self.generation, &bins_stamp, now);
        if due == Due::Send || !wanted.is_empty() {
            // A stage frame takes its layout, delay and eligibility from the grid result, so
            // that is formed even when only a stage goes out. That is rare: both paces run
            // on the same generation and stamp, so they fall due together except for a new
            // subscriber to one of them, and a client draws a stage only over its `tf`.
            let (c, frames) = self.current_with(&wanted);
            if let FrameData::Tf(tf) = &c {
                self.stages.follow(tf.meta.bins.as_deref());
            }
            for f in frames {
                let stage = f.meta.stage;
                if !e.send(bins_stamp, FrameData::Bins(f)) {
                    self.stages.unsent(stage);
                }
            }
            if due == Due::Send && !e.send(stamp, self.shown(c)) {
                self.pace.unsent();
            }
        }
        Flush::from_due(due).and(stages_flush)
    }

    fn capture(&mut self) -> Option<(StampArgs, FrameData)> {
        let end = self.end?;
        let c = self.current();
        Some((self.stamp(end), c))
    }
}

mod combine;
mod pace;
mod stages;
#[cfg(test)]
mod tests;
