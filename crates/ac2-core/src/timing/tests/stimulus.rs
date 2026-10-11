//! Stimulus stops, new epochs and silent loopback.

use super::*;

#[test]
fn stimulus_stop_goes_to_no_stimulus_without_jump() {
    let cfg = TimingConfig::for_rate(FS);
    let mut mon = LoopbackTiming::new(cfg);
    let d = 4800;
    let mut sim = Sim::new(Signal::Pink, 10.0, constant(d as f64));
    sim.stop_at = Some((3.0 * FS) as u64);
    sim.restart_at = Some((5.5 * FS) as u64);
    let rec = run(&mut sim, &mut mon, cfg.window as u64 + d as u64, 34);
    let ev = events(&rec);
    assert!(jumps(&rec).is_empty(), "{ev:?}");
    let offs: Vec<_> = ev
        .iter()
        .filter(|e| matches!(e, TimingEvent::StimulusOff { .. }))
        .collect();
    assert_eq!(offs.len(), 1, "{ev:?}");
    assert!(
        rec.iter().any(|r| r.state == TimingState::NoStimulus),
        "never reached NoStimulus"
    );
    assert!(
        !ev.iter().any(|e| matches!(e, TimingEvent::Lost { .. })),
        "{ev:?}"
    );
    // While stopped, the last lock is still reported for display with its age.
    let stopped = rec
        .iter()
        .find(|r| r.state == TimingState::NoStimulus)
        .expect("stopped");
    assert!(stopped.start < (5.5 * FS) as u64);
    let lock = mon.tracker().last_lock().expect("last lock");
    assert_eq!(lock.offset, d);
    // Restart re-locks at the same offset: a Locked event, no jump.
    let locks = ev
        .iter()
        .filter(|e| matches!(e, TimingEvent::Locked { offset, .. } if *offset == d))
        .count();
    assert_eq!(locks, 2, "{ev:?}");
    assert_eq!(
        rec.last().map(|r| r.state),
        Some(TimingState::Locked { offset: d })
    );
}

/// Judging a window from its newest W history samples first, and measuring it in full only
/// when the stimulus is present, gives exactly the measurements, events and states of full
/// windows throughout a stop and restart.
#[test]
fn no_stimulus_judged_from_newest_samples_matches_full_windows() {
    let cfg = TimingConfig::for_rate(FS);
    let mut full = LoopbackTiming::new(cfg);
    let mut cheap = LoopbackTiming::new(cfg);
    let d = 4800;
    let mut sim = Sim::new(Signal::Pink, 10.0, constant(d as f64));
    sim.stop_at = Some((3.0 * FS) as u64);
    sim.restart_at = Some((5.5 * FS) as u64);
    let first = cfg.window as u64 + d as u64;
    let mut quiet = 0;
    for k in 0..34 {
        let start = first + (k * cfg.hop) as u64;
        let range = full.search_range();
        assert_eq!(range, cheap.search_range());
        let (capture, reference) = sim.window(&cfg, start, range);
        let a = full
            .process_window(start, &capture, &reference, range)
            .expect("window");
        let newest = &reference[range.span()..];
        assert_eq!(newest.len(), cfg.window);
        let b = match cheap
            .process_if_no_stimulus(start, &capture, newest)
            .expect("window")
        {
            Some(b) => {
                quiet += 1;
                b
            }
            None => cheap
                .process_window(start, &capture, &reference, range)
                .expect("window"),
        };
        assert_eq!(a, b, "window {k}");
        assert_eq!(full.tracker().state(), cheap.tracker().state());
        assert_eq!(full.tracker().last_lock(), cheap.tracker().last_lock());
    }
    assert!(quiet >= 4, "only {quiet} windows without stimulus");
    let m = cheap
        .process_if_no_stimulus(0, &vec![0.0; cfg.window], &[0.0; 16])
        .err();
    assert_eq!(m, Some(TimingError::ReferenceLength));
}

#[test]
fn new_epoch_reacquires_without_jump() {
    let cfg = TimingConfig::for_rate(FS);
    let mut mon = LoopbackTiming::new(cfg);
    let at = (4.0 * FS) as u64;
    let mut sim = Sim::new(Signal::White, 10.0, step_offset(300.0, 2000.0, at));
    let first = cfg.window as u64 + 300;
    let rec1 = run(&mut sim, &mut mon, first, 8);
    assert_eq!(mon.tracker().state(), TimingState::Locked { offset: 300 });
    // The stream reopens (or an xrun is flagged); the offset may legitimately change.
    mon.new_epoch();
    assert_eq!(mon.tracker().state(), TimingState::Acquiring);
    let rec2 = run(&mut sim, &mut mon, at + FS as u64, 6);
    assert!(jumps(&rec1).is_empty() && jumps(&rec2).is_empty());
    assert_eq!(mon.tracker().state(), TimingState::Locked { offset: 2300 });
    assert!(events(&rec2).iter().any(|e| matches!(
        e,
        TimingEvent::Locked {
            epoch: 1,
            offset: 2300,
            ..
        }
    )));
}

#[test]
fn silent_loopback_is_never_a_measurement() {
    let cfg = TimingConfig::for_rate(FS);
    let mut est = GccPhat::new(cfg.window, cfg.acquisition.span());
    let range = LagRange { min: -64, max: 64 };
    let mut rng = Rng::new(1);
    let reference: Vec<f32> = (0..range.reference_len(cfg.window))
        .map(|_| (rng.uniform_unit_rms() * 0.1) as f32)
        .collect();
    let capture = vec![0.0f32; cfg.window];
    let m = est
        .measure(&cfg, 0, &capture, &reference, range)
        .expect("measure");
    assert_eq!(m.outcome, Outcome::NoEstimate(NoEstimate::LowLoopbackLevel));
    // Unrelated noise in the loopback: low confidence, never an offset.
    let capture: Vec<f32> = (0..cfg.window)
        .map(|_| (rng.uniform_unit_rms() * 0.1) as f32)
        .collect();
    let m = est
        .measure(&cfg, 0, &capture, &reference, cfg.acquisition)
        .err();
    assert_eq!(m, Some(TimingError::ReferenceLength));
    let m = est
        .measure(&cfg, 0, &capture, &reference, range)
        .expect("measure");
    assert_eq!(m.outcome, Outcome::NoEstimate(NoEstimate::LowConfidence));
}
