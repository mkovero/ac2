//! A transfer's analysis-stage bins (`bins/<meas>/<stage>`): one stage's own bins, where that
//! stage alone serves the curve, beside the grid stream `d/<meas>/tf` of the same
//! measurement. Element `i` is bin `first_bin + i`, at `(first_bin + i) · bin_hz`.

use serde::{Deserialize, Serialize};

use crate::model::FreqRange;
use crate::units::{Hz, MeasId, Samples, Seconds};

/// The analysis stages of a transfer at the `bins` resolution, in its `tf` frames: what each
/// stage's bins frame carries and where the grid is drawn instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinLayout {
    /// Per stage, shallowest (full rate) first; the index is the topic's stage.
    pub stages: Vec<BinStage>,
    /// Per hand-over between stage `b` and `b + 1`, the band where both stages' estimates
    /// are blended: no stage's own bins show it, so the grid is drawn there.
    pub blends: Vec<FreqRange>,
}

/// The bins one stage's frame carries.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinStage {
    /// First bin.
    pub first_bin: u32,
    /// Bins, the frame's `n`.
    pub bins: u32,
    /// Bin spacing.
    pub bin_hz: Hz,
}

/// Metadata of a stage's bins frame. Every field but `blocks` and `eff_avg` is static: a
/// change of any of them goes as a keyframe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinsMeta {
    /// Stage, 0 = full rate; the topic's.
    pub stage: u8,
    /// Decimation factor from the full rate.
    pub factor: u32,
    /// Stage sample rate.
    pub sample_rate_hz: Hz,
    /// Bin spacing.
    pub bin_hz: Hz,
    /// Bin of element 0.
    pub first_bin: u32,
    /// Hop between the stage's blocks in full-rate samples: a stage frame may lag the `tf`
    /// of the same stamp's newest block by up to this much plus the publish refresh.
    pub hop: Samples,
    /// Delay applied to the reference, as in the `tf` of the same emit.
    pub delay: Seconds,
    /// The part of `delay` the operator's nudges added, as in that `tf`.
    pub nudged: Seconds,
    /// A mic curve was subtracted from `mag`, at each bin's frequency.
    pub mic_curve: bool,
    /// The lowest and highest point of that mic curve (held at its end value outside them);
    /// `None` without a curve.
    pub mic_range: Option<FreqRange>,
    /// Blocks in the stage's average; 0: no average to show (the stage started over, or the
    /// job is stopped), every element NaN.
    pub blocks: u64,
    /// Model effective averages per bin of the stage (as `tf` columns state theirs); NaN
    /// with `blocks` 0.
    pub eff_avg: f32,
}

/// One stage's bins.
#[derive(Debug, Clone, PartialEq)]
pub struct BinsFrame {
    /// Measurement.
    pub meas: MeasId,
    /// Metadata.
    pub meta: BinsMeta,
    /// 20·log10|H1| per bin, dB, the mic curve taken off; NaN where the reference has no
    /// energy.
    pub mag: Vec<f32>,
    /// arg H1 per bin, degrees, the delay applied.
    pub phase: Vec<f32>,
    /// γ² per bin.
    pub coh: Vec<f32>,
}
