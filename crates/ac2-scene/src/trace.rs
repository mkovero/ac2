//! Transfer-function traces and the display math applied to them.
//!
//! The scene does no DSP: magnitude, phase and coherence arrive averaged and smoothed. What
//! it does compute is display math, all of it here and tested:
//!
//! - **Offset and polarity.** `mag + offset_db`; inverted polarity adds 180° to phase.
//! - **Validity.** A column with a non-zero validity mask or a non-finite value is a gap
//!   (NaN), so the line breaks instead of bridging over missing data.
//! - **Coherence.** Optional blanking (γ² below a threshold → gap) and an opacity curve
//!   (low coherence fades). Coherence itself is never blanked or faded.
//! - **Phase comparison time reference (decision 8b).** See below.
//! - **Unwrapped phase and group delay** for the alternative phase views. Group delay is
//!   the slope of a line fitted to the unwrapped phase over a stated span, not a
//!   neighbour difference: on a 48-per-octave grid neighbours are 2.9 % of f apart, so a
//!   phase error of 0.03° at 50 Hz is already 58 µs of a 187 µs group delay. The fit
//!   weights each column by `γ²/(1−γ²)`, the inverse of an averaged phase estimate's
//!   variance (columns without coherence weigh 1), and the span is
//!   [`group_delay_span`]; the slope's error falls as `n^{−3/2}` with the columns fitted.
//!
//! # Phase relative to the shared reference delay
//!
//! A live TF is measured with the reference delayed by the trace's delay `τ_k`, so for a
//! path whose true response is `H(f)` the trace holds `H_k(f) = H(f) · e^{+jωτ_k}`
//! (`ω = 2πf`): its own propagation delay is compensated, and two traces of identical
//! boxes at different distances would look identical. To keep relative arrival visible,
//! every trace in the shared time base of the session epoch is drawn as if it had been
//! measured with the *reference trace's* delay `τ_ref` (plus its own nudge `ν_k`,
//! decision 8a):
//!
//! ```text
//! H_disp,k(f) = H_k(f) · e^{-jω(τ_k − τ_ref − ν_k)}
//! φ_disp,k(f) = φ_k(f) − 360° · f · Δ_k,     Δ_k = τ_k − τ_ref − ν_k   [s]
//! ```
//!
//! For the reference trace itself `Δ = −ν` (normally 0). A trace that arrives 1 ms later
//! than the reference therefore shows an extra phase lag of 360° per kHz, and its group
//! delay reads 1 ms higher. Traces without a shared time base (imported, averaged, math,
//! another session epoch — decision 8a "marked independent") keep their own alignment:
//! `Δ = −ν`. The rotation changes phase only; magnitude and coherence are untouched.
//!
//! # A measurement's own delay steps
//!
//! The time base of a live curve is its *arrival* `τ_k = D_k − n_k`: the applied delay `D_k`
//! less what the operator's delay steps (`delay.nudge`: `,` `.` and Ctrl / Alt) added to it,
//! `n_k`. Its columns hold `H(f) · e^{+jωD_k}`, so it is drawn as
//! `H(f) · e^{+jω(τ_ref + n_k + ν_k)}`: a step of the measurement's delay moves its own
//! curve by `e^{+jωΔ}` — the direction a `.` display nudge of the same size moves a trace —
//! and, as `τ_ref` is an arrival too, moves no other curve even when it is the reference.
//! Referred to `D_k` instead, the step would cancel on every curve but the reference, and on
//! the reference it would drag every other curve along: the whole picture would turn
//! together and the step would look like nothing. A capture records `D_k` with `n_k` as its
//! display nudge, so it is drawn exactly where the live curve was.

use ac2_proto::frame::{FrameStamp, TfFrame, ValidityMask};
use ac2_proto::model::{
    ImportNote, MicState, Polarity, Smoothing, SmoothingMode, TraceData, TraceMicCurve, TraceSource,
};
use ac2_proto::units::{MeasId, Seconds, SessionEpoch, TraceId};
use std::sync::{Arc, Mutex};

use crate::primitives::Color;
use crate::time::Freshness;
use crate::view::CoherenceStyle;

/// Identity of a displayed trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TraceKey {
    /// A running measurement.
    Live(MeasId),
    /// A stored trace.
    Stored(TraceId),
}

/// Time base of a trace's phase.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TimeBase {
    /// Phase measured with `delay` in the shared time reference of session `epoch`.
    Shared { epoch: SessionEpoch, delay: Seconds },
    /// Own alignment only; never compared against the shared reference.
    Independent,
}

/// One TF trace as input to the scene: borrowed columns plus display edits.
#[derive(Clone, Debug)]
pub struct TfTrace<'a> {
    pub key: TraceKey,
    pub name: String,
    pub color: Color,
    /// Column frequencies (from the frame's grid, [`crate::grid::column_frequencies`]).
    pub freqs: &'a [f64],
    pub mag_db: &'a [f32],
    pub phase_deg: Option<&'a [f32]>,
    pub coherence: Option<&'a [f32]>,
    pub validity: Option<&'a [ValidityMask]>,
    pub offset_db: f64,
    pub polarity: Polarity,
    /// Per-trace delay nudge on top of the measured delay (decision 8a).
    pub nudge: Seconds,
    /// Live curves: what the measurement's delay steps added to its arrival (`n_k` of the
    /// module docs). Already in the columns and kept out of the time base, so it draws as a
    /// nudge; the legend's nudge tag includes it.
    pub delay_nudge: Seconds,
    pub time_base: TimeBase,
    /// Live traces: age of the newest frame. Stored traces: `None`.
    pub freshness: Option<Freshness>,
    /// Display smoothing the columns arrived with (the daemon applied it).
    pub smoothing: Option<Smoothing>,
    /// What the legend says after the name about where the curve comes from: a spatial
    /// average's `3 of 4 positions · power avg`.
    pub note: Option<String>,
    /// The stored trace the columns are borrowed from: they are then fixed for as long as
    /// it lives, so its display math can be kept between frames ([`DisplayCache`]).
    pub stored: Option<&'a Arc<TraceData>>,
    /// The selected stored trace: the trace keys act on it, so the plot and the legend
    /// mark it.
    pub selected: bool,
    /// Drawn for compare only (another measurement's or an imported curve, not the pane's
    /// group): the legend tags it [`COMPARED`].
    pub compared: bool,
    /// Ranges of the grid finer than the analysis behind the columns (the valid-resolution
    /// marker, [`crate::unresolved`]); `None`: not known (math channels, stored transfer
    /// curves).
    pub unresolved: Option<crate::unresolved::Source<'a>>,
}

/// The legend tag of a compared curve, and the measurement tree's mark of a compared row.
pub const COMPARED: &str = "cmp";

impl<'a> TfTrace<'a> {
    /// A live trace from a TF frame of measurement `frame.meas`.
    pub fn live(
        frame: &'a TfFrame,
        stamp: &FrameStamp,
        freqs: &'a [f64],
        name: impl Into<String>,
        color: Color,
        freshness: Freshness,
    ) -> Self {
        Self {
            key: TraceKey::Live(frame.meas),
            name: name.into(),
            color,
            freqs,
            mag_db: &frame.mag,
            phase_deg: Some(&frame.phase),
            coherence: Some(&frame.coh),
            validity: Some(&frame.validity),
            offset_db: 0.0,
            polarity: Polarity::Normal,
            nudge: Seconds(0.0),
            delay_nudge: frame.meta.nudged,
            // The arrival: see "A measurement's own delay steps" in the module docs.
            time_base: TimeBase::Shared {
                epoch: stamp.session_epoch,
                delay: Seconds(frame.meta.delay.0 - frame.meta.nudged.0),
            },
            freshness: Some(freshness),
            smoothing: frame.meta.smoothing,
            note: None,
            stored: None,
            selected: false,
            compared: false,
            unresolved: frame
                .meta
                .unresolved
                .as_ref()
                .map(|u| crate::unresolved::Source {
                    unresolved: u,
                    fill: crate::unresolved::Fill::Gaps,
                }),
        }
    }

    /// A stored trace; colour, offset, polarity and nudge come from its edit record.
    /// Captured traces keep their epoch's shared time base; every other source is
    /// independent (decision 8a).
    pub fn stored(data: &'a Arc<TraceData>, freqs: &'a [f64], color: Color) -> Self {
        let m = &data.meta;
        let time_base = match m.source.shared_epoch() {
            Some(epoch) => TimeBase::Shared {
                epoch,
                delay: m.delay,
            },
            None => TimeBase::Independent,
        };
        let note = match &m.source {
            TraceSource::Math {
                expr,
                operands,
                phase,
                ..
            } => Some(crate::math::capture_note(
                expr,
                operands,
                *phase,
                m.kind == ac2_proto::model::TraceKind::Transfer,
            )),
            _ => None,
        };
        Self {
            key: TraceKey::Stored(m.id),
            name: m.edit.name.clone(),
            color,
            freqs,
            mag_db: &data.mag_db,
            phase_deg: data.phase_deg.as_deref(),
            coherence: data.coherence.as_deref(),
            validity: None,
            offset_db: m.edit.offset.0,
            polarity: m.edit.polarity,
            nudge: m.edit.delay_nudge,
            delay_nudge: Seconds(0.0),
            time_base,
            freshness: None,
            smoothing: m.edit.smoothing,
            note,
            stored: Some(data),
            selected: false,
            compared: false,
            unresolved: data
                .sweep
                .as_ref()
                .and_then(|s| s.unresolved.as_ref())
                .map(|u| &u.response)
                .or(match &m.source {
                    ac2_proto::model::TraceSource::Imported {
                        ir_file: Some(f), ..
                    } => Some(&f.unresolved),
                    _ => None,
                })
                .map(|u| crate::unresolved::Source {
                    unresolved: u,
                    fill: crate::unresolved::Fill::Interpolated,
                }),
        }
    }
}

/// The phase reference in force for one scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhaseReference {
    pub key: TraceKey,
    pub epoch: SessionEpoch,
    pub delay: Seconds,
}

/// The reference per decision 8b: the wanted trace if it has a shared time base, else the
/// first trace that does. `None` when no trace has one.
pub fn resolve_reference(
    traces: &[TfTrace<'_>],
    wanted: Option<TraceKey>,
) -> Option<PhaseReference> {
    let shared = |t: &TfTrace<'_>| match t.time_base {
        TimeBase::Shared { epoch, delay } => Some(PhaseReference {
            key: t.key,
            epoch,
            delay,
        }),
        TimeBase::Independent => None,
    };
    wanted
        .and_then(|k| traces.iter().find(|t| t.key == k).and_then(shared))
        .or_else(|| traces.iter().find_map(shared))
}

/// How a trace's phase relates to the reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseRelation {
    /// It is the reference.
    Reference,
    /// Drawn relative to the reference delay.
    Relative,
    /// Own alignment (marked "indep." in the legend).
    Independent,
}

/// `Δ_k` of the module docs, seconds, and the relation it encodes.
pub fn phase_shift(t: &TfTrace<'_>, reference: Option<&PhaseReference>) -> (f64, PhaseRelation) {
    let nudge = t.nudge.0;
    match (t.time_base, reference) {
        (TimeBase::Shared { epoch, delay }, Some(r)) if epoch == r.epoch => {
            let rel = if r.key == t.key {
                PhaseRelation::Reference
            } else {
                PhaseRelation::Relative
            };
            (delay.0 - r.delay.0 - nudge, rel)
        }
        _ => (-nudge, PhaseRelation::Independent),
    }
}

/// Wraps degrees into (−180, 180].
pub fn wrap_deg(d: f64) -> f64 {
    let w = (d + 180.0).rem_euclid(360.0) - 180.0;
    if w == -180.0 { 180.0 } else { w }
}

/// Opacity for coherence `g2` under `style` (1 when coherence is unknown).
pub fn coherence_alpha(g2: f64, style: &CoherenceStyle) -> f32 {
    if !style.alpha || !g2.is_finite() {
        return 1.0;
    }
    let lo = f64::from(style.blank_below.unwrap_or(0.0));
    let hi = f64::from(style.alpha_full_at);
    let t = if hi > lo {
        ((g2 - lo) / (hi - lo)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let floor = f64::from(style.alpha_floor);
    (floor + (1.0 - floor) * t) as f32
}

/// A trace after display math; column-aligned with `freqs`. NaN = not drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayTrace {
    pub key: TraceKey,
    pub name: String,
    pub color: Color,
    pub relation: PhaseRelation,
    /// `Δ_k` applied to phase, seconds.
    pub shift_s: f64,
    pub offset_db: f64,
    pub inverted: bool,
    pub freqs: Vec<f64>,
    /// Magnitude + offset, dB.
    pub magnitude_db: Vec<f64>,
    /// Rotated phase, wrapped to (−180, 180].
    pub phase_wrapped_deg: Vec<f64>,
    /// Rotated phase, unwrapped along frequency.
    pub phase_unwrapped_deg: Vec<f64>,
    /// Group delay of the rotated phase over [`group_delay_span`], seconds.
    pub group_delay_s: Vec<f64>,
    /// γ² (not blanked by the coherence threshold).
    pub coherence: Vec<f64>,
    /// Per-column opacity from coherence.
    pub alpha: Vec<f32>,
    pub freshness: Option<Freshness>,
    pub smoothing: Option<Smoothing>,
    /// See [`TfTrace::note`].
    pub note: Option<String>,
}

impl DisplayTrace {
    pub fn is_stale(&self) -> bool {
        self.freshness.is_some_and(|f| f.is_stale())
    }
}

/// Unsmoothed phase gets its group delay over 1/12 octave (±1/24): about five columns of a
/// 48-per-octave grid, 2× less noise than the neighbour difference, while a resonance a
/// twelfth of an octave wide still shows.
pub const GROUP_DELAY_SPAN_B: u32 = 12;

/// The span group delay is fitted over, as `b` of 1/b octave (centred on the column): the
/// trace's phase smoothing when that is wider than [`GROUP_DELAY_SPAN_B`], since a slope
/// finer than the smoothing would only show its averaging. Magnitude-only smoothing leaves
/// the phase as measured.
pub fn group_delay_span(smoothing: Option<Smoothing>) -> u32 {
    match smoothing {
        Some(s) if s.mode == SmoothingMode::MagnitudePhase => {
            s.fraction.b().min(GROUP_DELAY_SPAN_B)
        }
        _ => GROUP_DELAY_SPAN_B,
    }
}

/// `−dφ/df / 360` of `unwrapped` (degrees) at each column: the slope of a weighted
/// least-squares line over the columns within ±1/(2b) octave, and at least the two
/// neighbours on a grid too coarse for that. NaN with fewer than three columns to fit.
fn fitted_group_delay(freqs: &[f64], unwrapped: &[f64], coherence: &[f64], b: u32) -> Vec<f64> {
    // γ² of 1 would weigh infinitely; 1 − 10⁻⁶ already outweighs any noisy column.
    let weight = |g: f64| {
        if g.is_finite() {
            let g = g.clamp(0.0, 1.0 - 1e-6);
            g / (1.0 - g)
        } else {
            1.0
        }
    };
    let half_oct = 0.5 / f64::from(b) * (1.0 + 1e-9);
    let n = freqs.len();
    let mut out = vec![f64::NAN; n];
    for i in 0..n {
        let fi = freqs[i];
        if !unwrapped[i].is_finite() || fi.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            continue;
        }
        let near = |j: usize| j.abs_diff(i) <= 1 || (freqs[j] / fi).log2().abs() <= half_oct;
        let lo = (0..i).rev().take_while(|&j| near(j)).last().unwrap_or(i);
        let hi = (i + 1..n).take_while(|&j| near(j)).last().unwrap_or(i);
        let (mut sw, mut sx, mut sy, mut sxx, mut sxy, mut used) = (0.0, 0.0, 0.0, 0.0, 0.0, 0);
        for j in lo..=hi {
            let w = weight(coherence[j]);
            if !unwrapped[j].is_finite() || w <= 0.0 {
                continue;
            }
            let (x, y) = (freqs[j] - fi, unwrapped[j] - unwrapped[i]);
            sw += w;
            sx += w * x;
            sy += w * y;
            sxx += w * x * x;
            sxy += w * x * y;
            used += 1;
        }
        let den = sw * sxx - sx * sx;
        if used >= 3 && den > 0.0 {
            out[i] = -(sw * sxy - sx * sy) / den / 360.0;
        }
    }
    out
}

fn get(v: Option<&[f32]>, i: usize) -> f64 {
    v.and_then(|v| v.get(i)).map_or(f64::NAN, |x| f64::from(*x))
}

/// Applies the display math of the module docs to one trace.
pub fn display_trace(
    t: &TfTrace<'_>,
    reference: Option<&PhaseReference>,
    style: &CoherenceStyle,
) -> DisplayTrace {
    let n = t.freqs.len().min(t.mag_db.len());
    let (shift_s, relation) = phase_shift(t, reference);
    let inverted = t.polarity == Polarity::Inverted;
    let flip = if inverted { 180.0 } else { 0.0 };
    let valid: Vec<bool> = (0..n)
        .map(|i| {
            t.validity
                .is_none_or(|v| v.get(i).is_some_and(|m| *m == ValidityMask::NONE))
                && t.freqs[i].is_finite()
                && t.mag_db[i].is_finite()
        })
        .collect();

    let coherence: Vec<f64> = (0..n)
        .map(|i| {
            if valid[i] {
                get(t.coherence, i)
            } else {
                f64::NAN
            }
        })
        .collect();
    let blanked: Vec<bool> = (0..n)
        .map(|i| {
            style
                .blank_below
                .is_some_and(|th| coherence[i].is_finite() && coherence[i] < f64::from(th))
        })
        .collect();
    let shown = |i: usize| valid[i] && !blanked[i];

    let magnitude_db = (0..n)
        .map(|i| {
            if shown(i) {
                f64::from(t.mag_db[i]) + t.offset_db
            } else {
                f64::NAN
            }
        })
        .collect();

    // Measured phase (+ polarity), unwrapped over valid columns. Blanking does not cut the
    // unwrap: blanked columns still carry phase continuity.
    let raw: Vec<f64> = (0..n)
        .map(|i| {
            let p = get(t.phase_deg, i);
            if valid[i] && p.is_finite() {
                p + flip
            } else {
                f64::NAN
            }
        })
        .collect();
    let mut unwrapped = vec![f64::NAN; n];
    let mut prev: Option<f64> = None;
    for i in 0..n {
        if !raw[i].is_finite() {
            continue;
        }
        let u = match prev {
            None => wrap_deg(raw[i]),
            Some(p) => p + wrap_deg(raw[i] - p),
        };
        unwrapped[i] = u;
        prev = Some(u);
    }
    let rot = |i: usize| -360.0 * t.freqs[i] * shift_s;

    // Group delay of the measured phase, then shifted by Δ: −dφ/df / 360 of the rotation
    // term is exactly Δ, so it is added rather than fitted.
    let mut group_delay_s = fitted_group_delay(
        &t.freqs[..n],
        &unwrapped,
        &coherence,
        group_delay_span(t.smoothing),
    );
    group_delay_s.iter_mut().for_each(|g| *g += shift_s);

    let phase_unwrapped_deg: Vec<f64> = (0..n)
        .map(|i| {
            if shown(i) {
                unwrapped[i] + rot(i)
            } else {
                f64::NAN
            }
        })
        .collect();
    let phase_wrapped_deg = (0..n)
        .map(|i| {
            if shown(i) && raw[i].is_finite() {
                wrap_deg(raw[i] + rot(i))
            } else {
                f64::NAN
            }
        })
        .collect();
    for (i, g) in group_delay_s.iter_mut().enumerate() {
        if !shown(i) {
            *g = f64::NAN;
        }
    }
    let alpha = coherence
        .iter()
        .map(|&g| coherence_alpha(g, style))
        .collect();

    DisplayTrace {
        key: t.key,
        name: t.name.clone(),
        color: t.color,
        relation,
        shift_s,
        offset_db: t.offset_db,
        inverted,
        freqs: t.freqs[..n].to_vec(),
        magnitude_db,
        phase_wrapped_deg,
        phase_unwrapped_deg,
        group_delay_s,
        coherence,
        alpha,
        freshness: t.freshness,
        smoothing: t.smoothing,
        note: t.note.clone(),
    }
}

/// What a stored trace's display math depends on beyond its name and colour.
#[derive(Debug)]
struct CacheKey {
    data: Arc<TraceData>,
    freqs: Vec<f64>,
    offset_db: f64,
    polarity: Polarity,
    shift_s: f64,
    relation: PhaseRelation,
    style: CoherenceStyle,
}

impl CacheKey {
    fn matches(
        &self,
        t: &TfTrace<'_>,
        data: &Arc<TraceData>,
        shift: (f64, PhaseRelation),
        style: &CoherenceStyle,
    ) -> bool {
        // The entry holds its `Arc`, so an equal pointer is the same, unchanged columns.
        Arc::ptr_eq(&self.data, data)
            && self.freqs == t.freqs
            && self.offset_db.to_bits() == t.offset_db.to_bits()
            && self.polarity == t.polarity
            && self.shift_s.to_bits() == shift.0.to_bits()
            && self.relation == shift.1
            && self.style == *style
    }
}

/// The trace's columns are its stored data's own (only then can they be keyed by it).
fn columns_of_stored(t: &TfTrace<'_>, data: &TraceData) -> bool {
    t.validity.is_none()
        && std::ptr::eq(t.mag_db, data.mag_db.as_slice())
        && t.phase_deg.map(<[f32]>::as_ptr) == data.phase_deg.as_deref().map(<[f32]>::as_ptr)
        && t.coherence.map(<[f32]>::as_ptr) == data.coherence.as_deref().map(<[f32]>::as_ptr)
}

/// Stored traces' display math from the previous scene. A stored trace's columns do not
/// change, so its unwrap, group delay and coherence opacity are worked out again only when
/// its data, edits, reference or the coherence style change — not on every frame a live
/// trace beside it moves. Entries not used by a scene are dropped. A clone starts empty.
///
/// It also keeps what min / max tracking marked on each curve in the previous scene, so the
/// marks hold still from frame to frame ([`crate::spectrum::peaks::PeakTracker`]).
#[derive(Default)]
pub struct DisplayCache {
    traces: Mutex<Vec<(CacheKey, DisplayTrace)>>,
    extremes: Mutex<Vec<(TraceKey, ExtremeTrackers)>>,
}

/// A curve's peak and dip trackers.
#[derive(Clone, Debug)]
struct ExtremeTrackers {
    max: crate::spectrum::peaks::PeakTracker,
    min: crate::spectrum::peaks::PeakTracker,
}

/// Curves whose marks are kept: a few transfer panes' worth, so the list stays small
/// while each pane's tracked curve keeps its marks.
const EXTREME_CURVES: usize = 8;

impl Clone for DisplayCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for DisplayCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.traces.lock().map_or(0, |v| v.len());
        write!(f, "DisplayCache({n})")
    }
}

impl DisplayCache {
    /// Stored traces whose display math is kept.
    pub fn len(&self) -> usize {
        self.traces.lock().map_or(0, |v| v.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `key`'s marks chosen again from a scene's `max` peaks and `min` dips: (peaks, dips),
    /// each in list order.
    pub(crate) fn track_extremes(
        &self,
        key: TraceKey,
        max: &[crate::spectrum::peaks::Peak],
        min: &[crate::spectrum::peaks::Peak],
    ) -> (
        Vec<crate::spectrum::peaks::Peak>,
        Vec<crate::spectrum::peaks::Peak>,
    ) {
        use crate::spectrum::peaks::{Extreme, PeakTracker};
        let mut all = self.extremes.lock().unwrap_or_else(|e| e.into_inner());
        let mut t = match all.iter().position(|(k, _)| *k == key) {
            Some(i) => all.remove(i).1,
            None => ExtremeTrackers {
                max: PeakTracker::new(Extreme::Max),
                min: PeakTracker::new(Extreme::Min),
            },
        };
        t.max.update(max);
        t.min.update(min);
        let out = (t.max.peaks().to_vec(), t.min.peaks().to_vec());
        all.push((key, t));
        if all.len() > EXTREME_CURVES {
            all.remove(0);
        }
        out
    }

    /// Forgets every curve's marks: tracking off, the next start chooses afresh.
    pub(crate) fn clear_extremes(&self) {
        self.extremes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

/// Display math for every trace against the resolved reference; stored traces come from
/// `cache` when nothing they depend on changed.
pub fn display_traces(
    traces: &[TfTrace<'_>],
    cache: &DisplayCache,
    wanted_reference: Option<TraceKey>,
    style: &CoherenceStyle,
) -> (Option<PhaseReference>, Vec<DisplayTrace>) {
    let reference = resolve_reference(traces, wanted_reference);
    let mut old = cache.traces.lock().unwrap_or_else(|e| e.into_inner());
    let mut kept = Vec::new();
    let shown = traces
        .iter()
        .map(|t| {
            let Some(data) = t.stored.filter(|d| columns_of_stored(t, d)) else {
                return display_trace(t, reference.as_ref(), style);
            };
            let shift = phase_shift(t, reference.as_ref());
            let hit = old
                .iter()
                .position(|(k, _)| k.matches(t, data, shift, style));
            let (key, mut d) = match hit {
                Some(i) => old.swap_remove(i),
                None => (
                    CacheKey {
                        data: Arc::clone(data),
                        freqs: t.freqs.to_vec(),
                        offset_db: t.offset_db,
                        polarity: t.polarity,
                        shift_s: shift.0,
                        relation: shift.1,
                        style: *style,
                    },
                    display_trace(t, reference.as_ref(), style),
                ),
            };
            d.key = t.key;
            d.name.clone_from(&t.name);
            d.color = t.color;
            d.freshness = t.freshness;
            d.smoothing = t.smoothing;
            kept.push((key, d.clone()));
            d
        })
        .collect();
    *old = kept;
    (reference, shown)
}

/// What an import note tells the operator.
pub fn import_note(n: ImportNote) -> &'static str {
    match n {
        ImportNote::SweepWithoutAnalysis => {
            "sweep export without its analysis facts and impulse response (written by an \
             older ac2): imported as its transfer function, distortion dropped"
        }
        ImportNote::SweepOffGrid => {
            "sweep export off the grid its header names: the response was resampled and \
             the distortion (never resampled) dropped"
        }
        ImportNote::MicCurveNotApplied => {
            "the export showed a mic curve applied after capture; the columns are without \
             it (apply it again: ac2 trace mic)"
        }
        ImportNote::IrTruncated => {
            "impulse response longer than the 131072 samples a trace keeps: cut from shortly \
             before its peak"
        }
    }
}

/// An imported impulse-response file in one line, the same in the app and the CLI:
/// `IR · channel 2 of 2 · 48 kHz · 4096 samples · peak at 2.083 ms · whole file · phase re peak`.
pub fn ir_file_text(f: &ac2_proto::model::IrFile) -> String {
    use ac2_proto::model::{IrAlign, IrImportWindow};
    let rate = format!("{} kHz", f.sample_rate.0 / 1000.0);
    let samples = if u64::from(f.kept) == f.frames {
        format!("{} samples", f.frames)
    } else {
        format!("{} of {} samples from {}", f.kept, f.frames, f.first)
    };
    let window = match f.import.window {
        IrImportWindow::Whole => "whole file".to_owned(),
        IrImportWindow::Gate { pre, post } => format!(
            "gate {} before, {} after the peak",
            crate::format::ms(pre.0, 1),
            crate::format::ms(post.0, 1)
        ),
    };
    let align = match f.import.align {
        IrAlign::Peak => "phase re peak",
        IrAlign::Start => "phase re file start",
    };
    format!(
        "IR · channel {} of {} · {rate} · {samples} · peak at {} · {window} · {align}",
        u32::from(f.channel) + 1,
        f.channels,
        crate::format::ms(f.peak.0, 3),
    )
}

/// A trace's mic and curve, e.g. `MM1 34804 (curve 90° in the columns, file …)`,
/// `MM1 34804 (curve 90° applied after capture, 0 dB at 1000 Hz, file …)`, `—`.
pub fn mic_text(mic: Option<&MicState>, applied: Option<&TraceMicCurve>) -> String {
    match (mic, applied) {
        (_, Some(a)) => format!(
            "{} (curve {} applied after capture, 0 dB at {} Hz, file {})",
            a.mic,
            a.curve.label,
            crate::format::fixed(a.f_norm.0, 0),
            a.curve.file_name
        ),
        (Some(m), None) => match &m.curve {
            Some(c) => format!(
                "{} (curve {} in the columns, file {})",
                m.name, c.label, c.file_name
            ),
            None => format!("{} (no curve)", m.name),
        },
        (None, None) => "—".into(),
    }
}

/// The mic-curve note of a stored trace's caption: `mic curve: MM1 34804 90°` whether the
/// curve is in its columns or applied afterwards; nothing without one.
pub fn curve_note(mic: Option<&MicState>, applied: Option<&TraceMicCurve>) -> Option<String> {
    match (mic, applied) {
        (_, Some(a)) => Some(format!(
            "mic curve: {}",
            crate::cal::curve_name(&a.mic, &a.curve.label)
        )),
        (Some(m), None) => m
            .curve
            .as_ref()
            .map(|c| format!("mic curve: {}", crate::cal::curve_name(&m.name, &c.label))),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests;
