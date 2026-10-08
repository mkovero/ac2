//! The math channel dialog: A, the operator and B picked by name from the live
//! measurements and stored traces that can be combined, or the operands of an average
//! ticked; the method, phase reference and smoothing where they apply; the name prefilled
//! from the expression. The same dialog edits an existing channel. Pure data, like the
//! other dialogs ([`crate::forms`]); the daemon does the maths and refuses what it cannot
//! combine, saying why.

use ac2_proto::model::{
    AverageMethod, MathConfig, MathDomain, MathExpr, MathOp, MathReference, MeasConfig, MeasKind,
    Measurement, Operand, Smoothing, SmoothingFraction, SmoothingMode, TraceKind, TraceMeta,
    TraceOwner,
};
use ac2_proto::units::MeasId;

use crate::forms::{Field, FieldId, Form, FormKind, Value};

/// Something a math channel can combine.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub operand: Operand,
    /// Its name, as the expression and the channel's name use it.
    pub name: String,
    /// As the dialog lists it: a stored trace says so.
    pub label: String,
    pub domain: MathDomain,
}

impl Candidate {
    /// Every live transfer, spectrum and RTA measurement (not a math channel: capture one
    /// to use it), then every stored trace of those kinds, in list order — `owner`'s live
    /// curve and traces first: the channel is made on that measurement.
    pub fn all(
        ms: &[&Measurement],
        traces: &[&TraceMeta],
        owner: Option<TraceOwner>,
    ) -> Vec<Candidate> {
        let mut v = Self::listed(ms, traces);
        if let Some(owner) = owner {
            let mine = |c: &Candidate| match c.operand {
                Operand::Meas { meas } => owner == TraceOwner::Meas { meas },
                Operand::Trace { trace } => traces
                    .iter()
                    .any(|t| t.id == trace && t.edit.owner == owner),
            };
            // Stable: the list's order within each part.
            v.sort_by_key(|c| !mine(c));
        }
        v
    }

    fn listed(ms: &[&Measurement], traces: &[&TraceMeta]) -> Vec<Candidate> {
        let live = ms.iter().filter_map(|m| {
            let domain = match m.config.kind {
                MeasKind::Transfer { .. } => MathDomain::Transfer,
                MeasKind::Spectrum { .. } => MathDomain::Spectrum,
                MeasKind::Rta { .. } => MathDomain::Rta,
                MeasKind::Spl { .. } | MeasKind::Math { .. } | MeasKind::Sweep { .. } => {
                    return None;
                }
            };
            Some(Candidate {
                operand: Operand::Meas { meas: m.id },
                name: m.config.name.clone(),
                label: format!("{} (live)", m.config.name),
                domain,
            })
        });
        let stored = traces.iter().map(|t| Candidate {
            operand: Operand::Trace { trace: t.id },
            name: t.edit.name.clone(),
            label: match t.edit.slot {
                Some(s) => format!("{} (stored, S{s})", t.edit.name),
                None => format!("{} (stored)", t.edit.name),
            },
            domain: match t.kind {
                TraceKind::Transfer | TraceKind::Sweep | TraceKind::Target => MathDomain::Transfer,
                TraceKind::Spectrum { .. } => MathDomain::Spectrum,
                TraceKind::Rta { .. } => MathDomain::Rta,
            },
        });
        live.chain(stored).collect()
    }
}

/// What the dialog's operator row offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Binary(MathOp),
    Average,
}

/// The operators of `domain`, as the row lists them.
fn operators(domain: MathDomain) -> Vec<(&'static str, Choice)> {
    match domain {
        MathDomain::Transfer => vec![
            ("÷  A relative to B", Choice::Binary(MathOp::Divide)),
            ("×  A cascaded with B", Choice::Binary(MathOp::Multiply)),
            (
                "+  sum (as A and B add acoustically)",
                Choice::Binary(MathOp::Add),
            ),
            ("−  complex difference", Choice::Binary(MathOp::Subtract)),
            ("average of several", Choice::Average),
        ],
        MathDomain::Spectrum | MathDomain::Rta => vec![
            ("−  level difference (dB)", Choice::Binary(MathOp::Subtract)),
            ("+  power sum", Choice::Binary(MathOp::Add)),
            ("average of several (power)", Choice::Average),
        ],
    }
}

/// An average operand row (index 0: in the average).
const MEMBER: [&str; 2] = ["in the average", "left out"];

/// Average methods of transfer math, as `ac2 math new --method` (index 0: power, its
/// default).
const METHODS: [(&str, AverageMethod); 3] = [
    (
        "power (level over the positions, no cancellation)",
        AverageMethod::Power,
    ),
    (
        "complex (as summed at one point: arrivals cancel)",
        AverageMethod::Complex,
    ),
    (
        "coherence-weighted (cleaner positions count more)",
        AverageMethod::CoherenceWeighted,
    ),
];

/// Smoothing of the result (index 0: none).
const SMOOTHING: [(&str, Option<SmoothingFraction>); 6] = [
    ("off", None),
    ("1/3 octave", Some(SmoothingFraction::Third)),
    ("1/6 octave", Some(SmoothingFraction::Sixth)),
    ("1/12 octave", Some(SmoothingFraction::Twelfth)),
    ("1/24 octave", Some(SmoothingFraction::TwentyFourth)),
    ("1/48 octave", Some(SmoothingFraction::FortyEighth)),
];

/// The dialog's own state beside its fields.
#[derive(Clone, Debug, PartialEq)]
pub struct MathForm {
    pub candidates: Vec<Candidate>,
    /// The channel being edited; `None` for a new one.
    pub edit: Option<MeasId>,
    /// Where the channel is listed: the measurement selected when it was made.
    pub owner: TraceOwner,
    /// The name the dialog last filled in: while the name field still holds it, it follows
    /// the expression.
    auto_name: String,
    /// The smoothing mode kept from the channel being edited.
    mode: SmoothingMode,
}

/// What the rows currently say.
#[derive(Clone, Debug, PartialEq)]
struct Picks {
    domain: MathDomain,
    a: Option<Operand>,
    choice: Choice,
    b: Option<Operand>,
    /// Operands ticked into an average, in candidate order.
    of: Vec<Operand>,
    method: AverageMethod,
    reference: Option<Operand>,
    smoothing: Option<SmoothingFraction>,
    name: String,
}

impl MathForm {
    fn find(&self, o: Operand) -> Option<&Candidate> {
        self.candidates.iter().find(|c| c.operand == o)
    }

    fn name_of(&self, o: Operand) -> String {
        self.find(o).map_or_else(|| "?".into(), |c| c.name.clone())
    }

    fn domain_of(&self, o: Option<Operand>) -> Option<MathDomain> {
        o.and_then(|o| self.find(o)).map(|c| c.domain)
    }

    /// B's choices: the other candidates of A's kind.
    fn bs(&self, domain: MathDomain, a: Option<Operand>) -> Vec<&Candidate> {
        self.candidates
            .iter()
            .filter(|c| c.domain == domain && Some(c.operand) != a)
            .collect()
    }

    /// An average's rows: the candidates of its kind, as many as an average takes.
    fn members(&self, domain: MathDomain) -> Vec<&Candidate> {
        self.candidates
            .iter()
            .filter(|c| c.domain == domain)
            .take(MathConfig::MAX_AVERAGE)
            .collect()
    }
}

impl Form {
    /// The new-math-channel dialog over `candidates`; `first` is preselected as A (the
    /// focused measurement or the selected trace). `None` with fewer than two candidates of
    /// one kind.
    pub fn math(
        candidates: Vec<Candidate>,
        first: Option<Operand>,
        owner: TraceOwner,
    ) -> Result<Form, String> {
        let pairable = |d: MathDomain| candidates.iter().filter(|c| c.domain == d).count() >= 2;
        let a = first
            .and_then(|o| candidates.iter().find(|c| c.operand == o))
            .filter(|c| pairable(c.domain))
            .or_else(|| candidates.iter().find(|c| pairable(c.domain)))
            .ok_or(
                "a math channel needs two transfer functions, spectra or RTAs of one kind, live \
                 or stored: make a measurement or capture one (Ctrl+1 … 9) first",
            )?;
        let b = candidates
            .iter()
            .find(|c| c.domain == a.domain && c.operand != a.operand)
            .map(|c| c.operand);
        let picks = Picks {
            domain: a.domain,
            a: Some(a.operand),
            choice: Choice::Binary(MathOp::Divide),
            b,
            of: Vec::new(),
            method: AverageMethod::Power,
            reference: None,
            smoothing: None,
            name: String::new(),
        };
        let mut f = Form::new(FormKind::Math, Vec::new());
        f.math = Some(Box::new(MathForm {
            candidates,
            edit: None,
            owner,
            auto_name: String::new(),
            mode: SmoothingMode::MagnitudePhase,
        }));
        f.lay_out(picks, Some(FieldId::OperandA));
        Ok(f)
    }

    /// The dialog editing math channel `m`, its rows as configured.
    pub fn edit_math(m: &Measurement, candidates: Vec<Candidate>) -> Result<Form, String> {
        let MeasKind::Math { config } = &m.config.kind else {
            return Err(format!("{} is not a math channel", m.config.name));
        };
        let (choice, a, b, of, method) = match &config.expr {
            MathExpr::Binary { a, op, b } => (
                Choice::Binary(*op),
                Some(*a),
                Some(*b),
                Vec::new(),
                AverageMethod::Power,
            ),
            MathExpr::Average { of, method } => (
                Choice::Average,
                of.first().copied(),
                None,
                of.clone(),
                *method,
            ),
        };
        let picks = Picks {
            domain: config.domain,
            a,
            choice,
            b,
            of,
            method,
            reference: match config.reference {
                MathReference::Operand { operand } => Some(operand),
                MathReference::Fixed { .. } => None,
            },
            smoothing: config.smoothing.map(|s| s.fraction),
            name: m.config.name.clone(),
        };
        let mut f = Form::new(FormKind::MathEdit, Vec::new());
        f.math = Some(Box::new(MathForm {
            candidates,
            edit: Some(m.id),
            owner: config.owner,
            // An edited channel keeps its name unless the operator types another.
            auto_name: String::new(),
            mode: config
                .smoothing
                .map_or(SmoothingMode::MagnitudePhase, |s| s.mode),
        }));
        f.lay_out(picks, Some(FieldId::OperandA));
        Ok(f)
    }

    /// What the math rows say now.
    fn picks(&self) -> Option<Picks> {
        let m = self.math.as_ref()?;
        let choice_of = |id: FieldId| -> Option<(usize, usize)> {
            match self.fields.iter().find(|f| f.id == id).map(|f| &f.value) {
                Some(Value::Choice { options, index }) => Some((*index, options.len())),
                _ => None,
            }
        };
        let all: Vec<Operand> = m.candidates.iter().map(|c| c.operand).collect();
        let of: Vec<Operand> = self
            .fields
            .iter()
            .filter_map(|f| match (f.id, &f.value) {
                (FieldId::Member(o), Value::Choice { index: 0, .. }) => Some(o),
                _ => None,
            })
            .collect();
        let a = choice_of(FieldId::OperandA)
            .and_then(|(i, _)| all.get(i).copied())
            .or_else(|| of.first().copied());
        // An average's kind is its rows', ticked or not.
        let domain = m
            .domain_of(a)
            .or_else(|| {
                self.fields.iter().find_map(|f| match f.id {
                    FieldId::Member(o) => m.domain_of(Some(o)),
                    _ => None,
                })
            })
            .unwrap_or(MathDomain::Transfer);
        let ops = operators(domain);
        let choice = choice_of(FieldId::Operator)
            .and_then(|(i, _)| ops.get(i).map(|o| o.1))
            .unwrap_or(Choice::Binary(MathOp::Divide));
        let b = choice_of(FieldId::OperandB)
            .and_then(|(i, _)| m.bs(domain, a).get(i).map(|c| c.operand));
        let in_expr: Vec<Operand> = match choice {
            Choice::Binary(_) => [a, b].into_iter().flatten().collect(),
            Choice::Average => of.clone(),
        };
        let reference = choice_of(FieldId::PhaseRef).and_then(|(i, _)| in_expr.get(i).copied());
        Some(Picks {
            domain,
            a,
            choice,
            b,
            of,
            method: choice_of(FieldId::Method)
                .and_then(|(i, _)| METHODS.get(i).map(|m| m.1))
                .unwrap_or(AverageMethod::Power),
            reference,
            smoothing: choice_of(FieldId::Smoothing)
                .and_then(|(i, _)| SMOOTHING.get(i).and_then(|s| s.1)),
            name: self.text(FieldId::Name).to_owned(),
        })
    }

    /// Rebuilds the rows from `p`: the operators of A's kind, B among A's kind, the
    /// average's ticks, and only the method, reference and smoothing rows that apply. The
    /// name follows the expression until the operator types one. `focus` keeps that row
    /// focused.
    fn lay_out(&mut self, mut p: Picks, focus: Option<FieldId>) {
        let Some(m) = self.math.take() else {
            return;
        };
        let mut m = *m;
        // A changed to another kind takes that kind.
        let domain = m.domain_of(p.a).unwrap_or(p.domain);
        let ops = operators(domain);
        if !ops.iter().any(|o| o.1 == p.choice) {
            p.choice = ops[0].1;
        }
        let mut fields = Vec::new();
        let entering_average = !self
            .fields
            .iter()
            .any(|f| matches!(f.id, FieldId::Member(_)));
        let op_field = Field::choice(
            FieldId::Operator,
            "Operator",
            &ops.iter().map(|o| o.0).collect::<Vec<_>>(),
            ops.iter().position(|o| o.1 == p.choice).unwrap_or(0),
        );
        match p.choice {
            Choice::Binary(_) => {
                let all: Vec<&Candidate> = m.candidates.iter().collect();
                let ai =
                    p.a.and_then(|a| all.iter().position(|c| c.operand == a))
                        .unwrap_or(0);
                p.a = all.get(ai).map(|c| c.operand);
                fields.push(Field::choice(
                    FieldId::OperandA,
                    "A",
                    &all.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
                    ai,
                ));
                fields.push(op_field);
                let bs = m.bs(domain, p.a);
                let bi =
                    p.b.and_then(|b| bs.iter().position(|c| c.operand == b))
                        .unwrap_or(0);
                p.b = bs.get(bi).map(|c| c.operand);
                let mut b = Field::choice(
                    FieldId::OperandB,
                    "B",
                    &bs.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
                    bi,
                );
                if bs.is_empty() {
                    b.hint = "nothing of A's kind to combine with".into();
                }
                fields.push(b);
            }
            Choice::Average => {
                fields.push(op_field);
                let members = m.members(domain);
                // An average just chosen takes every position of A's kind; one already
                // shown (or edited) keeps its ticks.
                if entering_average && p.of.is_empty() {
                    p.of = members.iter().map(|c| c.operand).collect();
                }
                for c in &members {
                    let ticked = p.of.contains(&c.operand);
                    fields.push(Field::choice(
                        FieldId::Member(c.operand),
                        c.label.clone(),
                        &MEMBER,
                        usize::from(!ticked),
                    ));
                }
                p.of.retain(|o| members.iter().any(|c| c.operand == *o));
                if p.a.is_none() {
                    p.a = p.of.first().copied();
                }
            }
        }
        let in_expr: Vec<Operand> = match p.choice {
            Choice::Binary(_) => [p.a, p.b].into_iter().flatten().collect(),
            Choice::Average => p.of.clone(),
        };
        let transfer = domain == MathDomain::Transfer;
        if transfer && p.choice == Choice::Average {
            fields.push(Field::choice(
                FieldId::Method,
                "Method",
                &METHODS.map(|x| x.0),
                METHODS.iter().position(|x| x.1 == p.method).unwrap_or(0),
            ));
        }
        let rereferred = matches!(
            p.choice,
            Choice::Average | Choice::Binary(MathOp::Add | MathOp::Subtract)
        );
        if transfer && rereferred && !in_expr.is_empty() {
            let names: Vec<String> = in_expr
                .iter()
                .map(|o| format!("{}'s delay", m.name_of(*o)))
                .collect();
            let ri = p
                .reference
                .and_then(|r| in_expr.iter().position(|o| *o == r))
                .unwrap_or(0);
            let mut f = Field::choice(
                FieldId::PhaseRef,
                "Phase reference",
                &names.iter().map(String::as_str).collect::<Vec<_>>(),
                ri,
            );
            f.hint = "the arrival the result's phase is relative to".into();
            fields.push(f);
        }
        if domain != MathDomain::Rta {
            fields.push(Field::choice(
                FieldId::Smoothing,
                "Smoothing",
                &SMOOTHING.map(|s| s.0),
                SMOOTHING
                    .iter()
                    .position(|s| s.1 == p.smoothing)
                    .unwrap_or(0),
            ));
        }
        let expr = match p.choice {
            Choice::Binary(op) => p.a.zip(p.b).map(|(a, b)| MathExpr::Binary { a, op, b }),
            Choice::Average => Some(MathExpr::Average {
                of: p.of.clone(),
                method: p.method,
            }),
        };
        let auto = expr
            .as_ref()
            .map(|e| ac2_scene::math::default_name(e, |o| m.name_of(o)))
            .unwrap_or_default();
        let name = if p.name.is_empty() || p.name == m.auto_name {
            m.auto_name = auto.clone();
            auto
        } else {
            p.name.clone()
        };
        fields.push(Field::text(FieldId::Name, "Name", name, ""));
        let focus = focus.or_else(|| self.fields.get(self.focus).map(|f| f.id));
        self.fields = fields;
        self.focus = focus
            .and_then(|id| self.fields.iter().position(|f| f.id == id))
            .unwrap_or(0)
            .min(self.fields.len().saturating_sub(1));
        self.math = Some(Box::new(m));
    }

    /// After ←/→: the rows that depend on what changed are laid out again.
    pub(crate) fn math_changed(&mut self) {
        if let Some(p) = self.picks() {
            self.lay_out(p, None);
        }
    }

    /// The `meas.create` / `meas.update` configuration of the math dialog.
    pub(crate) fn math_config(&self) -> Result<MeasConfig, String> {
        let m = self.math.as_ref().ok_or("not a math channel dialog")?;
        let p = self.picks().ok_or("not a math channel dialog")?;
        let a = p.a.or_else(|| p.of.first().copied());
        let domain = m.domain_of(a).ok_or("choose the operands")?;
        let expr = match p.choice {
            Choice::Binary(op) => {
                let (Some(a), Some(b)) = (p.a, p.b) else {
                    return Err(format!(
                        "choose B: another {} to combine with {}",
                        match domain {
                            MathDomain::Transfer => "transfer function",
                            MathDomain::Spectrum => "spectrum",
                            MathDomain::Rta => "RTA",
                        },
                        p.a.map_or_else(|| "A".into(), |a| m.name_of(a))
                    ));
                };
                MathExpr::Binary { a, op, b }
            }
            Choice::Average => {
                if p.of.len() < MathConfig::MIN_AVERAGE {
                    return Err(format!(
                        "an average needs at least {} operands: put them in with ←/→",
                        MathConfig::MIN_AVERAGE
                    ));
                }
                MathExpr::Average {
                    of: p.of.clone(),
                    method: if domain == MathDomain::Transfer {
                        p.method
                    } else {
                        AverageMethod::Power
                    },
                }
            }
        };
        let mut config = MathConfig::of(m.owner, domain, expr);
        if let Some(r) = p.reference {
            config.reference = MathReference::Operand { operand: r };
        }
        config.smoothing = p.smoothing.map(|fraction| Smoothing {
            fraction,
            mode: if domain == MathDomain::Transfer {
                m.mode
            } else {
                SmoothingMode::Magnitude
            },
        });
        let name = p.name.trim();
        if name.is_empty() {
            return Err("type a name".into());
        }
        Ok(MeasConfig {
            name: name.to_owned(),
            kind: MeasKind::Math { config },
        })
    }

    /// The channel this dialog edits, if it edits one.
    pub fn math_edit(&self) -> Option<MeasId> {
        self.math.as_ref().and_then(|m| m.edit)
    }

    /// Picks candidate `o` in operand row `id` (tests).
    pub fn pick_operand(&mut self, id: FieldId, o: Operand) -> bool {
        let Some(m) = self.math.as_ref() else {
            return false;
        };
        let Some(p) = self.picks() else {
            return false;
        };
        let list: Vec<Operand> = match id {
            FieldId::OperandB => m.bs(p.domain, p.a).iter().map(|c| c.operand).collect(),
            _ => m.candidates.iter().map(|c| c.operand).collect(),
        };
        let Some(i) = list.iter().position(|x| *x == o) else {
            return false;
        };
        if let Some(Value::Choice { index, .. }) = self
            .fields
            .iter_mut()
            .find(|f| f.id == id)
            .map(|f| &mut f.value)
        {
            *index = i;
            self.math_changed();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{MeasConfig, SpectrumConfig, TransferConfig};
    use ac2_proto::units::{Rev, TraceId};

    fn meas(id: u32, name: &str, kind: MeasKind) -> Measurement {
        Measurement {
            id: MeasId(id),
            config: MeasConfig {
                name: name.into(),
                kind,
            },
            config_rev: Rev(1),
            running: true,
            delay: None,
            grid_id: None,
        }
    }

    fn tf(id: u32, name: &str) -> Measurement {
        meas(
            id,
            name,
            MeasKind::Transfer {
                config: TransferConfig::with_inputs(0, 1),
            },
        )
    }

    fn spec(id: u32, name: &str) -> Measurement {
        meas(
            id,
            name,
            MeasKind::Spectrum {
                config: SpectrumConfig::on_input(1),
            },
        )
    }

    fn candidates() -> Vec<Candidate> {
        let ms = [
            tf(1, "Main L"),
            tf(2, "Sub"),
            spec(3, "Spec 1"),
            spec(4, "Spec 2"),
        ];
        let refs: Vec<&Measurement> = ms.iter().collect();
        Candidate::all(&refs, &[], None)
    }

    fn focus(f: &mut Form, id: FieldId) {
        let i = f.fields.iter().position(|x| x.id == id).unwrap_or(0);
        f.focus_field(i);
    }

    fn math_of(f: &Form) -> MathConfig {
        match f.math_config().map(|c| c.kind) {
            Ok(MeasKind::Math { config }) => config,
            other => panic!("{other:?}"),
        }
    }

    /// A ÷ B by default, named after itself; B follows A's kind.
    #[test]
    fn defaults_and_names() {
        let f =
            Form::math(candidates(), None, TraceOwner::Imported).unwrap_or_else(|e| panic!("{e}"));
        let c = f.math_config().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.name, "Main L ÷ Sub");
        let MeasKind::Math { config } = c.kind else {
            panic!()
        };
        assert_eq!(config.domain, MathDomain::Transfer);
        assert_eq!(
            config.expr,
            MathExpr::Binary {
                a: Operand::Meas { meas: MeasId(1) },
                op: MathOp::Divide,
                b: Operand::Meas { meas: MeasId(2) },
            }
        );
        // A spectrum as A: its operators are − and +, B a spectrum.
        let mut f = Form::math(
            candidates(),
            Some(Operand::Meas { meas: MeasId(3) }),
            TraceOwner::Imported,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let c = math_of(&f);
        assert_eq!(c.domain, MathDomain::Spectrum);
        assert_eq!(
            c.expr,
            MathExpr::Binary {
                a: Operand::Meas { meas: MeasId(3) },
                op: MathOp::Subtract,
                b: Operand::Meas { meas: MeasId(4) },
            }
        );
        assert_eq!(f.text(FieldId::Name), "Spec 1 − Spec 2");
        // The operator steps on to +: the name follows.
        focus(&mut f, FieldId::Operator);
        f.cycle(1);
        assert_eq!(f.text(FieldId::Name), "Spec 1 + Spec 2");
        // A typed name stays.
        f.set_text(FieldId::Name, "Sum");
        f.cycle(-1);
        assert_eq!(f.text(FieldId::Name), "Sum");
    }

    /// The average ticks every operand of A's kind; leaving all but one out is refused.
    #[test]
    fn average_ticks() {
        let mut f =
            Form::math(candidates(), None, TraceOwner::Imported).unwrap_or_else(|e| panic!("{e}"));
        focus(&mut f, FieldId::Operator);
        f.cycle(4);
        let c = math_of(&f);
        assert_eq!(
            c.expr,
            MathExpr::Average {
                of: vec![
                    Operand::Meas { meas: MeasId(1) },
                    Operand::Meas { meas: MeasId(2) }
                ],
                method: AverageMethod::Power,
            }
        );
        assert!(f.fields.iter().any(|x| x.id == FieldId::Method));
        assert!(f.fields.iter().any(|x| x.id == FieldId::PhaseRef));
        assert_eq!(f.text(FieldId::Name), "Average of 2");
        focus(&mut f, FieldId::Member(Operand::Meas { meas: MeasId(2) }));
        f.cycle(1);
        assert!(f.math_config().is_err());
    }

    /// Editing keeps the channel's name and expression.
    #[test]
    fn edit_prefills() {
        let mut m = tf(9, "Main + Sub");
        let mut c = MathConfig::of(
            TraceOwner::Meas { meas: MeasId(1) },
            MathDomain::Transfer,
            MathExpr::Binary {
                a: Operand::Meas { meas: MeasId(1) },
                op: MathOp::Add,
                b: Operand::Trace { trace: TraceId(5) },
            },
        );
        c.reference = MathReference::Operand {
            operand: Operand::Trace { trace: TraceId(5) },
        };
        m.config.kind = MeasKind::Math { config: c.clone() };
        let ms = [tf(1, "Main L"), tf(2, "Sub")];
        let refs: Vec<&Measurement> = ms.iter().collect();
        let mut cands = Candidate::all(&refs, &[], None);
        cands.push(Candidate {
            operand: Operand::Trace { trace: TraceId(5) },
            name: "Sub alone".into(),
            label: "Sub alone (stored)".into(),
            domain: MathDomain::Transfer,
        });
        let f = Form::edit_math(&m, cands).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(f.math_edit(), Some(MeasId(9)));
        let got = f.math_config().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(got.name, "Main + Sub");
        assert_eq!(got.kind, MeasKind::Math { config: c });
    }
}
