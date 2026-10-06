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
//! - **Unwrapped phase and group delay** for the alternative phase views.
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

use ac2_proto::frame::{FrameStamp, TfFrame, ValidityMask};
use ac2_proto::model::{
    ImportNote, MicState, Polarity, Smoothing, TraceData, TraceMicCurve, TraceSource,
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
}

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
            time_base: TimeBase::Shared {
                epoch: stamp.session_epoch,
                delay: frame.meta.delay,
            },
            freshness: Some(freshness),
            smoothing: frame.meta.smoothing,
            note: None,
            stored: None,
            selected: false,
        }
    }

    /// A stored trace; colour, offset, polarity and nudge come from its edit record.
    /// Captured traces keep their epoch's shared time base; every other source is
    /// independent (decision 8a).
    pub fn stored(data: &'a Arc<TraceData>, freqs: &'a [f64]) -> Self {
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
        let c = m.edit.color;
        Self {
            key: TraceKey::Stored(m.id),
            name: m.edit.name.clone(),
            color: Color::from_rgba8([c.r, c.g, c.b, 255]),
            freqs,
            mag_db: &data.mag_db,
            phase_deg: data.phase_deg.as_deref(),
            coherence: data.coherence.as_deref(),
            validity: None,
            offset_db: m.edit.offset.0,
            polarity: m.edit.polarity,
            nudge: m.edit.delay_nudge,
            time_base,
            freshness: None,
            smoothing: m.edit.smoothing,
            note,
            stored: Some(data),
            selected: false,
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
    /// Group delay of the rotated phase, seconds.
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

    // Group delay of the measured phase by central differences (one-sided at the ends of a
    // run), then shifted by Δ: −dφ/df / 360 of the rotation term is exactly Δ.
    let mut group_delay_s = vec![f64::NAN; n];
    for i in 0..n {
        if !unwrapped[i].is_finite() {
            continue;
        }
        let lo = (i > 0 && unwrapped[i - 1].is_finite()).then(|| i - 1);
        let hi = (i + 1 < n && unwrapped[i + 1].is_finite()).then_some(i + 1);
        let (a, b) = match (lo, hi) {
            (Some(a), Some(b)) => (a, b),
            (Some(a), None) => (a, i),
            (None, Some(b)) => (i, b),
            (None, None) => continue,
        };
        let df = t.freqs[b] - t.freqs[a];
        if df > 0.0 {
            group_delay_s[i] = -(unwrapped[b] - unwrapped[a]) / (360.0 * df) + shift_s;
        }
    }

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
#[derive(Default)]
pub struct DisplayCache(Mutex<Vec<(CacheKey, DisplayTrace)>>);

impl Clone for DisplayCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for DisplayCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.0.lock().map_or(0, |v| v.len());
        write!(f, "DisplayCache({n})")
    }
}

impl DisplayCache {
    /// Stored traces whose display math is kept.
    pub fn len(&self) -> usize {
        self.0.lock().map_or(0, |v| v.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
    let mut old = cache.0.lock().unwrap_or_else(|e| e.into_inner());
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
    }
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
mod tests {
    use super::*;

    #[test]
    fn mic_wording() {
        use ac2_proto::model::MicCurveRef;
        use ac2_proto::units::{Hz, WallNs};
        let c = MicCurveRef {
            label: "90°".into(),
            file_name: "449350_34804_90Grad.txt".into(),
            content_hash: "0".into(),
            points: 2,
            f_lo: Hz(20.0),
            f_hi: Hz(20_000.0),
            imported_at: WallNs(0),
            stated_sensitivity: None,
        };
        let m = MicState {
            name: "MM1 34804".into(),
            curve: Some(c.clone()),
        };
        assert_eq!(
            mic_text(Some(&m), None),
            "MM1 34804 (curve 90° in the columns, file 449350_34804_90Grad.txt)"
        );
        assert_eq!(
            curve_note(Some(&m), None).as_deref(),
            Some("mic curve: MM1 34804 90°")
        );
        let a = TraceMicCurve {
            mic: "MM1 34804".into(),
            curve: MicCurveRef {
                label: "0°".into(),
                ..c
            },
            f_norm: Hz(1000.0),
        };
        assert_eq!(
            mic_text(None, Some(&a)),
            "MM1 34804 (curve 0° applied after capture, 0 dB at 1000 Hz, file \
             449350_34804_90Grad.txt)"
        );
        assert_eq!(
            curve_note(None, Some(&a)).as_deref(),
            Some("mic curve: MM1 34804 0°")
        );
        assert_eq!(mic_text(None, None), "—");
        assert_eq!(curve_note(None, None), None);
    }

    const EPOCH: SessionEpoch = SessionEpoch(3);

    fn log_freqs(n: usize) -> Vec<f64> {
        // 20 Hz … 20 kHz, 48 per octave-ish.
        (0..n)
            .map(|i| 20.0 * 1000f64.powf(i as f64 / (n - 1) as f64))
            .collect()
    }

    /// Phase a TF with the reference delayed by `comp` shows for a path that is a pure
    /// delay `tau`: −360·f·(tau − comp), wrapped.
    fn measured_phase(freqs: &[f64], tau: f64, comp: f64) -> Vec<f32> {
        freqs
            .iter()
            .map(|f| wrap_deg(-360.0 * f * (tau - comp)) as f32)
            .collect()
    }

    struct Data {
        freqs: Vec<f64>,
        mag: Vec<f32>,
        phase: Vec<f32>,
        coh: Vec<f32>,
        validity: Vec<ValidityMask>,
    }

    impl Data {
        fn delay(n: usize, tau: f64, comp: f64) -> Self {
            let freqs = log_freqs(n);
            Self {
                phase: measured_phase(&freqs, tau, comp),
                mag: vec![0.0; n],
                coh: vec![1.0; n],
                validity: vec![ValidityMask::NONE; n],
                freqs,
            }
        }

        fn trace(&self, key: u32, time_base: TimeBase) -> TfTrace<'_> {
            TfTrace {
                key: TraceKey::Live(MeasId(key)),
                name: format!("m{key}"),
                color: Color::WHITE,
                freqs: &self.freqs,
                mag_db: &self.mag,
                phase_deg: Some(&self.phase),
                coherence: Some(&self.coh),
                validity: Some(&self.validity),
                offset_db: 0.0,
                polarity: Polarity::Normal,
                nudge: Seconds(0.0),
                time_base,
                freshness: None,
                smoothing: None,
                note: None,
                stored: None,
                selected: false,
            }
        }
    }

    fn shared(delay: f64) -> TimeBase {
        TimeBase::Shared {
            epoch: EPOCH,
            delay: Seconds(delay),
        }
    }

    #[test]
    fn wrap() {
        assert_eq!(wrap_deg(180.0), 180.0);
        assert_eq!(wrap_deg(-180.0), 180.0);
        assert_eq!(wrap_deg(190.0), -170.0);
        assert_eq!(wrap_deg(-190.0), 170.0);
        assert_eq!(wrap_deg(720.0), 0.0);
    }

    /// Decision 8b / Q8 test: two otherwise identical paths with different physical
    /// delays, each measured with its own delay compensated, show their relative phase in
    /// shared mode.
    #[test]
    fn identical_paths_show_relative_delay() {
        let (t1, t2) = (0.010, 0.0115); // 10 ms and 11.5 ms
        let a = Data::delay(200, t1, t1);
        let b = Data::delay(200, t2, t2);
        // Each trace on its own is flat: perfectly compensated.
        assert!(a.phase.iter().chain(&b.phase).all(|p| p.abs() < 1e-3));
        let traces = [a.trace(1, shared(t1)), b.trace(2, shared(t2))];
        let (r, d) = display_traces(
            &traces,
            &DisplayCache::default(),
            None,
            &CoherenceStyle::default(),
        );
        assert_eq!(r.map(|r| r.key), Some(TraceKey::Live(MeasId(1))));
        assert_eq!(d[0].relation, PhaseRelation::Reference);
        assert_eq!(d[1].relation, PhaseRelation::Relative);
        assert!((d[1].shift_s - 0.0015).abs() < 1e-12);
        for (i, f) in d[1].freqs.iter().enumerate() {
            let want = wrap_deg(-360.0 * f * (t2 - t1));
            let got = d[1].phase_wrapped_deg[i];
            assert!(wrap_deg(got - want).abs() < 1e-3, "{f} Hz: {got} vs {want}");
            assert!(d[0].phase_wrapped_deg[i].abs() < 1e-3);
            // Group delay reads the 1.5 ms arrival difference.
            assert!((d[1].group_delay_s[i] - 0.0015).abs() < 1e-6, "{f}");
            assert!(d[0].group_delay_s[i].abs() < 1e-6);
        }
        // At 1 kHz a 1.5 ms lag is −540° ≡ 180°.
        let k = crate::grid::nearest_column(&d[1].freqs, 1000.0).expect("col");
        let want = wrap_deg(-360.0 * d[1].freqs[k] * 0.0015);
        assert!((d[1].phase_wrapped_deg[k] - want).abs() < 1e-3);
        // Picking the other trace as reference flips the sign.
        let (_, d) = display_traces(
            &traces,
            &DisplayCache::default(),
            Some(TraceKey::Live(MeasId(2))),
            &CoherenceStyle::default(),
        );
        assert!((d[0].shift_s + 0.0015).abs() < 1e-12);
        assert!((d[0].group_delay_s[50] + 0.0015).abs() < 1e-6);
        assert_eq!(d[1].relation, PhaseRelation::Reference);
    }

    #[test]
    fn nudge_and_independent() {
        let a = Data::delay(100, 0.01, 0.01);
        let b = Data::delay(100, 0.02, 0.02);
        let mut tb = b.trace(2, shared(0.02));
        tb.nudge = Seconds(0.0005);
        let ta = a.trace(1, shared(0.01));
        let (_, d) = display_traces(
            &[ta.clone(), tb],
            &DisplayCache::default(),
            None,
            &CoherenceStyle::default(),
        );
        // Δ = τ_k − τ_ref − ν = 10 ms − 0.5 ms.
        assert!((d[1].shift_s - 0.0095).abs() < 1e-12);
        // Independent (imported) trace keeps its own alignment, nudge only.
        let mut ti = b.trace(3, TimeBase::Independent);
        ti.nudge = Seconds(0.001);
        let (_, d) = display_traces(
            &[ta.clone(), ti],
            &DisplayCache::default(),
            None,
            &CoherenceStyle::default(),
        );
        assert_eq!(d[1].relation, PhaseRelation::Independent);
        assert!((d[1].shift_s + 0.001).abs() < 1e-12);
        assert!((d[1].group_delay_s[10] + 0.001).abs() < 1e-6);
        // Another epoch is independent too.
        let to = b.trace(
            4,
            TimeBase::Shared {
                epoch: SessionEpoch(9),
                delay: Seconds(0.02),
            },
        );
        let (_, d) = display_traces(
            &[ta, to],
            &DisplayCache::default(),
            None,
            &CoherenceStyle::default(),
        );
        assert_eq!(d[1].relation, PhaseRelation::Independent);
        assert_eq!(d[1].shift_s, 0.0);
    }

    #[test]
    fn reference_falls_back_to_first_shared() {
        let a = Data::delay(10, 0.0, 0.0);
        let ti = a.trace(1, TimeBase::Independent);
        let ts = a.trace(2, shared(0.004));
        let traces = [ti, ts];
        // Wanting an independent trace cannot give a delay: first shared one is used.
        let r = resolve_reference(&traces, Some(TraceKey::Live(MeasId(1)))).expect("ref");
        assert_eq!(r.key, TraceKey::Live(MeasId(2)));
        assert_eq!(r.delay, Seconds(0.004));
        assert!(resolve_reference(&traces[..1], None).is_none());
    }

    #[test]
    fn polarity_offset_and_rotation_leave_magnitude() {
        let a = Data::delay(50, 0.001, 0.001);
        let mut t = a.trace(1, shared(0.001));
        t.offset_db = -6.0;
        t.polarity = Polarity::Inverted;
        let d = display_trace(&t, None, &CoherenceStyle::default());
        assert!(d.magnitude_db.iter().all(|m| *m == -6.0));
        assert!(
            d.phase_wrapped_deg
                .iter()
                .all(|p| (p.abs() - 180.0).abs() < 1e-3)
        );
        assert!(d.inverted);
    }

    #[test]
    fn stored_display_math_is_kept_until_what_it_depends_on_changes() {
        let mut raw = crate::distortion::tests::data();
        let n = raw.mag_db.len();
        raw.phase_deg = Some((0..n).map(|i| (i as f32 * 37.0) % 360.0 - 180.0).collect());
        raw.coherence = Some((0..n).map(|i| (i % 10) as f32 / 10.0).collect());
        let data = Arc::new(raw);
        let freqs = crate::grid::column_frequencies(&crate::distortion::tests::grid());
        let live = Data::delay(n, 0.001, 0.001);
        let cache = DisplayCache::default();
        let style = CoherenceStyle {
            blank_below: Some(0.3),
            ..CoherenceStyle::default()
        };
        // NaN gaps make `==` useless on whole traces; their debug text compares them.
        let fresh = |traces: &[TfTrace<'_>], style: &CoherenceStyle| {
            format!(
                "{:?}",
                display_traces(traces, &DisplayCache::default(), None, style)
            )
        };
        let kept = |traces: &[TfTrace<'_>], style: &CoherenceStyle| {
            format!("{:?}", display_traces(traces, &cache, None, style))
        };
        let mut traces = vec![live.trace(1, shared(0.001)), TfTrace::stored(&data, &freqs)];
        let first = kept(&traces, &style);
        assert_eq!(first, fresh(&traces, &style));
        assert_eq!(cache.len(), 1);
        // Unchanged: the kept result, identical to working it out again.
        assert_eq!(kept(&traces, &style), first);
        // Every input it depends on lays it out again.
        traces[1].offset_db = 3.0;
        traces[1].name = "renamed".into();
        let edited = kept(&traces, &style);
        assert_ne!(edited, first);
        assert_eq!(edited, fresh(&traces, &style));
        assert!(edited.contains("renamed"));
        traces[1].polarity = Polarity::Inverted;
        traces[1].nudge = Seconds(0.0005);
        assert_eq!(kept(&traces, &style), fresh(&traces, &style));
        // The reference moves: its delay changes the stored trace's rotation.
        traces[0] = live.trace(1, shared(0.002));
        assert_eq!(kept(&traces, &style), fresh(&traces, &style));
        let other = CoherenceStyle::default();
        assert_eq!(kept(&traces, &other), fresh(&traces, &other));
        // New data for the trace (a re-fetch) replaces the kept result.
        let mut refetched = (*data).clone();
        refetched.mag_db[9] = 12.0;
        let refetched = Arc::new(refetched);
        traces[1] = TfTrace::stored(&refetched, &freqs);
        assert_eq!(kept(&traces, &other), fresh(&traces, &other));
        let (_, d) = display_traces(&traces, &cache, None, &other);
        assert_eq!(d[1].magnitude_db[9], 12.0);
        assert_eq!(cache.len(), 1);
        // A trace no longer shown is forgotten.
        traces.pop();
        display_traces(&traces, &cache, None, &other);
        assert!(cache.is_empty());
    }

    #[test]
    fn invalid_columns_are_gaps_everywhere() {
        let mut a = Data::delay(20, 0.0, 0.0);
        a.validity[5] = ValidityMask::SETTLING;
        a.mag[7] = f32::NAN;
        let d = display_trace(&a.trace(1, shared(0.0)), None, &CoherenceStyle::default());
        for i in [5, 7] {
            assert!(d.magnitude_db[i].is_nan());
            assert!(d.phase_wrapped_deg[i].is_nan());
            assert!(d.group_delay_s[i].is_nan());
            assert!(d.coherence[i].is_nan());
        }
        assert!(d.magnitude_db[6].is_finite());
        // Group delay next to a gap is one-sided; an isolated column has none.
        assert!(d.group_delay_s[4].is_finite() && d.group_delay_s[8].is_finite());
        assert!(d.group_delay_s[6].is_nan());
    }

    #[test]
    fn coherence_blanking_and_alpha() {
        let style = CoherenceStyle {
            blank_below: Some(0.5),
            alpha: true,
            alpha_floor: 0.2,
            alpha_full_at: 0.9,
        };
        assert_eq!(coherence_alpha(0.5, &style), 0.2);
        assert_eq!(coherence_alpha(0.9, &style), 1.0);
        assert_eq!(coherence_alpha(0.95, &style), 1.0);
        assert!((coherence_alpha(0.7, &style) - 0.6).abs() < 1e-6);
        assert_eq!(coherence_alpha(f64::NAN, &style), 1.0);
        let off = CoherenceStyle {
            alpha: false,
            ..style
        };
        assert_eq!(coherence_alpha(0.1, &off), 1.0);
        let default = CoherenceStyle::default();
        assert_eq!(coherence_alpha(0.0, &default), 0.15);

        let mut a = Data::delay(10, 0.0, 0.0);
        a.coh[3] = 0.3;
        let d = display_trace(&a.trace(1, shared(0.0)), None, &style);
        // Blanked: gap in magnitude and phase, coherence itself still shown.
        assert!(d.magnitude_db[3].is_nan() && d.phase_wrapped_deg[3].is_nan());
        assert!((d.coherence[3] - 0.3).abs() < 1e-6);
        assert!(d.magnitude_db[2].is_finite());
        assert_eq!(d.alpha[2], 1.0);
    }

    #[test]
    fn unwrap_follows_a_long_delay() {
        // 2 ms uncompensated: phase falls 720° per kHz; unwrapping recovers the line.
        let a = Data::delay(400, 0.002, 0.0);
        let d = display_trace(
            &a.trace(1, TimeBase::Independent),
            None,
            &CoherenceStyle::default(),
        );
        let i0 = 0;
        for i in 1..200 {
            let want = -360.0 * (d.freqs[i] - d.freqs[i0]) * 0.002;
            let got = d.phase_unwrapped_deg[i] - d.phase_unwrapped_deg[i0];
            assert!((got - want).abs() < 1e-2, "{}: {got} vs {want}", d.freqs[i]);
            assert!((d.group_delay_s[i] - 0.002).abs() < 1e-6);
        }
    }
}
