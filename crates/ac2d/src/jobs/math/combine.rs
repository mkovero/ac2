//! The math result from its operands' answers on the combining grid: which operands go in
//! and why the others do not, the delay a phase-combining result is referred to, and the
//! frame that states it all.

use ac2_core::average::DelayReference as CoreReference;
use ac2_proto::frame::{
    Frame, FrameData, MathState, OperandState, OperandStatus, ProtectionFlags, RtaFrame, RtaMeta,
    SpecFrame, SpecMeta, TfFrame, TfMeta, ValidityMask,
};
use ac2_proto::grid::{GridDef, GridId};
use ac2_proto::model::{
    AlignCurves, AlignNoFit, AlignOutcome, AlignState, AverageMethod, BandFraction, BinRange,
    CalStatus, FreqRange, LevelScale, MathConfig, MathDomain, MathExpr, MathOp, MathReference,
    PhaseBasis, SpecLevel, Weighting, Window,
};
use ac2_proto::units::{Hz, MeasId, Seconds, SessionEpoch};
use ac2_traces::columns::Columns;
use ac2_traces::math::{self, Combine, Input};
use ac2_traces::ops::capture_columns;

use super::{Answer, stages};

/// Why a live operand's answer cannot go in, if it cannot.
pub(super) fn usable(
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
    level: Option<SpecLevel>,
    mic_curve: bool,
    /// Where a live transfer's mic curve has points (`TfMeta::mic_range`); `None` without
    /// one, and for a stored operand, which keeps no range.
    mic_range: Option<FreqRange>,
    /// How full a live transfer's averages are (`TfMeta::filled`); 1 for a stored operand,
    /// whose average is complete.
    filled: f32,
    /// Where a live transfer's averages are still filling (`TfMeta::settling`).
    settling: Option<FreqRange>,
    /// Where a live transfer's columns are finer than its bins (`TfMeta::unresolved`);
    /// `None` for a stored operand, which keeps no ranges.
    unresolved: Option<Vec<BinRange>>,
    frame: Option<FrameData>,
}

/// The unresolved ranges of a result combining operands with `each`'s: every piece covered
/// by any operand, at the coarsest (largest) bin width among the operands covering it,
/// since a column can only be as fine as the coarsest estimate that went into it.
/// Neighbouring pieces with one width stay one range.
pub(super) fn coarsest_bins(each: &[&[BinRange]]) -> Vec<BinRange> {
    let mut edges: Vec<f64> = each
        .iter()
        .flat_map(|r| r.iter().flat_map(|b| [b.lo.0, b.hi.0]))
        .collect();
    edges.sort_by(f64::total_cmp);
    edges.dedup();
    let mut out: Vec<BinRange> = Vec::new();
    for w in edges.windows(2) {
        let mid = 0.5 * (w[0] + w[1]);
        let Some(bin) = each
            .iter()
            .flat_map(|r| r.iter())
            .filter(|b| b.lo.0 <= mid && mid <= b.hi.0)
            .map(|b| b.bin_hz.0)
            .reduce(f64::max)
        else {
            continue;
        };
        match out.last_mut() {
            Some(last) if last.hi.0 == w[0] && last.bin_hz.0 == bin => last.hi = Hz(w[1]),
            _ => out.push(BinRange {
                lo: Hz(w[0]),
                hi: Hz(w[1]),
                bin_hz: Hz(bin),
            }),
        }
    }
    out
}

fn value(a: &Answer, f: Option<&Frame>, epoch: SessionEpoch) -> Option<Value> {
    if let Answer::Stored(h) = a {
        return Some(Value {
            columns: h.columns.clone(),
            delay: h.delay,
            time_base: h.time_base,
            validity: None,
            scale: h.scale,
            level: h.level,
            mic_curve: h.mic_curve,
            mic_range: None,
            filled: 1.0,
            settling: None,
            unresolved: None,
            frame: None,
        });
    }
    let f = f?;
    let (_, columns) = capture_columns(&f.data)?;
    let mut filled = 1.0;
    let mut settling = None;
    let mut unresolved = None;
    let mut mic_range = None;
    let (delay, validity, scale, level, mic_curve) = match &f.data {
        FrameData::Tf(t) => {
            filled = t.meta.filled;
            settling = t.meta.settling;
            unresolved.clone_from(&t.meta.unresolved);
            mic_range = t.meta.mic_range;
            (
                t.meta.delay.0,
                Some(t.validity.clone()),
                None,
                None,
                t.meta.mic_curve,
            )
        }
        FrameData::Spec(s) => (
            0.0,
            None,
            Some(s.meta.scale),
            Some(s.meta.level),
            s.meta.mic_curve,
        ),
        FrameData::Rta(r) => (
            0.0,
            Some(r.validity.clone()),
            Some(r.meta.scale),
            None,
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
        level,
        mic_curve,
        mic_range,
        filled,
        settling,
        unresolved,
        frame: Some(f.data.clone()),
    })
}

/// The math trace puts a level through a filter: a spectrum or RTA `A × B` or `A ÷ B`,
/// `B` a stored transfer function (`ac2_traces::math::filtered`).
pub(crate) fn is_filter(c: &MathConfig) -> bool {
    c.domain != MathDomain::Transfer
        && matches!(
            c.expr,
            MathExpr::Binary {
                op: MathOp::Divide | MathOp::Multiply,
                ..
            }
        )
}

/// Whether the expression combines phase across operands, so every operand must be in one
/// time base: a transfer sum or difference, or a complex or coherence-weighted average.
pub(crate) fn needs_shared_time_base(c: &MathConfig) -> bool {
    c.domain == MathDomain::Transfer
        && match &c.expr {
            MathExpr::Binary { op, .. } => matches!(op, MathOp::Add | MathOp::Subtract),
            MathExpr::Average { method, .. } => *method != AverageMethod::Power,
            MathExpr::Align { .. } => true,
        }
}

/// The delay a phase-combining transfer math refers its result to, with the operands at
/// expression positions `included` going in, in order: the named operand's delay (its
/// newest, while it is left out), else an explicit delay. An operand never seen falls back to
/// the first included operand's, and the frame states whichever delay was used.
pub(super) fn delay_reference(
    cfg: &MathConfig,
    included: &[usize],
    seen_delay: &[Option<f64>],
) -> CoreReference {
    match cfg.reference {
        MathReference::Fixed { delay } => CoreReference::Fixed(delay.0),
        MathReference::Operand { operand: r } => {
            match cfg.expr.operands().iter().position(|o| *o == r) {
                Some(k) => match included.iter().position(|i| *i == k) {
                    Some(j) => CoreReference::Trace(j),
                    None => match seen_delay.get(k).copied().flatten() {
                        Some(d) => CoreReference::Fixed(d),
                        None => CoreReference::Trace(0),
                    },
                },
                None => CoreReference::Trace(0),
            }
        }
    }
}

/// The math trace's result from its operands' answers (`answers[k]` is the expression's
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
    // A level through a filter: operand 1 is the filter's gain, which has no level scale.
    let filter = is_filter(cfg);
    let mut states = Vec::with_capacity(operands.len());
    let mut included: Vec<(usize, Value)> = Vec::new();
    for (k, (o, a)) in operands.iter().zip(answers).enumerate() {
        let frame = match a {
            Answer::Stored(_) => Ok(None),
            _ => usable(a, cfg.domain, epoch, grid_id).map(Some),
        };
        let status = match frame.map(|f| value(a, f, epoch)) {
            Ok(Some(v)) => {
                // Levels combine only in one scale, spectra only in one level (a tone level
                // and a density are different quantities); phase-combining transfer math
                // only in one time base (the first included operand's).
                let first = included.first().map(|(_, f)| f);
                let is_filter = filter && k == 1;
                let mismatch = first.is_some_and(|f| {
                    ((f.scale != v.scale || f.level != v.level) && !is_filter)
                        || (shared_needed && f.time_base != v.time_base)
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
        MathExpr::Binary { .. } | MathExpr::Align { .. } => included.len() == 2,
        MathExpr::Average { .. } => included.len() >= MathConfig::MIN_AVERAGE,
    };
    let included_at: Vec<usize> = included.iter().map(|(k, _)| *k).collect();
    let reference = delay_reference(cfg, &included_at, seen_delay);
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
        MathExpr::Binary { op, .. } => Some(Combine::Binary(*op)),
        MathExpr::Average { method, .. } => Some(Combine::Average(*method)),
        MathExpr::Align { .. } => None,
    };
    let inputs: Vec<Input<'_>> = included
        .iter()
        .map(|(_, v)| Input {
            columns: &v.columns,
            delay: v.delay,
            time_base: v.time_base,
        })
        .collect();
    let mut align = None;
    let mut curves = None;
    let result = if !enough {
        None
    } else if let (
        MathExpr::Align {
            band, max_delay, ..
        },
        [a, b],
    ) = (&cfg.expr, &inputs[..])
    {
        math::align(a, b, freqs, *band, *max_delay)
            .map(|(r, st, cv)| {
                align = Some(Box::new(st));
                curves = Some(cv);
                r
            })
            .ok()
    } else if let Some(combine_how) = combine_how {
        match cfg.domain {
            MathDomain::Transfer => math::transfer(combine_how, &inputs, grid, freqs, reference),
            MathDomain::Spectrum | MathDomain::Rta => match (combine_how, &inputs[..]) {
                (Combine::Binary(op), [level, gain]) if filter => math::filtered(
                    math::Filter::of(op).unwrap_or(math::Filter::Through),
                    level.columns,
                    &gain.columns.mag_db,
                ),
                _ => math::levels(combine_how, &inputs),
            },
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
    // An average of transfers also says how far its positions spread, over the columns
    // where the average itself has a value.
    let (range, mut range_band) = match (&cfg.expr, cfg.domain) {
        (MathExpr::Average { .. }, MathDomain::Transfer) => {
            let (spread, band) = math::range_band(&inputs);
            (Some(spread), band)
        }
        _ => (None, None),
    };
    if let Some(b) = range_band.as_mut() {
        for (i, v) in validity.iter().enumerate() {
            if *v != ValidityMask::NONE {
                b.lo[i] = f32::NAN;
                b.hi[i] = f32::NAN;
            }
        }
    }
    let phase = result.as_ref().map_or(PhaseBasis::NoPhase, |r| r.phase);
    // An alignment without both operands still says so, with its view's curves empty.
    if align.is_none() && matches!(cfg.expr, MathExpr::Align { .. }) {
        align = Some(Box::new(AlignState {
            outcome: AlignOutcome::NoFit {
                why: AlignNoFit::FewOperands,
            },
        }));
        curves = Some(AlignCurves {
            phase_before: vec![f32::NAN; n],
            phase_after: vec![f32::NAN; n],
            sum_before: vec![f32::NAN; n],
            sum_after: vec![f32::NAN; n],
        });
    }
    let bins = stages::layout(cfg, answers, &states);
    let state = Some(Box::new(MathState {
        operands: states,
        phase,
        align,
        range,
    }));
    let nan = || vec![f32::NAN; n];
    // A filter is no measurement: the result's mic curve is its level's.
    let mic_curve = !included.is_empty()
        && included
            .iter()
            .filter(|(k, _)| !(filter && *k == 1))
            .all(|(_, v)| v.mic_curve);
    // The result is as settled as its least settled operand; with none included nothing
    // has been averaged.
    let filled = if included.is_empty() {
        0.0
    } else {
        included.iter().map(|(_, v)| v.filled).fold(1.0, f32::min)
    };
    // Still filling wherever any included operand is.
    let settling = included
        .iter()
        .filter_map(|(_, v)| v.settling)
        .reduce(|a, b| FreqRange {
            lo: Hz(a.lo.0.min(b.lo.0)),
            hi: Hz(a.hi.0.max(b.hi.0)),
        });
    // Known only when every included operand states its ranges.
    let unresolved = included
        .iter()
        .map(|(_, v)| v.unresolved.as_deref())
        .collect::<Option<Vec<&[BinRange]>>>()
        .filter(|each| !each.is_empty())
        .map(|each| coarsest_bins(&each));
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
                mic_range: intersection(included.iter().map(|(_, v)| v.mic_range)),
                math: state,
                unresolved,
                bins,
                settling,
                filled,
            },
            mag,
            phase: phase_deg.unwrap_or_else(nan),
            coh: coh.unwrap_or_else(nan),
            validity,
            align: curves,
            range: range_band,
        }),
        MathDomain::Spectrum => {
            let (window, cal) = match first_frame {
                Some(FrameData::Spec(s)) => (s.meta.window, s.meta.cal),
                // A stored spectrum keeps no window; its level is as its kind says.
                _ => (Window::Hann, CalStatus::Uncalibrated),
            };
            FrameData::Spec(SpecFrame {
                meas,
                meta: SpecMeta {
                    mic_range: None,
                    window,
                    scale: level_scale(&included),
                    level: included
                        .first()
                        .and_then(|(_, v)| v.level)
                        .unwrap_or(SpecLevel::Tone),
                    cal,
                    mic_curve,
                    smoothing: None,
                    math: state,
                    bands: None,
                    harmonics: None,
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
                    mic_range: None,
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

/// Where every one of `ranges` corrects: their intersection; `None` if any is `None`, if
/// they do not overlap, or with none at all.
pub(super) fn intersection(
    ranges: impl IntoIterator<Item = Option<FreqRange>>,
) -> Option<FreqRange> {
    ranges
        .into_iter()
        .reduce(|a, b| match (a, b) {
            (Some(a), Some(b)) if a.lo.0.max(b.lo.0) <= a.hi.0.min(b.hi.0) => Some(FreqRange {
                lo: Hz(a.lo.0.max(b.lo.0)),
                hi: Hz(a.hi.0.min(b.hi.0)),
            }),
            _ => None,
        })
        .flatten()
}
