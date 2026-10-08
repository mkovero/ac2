//! Math channel job: evaluates a math channel's expression over its operands' current
//! results and publishes the result as a `tf`, `spec` or `rta` stream
//! (`docs/design/math-channels.md`).
//!
//! The job analyses no audio itself. The capture fan-out still feeds it every hand-off, so
//! it publishes on the same clock as its live operands, its frames carry the session's
//! sample index, and it goes STALE exactly when they do. A frame is due when a live operand
//! has a new result (its job's result generation, read through its probe on each hand-off)
//! or a command changed the channel; a channel of stored traces alone only refreshes, as any
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

use ac2_core::average::DelayReference as CoreReference;
use ac2_core::spectrum::column_max;
use ac2_proto::frame::{
    Frame, FrameData, MathState, OperandState, OperandStatus, ProtectionFlags, RtaFrame, RtaMeta,
    SpecFrame, SpecMeta, TfFrame, TfMeta, ValidityMask,
};
use ac2_proto::grid::{GridDef, GridId};
use ac2_proto::model::{
    AverageMethod, BandFraction, CalStatus, LevelScale, MathConfig, MathDomain, MathExpr, MathOp,
    MathReference, Operand, PhaseBasis, Weighting, Window,
};
use ac2_proto::topic::Topic;
use ac2_proto::units::{MeasId, Rev, Seconds, SessionEpoch};
use ac2_traces::columns::{Columns, frequencies};
use ac2_traces::math::{self, Combine, Input};
use ac2_traces::ops::capture_columns;

use super::{Analysis, Due, Emitter, Flush, JobCmd, Pace, Probes, SmoothingChange, StampArgs};
use crate::fanout::Block;

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
    /// A mic curve is in its columns.
    pub(crate) mic_curve: bool,
}

/// A live operand's request in flight (`Ok`), or a stored operand's columns (`Err`).
type Pending = Result<Option<Receiver<Option<Frame>>>, Arc<Held>>;

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

/// The grids a math channel works on.
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
    /// The live operands' result generations (and which of them run) when the job last
    /// looked: a new result forms only when this changes.
    operands_seen: Option<u64>,
    pace: Pace,
    /// Newest inserted delay seen per operand: the reference while that operand is left out.
    seen_delay: Vec<Option<f64>>,
}

impl MathJob {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        meas: MeasId,
        cfg: MathConfig,
        grids: Grids,
        stored: Vec<Option<Arc<Held>>>,
        epoch: SessionEpoch,
        probes: Arc<Probes>,
        config_rev: Rev,
    ) -> Self {
        let seen_delay = stored.iter().map(|s| s.as_ref().map(|h| h.delay)).collect();
        Self {
            meas,
            freqs: frequencies(&grids.grid),
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
            operands_seen: None,
            pace: Pace::new(Duration::ZERO),
            seen_delay,
        }
    }

    /// One value for the live operands' state: each running one's result generation, in
    /// expression order, and which are not running. Stored operands never change it.
    fn operands_state(&self) -> u64 {
        self.cfg.expr.operands().iter().zip(&self.stored).fold(
            0xcbf2_9ce4_8422_2325,
            |h, (o, s)| {
                let v = match (o, s) {
                    (Operand::Meas { meas }, None) => self
                        .probes
                        .get(*meas)
                        .map_or(0, |p| p.results().wrapping_add(1)),
                    _ => 0,
                };
                (h ^ v).wrapping_mul(0x0100_0000_01b3)
            },
        )
    }

    /// Every operand's current result: all requests to live operands go out first, so they
    /// form their results in parallel and the wait is the slowest one's, not the sum.
    fn ask_operands(&self) -> Vec<Answer> {
        let operands = self.cfg.expr.operands();
        let pending: Vec<Pending> = operands
            .iter()
            .zip(&self.stored)
            .map(|(o, s)| match (o, s) {
                (_, Some(h)) => Err(Arc::clone(h)),
                (Operand::Meas { meas }, None) => {
                    Ok(self.probes.get(*meas).and_then(|p| p.request()))
                }
                (Operand::Trace { .. }, None) => Ok(None),
            })
            .collect();
        let deadline = Instant::now() + OPERAND_WAIT;
        pending
            .into_iter()
            .map(|p| match p {
                Err(h) => Answer::Stored(h),
                Ok(None) => Answer::Stopped,
                Ok(Some(rx)) => {
                    let wait = deadline.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(wait) {
                        Ok(Some(f)) => Answer::Result(Box::new(f)),
                        Ok(None) | Err(_) => Answer::NoResult,
                    }
                }
            })
            .collect()
    }

    /// The current result before display smoothing, on the combining grid, formed from the
    /// operands now.
    fn current(&mut self) -> FrameData {
        let answers = self.ask_operands();
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
        match &mut c {
            FrameData::Tf(f) => f.meta.smoothing = self.cfg.smoothing,
            FrameData::Spec(f) => f.meta.smoothing = self.cfg.smoothing.map(|s| s.fraction),
            _ => {}
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
                FrameData::Tf(f)
            }
            FrameData::Spec(mut f) => {
                let level = smooth(f.level, None, None).mag_db;
                f.level = match &self.grids.display {
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

/// Why a live operand's answer cannot go in, if it cannot.
fn usable(
    a: &Answer,
    domain: MathDomain,
    epoch: SessionEpoch,
    grid_id: GridId,
) -> Result<&Frame, OperandStatus> {
    let f = match a {
        Answer::Stopped => return Err(OperandStatus::Stopped),
        Answer::NoResult => return Err(OperandStatus::Settling),
        Answer::Stored(_) => return Err(OperandStatus::Mismatch),
        Answer::Result(f) => f,
    };
    let kind_ok = matches!(
        (&f.data, domain),
        (FrameData::Tf(_), MathDomain::Transfer)
            | (FrameData::Spec(_), MathDomain::Spectrum)
            | (FrameData::Rta(_), MathDomain::Rta)
    );
    if !kind_ok {
        return Err(OperandStatus::Mismatch);
    }
    // Another epoch has another time base: a result of a session since closed cannot be
    // combined with this one's.
    if f.stamp.session_epoch != epoch {
        return Err(OperandStatus::Settling);
    }
    // Another grid holds other columns (another FFT length or band layout restarted the
    // operand; a transfer measurement cannot change grid while named).
    if f.stamp.grid_id != Some(grid_id) {
        return Err(OperandStatus::Mismatch);
    }
    let refusing = ProtectionFlags(f.stamp.protection.0 & OperandStatus::REFUSING.0);
    if refusing != ProtectionFlags::NONE {
        return Err(OperandStatus::Refused {
            protection: refusing,
        });
    }
    let any_value = match &f.data {
        FrameData::Tf(t) => t.validity.contains(&ValidityMask::NONE),
        FrameData::Rta(r) => r.validity.contains(&ValidityMask::NONE),
        FrameData::Spec(s) => s.level.iter().any(|v| v.is_finite()),
        _ => false,
    };
    if !any_value {
        return Err(OperandStatus::Settling);
    }
    Ok(f)
}

/// One included operand, as the math takes it.
struct Value {
    columns: Columns,
    delay: f64,
    time_base: Option<SessionEpoch>,
    validity: Option<Vec<ValidityMask>>,
    scale: Option<LevelScale>,
    mic_curve: bool,
    frame: Option<FrameData>,
}

fn value(a: &Answer, f: Option<&Frame>, epoch: SessionEpoch) -> Option<Value> {
    if let Answer::Stored(h) = a {
        return Some(Value {
            columns: h.columns.clone(),
            delay: h.delay,
            time_base: h.time_base,
            validity: None,
            scale: h.scale,
            mic_curve: h.mic_curve,
            frame: None,
        });
    }
    let f = f?;
    let (_, columns) = capture_columns(&f.data)?;
    let (delay, validity, scale, mic_curve) = match &f.data {
        FrameData::Tf(t) => (
            t.meta.delay.0,
            Some(t.validity.clone()),
            None,
            t.meta.mic_curve,
        ),
        FrameData::Spec(s) => (0.0, None, Some(s.meta.scale), s.meta.mic_curve),
        FrameData::Rta(r) => (
            0.0,
            Some(r.validity.clone()),
            Some(r.meta.scale),
            r.meta.mic_curve,
        ),
        _ => return None,
    };
    Some(Value {
        columns,
        delay,
        time_base: Some(epoch),
        validity,
        scale,
        mic_curve,
        frame: Some(f.data.clone()),
    })
}

/// Whether the expression combines phase across operands, so every operand must be in one
/// time base: a transfer sum or difference, or a complex or coherence-weighted average.
pub(crate) fn needs_shared_time_base(c: &MathConfig) -> bool {
    c.domain == MathDomain::Transfer
        && match &c.expr {
            MathExpr::Binary { op, .. } => matches!(op, MathOp::Add | MathOp::Subtract),
            MathExpr::Average { method, .. } => *method != AverageMethod::Power,
        }
}

/// The math channel's result from its operands' answers (`answers[k]` is the expression's
/// `k`-th operand's), before display smoothing, on `grid`. `seen_delay[k]` is the newest
/// inserted delay seen from operand `k`.
///
/// Without the usable operands the expression needs (both of a binary operator, two of an
/// average), every column is NaN with [`ValidityMask::FEW_OPERANDS`]: one position is not an
/// average, and half a ratio is no ratio. Otherwise a column has a value only where every
/// included operand has one; elsewhere its mask is the union of theirs.
pub(crate) fn combine(
    meas: MeasId,
    cfg: &MathConfig,
    grid: &GridDef,
    freqs: &[f64],
    epoch: SessionEpoch,
    seen_delay: &[Option<f64>],
    answers: &[Answer],
) -> FrameData {
    let grid_id = grid.id();
    let n = freqs.len();
    let operands = cfg.expr.operands();
    let shared_needed = needs_shared_time_base(cfg);
    let mut states = Vec::with_capacity(operands.len());
    let mut included: Vec<(usize, Value)> = Vec::new();
    for (k, (o, a)) in operands.iter().zip(answers).enumerate() {
        let frame = match a {
            Answer::Stored(_) => Ok(None),
            _ => usable(a, cfg.domain, epoch, grid_id).map(Some),
        };
        let status = match frame.map(|f| value(a, f, epoch)) {
            Ok(Some(v)) => {
                // Levels combine only in one scale; phase-combining transfer math only in
                // one time base (the first included operand's).
                let first = included.first().map(|(_, f)| f);
                let mismatch = first.is_some_and(|f| {
                    f.scale != v.scale || (shared_needed && f.time_base != v.time_base)
                }) || (shared_needed && v.time_base.is_none());
                if mismatch {
                    OperandStatus::Mismatch
                } else {
                    included.push((k, v));
                    OperandStatus::Included
                }
            }
            Ok(None) => OperandStatus::Settling,
            Err(s) => s,
        };
        states.push(OperandState {
            operand: *o,
            status,
        });
    }
    let enough = match &cfg.expr {
        MathExpr::Binary { .. } => included.len() == 2,
        MathExpr::Average { .. } => included.len() >= MathConfig::MIN_AVERAGE,
    };
    // The reference: the named operand's delay (its newest, while it is left out), else an
    // explicit delay. An operand never seen falls back to the first included operand's, and
    // the frame states whichever delay was used.
    let reference = match cfg.reference {
        MathReference::Fixed { delay } => CoreReference::Fixed(delay.0),
        MathReference::Operand { operand: r } => match operands.iter().position(|o| *o == r) {
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
    let mask_of = |i: usize| {
        included.iter().fold(ValidityMask::NONE, |m, (_, v)| {
            m.with(
                v.validity
                    .as_ref()
                    .and_then(|x| x.get(i).copied())
                    .unwrap_or(ValidityMask::NONE),
            )
        })
    };
    let combine_how = match &cfg.expr {
        MathExpr::Binary { op, .. } => Combine::Binary(*op),
        MathExpr::Average { method, .. } => Combine::Average(*method),
    };
    let inputs: Vec<Input<'_>> = included
        .iter()
        .map(|(_, v)| Input {
            columns: &v.columns,
            delay: v.delay,
            time_base: v.time_base,
        })
        .collect();
    let result = if enough {
        match cfg.domain {
            MathDomain::Transfer => math::transfer(combine_how, &inputs, grid, freqs, reference),
            MathDomain::Spectrum | MathDomain::Rta => math::levels(combine_how, &inputs),
        }
        .ok()
    } else {
        None
    };
    let validity: Vec<ValidityMask> = (0..n)
        .map(|i| match &result {
            None if !enough => ValidityMask::FEW_OPERANDS,
            Some(r) if r.columns.mag_db.get(i).is_some_and(|v| v.is_finite()) => ValidityMask::NONE,
            // Every operand valid but no value: a total cancellation, or no weight left.
            _ => match mask_of(i) {
                ValidityMask::NONE => ValidityMask::BELOW_FLOOR,
                m => m,
            },
        })
        .collect();
    let phase = result.as_ref().map_or(PhaseBasis::NoPhase, |r| r.phase);
    let state = Some(Box::new(MathState {
        operands: states,
        phase,
    }));
    let nan = || vec![f32::NAN; n];
    let mic_curve = !included.is_empty() && included.iter().all(|(_, v)| v.mic_curve);
    let first_frame = included.iter().find_map(|(_, v)| v.frame.as_ref());
    let fixed_delay = match reference {
        CoreReference::Fixed(d) => d,
        CoreReference::Trace(j) => included.get(j).map_or(0.0, |(_, v)| v.delay),
    };
    let (mag, phase_deg, coh, delay) = match result {
        Some(r) => (
            r.columns.mag_db,
            r.columns.phase_deg,
            r.columns.coherence,
            r.delay.0,
        ),
        None => (nan(), None, None, fixed_delay),
    };
    match cfg.domain {
        MathDomain::Transfer => FrameData::Tf(TfFrame {
            meas,
            meta: TfMeta {
                delay: Seconds(delay),
                nudged: Seconds(0.0),
                smoothing: None,
                mic_curve,
                math: state,
            },
            mag,
            phase: phase_deg.unwrap_or_else(nan),
            coh: coh.unwrap_or_else(nan),
            validity,
        }),
        MathDomain::Spectrum => {
            let (window, cal) = match first_frame {
                Some(FrameData::Spec(s)) => (s.meta.window, s.meta.cal),
                // A stored spectrum keeps no window; its level is a tone level either way.
                _ => (Window::Hann, CalStatus::Uncalibrated),
            };
            FrameData::Spec(SpecFrame {
                meas,
                meta: SpecMeta {
                    window,
                    scale: level_scale(&included),
                    cal,
                    mic_curve,
                    smoothing: None,
                    math: state,
                },
                level: mag,
            })
        }
        MathDomain::Rta => {
            let (fraction, weighting, cal) = match first_frame {
                Some(FrameData::Rta(r)) => (r.meta.fraction, r.meta.weighting, r.meta.cal),
                _ => (
                    match grid {
                        GridDef::IecBands { fraction, .. } => *fraction,
                        _ => BandFraction::Third,
                    },
                    // A stored RTA trace keeps no weighting; its bands are as measured.
                    Weighting::Z,
                    CalStatus::Uncalibrated,
                ),
            };
            FrameData::Rta(RtaFrame {
                meas,
                meta: RtaMeta {
                    fraction,
                    weighting,
                    scale: level_scale(&included),
                    cal,
                    mic_curve,
                    math: state,
                },
                level: mag,
                validity,
            })
        }
    }
}

fn level_scale(included: &[(usize, Value)]) -> LevelScale {
    included
        .first()
        .and_then(|(_, v)| v.scale)
        .unwrap_or(LevelScale::Dbfs)
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
        // The result changes only with a live operand's: a channel of stored traces is the
        // same until a command changes it (the pace refreshes it for the client), and one of
        // live operands forms a new result when one of them has, not on every hand-off.
        let state = self.operands_state();
        if self.operands_seen != Some(state) {
            self.operands_seen = Some(state);
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
            // A math channel holds no audio state of its own: delay, reset, calibration and
            // the rest belong to its operands.
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
        let stamp = StampArgs {
            grid_id: Some(self.display_id.unwrap_or(self.grid_id)),
            ..self.stamp(end)
        };
        let topic = Topic::Data {
            meas: self.meas,
            stream: self.cfg.domain.stream(),
        };
        let due = self
            .pace
            .due(e, topic, self.generation, &stamp, Instant::now());
        if due == Due::Send {
            let c = self.current();
            let f = self.shown(c);
            if !e.send(stamp, f) {
                self.pace.unsent();
            }
        }
        Flush::from_due(due)
    }

    fn capture(&mut self) -> Option<(StampArgs, FrameData)> {
        let end = self.end?;
        let c = self.current();
        Some((self.stamp(end), c))
    }
}

#[cfg(test)]
mod tests;
