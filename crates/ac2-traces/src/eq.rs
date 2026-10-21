//! EQ what-if sets on a grid: the wire model mapped onto [`ac2_core::eq`], a set's filter
//! curve and its prediction factor for a curve (`docs/design/eq-overlay.md`). The daemon
//! calls this; the app never evaluates a filter.

use std::fmt;

use ac2_core::eq::{self as core, Pass, Section};
use ac2_proto::model::{
    Alignment, Bandwidth, BesselNorm, EqBasis, EqBody, EqRealisation, EqSet, EqShape, EqSnapshot,
    PeakQ, ShelfFreq, ShelfOrder, ShelfSlope,
};
use ac2_proto::units::TraceId;
use num_complex::Complex64;

use crate::Columns;

/// Why a set cannot be evaluated: the section (0-based) and the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct EqError {
    /// Section index (0-based).
    pub index: usize,
    /// Its shape's word (`peaking`, `high-pass` …).
    pub shape: &'static str,
    /// Why.
    pub reason: String,
}

impl fmt::Display for EqError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} · {}: {}", self.index + 1, self.shape, self.reason)
    }
}

impl std::error::Error for EqError {}

/// The core realisation of a wire one.
pub fn realisation(r: EqRealisation) -> core::Realisation {
    match r {
        EqRealisation::Digital { rate } => core::Realisation::Digital { rate: rate.0 },
        EqRealisation::Analog => core::Realisation::Analog,
    }
}

fn peak_q(p: PeakQ) -> core::PeakQ {
    match p {
        PeakQ::HalfGain => core::PeakQ::HalfGain,
        PeakQ::Minus3dbFromPeak => core::PeakQ::Minus3dbFromPeak,
        PeakQ::BandPass => core::PeakQ::BandPass,
    }
}

fn bw(b: Bandwidth) -> core::Bandwidth {
    match b {
        Bandwidth::Q { value } => core::Bandwidth::Q(value),
        Bandwidth::Octaves { n } => core::Bandwidth::Octaves(n),
    }
}

fn shelf_order(o: ShelfOrder) -> core::ShelfOrder {
    match o {
        ShelfOrder::First => core::ShelfOrder::First,
        ShelfOrder::Second { slope } => core::ShelfOrder::Second(match slope {
            ShelfSlope::Q { value } => core::ShelfSlope::Q(value),
            ShelfSlope::S { value } => core::ShelfSlope::S(value),
            ShelfSlope::Octaves { n } => core::ShelfSlope::Octaves(n),
        }),
    }
}

fn shelf_freq(f: ShelfFreq) -> core::ShelfFreq {
    match f {
        ShelfFreq::Midpoint => core::ShelfFreq::Midpoint,
        ShelfFreq::Corner => core::ShelfFreq::Corner,
    }
}

fn alignment(a: Alignment) -> core::Alignment {
    match a {
        Alignment::Butterworth => core::Alignment::Butterworth,
        Alignment::LinkwitzRiley => core::Alignment::LinkwitzRiley,
        Alignment::Bessel { norm } => core::Alignment::Bessel(match norm {
            BesselNorm::Phase => core::BesselNorm::Phase,
            BesselNorm::Delay => core::BesselNorm::Delay,
            BesselNorm::Minus3db => core::BesselNorm::Minus3db,
        }),
    }
}

/// The core section of a shape (`default_q`: the set's reading of a peaking Q); `None` for
/// a `response` section, which is a stored trace, not a filter.
pub fn section(shape: &EqShape, default_q: PeakQ) -> Option<Section> {
    Some(match *shape {
        EqShape::Peaking {
            f,
            gain,
            bw: b,
            q_measured,
        } => Section::Peaking {
            f: f.0,
            gain_db: gain.0,
            bw: bw(b),
            q_measured: peak_q(q_measured.unwrap_or(default_q)),
        },
        EqShape::LowShelf {
            f,
            gain,
            order,
            f_at,
        } => Section::Shelf {
            pass: Pass::Low,
            f: f.0,
            gain_db: gain.0,
            order: shelf_order(order),
            f_at: shelf_freq(f_at),
        },
        EqShape::HighShelf {
            f,
            gain,
            order,
            f_at,
        } => Section::Shelf {
            pass: Pass::High,
            f: f.0,
            gain_db: gain.0,
            order: shelf_order(order),
            f_at: shelf_freq(f_at),
        },
        EqShape::HighPass {
            f,
            alignment: a,
            order,
        } => Section::Pass {
            pass: Pass::High,
            f: f.0,
            alignment: alignment(a),
            order,
        },
        EqShape::LowPass {
            f,
            alignment: a,
            order,
        } => Section::Pass {
            pass: Pass::Low,
            f: f.0,
            alignment: alignment(a),
            order,
        },
        EqShape::HighPassQ { f, q } => Section::PassQ {
            pass: Pass::High,
            f: f.0,
            q,
        },
        EqShape::LowPassQ { f, q } => Section::PassQ {
            pass: Pass::Low,
            f: f.0,
            q,
        },
        EqShape::Notch { f, bw: b } => Section::Notch { f: f.0, bw: bw(b) },
        EqShape::BandPass { f, bw: b } => Section::BandPass { f: f.0, bw: bw(b) },
        EqShape::AllPass1 { f } => Section::AllPass1 { f: f.0 },
        EqShape::AllPass2 { f, q } => Section::AllPass2 { f: f.0, q },
        EqShape::Gain { gain } => Section::Gain { gain_db: gain.0 },
        EqShape::Polarity => Section::Polarity,
        EqShape::Delay { seconds } => Section::Delay { seconds: seconds.0 },
        EqShape::Biquad {
            b0,
            b1,
            b2,
            a1,
            a2,
            rate,
        } => Section::Biquad {
            b: [b0, b1, b2],
            a: [a1, a2],
            rate: rate.0,
        },
        EqShape::Response { .. } => return None,
    })
}

/// A shape's word, as section rows, refusals and the text formats name it.
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

/// A stored transfer trace's response on the evaluation grid, for `response` sections:
/// `Err` says why the trace cannot be one.
pub type ResponseOf<'a> = dyn Fn(TraceId) -> Result<Vec<Complex64>, String> + 'a;

/// Checks every section of `body` (enabled or not: a refused section refuses the set,
/// whether or not it is switched on).
pub fn validate(body: &EqBody, response_of: &ResponseOf<'_>) -> Result<(), EqError> {
    let real = realisation(body.realisation);
    for (index, s) in body.sections.iter().enumerate() {
        let err = |reason: String| EqError {
            index,
            shape: shape_word(&s.shape),
            reason,
        };
        match (&s.shape, section(&s.shape, body.peak_q)) {
            (EqShape::Response { trace }, _) => {
                response_of(*trace).map_err(err)?;
            }
            (_, Some(c)) => {
                core::design(&c, real).map_err(|r| err(r.to_string()))?;
            }
            (_, None) => {}
        }
    }
    Ok(())
}

/// What `body` filters at `freqs`: the cascade of its enabled sections, all ones when the
/// set is bypassed. A digital realisation leaves columns at or above fs/2 NaN.
pub fn response(
    body: &EqBody,
    freqs: &[f64],
    response_of: &ResponseOf<'_>,
) -> Result<Vec<Complex64>, EqError> {
    validate(body, response_of)?;
    let mut h = vec![Complex64::new(1.0, 0.0); freqs.len()];
    if !body.enabled {
        return Ok(h);
    }
    let real = realisation(body.realisation);
    for (index, s) in body.sections.iter().enumerate() {
        if !s.enabled {
            continue;
        }
        match (&s.shape, section(&s.shape, body.peak_q)) {
            (EqShape::Response { trace }, _) => {
                let r = response_of(*trace).map_err(|reason| EqError {
                    index,
                    shape: "response",
                    reason,
                })?;
                for (o, v) in h.iter_mut().zip(r) {
                    *o *= v;
                }
            }
            (_, Some(c)) => {
                // Validated above.
                if let Ok(d) = core::design(&c, real) {
                    d.apply(real, freqs, &mut h);
                }
            }
            (_, None) => {}
        }
    }
    if let core::Realisation::Digital { rate } = real {
        for (o, &f) in h.iter_mut().zip(freqs) {
            if f >= rate / 2.0 {
                *o = Complex64::new(f64::NAN, f64::NAN);
            }
        }
    }
    Ok(h)
}

/// The factor a curve that recorded `eq_in` of `set` (`None`: none of it) is multiplied by
/// to predict it after the set: the whole set, only its change since the snapshot
/// (`H_now / H_in`), or nothing when it is unchanged. Bypassing a marked set gives
/// `1 / H_in`: taking the EQ out again.
pub fn prediction(
    set: &EqSet,
    eq_in: Option<&EqSnapshot>,
    freqs: &[f64],
    response_of: &ResponseOf<'_>,
) -> Result<(EqBasis, Vec<Complex64>), EqError> {
    let basis = set.basis(eq_in);
    let h = match (basis, eq_in) {
        (EqBasis::NoChange { .. }, _) => vec![Complex64::new(1.0, 0.0); freqs.len()],
        (EqBasis::Change { .. }, Some(s)) => {
            let now = response(&set.body, freqs, response_of)?;
            let then = response(&s.body, freqs, response_of)?;
            now.iter().zip(then).map(|(a, b)| a / b).collect()
        }
        _ => response(&set.body, freqs, response_of)?,
    };
    Ok((basis, h))
}

/// `h` as stored columns: dB, degrees in (−180, 180], coherence 1 (a filter is exact).
/// NaN (a gap) stays NaN.
pub fn columns(h: &[Complex64]) -> Columns {
    let gap = |v: &Complex64| !(v.re.is_finite() && v.im.is_finite());
    Columns {
        mag_db: h
            .iter()
            .map(|v| {
                if gap(v) {
                    f32::NAN
                } else {
                    (20.0 * v.norm().log10()) as f32
                }
            })
            .collect(),
        phase_deg: Some(
            h.iter()
                .map(|v| {
                    if gap(v) {
                        f32::NAN
                    } else {
                        let d = v.arg().to_degrees();
                        (if d <= -180.0 { d + 360.0 } else { d }) as f32
                    }
                })
                .collect(),
        ),
        coherence: Some(
            h.iter()
                .map(|v| if gap(v) { f32::NAN } else { 1.0 })
                .collect(),
        ),
    }
}

/// Columns of a transfer trace as complex values (a magnitude-only trace at zero phase).
pub fn complex(c: &Columns) -> Vec<Complex64> {
    c.mag_db
        .iter()
        .enumerate()
        .map(|(i, &db)| {
            let ph = c
                .phase_deg
                .as_ref()
                .and_then(|p| p.get(i).copied())
                .unwrap_or(0.0);
            if !(db.is_finite() && ph.is_finite()) {
                return Complex64::new(f64::NAN, f64::NAN);
            }
            Complex64::from_polar(10f64.powf(f64::from(db) / 20.0), f64::from(ph).to_radians())
        })
        .collect()
}

#[cfg(test)]
mod tests;
