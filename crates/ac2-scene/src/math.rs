//! What a math channel says about itself: its expression by name, which operands went into
//! its newest frame and why the others did not, and what its phase and coherence mean
//! (`docs/design/math-channels.md`).
//!
//! A result that silently dropped an operand would read as what its expression says while
//! being something else: the legend counts an average's positions, and a banner names the
//! operands left out. Without the operands the expression needs there is no result, and the
//! banner says so instead of a curve.

use ac2_proto::frame::{MathState, OperandStatus, ProtectionFlags};
use ac2_proto::model::{
    AverageMethod, MathConfig, MathDomain, MathExpr, MathOp, NamedOperand, Operand, PhaseBasis,
};

/// `power`, `complex`, `coherence-weighted`.
pub fn method_name(m: AverageMethod) -> &'static str {
    match m {
        AverageMethod::Power => "power",
        AverageMethod::Complex => "complex",
        AverageMethod::CoherenceWeighted => "coherence-weighted",
    }
}

/// `÷`, `×`, `+`, `−`.
pub fn op_symbol(op: MathOp) -> &'static str {
    match op {
        MathOp::Divide => "÷",
        MathOp::Multiply => "×",
        MathOp::Add => "+",
        MathOp::Subtract => "−",
    }
}

/// What the operator does in `domain`, for menus and captions: `relative (A ÷ B)`,
/// `level difference (A − B)` …
pub fn op_meaning(op: MathOp, domain: MathDomain) -> &'static str {
    match (op, domain) {
        (MathOp::Divide, _) => "relative",
        (MathOp::Multiply, _) => "cascade",
        (MathOp::Add, MathDomain::Transfer) => "summation (complex sum)",
        (MathOp::Subtract, MathDomain::Transfer) => "complex difference",
        (MathOp::Add, _) => "power sum",
        (MathOp::Subtract, _) => "level difference",
    }
}

/// The expression by name: `Main L ÷ Sub`, `average of Seat 1, Seat 2, Seat 3`.
pub fn expression(expr: &MathExpr, name_of: impl Fn(Operand) -> String) -> String {
    match expr {
        MathExpr::Binary { a, op, b } => {
            format!("{} {} {}", name_of(*a), op_symbol(*op), name_of(*b))
        }
        MathExpr::Average { of, .. } => format!(
            "average of {}",
            of.iter()
                .map(|o| name_of(*o))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The name a new math channel gets: the expression, `Main L ÷ Sub`; an average of
/// positions is `Average of 4`.
pub fn default_name(expr: &MathExpr, name_of: impl Fn(Operand) -> String) -> String {
    match expr {
        MathExpr::Binary { .. } => expression(expr, name_of),
        MathExpr::Average { of, .. } => format!("Average of {}", of.len()),
    }
}

/// Why an operand was left out, in the words of its own banner: `stopped`, `settling`,
/// `clip`, `no reference`, `check routing`, `no signal`, `does not combine`; `None` when it
/// went in.
pub fn left_out_reason(s: OperandStatus) -> Option<String> {
    match s {
        OperandStatus::Included => None,
        OperandStatus::Stopped => Some("stopped".into()),
        OperandStatus::Settling => Some("settling".into()),
        OperandStatus::Mismatch => {
            Some("does not combine (another level scale, grid or time base)".into())
        }
        OperandStatus::Refused { protection } => {
            let words: Vec<&str> = [
                (ProtectionFlags::CLIP, "clip"),
                (ProtectionFlags::NO_REFERENCE, "no reference"),
                (ProtectionFlags::CHECK_ROUTING, "check routing"),
                (ProtectionFlags::NO_SIGNAL, "no signal"),
            ]
            .into_iter()
            .filter(|(f, _)| protection.contains(*f))
            .map(|(_, w)| w)
            .collect();
            Some(if words.is_empty() {
                "refused".into()
            } else {
                words.join(", ")
            })
        }
    }
}

/// What a phase basis and coherence add to a transfer result's legend, if anything.
fn transfer_notes(expr: &MathExpr, phase: PhaseBasis) -> Vec<&'static str> {
    let mut notes = Vec::new();
    match phase {
        PhaseBasis::SharedTimeBase => {}
        PhaseBasis::OwnAlignments => notes.push("phase: own alignments"),
        PhaseBasis::NoPhase => notes.push("magnitude only"),
    }
    if let MathExpr::Binary {
        op: MathOp::Add | MathOp::Subtract,
        ..
    } = expr
    {
        // A sum's coherence would need the operands' cross-spectrum.
        notes.push("no coherence");
    }
    notes
}

/// How far apart the two operands of a transfer ÷, − or + arrive, A − B, by the delays
/// their phases are referred to (`delay_of`, seconds): `arrival Δ +3.7 µs · +1.3 mm @ 20 °C`.
/// What these operators show depends on that relative arrival — a ratio's phase slope, where
/// a difference or a sum cancels — and a few µs is a slope the phase pane hardly shows. Only
/// on a shared time base: operands each aligned on their own clock have no relative arrival,
/// and a cascade's delays add rather than compare.
pub fn arrival_note(
    expr: &MathExpr,
    phase: PhaseBasis,
    delay_of: impl Fn(Operand) -> Option<f64>,
    temp_c: f64,
) -> Option<String> {
    let MathExpr::Binary { a, op, b } = expr else {
        return None;
    };
    if *op == MathOp::Multiply || phase != PhaseBasis::SharedTimeBase {
        return None;
    }
    let (da, db) = (delay_of(*a)?, delay_of(*b)?);
    Some(format!(
        "arrival Δ {}",
        crate::readout::arrival_difference(da, db, temp_c)
    ))
}

/// A math channel's frame, as the display tells it.
#[derive(Clone, Debug, PartialEq)]
pub struct MathStatus {
    /// The expression by name.
    pub expression: String,
    /// Average method; `None` for a binary operator.
    pub method: Option<AverageMethod>,
    /// Operands that went into the frame.
    pub included: usize,
    /// Operands of the expression.
    pub operands: usize,
    /// `Seat 3: no signal`, one per operand left out, in expression order.
    pub left_out: Vec<String>,
    /// Legend notes: `phase: own alignments`, `no coherence` …
    pub notes: Vec<&'static str>,
    /// The operands' arrival difference ([`arrival_note`]), when the caller knows their delays.
    pub arrival: Option<String>,
}

impl MathStatus {
    /// From the channel's configuration and its frame's [`MathState`], operands named by
    /// `name_of`.
    pub fn new(c: &MathConfig, s: &MathState, name_of: impl Fn(Operand) -> String) -> Self {
        let notes = match c.domain {
            MathDomain::Transfer => transfer_notes(&c.expr, s.phase),
            MathDomain::Spectrum | MathDomain::Rta => match &c.expr {
                MathExpr::Binary { op, .. } => vec![op_meaning(*op, c.domain)],
                MathExpr::Average { .. } => Vec::new(),
            },
        };
        Self {
            expression: expression(&c.expr, &name_of),
            method: match &c.expr {
                MathExpr::Average { method, .. } => Some(*method),
                MathExpr::Binary { .. } => None,
            },
            included: s.included(),
            operands: s.operands.len(),
            left_out: s
                .operands
                .iter()
                .filter_map(|m| {
                    left_out_reason(m.status).map(|r| format!("{}: {r}", name_of(m.operand)))
                })
                .collect(),
            notes,
            arrival: None,
        }
    }

    /// This status with the operands' arrival difference ([`arrival_note`]) in its tag.
    pub fn with_arrival(
        mut self,
        c: &MathConfig,
        s: &MathState,
        delay_of: impl Fn(Operand) -> Option<f64>,
        temp_c: f64,
    ) -> Self {
        if c.domain == MathDomain::Transfer {
            self.arrival = arrival_note(&c.expr, s.phase, delay_of, temp_c);
        }
        self
    }

    /// Whether the frame carries a result at all.
    pub fn has_value(&self) -> bool {
        match self.method {
            Some(_) => self.included >= MathConfig::MIN_AVERAGE,
            None => self.included == self.operands,
        }
    }

    /// `4 positions` when every operand is in, else `3 of 4 positions`.
    pub fn count(&self) -> String {
        if self.included == self.operands {
            format!("{} positions", self.operands)
        } else {
            format!("{} of {} positions", self.included, self.operands)
        }
    }

    /// The legend tag: `3 of 4 positions · power avg`; `Main L + Sub · no coherence`;
    /// `Main L ÷ Sub · arrival Δ +3.7 µs · +1.3 mm @ 20 °C`.
    pub fn tag(&self) -> String {
        let head = match self.method {
            Some(m) => format!("{} · {} avg", self.count(), method_name(m)),
            None => self.expression.clone(),
        };
        std::iter::once(head.as_str())
            .chain(self.notes.iter().copied())
            .chain(self.arrival.as_deref())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// Banner text and detail when an operand is left out: an average's
    /// `AVERAGE · 3 OF 4 POSITIONS` (warning) or, below two, `NO AVERAGE · 1 OF 4
    /// POSITIONS` (fault); a binary operator's `NO RESULT · 1 OF 2 OPERANDS` (fault). The
    /// detail names what was left out and why. `None` when every operand is in.
    pub fn banner(&self, name: &str) -> Option<(bool, String, String)> {
        if self.left_out.is_empty() {
            return None;
        }
        let fault = !self.has_value();
        let text = match self.method {
            Some(_) => format!(
                "{} · {} OF {} POSITIONS",
                if fault { "NO AVERAGE" } else { "AVERAGE" },
                self.included,
                self.operands
            ),
            None => format!(
                "NO RESULT · {} OF {} OPERANDS",
                self.included, self.operands
            ),
        };
        Some((
            fault,
            text,
            format!("{name}: left out {}", self.left_out.join(" · ")),
        ))
    }
}

/// The legend note of a stored math capture: `Main L ÷ Sub`, `4 positions · power avg`,
/// with what its phase means.
pub fn capture_note(
    expr: &MathExpr,
    operands: &[NamedOperand],
    phase: PhaseBasis,
    transfer: bool,
) -> String {
    let name_of = |o: Operand| {
        operands
            .iter()
            .find(|n| n.operand == o)
            .map_or_else(|| "?".to_owned(), |n| n.name.clone())
    };
    let head = match expr {
        MathExpr::Binary { .. } => expression(expr, name_of),
        MathExpr::Average { method, .. } => {
            format!(
                "{} positions · {} avg",
                operands.len(),
                method_name(*method)
            )
        }
    };
    let notes = if transfer {
        transfer_notes(expr, phase)
    } else {
        Vec::new()
    };
    std::iter::once(head.as_str())
        .chain(notes)
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::frame::OperandState;
    use ac2_proto::units::{MeasId, TraceId};

    fn meas(m: u32) -> Operand {
        Operand::Meas { meas: MeasId(m) }
    }

    fn name(o: Operand) -> String {
        match o {
            Operand::Meas { meas } => format!("Seat {}", meas.0),
            Operand::Trace { trace } => format!("S{}", trace.0),
        }
    }

    fn avg(statuses: &[OperandStatus]) -> (MathConfig, MathState) {
        let of: Vec<Operand> = (1..=statuses.len() as u32).map(meas).collect();
        let c = MathConfig::power_average(
            ac2_proto::model::TraceOwner::Imported,
            MathDomain::Transfer,
            of.clone(),
        );
        let s = MathState {
            operands: of
                .into_iter()
                .zip(statuses)
                .map(|(operand, status)| OperandState {
                    operand,
                    status: *status,
                })
                .collect(),
            phase: PhaseBasis::SharedTimeBase,
        };
        (c, s)
    }

    fn status(c: &(MathConfig, MathState)) -> MathStatus {
        MathStatus::new(&c.0, &c.1, name)
    }

    #[test]
    fn every_position_in() {
        let s = status(&avg(&[OperandStatus::Included; 4]));
        assert_eq!(s.tag(), "4 positions · power avg");
        assert_eq!(s.banner("Audience"), None);
        assert_eq!(s.expression, "average of Seat 1, Seat 2, Seat 3, Seat 4");
    }

    #[test]
    fn positions_left_out_are_named_with_their_reason() {
        let s = status(&avg(&[
            OperandStatus::Included,
            OperandStatus::Included,
            OperandStatus::Refused {
                protection: ProtectionFlags::NO_SIGNAL.with(ProtectionFlags::CLIP),
            },
            OperandStatus::Stopped,
        ]));
        assert_eq!(s.tag(), "2 of 4 positions · power avg");
        assert_eq!(
            s.banner("Audience"),
            Some((
                false,
                "AVERAGE · 2 OF 4 POSITIONS".into(),
                "Audience: left out Seat 3: clip, no signal · Seat 4: stopped".into()
            ))
        );
    }

    #[test]
    fn one_position_is_no_average() {
        let s = status(&avg(&[OperandStatus::Included, OperandStatus::Settling]));
        assert!(!s.has_value());
        let (fault, text, detail) = s.banner("Audience").unwrap_or_default();
        assert!(fault);
        assert_eq!(text, "NO AVERAGE · 1 OF 2 POSITIONS");
        assert_eq!(detail, "Audience: left out Seat 2: settling");
    }

    /// A binary operator's legend is its expression with what its phase and coherence
    /// mean; half of it is no result.
    #[test]
    fn binary_tags_and_banners() {
        let state = |b: OperandStatus, phase| MathState {
            operands: vec![
                OperandState {
                    operand: meas(1),
                    status: OperandStatus::Included,
                },
                OperandState {
                    operand: Operand::Trace { trace: TraceId(2) },
                    status: b,
                },
            ],
            phase,
        };
        let cfg = |op| {
            MathConfig::of(
                ac2_proto::model::TraceOwner::Imported,
                MathDomain::Transfer,
                MathExpr::Binary {
                    a: meas(1),
                    op,
                    b: Operand::Trace { trace: TraceId(2) },
                },
            )
        };
        let s = MathStatus::new(
            &cfg(MathOp::Divide),
            &state(OperandStatus::Included, PhaseBasis::SharedTimeBase),
            name,
        );
        assert_eq!(s.tag(), "Seat 1 ÷ S2");
        let s = MathStatus::new(
            &cfg(MathOp::Divide),
            &state(OperandStatus::Included, PhaseBasis::OwnAlignments),
            name,
        );
        assert_eq!(s.tag(), "Seat 1 ÷ S2 · phase: own alignments");
        let s = MathStatus::new(
            &cfg(MathOp::Add),
            &state(OperandStatus::Included, PhaseBasis::SharedTimeBase),
            name,
        );
        assert_eq!(s.tag(), "Seat 1 + S2 · no coherence");
        let s = MathStatus::new(
            &cfg(MathOp::Add),
            &state(OperandStatus::Mismatch, PhaseBasis::NoPhase),
            name,
        );
        assert!(!s.has_value());
        let (fault, text, detail) = s.banner("Sum").unwrap_or_default();
        assert!(fault);
        assert_eq!(text, "NO RESULT · 1 OF 2 OPERANDS");
        assert_eq!(
            detail,
            "Sum: left out S2: does not combine (another level scale, grid or time base)"
        );
        let spec = MathConfig::of(
            ac2_proto::model::TraceOwner::Imported,
            MathDomain::Spectrum,
            MathExpr::Binary {
                a: meas(1),
                op: MathOp::Subtract,
                b: meas(2),
            },
        );
        let both = MathState {
            operands: vec![
                OperandState {
                    operand: meas(1),
                    status: OperandStatus::Included,
                },
                OperandState {
                    operand: meas(2),
                    status: OperandStatus::Included,
                },
            ],
            phase: PhaseBasis::NoPhase,
        };
        assert_eq!(
            MathStatus::new(&spec, &both, name).tag(),
            "Seat 1 − Seat 2 · level difference"
        );
    }

    /// A ratio, difference or sum of two operands on one time base says how far apart they
    /// arrive, to 0.1 µs; a cascade, operands on their own clocks, or an operand without a
    /// delay say nothing.
    #[test]
    fn binary_transfer_carries_the_arrival_difference() {
        let cfg = |op, b| {
            MathConfig::of(
                ac2_proto::model::TraceOwner::Imported,
                MathDomain::Transfer,
                MathExpr::Binary { a: meas(1), op, b },
            )
        };
        let state = |b, phase| MathState {
            operands: [meas(1), b]
                .into_iter()
                .map(|operand| OperandState {
                    operand,
                    status: OperandStatus::Included,
                })
                .collect(),
            phase,
        };
        let s2 = Operand::Trace { trace: TraceId(2) };
        // Seat 1 at 600.37 samples, S2 at 600 (48 kHz).
        let delay_of = |o: Operand| match o {
            Operand::Meas { .. } => Some(600.37 / 48_000.0),
            Operand::Trace { .. } => Some(600.0 / 48_000.0),
        };
        let tag = |op, b, phase: PhaseBasis| {
            let (c, s) = (cfg(op, b), state(b, phase));
            MathStatus::new(&c, &s, name)
                .with_arrival(&c, &s, delay_of, 20.0)
                .tag()
        };
        assert_eq!(
            tag(MathOp::Divide, s2, PhaseBasis::SharedTimeBase),
            "Seat 1 ÷ S2 · arrival Δ +7.7 µs · +2.6 mm @ 20 °C"
        );
        assert_eq!(
            tag(MathOp::Subtract, s2, PhaseBasis::SharedTimeBase),
            "Seat 1 − S2 · no coherence · arrival Δ +7.7 µs · +2.6 mm @ 20 °C"
        );
        assert_eq!(
            tag(MathOp::Divide, s2, PhaseBasis::OwnAlignments),
            "Seat 1 ÷ S2 · phase: own alignments"
        );
        assert_eq!(
            tag(MathOp::Multiply, s2, PhaseBasis::SharedTimeBase),
            "Seat 1 × S2"
        );
        let (c, s) = (
            cfg(MathOp::Divide, s2),
            state(s2, PhaseBasis::SharedTimeBase),
        );
        let no_delay = MathStatus::new(&c, &s, name).with_arrival(
            &c,
            &s,
            |o| matches!(o, Operand::Meas { .. }).then_some(0.0125),
            20.0,
        );
        assert_eq!(no_delay.tag(), "Seat 1 ÷ S2");
        // Levels have no arrival.
        let spec = MathConfig::of(
            ac2_proto::model::TraceOwner::Imported,
            MathDomain::Spectrum,
            MathExpr::Binary {
                a: meas(1),
                op: MathOp::Subtract,
                b: s2,
            },
        );
        let s = state(s2, PhaseBasis::NoPhase);
        assert_eq!(
            MathStatus::new(&spec, &s, name)
                .with_arrival(&spec, &s, delay_of, 20.0)
                .tag(),
            "Seat 1 − S2 · level difference"
        );
    }

    #[test]
    fn names() {
        let e = MathExpr::Binary {
            a: meas(1),
            op: MathOp::Divide,
            b: meas(2),
        };
        assert_eq!(default_name(&e, name), "Seat 1 ÷ Seat 2");
        let e = MathExpr::Average {
            of: vec![meas(1), meas(2), meas(3)],
            method: AverageMethod::Power,
        };
        assert_eq!(default_name(&e, name), "Average of 3");
        let named: Vec<NamedOperand> = [1, 2]
            .iter()
            .map(|m| NamedOperand {
                operand: meas(*m),
                name: format!("Seat {m}"),
            })
            .collect();
        assert_eq!(
            capture_note(&e, &named, PhaseBasis::SharedTimeBase, true),
            "2 positions · power avg"
        );
    }
}
