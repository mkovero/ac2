//! `ac2 meas eq …`: EQ what-if sets (`docs/design/eq-overlay.md` §4). Every edit is one
//! `meas.eq` request; the daemon validates the set and recomputes its curves.

use ac2_client::Client;
use ac2_proto::Command;
use ac2_proto::model::*;
use ac2_proto::units::{Db, EqSetId, Hz, MeasId};
use ac2_scene::eq as text;
use ac2_scene::unit_text::{Freq, Time, in_field};
use ac2_traces::text::{apo, eq as eq_text};
use serde_json::json;

use super::meas::meas_call;
use super::{find_meas, find_trace, rate, state};
use crate::CliError;
use crate::args::*;
use crate::output::Out;

fn usage(m: impl Into<String>) -> CliError {
    CliError::Usage(m.into())
}

/// The set `pick` names, or the measurement's only one.
fn pick_set<'m>(m: &'m Measurement, pick: &SetPick) -> Result<&'m EqSet, CliError> {
    let names = || {
        m.eq.iter()
            .map(|s| format!("{} (id {})", s.body.name, s.id.0))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match &pick.set {
        Some(r) => {
            m.eq.iter()
                .find(|s| r.id() == Some(s.id.0) || s.body.name == r.0)
                .ok_or_else(|| {
                    usage(format!(
                        "{} has no EQ set {:?}: {}",
                        m.config.name,
                        r.0,
                        names()
                    ))
                })
        }
        None => match m.eq.as_slice() {
            [s] => Ok(s),
            [] => Err(usage(format!(
                "{} has no EQ set: `ac2 meas eq new {:?}`",
                m.config.name, m.config.name
            ))),
            _ => Err(usage(format!(
                "{} has {} EQ sets ({}): add --set",
                m.config.name,
                m.eq.len(),
                names()
            ))),
        },
    }
}

fn peak_q(a: QMeasuredArg) -> PeakQ {
    match a {
        QMeasuredArg::HalfGain => PeakQ::HalfGain,
        QMeasuredArg::Minus3dbFromPeak => PeakQ::Minus3dbFromPeak,
        QMeasuredArg::BandPass => PeakQ::BandPass,
    }
}

fn freq(v: &str) -> Result<Hz, CliError> {
    Ok(in_field::<Freq>(v, "hz")
        .map_err(|e| usage(e.to_string()))?
        .0)
}

fn number(v: &str, what: &str) -> Result<f64, CliError> {
    v.parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
        .ok_or_else(|| usage(format!("{what} {v:?} is not a number")))
}

fn bandwidth(a: &EqSectionArgs, what: &str) -> Result<Bandwidth, CliError> {
    match (a.q, a.oct) {
        (Some(value), _) => Ok(Bandwidth::Q { value }),
        (_, Some(n)) => Ok(Bandwidth::Octaves { n }),
        _ => Err(usage(format!("{what} needs --q or --oct"))),
    }
}

/// The shape `eq add`'s arguments describe.
fn shape(a: &EqSectionArgs, s: &State) -> Result<EqShape, CliError> {
    use EqShapeArg as A;
    let want = match a.shape {
        A::Peaking | A::LowShelf | A::HighShelf => 2,
        A::HighPass | A::LowPass | A::Notch | A::BandPass | A::AllPass | A::Gain | A::Delay => 1,
        A::Polarity | A::Response => 0,
        A::Biquad => 5,
    };
    if a.values.len() != want {
        return Err(usage(format!(
            "{:?} takes {want} value(s), got {}",
            a.shape,
            a.values.len()
        )));
    }
    let v = &a.values;
    let shelf = |low: bool| -> Result<EqShape, CliError> {
        let order = if a.first {
            ShelfOrder::First
        } else {
            ShelfOrder::Second {
                slope: match (a.q, a.oct) {
                    (Some(value), _) => ShelfSlope::Q { value },
                    (_, Some(n)) => ShelfSlope::Octaves { n },
                    _ => ShelfSlope::S {
                        value: a.s.unwrap_or(1.0),
                    },
                },
            }
        };
        let (f, gain) = (freq(&v[0])?, Db(number(&v[1], "gain")?));
        let f_at = if a.corner {
            ShelfFreq::Corner
        } else {
            ShelfFreq::Midpoint
        };
        Ok(if low {
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
        })
    };
    let pass = |high: bool| -> Result<EqShape, CliError> {
        let f = freq(&v[0])?;
        let aligned = match (a.lr, a.bw, a.bessel) {
            (Some(n), _, _) => Some((Alignment::LinkwitzRiley, n)),
            (_, Some(n), _) => Some((Alignment::Butterworth, n)),
            (_, _, Some(n)) => Some((
                Alignment::Bessel {
                    norm: match a.norm.unwrap_or(BesselNormArg::Phase) {
                        BesselNormArg::Phase => BesselNorm::Phase,
                        BesselNormArg::Delay => BesselNorm::Delay,
                        BesselNormArg::Minus3db => BesselNorm::Minus3db,
                    },
                },
                n,
            )),
            _ => None,
        };
        Ok(match (aligned, a.q, high) {
            (Some((alignment, order)), _, true) => EqShape::HighPass {
                f,
                alignment,
                order,
            },
            (Some((alignment, order)), _, false) => EqShape::LowPass {
                f,
                alignment,
                order,
            },
            (None, Some(q), true) => EqShape::HighPassQ { f, q },
            (None, Some(q), false) => EqShape::LowPassQ { f, q },
            (None, None, _) => {
                return Err(usage("a high-/low-pass needs --lr, --bw, --bessel or --q"));
            }
        })
    };
    Ok(match a.shape {
        A::Peaking => EqShape::Peaking {
            f: freq(&v[0])?,
            gain: Db(number(&v[1], "gain")?),
            bw: bandwidth(a, "peaking")?,
            q_measured: a.q_measured.map(peak_q),
        },
        A::LowShelf => shelf(true)?,
        A::HighShelf => shelf(false)?,
        A::HighPass => pass(true)?,
        A::LowPass => pass(false)?,
        A::Notch => EqShape::Notch {
            f: freq(&v[0])?,
            bw: bandwidth(a, "notch")?,
        },
        A::BandPass => EqShape::BandPass {
            f: freq(&v[0])?,
            bw: bandwidth(a, "band-pass")?,
        },
        A::AllPass => match a.q {
            Some(q) => EqShape::AllPass2 { f: freq(&v[0])?, q },
            None => EqShape::AllPass1 { f: freq(&v[0])? },
        },
        A::Gain => EqShape::Gain {
            gain: Db(number(&v[0], "gain")?),
        },
        A::Polarity => EqShape::Polarity,
        A::Delay => EqShape::Delay {
            seconds: in_field::<Time>(&v[0], "ms")
                .map_err(|e| usage(e.to_string()))?
                .0,
        },
        A::Biquad => {
            let c: Vec<f64> = v
                .iter()
                .map(|x| number(x, "coefficient"))
                .collect::<Result<_, _>>()?;
            let rate = a.rate.ok_or_else(|| usage("a biquad needs --rate"))?.0;
            EqShape::Biquad {
                b0: c[0],
                b1: c[1],
                b2: c[2],
                a1: c[3],
                a2: c[4],
                rate,
            }
        }
        A::Response => {
            let r = a
                .trace
                .as_ref()
                .ok_or_else(|| usage("a response needs --trace"))?;
            EqShape::Response {
                trace: find_trace(s, r)?.id,
            }
        }
    })
}

/// One set as `eq show` and every edit print it.
fn set_text(s: Option<&State>, m: &Measurement, set: &EqSet) -> String {
    let trace_name = |id| {
        s.and_then(|s| s.traces.iter().find(|t| t.id == id))
            .map_or_else(|| format!("trace {}", id.0), |t| t.edit.name.clone())
    };
    let mut head = vec![
        format!(
            "{} · {} (set {}, rev {})",
            m.config.name, set.body.name, set.id.0, set.rev
        ),
        text::realisation_text(set.body.realisation),
        format!("Q measured {}", text::peak_q_text(set.body.peak_q)),
        text::mark_text(set),
    ];
    if !set.body.enabled {
        head.push("bypassed".into());
    }
    let mut out = head.join(" · ");
    if set.body.sections.is_empty() {
        out.push_str("\n  no sections");
    }
    for (i, sec) in set.body.sections.iter().enumerate() {
        out.push_str("\n  ");
        out.push_str(&text::section_text(i, sec, set.body.peak_q, &trace_name));
    }
    if let Some(s) = s {
        for p in &s.measurements {
            let MeasKind::Math { config } = &p.config.kind else {
                continue;
            };
            let Some((meas, id, curve)) = text::prediction(&config.expr) else {
                continue;
            };
            if meas == m.id && id == set.id {
                let basis = text::curve_basis(set, curve, |t| s.traces.iter().find(|x| x.id == t));
                out.push_str(&format!(
                    "\n  predicts {} · {}",
                    p.config.name,
                    text::basis_tag(&set.body.name, basis)
                ));
            }
        }
    }
    out
}

async fn edit(c: &Client, meas: MeasId, edit: EqEdit) -> Result<Measurement, CliError> {
    meas_call(c, Command::MeasEq { meas, edit }).await
}

fn emit_set(
    out: &mut Out<'_>,
    m: &Measurement,
    id: EqSetId,
    s: Option<&State>,
) -> Result<(), CliError> {
    let set =
        m.eq.iter()
            .find(|x| x.id == id)
            .or(m.eq.last())
            .ok_or_else(|| usage(format!("{} has no EQ set", m.config.name)))?;
    out.emit(set, || set_text(s, m, set))?;
    Ok(())
}

pub(crate) async fn meas_eq(
    c: &Client,
    cmd: &MeasEqCmd,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let s = state(c).await?;
    match cmd {
        MeasEqCmd::New {
            meas,
            name,
            rate: r,
            analog,
            q_measured,
        } => {
            let m = find_meas(&s, meas)?;
            let realisation = match (analog, r) {
                (true, _) => Some(EqRealisation::Analog),
                (_, Some(f)) => Some(EqRealisation::Digital { rate: f.0 }),
                _ => None,
            };
            let before: Vec<EqSetId> = m.eq.iter().map(|x| x.id).collect();
            let m2 = edit(
                c,
                m.id,
                EqEdit::New {
                    name: name.clone(),
                    realisation,
                    peak_q: q_measured.map(peak_q),
                    sections: Vec::new(),
                },
            )
            .await?;
            let id = m2
                .eq
                .iter()
                .find(|x| !before.contains(&x.id))
                .map_or(EqSetId(0), |x| x.id);
            emit_set(out, &m2, id, None)?;
        }
        MeasEqCmd::Add {
            meas,
            section,
            at,
            pick,
        } => {
            let m = find_meas(&s, meas)?;
            let set = pick_set(m, pick)?;
            let mut body = set.body.clone();
            let sec = EqSection {
                enabled: true,
                shape: shape(section, &s)?,
            };
            match at {
                Some(n) if (1..=body.sections.len() + 1).contains(n) => {
                    body.sections.insert(n - 1, sec)
                }
                Some(n) => {
                    return Err(usage(format!(
                        "--at {n}: {} has {} section(s)",
                        set.body.name,
                        body.sections.len()
                    )));
                }
                None => body.sections.push(sec),
            }
            let m2 = edit(c, m.id, EqEdit::Put { set: set.id, body }).await?;
            emit_set(out, &m2, set.id, Some(&s))?;
        }
        MeasEqCmd::Set {
            meas,
            index,
            f,
            gain,
            q,
            oct,
            off,
            on,
            pick,
        } => {
            let m = find_meas(&s, meas)?;
            let set = pick_set(m, pick)?;
            let mut body = set.body.clone();
            let n = body.sections.len();
            let sec = index
                .checked_sub(1)
                .and_then(|i| body.sections.get_mut(i))
                .ok_or_else(|| {
                    usage(format!(
                        "{} has no section {index} (it has {n})",
                        set.body.name
                    ))
                })?;
            if *off {
                sec.enabled = false;
            }
            if *on {
                sec.enabled = true;
            }
            set_fields(&mut sec.shape, f.map(|x| x.0), *gain, *q, *oct)?;
            let m2 = edit(c, m.id, EqEdit::Put { set: set.id, body }).await?;
            emit_set(out, &m2, set.id, Some(&s))?;
        }
        MeasEqCmd::Rm { meas, index, pick } => {
            let m = find_meas(&s, meas)?;
            let set = pick_set(m, pick)?;
            let mut body = set.body.clone();
            if *index == 0 || *index > body.sections.len() {
                return Err(usage(format!(
                    "{} has no section {index} (it has {})",
                    set.body.name,
                    body.sections.len()
                )));
            }
            body.sections.remove(index - 1);
            let m2 = edit(c, m.id, EqEdit::Put { set: set.id, body }).await?;
            emit_set(out, &m2, set.id, Some(&s))?;
        }
        MeasEqCmd::Bypass { meas, off, pick } => {
            let m = find_meas(&s, meas)?;
            let set = pick_set(m, pick)?;
            let mut body = set.body.clone();
            body.enabled = *off;
            let m2 = edit(c, m.id, EqEdit::Put { set: set.id, body }).await?;
            emit_set(out, &m2, set.id, Some(&s))?;
        }
        MeasEqCmd::On {
            meas,
            curve,
            off,
            pick,
        } => {
            let m = find_meas(&s, meas)?;
            let set = pick_set(m, pick)?;
            let operand = if curve.0 == "live" || curve.0 == m.config.name {
                Operand::Meas { meas: m.id }
            } else {
                Operand::Trace {
                    trace: find_trace(&s, curve)?.id,
                }
            };
            edit(
                c,
                m.id,
                EqEdit::Predict {
                    set: set.id,
                    curve: operand,
                    on: !off,
                },
            )
            .await?;
            let s2 = state(c).await?;
            let m2 = find_meas(&s2, meas)?;
            emit_set(out, m2, set.id, Some(&s2))?;
        }
        MeasEqCmd::Mark { meas, clear, pick } => {
            let m = find_meas(&s, meas)?;
            let set = pick_set(m, pick)?;
            let m2 = edit(
                c,
                m.id,
                EqEdit::Mark {
                    set: set.id,
                    clear: *clear,
                },
            )
            .await?;
            emit_set(out, &m2, set.id, Some(&s))?;
        }
        MeasEqCmd::Copy { meas, to, pick } => {
            let m = find_meas(&s, meas)?;
            let set = pick_set(m, pick)?;
            let dest = find_meas(&s, to)?;
            let before: Vec<EqSetId> = dest.eq.iter().map(|x| x.id).collect();
            edit(
                c,
                m.id,
                EqEdit::Copy {
                    set: set.id,
                    to: dest.id,
                },
            )
            .await?;
            let s2 = state(c).await?;
            let d2 = find_meas(&s2, to)?;
            let id = d2
                .eq
                .iter()
                .find(|x| !before.contains(&x.id))
                .map_or(EqSetId(0), |x| x.id);
            emit_set(out, d2, id, Some(&s2))?;
        }
        MeasEqCmd::Delete { meas, pick } => {
            let m = find_meas(&s, meas)?;
            let set = pick_set(m, pick)?;
            let name = set.body.name.clone();
            edit(c, m.id, EqEdit::Delete { set: set.id }).await?;
            out.emit(&json!({ "deleted": set.id }), || {
                format!("deleted {name} of {}", m.config.name)
            })?;
        }
        MeasEqCmd::Show { meas } => {
            let m = find_meas(&s, meas)?;
            out.emit(&m.eq, || {
                if m.eq.is_empty() {
                    format!("{} has no EQ set", m.config.name)
                } else {
                    m.eq.iter()
                        .map(|set| set_text(Some(&s), m, set))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            })?;
        }
        MeasEqCmd::Export {
            meas,
            file,
            format,
            skip_unsupported,
            pick,
        } => {
            let m = find_meas(&s, meas)?;
            let set = pick_set(m, pick)?;
            let content = match format {
                EqFormatArg::Ac2 => eq_text::write(&set.body),
                EqFormatArg::Apo => apo::write(&set.body, *skip_unsupported).map_err(|r| {
                    usage(format!(
                        "{} cannot be written as APO text exactly (--skip-unsupported writes \
                         the rest):\n  {}",
                        set.body.name,
                        r.iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("\n  ")
                    ))
                })?,
            };
            std::fs::write(file, &content)?;
            let path = file.display().to_string();
            // The mark says what the console holds; it is not part of any filter.
            let unmarked = if *format == EqFormatArg::Apo && set.in_console.is_some() {
                " (the in-console mark is not a filter and is not written)"
            } else {
                ""
            };
            out.emit(&json!({ "written": path, "set": set.id }), || {
                format!("wrote {} to {path}{unmarked}", set.body.name)
            })?;
        }
        MeasEqCmd::Import {
            meas,
            file,
            format,
            pick,
        } => {
            let m = find_meas(&s, meas)?;
            let content = std::fs::read_to_string(file)?;
            let path = file.display().to_string();
            let target = pick.set.as_ref().map(|_| pick_set(m, pick)).transpose()?;
            let format = format.unwrap_or(if eq_text::is_ac2_eq(&content) {
                EqFormatArg::Ac2
            } else {
                EqFormatArg::Apo
            });
            let body = match format {
                EqFormatArg::Ac2 => {
                    eq_text::read(&content).map_err(|e| usage(format!("{path}: {e}")))?
                }
                EqFormatArg::Apo => {
                    // A file's corner-specified shelves move to their midpoint at the rate
                    // the set will be evaluated at.
                    let realisation = target.map_or(
                        EqRealisation::Digital {
                            rate: Hz(f64::from(rate(&s).unwrap_or(48_000))),
                        },
                        |t| t.body.realisation,
                    );
                    let sections = apo::read(&content, realisation)
                        .map_err(|e| usage(format!("{path}: {e}")))?;
                    EqBody {
                        name: target.map_or_else(String::new, |t| t.body.name.clone()),
                        realisation,
                        peak_q: target.map_or(PeakQ::HalfGain, |t| t.body.peak_q),
                        enabled: true,
                        sections,
                    }
                }
            };
            let (m2, id) = match target {
                Some(t) => {
                    // The set named keeps its name: two sets of one name could not be told apart.
                    let body = EqBody {
                        name: t.body.name.clone(),
                        ..body
                    };
                    (edit(c, m.id, EqEdit::Put { set: t.id, body }).await?, t.id)
                }
                None => {
                    let before: Vec<EqSetId> = m.eq.iter().map(|x| x.id).collect();
                    let enabled = body.enabled;
                    let mut m2 = edit(
                        c,
                        m.id,
                        EqEdit::New {
                            name: (!body.name.is_empty()).then(|| body.name.clone()),
                            realisation: Some(body.realisation),
                            peak_q: Some(body.peak_q),
                            sections: body.sections.clone(),
                        },
                    )
                    .await?;
                    let id = m2
                        .eq
                        .iter()
                        .find(|x| !before.contains(&x.id))
                        .map_or(EqSetId(0), |x| x.id);
                    if !enabled && let Some(new) = m2.eq.iter().find(|x| x.id == id) {
                        let body = EqBody {
                            enabled: false,
                            ..new.body.clone()
                        };
                        m2 = edit(c, m.id, EqEdit::Put { set: id, body }).await?;
                    }
                    (m2, id)
                }
            };
            emit_set(out, &m2, id, Some(&s))?;
        }
    }
    Ok(())
}

/// `eq set`'s field changes on a section, refused where the shape has no such field.
fn set_fields(
    shape: &mut EqShape,
    f: Option<Hz>,
    gain: Option<f64>,
    q: Option<f64>,
    oct: Option<f64>,
) -> Result<(), CliError> {
    let word = text::shape_word(shape);
    let no = |what: &str| usage(format!("a {word} section has no {what}"));
    if let Some(new) = f {
        match shape {
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
            | EqShape::AllPass2 { f, .. } => *f = new,
            _ => return Err(no("frequency")),
        }
    }
    if let Some(g) = gain {
        match shape {
            EqShape::Peaking { gain, .. }
            | EqShape::LowShelf { gain, .. }
            | EqShape::HighShelf { gain, .. }
            | EqShape::Gain { gain } => *gain = Db(g),
            _ => return Err(no("gain")),
        }
    }
    let bw = match (q, oct) {
        (Some(value), _) => Some(Bandwidth::Q { value }),
        (_, Some(n)) => Some(Bandwidth::Octaves { n }),
        _ => None,
    };
    if let Some(new) = bw {
        match shape {
            EqShape::Peaking { bw, .. }
            | EqShape::Notch { bw, .. }
            | EqShape::BandPass { bw, .. } => {
                *bw = new;
            }
            EqShape::LowShelf { order, .. } | EqShape::HighShelf { order, .. } => {
                *order = ShelfOrder::Second {
                    slope: match new {
                        Bandwidth::Q { value } => ShelfSlope::Q { value },
                        Bandwidth::Octaves { n } => ShelfSlope::Octaves { n },
                    },
                };
            }
            EqShape::HighPassQ { q: qq, .. }
            | EqShape::LowPassQ { q: qq, .. }
            | EqShape::AllPass2 { q: qq, .. } => match new {
                Bandwidth::Q { value } => *qq = value,
                Bandwidth::Octaves { .. } => return Err(no("bandwidth in octaves")),
            },
            _ => return Err(no("Q")),
        }
    }
    Ok(())
}
