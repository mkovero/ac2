//! EQ what-if samples: a set of every shape, its filter curve, a prediction and every edit.

use super::builders::*;
use super::traces::*;
use crate::ctrl::Command;
use crate::model::*;
use crate::units::*;

fn sec(shape: EqShape) -> EqSection {
    EqSection {
        enabled: true,
        shape,
    }
}

/// One section of every shape.
pub(super) fn eq_sections() -> Vec<EqSection> {
    vec![
        sec(EqShape::Peaking {
            f: Hz(1250.0),
            gain: Db(-4.0),
            bw: Bandwidth::Q { value: 2.0 },
            q_measured: None,
        }),
        EqSection {
            enabled: false,
            shape: EqShape::Peaking {
                f: Hz(3150.0),
                gain: Db(6.0),
                bw: Bandwidth::Octaves { n: 0.7 },
                q_measured: Some(PeakQ::Minus3dbFromPeak),
            },
        },
        sec(EqShape::LowShelf {
            f: Hz(120.0),
            gain: Db(3.0),
            order: ShelfOrder::Second {
                slope: ShelfSlope::S { value: 1.0 },
            },
            f_at: ShelfFreq::Midpoint,
        }),
        sec(EqShape::HighShelf {
            f: Hz(8000.0),
            gain: Db(-6.0),
            order: ShelfOrder::First,
            f_at: ShelfFreq::Corner,
        }),
        sec(EqShape::HighPass {
            f: Hz(80.0),
            alignment: Alignment::LinkwitzRiley,
            order: 4,
        }),
        sec(EqShape::LowPass {
            f: Hz(18_000.0),
            alignment: Alignment::Bessel {
                norm: BesselNorm::Phase,
            },
            order: 2,
        }),
        sec(EqShape::HighPassQ {
            f: Hz(40.0),
            q: 1.2,
        }),
        sec(EqShape::LowPassQ {
            f: Hz(16_000.0),
            q: 0.5,
        }),
        sec(EqShape::Notch {
            f: Hz(60.0),
            bw: Bandwidth::Q { value: 8.0 },
        }),
        sec(EqShape::BandPass {
            f: Hz(500.0),
            bw: Bandwidth::Q { value: 1.4 },
        }),
        sec(EqShape::AllPass1 { f: Hz(300.0) }),
        sec(EqShape::AllPass2 {
            f: Hz(90.0),
            q: 0.7,
        }),
        sec(EqShape::Gain { gain: Db(-1.5) }),
        sec(EqShape::Polarity),
        sec(EqShape::Delay {
            seconds: Seconds(0.0012),
        }),
        sec(EqShape::Biquad {
            b0: 1.0,
            b1: -1.9,
            b2: 0.91,
            a1: -1.88,
            a2: 0.9,
            rate: Hz(48_000.0),
        }),
        sec(EqShape::Response { trace: TraceId(8) }),
    ]
}

pub(super) fn eq_body() -> EqBody {
    EqBody {
        name: "EQ 1".into(),
        realisation: EqRealisation::Digital { rate: Hz(96_000.0) },
        peak_q: PeakQ::HalfGain,
        enabled: true,
        sections: eq_sections(),
    }
}

/// A set marked in the console at rev 4, edited since.
pub(super) fn eq_set() -> EqSet {
    EqSet {
        id: EqSetId(1),
        rev: 5,
        body: eq_body(),
        in_console: Some(EqSnapshot {
            set: EqSetId(1),
            rev: 4,
            body: EqBody {
                realisation: EqRealisation::Analog,
                sections: eq_sections()[..2].to_vec(),
                ..eq_body()
            },
        }),
        trace: TraceId(11),
    }
}

/// A transfer measurement holding a set.
pub fn eq_measurement() -> Measurement {
    Measurement {
        eq: vec![eq_set()],
        ..measurement()
    }
}

/// A set's filter curve.
pub(super) fn eq_trace_meta() -> TraceMeta {
    TraceMeta {
        id: TraceId(11),
        source: TraceSource::Eq {
            meas: MeasId(1),
            set: EqSetId(1),
            rev: 5,
        },
        delay: Seconds(0.0),
        depth: None,
        cal: CalState::Uncalibrated,
        mic: None,
        eq_in: Vec::new(),
        position: None,
        captured_unsettled: false,
        superseded: false,
        ..trace_meta()
    }
}

/// A capture taken while the set was marked.
pub(super) fn eq_capture_meta() -> TraceMeta {
    TraceMeta {
        id: TraceId(12),
        eq_in: vec![eq_set().in_console.expect("marked")],
        ..trace_meta()
    }
}

/// A prediction: the live curve after the set.
pub(super) fn eq_prediction() -> Measurement {
    Measurement {
        id: MeasId(9),
        config: MeasConfig {
            scope: MeasScope::Session,
            name: "Main L after EQ 1".into(),
            kind: MeasKind::Math {
                config: MathConfig::of(
                    TraceOwner::Meas { meas: MeasId(1) },
                    MathDomain::Transfer,
                    MathExpr::Binary {
                        a: Operand::Meas { meas: MeasId(1) },
                        op: MathOp::Multiply,
                        b: Operand::Eq {
                            meas: MeasId(1),
                            set: EqSetId(1),
                        },
                    },
                ),
            },
        },
        colour: None,
        layout: None,
        delay: None,
        ..measurement()
    }
}

/// The `meas.eq` sample (one per command).
pub(super) fn eq_commands() -> Vec<Command> {
    vec![Command::MeasEq {
        meas: MeasId(1),
        edit: EqEdit::Put {
            set: EqSetId(1),
            body: eq_body(),
        },
    }]
}

/// Every [`EqEdit`].
pub fn eq_edits() -> Vec<EqEdit> {
    vec![
        EqEdit::New {
            name: None,
            realisation: Some(EqRealisation::Analog),
            peak_q: Some(PeakQ::BandPass),
            sections: eq_sections()[..1].to_vec(),
        },
        EqEdit::Put {
            set: EqSetId(1),
            body: eq_body(),
        },
        EqEdit::Delete { set: EqSetId(2) },
        EqEdit::Mark {
            set: EqSetId(1),
            clear: false,
        },
        EqEdit::Predict {
            set: EqSetId(1),
            curve: Operand::Trace { trace: TraceId(7) },
            on: true,
        },
        EqEdit::Copy {
            set: EqSetId(1),
            to: MeasId(2),
        },
    ]
}
