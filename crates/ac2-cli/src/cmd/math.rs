//! `math new|set`: math channels with their operands by name.

use ac2_proto::model::{
    MathConfig, MathDomain, MathExpr, MathOp, MathReference, MeasConfig, MeasKind, Operand,
    Smoothing, SmoothingMode, State, TraceKind,
};
use ac2_proto::{Command, model::Measurement};

use super::{connect, find_meas, find_trace, state};
use crate::CliError;
use crate::args::{AverageArg, Cli, MathArgs, MathCmd, MathOpArg, MeasRef};
use crate::output::{self, Out};

pub(crate) async fn run(cli: &Cli, cmd: &MathCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    let m = match cmd {
        MathCmd::New { math, start } => {
            let config = math_config(math, &s, None)?;
            let mut m = super::basic::meas_call(&c, Command::MeasCreate { config }).await?;
            if *start {
                m = super::basic::meas_call(&c, Command::MeasStart { meas: m.id }).await?;
            }
            m
        }
        MathCmd::Set { channel, math } => {
            let existing = find_meas(&s, channel)?;
            let config = math_config(math, &s, Some(existing))?;
            super::basic::meas_call(
                &c,
                Command::MeasUpdate {
                    meas: existing.id,
                    config,
                },
            )
            .await?
        }
    };
    out.emit(&m, || output::measurement(&m))?;
    Ok(())
}

/// An operand by id or name: a measurement, else a stored trace; `trace:NAME` and
/// `meas:NAME` say which when both are named alike. A bare number is a measurement id or
/// name (a trace by id is `trace:7`).
pub(crate) fn find_operand(s: &State, r: &MeasRef) -> Result<Operand, CliError> {
    let text = r.0.as_str();
    let named = |t: &str| MeasRef(t.trim().to_owned());
    if let Some(t) = text.strip_prefix("trace:") {
        return Ok(Operand::Trace {
            trace: find_trace(s, &named(t))?.id,
        });
    }
    if let Some(t) = text.strip_prefix("meas:") {
        return Ok(Operand::Meas {
            meas: find_meas(s, &named(t))?.id,
        });
    }
    let m = find_meas(s, r);
    if r.id().is_some() {
        return m.map(|m| Operand::Meas { meas: m.id }).map_err(|_| {
            CliError::Usage(format!(
                "no measurement {text}; a stored trace by id is trace:{text}"
            ))
        });
    }
    match (m, find_trace(s, r)) {
        (Ok(m), Err(_)) => Ok(Operand::Meas { meas: m.id }),
        (Err(_), Ok(t)) => Ok(Operand::Trace { trace: t.id }),
        (Ok(_), Ok(_)) => Err(CliError::Usage(format!(
            "{text:?} names a measurement and a stored trace: write meas:{text} or trace:{text}"
        ))),
        (Err(_), Err(_)) => Err(CliError::Usage(format!(
            "no measurement or stored trace named {text:?}"
        ))),
    }
}

/// An operand's name as the daemon's state has it.
fn operand_name(s: &State, o: Operand) -> String {
    match o {
        Operand::Meas { meas } => s
            .measurements
            .iter()
            .find(|m| m.id == meas)
            .map_or_else(|| format!("measurement {meas}"), |m| m.config.name.clone()),
        Operand::Trace { trace } => s
            .traces
            .iter()
            .find(|t| t.id == trace)
            .map_or_else(|| format!("trace {trace}"), |t| t.edit.name.clone()),
    }
}

/// What a math channel of `o` combines: transfer functions, spectra or RTA bands.
fn domain_of(s: &State, o: Operand) -> Result<MathDomain, CliError> {
    let name = operand_name(s, o);
    let not = || {
        CliError::Usage(format!(
            "{name} is neither a transfer function, a spectrum nor an RTA"
        ))
    };
    match o {
        Operand::Meas { meas } => {
            let m = s
                .measurements
                .iter()
                .find(|m| m.id == meas)
                .ok_or_else(not)?;
            match &m.config.kind {
                MeasKind::Transfer { .. } => Ok(MathDomain::Transfer),
                MeasKind::Spectrum { .. } => Ok(MathDomain::Spectrum),
                MeasKind::Rta { .. } => Ok(MathDomain::Rta),
                MeasKind::Math { .. } => Err(CliError::Usage(format!(
                    "{name} is a math channel: capture it (`ac2 trace capture`) and use the trace"
                ))),
                MeasKind::Spl { .. } => Err(not()),
            }
        }
        Operand::Trace { trace } => {
            let t = s.traces.iter().find(|t| t.id == trace).ok_or_else(not)?;
            Ok(match t.kind {
                TraceKind::Transfer | TraceKind::Sweep | TraceKind::Target => MathDomain::Transfer,
                TraceKind::Spectrum { .. } => MathDomain::Spectrum,
                TraceKind::Rta { .. } => MathDomain::Rta,
            })
        }
    }
}

/// `"Main L / Sub"` → (`Main L`, ÷, `Sub`). The operator stands alone between spaces, so
/// names may hold `-` or `/` themselves.
pub(crate) fn split_expr(text: &str) -> Result<(MeasRef, MathOp, MeasRef), CliError> {
    const OPS: [(&str, MathOp); 7] = [
        (" / ", MathOp::Divide),
        (" ÷ ", MathOp::Divide),
        (" * ", MathOp::Multiply),
        (" × ", MathOp::Multiply),
        (" + ", MathOp::Add),
        (" - ", MathOp::Subtract),
        (" − ", MathOp::Subtract),
    ];
    let found: Vec<(usize, &str, MathOp)> = OPS
        .iter()
        .flat_map(|(t, op)| text.match_indices(t).map(move |(i, _)| (i, *t, *op)))
        .collect();
    let [(i, t, op)] = found.as_slice() else {
        return Err(CliError::Usage(format!(
            "{text:?}: write one operator between spaces, e.g. \"Main L / Sub\" (or use --op \
             with --a and --b)"
        )));
    };
    let side = |s: &str| -> Result<MeasRef, CliError> {
        s.trim()
            .parse()
            .map_err(|_| CliError::Usage(format!("{text:?}: an operand is missing")))
    };
    Ok((side(&text[..*i])?, *op, side(&text[i + t.len()..])?))
}

fn method(m: AverageArg) -> ac2_proto::model::AverageMethod {
    super::traces::method(m)
}

/// The configuration `math new` (`existing` `None`) or `math set` creates from `a`,
/// operands resolved against `s`.
pub(crate) fn math_config(
    a: &MathArgs,
    s: &State,
    existing: Option<&Measurement>,
) -> Result<MeasConfig, CliError> {
    let old = match existing.map(|m| &m.config.kind) {
        Some(MeasKind::Math { config }) => Some(config),
        Some(_) => {
            return Err(CliError::Usage(format!(
                "{} is not a math channel",
                existing.map_or("", |m| m.config.name.as_str())
            )));
        }
        None => None,
    };
    let find = |r: &MeasRef| find_operand(s, r);
    let mut expr = match (&a.expr, a.op) {
        (Some(text), _) => {
            let (x, op, y) = split_expr(text)?;
            MathExpr::Binary {
                a: find(&x)?,
                op,
                b: find(&y)?,
            }
        }
        (None, Some(MathOpArg::Avg)) => {
            if a.of.len() < MathConfig::MIN_AVERAGE {
                return Err(CliError::Usage(format!(
                    "an average needs at least {} operands: --of A,B",
                    MathConfig::MIN_AVERAGE
                )));
            }
            MathExpr::Average {
                of: a.of.iter().map(find).collect::<Result<_, _>>()?,
                method: method(a.method.unwrap_or(AverageArg::Power)),
            }
        }
        (None, Some(op)) => {
            let (Some(x), Some(y)) = (&a.a, &a.b) else {
                return Err(CliError::Usage("--op needs --a and --b".into()));
            };
            let op = match op {
                MathOpArg::Div => MathOp::Divide,
                MathOpArg::Mul => MathOp::Multiply,
                MathOpArg::Add => MathOp::Add,
                MathOpArg::Sub | MathOpArg::Avg => MathOp::Subtract,
            };
            MathExpr::Binary {
                a: find(x)?,
                op,
                b: find(y)?,
            }
        }
        (None, None) => match old {
            Some(c) => {
                let mut e = c.expr.clone();
                if let (MathExpr::Average { of, .. }, false) = (&mut e, a.of.is_empty()) {
                    *of = a.of.iter().map(find).collect::<Result<_, _>>()?;
                }
                e
            }
            None => {
                return Err(CliError::Usage(
                    "give the expression, e.g. \"Main L / Sub\", or --op with --a/--b or --of"
                        .into(),
                ));
            }
        },
    };
    if let Some(m) = a.method {
        match &mut expr {
            MathExpr::Average { method: x, .. } => *x = method(m),
            MathExpr::Binary { .. } => {
                return Err(CliError::Usage("--method applies to an average".into()));
            }
        }
    }
    if !a.of.is_empty() && matches!(expr, MathExpr::Binary { .. }) {
        return Err(CliError::Usage(
            "--of applies to an average (--op avg)".into(),
        ));
    }
    let operands = expr.operands();
    let first = *operands
        .first()
        .ok_or_else(|| CliError::Usage("no operands".into()))?;
    let domain = domain_of(s, first)?;
    let reference = match (&a.phase_ref, a.ref_delay) {
        (_, Some(d)) => MathReference::Fixed { delay: d.0 },
        (Some(r), None) => {
            let o = find(r)?;
            if !operands.contains(&o) {
                return Err(CliError::Usage(format!(
                    "the phase reference {} is not an operand",
                    r.0
                )));
            }
            MathReference::Operand { operand: o }
        }
        (None, None) => match old.map(|c| c.reference) {
            Some(MathReference::Operand { operand }) if operands.contains(&operand) => {
                MathReference::Operand { operand }
            }
            Some(f @ MathReference::Fixed { .. }) => f,
            _ => MathReference::Operand { operand: first },
        },
    };
    let smoothing = match (a.smooth, a.no_smooth) {
        (_, true) => None,
        (Some(f), false) => Some(Smoothing {
            fraction: super::basic::smoothing(f)?,
            mode: if a.smooth_magnitude_only || domain != MathDomain::Transfer {
                SmoothingMode::Magnitude
            } else {
                SmoothingMode::MagnitudePhase
            },
        }),
        (None, false) => old.and_then(|c| c.smoothing),
    };
    let name = match (&a.name, existing) {
        (Some(n), _) => n.clone(),
        (None, Some(m)) => m.config.name.clone(),
        (None, None) => ac2_scene::math::default_name(&expr, |o| operand_name(s, o)),
    };
    Ok(MeasConfig {
        name,
        kind: MeasKind::Math {
            config: MathConfig {
                domain,
                expr,
                reference,
                smoothing,
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions_split_on_the_spaced_operator() {
        let (a, op, b) = split_expr("Main L / Sub").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            (a.0.as_str(), op, b.0.as_str()),
            ("Main L", MathOp::Divide, "Sub")
        );
        let (a, op, b) = split_expr("L-R − Sub 2").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            (a.0.as_str(), op, b.0.as_str()),
            ("L-R", MathOp::Subtract, "Sub 2")
        );
        let (_, op, _) = split_expr("Main × Fill").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(op, MathOp::Multiply);
        assert!(split_expr("Main/Sub").is_err(), "unspaced");
        assert!(split_expr("A + B + C").is_err(), "two operators");
        assert!(split_expr(" / Sub").is_err(), "no A");
    }
}
