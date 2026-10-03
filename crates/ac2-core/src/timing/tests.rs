//! Simulated duplex streams: a generator writes the output stream, the loopback capture is
//! that stream delayed by a (possibly time-varying) offset plus noise.

use super::*;
use crate::generator::{
    BandLimit, Generator, GeneratorConfig, LevelControl, Rng, Signal, dbfs_to_rms,
};

const FS: f64 = 48_000.0;

/// Unoptimised builds instantiate the FFT generics at opt-level 0 (≈15 ms per 2^16
/// transform), so debug runs simulate shorter spans; release runs the full spans.
fn by_build(debug: usize, release: usize) -> usize {
    if cfg!(debug_assertions) {
        debug
    } else {
        release
    }
}
const STIMULUS_DBFS: f64 = -20.0;

type OffsetFn = Box<dyn Fn(u64) -> f64>;

struct Sim {
    generator: Generator,
    level: LevelControl,
    out: Vec<f32>,
    out_base: u64,
    cap: Vec<f32>,
    cap_base: u64,
    offset: OffsetFn,
    noise: Rng,
    noise_rms: f64,
    /// Output index at which the generator fades out, and at which it fades back in.
    stop_at: Option<u64>,
    restart_at: Option<u64>,
}

impl Sim {
    fn new(signal: Signal, snr_db: f64, offset: OffsetFn) -> Self {
        let generator = Generator::new(&GeneratorConfig {
            signal,
            sample_rate: FS,
            seed: 42,
            band: BandLimit::NONE,
            level_dbfs: STIMULUS_DBFS,
            ceiling_dbfs: -10.0,
        })
        .expect("generator");
        let level = generator.level_control();
        Self {
            generator,
            level,
            out: Vec::new(),
            out_base: 0,
            cap: Vec::new(),
            cap_base: 0,
            offset,
            noise: Rng::new(7),
            noise_rms: dbfs_to_rms(STIMULUS_DBFS - snr_db),
            stop_at: None,
            restart_at: None,
        }
    }

    fn out_end(&self) -> u64 {
        self.out_base + self.out.len() as u64
    }

    fn cap_end(&self) -> u64 {
        self.cap_base + self.cap.len() as u64
    }

    fn render_out_to(&mut self, end: u64) {
        let mut block = [0.0f32; 1024];
        while self.out_end() < end {
            let at = self.out_end();
            if self.stop_at.is_some_and(|s| at >= s) && self.restart_at.is_none_or(|r| at < r) {
                self.level.fade_out();
            } else {
                self.level.set_muted(false);
            }
            self.generator.fill(&mut block);
            self.out.extend_from_slice(&block);
        }
    }

    fn out_at(&self, i: i64) -> f32 {
        if i < self.out_base as i64 {
            0.0
        } else {
            self.out[(i - self.out_base as i64) as usize]
        }
    }

    fn render_cap_to(&mut self, end: u64) {
        while self.cap_end() < end {
            let c = self.cap_end();
            let src = c as f64 - (self.offset)(c);
            let i = src.floor();
            let frac = (src - i) as f32;
            self.render_out_to((i as i64 + 2).max(0) as u64);
            let a = self.out_at(i as i64);
            let b = self.out_at(i as i64 + 1);
            let noise = self.noise.uniform_unit_rms() * self.noise_rms;
            self.cap.push(a + (b - a) * frac + noise as f32);
        }
    }

    fn window(&mut self, cfg: &TimingConfig, start: u64, range: LagRange) -> (Vec<f32>, Vec<f32>) {
        let w = cfg.window;
        self.render_cap_to(start + w as u64);
        let r0 = range.reference_start(start);
        let len = range.reference_len(w);
        self.render_out_to((r0 + len as i64).max(0) as u64);
        let reference = (0..len as i64).map(|j| self.out_at(r0 + j)).collect();
        let s = (start - self.cap_base) as usize;
        let capture = self.cap[s..s + w].to_vec();
        // Keep a few seconds of history so long runs stay small.
        let keep = 3 * FS as u64;
        if start > self.cap_base + 2 * keep {
            let drop = (start - keep - self.cap_base) as usize;
            self.cap.drain(..drop);
            self.cap_base += drop as u64;
        }
        let out_keep_from = (r0 - 2 * FS as i64).max(0) as u64;
        if out_keep_from > self.out_base + 2 * keep {
            let drop = (out_keep_from - self.out_base) as usize;
            self.out.drain(..drop);
            self.out_base += drop as u64;
        }
        (capture, reference)
    }
}

struct Record {
    start: u64,
    state: TimingState,
    events: Vec<TimingEvent>,
    measurement: WindowMeasurement,
}

/// Runs `windows` hops starting at capture index `first`.
fn run(sim: &mut Sim, mon: &mut LoopbackTiming, first: u64, windows: usize) -> Vec<Record> {
    let cfg = *mon.config();
    (0..windows)
        .map(|k| {
            let start = first + (k * cfg.hop) as u64;
            let range = mon.search_range();
            let (capture, reference) = sim.window(&cfg, start, range);
            let (measurement, ev) = mon
                .process_window(start, &capture, &reference, range)
                .expect("window");
            Record {
                start,
                state: mon.tracker().state(),
                events: ev.iter().copied().collect(),
                measurement,
            }
        })
        .collect()
}

fn events(rec: &[Record]) -> Vec<TimingEvent> {
    rec.iter().flat_map(|r| r.events.iter().copied()).collect()
}

fn jumps(rec: &[Record]) -> Vec<(i64, i64)> {
    events(rec)
        .iter()
        .filter_map(|e| match e {
            TimingEvent::Jump { from, to, .. } => Some((*from, *to)),
            _ => None,
        })
        .collect()
}

fn constant(d: f64) -> OffsetFn {
    Box::new(move |_| d)
}

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

fn step_offset(d0: f64, delta: f64, at: u64) -> OffsetFn {
    Box::new(move |c| if c < at { d0 } else { d0 + delta })
}

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

/// A slow sweep starts as a near-tone (20–35 Hz over a whole window). PHAT gives the bins
/// outside that band unit weight too; with a rectangular capture window they carry only the
/// leakage of its ends, which lines up with the ends of the reference slice and reads as a
/// confident offset exactly on an end of the searched range (`min` or `max`), reported as an
/// output timing jump. The window must give no estimate rather than a wrong one: pink noise
/// locks, the generator stops, and the first seconds of a 20 Hz – 20 kHz, 12 s sweep follow.
#[test]
fn a_slow_sweep_start_never_reads_as_a_range_edge() {
    use crate::generator::EssConfig;
    const DELAY: usize = 900;
    let mk = |signal| {
        Generator::new(&GeneratorConfig {
            signal,
            sample_rate: FS,
            seed: 3,
            band: BandLimit::NONE,
            level_dbfs: -50.0,
            ceiling_dbfs: -10.0,
        })
        .expect("generator")
    };
    let n = |s: f64| (s * FS) as usize;
    let mut out = vec![0.0f32; n(2.0)];
    mk(Signal::Pink).fill(&mut out);
    out.extend(std::iter::repeat_n(0.0, n(1.0)));
    let rate = 12.0 / 1000f64.ln();
    let mut sweep = vec![0.0f32; n(by_build(3, 6) as f64)];
    mk(Signal::Ess(EssConfig {
        start_hz: 20.0,
        end_hz: 20_000.0,
        duration_s: 12.0,
        fade_in_s: rate * std::f64::consts::LN_2 / 6.0,
        fade_out_s: rate * std::f64::consts::LN_2 / 24.0,
    }))
    .fill(&mut sweep);
    out.extend_from_slice(&sweep);
    let mut noise = Rng::new(9);
    let noise_rms = dbfs_to_rms(-110.0);
    let cap: Vec<f32> = (0..out.len())
        .map(|i| {
            let s = i.checked_sub(DELAY).map_or(0.0, |j| out[j]);
            s + (noise.uniform_unit_rms() * noise_rms) as f32
        })
        .collect();

    let cfg = TimingConfig::for_rate(FS);
    let mut mon = LoopbackTiming::new(cfg);
    let mut ev = Vec::new();
    let mut start = 0u64;
    loop {
        let range = mon.search_range();
        let r0 = range.reference_start(start);
        let len = range.reference_len(cfg.window);
        if r0 + len as i64 > out.len() as i64 {
            break;
        }
        let reference: Vec<f32> = (r0..r0 + len as i64)
            .map(|i| usize::try_from(i).map_or(0.0, |i| out[i]))
            .collect();
        let capture = &cap[start as usize..start as usize + cfg.window];
        let (m, e) = mon
            .process_window(start, capture, &reference, range)
            .expect("window");
        if let Outcome::Offset(p) = m.outcome {
            assert!(
                (p.offset - DELAY as i64).abs() <= 1,
                "window at {:.2} s read offset {} (searched {range:?})",
                start as f64 / FS,
                p.offset
            );
        }
        ev.extend(e.iter().copied());
        start += cfg.hop as u64;
    }
    assert!(
        ev.iter()
            .any(|e| matches!(e, TimingEvent::Locked { offset, .. } if *offset == DELAY as i64)),
        "{ev:?}"
    );
    assert!(
        !ev.iter().any(|e| matches!(e, TimingEvent::Jump { .. })),
        "{ev:?}"
    );
}
