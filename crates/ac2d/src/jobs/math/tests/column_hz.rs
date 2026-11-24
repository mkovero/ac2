//! When a live operand's column frequencies (`Live::column_hz`) turn its phase: only when
//! every operand states the same ones, only by a change of delay, and then to the analytic
//! value at those frequencies; otherwise every operand is taken at the column centres.

use ac2_proto::frame::{ProtectionFlags, TfFrame};
use ac2_proto::model::{AverageMethod, MathOp, MathReference, Operand};
use ac2_proto::units::{Seconds, TraceId};
use ac2_traces::columns::wrap_deg;

use super::{Answer, EPOCH, at_hz, avg, binary, grid, meas, member, run, stored};

/// The grid's centres scaled by `k`: where a live column's bins lie when they sit off its
/// centre.
fn off_centres(k: f64) -> Vec<f64> {
    grid().centres().iter().map(|f| f * k).collect()
}

fn live(m: u32, arrival: f64, inserted: f64) -> Answer {
    member(m, 0.0, arrival, inserted, ProtectionFlags::NONE)
}

/// The two results are the same bit for bit (a gap is a gap in both).
fn same_bits(a: &TfFrame, b: &TfFrame) {
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(a.meta.delay, b.meta.delay);
    assert_eq!(bits(&a.mag), bits(&b.mag));
    assert_eq!(bits(&a.phase), bits(&b.phase));
    assert_eq!(bits(&a.coh), bits(&b.coh));
}

/// Operands all measured with one non-zero delay need no referral, so stating where their
/// bins lie changes nothing in a sum, difference, average or ratio.
#[test]
fn equal_delays_are_unchanged_bit_for_bit() {
    let hz = off_centres(1.03);
    let d = 0.02;
    let configs = [
        binary(meas(1), MathOp::Add, meas(2)),
        binary(meas(1), MathOp::Subtract, meas(2)),
        binary(meas(1), MathOp::Divide, meas(2)),
        avg(&[1, 2], AverageMethod::Complex),
    ];
    for c in &configs {
        // The paths arrive 0.3 ms apart, so the phases are not trivially zero.
        let plain = [live(1, d, d), live(2, d + 3e-4, d)];
        let stated = [
            at_hz(live(1, d, d), hz.clone()),
            at_hz(live(2, d + 3e-4, d), hz.clone()),
        ];
        same_bits(&run(c, &[None; 2], &stated), &run(c, &[None; 2], &plain));
    }
}

/// A live operand over a stored one of the same path, both measured with 20 ms, is 0 dB and
/// 0°: the stored operand states no bins, so neither turns.
#[test]
fn live_over_stored_at_one_delay_is_unity() {
    let d = 0.02;
    let c = binary(
        meas(1),
        MathOp::Divide,
        Operand::Trace { trace: TraceId(4) },
    );
    let stated = run(
        &c,
        &[None; 2],
        &[
            at_hz(live(1, d, d), off_centres(1.03)),
            stored(0.0, d, Some(EPOCH)),
        ],
    );
    let plain = run(
        &c,
        &[None; 2],
        &[live(1, d, d), stored(0.0, d, Some(EPOCH))],
    );
    same_bits(&stated, &plain);
    assert!(stated.mag.iter().all(|m| *m == 0.0), "{:?}", stated.mag);
    assert!(stated.phase.iter().all(|p| *p == 0.0), "{:?}", stated.phase);
}

/// Two live operands whose bins lie at different frequencies (another analysis ladder) have
/// no common frequency to turn at: both are taken at the centres, as without the statement.
#[test]
fn different_ladders_are_referred_at_the_centres() {
    let c = binary(meas(1), MathOp::Divide, meas(2));
    let (da, db) = (0.0125, 0.01);
    let stated = run(
        &c,
        &[None; 2],
        &[
            at_hz(live(1, da, da), off_centres(1.03)),
            at_hz(live(2, db, db), off_centres(0.97)),
        ],
    );
    let plain = run(&c, &[None; 2], &[live(1, da, da), live(2, db, db)]);
    same_bits(&stated, &plain);
}

/// A live operand minus a stored copy of it, both measured with 20 ms and referred to a
/// fixed 0 s: turning only the live one would leave 360·(f − centre)·20 ms between them;
/// both are taken at the centres and the difference nulls.
#[test]
fn live_minus_its_stored_copy_nulls_under_a_fixed_reference() {
    let d = 0.02;
    let mut c = binary(
        meas(1),
        MathOp::Subtract,
        Operand::Trace { trace: TraceId(4) },
    );
    c.reference = MathReference::Fixed {
        delay: Seconds(0.0),
    };
    let stated = run(
        &c,
        &[None; 2],
        &[
            at_hz(live(1, d, d), off_centres(1.03)),
            stored(0.0, d, Some(EPOCH)),
        ],
    );
    let plain = run(
        &c,
        &[None; 2],
        &[live(1, d, d), stored(0.0, d, Some(EPOCH))],
    );
    same_bits(&stated, &plain);
    assert!(
        stated.mag.iter().all(|m| m.is_nan() || *m < -100.0),
        "{:?}",
        stated.mag
    );
}

/// `A ÷ B` of two live operands on one ladder, A's path 2.5 ms later: each was referred to
/// its delay at its bins, so the ratio is `e^{−j2π·f·2.5 ms}` at the bins' frequency, which
/// above a few kHz reads apart from the value at the centre.
#[test]
fn ratio_of_one_ladder_turns_at_the_bins() {
    let hz = off_centres(1.05);
    let centres = grid().centres();
    let (da, db) = (0.0125, 0.01);
    let c = binary(meas(1), MathOp::Divide, meas(2));
    let f = run(
        &c,
        &[None; 2],
        &[
            at_hz(live(1, da, da), hz.clone()),
            at_hz(live(2, db, db), hz.clone()),
        ],
    );
    assert_eq!(f.meta.delay, Seconds(0.0));
    let mut apart = 0;
    for (i, (h, c)) in hz.iter().zip(&centres).enumerate() {
        let got = f64::from(f.phase[i]);
        let miss = wrap_deg(got + 360.0 * h * (da - db));
        assert!(miss.abs() < 1e-2, "column {i}: {got} off by {miss}");
        assert!(f.mag[i].abs() < 1e-6, "column {i}: {}", f.mag[i]);
        apart += usize::from(wrap_deg(got + 360.0 * c * (da - db)).abs() > 0.1);
    }
    assert!(apart > centres.len() / 4, "{apart}");
}
