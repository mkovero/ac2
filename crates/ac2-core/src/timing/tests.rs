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

/// The sweep ac2 emits starts below the asked band and fades in over its first second: at
/// 96 kHz with 0.68 s windows, everything up to about 1.4 kHz is too narrow to time to a
/// sample. Those windows still have clear peaks, some at the edge of the searched range and
/// the rest pulled tens of samples short of the true lag, which read as a lock elsewhere, a
/// jump back and a tilted drift line. They must give no offset and leave the lock alone:
/// pink noise locks, then a 20 Hz – 40 kHz, 5.5 s sweep (emitted from 4.3 Hz) follows.
#[test]
fn an_extended_sweep_start_neither_jumps_nor_loses_the_lock() {
    use crate::grid::LogGrid;
    use crate::sweep::{DEFAULT_MAX_ORDER, SweepSpec, SweepTiming};
    const FS96: f64 = 96_000.0;
    const DELAY: usize = 1743;
    let spec = SweepSpec {
        ess: crate::generator::EssConfig {
            start_hz: 20.0,
            end_hz: 40_000.0,
            duration_s: 5.5,
            fade_in_s: 0.08,
            fade_out_s: 0.02,
        },
        level_dbfs: -10.0,
        sample_rate: FS96,
        max_order: DEFAULT_MAX_ORDER,
        gate_s: None,
        tail_s: None,
        grid: LogGrid::covering(48, 20.0, 40_000.0),
        lf_harmonics: crate::sweep::LfHarmonics::Standard,
    };
    let t = SweepTiming::new(&spec).expect("timing");
    assert!(t.emitted.start_hz < 5.0, "{:?}", t.emitted);
    let n = |s: f64| (s * FS96) as usize;
    let mut out = vec![0.0f32; n(2.0)];
    Generator::new(&GeneratorConfig {
        signal: Signal::Pink,
        sample_rate: FS96,
        seed: 3,
        band: BandLimit::NONE,
        level_dbfs: -20.0,
        ceiling_dbfs: -10.0,
    })
    .expect("generator")
    .fill(&mut out);
    out.extend(std::iter::repeat_n(0.0, n(1.0)));
    let amp = dbfs_to_rms(spec.level_dbfs) * std::f64::consts::SQRT_2;
    out.extend((0..t.sweep_samples()).map(|k| (amp * t.plan.sample(k)) as f32));
    out.extend(std::iter::repeat_n(0.0, n(1.0)));
    let mut noise = Rng::new(9);
    let noise_rms = dbfs_to_rms(-110.0);
    let cap: Vec<f32> = (0..out.len())
        .map(|i| {
            let s = i.checked_sub(DELAY).map_or(0.0, |j| out[j]);
            s + (noise.uniform_unit_rms() * noise_rms) as f32
        })
        .collect();

    let cfg = TimingConfig::for_rate(FS96);
    let mut mon = LoopbackTiming::new(cfg);
    let mut ev = Vec::new();
    let mut narrow = 0;
    let mut start = 0u64;
    while start as usize + cfg.window <= cap.len() {
        let range = mon.search_range();
        let r0 = range.reference_start(start);
        let len = range.reference_len(cfg.window);
        let reference: Vec<f32> = (r0..r0 + len as i64)
            .map(|i| {
                usize::try_from(i)
                    .ok()
                    .and_then(|i| out.get(i).copied())
                    .unwrap_or(0.0)
            })
            .collect();
        let capture = &cap[start as usize..start as usize + cfg.window];
        let (m, e) = mon
            .process_window(start, capture, &reference, range)
            .expect("window");
        match m.outcome {
            Outcome::Offset(p) => assert!(
                (p.offset - DELAY as i64).abs() <= 1,
                "window at {:.2} s read offset {} (lobe {})",
                start as f64 / FS96,
                p.offset,
                p.lobe
            ),
            Outcome::NoEstimate(NoEstimate::Narrowband) => narrow += 1,
            _ => {}
        }
        ev.extend(e.iter().copied());
        start += cfg.hop as u64;
    }
    assert!(narrow >= 10, "{narrow} narrowband windows");
    let locks: Vec<_> = ev
        .iter()
        .filter(|e| matches!(e, TimingEvent::Locked { .. }))
        .collect();
    assert!(
        locks
            .iter()
            .all(|e| matches!(e, TimingEvent::Locked { offset, .. } if *offset == DELAY as i64))
            && !locks.is_empty(),
        "{ev:?}"
    );
    assert!(
        !ev.iter().any(|e| matches!(
            e,
            TimingEvent::Jump { .. } | TimingEvent::Lost { .. } | TimingEvent::DriftWarning { .. }
        )),
        "{ev:?}"
    );
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

/// Feeds the tracker directly: bursts of windows `len_s` long every `period_s`, silence
/// between them, each window timed at `offset(x)` (x its centre, capture samples).
fn bursts(
    cfg: &TimingConfig,
    count: usize,
    len_s: f64,
    period_s: f64,
    offset: impl Fn(f64, f64) -> f64,
) -> (TimingTracker, Vec<TimingEvent>) {
    let mut t = TimingTracker::new(*cfg);
    let mut ev = Vec::new();
    let windows = (period_s * cfg.sample_rate / cfg.hop as f64) as usize;
    let in_burst = (len_s * cfg.sample_rate / cfg.hop as f64) as usize;
    for b in 0..count {
        for k in 0..windows {
            let start = ((b * windows + k) * cfg.hop) as u64;
            let x = start as f64 + cfg.window as f64 / 2.0;
            let outcome = if k < in_burst {
                let y = offset(x, k as f64 / in_burst as f64);
                Outcome::Offset(Peak {
                    offset: y.round() as i64,
                    fraction: y - y.round(),
                    psr_db: 40.0,
                    lobe: 1,
                })
            } else {
                Outcome::NoStimulus
            };
            let m = WindowMeasurement {
                capture_start: start,
                outcome,
                loopback_dbfs: -20.0,
                stimulus_dbfs: if k < in_burst { -20.0 } else { -200.0 },
            };
            ev.extend(t.observe(&m).iter().copied());
        }
    }
    (t, ev)
}

/// A sweep times the loopback at the group delay of the frequency it is at, plus the
/// estimator's band-dependent bias, so on one clock its offset still rises half a sample
/// in 2.25 s (2.3 ppm; pupu's FF400 loopback). Extrapolated across the 20 s to the next
/// sweep that slope predicts 4 samples too much: no sweep may read as a jump, and the
/// slope of single sweeps may not add up to drift.
#[test]
fn the_offset_rising_within_each_sweep_is_neither_jump_nor_drift() {
    let cfg = TimingConfig::for_rate(96_000.0);
    let (t, ev) = bursts(&cfg, 6, 2.25, 22.0, |_, u| 1742.6 + 0.5 * u);
    let jumps: Vec<_> = ev
        .iter()
        .filter(|e| {
            matches!(
                e,
                TimingEvent::Jump { .. } | TimingEvent::DriftWarning { .. }
            )
        })
        .collect();
    assert!(jumps.is_empty(), "{jumps:?}");
    let d = t.drift().expect("drift");
    assert!(d.ppm.abs() < 0.5, "{d:?}");
}

/// The same sweeps on clocks 5 ppm apart: the offset moves 10 samples between sweeps,
/// which the line follows (no jump) and judges as drift.
#[test]
fn drift_between_sweeps_is_followed_and_judged() {
    let cfg = TimingConfig::for_rate(96_000.0);
    let (t, ev) = bursts(&cfg, 6, 2.25, 22.0, |x, u| 1742.6 + 0.5 * u + 5e-6 * x);
    assert!(
        !ev.iter().any(|e| matches!(e, TimingEvent::Jump { .. })),
        "{ev:?}"
    );
    let d = t.drift().expect("drift");
    assert!(d.warning && (d.ppm - 5.0).abs() < 1.0, "{d:?}");
}

/// A step across a gap is still a jump once it exceeds what the slope's uncertainty allows
/// over that gap.
#[test]
fn a_step_between_sweeps_is_a_jump() {
    let cfg = TimingConfig::for_rate(96_000.0);
    let step_at = 3.0 * 22.0 * cfg.sample_rate;
    let (_, ev) = bursts(&cfg, 6, 2.25, 22.0, |x, u| {
        1742.6 + 0.5 * u + if x > step_at { 12.0 } else { 0.0 }
    });
    let jumps: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            TimingEvent::Jump { from, to, .. } => Some((*from, *to)),
            _ => None,
        })
        .collect();
    assert_eq!(jumps.len(), 1, "{ev:?}");
    assert_eq!(jumps[0].1 - jumps[0].0, 12, "{jumps:?}");
}
