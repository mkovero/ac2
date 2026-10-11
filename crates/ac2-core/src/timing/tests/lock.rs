//! Locking onto a fixed offset, across rates and down to 0 dB SNR.

use super::*;

#[test]
fn window_scales_with_rate() {
    assert_eq!(window_for_rate(48_000.0), 1 << 15);
    assert_eq!(window_for_rate(44_100.0), 1 << 15);
    assert_eq!(window_for_rate(96_000.0), 1 << 16);
    assert_eq!(window_for_rate(192_000.0), 1 << 17);
}

#[test]
fn fixed_offsets_lock_within_four_windows_white() {
    fixed_offsets(Signal::White);
}

#[test]
fn fixed_offsets_lock_within_four_windows_pink() {
    fixed_offsets(Signal::Pink);
}

fn fixed_offsets(signal: Signal) {
    for d in [0i64, 37, 4800, 48_000] {
        let cfg = TimingConfig::for_rate(FS);
        let mut mon = LoopbackTiming::new(cfg);
        let mut sim = Sim::new(signal, 20.0, constant(d as f64));
        let rec = run(&mut sim, &mut mon, d as u64 + cfg.window as u64, 5);
        let first_lock = rec
            .iter()
            .position(|r| r.state == TimingState::Locked { offset: d })
            .expect("locked");
        println!("{signal:?} offset {d}: locked at window {}", first_lock + 1);
        assert!(
            first_lock < 4,
            "{signal:?} {d}: locked at window {}",
            first_lock + 1
        );
        assert!(
            rec[first_lock..]
                .iter()
                .all(|r| r.state == TimingState::Locked { offset: d })
        );
        assert_eq!(events(&rec).len(), 1, "{signal:?} {d}: {:?}", events(&rec));
        if let Outcome::Offset(p) = rec[first_lock].measurement.outcome {
            assert!(
                p.fraction.abs() < 0.1,
                "integer delay reads fraction {}",
                p.fraction
            );
        }
        assert!(mon.tracker().internal_reference_allowed());
    }
}

fn zero_db_snr(signal: Signal, seconds: f64) {
    let cfg = TimingConfig::for_rate(FS);
    let mut mon = LoopbackTiming::new(cfg);
    let d = 12_345;
    let mut sim = Sim::new(signal, 0.0, constant(d as f64));
    let windows = (seconds / HOP_SECONDS) as usize;
    let rec = run(&mut sim, &mut mon, cfg.window as u64 + d as u64, windows);
    let first_lock = rec
        .iter()
        .position(|r| r.state == TimingState::Locked { offset: d })
        .expect("locked");
    let unlocked = rec[first_lock..]
        .iter()
        .filter(|r| r.state != TimingState::Locked { offset: d })
        .count();
    let min_psr = rec
        .iter()
        .filter_map(|r| match r.measurement.outcome {
            Outcome::Offset(p) => Some(p.psr_db),
            _ => None,
        })
        .fold(f64::MAX, f64::min);
    println!(
        "{signal:?} at 0 dB SNR, {seconds} s: locked at window {}, {unlocked} unlocked windows after, min PSR {min_psr:.1} dB",
        first_lock + 1
    );
    assert!(first_lock < 4);
    assert!(jumps(&rec).is_empty(), "{:?}", events(&rec));
    assert_eq!(unlocked, 0);
    assert!(!mon.tracker().drift().is_some_and(|d| d.warning));
}

#[test]
fn zero_db_snr_white_stays_locked() {
    zero_db_snr(Signal::White, by_build(8, 60) as f64);
}

#[test]
fn zero_db_snr_pink_stays_locked() {
    zero_db_snr(Signal::Pink, by_build(8, 60) as f64);
}

#[test]
#[ignore = "10 min simulated per stimulus; run with --release --ignored"]
fn zero_db_snr_ten_minutes() {
    zero_db_snr(Signal::White, 600.0);
    zero_db_snr(Signal::Pink, 600.0);
}
