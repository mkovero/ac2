//! The ac2 EQ text (`docs/design/eq-overlay.md` §4): one set, hand-editable, one section per
//! line, keys the wire model's field names. It carries the whole model, so a set written and
//! read back is the same set.
//!
//! ```text
//! # ac2 eq 1
//! name = EQ 1
//! realisation = digital 96000
//! peaking   f=1250 gain=-4 q=2 q_measured=half_gain
//! high_pass f=80 alignment=linkwitz_riley order=4
//! low_shelf f=120 gain=3 s=1 f_at=midpoint   off
//! ```

use std::fmt::{self, Write as _};

use ac2_proto::model::{
    Alignment, Bandwidth, BesselNorm, EqBody, EqRealisation, EqSection, EqShape, PeakQ, ShelfFreq,
    ShelfOrder, ShelfSlope,
};
use ac2_proto::units::{Db, Hz, Seconds, TraceId};

/// The first line of every ac2 EQ text.
pub const HEADER: &str = "# ac2 eq 1";

/// Why a text is not an ac2 EQ set: the line (1-based) and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EqTextError {
    /// 1-based line number.
    pub line: usize,
    /// Why.
    pub reason: String,
}

impl fmt::Display for EqTextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.reason)
    }
}

impl std::error::Error for EqTextError {}

/// Whether `text` starts as an ac2 EQ text (its first non-blank line is [`HEADER`]).
pub fn is_ac2_eq(text: &str) -> bool {
    text.lines().find(|l| !l.trim().is_empty()).map(str::trim) == Some(HEADER)
}

fn peak_q_word(p: PeakQ) -> &'static str {
    match p {
        PeakQ::HalfGain => "half_gain",
        PeakQ::Minus3dbFromPeak => "minus_3db_from_peak",
        PeakQ::BandPass => "band_pass",
    }
}

fn shape_name(s: &EqShape) -> &'static str {
    match s {
        EqShape::Peaking { .. } => "peaking",
        EqShape::LowShelf { .. } => "low_shelf",
        EqShape::HighShelf { .. } => "high_shelf",
        EqShape::HighPass { .. } => "high_pass",
        EqShape::LowPass { .. } => "low_pass",
        EqShape::HighPassQ { .. } => "high_pass_q",
        EqShape::LowPassQ { .. } => "low_pass_q",
        EqShape::Notch { .. } => "notch",
        EqShape::BandPass { .. } => "band_pass",
        EqShape::AllPass1 { .. } => "all_pass_1",
        EqShape::AllPass2 { .. } => "all_pass_2",
        EqShape::Gain { .. } => "gain",
        EqShape::Polarity => "polarity",
        EqShape::Delay { .. } => "delay",
        EqShape::Biquad { .. } => "biquad",
        EqShape::Response { .. } => "response",
    }
}

fn bw_text(b: Bandwidth) -> String {
    match b {
        Bandwidth::Q { value } => format!("q={value}"),
        Bandwidth::Octaves { n } => format!("oct={n}"),
    }
}

fn shelf_text(f: Hz, gain: Db, order: ShelfOrder, f_at: ShelfFreq) -> String {
    let slope = match order {
        ShelfOrder::First => "order=1".to_owned(),
        ShelfOrder::Second { slope } => match slope {
            ShelfSlope::Q { value } => format!("q={value}"),
            ShelfSlope::S { value } => format!("s={value}"),
            ShelfSlope::Octaves { n } => format!("oct={n}"),
        },
    };
    let at = match f_at {
        ShelfFreq::Midpoint => "midpoint",
        ShelfFreq::Corner => "corner",
    };
    format!("f={} gain={} {slope} f_at={at}", f.0, gain.0)
}

fn pass_text(f: Hz, a: Alignment, order: u8) -> String {
    let a = match a {
        Alignment::Butterworth => "alignment=butterworth".to_owned(),
        Alignment::LinkwitzRiley => "alignment=linkwitz_riley".to_owned(),
        Alignment::Bessel { norm } => format!(
            "alignment=bessel norm={}",
            match norm {
                BesselNorm::Phase => "phase",
                BesselNorm::Delay => "delay",
                BesselNorm::Minus3db => "minus_3db",
            }
        ),
    };
    format!("f={} {a} order={order}", f.0)
}

fn params(s: &EqShape) -> String {
    match *s {
        EqShape::Peaking {
            f,
            gain,
            bw,
            q_measured,
        } => {
            let mut t = format!("f={} gain={} {}", f.0, gain.0, bw_text(bw));
            if let Some(p) = q_measured {
                let _ = write!(t, " q_measured={}", peak_q_word(p));
            }
            t
        }
        EqShape::LowShelf {
            f,
            gain,
            order,
            f_at,
        }
        | EqShape::HighShelf {
            f,
            gain,
            order,
            f_at,
        } => shelf_text(f, gain, order, f_at),
        EqShape::HighPass {
            f,
            alignment,
            order,
        }
        | EqShape::LowPass {
            f,
            alignment,
            order,
        } => pass_text(f, alignment, order),
        EqShape::HighPassQ { f, q } | EqShape::LowPassQ { f, q } | EqShape::AllPass2 { f, q } => {
            format!("f={} q={q}", f.0)
        }
        EqShape::Notch { f, bw } | EqShape::BandPass { f, bw } => {
            format!("f={} {}", f.0, bw_text(bw))
        }
        EqShape::AllPass1 { f } => format!("f={}", f.0),
        EqShape::Gain { gain } => format!("gain={}", gain.0),
        EqShape::Polarity => String::new(),
        EqShape::Delay { seconds } => format!("seconds={}", seconds.0),
        EqShape::Biquad {
            b0,
            b1,
            b2,
            a1,
            a2,
            rate,
        } => format!("b0={b0} b1={b1} b2={b2} a1={a1} a2={a2} rate={}", rate.0),
        EqShape::Response { trace } => format!("trace={}", trace.0),
    }
}

/// The set as ac2 EQ text.
pub fn write(body: &EqBody) -> String {
    let mut t = format!("{HEADER}\nname = {}\n", body.name.replace('\n', " "));
    let _ = match body.realisation {
        EqRealisation::Digital { rate } => writeln!(t, "realisation = digital {}", rate.0),
        EqRealisation::Analog => writeln!(t, "realisation = analog"),
    };
    let _ = writeln!(t, "peak_q = {}", peak_q_word(body.peak_q));
    if !body.enabled {
        let _ = writeln!(t, "enabled = false");
    }
    for s in &body.sections {
        let p = params(&s.shape);
        let mut line = shape_name(&s.shape).to_owned();
        if !p.is_empty() {
            line.push(' ');
            line.push_str(&p);
        }
        if !s.enabled {
            line.push_str(" off");
        }
        t.push_str(&line);
        t.push('\n');
    }
    t
}

/// A section line's `key=value` pairs, each taken once.
struct Keys<'a> {
    pairs: Vec<(&'a str, &'a str, bool)>,
}

impl<'a> Keys<'a> {
    fn num(&mut self, key: &str) -> Result<Option<f64>, String> {
        self.word(key)?
            .map(|v| {
                v.parse::<f64>()
                    .ok()
                    .filter(|x| x.is_finite())
                    .ok_or_else(|| format!("{key}={v}: not a number"))
            })
            .transpose()
    }

    fn need(&mut self, key: &str) -> Result<f64, String> {
        self.num(key)?.ok_or_else(|| format!("missing {key}="))
    }

    fn word(&mut self, key: &str) -> Result<Option<&'a str>, String> {
        let mut found = None;
        for p in &mut self.pairs {
            if p.0 == key {
                if p.2 || found.is_some() {
                    return Err(format!("{key}= given twice"));
                }
                p.2 = true;
                found = Some(p.1);
            }
        }
        Ok(found)
    }

    fn bw(&mut self) -> Result<Bandwidth, String> {
        match (self.num("q")?, self.num("oct")?) {
            (Some(value), None) => Ok(Bandwidth::Q { value }),
            (None, Some(n)) => Ok(Bandwidth::Octaves { n }),
            _ => Err("give one of q= or oct=".into()),
        }
    }

    fn done(&self) -> Result<(), String> {
        match self.pairs.iter().find(|p| !p.2) {
            Some(p) => Err(format!("unknown key {}=", p.0)),
            None => Ok(()),
        }
    }
}

fn peak_q_of(w: &str) -> Result<PeakQ, String> {
    match w {
        "half_gain" => Ok(PeakQ::HalfGain),
        "minus_3db_from_peak" => Ok(PeakQ::Minus3dbFromPeak),
        "band_pass" => Ok(PeakQ::BandPass),
        _ => Err(format!(
            "{w}: Q is measured at half_gain, minus_3db_from_peak or band_pass"
        )),
    }
}

fn shelf_of(k: &mut Keys<'_>) -> Result<(Hz, Db, ShelfOrder, ShelfFreq), String> {
    let f = Hz(k.need("f")?);
    let gain = Db(k.need("gain")?);
    let first = match k.word("order")? {
        None => false,
        Some("1") => true,
        Some("2") => false,
        Some(o) => return Err(format!("order={o}: a shelf is order 1 or 2")),
    };
    let slopes = [k.num("q")?, k.num("s")?, k.num("oct")?];
    let order = match (first, slopes) {
        (true, [None, None, None]) => ShelfOrder::First,
        (true, _) => return Err("a first-order shelf has no slope".into()),
        (false, [Some(value), None, None]) => ShelfOrder::Second {
            slope: ShelfSlope::Q { value },
        },
        (false, [None, Some(value), None]) => ShelfOrder::Second {
            slope: ShelfSlope::S { value },
        },
        (false, [None, None, Some(n)]) => ShelfOrder::Second {
            slope: ShelfSlope::Octaves { n },
        },
        (false, _) => return Err("give order=1 or one of q=, s=, oct=".into()),
    };
    let f_at = match k.word("f_at")? {
        None | Some("midpoint") => ShelfFreq::Midpoint,
        Some("corner") => ShelfFreq::Corner,
        Some(w) => return Err(format!("f_at={w}: midpoint or corner")),
    };
    Ok((f, gain, order, f_at))
}

fn pass_of(k: &mut Keys<'_>) -> Result<(Hz, Alignment, u8), String> {
    let f = Hz(k.need("f")?);
    let alignment = match k.word("alignment")? {
        Some("butterworth") => Alignment::Butterworth,
        Some("linkwitz_riley") => Alignment::LinkwitzRiley,
        Some("bessel") => Alignment::Bessel {
            norm: match k.word("norm")? {
                Some("phase") => BesselNorm::Phase,
                Some("delay") => BesselNorm::Delay,
                Some("minus_3db") => BesselNorm::Minus3db,
                Some(w) => return Err(format!("norm={w}: phase, delay or minus_3db")),
                None => return Err("a Bessel filter needs norm=".into()),
            },
        },
        Some(w) => {
            return Err(format!(
                "alignment={w}: butterworth, linkwitz_riley or bessel"
            ));
        }
        None => return Err("missing alignment=".into()),
    };
    let order = k.need("order")?;
    if order.fract() != 0.0 || !(1.0..=255.0).contains(&order) {
        return Err(format!("order={order}: a whole number"));
    }
    Ok((f, alignment, order as u8))
}

fn section_of(line: &str) -> Result<EqSection, String> {
    let mut words: Vec<&str> = line.split_whitespace().collect();
    let enabled = match words.last() {
        Some(&"off") => {
            words.pop();
            false
        }
        _ => true,
    };
    let (name, rest) = words.split_first().ok_or("an empty section")?;
    let mut pairs = Vec::with_capacity(rest.len());
    for w in rest {
        let (k, v) = w
            .split_once('=')
            .ok_or_else(|| format!("{w}: write key=value"))?;
        pairs.push((k, v, false));
    }
    let mut k = Keys { pairs };
    let shape = match *name {
        "peaking" => EqShape::Peaking {
            f: Hz(k.need("f")?),
            gain: Db(k.need("gain")?),
            bw: k.bw()?,
            q_measured: k.word("q_measured")?.map(peak_q_of).transpose()?,
        },
        "low_shelf" => {
            let (f, gain, order, f_at) = shelf_of(&mut k)?;
            EqShape::LowShelf {
                f,
                gain,
                order,
                f_at,
            }
        }
        "high_shelf" => {
            let (f, gain, order, f_at) = shelf_of(&mut k)?;
            EqShape::HighShelf {
                f,
                gain,
                order,
                f_at,
            }
        }
        "high_pass" => {
            let (f, alignment, order) = pass_of(&mut k)?;
            EqShape::HighPass {
                f,
                alignment,
                order,
            }
        }
        "low_pass" => {
            let (f, alignment, order) = pass_of(&mut k)?;
            EqShape::LowPass {
                f,
                alignment,
                order,
            }
        }
        "high_pass_q" => EqShape::HighPassQ {
            f: Hz(k.need("f")?),
            q: k.need("q")?,
        },
        "low_pass_q" => EqShape::LowPassQ {
            f: Hz(k.need("f")?),
            q: k.need("q")?,
        },
        "notch" => EqShape::Notch {
            f: Hz(k.need("f")?),
            bw: k.bw()?,
        },
        "band_pass" => EqShape::BandPass {
            f: Hz(k.need("f")?),
            bw: k.bw()?,
        },
        "all_pass_1" => EqShape::AllPass1 {
            f: Hz(k.need("f")?),
        },
        "all_pass_2" => EqShape::AllPass2 {
            f: Hz(k.need("f")?),
            q: k.need("q")?,
        },
        "gain" => EqShape::Gain {
            gain: Db(k.need("gain")?),
        },
        "polarity" => EqShape::Polarity,
        "delay" => EqShape::Delay {
            seconds: Seconds(k.need("seconds")?),
        },
        "biquad" => EqShape::Biquad {
            b0: k.need("b0")?,
            b1: k.need("b1")?,
            b2: k.need("b2")?,
            a1: k.need("a1")?,
            a2: k.need("a2")?,
            rate: Hz(k.need("rate")?),
        },
        "response" => {
            let t = k.need("trace")?;
            if t.fract() != 0.0 || !(0.0..=f64::from(u32::MAX)).contains(&t) {
                return Err(format!("trace={t}: a trace id"));
            }
            EqShape::Response {
                trace: TraceId(t as u32),
            }
        }
        other => return Err(format!("unknown shape {other}")),
    };
    k.done()?;
    Ok(EqSection { enabled, shape })
}

/// Reads an ac2 EQ text. Values are taken as written; whether the filters can be built is
/// the daemon's check (`ac2_traces::eq::validate`), which names the section.
pub fn read(text: &str) -> Result<EqBody, EqTextError> {
    let err = |line: usize, reason: String| EqTextError { line, reason };
    let mut lines = text.lines().enumerate().map(|(i, l)| (i + 1, l.trim()));
    match lines.find(|(_, l)| !l.is_empty()) {
        Some((_, HEADER)) => {}
        Some((n, _)) => return Err(err(n, format!("not an ac2 EQ text (want {HEADER:?})"))),
        None => return Err(err(1, "empty".into())),
    }
    let mut body = EqBody {
        name: String::new(),
        realisation: EqRealisation::Analog,
        peak_q: PeakQ::HalfGain,
        enabled: true,
        sections: Vec::new(),
    };
    let mut realisation = None;
    for (n, l) in lines {
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = l.split_once(" = ").or_else(|| {
            l.split_once('=')
                .filter(|(k, _)| !k.trim().contains(char::is_whitespace))
                .filter(|(k, _)| matches!(k.trim(), "name" | "realisation" | "peak_q" | "enabled"))
        }) {
            let (key, value) = (key.trim(), value.trim());
            if !body.sections.is_empty() {
                return Err(err(n, format!("{key} belongs before the sections")));
            }
            match key {
                "name" => body.name = value.to_owned(),
                "realisation" => {
                    realisation = Some(match value.split_whitespace().collect::<Vec<_>>()[..] {
                        ["analog"] => EqRealisation::Analog,
                        ["digital", rate] => EqRealisation::Digital {
                            rate: Hz(rate
                                .parse::<f64>()
                                .ok()
                                .filter(|r| r.is_finite() && *r > 0.0)
                                .ok_or_else(|| err(n, format!("{rate}: not a sample rate")))?),
                        },
                        _ => return Err(err(n, "realisation = digital <rate> or analog".into())),
                    });
                }
                "peak_q" => body.peak_q = peak_q_of(value).map_err(|r| err(n, r))?,
                "enabled" => {
                    body.enabled = match value {
                        "true" => true,
                        "false" => false,
                        _ => return Err(err(n, "enabled = true or false".into())),
                    }
                }
                other => return Err(err(n, format!("unknown setting {other}"))),
            }
            continue;
        }
        body.sections.push(section_of(l).map_err(|r| err(n, r))?);
    }
    if body.name.is_empty() {
        return Err(err(1, "missing name = …".into()));
    }
    body.realisation = realisation.ok_or_else(|| err(1, "missing realisation = …".into()))?;
    Ok(body)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn every_shape() -> EqBody {
        let s = |shape| EqSection {
            enabled: true,
            shape,
        };
        EqBody {
            name: "EQ 1".into(),
            realisation: EqRealisation::Digital { rate: Hz(96_000.0) },
            peak_q: PeakQ::Minus3dbFromPeak,
            enabled: false,
            sections: vec![
                s(EqShape::Peaking {
                    f: Hz(1250.0),
                    gain: Db(-4.0),
                    bw: Bandwidth::Q { value: 2.0 },
                    q_measured: Some(PeakQ::HalfGain),
                }),
                EqSection {
                    enabled: false,
                    shape: EqShape::Peaking {
                        f: Hz(3150.5),
                        gain: Db(6.25),
                        bw: Bandwidth::Octaves { n: 0.7 },
                        q_measured: None,
                    },
                },
                s(EqShape::LowShelf {
                    f: Hz(120.0),
                    gain: Db(3.0),
                    order: ShelfOrder::Second {
                        slope: ShelfSlope::S { value: 1.0 },
                    },
                    f_at: ShelfFreq::Midpoint,
                }),
                s(EqShape::HighShelf {
                    f: Hz(8000.0),
                    gain: Db(-6.0),
                    order: ShelfOrder::First,
                    f_at: ShelfFreq::Corner,
                }),
                s(EqShape::HighShelf {
                    f: Hz(9000.0),
                    gain: Db(-6.0),
                    order: ShelfOrder::Second {
                        slope: ShelfSlope::Octaves { n: 1.5 },
                    },
                    f_at: ShelfFreq::Midpoint,
                }),
                s(EqShape::HighPass {
                    f: Hz(80.0),
                    alignment: Alignment::LinkwitzRiley,
                    order: 4,
                }),
                s(EqShape::LowPass {
                    f: Hz(18_000.0),
                    alignment: Alignment::Bessel {
                        norm: BesselNorm::Minus3db,
                    },
                    order: 3,
                }),
                s(EqShape::HighPassQ {
                    f: Hz(40.0),
                    q: 1.2,
                }),
                s(EqShape::LowPassQ {
                    f: Hz(16_000.0),
                    q: 0.5,
                }),
                s(EqShape::Notch {
                    f: Hz(60.0),
                    bw: Bandwidth::Q { value: 8.0 },
                }),
                s(EqShape::BandPass {
                    f: Hz(500.0),
                    bw: Bandwidth::Octaves { n: 1.0 },
                }),
                s(EqShape::AllPass1 { f: Hz(300.0) }),
                s(EqShape::AllPass2 {
                    f: Hz(90.0),
                    q: 0.7,
                }),
                s(EqShape::Gain { gain: Db(-1.5) }),
                s(EqShape::Polarity),
                s(EqShape::Delay {
                    seconds: Seconds(0.00125),
                }),
                s(EqShape::Biquad {
                    b0: 1.0,
                    b1: -1.9,
                    b2: 0.91,
                    a1: -1.8,
                    a2: 0.82,
                    rate: Hz(48_000.0),
                }),
                s(EqShape::Response { trace: TraceId(7) }),
            ],
        }
    }

    #[test]
    fn every_shape_round_trips() {
        let b = every_shape();
        let t = write(&b);
        assert!(is_ac2_eq(&t));
        assert_eq!(read(&t).unwrap(), b, "{t}");
        let mut a = b.clone();
        a.realisation = EqRealisation::Analog;
        a.enabled = true;
        assert_eq!(read(&write(&a)).unwrap(), a);
    }

    #[test]
    fn the_note_example_reads() {
        let t = "# ac2 eq 1\nname = EQ 1\nrealisation = digital 96000\n\
                 peaking   f=1250 gain=-4.0 q=2.0 q_measured=half_gain\n\
                 high_pass f=80 alignment=linkwitz_riley order=4\n\
                 low_shelf f=120 gain=3.0 s=1.0 f_at=midpoint   off\n";
        let b = read(t).unwrap();
        assert_eq!(b.sections.len(), 3);
        assert!(!b.sections[2].enabled);
        assert_eq!(b.realisation, EqRealisation::Digital { rate: Hz(96_000.0) });
    }

    #[test]
    fn refusals_name_the_line() {
        let head = "# ac2 eq 1\nname = A\nrealisation = analog\n";
        for (body, line, reason) in [
            ("peaking f=1k gain=1 q=1\n", 4, "f=1k: not a number"),
            ("peaking f=1000 gain=1\n", 4, "give one of q= or oct="),
            (
                "peaking f=1000 gain=1 q=1 oct=1\n",
                4,
                "give one of q= or oct=",
            ),
            (
                "peaking f=1000 gain=1 q=1 width=3\n",
                4,
                "unknown key width=",
            ),
            ("shelf f=1000\n", 4, "unknown shape shelf"),
            ("high_pass f=80 order=4\n", 4, "missing alignment="),
            (
                "high_pass f=80 alignment=bessel order=4\n",
                4,
                "a Bessel filter needs norm=",
            ),
            (
                "low_shelf f=1 gain=1 order=1 s=1\n",
                4,
                "a first-order shelf has no slope",
            ),
            ("gain gain=1 gain=2\n", 4, "gain= given twice"),
            (
                "gain gain=1\nname = B\n",
                5,
                "name belongs before the sections",
            ),
            ("peaking f=1000\n", 4, "missing gain="),
        ] {
            let e = read(&format!("{head}{body}")).unwrap_err();
            assert_eq!((e.line, e.reason.as_str()), (line, reason), "{body}");
        }
        assert_eq!(read("Filter: ON PK Fc 1 Hz").unwrap_err().line, 1);
        let e = read("# ac2 eq 1\nrealisation = analog\n").unwrap_err();
        assert_eq!(e.to_string(), "line 1: missing name = …");
        let e = read("# ac2 eq 1\nname = x\nrealisation = digital fast\n").unwrap_err();
        assert_eq!(e.to_string(), "line 3: fast: not a sample rate");
    }
}
