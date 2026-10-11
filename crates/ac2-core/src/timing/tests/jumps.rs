//! Single output timing jumps.

use super::*;

/// Output drops 17 frames: offset −17.
#[test]
fn single_jump_of_minus_17_detected_exactly_once() {
    single_jump(-17);
}

/// Output repeats 64 frames: offset +64, the edge of the tracking window.
#[test]
fn single_jump_of_plus_64_detected_exactly_once() {
    single_jump(64);
}

/// A slip far outside the tracking window is found by the wide search.
#[test]
fn single_jump_of_plus_3000_detected_exactly_once() {
    single_jump(3000);
}

fn single_jump(delta: i64) {
    let cfg = TimingConfig::for_rate(FS);
    let mut mon = LoopbackTiming::new(cfg);
    let d0 = 1000;
    let at = (3.0 * FS) as u64;
    let mut sim = Sim::new(Signal::Pink, 10.0, step_offset(d0 as f64, delta as f64, at));
    let rec = run(
        &mut sim,
        &mut mon,
        cfg.window as u64 + d0 as u64,
        by_build(26, 80),
    );
    assert_eq!(
        jumps(&rec),
        vec![(d0, d0 + delta)],
        "Δ {delta}: {:?}",
        events(&rec)
    );
    let jump_at = events(&rec)
        .iter()
        .find_map(|e| match e {
            TimingEvent::Jump {
                at_capture_sample, ..
            } => Some(*at_capture_sample),
            _ => None,
        })
        .expect("jump");
    // The first window showing the new offset either straddles the jump or is the first
    // one entirely after it.
    assert!(
        jump_at + cfg.window as u64 > at && jump_at < at + cfg.hop as u64,
        "jump at {at} located at {jump_at}"
    );
    let detected = rec
        .iter()
        .find(|r| matches!(r.state, TimingState::Jumped { .. }))
        .expect("jumped state");
    println!(
        "Δ {delta}: detected {:.2} s after the jump",
        (detected.start + cfg.window as u64 - at) as f64 / FS
    );
    assert_eq!(
        rec.last().map(|r| r.state),
        Some(TimingState::Locked { offset: d0 + delta })
    );
}
