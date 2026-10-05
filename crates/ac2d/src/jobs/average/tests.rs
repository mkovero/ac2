//! The spatial average against analytic expectations: power and complex means of members
//! with known responses, member exclusion, and the per-member delay reference.

use ac2_proto::frame::{FrameStamp, MemberStatus, ProtectionFlags, TfFrame, TfMeta, ValidityMask};
use ac2_proto::grid::GridDef;
use ac2_proto::model::{AverageMethod, AverageReference, SpatialAverageConfig};
use ac2_proto::units::{
    DaemonIncarnation, MeasId, Rev, SampleIndex, Seconds, SessionEpoch, WallNs,
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

/// A member result: flat `db`, a path arriving `arrival` s late measured with `inserted`
/// s of delay (stored phase −360·f·(arrival − inserted)), γ² = 0.9.
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
                average: None,
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

fn cfg(members: &[u32], method: AverageMethod) -> SpatialAverageConfig {
    SpatialAverageConfig {
        method,
        ..SpatialAverageConfig::power_of(members.iter().map(|m| MeasId(*m)).collect())
    }
}

fn run(c: &SpatialAverageConfig, seen: &[Option<f64>], answers: &[Answer]) -> TfFrame {
    let g = grid();
    combine(MeasId(9), c, &g, &frequencies(&g), EPOCH, seen, answers)
}

fn statuses(f: &TfFrame) -> Vec<MemberStatus> {
    f.meta
        .average
        .as_ref()
        .map(|a| a.members.iter().map(|m| m.status).collect())
        .unwrap_or_default()
}

/// +3 dB and −3 dB members average by power to 10·log10((10^0.3 + 10^−0.3)/2) ≈ 0.96 dB;
/// a member with NO SIGNAL and a stopped one are left out and said to be.
#[test]
fn power_average_leaves_out_refused_and_stopped_members() {
    let c = cfg(&[1, 2, 3, 4], AverageMethod::Power);
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
            MemberStatus::Included,
            MemberStatus::Included,
            MemberStatus::Refused {
                protection: ProtectionFlags::NO_SIGNAL
            },
            MemberStatus::Stopped,
        ]
    );
    assert_eq!(f.meta.average.as_ref().map(|a| a.included()), Some(2));
}

/// A weak reference only holds a member's averaging: it stays in.
#[test]
fn weak_reference_does_not_refuse() {
    let c = cfg(&[1, 2], AverageMethod::Power);
    let answers = [
        flat(1, 0.0),
        member(2, 0.0, 0.0, 0.0, ProtectionFlags::WEAK_REFERENCE),
    ];
    let f = run(&c, &[None; 2], &answers);
    assert_eq!(statuses(&f), vec![MemberStatus::Included; 2]);
}

/// One usable member is not an average: no value anywhere, every column says why.
#[test]
fn fewer_than_two_members_refuse() {
    let c = cfg(&[1, 2, 3], AverageMethod::Power);
    let answers = [flat(1, 0.0), Answer::NoResult, Answer::Stopped];
    let f = run(&c, &[None; 3], &answers);
    assert!(f.mag.iter().all(|m| m.is_nan()));
    assert!(f.validity.iter().all(|v| *v == ValidityMask::FEW_MEMBERS));
    assert_eq!(
        statuses(&f),
        vec![
            MemberStatus::Included,
            MemberStatus::Settling,
            MemberStatus::Stopped
        ]
    );
}

/// Two members of the same 0 dB path, one measured 0.5 ms later than the other and aligned
/// by its own inserted delay: the complex average re-refers it to the first member's delay
/// and shows the acoustic comb |cos(π f τ)|; the power average stays flat at 0 dB.
#[test]
fn complex_average_rerefers_each_members_delay() {
    let tau = 0.5e-3;
    let answers = [
        member(1, 0.0, 0.010, 0.010, ProtectionFlags::NONE),
        member(2, 0.0, 0.010 + tau, 0.010 + tau, ProtectionFlags::NONE),
    ];
    let g = grid();
    let freqs = frequencies(&g);
    let f = run(&cfg(&[1, 2], AverageMethod::Complex), &[None; 2], &answers);
    assert_eq!(f.meta.delay, Seconds(0.010));
    for (k, hz) in freqs.iter().enumerate() {
        let want = (std::f64::consts::PI * hz * tau).cos().abs();
        let got = 10f64.powf(f64::from(f.mag[k]) / 20.0);
        // Within a column of a null the dB value is steep; compare linear magnitude.
        assert!((got - want).abs() < 1e-4, "{hz} Hz: {got} vs {want}");
    }
    let p = run(&cfg(&[1, 2], AverageMethod::Power), &[None; 2], &answers);
    assert!(p.mag.iter().all(|m| m.abs() < 1e-5));
}

/// The reference member left out: its newest seen delay still refers the phase, so the
/// average does not jump to another member's arrival.
#[test]
fn reference_member_left_out_keeps_its_delay() {
    let mut c = cfg(&[1, 2, 3], AverageMethod::Complex);
    c.reference = AverageReference::Member { meas: MeasId(1) };
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

/// A column a member has no value for has no average value, with that member's reason.
#[test]
fn a_member_gap_is_a_gap_in_the_average() {
    let c = cfg(&[1, 2], AverageMethod::Power);
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
        &cfg(&[1, 2], AverageMethod::CoherenceWeighted),
        &[None; 2],
        &answers,
    );
    let c = run(&cfg(&[1, 2], AverageMethod::Complex), &[None; 2], &answers);
    let want = 20.0 * ((10f64.powf(0.3) + 1.0) / 2.0).log10();
    assert!((f64::from(c.mag[5]) - want).abs() < 1e-5);
    assert!((w.mag[5] - c.mag[5]).abs() < 1e-5);
}
