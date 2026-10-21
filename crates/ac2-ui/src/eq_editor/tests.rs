use super::*;

fn peaking(f: f64, gain: f64, q: f64) -> EqShape {
    EqShape::Peaking {
        f: Hz(f),
        gain: Db(gain),
        bw: Bandwidth::Q { value: q },
        q_measured: None,
    }
}

#[test]
fn field_steps() {
    let mut s = peaking(1000.0, -4.0, 2.0);
    step_field(&mut s, FieldKind::Freq, 24, Step::Plain, PeakQ::HalfGain);
    let EqShape::Peaking { f, .. } = s else {
        panic!()
    };
    assert!(
        (f.0 - 2000.0).abs() < 1e-9,
        "a 1/24-octave step, 24 of them an octave"
    );
    step_field(&mut s, FieldKind::Gain, 1, Step::Plain, PeakQ::HalfGain);
    step_field(&mut s, FieldKind::Gain, -1, Step::Coarse, PeakQ::HalfGain);
    let EqShape::Peaking { gain, .. } = s else {
        panic!()
    };
    assert_eq!(gain.0, -5.5);
    // ← widens by 1/12 octave: the Q falls.
    let q_of = |s: &EqShape| match s {
        EqShape::Peaking {
            bw: Bandwidth::Q { value },
            ..
        } => *value,
        _ => panic!(),
    };
    let before = text::octaves_of_q(q_of(&s));
    step_field(&mut s, FieldKind::Q, -1, Step::Plain, PeakQ::HalfGain);
    assert!((text::octaves_of_q(q_of(&s)) - before - 1.0 / 12.0).abs() < 1e-9);
    step_field(
        &mut s,
        FieldKind::QMeasured,
        1,
        Step::Plain,
        PeakQ::HalfGain,
    );
    assert!(matches!(
        s,
        EqShape::Peaking {
            q_measured: Some(PeakQ::Minus3dbFromPeak),
            ..
        }
    ));
    // A new shape keeps f and gain.
    step_field(&mut s, FieldKind::Shape, 1, Step::Plain, PeakQ::HalfGain);
    assert!(
        matches!(s, EqShape::LowShelf { f, gain, .. } if f.0 == 2000.0 && gain.0 == -5.5),
        "{s:?}"
    );
    let mut hp = EqShape::HighPass {
        f: Hz(80.0),
        alignment: Alignment::LinkwitzRiley,
        order: 4,
    };
    step_field(&mut hp, FieldKind::Order, 1, Step::Plain, PeakQ::HalfGain);
    assert!(matches!(hp, EqShape::HighPass { order: 6, .. }));
    assert!(type_field(&mut hp, FieldKind::Freq, "1.25k").is_some());
    assert!(matches!(hp, EqShape::HighPass { f, .. } if f.0 == 1250.0));
    assert!(type_field(&mut hp, FieldKind::Freq, "-").is_none());
}

#[test]
fn realisation_cycles_through_analog() {
    let d48 = EqRealisation::Digital { rate: Hz(48_000.0) };
    assert_eq!(
        step_realisation(d48, 1),
        EqRealisation::Digital { rate: Hz(88_200.0) }
    );
    assert_eq!(
        step_realisation(
            EqRealisation::Digital {
                rate: Hz(192_000.0)
            },
            1
        ),
        EqRealisation::Analog
    );
    assert_eq!(
        step_realisation(EqRealisation::Analog, 1),
        EqRealisation::Digital { rate: Hz(44_100.0) }
    );
}

#[test]
fn rows_and_fields() {
    let mut e = EqEditor::new(MeasId(1), None);
    e.move_row(1, 2, 1);
    assert_eq!(e.row, Row::Section(0));
    assert_eq!(e.field, 0);
    e.move_row(5, 2, 1);
    assert_eq!(e.row, Row::Curve(0));
    e.move_row(-9, 2, 1);
    assert_eq!(e.row, Row::Header);
    e.move_field(
        -1,
        &EqBody {
            name: "EQ 1".into(),
            realisation: EqRealisation::Analog,
            peak_q: PeakQ::HalfGain,
            enabled: true,
            sections: Vec::new(),
        },
    );
    assert_eq!(e.header_field(), Some(HeaderField::PeakQ));
}
