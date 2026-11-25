use std::sync::mpsc::channel;

use ac2_audio::BlockFlags;
use ac2_core::mic_curve::MicCurve;

use super::*;
use crate::calstore::InputCal;
use crate::jobs::{JobCmd, JobCtx, StampArgs};

#[test]
fn routing_check() {
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let noise: Vec<f32> = (0..4800)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ((x >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.2
        })
        .collect();
    let mut c = RoutingCheck::default();
    // Same signal on both inputs, different gain.
    let half: Vec<f32> = noise.iter().map(|v| v * 0.5).collect();
    c.push(&noise, &half);
    assert_eq!(c.take(-80.0), (Routing::Identical, 4800));
    // Reference silent, measurement live.
    c.push(&vec![0.0; 4800], &noise);
    assert_eq!(c.take(-80.0), (Routing::ReferenceSilent, 4800));
    // Delayed copy: not flagged.
    let delayed: Vec<f32> = std::iter::repeat_n(0.0, 37)
        .chain(noise.iter().copied())
        .take(4800)
        .collect();
    c.push(&noise, &delayed);
    assert_eq!(c.take(-80.0).0, Routing::Clean);
    // Nothing at all: not flagged.
    c.push(&vec![0.0; 4800], &vec![0.0; 4800]);
    assert_eq!(c.take(-80.0).0, Routing::Clean);
}

/// Room noise hovering at the level rule's threshold (present one interval in three, as
/// on the rig with no reference connected) never raises CHECK ROUTING; a standing finding
/// does, after a second, and clears only after two seconds without it.
#[test]
fn routing_latch_needs_a_standing_finding() {
    let dt = 1.0 / 30.0;
    let mut l = RoutingLatch::default();
    for k in 0..300 {
        assert!(!l.step(k % 3 == 0, dt), "flashed at interval {k}");
    }
    let mut raised_at = None;
    for k in 0..60 {
        if l.step(true, dt) && raised_at.is_none() {
            raised_at = Some(k);
        }
    }
    let r = raised_at.expect("raised");
    assert!((28..=30).contains(&r), "raised after {r} intervals");
    // A short gap keeps it up; a long one clears it.
    for _ in 0..30 {
        assert!(l.step(false, dt));
    }
    assert!(l.step(true, dt));
    let mut cleared = false;
    for _ in 0..70 {
        cleared |= !l.step(false, dt);
    }
    assert!(cleared);
}

/// A stage frame takes the mic curve off at each bin's own frequency, and a new curve (here:
/// none) changes the next frame of the same averages, so its pace sends it again.
#[test]
fn stage_bins_take_the_mic_curve_off_per_bin() {
    const FS: u32 = 48_000;
    const BLOCK: usize = 4800;
    let curve = MicCurve::from_points(&[(10.0, -6.0), (1000.0, 0.0), (20_000.0, 4.0)])
        .expect("a curve")
        .normalised(1000.0);
    let mut cfg = TransferConfig::with_inputs(0, 1);
    cfg.resolution = TransferResolution::Bins;
    let (tx, _rx) = channel();
    let mut job = Transfer::new(
        JobCtx {
            meas: MeasId(1),
            cfg,
            config_rev: Rev(1),
            epoch: StreamEpoch(1),
        },
        FS,
        (0, 1),
        &DelayState {
            applied: Seconds(0.0),
            applied_samples: 0.0,
            nudged: Seconds(0.0),
            nudged_samples: 0.0,
            tracking: false,
            awaiting_pick: false,
            last_finding: None,
        },
        tx,
        Some(&curve),
        false,
    )
    .expect("a transfer job");
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x as f64 / u64::MAX as f64 - 0.5) as f32
    };
    for b in 0..30u64 {
        let data = (0..BLOCK)
            .flat_map(|_| {
                let r = next();
                [r, 0.5 * r + 0.05 * next()]
            })
            .collect();
        job.push(&Block::new(
            b * BLOCK as u64,
            BLOCK as u32,
            2,
            BlockFlags::NONE,
            0,
            data,
        ));
    }
    let stamp = StampArgs {
        audio_sample: 30 * BLOCK as u64,
        config_rev: Rev(1),
        applied_at: 0,
        wall_ns: 0,
        grid_id: None,
        protection: ProtectionFlags::NONE,
    };
    let shared = job.shared();
    let b = job.bins.as_mut().expect("at the bins resolution");
    let stages = b.layout().stages.len();
    let corrected: Vec<_> = (0..stages).map(|i| b.frame(i, &job.mtw, &shared)).collect();
    let before: Vec<_> = (0..stages)
        .map(|i| b.generation(i, &job.mtw, &stamp, &shared))
        .collect();
    // The same averages and curve form the same frame: the generation stays.
    for (i, g) in before.iter().enumerate() {
        assert_eq!(b.generation(i, &job.mtw, &stamp, &shared), *g);
    }

    job.command(JobCmd::Cal(Box::new(InputCal::none())));
    let shared = job.shared();
    let b = job.bins.as_mut().expect("at the bins resolution");
    for (i, c) in corrected.iter().enumerate() {
        assert!(c.meta.mic_curve && c.meta.blocks > 0);
        let plain = b.frame(i, &job.mtw, &shared);
        assert!(!plain.meta.mic_curve);
        assert_eq!(plain.meta.blocks, c.meta.blocks);
        let bin_hz = c.meta.bin_hz.0;
        let mut differs = 0;
        for (j, (m, p)) in c.mag.iter().zip(&plain.mag).enumerate() {
            assert!(m.is_finite(), "stage {i} bin {j}");
            let k = c.meta.first_bin as usize + j;
            let want = f64::from(*p) - curve.db(k as f64 * bin_hz);
            assert!(
                (f64::from(*m) - want).abs() < 1e-4,
                "stage {i} bin {k}: {m} {want}"
            );
            differs += usize::from((m - p).abs() > 0.01);
        }
        assert!(differs > 0, "stage {i}");
        assert!(b.generation(i, &job.mtw, &stamp, &shared) > before[i]);
    }
}
