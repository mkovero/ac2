use super::*;
use ac2_proto::model::EqSection;
use ac2_proto::units::{Db, EqSetId, Hz};

fn peak(gain: f64) -> EqSection {
    EqSection {
        enabled: true,
        shape: EqShape::Peaking {
            f: Hz(1000.0),
            gain: Db(gain),
            bw: Bandwidth::Q { value: 2.0 },
            q_measured: None,
        },
    }
}

fn body(sections: Vec<EqSection>) -> EqBody {
    EqBody {
        name: "EQ 1".into(),
        realisation: EqRealisation::Analog,
        peak_q: PeakQ::HalfGain,
        enabled: true,
        sections,
    }
}

fn no_traces(t: TraceId) -> Result<Vec<Complex64>, String> {
    Err(format!("no trace {t}"))
}

fn db_at_1k(h: &[Complex64]) -> f64 {
    20.0 * h[0].norm().log10()
}

#[test]
fn prediction_is_the_whole_set_its_change_or_nothing() {
    let marked = body(vec![peak(-6.0)]);
    let mut set = EqSet {
        id: EqSetId(1),
        rev: 3,
        body: marked.clone(),
        in_console: None,
        trace: TraceId(9),
    };
    let f = [1000.0];
    let (basis, h) = prediction(&set, None, &f, &no_traces).expect("ok");
    assert_eq!(basis, EqBasis::Whole);
    assert!((db_at_1k(&h) + 6.0).abs() < 1e-9);
    let snap = set.snapshot();
    let (basis, h) = prediction(&set, Some(&snap), &f, &no_traces).expect("ok");
    assert_eq!(basis, EqBasis::NoChange { rev: 3 });
    assert!(db_at_1k(&h).abs() < 1e-12);
    set.body.sections[0] = peak(-8.0);
    set.rev = 4;
    let (basis, h) = prediction(&set, Some(&snap), &f, &no_traces).expect("ok");
    assert_eq!(basis, EqBasis::Change { since: 3 });
    assert!((db_at_1k(&h) + 2.0).abs() < 1e-9);
    // Bypassed while marked: the EQ taken out again.
    set.body.enabled = false;
    let (_, h) = prediction(&set, Some(&snap), &f, &no_traces).expect("ok");
    assert!((db_at_1k(&h) - 6.0).abs() < 1e-9);
}

#[test]
fn a_refused_section_is_named_even_when_off() {
    let mut s = peak(-6.0);
    s.enabled = false;
    if let EqShape::Peaking { f, .. } = &mut s.shape {
        *f = Hz(30_000.0);
    }
    let mut b = body(vec![peak(1.0), s]);
    b.realisation = EqRealisation::Digital { rate: Hz(48_000.0) };
    let e = response(&b, &[1000.0], &no_traces).expect_err("refused");
    assert_eq!(
        e.to_string(),
        "2 · peaking: 30 kHz must be below fs/2 = 24 kHz"
    );
    let mut b = body(vec![EqSection {
        enabled: true,
        shape: EqShape::Response { trace: TraceId(4) },
    }]);
    assert_eq!(
        response(&b, &[1000.0], &no_traces)
            .expect_err("refused")
            .to_string(),
        "1 · response: no trace 4"
    );
    b.enabled = false;
    assert!(
        response(&b, &[1000.0], &no_traces).is_err(),
        "validated when bypassed"
    );
}

#[test]
fn columns_round_trip_and_keep_gaps() {
    let h = vec![
        Complex64::from_polar(0.5, 1.0),
        Complex64::new(f64::NAN, f64::NAN),
        Complex64::new(-1.0, 0.0),
    ];
    let c = columns(&h);
    assert!(c.mag_db[1].is_nan());
    assert_eq!(c.phase_deg.as_ref().expect("phase")[2], 180.0);
    let back = complex(&c);
    assert!((back[0] - h[0]).norm() < 1e-6);
    assert!(back[1].re.is_nan());
}
