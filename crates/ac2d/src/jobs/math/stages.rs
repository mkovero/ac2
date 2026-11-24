//! A transfer math over live transfers at the `bins` resolution digs down as they do: its
//! `tf` states their common layout, and each analysis stage's bins (`bins/<math>/<stage>`)
//! are the operands' own bins of that stage combined per bin, by the expression and delay
//! reference the grid result uses.
//!
//! Only operands with one layout have bins at the same frequencies, so every operand must
//! be live, included and at `bins` with an equal layout; anything else (a stored trace, a
//! grid resolution, another sample rate or LF window, an operand left out) keeps the math on
//! the grid. An alignment is viewed as its own scene and level math has no bins.

use std::time::{Duration, Instant};

use ac2_proto::frame::{
    BinLayout, BinsFrame, BinsMeta, FrameData, OperandState, OperandStatus, TfFrame,
};
use ac2_proto::grid::GridDef;
use ac2_proto::model::{MathConfig, MathDomain, MathExpr};
use ac2_proto::topic::Topic;
use ac2_proto::units::{MeasId, SessionEpoch};
use ac2_traces::columns::Columns;
use ac2_traces::math::{self, Combine, Input};

use super::Answer;
use super::combine::{delay_reference, intersection};
use crate::jobs::{Due, Emitter, Flush, Pace, StampArgs};

/// The layout every operand's `tf` states, when the math is eligible for stage bins.
pub(super) fn layout(
    cfg: &MathConfig,
    answers: &[Answer],
    states: &[OperandState],
) -> Option<Box<BinLayout>> {
    if cfg.domain != MathDomain::Transfer || matches!(cfg.expr, MathExpr::Align { .. }) {
        return None;
    }
    if states
        .iter()
        .any(|s| !matches!(s.status, OperandStatus::Included))
    {
        return None;
    }
    let mut common: Option<&BinLayout> = None;
    for a in answers {
        let Answer::Result(f) = a else {
            return None;
        };
        let FrameData::Tf(t) = &f.frame.data else {
            return None;
        };
        let l = t.meta.bins.as_deref()?;
        if common.is_some_and(|c| c != l) {
            return None;
        }
        common = Some(l);
    }
    common.map(|l| Box::new(l.clone()))
}

/// Stage frames `wanted` of the math result `tf`, from the operands' `answers` and their
/// stage bins `bins` (`bins[k]` operand `k`'s, from the same averages as its answer). None
/// unless `tf` states a layout and every operand sent that many stages: one that keeps no
/// stage bins makes the result ineligible.
pub(super) fn frames(
    cfg: &MathConfig,
    tf: &TfFrame,
    answers: &[Answer],
    bins: &[Option<Vec<BinsFrame>>],
    seen_delay: &[Option<f64>],
    epoch: SessionEpoch,
    wanted: &[usize],
) -> Vec<BinsFrame> {
    let Some(l) = tf.meta.bins.as_deref() else {
        return Vec::new();
    };
    let how = match &cfg.expr {
        MathExpr::Binary { op, .. } => Combine::Binary(*op),
        MathExpr::Average { method, .. } => Combine::Average(*method),
        MathExpr::Align { .. } => return Vec::new(),
    };
    let delays: Option<Vec<f64>> = answers
        .iter()
        .map(|a| match a {
            Answer::Result(f) => match &f.frame.data {
                FrameData::Tf(t) => Some(t.meta.delay.0),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let Some(delays) = delays else {
        return Vec::new();
    };
    let Some(bins) = bins
        .iter()
        .map(|b| b.as_deref().filter(|b| b.len() == l.stages.len()))
        .collect::<Option<Vec<&[BinsFrame]>>>()
    else {
        return Vec::new();
    };
    if bins.len() != delays.len() {
        return Vec::new();
    }
    // Every operand is included: the grid result's reference, with all of them going in.
    let all: Vec<usize> = (0..answers.len()).collect();
    let reference = delay_reference(cfg, &all, seen_delay);
    wanted
        .iter()
        .filter(|&&s| s < l.stages.len())
        .map(|&s| {
            let ops: Vec<(f64, &BinsFrame)> =
                delays.iter().zip(&bins).map(|(d, b)| (*d, &b[s])).collect();
            stage_frame(how, &ops, tf, reference, epoch)
        })
        .collect()
}

/// One stage's frame: the operands' bins `ops` (each with its inserted delay) combined at
/// each bin's frequency.
fn stage_frame(
    how: Combine,
    ops: &[(f64, &BinsFrame)],
    tf: &TfFrame,
    reference: ac2_core::average::DelayReference,
    epoch: SessionEpoch,
) -> BinsFrame {
    let m0 = ops[0].1.meta.clone();
    let n = ops[0].1.mag.len();
    let blocks = ops.iter().map(|(_, f)| f.meta.blocks).min().unwrap_or(0);
    let freqs: Vec<f64> = (0..n)
        .map(|i| (m0.first_bin as usize + i) as f64 * m0.bin_hz.0)
        .collect();
    let cols: Vec<Columns> = ops
        .iter()
        .map(|(_, f)| Columns {
            mag_db: f.mag.clone(),
            phase_deg: Some(f.phase.clone()),
            coherence: Some(f.coh.clone()),
        })
        .collect();
    let inputs: Vec<Input<'_>> = cols
        .iter()
        .zip(ops)
        .map(|(c, (d, _))| Input {
            columns: c,
            delay: *d,
            time_base: Some(epoch),
        })
        .collect();
    // Labels an average's result only; the combination itself is per frequency.
    let grid = GridDef::Linear {
        fs: m0.rate,
        n: (m0.rate.0 / m0.bin_hz.0).round() as u32,
    };
    let nan = || vec![f32::NAN; n];
    let result = (blocks > 0 && ops.iter().all(|(_, f)| f.mag.len() == n))
        .then(|| math::transfer(how, &inputs, &grid, &freqs, reference).ok())
        .flatten();
    let (mag, phase, coh) = match result {
        Some(r) => (
            r.columns.mag_db,
            r.columns.phase_deg.unwrap_or_else(nan),
            r.columns.coherence.unwrap_or_else(nan),
        ),
        None => (nan(), nan(), nan()),
    };
    // As the math `tf`'s: where every operand's curve corrects.
    let mic_range = intersection(ops.iter().map(|(_, f)| f.meta.mic_range));
    BinsFrame {
        meas: tf.meas,
        meta: BinsMeta {
            hop: ops.iter().map(|(_, f)| f.meta.hop).max().unwrap_or(m0.hop),
            // The freshness rule compares these with the math's own `tf`.
            delay: tf.meta.delay,
            nudged: tf.meta.nudged,
            mic_curve: tf.meta.mic_curve,
            mic_range,
            blocks,
            eff_avg: if blocks > 0 {
                ops.iter()
                    .map(|(_, f)| f.meta.eff_avg)
                    .fold(f32::INFINITY, f32::min)
            } else {
                f32::NAN
            },
            ..m0
        },
        mag,
        phase,
        coh,
    }
}

/// The paces of a math's stage streams, for the layout its newest result stated.
pub(super) struct Streams {
    meas: MeasId,
    layout: Option<BinLayout>,
    paces: Vec<Pace>,
}

impl Streams {
    pub(super) fn new(meas: MeasId) -> Self {
        Self {
            meas,
            layout: None,
            paces: Vec::new(),
        }
    }

    /// Takes the layout a new result states: another one (or none) starts every stage
    /// afresh, so a subscriber gets the new stages on the next emit.
    pub(super) fn follow(&mut self, l: Option<&BinLayout>) {
        if self.layout.as_ref() != l {
            self.layout = l.cloned();
            self.paces = (0..l.map_or(0, |l| l.stages.len()))
                .map(|_| Pace::new(Duration::ZERO))
                .collect();
        }
    }

    /// The stages due now, subscribed ones only, and when to come back for the rest.
    pub(super) fn due(
        &mut self,
        e: &Emitter,
        generation: u64,
        stamp: &StampArgs,
        now: Instant,
    ) -> (Vec<usize>, Flush) {
        let mut flush = Flush::Done;
        let mut wanted = Vec::new();
        for (s, p) in self.paces.iter_mut().enumerate() {
            let topic = Topic::Bins {
                meas: self.meas,
                stage: s as u8,
            };
            let due = p.due(e, topic, generation, stamp, now);
            if due == Due::Send {
                wanted.push(s);
            }
            flush = flush.and(Flush::from_due(due));
        }
        (wanted, flush)
    }

    /// Stage `stage`'s frame taken as sent did not go out.
    pub(super) fn unsent(&mut self, stage: u8) {
        if let Some(p) = self.paces.get_mut(usize::from(stage)) {
            p.unsent();
        }
    }
}
