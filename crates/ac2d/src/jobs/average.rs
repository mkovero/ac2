//! Live spatial average job: combines the current results of its member transfer
//! measurements into one `tf` stream (`docs/design/spatial-average.md`).
//!
//! The job analyses no audio itself. The capture fan-out still feeds it every hand-off, so
//! it publishes on the same clock as its members, its frames carry the session's sample
//! index, and it goes STALE exactly when they do. Whenever a frame is due it asks every
//! member for its current unsmoothed result (the same request `trace.capture` makes), so the
//! average is formed once per published frame from the members' newest state, never from
//! frames the members happened to publish or from earlier averages.
//!
//! The mathematics is `trace.average`'s (`ac2_traces::ops::average_on_time_base`): all
//! members run in one session epoch, so they share one time base, and each member's phase
//! is re-referred from its own inserted delay to the configured reference before combining.

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use ac2_core::average::DelayReference as CoreReference;
use ac2_proto::frame::{
    AverageMemberState, Frame, FrameData, MemberStatus, ProtectionFlags, TfAverage, TfFrame,
    TfMeta, ValidityMask,
};
use ac2_proto::grid::{GridDef, GridId};
use ac2_proto::model::{AverageReference, SpatialAverageConfig};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{MeasId, Rev, Seconds, SessionEpoch};
use ac2_traces::columns::{Columns, frequencies};
use ac2_traces::ops::{average_on_time_base, capture_columns};

use super::{Analysis, Due, Emitter, Flush, JobCmd, Pace, Probes, SmoothingChange, StampArgs};
use crate::fanout::Block;

/// Longest the job waits for its members' answers before it publishes without the ones
/// that have not answered. A running member answers after at most one drain of its queued
/// audio, a small fraction of this.
const MEMBER_WAIT: Duration = Duration::from_millis(250);

/// What one member gave the average.
#[derive(Debug)]
pub(crate) enum Answer {
    /// No running transfer job.
    Stopped,
    /// Running, but no answer in time, or nothing formed yet.
    NoResult,
    /// Its current result.
    Result(Box<Frame>),
}

pub(crate) struct Average {
    meas: MeasId,
    cfg: SpatialAverageConfig,
    grid: GridDef,
    grid_id: GridId,
    freqs: Vec<f64>,
    epoch: SessionEpoch,
    probes: Arc<Probes>,
    frozen: bool,
    /// The result shown while frozen.
    held: Option<TfFrame>,
    config_rev: Rev,
    applied_at: u64,
    apply_pending: bool,
    end: Option<u64>,
    wall: u64,
    generation: u64,
    pace: Pace,
    /// Newest inserted delay seen per member: the reference while that member is left out.
    seen_delay: Vec<Option<f64>>,
}

impl Average {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        meas: MeasId,
        cfg: SpatialAverageConfig,
        grid: GridDef,
        epoch: SessionEpoch,
        probes: Arc<Probes>,
        frozen: bool,
        config_rev: Rev,
    ) -> Self {
        let seen_delay = vec![None; cfg.members.len()];
        Self {
            meas,
            freqs: frequencies(&grid),
            grid_id: grid.id(),
            grid,
            cfg,
            epoch,
            probes,
            frozen,
            held: None,
            config_rev,
            applied_at: 0,
            apply_pending: true,
            end: None,
            wall: 0,
            generation: 0,
            pace: Pace::new(Duration::ZERO),
            seen_delay,
        }
    }

    /// Every member's current result: all requests go out first, so the members form their
    /// results in parallel and the wait is the slowest member's, not the sum.
    fn ask_members(&self) -> Vec<Answer> {
        let pending: Vec<Option<Receiver<Option<Frame>>>> = self
            .cfg
            .members
            .iter()
            .map(|m| self.probes.get(*m).and_then(|p| p.request()))
            .collect();
        let deadline = Instant::now() + MEMBER_WAIT;
        pending
            .into_iter()
            .map(|rx| match rx {
                None => Answer::Stopped,
                Some(rx) => {
                    let wait = deadline.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(wait) {
                        Ok(Some(f)) => Answer::Result(Box::new(f)),
                        Ok(None) | Err(_) => Answer::NoResult,
                    }
                }
            })
            .collect()
    }

    /// The current combined result before display smoothing (its `smoothing` names the
    /// average's, as a transfer capture's does): held while frozen, else formed from the
    /// members now.
    fn current(&mut self) -> TfFrame {
        if self.frozen
            && let Some(h) = &self.held
        {
            return h.clone();
        }
        let answers = self.ask_members();
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
            &self.grid,
            &self.freqs,
            self.epoch,
            &self.seen_delay,
            &answers,
        );
        c.meta.frozen = self.frozen;
        c.meta.smoothing = self.cfg.smoothing;
        if self.frozen {
            self.held = Some(c.clone());
        }
        c
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

    /// `c` with the average's display smoothing applied.
    fn smoothed(&self, mut f: TfFrame) -> TfFrame {
        let Some(s) = self.cfg.smoothing else {
            return f;
        };
        let cols = Columns {
            mag_db: f.mag,
            phase_deg: Some(f.phase),
            coherence: Some(f.coh),
        };
        let sm = ac2_traces::smooth::smooth(&self.grid, &cols, s);
        f.mag = sm.mag_db;
        f.phase = sm.phase_deg.unwrap_or_default();
        f.coh = sm.coherence.unwrap_or_default();
        f
    }
}

/// Why a member's answer cannot go into the average, if it cannot.
fn usable(a: &Answer, epoch: SessionEpoch, grid_id: GridId) -> Result<&TfFrame, MemberStatus> {
    let f = match a {
        Answer::Stopped => return Err(MemberStatus::Stopped),
        Answer::NoResult => return Err(MemberStatus::Settling),
        Answer::Result(f) => f,
    };
    let FrameData::Tf(tf) = &f.data else {
        return Err(MemberStatus::Stopped);
    };
    // Another epoch has another time base, another grid other columns: neither can be
    // combined (a member restarting with a new grid is refused at `meas.update`).
    if f.stamp.session_epoch != epoch || f.stamp.grid_id != Some(grid_id) {
        return Err(MemberStatus::Settling);
    }
    let refusing = ProtectionFlags(f.stamp.protection.0 & MemberStatus::REFUSING.0);
    if refusing != ProtectionFlags::NONE {
        return Err(MemberStatus::Refused {
            protection: refusing,
        });
    }
    if !tf.validity.contains(&ValidityMask::NONE) {
        return Err(MemberStatus::Settling);
    }
    Ok(tf)
}

/// The spatial average of the members' answers (`answers[k]` is `cfg.members[k]`'s), before
/// display smoothing. `seen_delay[k]` is the newest inserted delay seen from member `k`.
///
/// Fewer than [`SpatialAverageConfig::MIN_MEMBERS`] usable members: every column is NaN
/// with [`ValidityMask::FEW_MEMBERS`] — one member is not an average, and showing it as one
/// would mislead. Otherwise a column has a value only where every included member's has
/// (`ac2_core::average`); elsewhere its mask is the union of theirs.
pub(crate) fn combine(
    meas: MeasId,
    cfg: &SpatialAverageConfig,
    grid: &GridDef,
    freqs: &[f64],
    epoch: SessionEpoch,
    seen_delay: &[Option<f64>],
    answers: &[Answer],
) -> TfFrame {
    let grid_id = grid.id();
    let n = freqs.len();
    let mut members = Vec::with_capacity(cfg.members.len());
    let mut included: Vec<(usize, &TfFrame)> = Vec::new();
    for (k, (m, a)) in cfg.members.iter().zip(answers).enumerate() {
        let status = match usable(a, epoch, grid_id) {
            Ok(tf) => {
                included.push((k, tf));
                MemberStatus::Included
            }
            Err(s) => s,
        };
        members.push(AverageMemberState { meas: *m, status });
    }
    // The reference: the named member's delay (its newest, while it is left out), else an
    // explicit delay. A member never seen falls back to the first included member's delay,
    // and the frame states whichever delay was used.
    let reference = match cfg.reference {
        AverageReference::Fixed { delay } => CoreReference::Fixed(delay.0),
        AverageReference::Member { meas: r } => match cfg.members.iter().position(|m| *m == r) {
            Some(k) => match included.iter().position(|(i, _)| *i == k) {
                Some(j) => CoreReference::Trace(j),
                None => match seen_delay.get(k).copied().flatten() {
                    Some(d) => CoreReference::Fixed(d),
                    None => CoreReference::Trace(0),
                },
            },
            None => CoreReference::Trace(0),
        },
    };
    let average = TfAverage {
        method: cfg.method,
        members,
    };
    let nan = || vec![f32::NAN; n];
    let refused = |delay: f64, average: TfAverage| TfFrame {
        meas,
        meta: TfMeta {
            delay: Seconds(delay),
            frozen: false,
            smoothing: None,
            mic_curve: false,
            average: Some(Box::new(average)),
        },
        mag: nan(),
        phase: nan(),
        coh: nan(),
        validity: vec![ValidityMask::FEW_MEMBERS; n],
    };
    let fixed_delay = match reference {
        CoreReference::Fixed(d) => d,
        CoreReference::Trace(j) => included.get(j).map_or(0.0, |(_, f)| f.meta.delay.0),
    };
    if included.len() < SpatialAverageConfig::MIN_MEMBERS {
        return refused(fixed_delay, average);
    }
    let cols: Vec<Columns> = included
        .iter()
        .filter_map(|(_, f)| capture_columns(&FrameData::Tf((*f).clone())).map(|(_, c)| c))
        .collect();
    let delays: Vec<f64> = included.iter().map(|(_, f)| f.meta.delay.0).collect();
    let mask_of = |i: usize| {
        included.iter().fold(ValidityMask::NONE, |m, (_, f)| {
            m.with(f.validity.get(i).copied().unwrap_or(ValidityMask::NONE))
        })
    };
    let d = match average_on_time_base(grid, freqs, &cols, &delays, cfg.method, reference) {
        Ok(d) => d,
        Err(_) => {
            // No column valid in every member: nothing to show, each column says why.
            let mut c = refused(fixed_delay, average);
            c.validity = (0..n).map(mask_of).collect();
            return c;
        }
    };
    let validity = (0..n)
        .map(|i| {
            if d.columns.mag_db[i].is_finite() {
                ValidityMask::NONE
            } else {
                match mask_of(i) {
                    // Every member valid, no weight left (coherence-weighted, γ² = 0 in all).
                    ValidityMask::NONE => ValidityMask::BELOW_FLOOR,
                    m => m,
                }
            }
        })
        .collect();
    let mic_curve = included.iter().all(|(_, f)| f.meta.mic_curve);
    TfFrame {
        meas,
        meta: TfMeta {
            delay: d.delay,
            frozen: false,
            smoothing: None,
            mic_curve,
            average: Some(Box::new(average)),
        },
        mag: d.columns.mag_db,
        phase: d.columns.phase_deg.unwrap_or_else(nan),
        coh: d.columns.coherence.unwrap_or_else(nan),
        validity,
    }
}

impl Analysis for Average {
    fn push(&mut self, b: &Block) {
        if self.apply_pending {
            self.applied_at = b.start_sample;
            self.apply_pending = false;
        }
        if !self.frozen {
            // The members have taken this audio too: their results may have changed.
            self.generation += 1;
        }
        self.end = Some(b.end_sample());
        self.wall = b.wall_ns;
    }

    fn command(&mut self, c: JobCmd) {
        self.generation += 1;
        match c {
            JobCmd::Freeze(f) => {
                self.frozen = f;
                if !f {
                    self.held = None;
                }
            }
            JobCmd::Smoothing {
                change: SmoothingChange::Transfer(smoothing),
                rev,
            } => {
                self.cfg.smoothing = smoothing;
                self.config_rev = rev;
                self.apply_pending = true;
            }
            // An average holds no audio state of its own: delay, reset, calibration and
            // the rest belong to its members.
            JobCmd::Find { .. }
            | JobCmd::Track { .. }
            | JobCmd::SetDelay { .. }
            | JobCmd::Reset
            | JobCmd::Smoothing { .. }
            | JobCmd::Spl { .. }
            | JobCmd::Cal(_) => {}
        }
    }

    fn emit(&mut self, e: &Emitter) -> Flush {
        let Some(end) = self.end else {
            return Flush::Done;
        };
        let stamp = self.stamp(end);
        let topic = Topic::Data {
            meas: self.meas,
            stream: Stream::Tf,
        };
        let due = self
            .pace
            .due(e, topic, self.generation, &stamp, Instant::now());
        if due == Due::Send {
            let c = self.current();
            let f = self.smoothed(c);
            if !e.send(stamp, FrameData::Tf(f)) {
                self.pace.unsent();
            }
        }
        Flush::from_due(due)
    }

    fn capture(&mut self) -> Option<(StampArgs, FrameData)> {
        let end = self.end?;
        let c = self.current();
        Some((self.stamp(end), FrameData::Tf(c)))
    }
}

#[cfg(test)]
mod tests;
