use super::*;
use ac2_proto::model::{MeasConfig, SpectrumConfig, TransferConfig};
use ac2_proto::units::{Rev, TraceId};

fn meas(id: u32, name: &str, kind: MeasKind) -> Measurement {
    Measurement {
        id: MeasId(id),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: name.into(),
            kind,
        },
        config_rev: Rev(1),
        running: true,
        delay: None,
        grid_id: None,
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: ac2_proto::units::Db(0.0),
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
    let f = Form::math(candidates(), None, TraceOwner::Imported).expect("two candidates");
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
    .expect("two candidates");
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
    let mut f = Form::math(candidates(), None, TraceOwner::Imported).expect("two candidates");
    focus(&mut f, FieldId::Operator);
    f.cycle(5);
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

/// A spectrum × or ÷ takes B among the stored transfer traces (a speaker's IR): a level
/// through a filter, or with it removed.
#[test]
fn a_level_goes_through_a_stored_transfer_trace() {
    let mut cands = candidates();
    cands.push(Candidate {
        operand: Operand::Trace { trace: TraceId(7) },
        name: "speaker".into(),
        label: "speaker (stored)".into(),
        domain: MathDomain::Transfer,
    });
    let mut f = Form::math(
        cands,
        Some(Operand::Meas { meas: MeasId(3) }),
        TraceOwner::Imported,
    )
    .expect("two candidates");
    focus(&mut f, FieldId::Operator);
    f.cycle(2);
    let c = math_of(&f);
    assert_eq!(c.domain, MathDomain::Spectrum);
    assert_eq!(
        c.expr,
        MathExpr::Binary {
            a: Operand::Meas { meas: MeasId(3) },
            op: MathOp::Multiply,
            b: Operand::Trace { trace: TraceId(7) },
        }
    );
    assert_eq!(f.text(FieldId::Name), "Spec 1 × speaker");
    // A live transfer function is no filter: B offers the stored one only.
    assert!(!f.pick_operand(FieldId::OperandB, Operand::Meas { meas: MeasId(1) }));
    f.cycle(1);
    assert!(matches!(
        math_of(&f).expr,
        MathExpr::Binary {
            op: MathOp::Divide,
            ..
        }
    ));
}

/// A main rising and a sub falling, crossing at 90 Hz, on one grid.
fn crossing() -> Vec<Level> {
    let freqs: Vec<f64> = (0..200).map(|i| 20.0 * 1.03f64.powi(i)).collect();
    let cols = std::sync::Arc::new(ac2_scene::grid::GridColumns {
        freqs: freqs.clone(),
        edges: Vec::new(),
        bin_hz: None,
    });
    let curve = |sign: f64| -> Vec<f32> {
        freqs
            .iter()
            .map(|f| {
                let o = (f / 90.0).log10();
                (40.0 * if sign > 0.0 { o.min(0.0) } else { -o.max(0.0) }) as f32
            })
            .collect()
    };
    vec![
        Level {
            operand: Operand::Meas { meas: MeasId(1) },
            cols: cols.clone(),
            db: curve(1.0),
        },
        Level {
            operand: Operand::Meas { meas: MeasId(2) },
            cols,
            db: curve(-1.0),
        },
    ]
}

fn ids(f: &Form) -> Vec<FieldId> {
    f.fields.iter().map(|x| x.id).collect()
}

/// Alt+M: the operator on align, the band prefilled where the two curves are alike, the
/// name `A ↔ B`; no smoothing or phase reference rows.
#[test]
fn alignment_prefills_the_band() {
    let mut f = Form::math(candidates(), None, TraceOwner::Imported).expect("two candidates");
    f.set_levels(crossing());
    f.choose_align();
    assert_eq!(
        ids(&f),
        [
            FieldId::OperandA,
            FieldId::Operator,
            FieldId::OperandB,
            FieldId::AlignFrom,
            FieldId::AlignTo,
            FieldId::MaxDelay,
            FieldId::Name,
        ]
    );
    assert_eq!(f.fields[f.focus].id, FieldId::OperandB);
    assert_eq!(f.text(FieldId::AlignFrom), "65 Hz");
    assert_eq!(f.text(FieldId::AlignTo), "130 Hz");
    assert_eq!(f.text(FieldId::MaxDelay), "50 ms");
    assert_eq!(
        f.field(FieldId::AlignFrom)
            .expect("an alignment has a band row")
            .hint,
        align::PREFILLED
    );
    assert_eq!(f.text(FieldId::Name), "Main L ↔ Sub");
    assert_eq!(
        math_of(&f).expr,
        MathExpr::Align {
            a: Operand::Meas { meas: MeasId(1) },
            b: Operand::Meas { meas: MeasId(2) },
            band: AlignBand {
                lo: ac2_proto::units::Hz(65.0),
                hi: ac2_proto::units::Hz(130.0),
            },
            max_delay: ac2_proto::units::Seconds(0.05),
        }
    );
}

/// Without curves to read, the band is asked: empty, and Enter says what is missing. A
/// typed band and max delay are what is made.
#[test]
fn alignment_band_is_asked_without_curves() {
    let mut f = Form::math(candidates(), None, TraceOwner::Imported).expect("two candidates");
    f.choose_align();
    assert_eq!(f.text(FieldId::AlignFrom), "");
    assert_eq!(
        f.field(FieldId::AlignFrom)
            .expect("an alignment has a band row")
            .hint,
        align::ASKED
    );
    let e = f.math_config().expect_err("no band");
    assert!(e.starts_with("overlap from"), "{e}");
    focus(&mut f, FieldId::AlignFrom);
    f.type_text("800");
    focus(&mut f, FieldId::AlignTo);
    f.type_text("1.8 kHz");
    focus(&mut f, FieldId::MaxDelay);
    f.select_all();
    f.type_text("20");
    let MathExpr::Align {
        band, max_delay, ..
    } = math_of(&f).expr
    else {
        panic!()
    };
    assert_eq!((band.lo.0, band.hi.0), (800.0, 1800.0));
    assert!((max_delay.0 - 0.02).abs() < 1e-12, "{max_delay:?}");
    focus(&mut f, FieldId::AlignTo);
    f.select_all();
    f.type_text("400");
    assert_eq!(
        f.math_config().expect_err("upside down"),
        "the overlap band needs from below to (Hz)"
    );
}

/// An alignment edited: its rows as made.
#[test]
fn alignment_edit_prefills_its_rows() {
    let m = meas(
        9,
        "Sub ↔ Main",
        MeasKind::Math {
            config: MathConfig::of(
                TraceOwner::Imported,
                MathDomain::Transfer,
                MathExpr::Align {
                    a: Operand::Meas { meas: MeasId(2) },
                    b: Operand::Meas { meas: MeasId(1) },
                    band: AlignBand {
                        lo: ac2_proto::units::Hz(60.0),
                        hi: ac2_proto::units::Hz(120.0),
                    },
                    max_delay: ac2_proto::units::Seconds(0.03),
                },
            ),
        },
    );
    let mut f = Form::edit_math(&m, candidates()).unwrap_or_else(|e| panic!("{e}"));
    f.set_levels(crossing());
    assert_eq!(f.text(FieldId::AlignFrom), "60 Hz");
    assert_eq!(f.text(FieldId::AlignTo), "120 Hz");
    assert_eq!(f.text(FieldId::MaxDelay), "30 ms");
    assert_eq!(f.text(FieldId::Name), "Sub ↔ Main");
    assert_eq!(math_of(&f).expr, m_expr(&m));
}

fn m_expr(m: &Measurement) -> MathExpr {
    match &m.config.kind {
        MeasKind::Math { config } => config.expr.clone(),
        _ => panic!(),
    }
}
