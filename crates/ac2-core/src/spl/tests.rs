use super::*;
use ac2_testkit::golden::GoldenSet;
use std::f64::consts::TAU;

const RATES: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];

fn meter(fs: f64, w: Weighting, tw: TimeWeighting, pw: PeakWeighting) -> SplMeter {
    SplMeter::new(SplMeterConfig {
        fs,
        weighting: w,
        time_weighting: tw,
        peak_weighting: pw,
    })
    .expect("meter")
}

fn sine(f: f64, amp: f64, fs: f64, n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| amp * (TAU * f * i as f64 / fs).sin())
        .collect()
}

/// A 4 kHz toneburst of `tb` seconds (whole samples) after `pre` seconds of silence,
/// followed by `post` seconds of silence.
fn burst(fs: f64, amp: f64, pre: f64, tb: f64, post: f64) -> Vec<f64> {
    let (npre, nb, npost) = (
        (pre * fs).round() as usize,
        (tb * fs).round() as usize,
        (post * fs).round() as usize,
    );
    let mut x = vec![0.0; npre + nb + npost];
    for k in 0..nb {
        x[npre + k] = amp * (TAU * 4000.0 * k as f64 / fs).sin();
    }
    x
}

#[test]
fn detector_toneburst_matches_golden() {
    let g = GoldenSet::load("spl_toneburst").expect("golden");
    let fs = g.parameter("fs_hz").and_then(|v| v.as_f64()).expect("fs");
    let amp = 0.5;
    for (tw, tbs, key) in [
        (TimeWeighting::Fast, "tb_f_ms", "response_f_db"),
        (TimeWeighting::Slow, "tb_s_ms", "response_s_db"),
    ] {
        let tb = g.f64(tbs).expect("tb");
        let resp: Vec<f64> = tb
            .iter()
            .map(|&ms| {
                let x = burst(fs, amp, 0.01, ms * 1e-3, 5.0 * tw.rise_s());
                let mut d = TimeWeightedDetector::new(tw, fs);
                let max = x.iter().fold(0.0f64, |m, &v| m.max(d.push(v)));
                10.0 * (max / (amp * amp / 2.0)).log10()
            })
            .collect();
        g.assert_f64(key, &resp);
    }
}

/// F, S and I against the analytic detector of `tools/refgen` (`spl_time_weighting`):
/// the level after a step of the mean square (10·lg(1 − e^(−t/τ)), τ = 125 ms, 1 s,
/// 35 ms), its fall after the signal stops (−4.343·t/τ dB, τ = 125 ms, 1 s, 1.5 s) and the
/// I-weighted maximum of 4 kHz tonebursts.
#[test]
fn time_weighting_matches_golden() {
    let g = GoldenSet::load("spl_time_weighting").expect("golden");
    let fs = g.parameter("fs_hz").and_then(|v| v.as_f64()).expect("fs");
    let tws = [
        ("f", TimeWeighting::Fast),
        ("s", TimeWeighting::Slow),
        ("i", TimeWeighting::Impulse),
    ];
    let step_ms = g.f64("step_ms").expect("step_ms");
    let decay_ms = g.f64("decay_ms").expect("decay_ms");
    // Feeds `x` until sample `upto` (exclusive) and returns the mean square there.
    let run = |d: &mut TimeWeightedDetector, n: &mut usize, upto: usize, x: f64| {
        let mut ms = d.mean_square();
        while *n < upto {
            ms = d.push(x);
            *n += 1;
        }
        ms
    };
    for (name, tw) in tws {
        // Step: x = 1 from sample 0, read at the last sample of the first t seconds.
        let mut d = TimeWeightedDetector::new(tw, fs);
        let mut n = 0usize;
        let levels: Vec<f64> = step_ms
            .iter()
            .map(|&t| 10.0 * run(&mut d, &mut n, (t * 1e-3 * fs).round() as usize, 1.0).log10())
            .collect();
        g.assert_f64(&format!("step_{name}_db"), &levels);
        // Fall: 10 s of x = 1, then silence.
        let mut d = TimeWeightedDetector::new(tw, fs);
        let mut n = 0usize;
        let at_stop = run(&mut d, &mut n, (10.0 * fs) as usize, 1.0);
        let mut n = 0usize;
        let levels: Vec<f64> = decay_ms
            .iter()
            .map(|&t| {
                let ms = run(&mut d, &mut n, (t * 1e-3 * fs).round() as usize, 0.0);
                10.0 * (ms / at_stop).log10()
            })
            .collect();
        g.assert_f64(&format!("decay_{name}_db"), &levels);
        // The analytic fall the golden values follow, as a sanity bound.
        for (&t, l) in decay_ms.iter().zip(&levels) {
            let want = -10.0 * std::f64::consts::E.log10() * t * 1e-3 / tw.fall_s();
            assert!(
                (l - want).abs() < 0.01,
                "{tw:?} fall at {t} ms: {l} vs {want}"
            );
        }
    }
    let tb = g.f64("tb_i_ms").expect("tb");
    let amp = 0.5;
    let resp: Vec<f64> = tb
        .iter()
        .map(|&ms| {
            let x = burst(fs, amp, 0.01, ms * 1e-3, 0.2);
            let mut d = TimeWeightedDetector::new(TimeWeighting::Impulse, fs);
            let max = x.iter().fold(0.0f64, |m, &v| m.max(d.push(v)));
            10.0 * (max / (amp * amp / 2.0)).log10()
        })
        .collect();
    g.assert_f64("response_i_db", &resp);
}

/// IEC 60651 Impulse single-burst responses of an A-weighted 4 kHz tone (−3.6, −8.8 and
/// −12.6 dB at 20, 5 and 2 ms, type 1 tolerances ±1.5, ±2 and −4/+2 dB) at every rate, and
/// the held value: 1 s after a 5 ms burst LAI has fallen 2.9 dB, LAF some 35 dB.
#[test]
fn impulse_bursts_per_rate() {
    let amp = 0.5;
    for fs in RATES {
        let mut m = meter(fs, Weighting::A, TimeWeighting::Impulse, PeakWeighting::Z);
        m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
        m.reset_interval();
        m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
        let steady = m.levels().leq;
        for (tb_ms, dref, minus, plus) in [
            (20.0, -3.6, 1.5, 1.5),
            (5.0, -8.8, 2.0, 2.0),
            (2.0, -12.6, 4.0, 2.0),
        ] {
            let x = burst(fs, amp, 0.01, tb_ms * 1e-3, 0.5);
            let mut m = meter(fs, Weighting::A, TimeWeighting::Impulse, PeakWeighting::Z);
            m.process(&x, |_| {});
            let d = m.levels().lmax - steady - dref;
            assert!(d >= -minus && d <= plus, "{fs} Tb={tb_ms}: {d:.3} dB");
            assert!(
                d.abs() < 0.15,
                "{fs} Tb={tb_ms}: {d:.3} dB off the analytic"
            );
        }
        let x = burst(fs, amp, 0.01, 0.005, 1.0);
        let mut m = meter(fs, Weighting::A, TimeWeighting::Impulse, PeakWeighting::Z);
        m.process(&x, |_| {});
        let l = m.levels();
        let fast = m.levels_of(Weighting::A, TimeWeighting::Fast, PeakWeighting::Z);
        assert!(
            (l.lmax - l.level - 2.9).abs() < 0.1,
            "{fs}: {}",
            l.lmax - l.level
        );
        assert!(
            fast.lmax - fast.level > 30.0,
            "{fs}: F {}",
            fast.lmax - fast.level
        );
    }
}

/// A meter switched to other weightings mid-run reads exactly what a meter built with
/// them from the start reads: level, Lmax, Lmin, Leq and Lpeak over the same interval.
#[test]
fn select_reads_every_combination_of_the_same_interval() {
    let fs = 48_000.0;
    let mut x = sine(1000.0, 0.5, fs, (fs * 3.0) as usize);
    x.extend(sine(63.0, 0.05, fs, (fs * 4.0) as usize));
    x.extend(burst(fs, 0.3, 0.0, 0.2, 0.5));
    let (a, b) = x.split_at(x.len() / 2);
    let mut switched = meter(fs, Weighting::A, TimeWeighting::Fast, PeakWeighting::C);
    switched.process(a, |_| {});
    let bits =
        |v: Levels| [v.level, v.lmax, v.lmin, v.leq, v.lpeak, v.duration_s].map(f64::to_bits);
    for w in [Weighting::A, Weighting::C, Weighting::Z] {
        for tw in [
            TimeWeighting::Fast,
            TimeWeighting::Slow,
            TimeWeighting::Impulse,
        ] {
            for pw in [PeakWeighting::C, PeakWeighting::Z] {
                let mut s = switched.clone();
                s.select(w, tw, pw);
                s.process(b, |_| {});
                let mut fresh = meter(fs, w, tw, pw);
                fresh.process(&x, |_| {});
                let (l, f) = (s.levels(), fresh.levels());
                assert_eq!(bits(l), bits(f), "{w:?} {tw:?} {pw:?}");
                assert_eq!(s.config().weighting, w);
                assert_eq!(s.leq().level_dbfs().to_bits(), f.leq.to_bits());
            }
        }
    }
}

/// IEC 61672-1 Table 4: A-weighted 4 kHz toneburst responses for F and S (maximum
/// time-weighted level) and sound exposure level, within class 1 limits, per rate.
#[test]
fn toneburst_table4_class1_per_rate() {
    // (Tb ms, δref F, class 1 −, +), (… S …), (… LE …)
    let f_rows = [
        (1000.0, 0.0, 0.5, 0.5),
        (500.0, -0.1, 0.5, 0.5),
        (200.0, -1.0, 0.5, 0.5),
        (100.0, -2.6, 1.0, 1.0),
        (50.0, -4.8, 1.0, 1.0),
        (20.0, -8.3, 1.0, 1.0),
        (10.0, -11.1, 1.0, 1.0),
        (5.0, -14.1, 1.0, 1.0),
        (2.0, -18.0, 1.5, 1.0),
        (1.0, -21.0, 2.0, 1.0),
        (0.5, -24.0, 2.5, 1.0),
        (0.25, -27.0, 3.0, 1.0),
    ];
    let s_rows = [
        (1000.0, -2.0, 0.5, 0.5),
        (500.0, -4.1, 0.5, 0.5),
        (200.0, -7.4, 0.5, 0.5),
        (100.0, -10.2, 1.0, 1.0),
        (50.0, -13.1, 1.0, 1.0),
        (20.0, -17.0, 1.5, 1.0),
        (10.0, -20.0, 2.0, 1.0),
        (5.0, -23.0, 2.5, 1.0),
        (2.0, -27.0, 3.0, 1.0),
    ];
    let amp = 0.5;
    for fs in RATES {
        // Steady A-weighted level of the 4 kHz tone (the reference for every δ).
        let mut m = meter(fs, Weighting::A, TimeWeighting::Fast, PeakWeighting::Z);
        m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
        m.reset_interval();
        m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
        let l_steady = m.levels().leq;
        let mut worst: f64 = 0.0;
        for (tw, rows) in [
            (TimeWeighting::Fast, &f_rows[..]),
            (TimeWeighting::Slow, &s_rows[..]),
        ] {
            for &(tb_ms, dref, minus, plus) in rows {
                let x = burst(fs, amp, 0.01, tb_ms * 1e-3, 5.0 * tw.rise_s());
                let mut m = meter(fs, Weighting::A, tw, PeakWeighting::Z);
                m.process(&x, |_| {});
                let lv = m.levels();
                let d = lv.lmax - l_steady - dref;
                assert!(
                    d >= -minus && d <= plus,
                    "{fs} {tw:?} Tb={tb_ms} ms: deviation {d:.3} dB"
                );
                // Deviation from Formula 7 itself is far tighter than the class limits.
                let eq7 = 10.0 * (1.0 - (-tb_ms * 1e-3 / tw.rise_s()).exp()).log10();
                worst = worst.max((lv.lmax - l_steady - eq7).abs());
                if tw == TimeWeighting::Fast {
                    // LAE − LA = 10 lg(Tb / 1 s), class 1 limits as for F.
                    let de = lv.leq + 10.0 * lv.duration_s.log10() - l_steady;
                    let eref = 10.0 * (tb_ms * 1e-3f64).log10();
                    let dev = de - eref;
                    assert!(
                        dev >= -minus && dev <= plus,
                        "{fs} LE Tb={tb_ms} ms: deviation {dev:.3} dB"
                    );
                    let le = m.leq().exposure_level_dbfs();
                    assert!((le - (lv.leq + 10.0 * lv.duration_s.log10())).abs() < 1e-9);
                }
            }
        }
        eprintln!("{fs} Hz: max |toneburst response − Formula 7| = {worst:.3} dB");
        assert!(worst < 0.2, "{fs}: {worst}");
    }
}

/// Decay after a steady 4 kHz tone stops: 34.7 dB/s (F, +3.8/−3.7), 4.3 dB/s
/// (S, +0.8/−0.7) per IEC 61672-1 5.8; Impulse falls with 1.5 s → 2.9 dB/s.
#[test]
fn decay_rates() {
    let fs = 48_000.0;
    for (tw, rate, minus, plus) in [
        (TimeWeighting::Fast, 34.7, 3.7, 3.8),
        (TimeWeighting::Slow, 4.3, 0.7, 0.8),
        (TimeWeighting::Impulse, 2.895, 0.01, 0.01),
    ] {
        let mut m = meter(fs, Weighting::A, tw, PeakWeighting::Z);
        m.process(&sine(4000.0, 0.5, fs, (fs * 8.0) as usize), |_| {});
        let mut silence = vec![0.0; (fs * 0.2) as usize];
        m.process(&silence, |_| {});
        let l0 = m.levels().level;
        silence.resize((fs * 0.5) as usize, 0.0);
        m.process(&silence, |_| {});
        let l1 = m.levels().level;
        let measured = (l0 - l1) / 0.5;
        assert!(
            measured >= rate - minus && measured <= rate + plus,
            "{tw:?}: {measured} dB/s"
        );
    }
}

/// Steady 1 kHz sine: F, S, Impulse and Leq agree within ±0.1 dB (IEC 61672-1 5.8.3) and
/// read the sine's dBFS level (A(1 kHz) = 0 dB).
#[test]
fn steady_state_accuracy() {
    for fs in RATES {
        let amp = 0.5;
        let expect = 20.0 * f64::log10(amp);
        for tw in [
            TimeWeighting::Fast,
            TimeWeighting::Slow,
            TimeWeighting::Impulse,
        ] {
            let mut m = meter(fs, Weighting::A, tw, PeakWeighting::C);
            m.process(&sine(1000.0, amp, fs, (fs * 8.0) as usize), |_| {});
            m.reset_interval();
            m.process(&sine(1000.0, amp, fs, (fs * 2.0) as usize), |_| {});
            let lv = m.levels();
            assert!(
                (lv.level - expect).abs() < 0.01,
                "{fs} {tw:?} L {}",
                lv.level
            );
            assert!((lv.leq - expect).abs() < 0.001, "{fs} Leq {}", lv.leq);
            assert!((lv.level - lv.leq).abs() < 0.1);
            // Sine peak is 3.01 dB above its RMS level on this scale. Lpeak is a sample
            // peak, so at 1 kHz it may miss the crest by up to half a sample period
            // (cos(π·1000/44100) → −0.022 dB).
            assert!(
                (lv.lpeak - lv.leq - 3.0103).abs() < 0.025,
                "{}",
                lv.lpeak - lv.leq
            );
            assert!(lv.lmax >= lv.level && lv.lmin <= lv.level);
            // Fast ripple at 2 kHz is ~α/(2 sin(π·2f/fs)) relative: well under 0.01 dB.
            assert!(lv.lmax - lv.lmin < 0.01 || tw == TimeWeighting::Impulse);
        }
    }
}

/// Impulse: 35 ms rise and 1.5 s fall from a step in mean square (the 35 ms average has
/// long decayed when the 1.5 s fall is read).
#[test]
fn impulse_asymmetric_time_constants() {
    let fs = 48_000.0;
    let mut d = TimeWeightedDetector::new(TimeWeighting::Impulse, fs);
    let n_rise = (0.035 * fs) as usize;
    let mut ms = 0.0;
    for _ in 0..n_rise {
        ms = d.push(1.0);
    }
    assert!((ms - (1.0 - (-1.0f64).exp())).abs() < 1e-3, "{ms}");
    for _ in 0..(1.5 * fs) as usize {
        ms = d.push(0.0);
    }
    let expect = (1.0 - (-1.0f64).exp()) * (-1.0f64).exp();
    assert!((ms - expect).abs() < 1e-3, "{ms} vs {expect}");
}

/// Leq of a known signal: two equal-length segments at different levels, and exactness
/// of the f64 accumulation over 2²⁵ samples (≈ 11.7 min at 48 kHz).
#[test]
fn leq_known_signals() {
    let fs = 48_000.0;
    let mut m = meter(fs, Weighting::Z, TimeWeighting::Fast, PeakWeighting::Z);
    let a1 = 0.5;
    let a2 = 0.05;
    m.process(&sine(1000.0, a1, fs, 48_000), |_| {});
    m.process(&sine(1000.0, a2, fs, 48_000), |_| {});
    let expect = power_dbfs((a1 * a1 / 2.0 + a2 * a2 / 2.0) / 2.0);
    assert!((m.levels().leq - expect).abs() < 1e-6, "{}", m.levels().leq);
    assert!((m.levels().duration_s - 2.0).abs() < 1e-12);

    let mut leq = Leq::new(fs);
    let block = vec![1e-3; 1 << 15];
    for _ in 0..1 << 10 {
        leq.push(&block);
    }
    assert_eq!(leq.samples(), 1 << 25);
    assert!(
        (leq.mean_square() / 1e-6 - 1.0).abs() < 1e-13,
        "{}",
        leq.mean_square()
    );
    let mut by_sample = Leq::new(fs);
    for &v in &block {
        by_sample.push_sample(v);
    }
    assert!((by_sample.mean_square() / 1e-6 - 1.0).abs() < 1e-13);
    assert!(Leq::new(fs).mean_square().is_nan());
}

/// IEC 61672-1 Table 5: LCpeak − LC for one-cycle and half-cycle signals, class 1, per
/// rate. Frequencies are the exact ones (Annex D): 10^1.5, 10^2.7, 10^3.9 Hz.
#[test]
fn c_peak_table5_class1_per_rate() {
    let cases = [
        // (frequency, cycles: 1.0 full, 0.5 positive half, -0.5 negative half, ref, tol)
        (10f64.powf(1.5), 1.0f64, 2.5, 2.0),
        (10f64.powf(2.7), 1.0, 3.5, 1.0),
        (10f64.powf(3.9), 1.0, 3.4, 2.0),
        (10f64.powf(2.7), 0.5, 2.4, 1.0),
        (10f64.powf(2.7), -0.5, 2.4, 1.0),
    ];
    let amp = 0.25;
    for fs in RATES {
        for (f, cycles, dref, tol) in cases {
            let sign = if cycles < 0.0 { -1.0 } else { 1.0 };
            let n = (cycles.abs() / f * fs).round() as usize;
            let mut x = vec![0.0; (0.05 * fs) as usize];
            x.extend((0..n).map(|i| sign * amp * (TAU * f * i as f64 / fs).sin()));
            x.extend(std::iter::repeat_n(0.0, (2.0 * fs) as usize));
            let mut m = meter(fs, Weighting::C, TimeWeighting::Fast, PeakWeighting::C);
            m.process(&x, |_| {});
            let lcpeak = m.levels().lpeak;
            // LC of the steady sine, measured.
            let mut s = meter(fs, Weighting::C, TimeWeighting::Fast, PeakWeighting::C);
            s.process(&sine(f, amp, fs, (fs * 2.0) as usize), |_| {});
            s.reset_interval();
            s.process(&sine(f, amp, fs, (fs * 2.0) as usize), |_| {});
            let lc = s.levels().leq;
            let dev = lcpeak - lc - dref;
            assert!(
                dev.abs() <= tol,
                "{fs} f={f:.1} cycles={cycles}: LCpeak-LC = {:.2} (ref {dref})",
                lcpeak - lc
            );
            eprintln!(
                "{fs} Hz f={f:.1} cycles={cycles:+}: LCpeak-LC = {:.2} dB (ref {dref})",
                lcpeak - lc
            );
        }
    }
}

#[test]
fn sensitivity_and_calibrated_levels() {
    let s = Sensitivity::from_calibrator(94.0, -26.0);
    assert_eq!(s.offset_db, 120.0);
    assert_eq!(s.spl(-26.0), 94.0);
    // Calibrate on a 1 kHz tone, then read the same tone in dB SPL.
    let fs = 48_000.0;
    let mut m = meter(fs, Weighting::A, TimeWeighting::Slow, PeakWeighting::C);
    let amp = 10f64.powf(-26.0 / 20.0);
    m.process(&sine(1000.0, amp, fs, (fs * 8.0) as usize), |_| {});
    m.reset_interval();
    m.process(&sine(1000.0, amp, fs, fs as usize), |_| {});
    let cal = Sensitivity::from_calibrator(94.0, m.levels().leq);
    let spl = m.levels().calibrated(cal);
    assert_eq!(spl.scale, LevelScale::DbSpl);
    assert!((spl.leq - 94.0).abs() < 1e-12);
    assert!((spl.level - 94.0).abs() < 0.01);
}

mod chain;
