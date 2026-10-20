//! Equalizer APO / REW filter text (`docs/design/eq-overlay.md` §4): the
//! `Filter: ON PK Fc 1250 Hz Gain -4.0 dB Q 2.00` line grammar Equalizer APO's `config.txt`
//! reads and REW's "export filter settings as text" writes. One reader for both; the writer
//! writes only what the grammar expresses exactly and refuses the rest.
//!
//! The readings were settled against REW 5.40's Generic equaliser (its predicted responses
//! are `tests/fixtures/rew_generic_eq.json`): `PK` is the cookbook peaking EQ with the Q read
//! at half the dB gain; `LP` / `HP` without a Q are the cookbook sections at Q 1/√2; plain
//! `LS` / `HS` are cookbook shelves of slope S = 1 at their midpoint; `LS 6dB` / `HS 6dB` are
//! first-order and `LS 12dB` / `HS 12dB` second-order (Q 1/√2) shelves whose `Fc` is the
//! corner on the plateau side, `tan(π·f_mid/fs) = K^{±1/(2n)}·tan(π·Fc/fs)` (low ×, high ÷,
//! K the linear gain magnitude, n the order); `NO` without a Q is the cookbook notch at Q 30.

use std::f64::consts::{FRAC_1_SQRT_2, PI};
use std::fmt::{self, Write as _};

use ac2_core::eq::{self as core, Pass, peak_cookbook_q, q_from_octaves, shelf_plan};
use ac2_proto::model::{
    Alignment, Bandwidth, EqBody, EqRealisation, EqSection, EqShape, PeakQ, ShelfFreq, ShelfOrder,
    ShelfSlope,
};
use ac2_proto::units::{Db, Hz, Seconds};

use crate::eq::shape_word;

/// The Q of APO's / REW's `NO` line without one (REW 5.40 Generic: fitted 30.00).
pub const NOTCH_Q: f64 = 30.0;

/// Why a text is not read: the line (1-based) and the reason. Nothing is imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApoLineError {
    /// 1-based line number.
    pub line: usize,
    /// Why.
    pub reason: String,
}

impl fmt::Display for ApoLineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.reason)
    }
}

impl std::error::Error for ApoLineError {}

/// A section the writer cannot express exactly: its index (0-based), shape and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRefusal {
    /// Section index (0-based); `None` for the set as a whole.
    pub index: Option<usize>,
    /// The shape's word (`high-pass` …), or `set`.
    pub what: String,
    /// Why.
    pub reason: String,
}

impl fmt::Display for ExportRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.index {
            Some(i) => write!(f, "{} · {}: {}", i + 1, self.what, self.reason),
            None => write!(f, "{}: {}", self.what, self.reason),
        }
    }
}

impl std::error::Error for ExportRefusal {}

/// The shelf's corner (`Fc` of `LS 6dB` …) moved to its midpoint (`to_mid`) or back, for
/// order `n` and gain `gain_db`. Digital sections are prewarped, so the relation holds on
/// `tan(π·f/fs)`; an analog one on f itself. `None` at or above fs/2.
fn shelf_corner(
    pass: Pass,
    f: f64,
    gain_db: f64,
    n: u8,
    real: EqRealisation,
    to_mid: bool,
) -> Option<f64> {
    let k = 10f64
        .powf(gain_db.abs() / 20.0)
        .powf(1.0 / (2.0 * f64::from(n)));
    // The midpoint sits above a low shelf's corner and below a high shelf's.
    let ratio = match (pass, to_mid) {
        (Pass::Low, true) | (Pass::High, false) => k,
        (Pass::High, true) | (Pass::Low, false) => 1.0 / k,
    };
    match real {
        EqRealisation::Analog => Some(f * ratio),
        EqRealisation::Digital { rate } => {
            let fs = rate.0;
            (f < fs / 2.0).then(|| fs / PI * ((PI * f / fs).tan() * ratio).atan())
        }
    }
}

/// One filter line's parameters.
#[derive(Default)]
struct Params {
    fc: Option<f64>,
    gain: Option<f64>,
    q: Option<f64>,
}

fn number(tok: Option<&str>, what: &str) -> Result<f64, String> {
    let t = tok.ok_or_else(|| format!("{what} has no value"))?;
    t.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("{what} value {t:?} is not a number"))
}

fn params(toks: &[&str]) -> Result<Params, String> {
    let mut p = Params::default();
    let mut i = 0;
    let unit = |i: usize, want: &str| toks.get(i).is_some_and(|t| t.eq_ignore_ascii_case(want));
    while i < toks.len() {
        match toks[i] {
            "Fc" => {
                p.fc = Some(number(toks.get(i + 1).copied(), "Fc")?);
                i += 2;
                if unit(i, "Hz") {
                    i += 1;
                }
            }
            "Gain" => {
                p.gain = Some(number(toks.get(i + 1).copied(), "Gain")?);
                i += 2;
                if unit(i, "dB") {
                    i += 1;
                }
            }
            "Q" => {
                p.q = Some(number(toks.get(i + 1).copied(), "Q")?);
                i += 2;
            }
            "BW" => {
                return Err(
                    "`BW Oct` is not read: how APO and REW turn it into a Q at the \
                            device rate is not verified, and it is not guessed"
                        .into(),
                );
            }
            "T60" => return Err("`T60` (modal) is not read".into()),
            t => return Err(format!("unknown parameter {t:?}")),
        }
    }
    Ok(p)
}

fn need(v: Option<f64>, what: &str, ty: &str) -> Result<f64, String> {
    v.ok_or_else(|| format!("{ty} needs {what}"))
}

fn none(v: Option<f64>, what: &str, ty: &str) -> Result<(), String> {
    match v {
        Some(_) => Err(format!("{ty} takes no {what}")),
        None => Ok(()),
    }
}

/// One `Filter` line's type and parameters as a shape (`None`: REW's empty `None` slot).
fn filter(toks: &[&str], real: EqRealisation) -> Result<Option<EqShape>, String> {
    let Some(&ty) = toks.first() else {
        return Err("filter line has no type".into());
    };
    if ty == "None" {
        return Ok(None);
    }
    // `LS 6dB` … are two tokens; `LSC 12 dB` is APO's slope form.
    let (base, slope_db, rest) = match (ty, toks.get(1).copied()) {
        ("LS" | "HS", Some("6dB")) => (ty, 6u8, &toks[2..]),
        ("LS" | "HS", Some("12dB")) => (ty, 12, &toks[2..]),
        _ => (ty, 0, &toks[1..]),
    };
    if matches!(base, "LSC" | "HSC") && rest.first().is_some_and(|t| t.parse::<f64>().is_ok()) {
        return Err(format!(
            "`{base} x dB` (APO's slope form) is not read: its definition is not verified"
        ));
    }
    let p = params(rest)?;
    let name = if slope_db == 0 {
        base.to_string()
    } else {
        format!("{base} {slope_db}dB")
    };
    let name = name.as_str();
    let fc = need(p.fc, "Fc", name)?;
    if fc <= 0.0 {
        return Err(format!("Fc {fc} Hz is not positive"));
    }
    let f = Hz(fc);
    Ok(Some(match (base, slope_db) {
        ("PK" | "PEQ", 0) => EqShape::Peaking {
            f,
            gain: Db(need(p.gain, "Gain", name)?),
            bw: Bandwidth::Q {
                value: need(p.q, "Q", name)?,
            },
            q_measured: Some(PeakQ::HalfGain),
        },
        ("Modal", 0) => return Err("`Modal` is not read (REW's modal Q is not verified)".into()),
        ("LP" | "LPQ" | "HP" | "HPQ", 0) => {
            none(p.gain, "Gain", name)?;
            let q = p.q.unwrap_or(FRAC_1_SQRT_2);
            if name.starts_with('L') {
                EqShape::LowPassQ { f, q }
            } else {
                EqShape::HighPassQ { f, q }
            }
        }
        ("LS" | "LSC" | "HS" | "HSC", 0) => {
            let gain = Db(need(p.gain, "Gain", name)?);
            let order = ShelfOrder::Second {
                slope: match p.q {
                    Some(value) => ShelfSlope::Q { value },
                    None => ShelfSlope::S { value: 1.0 },
                },
            };
            let f_at = ShelfFreq::Midpoint;
            if name.starts_with('L') {
                EqShape::LowShelf {
                    f,
                    gain,
                    order,
                    f_at,
                }
            } else {
                EqShape::HighShelf {
                    f,
                    gain,
                    order,
                    f_at,
                }
            }
        }
        (lh @ ("LS" | "HS"), n) => {
            none(p.q, "Q", name)?;
            let g = need(p.gain, "Gain", name)?;
            let pass = if lh == "LS" { Pass::Low } else { Pass::High };
            let order_n = if n == 6 { 1 } else { 2 };
            let mid = shelf_corner(pass, fc, g, order_n, real, true)
                .ok_or_else(|| format!("Fc {fc} Hz is at or above half the rate"))?;
            let order = if order_n == 1 {
                ShelfOrder::First
            } else {
                ShelfOrder::Second {
                    slope: ShelfSlope::Q {
                        value: FRAC_1_SQRT_2,
                    },
                }
            };
            let (f, gain, f_at) = (Hz(mid), Db(g), ShelfFreq::Midpoint);
            if matches!(pass, Pass::Low) {
                EqShape::LowShelf {
                    f,
                    gain,
                    order,
                    f_at,
                }
            } else {
                EqShape::HighShelf {
                    f,
                    gain,
                    order,
                    f_at,
                }
            }
        }
        ("NO", 0) => {
            none(p.gain, "Gain", name)?;
            EqShape::Notch {
                f,
                bw: Bandwidth::Q {
                    value: p.q.unwrap_or(NOTCH_Q),
                },
            }
        }
        ("BP", 0) => {
            none(p.gain, "Gain", name)?;
            EqShape::BandPass {
                f,
                bw: Bandwidth::Q {
                    value: need(p.q, "Q (its default is not verified)", name)?,
                },
            }
        }
        ("AP", 0) => {
            none(p.gain, "Gain", name)?;
            EqShape::AllPass2 {
                f,
                q: need(p.q, "Q", name)?,
            }
        }
        _ => return Err(format!("unknown filter type {name:?}")),
    }))
}

/// Whether `text` looks like APO / REW filter text (its first meaningful line is a REW
/// header or an APO `Filter` / `Preamp` command).
pub fn is_apo(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .is_some_and(|l| {
            l == "Filter Settings file" || l.starts_with("Filter") || l.starts_with("Preamp:")
        })
}

/// Reads APO / REW filter text as sections, for a set of realisation `real` (the corner of
/// `LS 6dB` … is moved to the midpoint at its rate). `Preamp` lines are summed into one
/// `gain` section first. Anything that selects channels or devices, references other files,
/// or is not verified is refused with its line number, and nothing is imported.
pub fn read(text: &str, real: EqRealisation) -> Result<Vec<EqSection>, ApoLineError> {
    let mut sections = Vec::new();
    let mut preamp: Option<f64> = None;
    // REW's export starts with free-form header lines (title, version, date, notes, the
    // measurement's name) up to its first filter line.
    let mut rew_header = false;
    let mut seen_command = false;
    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        let err = |reason: String| ApoLineError { line, reason };
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if !seen_command && l == "Filter Settings file" {
            rew_header = true;
            continue;
        }
        if let Some(eq) = l.strip_prefix("Equaliser:") {
            let eq = eq.trim();
            if eq != "Generic" {
                return Err(err(format!(
                    "REW equaliser {eq:?} is not read: its Q conventions are not verified \
                     (export with the Generic equaliser)"
                )));
            }
            continue;
        }
        let (cmd, rest) = match l.split_once(':') {
            Some((c, r)) => (c.trim(), r.trim()),
            None => (l, ""),
        };
        let filter_cmd = cmd == "Filter"
            || cmd
                .strip_prefix("Filter")
                .is_some_and(|n| n.trim().parse::<u32>().is_ok());
        if rew_header && !seen_command && !filter_cmd && cmd != "Preamp" && cmd != "Delay" {
            continue;
        }
        seen_command = true;
        if filter_cmd {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            let enabled = match toks.first().copied() {
                Some("ON") => true,
                Some("OFF") => false,
                _ => return Err(err("a filter line starts with ON or OFF".into())),
            };
            if let Some(shape) = filter(&toks[1..], real).map_err(err)? {
                sections.push(EqSection { enabled, shape });
            }
            continue;
        }
        match cmd {
            "Preamp" => {
                let toks: Vec<&str> = rest.split_whitespace().collect();
                let g = number(toks.first().copied(), "Preamp").map_err(err)?;
                if toks.get(1).is_some_and(|u| !u.eq_ignore_ascii_case("dB")) || toks.len() > 2 {
                    return Err(err("Preamp is `Preamp: g dB`".into()));
                }
                *preamp.get_or_insert(0.0) += g;
            }
            "Delay" => {
                let toks: Vec<&str> = rest.split_whitespace().collect();
                let t = number(toks.first().copied(), "Delay").map_err(err)?;
                match toks.get(1).copied() {
                    Some("ms") if toks.len() == 2 && t >= 0.0 => sections.push(EqSection {
                        enabled: true,
                        shape: EqShape::Delay {
                            seconds: Seconds(t / 1000.0),
                        },
                    }),
                    Some("samples") => {
                        return Err(err(
                            "a delay in samples is not read: APO counts them at the device \
                             rate, which the file does not name"
                                .into(),
                        ));
                    }
                    _ => return Err(err("Delay is `Delay: t ms` (t ≥ 0)".into())),
                }
            }
            c => {
                return Err(err(format!(
                    "{c:?} is not read: it selects channels or devices or references other \
                     files, so the list would not be the filter one output sees"
                )));
            }
        }
    }
    if let Some(g) = preamp {
        sections.insert(
            0,
            EqSection {
                enabled: true,
                shape: EqShape::Gain { gain: Db(g) },
            },
        );
    }
    Ok(sections)
}

/// Butterworth section Qs of even order `n`: `1/(2·cos θ)`, θ the pole angles.
fn butterworth_qs(n: u8) -> Vec<f64> {
    (1..=n / 2)
        .map(|k| 1.0 / (2.0 * (PI * f64::from(2 * k - 1) / (2.0 * f64::from(n))).cos()))
        .collect()
}

/// One section as APO lines (the text after `Filter: ON `), or why it is not expressible.
fn lines_of(shape: &EqShape, body: &EqBody) -> Result<Vec<String>, String> {
    let refuse = |r: &str| Err(r.to_string());
    let real = body.realisation;
    Ok(match *shape {
        EqShape::Peaking {
            f,
            gain,
            bw,
            q_measured,
        } => {
            let q = match bw {
                Bandwidth::Q { value } => value,
                Bandwidth::Octaves { n } => q_from_octaves(n),
            };
            let measured = match q_measured.unwrap_or(body.peak_q) {
                PeakQ::HalfGain => core::PeakQ::HalfGain,
                PeakQ::Minus3dbFromPeak => core::PeakQ::Minus3dbFromPeak,
                PeakQ::BandPass => core::PeakQ::BandPass,
            };
            let Some(q) = peak_cookbook_q(q, gain.0, measured).map_err(|r| r.to_string())? else {
                return refuse("a band-pass reading of Q is another filter family than PK");
            };
            vec![format!("PK Fc {} Hz Gain {} dB Q {q}", f.0, gain.0)]
        }
        EqShape::HighPass {
            f,
            alignment,
            order,
        }
        | EqShape::LowPass {
            f,
            alignment,
            order,
        } => {
            let ty = if matches!(shape, EqShape::HighPass { .. }) {
                "HPQ"
            } else {
                "LPQ"
            };
            let qs = match alignment {
                Alignment::Butterworth if order % 2 == 0 && order > 0 => butterworth_qs(order),
                Alignment::LinkwitzRiley if order % 4 == 0 && order > 0 => {
                    let half = butterworth_qs(order / 2);
                    half.iter().chain(&half).copied().collect()
                }
                Alignment::Bessel { .. } => {
                    return refuse(
                        "Bessel sections sit at different frequencies, so per-line prewarping \
                         differs",
                    );
                }
                _ => return refuse("an odd-order section has no APO line"),
            };
            qs.iter()
                .map(|q| format!("{ty} Fc {} Hz Q {q}", f.0))
                .collect()
        }
        EqShape::HighPassQ { f, q } => vec![format!("HPQ Fc {} Hz Q {q}", f.0)],
        EqShape::LowPassQ { f, q } => vec![format!("LPQ Fc {} Hz Q {q}", f.0)],
        EqShape::LowShelf { f, gain, .. } | EqShape::HighShelf { f, gain, .. } => {
            let (pass, lh) = if matches!(shape, EqShape::LowShelf { .. }) {
                (Pass::Low, "L")
            } else {
                (Pass::High, "H")
            };
            let c = crate::eq::section(shape, body.peak_q).ok_or("not a filter")?;
            let core::Section::Shelf {
                order: co,
                f_at: cf,
                ..
            } = c
            else {
                return refuse("not a shelf");
            };
            let plan = shelf_plan(pass, f.0, gain.0, co, cf).map_err(|r| r.to_string())?;
            match plan.q {
                Some(q) => vec![format!(
                    "{lh}SC Fc {} Hz Gain {} dB Q {q}",
                    plan.midpoint, gain.0
                )],
                None => {
                    let fc = shelf_corner(pass, plan.midpoint, gain.0, 1, real, false)
                        .ok_or("its midpoint is at or above half the rate")?;
                    vec![format!("{lh}S 6dB Fc {fc} Hz Gain {} dB", gain.0)]
                }
            }
        }
        EqShape::Notch { f, bw } => vec![format!("NO Fc {} Hz Q {}", f.0, bw_q(bw))],
        EqShape::BandPass { f, bw } => vec![format!("BP Fc {} Hz Q {}", f.0, bw_q(bw))],
        EqShape::AllPass2 { f, q } => vec![format!("AP Fc {} Hz Q {q}", f.0)],
        EqShape::AllPass1 { .. } => return refuse("APO has no first-order all-pass line"),
        EqShape::Polarity => return refuse("polarity is a channel operation in APO"),
        EqShape::Biquad { .. } => return refuse("raw coefficients have no APO / REW line"),
        EqShape::Response { .. } => return refuse("a stored response is not a filter line"),
        EqShape::Gain { .. } | EqShape::Delay { .. } => Vec::new(),
    })
}

fn bw_q(bw: Bandwidth) -> f64 {
    match bw {
        Bandwidth::Q { value } => value,
        Bandwidth::Octaves { n } => q_from_octaves(n),
    }
}

/// Writes `body` as APO filter text. Every section that the grammar cannot express exactly
/// is refused (all of them, named); with `skip_unsupported` the rest is written and each
/// skipped section listed as a `#` comment at the top. Gains are summed into one `Preamp`,
/// disabled filters written `OFF`; the set's bypass and in-console mark are not filters and
/// are not written.
pub fn write(body: &EqBody, skip_unsupported: bool) -> Result<String, Vec<ExportRefusal>> {
    if body.realisation == EqRealisation::Analog {
        return Err(vec![ExportRefusal {
            index: None,
            what: "set".into(),
            reason: "an analog set has no APO form (APO filters are digital)".into(),
        }]);
    }
    let mut refused = Vec::new();
    let mut filters = Vec::new();
    let mut preamp: Option<f64> = None;
    let mut delays = Vec::new();
    for (i, s) in body.sections.iter().enumerate() {
        match s.shape {
            EqShape::Gain { gain } if s.enabled => *preamp.get_or_insert(0.0) += gain.0,
            EqShape::Delay { seconds } if s.enabled => delays.push(seconds.0 * 1000.0),
            _ => {}
        }
        match lines_of(&s.shape, body) {
            Ok(ls) => {
                let on = if s.enabled { "ON" } else { "OFF" };
                filters.extend(ls.into_iter().map(|l| format!("Filter: {on} {l}")));
            }
            Err(reason) => refused.push(ExportRefusal {
                index: Some(i),
                what: shape_word(&s.shape).to_string(),
                reason,
            }),
        }
    }
    if !refused.is_empty() && !skip_unsupported {
        return Err(refused);
    }
    let mut out = String::new();
    let _ = writeln!(out, "# {} · ac2 EQ", body.name);
    if let EqRealisation::Digital { rate } = body.realisation {
        let _ = writeln!(
            out,
            "# designed at {} Hz; APO runs at the device's rate",
            rate.0
        );
    }
    for r in &refused {
        let _ = writeln!(out, "# skipped {r}");
    }
    if let Some(g) = preamp {
        let _ = writeln!(out, "Preamp: {g} dB");
    }
    for d in delays {
        let _ = writeln!(out, "Delay: {d} ms");
    }
    for f in filters {
        out.push_str(&f);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
