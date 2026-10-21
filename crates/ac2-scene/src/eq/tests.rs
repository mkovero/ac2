use super::*;
use ac2_proto::model::{EqBody, EqSnapshot};
use ac2_proto::units::{Db, Hz, Seconds};

fn set(enabled: bool, mark: Option<u32>) -> EqSet {
    let body = EqBody {
        name: "EQ 1".into(),
        realisation: EqRealisation::Digital { rate: Hz(48_000.0) },
        peak_q: PeakQ::HalfGain,
        enabled,
        sections: vec![EqSection {
            enabled: true,
            shape: EqShape::Gain { gain: Db(-1.0) },
        }],
    };
    EqSet {
        id: EqSetId(1),
        rev: 5,
        in_console: mark.map(|rev| EqSnapshot {
            set: EqSetId(1),
            rev,
            body: body.clone(),
        }),
        body,
        trace: TraceId(9),
    }
}

fn row(i: usize, enabled: bool, shape: EqShape) -> String {
    section_text(i, &EqSection { enabled, shape }, PeakQ::HalfGain, &|t| {
        format!("FIR v{}", t.0)
    })
}

#[test]
fn section_rows() {
    let cases = [
        (
            EqShape::Peaking {
                f: Hz(1250.0),
                gain: Db(-4.0),
                bw: Bandwidth::Q { value: 2.0 },
                q_measured: None,
            },
            "3 · peaking 1.25 kHz −4.0 dB Q 2.0 (half-gain)",
        ),
        (
            EqShape::Peaking {
                f: Hz(300.0),
                gain: Db(3.0),
                bw: Bandwidth::Octaves { n: 0.7 },
                q_measured: Some(PeakQ::Minus3dbFromPeak),
            },
            "3 · peaking 300 Hz +3.0 dB 0.70 oct (3 dB from peak)",
        ),
        (
            EqShape::LowShelf {
                f: Hz(120.0),
                gain: Db(3.0),
                order: ShelfOrder::Second {
                    slope: ShelfSlope::S { value: 1.0 },
                },
                f_at: ShelfFreq::Midpoint,
            },
            "3 · low shelf 120 Hz +3.0 dB S 1.00",
        ),
        (
            EqShape::HighShelf {
                f: Hz(4000.0),
                gain: Db(-5.0),
                order: ShelfOrder::First,
                f_at: ShelfFreq::Corner,
            },
            "3 · high shelf 4.00 kHz −5.0 dB 1st order (corner)",
        ),
        (
            EqShape::HighPass {
                f: Hz(80.0),
                alignment: Alignment::LinkwitzRiley,
                order: 4,
            },
            "3 · high-pass 80.0 Hz LR 4",
        ),
        (
            EqShape::LowPass {
                f: Hz(80.0),
                alignment: Alignment::Bessel {
                    norm: BesselNorm::Phase,
                },
                order: 2,
            },
            "3 · low-pass 80.0 Hz Bessel 2 (phase)",
        ),
        (
            EqShape::HighPassQ {
                f: Hz(25.0),
                q: 0.9,
            },
            "3 · high-pass 25.0 Hz Q 0.90",
        ),
        (
            EqShape::Notch {
                f: Hz(60.0),
                bw: Bandwidth::Q { value: 30.0 },
            },
            "3 · notch 60.0 Hz Q 30.0",
        ),
        (
            EqShape::AllPass2 {
                f: Hz(900.0),
                q: 0.6,
            },
            "3 · all-pass 2nd order 900 Hz Q 0.60",
        ),
        (EqShape::Gain { gain: Db(-3.0) }, "3 · gain −3.0 dB"),
        (EqShape::Polarity, "3 · polarity inverted"),
        (
            EqShape::Delay {
                seconds: Seconds(0.004),
            },
            "3 · delay 4.00 ms",
        ),
        (
            EqShape::Response { trace: TraceId(3) },
            "3 · response of FIR v3",
        ),
    ];
    for (shape, want) in cases {
        assert_eq!(row(2, true, shape), want);
    }
    assert_eq!(
        row(0, false, EqShape::Gain { gain: Db(1.0) }),
        "1 · gain +1.0 dB · off"
    );
}

#[test]
fn basis_and_set_tags() {
    assert_eq!(basis_tag("EQ 1", EqBasis::Whole), "assumes none of it in");
    assert_eq!(
        basis_tag("EQ 1", EqBasis::Change { since: 4 }),
        "change since rev 4"
    );
    assert_eq!(
        basis_tag("EQ 1", EqBasis::NoChange { rev: 4 }),
        "EQ 1 in · no change"
    );
    assert_eq!(set_tags(&set(false, None)), ["bypassed"]);
    assert_eq!(set_tags(&set(true, Some(4))), ["in console (rev 4)"]);
    assert_eq!(mark_text(&set(true, None)), "not marked");
    assert_eq!(
        realisation_text(EqRealisation::Digital { rate: Hz(48_000.0) }),
        "digital 48 kHz"
    );
    assert_eq!(
        realisation_text(EqRealisation::Digital { rate: Hz(44_100.0) }),
        "digital 44.1 kHz"
    );
    // The live curve contains what is marked now; unchanged since the mark: no change.
    let s = set(true, Some(5));
    assert_eq!(
        curve_basis(&s, &Operand::Meas { meas: MeasId(1) }, |_| None),
        EqBasis::NoChange { rev: 5 }
    );
    assert_eq!(
        curve_basis(&s, &Operand::Trace { trace: TraceId(4) }, |_| None),
        EqBasis::Whole
    );
}

#[test]
fn deleting_a_set_names_the_curves_that_go_with_it() {
    let c = delete_set_confirm(&set(true, None), "Main L", &["Main L after EQ 1".into()]);
    assert_eq!(c.title, "Delete EQ 1?");
    assert_eq!(
        c.lines,
        [
            "EQ set of Main L · 1 section",
            "Its filter curve goes with it, and the curves after it: Main L after EQ 1. It \
             cannot be undone."
        ]
    );
    assert_eq!(
        delete_set_confirm(&set(true, None), "Main L", &[]).lines[1],
        "Its filter curve goes with it; it cannot be undone."
    );
}
