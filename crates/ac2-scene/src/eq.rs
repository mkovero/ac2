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
