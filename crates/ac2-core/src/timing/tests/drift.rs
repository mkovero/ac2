//! Clock drift: warnings, following the slope, and telling drift from jumps.

use super::*;

#[test]
fn drift_of_20_ppm_warns_within_30_s() {
    let cfg = TimingConfig::for_rate(FS);
    let mut mon = LoopbackTiming::new(cfg);
    let d0 = 500.0;
    let mut sim = Sim::new(Signal::Pink, 10.0, Box::new(move |c| d0 + 20e-6 * c as f64));
    let first = cfg.window as u64 + 600;
    let windows = by_build(52, (30.0 / HOP_SECONDS) as usize);
    let rec = run(&mut sim, &mut mon, first, windows);
    let warning = rec
        .iter()
        .find_map(|r| {
            r.events.iter().find_map(|e| match e {
                TimingEvent::DriftWarning { ppm, .. } => Some((r.start, *ppm)),
                _ => None,
            })
        })
        .expect("drift warning within 30 s");
    println!(
        "drift warning after {:.2} s at {:.2} ppm",
        (warning.0 + cfg.window as u64) as f64 / FS,
        warning.1
    );
    assert!((warning.1 - 20.0).abs() < 2.0, "{} ppm", warning.1);
    assert!(jumps(&rec).is_empty(), "{:?}", events(&rec));
    assert!(!mon.tracker().internal_reference_allowed());
    let d = mon.tracker().drift().expect("drift");
    println!("drift at end: {:.3} ppm over {:.1} s", d.ppm, d.span_s);
    assert!((d.ppm - 20.0).abs() < 1.0);
    // The sub-sample estimate follows the true fractional offset.
    for r in &rec[rec.len() - 20..] {
        if let Outcome::Offset(p) = r.measurement.outcome {
            let mid = r.start + cfg.window as u64 / 2;
            let truth = d0 + 20e-6 * mid as f64;
            let est = p.offset as f64 + p.fraction;
            assert!((est - truth).abs() < 0.3, "estimate {est:.3} vs {truth:.3}");
        }
    }
}

#[test]
fn stable_clock_reports_no_drift() {
    let cfg = TimingConfig::for_rate(FS);
    let mut mon = LoopbackTiming::new(cfg);
    let mut sim = Sim::new(Signal::White, 10.0, constant(2400.0));
    let rec = run(&mut sim, &mut mon, cfg.window as u64 + 2400, 48);
    let d = mon.tracker().drift().expect("drift");
    println!("stable clock: {:.4} ppm over {:.1} s", d.ppm, d.span_s);
    assert!(!d.warning && d.ppm.abs() < 0.2);
    assert!(jumps(&rec).is_empty());
}

fn drift_warnings(rec: &[Record]) -> Vec<f64> {
    events(rec)
        .iter()
        .filter_map(|e| match e {
            TimingEvent::DriftWarning { ppm, .. } => Some(*ppm),
            _ => None,
        })
        .collect()
}

/// PLAN §12's example, 600 µs in 6 s: the offset moves 1.2 samples per hop, more than the
/// agreement tolerance, and must read as drift, never as a stream of timing jumps.
#[test]
fn drift_of_100_ppm_is_followed_not_jumped() {
    for ppm in [-100.0, 100.0, 200.0] {
        let cfg = TimingConfig::for_rate(FS);
        let mut mon = LoopbackTiming::new(cfg);
        let mut sim = Sim::new(
            Signal::Pink,
            10.0,
            Box::new(move |c| 5000.0 + ppm * 1e-6 * c as f64),
        );
        let rec = run(
            &mut sim,
            &mut mon,
            cfg.window as u64 + 5000,
            by_build(50, 80),
        );
        assert!(jumps(&rec).is_empty(), "{ppm}: {:?}", events(&rec));
        assert!(matches!(mon.tracker().state(), TimingState::Locked { .. }));
        let d = mon.tracker().drift().expect("drift");
        println!("{ppm} ppm: {:.3} ppm over {:.1} s", d.ppm, d.span_s);
        assert!((d.ppm - ppm).abs() < 0.02 * ppm.abs(), "{ppm}: {d:?}");
        if cfg!(debug_assertions) {
            continue;
        }
        let w = drift_warnings(&rec);
        assert_eq!(w.len(), 1, "{ppm}: {w:?}");
        assert!((w[0] - ppm).abs() < 0.02 * ppm.abs(), "{ppm}: {w:?}");
    }
}

/// One dropped output frame is a timing jump of one sample, not a clock drift: an
/// unmodelled step of one sample biases a 10 s regression by 3 ppm.
#[test]
fn one_sample_step_is_a_jump_not_drift() {
    let cfg = TimingConfig::for_rate(FS);
    let mut mon = LoopbackTiming::new(cfg);
    let at = (5.0 * FS) as u64;
    let mut sim = Sim::new(Signal::Pink, 10.0, step_offset(500.0, 1.0, at));
    let rec = run(
        &mut sim,
        &mut mon,
        cfg.window as u64 + 600,
        by_build(40, 80),
    );
    assert_eq!(jumps(&rec), vec![(500, 501)], "{:?}", events(&rec));
    assert!(drift_warnings(&rec).is_empty(), "{:?}", events(&rec));
    let d = mon.tracker().drift().expect("drift");
    println!(
        "after a one-sample step: {:.3} ppm over {:.1} s",
        d.ppm, d.span_s
    );
    assert!(d.ppm.abs() < 0.3 && !d.warning, "{d:?}");
}

/// A jump during drift is measured against the drifted offset and taken out of the line:
/// one jump, one drift warning, the slope unchanged.
#[test]
fn jump_during_drift_keeps_the_slope() {
    let cfg = TimingConfig::for_rate(FS);
    let mut mon = LoopbackTiming::new(cfg);
    let at = (6.0 * FS) as u64;
    let mut sim = Sim::new(
        Signal::Pink,
        10.0,
        Box::new(move |c| 2000.0 + 50e-6 * c as f64 - if c < at { 0.0 } else { 17.0 }),
    );
    let rec = run(
        &mut sim,
        &mut mon,
        cfg.window as u64 + 2000,
        by_build(56, 80),
    );
    let j = jumps(&rec);
    assert_eq!(j.len(), 1, "{:?}", events(&rec));
    // `from` is the last followed offset, before the confirming windows drifted on.
    assert!((j[0].1 - j[0].0 + 17).abs() <= 3, "{j:?}");
    let d = mon.tracker().drift().expect("drift");
    println!(
        "50 ppm with a −17 jump: {:.3} ppm over {:.1} s",
        d.ppm, d.span_s
    );
    assert!((d.ppm - 50.0).abs() < 1.0, "{d:?}");
    if !cfg!(debug_assertions) {
        assert_eq!(drift_warnings(&rec).len(), 1, "{:?}", events(&rec));
    }
}

/// The clocks keep drifting while the generator is off: the re-lock after a gap lands where
/// the line predicts (77 samples further at 80 ppm over 20 s), which is no jump, and the
/// judged drift stays shown through the gap.
#[test]
fn drift_across_a_stimulus_gap_relocks_without_a_jump() {
    let cfg = TimingConfig::for_rate(FS);
    let mut mon = LoopbackTiming::new(cfg);
    let mut sim = Sim::new(
        Signal::Pink,
        10.0,
        Box::new(move |c| 3000.0 + 80e-6 * c as f64),
    );
    sim.stop_at = Some((12.0 * FS) as u64);
    sim.restart_at = Some((32.0 * FS) as u64);
    let first = cfg.window as u64 + 3000;
    let windows = ((36.0 * FS - first as f64) / cfg.hop as f64) as usize;
    let rec = run(&mut sim, &mut mon, first, windows);
    assert!(jumps(&rec).is_empty(), "{:?}", events(&rec));
    let quiet = rec
        .iter()
        .filter(|r| r.state == TimingState::NoStimulus)
        .count();
    assert!(quiet > 60, "{quiet} windows without stimulus");
    let locks = events(&rec)
        .iter()
        .filter(|e| matches!(e, TimingEvent::Locked { .. }))
        .count();
    assert_eq!(locks, 2, "{:?}", events(&rec));
    assert_eq!(drift_warnings(&rec).len(), 1, "{:?}", events(&rec));
    let d = mon.tracker().drift().expect("drift");
    assert!((d.ppm - 80.0).abs() < 1.0 && d.warning, "{d:?}");
    assert!(!mon.tracker().internal_reference_allowed());
}

/// Beyond what one line can follow the monitor says Lost; it never stays Locked at an
/// offset no window confirms any more.
#[test]
fn drift_beyond_the_followed_range_is_lost_not_locked() {
    let cfg = TimingConfig::for_rate(FS);
    let mut mon = LoopbackTiming::new(cfg);
    let mut sim = Sim::new(
        Signal::Pink,
        10.0,
        Box::new(move |c| 500.0 + 1000e-6 * c as f64),
    );
    let rec = run(
        &mut sim,
        &mut mon,
        cfg.window as u64 + 600,
        by_build(60, 80),
    );
    // A lock that stops matching gives way within the windows a jump needs to confirm plus
    // the loss count.
    let patience = cfg.window.div_ceil(cfg.hop) + cfg.lost_after_windows + 1;
    let mut stale = 0;
    for r in &rec {
        let truth = 500.0 + 1000e-6 * (r.start as f64 + cfg.window as f64 / 2.0);
        match r.state {
            TimingState::Locked { offset } if (offset as f64 - truth).abs() > 8.0 => stale += 1,
            _ => stale = 0,
        }
        assert!(
            stale <= patience,
            "Locked at a stale offset, truth {truth:.1}"
        );
    }
    assert!(
        rec.iter().any(|r| r.state == TimingState::Lost),
        "{:?}",
        events(&rec)
    );
    assert!(drift_warnings(&rec).is_empty(), "{:?}", events(&rec));
}
