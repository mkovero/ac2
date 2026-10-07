//! The math job against analytic expectations: power and complex means of operands with
//! known responses, a ratio and a sum of live and stored operands, operands left out and
//! why, and the per-operand delay reference.

use ac2_proto::frame::{FrameStamp, OperandStatus, ProtectionFlags, TfFrame, TfMeta, ValidityMask};
use ac2_proto::grid::GridDef;
use ac2_proto::model::{
    AverageMethod, MathConfig, MathDomain, MathExpr, MathOp, MathReference, Operand,
};
use ac2_proto::units::{
    DaemonIncarnation, MeasId, Rev, SampleIndex, Seconds, SessionEpoch, TraceId, WallNs,
};

use super::*;

const EPOCH: SessionEpoch = SessionEpoch(3);

fn grid() -> GridDef {
    GridDef::Log {
        ppo: 12,
        k_min: -60,
        k_max: 59,
    }
}

/// A live operand's result: flat `db`, a path arriving `arrival` s late measured with
/// `inserted` s of delay (stored phase −360·f·(arrival − inserted)), γ² = 0.9.
fn member(meas: u32, db: f32, arrival: f64, inserted: f64, prot: ProtectionFlags) -> Answer {
    let g = grid();
    let f = frequencies(&g);
    let phase = f
        .iter()
        .map(|f| ac2_traces::columns::wrap_deg(-360.0 * f * (arrival - inserted)) as f32)
        .collect();
    Answer::Result(Box::new(Frame {
        stamp: FrameStamp {
            seq: 0,
            audio_sample: SampleIndex(48_000),
            session_epoch: EPOCH,
            daemon_incarnation: DaemonIncarnation(1),
            config_rev: Rev(1),
            config_applied_at: SampleIndex(0),
            capture_wall_ns: WallNs(0),
            grid_id: Some(g.id()),
            protection: prot,
        },
        data: FrameData::Tf(TfFrame {
            meas: MeasId(meas),
            meta: TfMeta {
                delay: Seconds(inserted),
                frozen: false,
                smoothing: None,
                mic_curve: false,
                math: None,
            },
            mag: vec![db; f.len()],
            phase,
            coh: vec![0.9; f.len()],
            validity: vec![ValidityMask::NONE; f.len()],
        }),
    }))
}

fn flat(meas: u32, db: f32) -> Answer {
    member(meas, db, 0.0, 0.0, ProtectionFlags::NONE)
}

/// A stored operand: flat `db`, phase 0, measured with `delay`, in `time_base`.
fn stored(db: f32, delay: f64, time_base: Option<SessionEpoch>) -> Answer {
    let n = frequencies(&grid()).len();
    Answer::Stored(Arc::new(Held {
        columns: Columns {
            mag_db: vec![db; n],
            phase_deg: Some(vec![0.0; n]),
            coherence: Some(vec![0.5; n]),
        },
        delay,
        time_base,
        scale: None,
        mic_curve: false,
    }))
}

fn meas(m: u32) -> Operand {
    Operand::Meas { meas: MeasId(m) }
}

fn avg(members: &[u32], method: AverageMethod) -> MathConfig {
    MathConfig::of(
        ac2_proto::model::TraceOwner::Imported,
        MathDomain::Transfer,
        MathExpr::Average {
            of: members.iter().map(|m| meas(*m)).collect(),
            method,
        },
    )
}

fn binary(a: Operand, op: MathOp, b: Operand) -> MathConfig {
    MathConfig::of(
        ac2_proto::model::TraceOwner::Imported,
        MathDomain::Transfer,
        MathExpr::Binary { a, op, b },
    )
}

fn run(c: &MathConfig, seen: &[Option<f64>], answers: &[Answer]) -> TfFrame {
    let g = grid();
    match combine(MeasId(9), c, &g, &frequencies(&g), EPOCH, seen, answers) {
        FrameData::Tf(f) => f,
        other => panic!("{other:?}"),
    }
}

fn statuses(f: &TfFrame) -> Vec<OperandStatus> {
    f.meta
        .math
        .as_ref()
        .map(|a| a.operands.iter().map(|m| m.status).collect())
        .unwrap_or_default()
}

/// +3 dB and −3 dB operands average by power to 10·log10((10^0.3 + 10^−0.3)/2) ≈ 0.96 dB;
/// an operand with NO SIGNAL and a stopped one are left out and said to be.
#[test]
fn power_average_leaves_out_refused_and_stopped_operands() {
    let c = avg(&[1, 2, 3, 4], AverageMethod::Power);
    let answers = [
        flat(1, 3.0),
        flat(2, -3.0),
        member(3, 40.0, 0.0, 0.0, ProtectionFlags::NO_SIGNAL),
        Answer::Stopped,
    ];
    let f = run(&c, &[None; 4], &answers);
    let want = 10.0 * ((10f64.powf(0.3) + 10f64.powf(-0.3)) / 2.0).log10();
    assert!(
        f.mag.iter().all(|m| (f64::from(*m) - want).abs() < 1e-5),
        "{:?}",
        &f.mag[..3]
    );
    assert!(f.validity.iter().all(|v| *v == ValidityMask::NONE));
    assert!(f.coh.iter().all(|c| (c - 0.9).abs() < 1e-6));
    assert_eq!(
        statuses(&f),
        vec![
            OperandStatus::Included,
            OperandStatus::Included,
            OperandStatus::Refused {
                protection: ProtectionFlags::NO_SIGNAL
            },
            OperandStatus::Stopped,
        ]
    );
    let m = f.meta.math.as_ref().expect("math metadata");
    assert_eq!(m.included(), 2);
    assert_eq!(m.phase, PhaseBasis::SharedTimeBase);
}

/// A weak reference only holds an operand's averaging: it stays in.
#[test]
fn weak_reference_does_not_refuse() {
    let c = avg(&[1, 2], AverageMethod::Power);
    let answers = [
        flat(1, 0.0),
        member(2, 0.0, 0.0, 0.0, ProtectionFlags::WEAK_REFERENCE),
    ];
    let f = run(&c, &[None; 2], &answers);
    assert_eq!(statuses(&f), vec![OperandStatus::Included; 2]);
}

/// One usable operand is not an average: no value anywhere, every column says why.
#[test]
fn fewer_than_two_operands_refuse() {
    let c = avg(&[1, 2, 3], AverageMethod::Power);
    let answers = [flat(1, 0.0), Answer::NoResult, Answer::Stopped];
    let f = run(&c, &[None; 3], &answers);
    assert!(f.mag.iter().all(|m| m.is_nan()));
    assert!(f.validity.iter().all(|v| *v == ValidityMask::FEW_OPERANDS));
    assert_eq!(
        statuses(&f),
        vec![
            OperandStatus::Included,
            OperandStatus::Settling,
            OperandStatus::Stopped
        ]
    );
}

/// Two operands of the same 0 dB path, one measured 0.5 ms later than the other and aligned
/// by its own inserted delay: the complex average re-refers it to the first operand's delay
/// and shows the acoustic comb |cos(π f τ)|; the power average stays flat at 0 dB.
#[test]
fn complex_average_rerefers_each_operands_delay() {
    let tau = 0.5e-3;
    let answers = [
        member(1, 0.0, 0.010, 0.010, ProtectionFlags::NONE),
        member(2, 0.0, 0.010 + tau, 0.010 + tau, ProtectionFlags::NONE),
    ];
    let g = grid();
    let freqs = frequencies(&g);
    let f = run(&avg(&[1, 2], AverageMethod::Complex), &[None; 2], &answers);
    assert_eq!(f.meta.delay, Seconds(0.010));
    for (k, hz) in freqs.iter().enumerate() {
        let want = (std::f64::consts::PI * hz * tau).cos().abs();
        let got = 10f64.powf(f64::from(f.mag[k]) / 20.0);
        // Within a column of a null the dB value is steep; compare linear magnitude.
        assert!((got - want).abs() < 1e-4, "{hz} Hz: {got} vs {want}");
    }
    let p = run(&avg(&[1, 2], AverageMethod::Power), &[None; 2], &answers);
    assert!(p.mag.iter().all(|m| m.abs() < 1e-5));
}

/// The reference operand left out: its newest seen delay still refers the phase, so the
/// average does not jump to another operand's arrival.
#[test]
fn reference_operand_left_out_keeps_its_delay() {
    let mut c = avg(&[1, 2, 3], AverageMethod::Complex);
    c.reference = MathReference::Operand { operand: meas(1) };
    let answers = [
        member(1, 0.0, 0.0, 0.004, ProtectionFlags::CLIP),
        member(2, 0.0, 0.006, 0.006, ProtectionFlags::NONE),
        member(3, 0.0, 0.006, 0.006, ProtectionFlags::NONE),
    ];
    let f = run(&c, &[Some(0.004), Some(0.006), Some(0.006)], &answers);
    assert_eq!(f.meta.delay, Seconds(0.004));
    // Both included arrive 2 ms after the reference: phase −360·f·0.002 (wrapped).
    let freqs = frequencies(&grid());
    let k = freqs
        .iter()
        .position(|f| (f - 125.0).abs() < 1.0)
        .unwrap_or(0);
    let want = ac2_traces::columns::wrap_deg(-360.0 * freqs[k] * 0.002);
    assert!(
        (f64::from(f.phase[k]) - want).abs() < 1e-3,
        "{} vs {want}",
        f.phase[k]
    );
}

/// A column an operand has no value for has no result, with that operand's reason.
#[test]
fn an_operand_gap_is_a_gap_in_the_result() {
    let c = avg(&[1, 2], AverageMethod::Power);
    let mut b = flat(2, 0.0);
    if let Answer::Result(fr) = &mut b
        && let FrameData::Tf(tf) = &mut fr.data
    {
        tf.validity[0] = ValidityMask::SETTLING;
        tf.mag[0] = f32::NAN;
    }
    let f = run(&c, &[None; 2], &[flat(1, 0.0), b]);
    assert!(f.mag[0].is_nan());
    assert_eq!(f.validity[0], ValidityMask::SETTLING);
    assert_eq!(f.validity[1], ValidityMask::NONE);
}

/// Coherence weighting with equal coherence is the complex mean.
#[test]
fn equal_coherence_weighting_is_the_complex_mean() {
    let answers = [flat(1, 6.0), flat(2, 0.0)];
    let w = run(
        &avg(&[1, 2], AverageMethod::CoherenceWeighted),
        &[None; 2],
        &answers,
    );
    let c = run(&avg(&[1, 2], AverageMethod::Complex), &[None; 2], &answers);
    let want = 20.0 * ((10f64.powf(0.3) + 1.0) / 2.0).log10();
    assert!((f64::from(c.mag[5]) - want).abs() < 1e-5);
    assert!((w.mag[5] - c.mag[5]).abs() < 1e-5);
}

/// A live operand over a stored one of the same epoch: the ratio's magnitude is the level
/// difference and its phase the relative arrival; its coherence the lower of the two. Half
/// a ratio is no ratio.
#[test]
fn ratio_of_a_live_and_a_stored_operand() {
    let c = binary(
        meas(1),
        MathOp::Divide,
        Operand::Trace { trace: TraceId(4) },
    );
    let answers = [
        member(1, 3.0, 0.011, 0.011, ProtectionFlags::NONE),
        stored(-3.0, 0.010, Some(EPOCH)),
    ];
    let f = run(&c, &[None; 2], &answers);
    assert_eq!(f.meta.delay, Seconds(0.0));
    let freqs = frequencies(&grid());
    for (k, hz) in freqs.iter().enumerate() {
        assert!((f.mag[k] - 6.0).abs() < 1e-4);
        // Compared on the circle: ±180° are one phase.
        let d = ac2_traces::columns::wrap_deg(f64::from(f.phase[k]) + 360.0 * hz * 0.001);
        assert!(d.abs() < 1e-2, "{hz} Hz: {} off", d);
        assert!((f.coh[k] - 0.5).abs() < 1e-6);
    }
    let m = f.meta.math.as_ref().expect("math metadata");
    assert_eq!(m.phase, PhaseBasis::SharedTimeBase);

    let gone = run(&c, &[None; 2], &[Answer::Stopped, stored(-3.0, 0.0, None)]);
    assert!(gone.mag.iter().all(|v| v.is_nan()));
    assert!(
        gone.validity
            .iter()
            .all(|v| *v == ValidityMask::FEW_OPERANDS)
    );
    assert_eq!(
        statuses(&gone),
        vec![OperandStatus::Stopped, OperandStatus::Included]
    );
}

/// A sum needs one time base: a stored operand of another epoch is a mismatch, said so,
/// and the sum has no value; a ratio with it is marked as of own alignments.
#[test]
fn sum_across_time_bases_leaves_the_operand_out() {
    let other = Some(SessionEpoch(1));
    let add = binary(meas(1), MathOp::Add, Operand::Trace { trace: TraceId(4) });
    let f = run(&add, &[None; 2], &[flat(1, 0.0), stored(0.0, 0.0, other)]);
    assert_eq!(
        statuses(&f),
        vec![OperandStatus::Included, OperandStatus::Mismatch]
    );
    assert!(f.validity.iter().all(|v| *v == ValidityMask::FEW_OPERANDS));
    let div = binary(
        meas(1),
        MathOp::Divide,
        Operand::Trace { trace: TraceId(4) },
    );
    let f = run(&div, &[None; 2], &[flat(1, 0.0), stored(0.0, 0.0, other)]);
    assert_eq!(
        f.meta.math.as_ref().map(|m| m.phase),
        Some(PhaseBasis::OwnAlignments)
    );
    assert!(f.mag.iter().all(|v| v.abs() < 1e-6));
}

/// Two copies of one path, the stored one captured earlier in this epoch arriving 0.5 ms
/// before the live one: their sum is the comb `|1 + e^{−j2πfτ}|`, referred to the first's
/// delay, with no coherence.
#[test]
fn summation_of_a_capture_and_a_live_operand() {
    let tau = 0.5e-3;
    let add = binary(Operand::Trace { trace: TraceId(4) }, MathOp::Add, meas(2));
    let answers = [
        stored(0.0, 0.010, Some(EPOCH)),
        member(2, 0.0, 0.010 + tau, 0.010 + tau, ProtectionFlags::NONE),
    ];
    let f = run(&add, &[None; 2], &answers);
    assert_eq!(f.meta.delay, Seconds(0.010));
    for (k, hz) in frequencies(&grid()).iter().enumerate() {
        let want = 2.0 * (std::f64::consts::PI * hz * tau).cos().abs();
        let got = 10f64.powf(f64::from(f.mag[k]) / 20.0);
        if want > 1e-3 {
            assert!((got - want).abs() < 1e-4, "{hz} Hz: {got} vs {want}");
        }
        assert!(f.coh[k].is_nan(), "a sum has no coherence");
    }
}
