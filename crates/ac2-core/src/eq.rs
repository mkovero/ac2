//! Equaliser sections: the complex response of the filters a console or processor applies,
//! evaluated exactly at a list of frequencies (no FFT), for the EQ what-if overlay
//! (`docs/design/eq-overlay.md`).
//!
//! Every shape is an s-domain prototype normalised to its own f₀ (a cascade of second- and
//! first-order sections). [`Realisation::Analog`] evaluates it at `s = j·f/f₀`;
//! [`Realisation::Digital`] maps it by the bilinear transform prewarped at f₀, which on the
//! unit circle is exactly `s = j·tan(πf/fs)/tan(πf₀/fs)` — the same `H(e^{j2πf/fs})` a biquad
//! of the cookbook coefficients gives, so the cramping near Nyquist is the console's.

use num_complex::Complex64;
use std::f64::consts::{LN_2, PI};
use std::fmt;

mod proto;
#[cfg(test)]
mod tests;

pub use proto::bessel_poles;

/// Largest boost or cut a gain-bearing section takes, dB.
pub const MAX_GAIN_DB: f64 = 30.0;
/// Largest Q (and so narrowest bandwidth) a section takes.
pub const MAX_Q: f64 = 100.0;
/// Highest order of a high- or low-pass (48 dB/oct).
pub const MAX_ORDER: u8 = 8;
/// Longest delay section, s.
pub const MAX_DELAY_S: f64 = 1.0;
/// How far from flat a shelf's corner frequency is read, dB.
pub const CORNER_DB: f64 = 3.0;

/// How a set's sections are realised.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Realisation {
    /// Bilinear transform prewarped at each section's f₀, at the processor's sample rate.
    Digital {
        /// Processor sample rate, Hz.
        rate: f64,
    },
    /// The s-domain prototype itself.
    Analog,
}

/// What a peaking section's entered Q measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeakQ {
    /// Symmetric (cookbook) peaking EQ: f₀ over the bandwidth between the half-gain points.
    HalfGain,
    /// The same filter, Q read between the points 3 dB below the peak (above the dip).
    Minus3dbFromPeak,
    /// Band-pass-sum family `1 + (K−1)·BP(s)`: the added band-pass's own Q.
    BandPass,
}

/// A free bandwidth, as Q or in octaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bandwidth {
    /// Quality factor.
    Q(f64),
    /// Bandwidth in octaves.
    Octaves(f64),
}

impl Bandwidth {
    /// The Q this bandwidth is (the analog relation for octaves).
    pub fn q(self) -> f64 {
        match self {
            Bandwidth::Q(q) => q,
            Bandwidth::Octaves(n) => q_from_octaves(n),
        }
    }
}

/// A second-order shelf's slope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShelfSlope {
    /// Quality factor.
    Q(f64),
    /// Cookbook shelf slope S (1: the steepest without overshoot).
    S(f64),
    /// Bandwidth in octaves, read as a Q by the analog relation.
    Octaves(f64),
}

/// A shelf's order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShelfOrder {
    /// Pole and zero a factor K apart (6 dB/oct transition).
    First,
    /// The cookbook shelf.
    Second(ShelfSlope),
}

/// Which frequency of a shelf `f` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShelfFreq {
    /// Where the gain is half the dB gain (cookbook).
    Midpoint,
    /// The point [`CORNER_DB`] from flat on the flat side.
    Corner,
}

/// Normalisation of a Bessel high- or low-pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BesselNorm {
    /// Asymptotic phase matches the Butterworth's of the same order.
    Phase,
    /// Unit group delay at DC of the low-pass prototype: f is 1/(2πτ).
    Delay,
    /// −3 dB at f.
    Minus3db,
}

/// A high- or low-pass alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    /// Maximally flat, −3 dB at f.
    Butterworth,
    /// Square of the Butterworth of half the order (even orders), −6 dB at f.
    LinkwitzRiley,
    /// Maximally flat group delay.
    Bessel(BesselNorm),
}

/// Pass direction of a high- / low-pass or shelf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Low-pass / low shelf.
    Low,
    /// High-pass / high shelf.
    High,
}

/// One equaliser section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Section {
    /// Peaking (bell).
    Peaking {
        /// Centre, Hz.
        f: f64,
        /// Boost (+) or cut (−), dB.
        gain_db: f64,
        /// Bandwidth as entered.
        bw: Bandwidth,
        /// What the entered Q measures.
        q_measured: PeakQ,
    },
    /// Low or high shelf.
    Shelf {
        /// Which side moves.
        pass: Pass,
        /// Frequency named by `f_at`, Hz.
        f: f64,
        /// Shelf gain, dB.
        gain_db: f64,
        /// Order and slope.
        order: ShelfOrder,
        /// Which frequency `f` is.
        f_at: ShelfFreq,
    },
    /// High- or low-pass of an alignment and order.
    Pass {
        /// Direction.
        pass: Pass,
        /// Corner, Hz (meaning per alignment).
        f: f64,
        /// Alignment.
        alignment: Alignment,
        /// Order, 1 … [`MAX_ORDER`] (6 dB/oct per order).
        order: u8,
    },
    /// One free second-order high- or low-pass section.
    PassQ {
        /// Direction.
        pass: Pass,
        /// Natural frequency, Hz.
        f: f64,
        /// Quality factor.
        q: f64,
    },
    /// Notch; the bandwidth is the notch's −3 dB bandwidth.
    Notch {
        /// Centre, Hz.
        f: f64,
        /// Bandwidth.
        bw: Bandwidth,
    },
    /// Band-pass with a 0 dB peak.
    BandPass {
        /// Centre, Hz.
        f: f64,
        /// Bandwidth.
        bw: Bandwidth,
    },
    /// First-order all-pass, −90° at f.
    AllPass1 {
        /// 90° point, Hz.
        f: f64,
    },
    /// Second-order all-pass, −180° at f.
    AllPass2 {
        /// 180° point, Hz.
        f: f64,
        /// Quality factor.
        q: f64,
    },
    /// Broadband gain.
    Gain {
        /// Gain, dB.
        gain_db: f64,
    },
    /// Polarity inversion (×−1).
    Polarity,
    /// Pure delay `e^{−j2πfτ}`, exact (not rounded to samples).
    Delay {
        /// Delay, s.
        seconds: f64,
    },
    /// A raw biquad, always digital at its own rate (`a0 = 1`).
    Biquad {
        /// Numerator `b0 b1 b2`.
        b: [f64; 3],
        /// Denominator `a1 a2`.
        a: [f64; 2],
        /// Its sample rate, Hz.
        rate: f64,
    },
}

/// Why a section is refused.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// f not a positive frequency.
    FreqNotPositive,
    /// f at or above the processor's Nyquist frequency.
    FreqAboveNyquist {
        /// The frequency, Hz.
        f: f64,
        /// fs/2, Hz.
        nyquist: f64,
    },
    /// Q outside (0, [`MAX_Q`]].
    QRange {
        /// The Q.
        q: f64,
    },
    /// Octaves not positive.
    OctavesNotPositive,
    /// Gain beyond ±[`MAX_GAIN_DB`].
    GainRange {
        /// The gain, dB.
        gain_db: f64,
    },
    /// A Q read 3 dB from the peak needs more than 3 dB of gain.
    PeakTooSmallFor3db,
    /// A shelf corner needs more than [`CORNER_DB`] of gain.
    ShelfTooSmallForCorner,
    /// The shelf slope S is beyond what the gain allows.
    ShelfSlopeRange {
        /// The slope.
        s: f64,
    },
    /// Order not available for the alignment.
    Order {
        /// The order.
        order: u8,
        /// Alignment.
        alignment: Alignment,
    },
    /// A delay outside [0, [`MAX_DELAY_S`]].
    DelayRange,
    /// Sample rate not a positive finite number.
    RateRange,
    /// A biquad coefficient not finite.
    CoefficientNotFinite,
    /// A biquad pole on or outside the unit circle.
    Unstable,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::FreqNotPositive => write!(f, "frequency must be positive"),
            Refusal::FreqAboveNyquist { f: hz, nyquist } => write!(
                f,
                "{} must be below fs/2 = {}",
                hz_text(*hz),
                hz_text(*nyquist)
            ),
            Refusal::QRange { q } => write!(f, "Q {q} outside 0 … {MAX_Q}"),
            Refusal::OctavesNotPositive => write!(f, "bandwidth in octaves must be positive"),
            Refusal::GainRange { gain_db } => {
                write!(f, "gain {gain_db:+.1} dB beyond ±{MAX_GAIN_DB} dB")
            }
            Refusal::PeakTooSmallFor3db => write!(
                f,
                "Q read 3 dB from the peak needs more than 3.01 dB of gain"
            ),
            Refusal::ShelfTooSmallForCorner => {
                write!(f, "a shelf of {CORNER_DB} dB or less has no corner")
            }
            Refusal::ShelfSlopeRange { s } => {
                write!(f, "shelf slope S {s} too steep for this gain")
            }
            Refusal::Order { order, alignment } => match alignment {
                Alignment::LinkwitzRiley => {
                    write!(f, "Linkwitz-Riley order {order}: even orders 2 … 8 only")
                }
                _ => write!(f, "order {order} outside 1 … {MAX_ORDER}"),
            },
            Refusal::DelayRange => write!(f, "delay outside 0 … {MAX_DELAY_S} s"),
            Refusal::RateRange => write!(f, "sample rate must be positive"),
            Refusal::CoefficientNotFinite => write!(f, "biquad coefficient not finite"),
            Refusal::Unstable => write!(f, "biquad pole on or outside the unit circle"),
        }
    }
}

impl std::error::Error for Refusal {}

fn hz_text(hz: f64) -> String {
    if hz >= 1000.0 {
        format!("{} kHz", trim(hz / 1000.0))
    } else {
        format!("{} Hz", trim(hz))
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `(b0 s² + b1 s + b2) / (a0 s² + a1 s + a2)` in s normalised to the section's f₀.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sos {
    pub b: [f64; 3],
    pub a: [f64; 3],
}

impl Sos {
    fn at(&self, s: Complex64) -> Complex64 {
        let s2 = s * s;
        (s2 * self.b[0] + s * self.b[1] + self.b[2]) / (s2 * self.a[0] + s * self.a[1] + self.a[2])
    }
}

/// A section designed for one realisation, ready to evaluate.
#[derive(Debug, Clone, PartialEq)]
pub struct Designed {
    stage: Stage,
}

#[derive(Debug, Clone, PartialEq)]
enum Stage {
    /// Prototype normalised at `f0`, mapped by the set's realisation.
    Proto { f0: f64, sos: Vec<Sos> },
    /// Frequency-independent factor.
    Const(f64),
    /// Pure delay, s.
    Delay(f64),
    /// Raw biquad at its own rate.
    Biquad { b: [f64; 3], a: [f64; 2], rate: f64 },
}

fn check_f(f: f64, real: Realisation) -> Result<(), Refusal> {
    if !(f.is_finite() && f > 0.0) {
        return Err(Refusal::FreqNotPositive);
    }
    if let Realisation::Digital { rate } = real
        && f >= rate / 2.0
    {
        return Err(Refusal::FreqAboveNyquist {
            f,
            nyquist: rate / 2.0,
        });
    }
    Ok(())
}

fn check_q(q: f64) -> Result<f64, Refusal> {
    if q.is_finite() && q > 0.0 && q <= MAX_Q {
        Ok(q)
    } else {
        Err(Refusal::QRange { q })
    }
}

fn check_bw(bw: Bandwidth) -> Result<f64, Refusal> {
    if let Bandwidth::Octaves(n) = bw
        && !(n.is_finite() && n > 0.0)
    {
        return Err(Refusal::OctavesNotPositive);
    }
    check_q(bw.q())
}

fn check_gain(g: f64) -> Result<f64, Refusal> {
    if g.is_finite() && g.abs() <= MAX_GAIN_DB {
        Ok(g)
    } else {
        Err(Refusal::GainRange { gain_db: g })
    }
}

/// Q of a bandwidth of `n` octaves: `1/Q = 2·sinh(ln2/2 · n)` (exact for the s-domain
/// prototype, whose band edges are geometric about f₀).
pub fn q_from_octaves(n: f64) -> f64 {
    1.0 / (2.0 * (LN_2 / 2.0 * n).sinh())
}

/// Octaves of bandwidth of a Q, the inverse of [`q_from_octaves`].
pub fn octaves_from_q(q: f64) -> f64 {
    2.0 / LN_2 * (1.0 / (2.0 * q)).asinh()
}

/// The cookbook shelf's Q of slope `s` at `gain_db`: `1/Q = √((A + 1/A)(1/S − 1) + 2)`.
/// `None` when the slope is beyond what the gain allows (a negative radicand).
pub fn shelf_q_from_s(s: f64, gain_db: f64) -> Option<f64> {
    let a = 10f64.powf(gain_db / 40.0);
    let r = (a + 1.0 / a) * (1.0 / s - 1.0) + 2.0;
    (s.is_finite() && s > 0.0 && r > 0.0).then(|| 1.0 / r.sqrt())
}

/// The cookbook slope S of a shelf Q at `gain_db`, the inverse of [`shelf_q_from_s`].
pub fn shelf_s_from_q(q: f64, gain_db: f64) -> f64 {
    let a = 10f64.powf(gain_db / 40.0);
    1.0 / ((1.0 / (q * q) - 2.0) / (a + 1.0 / a) + 1.0)
}

/// The symmetric (cookbook) Q of a peaking section whose Q is read `measured`, or `None`
/// for the band-pass family (another filter, no cookbook Q describes it).
///
/// A Q read 3 dB below the peak maps by `Q = Q₃·A/√(A⁴ − 2)`, `A = 10^{|g|/40}`: solving
/// `|H|² = A⁴/2` for the symmetric filter gives a band of `A/(Q·√(A⁴ − 2))` (ω₁ω₂ = 1).
pub fn peak_cookbook_q(q: f64, gain_db: f64, measured: PeakQ) -> Result<Option<f64>, Refusal> {
    match measured {
        PeakQ::HalfGain => Ok(Some(q)),
        PeakQ::Minus3dbFromPeak => {
            let a = 10f64.powf(gain_db.abs() / 40.0);
            let a4 = a.powi(4);
            if a4 <= 2.0 {
                return Err(Refusal::PeakTooSmallFor3db);
            }
            Ok(Some(q * a / (a4 - 2.0).sqrt()))
        }
        PeakQ::BandPass => Ok(None),
    }
}

/// The Q read 3 dB from the peak of a symmetric section of cookbook Q `q`; the inverse of
/// [`peak_cookbook_q`] for [`PeakQ::Minus3dbFromPeak`].
pub fn peak_minus3db_q(q: f64, gain_db: f64) -> Result<f64, Refusal> {
    let a = 10f64.powf(gain_db.abs() / 40.0);
    let a4 = a.powi(4);
    if a4 <= 2.0 {
        return Err(Refusal::PeakTooSmallFor3db);
    }
    Ok(q * (a4 - 2.0).sqrt() / a)
}

/// Designs one section for a realisation, or says why it is refused.
pub fn design(section: &Section, real: Realisation) -> Result<Designed, Refusal> {
    if let Realisation::Digital { rate } = real
        && !(rate.is_finite() && rate > 0.0)
    {
        return Err(Refusal::RateRange);
    }
    let stage = match *section {
        Section::Peaking {
            f,
            gain_db,
            bw,
            q_measured,
        } => {
            check_f(f, real)?;
            let g = check_gain(gain_db)?;
            let q = check_bw(bw)?;
            let sos = match peak_cookbook_q(q, g, q_measured)? {
                Some(qc) => {
                    let qc = check_q(qc)?;
                    proto::peaking_symmetric(g, qc)
                }
                None => proto::peaking_band_pass(g, q),
            };
            Stage::Proto {
                f0: f,
                sos: vec![sos],
            }
        }
        Section::Shelf {
            pass,
            f,
            gain_db,
            order,
            f_at,
        } => {
            check_f(f, real)?;
            let g = check_gain(gain_db)?;
            let sos = match order {
                ShelfOrder::First => proto::shelf_first(pass, g),
                ShelfOrder::Second(slope) => {
                    let q = match slope {
                        ShelfSlope::Q(q) => q,
                        ShelfSlope::Octaves(n) => {
                            if !(n.is_finite() && n > 0.0) {
                                return Err(Refusal::OctavesNotPositive);
                            }
                            q_from_octaves(n)
                        }
                        ShelfSlope::S(s) => {
                            shelf_q_from_s(s, g).ok_or(Refusal::ShelfSlopeRange { s })?
                        }
                    };
                    proto::shelf_second(pass, g, check_q(q)?)
                }
            };
            let f0 = match f_at {
                ShelfFreq::Midpoint => f,
                ShelfFreq::Corner => {
                    if g.abs() <= CORNER_DB {
                        return Err(Refusal::ShelfTooSmallForCorner);
                    }
                    f / proto::corner_ratio(pass, g, &sos)
                }
            };
            check_f(f0, real)?;
            Stage::Proto { f0, sos: vec![sos] }
        }
        Section::Pass {
            pass,
            f,
            alignment,
            order,
        } => {
            check_f(f, real)?;
            let ok = match alignment {
                Alignment::LinkwitzRiley => matches!(order, 2 | 4 | 6 | 8),
                _ => (1..=MAX_ORDER).contains(&order),
            };
            if !ok {
                return Err(Refusal::Order { order, alignment });
            }
            Stage::Proto {
                f0: f,
                sos: proto::pass(pass, alignment, order),
            }
        }
        Section::PassQ { pass, f, q } => {
            check_f(f, real)?;
            let q = check_q(q)?;
            let sos = match pass {
                Pass::Low => Sos {
                    b: [0.0, 0.0, 1.0],
                    a: [1.0, 1.0 / q, 1.0],
                },
                Pass::High => Sos {
                    b: [1.0, 0.0, 0.0],
                    a: [1.0, 1.0 / q, 1.0],
                },
            };
            Stage::Proto {
                f0: f,
                sos: vec![sos],
            }
        }
        Section::Notch { f, bw } => {
            check_f(f, real)?;
            let q = check_bw(bw)?;
            Stage::Proto {
                f0: f,
                sos: vec![Sos {
                    b: [1.0, 0.0, 1.0],
                    a: [1.0, 1.0 / q, 1.0],
                }],
            }
        }
        Section::BandPass { f, bw } => {
            check_f(f, real)?;
            let q = check_bw(bw)?;
            Stage::Proto {
                f0: f,
                sos: vec![Sos {
                    b: [0.0, 1.0 / q, 0.0],
                    a: [1.0, 1.0 / q, 1.0],
                }],
            }
        }
        Section::AllPass1 { f } => {
            check_f(f, real)?;
            Stage::Proto {
                f0: f,
                sos: vec![Sos {
                    b: [0.0, -1.0, 1.0],
                    a: [0.0, 1.0, 1.0],
                }],
            }
        }
        Section::AllPass2 { f, q } => {
            check_f(f, real)?;
            let q = check_q(q)?;
            Stage::Proto {
                f0: f,
                sos: vec![Sos {
                    b: [1.0, -1.0 / q, 1.0],
                    a: [1.0, 1.0 / q, 1.0],
                }],
            }
        }
        Section::Gain { gain_db } => Stage::Const(10f64.powf(check_gain(gain_db)? / 20.0)),
        Section::Polarity => Stage::Const(-1.0),
        Section::Delay { seconds } => {
            if !(seconds.is_finite() && (0.0..=MAX_DELAY_S).contains(&seconds)) {
                return Err(Refusal::DelayRange);
            }
            Stage::Delay(seconds)
        }
        Section::Biquad { b, a, rate } => {
            if !(rate.is_finite() && rate > 0.0) {
                return Err(Refusal::RateRange);
            }
            if !b.iter().chain(a.iter()).all(|c| c.is_finite()) {
                return Err(Refusal::CoefficientNotFinite);
            }
            // Poles of z² + a1·z + a2 strictly inside the unit circle (the stability
            // triangle): |a2| < 1 and |a1| < 1 + a2.
            if !(a[1].abs() < 1.0 && a[0].abs() < 1.0 + a[1]) {
                return Err(Refusal::Unstable);
            }
            Stage::Biquad { b, a, rate }
        }
    };
    Ok(Designed { stage })
}

impl Designed {
    /// Multiplies `out[i]` by this section's response at `freqs[i]`; NaN where the
    /// realisation has no value (at or above a digital rate's fs/2).
    pub fn apply(&self, real: Realisation, freqs: &[f64], out: &mut [Complex64]) {
        let nan = Complex64::new(f64::NAN, f64::NAN);
        match &self.stage {
            Stage::Proto { f0, sos } => {
                let warp = match real {
                    Realisation::Digital { rate } => Some((rate, (PI * f0 / rate).tan())),
                    Realisation::Analog => None,
                };
                for (o, &f) in out.iter_mut().zip(freqs) {
                    let w = match warp {
                        Some((rate, t0)) => {
                            if f >= rate / 2.0 {
                                *o = nan;
                                continue;
                            }
                            (PI * f / rate).tan() / t0
                        }
                        None => f / f0,
                    };
                    let s = Complex64::new(0.0, w);
                    *o *= sos
                        .iter()
                        .fold(Complex64::new(1.0, 0.0), |h, q| h * q.at(s));
                }
            }
            Stage::Const(k) => out.iter_mut().for_each(|o| *o *= *k),
            Stage::Delay(t) => {
                for (o, &f) in out.iter_mut().zip(freqs) {
                    *o *= Complex64::from_polar(1.0, -2.0 * PI * f * t);
                }
            }
            Stage::Biquad { b, a, rate } => {
                for (o, &f) in out.iter_mut().zip(freqs) {
                    if f >= rate / 2.0 {
                        *o = nan;
                        continue;
                    }
                    let z1 = Complex64::from_polar(1.0, -2.0 * PI * f / rate);
                    let z2 = z1 * z1;
                    *o *= (z1 * b[1] + z2 * b[2] + b[0]) / (z1 * a[0] + z2 * a[1] + 1.0);
                }
            }
        }
    }
}

/// A section refused by its index in the list.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionError {
    /// Index of the section (0-based).
    pub index: usize,
    /// Why.
    pub refusal: Refusal,
}

impl fmt::Display for SectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "section {}: {}", self.index + 1, self.refusal)
    }
}

impl std::error::Error for SectionError {}

/// The cascade of `sections` at `freqs`. Every section is designed (and so validated) first;
/// a refused one refuses the whole list. A digital realisation leaves every column at or
/// above fs/2 NaN, whatever the sections: the processor has no output there.
pub fn response(
    sections: &[Section],
    real: Realisation,
    freqs: &[f64],
) -> Result<Vec<Complex64>, SectionError> {
    let designed = sections
        .iter()
        .enumerate()
        .map(|(index, s)| design(s, real).map_err(|refusal| SectionError { index, refusal }))
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = vec![Complex64::new(1.0, 0.0); freqs.len()];
    for d in &designed {
        d.apply(real, freqs, &mut out);
    }
    if let Realisation::Digital { rate } = real {
        for (o, &f) in out.iter_mut().zip(freqs) {
            if f >= rate / 2.0 {
                *o = Complex64::new(f64::NAN, f64::NAN);
            }
        }
    }
    Ok(out)
}
