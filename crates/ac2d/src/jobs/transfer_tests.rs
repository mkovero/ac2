use super::*;

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
