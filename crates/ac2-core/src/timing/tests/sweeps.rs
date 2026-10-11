//! Sweeps and burst stimuli.

use super::*;

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
