//! The alignment and EQ fixtures: math channels as the daemon would publish them.

use super::*;

impl Rig {
    /// Adds "Delay tower ↔ Main L" (9), an alignment of the two in 60–120 Hz: Main L 4.21 ms
    /// late, the fit's curves where both play (30–400 Hz).
    pub fn add_alignment(&self) {
        use ac2_proto::frame::{MathState, OperandState, OperandStatus};
        use ac2_proto::model::{
            AlignBand, AlignCandidate, AlignCurves, AlignFit, AlignOutcome, AlignSide, AlignState,
            MathConfig, MathDomain, MathExpr, Operand, PhaseBasis, Summation,
        };
        let (a, b) = (
            Operand::Meas { meas: MeasId(2) },
            Operand::Meas { meas: MeasId(1) },
        );
        let tau = 0.00421;
        {
            let mut s = self.fake.lock();
            let config = MathConfig::of(
                ac2_proto::model::TraceOwner::Meas { meas: MeasId(2) },
                MathDomain::Transfer,
                MathExpr::Align {
                    a,
                    b,
                    band: AlignBand {
                        lo: Hz(60.0),
                        hi: Hz(120.0),
                    },
                    max_delay: Seconds(0.05),
                },
            );
            let m = measurement(
                9,
                "Delay tower ↔ Main L",
                MeasKind::Math { config },
                None,
                Some(&TF_GRID),
            );
            s.commit(Change::Measurement(Patch::Set(m)));
        }
        let freqs = TF_GRID.centres();
        let both = |f: f64| (30.0..=400.0).contains(&f);
        let sum = |deg: f64| (20.0 * (deg.to_radians() / 2.0).cos().abs().log10()).max(-30.0);
        let before: Vec<f64> = freqs
            .iter()
            .map(|f| wrap(-360.0 * f * tau + 20.0))
            .collect();
        let after: Vec<f64> = freqs.iter().map(|f| 14.0 * (f / 25.0).ln().sin()).collect();
        let only = |v: &[f64], g: &dyn Fn(f64) -> f64| -> Vec<f32> {
            freqs
                .iter()
                .zip(v)
                .map(|(f, x)| if both(*f) { g(*x) as f32 } else { f32::NAN })
                .collect()
        };
        let curves = AlignCurves {
            phase_before: only(&before, &|x| x),
            phase_after: only(&after, &|x| x),
            sum_before: only(&before, &sum),
            sum_after: only(&after, &sum),
        };
        let candidate = |ms: f64, inverted: bool, res: f64, score: f64| AlignCandidate {
            delay: Seconds(ms / 1000.0),
            delayed: AlignSide::A,
            inverted,
            residual_deg: res,
            score,
        };
        let fit = AlignFit {
            best: candidate(4.21, false, 11.0, 0.98),
            other_polarity: Some(candidate(9.8, true, 41.0, 0.71)),
            runner_up: None,
            before: Summation {
                mean_db: -7.9,
                worst_db: -21.0,
            },
            after: Summation {
                mean_db: -0.3,
                worst_db: -1.2,
            },
            columns: 24,
        };
        let mut f = tf_frame(9, 0.0, 0.0, 90.0);
        f.coh = vec![f32::NAN; f.coh.len()];
        f.meta.math = Some(Box::new(MathState {
            operands: [a, b]
                .map(|operand| OperandState {
                    operand,
                    status: OperandStatus::Included,
                })
                .to_vec(),
            phase: PhaseBasis::SharedTimeBase,
            align: Some(Box::new(AlignState {
                outcome: AlignOutcome::Fit { fit },
            })),
        }));
        f.align = Some(curves);
        self.extra
            .lock()
            .unwrap()
            .push((FrameData::Tf(f), Some(TF_GRID.id())));
    }

    /// Gives Main L (1) the set "EQ 1" (a 2 kHz cut over its bump, a gentle top shelf), the
    /// set's dashed filter curve (trace 40) and "Main L after EQ 1" (6), the prediction of
    /// the live curve, as the daemon would make them.
    pub fn add_eq(&self) {
        use ac2_proto::model::{MathConfig, MathDomain, MathExpr, MathOp, Operand};
        let body = EqBody {
            name: "EQ 1".into(),
            realisation: EqRealisation::Digital { rate: Hz(48_000.0) },
            peak_q: PeakQ::HalfGain,
            enabled: true,
            sections: vec![
                EqSection {
                    enabled: true,
                    shape: EqShape::Peaking {
                        f: Hz(2000.0),
                        gain: Db(-4.5),
                        bw: Bandwidth::Q { value: 1.4 },
                        q_measured: None,
                    },
                },
                EqSection {
                    enabled: true,
                    shape: EqShape::HighShelf {
                        f: Hz(10_000.0),
                        gain: Db(2.0),
                        order: ShelfOrder::Second {
                            slope: ShelfSlope::S { value: 1.0 },
                        },
                        f_at: ShelfFreq::Midpoint,
                    },
                },
            ],
        };
        let freqs = TF_GRID.centres();
        let none = |_| Err(String::from("no response sections here"));
        let h = ac2_traces::eq::response(&body, &freqs, &none).unwrap();
        let set = EqSet {
            id: EqSetId(1),
            rev: 3,
            body,
            in_console: None,
            trace: TraceId(40),
        };
        {
            let mut s = self.fake.lock();
            let mut main = s
                .state
                .measurements
                .iter()
                .find(|m| m.id == MeasId(1))
                .cloned()
                .unwrap();
            main.eq = vec![set];
            s.commit(Change::Measurement(Patch::Set(main)));
            let mut edit = ac2_traces::meta::new_edit(
                TraceId(40),
                "EQ 1".into(),
                None,
                TraceOwner::Meas { meas: MeasId(1) },
            );
            edit.locked = true;
            let curve = TraceMeta {
                id: TraceId(40),
                edit,
                kind: TraceKind::Transfer,
                source: TraceSource::Eq {
                    meas: MeasId(1),
                    set: EqSetId(1),
                    rev: 3,
                },
                grid_id: TF_GRID.id(),
                delay: Seconds(0.0),
                depth: None,
                cal: CalState::Uncalibrated,
                mic: None,
                mic_curve: None,
                created_at: WallNs(0),
                eq_in: Vec::new(),
            };
            s.put_trace(curve, TF_GRID, ac2_traces::eq::columns(&h));
            let config = MathConfig::of(
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
            );
            let after = measurement(
                6,
                "Main L after EQ 1",
                MeasKind::Math { config },
                None,
                Some(&TF_GRID),
            );
            s.commit(Change::Measurement(Patch::Set(after)));
        }
        let mut f = tf_frame(6, 0.0, 0.0, 2000.0);
        for ((m, p), v) in f.mag.iter_mut().zip(f.phase.iter_mut()).zip(&h) {
            *m += (20.0 * v.norm().log10()) as f32;
            *p = wrap(f64::from(*p) + v.arg().to_degrees()) as f32;
        }
        self.extra
            .lock()
            .unwrap()
            .push((FrameData::Tf(f), Some(TF_GRID.id())));
    }
}
