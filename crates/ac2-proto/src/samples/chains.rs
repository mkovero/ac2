//! Delta chains: frames of one topic in send order, each a delta on the one before, with
//! the transitions a delta must carry exactly (NaN and ±∞, a phase crossing ±180°, a
//! `bfp16` exponent change, per-frame metadata).

use super::frames::{frames, tf_frame};
use crate::frame::{Frame, FrameData, ValidityMask};
use crate::units::*;

/// `f` one step later: `seq` up by `dseq`, newer sample and wall clock.
fn later(f: &Frame, dseq: u64) -> Frame {
    let mut g = f.clone();
    g.stamp.seq += dseq;
    g.stamp.audio_sample = SampleIndex(f.stamp.audio_sample.0 + 800 * dseq);
    g.stamp.capture_wall_ns = WallNs(f.stamp.capture_wall_ns.0 + 16_666_667 * dseq);
    g
}

fn tf_chain() -> Vec<Frame> {
    let f0 = tf_frame();
    let mut f1 = later(&f0, 1);
    let FrameData::Tf(t) = &mut f1.data else {
        unreachable!("a tf sample")
    };
    for x in &mut t.mag[10..20] {
        *x += 0.5;
    }
    t.phase[100] = 179.5;
    t.meta.filled = 0.75;
    // A conflated frame never went out: the next delta skips its `seq`.
    let mut f2 = later(&f1, 2);
    let FrameData::Tf(t) = &mut f2.data else {
        unreachable!("a tf sample")
    };
    t.phase[100] = -179.5;
    t.mag[2] = -3.25;
    t.mag[200] = f32::NAN;
    t.mag[201] = f32::INFINITY;
    t.phase[300] = f32::NAN;
    t.coh[50] = f32::NAN;
    t.validity[5] = ValidityMask(t.validity[5].0 ^ ValidityMask::THINNED.0);
    t.meta.filled = 0.875;
    let mut f3 = later(&f2, 1);
    let FrameData::Tf(t) = &mut f3.data else {
        unreachable!("a tf sample")
    };
    t.phase[300] = 12.0;
    t.mag[201] = f32::NEG_INFINITY;
    t.coh[50] = 0.5;
    vec![f0, f1, f2, f3]
}

fn of_kind(pick: impl Fn(&FrameData) -> bool) -> Frame {
    frames()
        .into_iter()
        .find(|f| pick(&f.data))
        .unwrap_or_else(|| unreachable!("every kind has a sample"))
}

fn ir_chain() -> Vec<Frame> {
    let f0 = of_kind(|d| matches!(d, FrameData::Ir(_)));
    let mut f1 = later(&f0, 1);
    let FrameData::Ir(r) = &mut f1.data else {
        unreachable!("an ir sample")
    };
    // The same peak, so the same exponent: a mantissa delta.
    r.linear[3] = 0.25;
    r.linear[4] = -0.0625;
    // Half the level: another exponent, so the array goes intra inside the delta.
    let mut f2 = later(&f1, 1);
    let FrameData::Ir(r) = &mut f2.data else {
        unreachable!("an ir sample")
    };
    for x in &mut r.linear {
        *x *= 0.5;
    }
    vec![f0, f1, f2]
}

fn spl_chain() -> Vec<Frame> {
    let f0 = of_kind(|d| matches!(d, FrameData::Spl(_)));
    let mut f1 = later(&f0, 1);
    let FrameData::Spl(s) = &mut f1.data else {
        unreachable!("an spl sample")
    };
    s.meta.level += 1.5;
    s.meta.lmax += 1.5;
    s.meta.duration = Seconds(s.meta.duration.0 + 0.05);
    vec![f0, f1]
}

/// Every chain, by name.
pub fn chains() -> Vec<(&'static str, Vec<Frame>)> {
    vec![("tf", tf_chain()), ("ir", ir_chain()), ("spl", spl_chain())]
}
