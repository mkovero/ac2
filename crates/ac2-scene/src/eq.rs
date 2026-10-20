//! EQ what-if text (`docs/design/eq-overlay.md` §3): one vocabulary for the editor, the
//! legend and the CLI. A set is `EQ 1`; a prediction `Main L after EQ 1 · assumes none of it
//! in`; a section `3 · peaking 1.25 kHz −4.0 dB Q 2.0 (half-gain)`.

use ac2_proto::model::{
    Alignment, Bandwidth, BesselNorm, EqBasis, EqRealisation, EqSection, EqSet, EqShape, MathExpr,
    MathOp, Operand, PeakQ, ShelfFreq, ShelfOrder, ShelfSlope, TraceMeta,
};
use ac2_proto::units::{EqSetId, MeasId, TraceId};

use crate::format;

/// What a prediction's legend says about what its curve already contains of the set.
pub fn basis_tag(set_name: &str, basis: EqBasis) -> String {
    match basis {
        EqBasis::Whole => "assumes none of it in".to_owned(),
        EqBasis::Change { since } => format!("change since rev {since}"),
        EqBasis::NoChange { .. } => format!("{set_name} in · no change"),
    }
}

/// A set's tags after its name: `bypassed`, `in console (rev 4)`.
pub fn set_tags(set: &EqSet) -> Vec<String> {
    let mut t = Vec::new();
    if !set.body.enabled {
        t.push("bypassed".to_owned());
    }
    if let Some(s) = &set.in_console {
        t.push(format!("in console (rev {})", s.rev));
    }
    t
}

/// The mark as the editor's header and `eq show` say it.
pub fn mark_text(set: &EqSet) -> String {
    match &set.in_console {
        Some(s) => format!("in console (rev {})", s.rev),
        None => "not marked".to_owned(),
    }
}

/// `digital 48 kHz`, `analog`.
pub fn realisation_text(r: EqRealisation) -> String {
    match r {
        EqRealisation::Digital { rate } => format!("digital {}", rate_text(rate.0)),
        EqRealisation::Analog => "analog".to_owned(),
    }
}

fn rate_text(hz: f64) -> String {
    format!("{} kHz", format::freq_tick(hz).trim_end_matches('k'))
}

/// How a peaking Q is read: `half-gain`, `3 dB from peak`, `band-pass`.
pub fn peak_q_text(p: PeakQ) -> &'static str {
    match p {
        PeakQ::HalfGain => "half-gain",
        PeakQ::Minus3dbFromPeak => "3 dB from peak",
        PeakQ::BandPass => "band-pass",
    }
}

/// A shape's word, as the daemon's refusals name it.
pub fn shape_word(shape: &EqShape) -> &'static str {
    match shape {
        EqShape::Peaking { .. } => "peaking",
        EqShape::LowShelf { .. } => "low shelf",
        EqShape::HighShelf { .. } => "high shelf",
        EqShape::HighPass { .. } => "high-pass",
        EqShape::LowPass { .. } => "low-pass",
        EqShape::HighPassQ { .. } => "high-pass Q",
        EqShape::LowPassQ { .. } => "low-pass Q",
        EqShape::Notch { .. } => "notch",
        EqShape::BandPass { .. } => "band-pass",
        EqShape::AllPass1 { .. } => "all-pass 1st order",
        EqShape::AllPass2 { .. } => "all-pass 2nd order",
        EqShape::Gain { .. } => "gain",
        EqShape::Polarity => "polarity",
        EqShape::Delay { .. } => "delay",
        EqShape::Biquad { .. } => "biquad",
        EqShape::Response { .. } => "response",
    }
}

/// `Q 2.0`, `Q 0.71`: two significant digits below 1, one decimal above.
pub fn q_text(q: f64) -> String {
    format!("Q {}", format::fixed(q, if q < 1.0 { 2 } else { 1 }))
}

/// `Q 2.0`, `0.70 oct`.
pub fn bw_text(b: Bandwidth) -> String {
    match b {
        Bandwidth::Q { value } => q_text(value),
        Bandwidth::Octaves { n } => format!("{} oct", format::fixed(n, 2)),
    }
}

fn gain_text(db: f64) -> String {
    format!("{} dB", format::signed(db, 1))
}

fn alignment_text(a: Alignment, order: u8) -> String {
    match a {
        Alignment::Butterworth => format!("BW {order}"),
        Alignment::LinkwitzRiley => format!("LR {order}"),
        Alignment::Bessel { norm } => format!(
            "Bessel {order} ({})",
            match norm {
                BesselNorm::Phase => "phase",
                BesselNorm::Delay => "delay",
                BesselNorm::Minus3db => "−3 dB",
            }
        ),
    }
}

fn shelf_text(order: ShelfOrder, f_at: ShelfFreq) -> String {
    let o = match order {
        ShelfOrder::First => "1st order".to_owned(),
        ShelfOrder::Second {
            slope: ShelfSlope::S { value },
        } => format!("S {}", format::fixed(value, 2)),
        ShelfOrder::Second {
            slope: ShelfSlope::Q { value },
        } => q_text(value),
        ShelfOrder::Second {
            slope: ShelfSlope::Octaves { n },
        } => format!("{} oct", format::fixed(n, 2)),
    };
    match f_at {
        ShelfFreq::Midpoint => o,
        ShelfFreq::Corner => format!("{o} (corner)"),
    }
}

/// What a section is, without its number: `peaking 1.25 kHz −4.0 dB Q 2.0 (half-gain)`.
/// `peak_q`: the set's default reading; `trace_name` names a `response` section's trace.
pub fn shape_text(
    shape: &EqShape,
    peak_q: PeakQ,
    trace_name: &dyn Fn(TraceId) -> String,
) -> String {
    let w = shape_word(shape);
    let f = |hz: f64| format::freq_readout(hz);
    match *shape {
        EqShape::Peaking {
            f: fr,
            gain,
            bw,
            q_measured,
        } => format!(
            "{w} {} {} {} ({})",
            f(fr.0),
            gain_text(gain.0),
            bw_text(bw),
            peak_q_text(q_measured.unwrap_or(peak_q))
        ),
        EqShape::LowShelf {
            f: fr,
            gain,
            order,
            f_at,
        }
        | EqShape::HighShelf {
            f: fr,
            gain,
            order,
            f_at,
        } => format!(
            "{w} {} {} {}",
            f(fr.0),
            gain_text(gain.0),
            shelf_text(order, f_at)
        ),
        EqShape::HighPass {
            f: fr,
            alignment,
            order,
        }
        | EqShape::LowPass {
            f: fr,
            alignment,
            order,
        } => format!("{w} {} {}", f(fr.0), alignment_text(alignment, order)),
        EqShape::HighPassQ { f: fr, q } | EqShape::LowPassQ { f: fr, q } => {
            format!("{} {} {}", &w[..w.len() - 2], f(fr.0), q_text(q))
        }
        EqShape::Notch { f: fr, bw } | EqShape::BandPass { f: fr, bw } => {
            format!("{w} {} {}", f(fr.0), bw_text(bw))
        }
        EqShape::AllPass1 { f: fr } => format!("{w} {}", f(fr.0)),
        EqShape::AllPass2 { f: fr, q } => format!("{w} {} {}", f(fr.0), q_text(q)),
        EqShape::Gain { gain } => format!("{w} {}", gain_text(gain.0)),
        EqShape::Polarity => "polarity inverted".to_owned(),
        EqShape::Delay { seconds } => format!("{w} {}", format::delay(seconds.0)),
        EqShape::Biquad { rate, .. } => format!("{w} at {}", rate_text(rate.0)),
        EqShape::Response { trace } => format!("{w} of {}", trace_name(trace)),
    }
}

/// A section row: `3 · peaking 1.25 kHz −4.0 dB Q 2.0 (half-gain)`, `· off` when skipped.
pub fn section_text(
    index: usize,
    s: &EqSection,
    peak_q: PeakQ,
    trace_name: &dyn Fn(TraceId) -> String,
) -> String {
    let mut t = format!(
        "{} · {}",
        index + 1,
        shape_text(&s.shape, peak_q, trace_name)
    );
    if !s.enabled {
        t.push_str(" · off");
    }
    t
}

/// One field of the editor's selected section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Shape,
    Freq,
    Gain,
    /// Q or bandwidth (peaking, notch, band-pass, all-pass, high/low-pass Q, shelf slope).
    Q,
    /// How a peaking Q is read.
    QMeasured,
    /// High/low-pass alignment.
    Alignment,
    /// High/low-pass order.
    Order,
    Delay,
}

/// The fields a shape has, in Tab order.
pub fn fields(shape: &EqShape) -> &'static [FieldKind] {
    use FieldKind as F;
    match shape {
        EqShape::Peaking { .. } => &[F::Shape, F::Freq, F::Gain, F::Q, F::QMeasured],
        EqShape::LowShelf { .. } | EqShape::HighShelf { .. } => &[F::Shape, F::Freq, F::Gain, F::Q],
        EqShape::HighPass { .. } | EqShape::LowPass { .. } => {
            &[F::Shape, F::Freq, F::Alignment, F::Order]
        }
        EqShape::HighPassQ { .. }
        | EqShape::LowPassQ { .. }
        | EqShape::Notch { .. }
        | EqShape::BandPass { .. }
        | EqShape::AllPass2 { .. } => &[F::Shape, F::Freq, F::Q],
        EqShape::AllPass1 { .. } => &[F::Shape, F::Freq],
        EqShape::Gain { .. } => &[F::Shape, F::Gain],
        EqShape::Delay { .. } => &[F::Shape, F::Delay],
        EqShape::Polarity | EqShape::Biquad { .. } | EqShape::Response { .. } => &[F::Shape],
    }
}

/// Octaves of bandwidth a Q spans: `n = 2/ln2 · asinh(1/(2Q))`, exact for the s-domain
/// prototype, whose band edges are geometric about f₀.
pub fn octaves_of_q(q: f64) -> f64 {
    2.0 / std::f64::consts::LN_2 * (1.0 / (2.0 * q)).asinh()
}

/// The Q of `n` octaves, the inverse of [`octaves_of_q`].
pub fn q_of_octaves(n: f64) -> f64 {
    1.0 / (2.0 * (std::f64::consts::LN_2 / 2.0 * n).sinh())
}

/// `Q 2.0 · 0.71 oct`: a Q with the bandwidth it spans.
pub fn q_with_octaves(q: f64) -> String {
    let n = octaves_of_q(q);
    format!("{} · {} oct", q_text(q), format::fixed(n, 2))
}

/// A field as the editor shows it: `f 1.25 kHz`, `gain −4.0 dB`, `Q 2.0 · 0.71 oct`.
pub fn field_text(shape: &EqShape, field: FieldKind, peak_q: PeakQ) -> String {
    use FieldKind as F;
    let bw = |b: Bandwidth| match b {
        Bandwidth::Q { value } => q_with_octaves(value),
        Bandwidth::Octaves { n } => format!("{} oct", format::fixed(n, 2)),
    };
    match (field, *shape) {
        (F::Shape, s) => shape_word(&s).to_owned(),
        (
            F::Freq,
            EqShape::Peaking { f, .. }
            | EqShape::LowShelf { f, .. }
            | EqShape::HighShelf { f, .. }
            | EqShape::HighPass { f, .. }
            | EqShape::LowPass { f, .. }
            | EqShape::HighPassQ { f, .. }
            | EqShape::LowPassQ { f, .. }
            | EqShape::Notch { f, .. }
            | EqShape::BandPass { f, .. }
            | EqShape::AllPass1 { f }
            | EqShape::AllPass2 { f, .. },
        ) => format!("f {}", format::freq_readout(f.0)),
        (
            F::Gain,
            EqShape::Peaking { gain, .. }
            | EqShape::LowShelf { gain, .. }
            | EqShape::HighShelf { gain, .. }
            | EqShape::Gain { gain },
        ) => format!("gain {}", gain_text(gain.0)),
        (
            F::Q,
            EqShape::Peaking { bw: b, .. }
            | EqShape::Notch { bw: b, .. }
            | EqShape::BandPass { bw: b, .. },
        ) => bw(b),
        (F::Q, EqShape::LowShelf { order, f_at, .. } | EqShape::HighShelf { order, f_at, .. }) => {
            shelf_text(order, f_at)
        }
        (
            F::Q,
            EqShape::HighPassQ { q, .. }
            | EqShape::LowPassQ { q, .. }
            | EqShape::AllPass2 { q, .. },
        ) => q_text(q),
        (F::QMeasured, EqShape::Peaking { q_measured, .. }) => {
            format!("Q measured {}", peak_q_text(q_measured.unwrap_or(peak_q)))
        }
        (
            F::Alignment,
            EqShape::HighPass { alignment, .. } | EqShape::LowPass { alignment, .. },
        ) => match alignment {
            Alignment::Butterworth => "Butterworth".to_owned(),
            Alignment::LinkwitzRiley => "Linkwitz-Riley".to_owned(),
            Alignment::Bessel { norm } => format!(
                "Bessel ({})",
                match norm {
                    BesselNorm::Phase => "phase",
                    BesselNorm::Delay => "delay",
                    BesselNorm::Minus3db => "−3 dB",
                }
            ),
        },
        (F::Order, EqShape::HighPass { order, .. } | EqShape::LowPass { order, .. }) => {
            format!("order {order}")
        }
        (F::Delay, EqShape::Delay { seconds }) => format::delay(seconds.0),
        _ => format::NO_VALUE.to_owned(),
    }
}

/// The editor's curve row: `[x] capture 2 · change since rev 4`.
pub fn curve_row(name: &str, predicted: bool, set_name: &str, basis: EqBasis) -> String {
    let tick = if predicted { "[x]" } else { "[ ]" };
    let b = match basis {
        EqBasis::Whole => "assumes none in".to_owned(),
        b => basis_tag(set_name, b),
    };
    format!("{tick} {name} · {b}")
}

/// The question M asks before marking.
pub fn mark_question(set: &EqSet, meas_name: &str) -> String {
    let n = set.body.sections.len();
    format!(
        "Mark {} rev {} ({n} section{}) as in the console? Captures of {meas_name} from now on \
         record it; predictions show only later changes. M marks, Shift+M clears the mark, Esc \
         cancels",
        set.body.name,
        set.rev,
        if n == 1 { "" } else { "s" }
    )
}

/// The editor's key line.
pub const EDITOR_KEYS: &str = "↑/↓ rows · Tab fields · ←/→ step (Shift coarser, Alt finer) · \
     type a number · Insert add · Delete remove · B bypass (Shift+B the set) · [ ] move · \
     P predict a curve · M mark in console · Shift+Insert new set · Esc closes";

/// The editor's title.
pub fn editor_title(meas_name: &str) -> String {
    format!("EQ · {meas_name}")
}

/// The parts of a prediction (`curve × EQ`): its measurement, set and curve.
pub fn prediction(expr: &MathExpr) -> Option<(MeasId, EqSetId, &Operand)> {
    match expr {
        MathExpr::Binary {
            a,
            op: MathOp::Multiply,
            b: Operand::Eq { meas, set },
        } => Some((*meas, *set, a)),
        _ => None,
    }
}

/// What a curve of a measurement contains of `set`: the live curve what is marked in the
/// console now, a stored trace what it recorded. `trace` looks a stored trace up.
pub fn curve_basis<'a>(
    set: &EqSet,
    curve: &Operand,
    trace: impl Fn(TraceId) -> Option<&'a TraceMeta>,
) -> EqBasis {
    match curve {
        Operand::Trace { trace: id } => {
            let snap = trace(*id).and_then(|m| m.eq_in.iter().find(|s| s.set == set.id));
            set.basis(snap)
        }
        _ => set.basis(set.in_console.as_ref()),
    }
}

#[cfg(test)]
mod tests;
