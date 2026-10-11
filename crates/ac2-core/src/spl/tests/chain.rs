use super::*;

/// A smooth mic model: 2nd-order Butterworth roll-off at 30 Hz and a first-order
/// shelf rising towards +6 dB above 8 kHz, sampled at 1/6-octave points.
fn mic_model_db(f: f64) -> f64 {
    let hp = 10.0 * (f.powi(4) / (f.powi(4) + 30f64.powi(4))).log10();
    let shelf = 10.0 * ((1.0 + (f / 4000.0).powi(2) * 4.0) / (1.0 + (f / 4000.0).powi(2))).log10();
    hp + shelf
}

fn mic_curve() -> crate::mic_curve::Correction {
    use crate::mic_curve::MicCurve;
    let pts: Vec<(f64, f64)> = (-36..=26)
        .map(|k| 1000.0 * 2f64.powf(f64::from(k) / 6.0))
        .map(|f| (f, mic_model_db(f)))
        .collect();
    MicCurve::from_points(&pts)
        .expect("curve")
        .normalised(1000.0)
}

fn mic_correction(fs: f64) -> Vec<f64> {
    mic_curve().design_fir(fs)
}

/// The calibrator frequency reads the same with and without the curve (nothing counts
/// twice); elsewhere a tone reads its level minus the curve; Lpeak is untouched.
#[test]
fn mic_correction_levels() {
    for fs in RATES {
        let h = mic_correction(fs);
        let c = mic_curve();
        for f in [1000.0, 8000.0, 31.5, 63.0, 12_500.0] {
            let curve_db = c.db(f);
            let amp = 0.25;
            // One continuous tone: a phase jump between the intervals would ring the LF boost.
            let x = sine(f, amp, fs, (fs * 6.0) as usize);
            let (x1, x2) = x.split_at(x.len() / 2);
            let mut plain = meter(fs, Weighting::Z, TimeWeighting::Slow, PeakWeighting::Z);
            let mut corr = meter(fs, Weighting::Z, TimeWeighting::Slow, PeakWeighting::Z);
            corr.set_correction(Some(&h));
            assert!(corr.has_correction() && !plain.has_correction());
            for m in [&mut plain, &mut corr] {
                m.process(x1, |_| {});
                m.reset_interval();
                m.process(x2, |_| {});
            }
            let (p, cl) = (plain.levels(), corr.levels());
            let err = cl.leq - (p.leq - curve_db);
            eprintln!("{fs} Hz, tone {f} Hz: corrected − expected = {err:+.4} dB");
            assert!(err.abs() < 0.02, "{fs} {f}: {err}");
            assert_eq!(cl.lpeak, p.lpeak, "Lpeak must stay on the raw path");
        }
    }
}

/// Toneburst responses relative to the steady level are unchanged by a correction in
/// the path (minimum phase: no pre-ringing, the filter's energy sits at its start).
#[test]
fn mic_correction_keeps_toneburst_response() {
    let fs = 48_000.0;
    let h = mic_correction(fs);
    let amp = 0.5;
    let steady = |corrected: bool| {
        let mut m = meter(fs, Weighting::A, TimeWeighting::Fast, PeakWeighting::Z);
        if corrected {
            m.set_correction(Some(&h));
        }
        m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
        m.reset_interval();
        m.process(&sine(4000.0, amp, fs, fs as usize), |_| {});
        m.levels().leq
    };
    let (s_corr, s_plain) = (steady(true), steady(false));
    for tw in [
        TimeWeighting::Fast,
        TimeWeighting::Slow,
        TimeWeighting::Impulse,
    ] {
        for tb_ms in [1000.0, 200.0, 50.0, 10.0, 2.0] {
            let x = burst(fs, amp, 0.01, tb_ms * 1e-3, 5.0 * tw.fall_s());
            let mut m = meter(fs, Weighting::A, tw, PeakWeighting::Z);
            m.set_correction(Some(&h));
            m.process(&x, |_| {});
            let mut r = meter(fs, Weighting::A, tw, PeakWeighting::Z);
            r.process(&x, |_| {});
            let d = (m.levels().lmax - s_corr) - (r.levels().lmax - s_plain);
            // A 2 ms burst spreads ±500 Hz around 4 kHz, where the curve slopes, so it
            // is corrected slightly differently from the steady tone; the class 1
            // tolerance there is +1/−1.5 dB.
            let tol = if tb_ms < 5.0 { 0.1 } else { 0.02 };
            assert!(d.abs() < tol, "{tw:?} Tb={tb_ms}: {d}");
        }
    }
}

#[test]
fn lmax_lmin_track_level_changes() {
    let fs = 48_000.0;
    let mut m = meter(fs, Weighting::Z, TimeWeighting::Fast, PeakWeighting::Z);
    let n = (2.0 * fs) as usize;
    m.process(&sine(1000.0, 0.5, fs, n), |_| {});
    m.process(&sine(1000.0, 0.05, fs, n), |_| {});
    m.process(&sine(1000.0, 0.5, fs, n), |_| {});
    let lv = m.levels();
    assert!((lv.lmax - power_dbfs(0.125)).abs() < 0.01, "{}", lv.lmax);
    assert!((lv.lmin - power_dbfs(0.00125)).abs() < 0.05, "{}", lv.lmin);
    // Lmin ignores the detector's rise from silence after a full reset.
    m.reset();
    m.process(&sine(1000.0, 0.5, fs, n), |_| {});
    assert!((m.levels().lmin - power_dbfs(0.125)).abs() < 0.05);
}

/// Noise whose level steps between −10 and −50 dBFS every 0.7 s, so the detectors rise
/// and fall and the sums mix very different magnitudes.
fn stepped_noise(fs: f64, seconds: f64) -> Vec<f64> {
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let step = (0.7 * fs) as usize;
    (0..(seconds * fs) as usize)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let u = (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
            let amp = if (i / step).is_multiple_of(2) {
                0.6
            } else {
                0.006
            };
            amp * u
        })
        .collect()
}

/// Neumaier sum: the reference totals below.
#[derive(Default)]
struct Exact {
    s: f64,
    c: f64,
}

impl Exact {
    fn add(&mut self, v: f64) {
        let t = self.s + v;
        if self.s.abs() >= v.abs() {
            self.c += (self.s - t) + v;
        } else {
            self.c += (v - t) + self.s;
        }
        self.s = t;
    }

    fn get(&self) -> f64 {
        self.s + self.c
    }
}

/// The meter's filters, detectors, Leq and per-second energies built one by one, every
/// sum compensated per sample: what the shared chain must reproduce. `taps` (when
/// given) is convolved in f64 over the whole signal and delayed by the partition, as
/// the meter's correction.
struct Reference {
    levels: [[Levels; 3]; 3],
    leq: [f64; 3],
    seconds: Vec<[f64; 3]>,
    /// Per second: highest C peak² and LAF mean square.
    maxima: Vec<[f64; 2]>,
}

fn reference(x: &[f64], fs: f64, taps: Option<&[f64]>) -> Reference {
    let xc: Vec<f64> = match taps {
        None => x.to_vec(),
        Some(h) => {
            let lat = crate::mic_curve::fir_partition(fs);
            let full = convolve_f64(x, h);
            (0..x.len())
                .map(|n| if n < lat { 0.0 } else { full[n - lat] })
                .collect()
        }
    };
    let mut a = WeightingFilter::new(Weighting::A, fs).expect("A");
    let mut c = WeightingFilter::new(Weighting::C, fs).expect("C");
    let mut pc = WeightingFilter::new(Weighting::C, fs).expect("C");
    let mut det = [0; 3].map(|_| TIME_WEIGHTINGS.map(|t| TimeWeightedDetector::new(t, fs)));
    let lat = taps.map_or(0, |_| crate::mic_curve::fir_partition(fs) as u64);
    let settle = TIME_WEIGHTINGS.map(|t| settle_samples(t, fs) + lat);
    let mut max = [[0.0f64; 3]; 3];
    let mut min = [[f64::INFINITY; 3]; 3];
    let mut leq: [Exact; 3] = Default::default();
    let mut sec: [Exact; 3] = Default::default();
    let mut seconds = Vec::new();
    let mut pk_c = 0.0f64;
    let mut sec_max = [0.0f64; 2];
    let mut maxima = Vec::new();
    let per = fs as usize;
    for (n, (&xr, &xv)) in x.iter().zip(&xc).enumerate() {
        let y = [a.process_sample(xv), c.process_sample(xv), xv];
        let p = pc.process_sample(xr);
        pk_c = pk_c.max(p.abs());
        sec_max[0] = sec_max[0].max(p * p);
        for w in 0..3 {
            leq[w].add(y[w] * y[w]);
            sec[w].add(y[w] * y[w]);
            for t in 0..3 {
                let ms = det[w][t].push(y[w]);
                if (w, t) == (0, 0) {
                    sec_max[1] = sec_max[1].max(ms);
                }
                max[w][t] = max[w][t].max(ms);
                if n as u64 >= settle[t] {
                    min[w][t] = min[w][t].min(ms);
                }
            }
        }
        if (n + 1) % per == 0 {
            seconds.push([0, 1, 2].map(|w| sec[w].get() / fs));
            sec = Default::default();
            maxima.push(sec_max);
            sec_max = [0.0; 2];
        }
    }
    let levels = [0, 1, 2].map(|w| {
        [0, 1, 2].map(|t| Levels {
            scale: LevelScale::Dbfs,
            level: det[w][t].level_dbfs(),
            lmax: power_dbfs(max[w][t]),
            lmin: power_dbfs(min[w][t]),
            leq: power_dbfs(leq[w].get() / x.len() as f64),
            lpeak: power_dbfs(pk_c * pk_c),
            duration_s: x.len() as f64 / fs,
        })
    });
    Reference {
        levels,
        leq: [0, 1, 2].map(|w| leq[w].get()),
        seconds,
        maxima,
    }
}

/// Linear convolution in f64 through one large FFT.
fn convolve_f64(x: &[f64], h: &[f64]) -> Vec<f64> {
    let n = (x.len() + h.len()).next_power_of_two();
    let mut planner = realfft::RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let spec = |v: &[f64]| {
        let mut t = vec![0.0; n];
        t[..v.len()].copy_from_slice(v);
        let mut s = fwd.make_output_vec();
        fwd.process(&mut t, &mut s).expect("fft");
        s
    };
    let (sx, sh) = (spec(x), spec(h));
    let mut p: Vec<_> = sx.iter().zip(&sh).map(|(a, b)| a * b).collect();
    let last = p.len() - 1;
    p[0].im = 0.0;
    p[last].im = 0.0;
    let mut y = vec![0.0; n];
    inv.process(&mut p, &mut y).expect("ifft");
    y.iter().map(|v| v / n as f64).collect()
}

fn run_meter(x: &[f64], fs: f64, taps: Option<&[f64]>) -> (SplMeter, Vec<Second>) {
    let mut m = meter(fs, Weighting::A, TimeWeighting::Fast, PeakWeighting::C);
    m.set_correction(taps);
    let mut out = Vec::new();
    // Odd block sizes: chunk, partition and second boundaries all fall mid-block.
    let mut i = 0;
    for size in [256usize, 1000, 37, 4096, 511].iter().cycle() {
        if i >= x.len() {
            break;
        }
        let e = (i + size).min(x.len());
        m.process(&x[i..e], |s| out.push(s));
        i = e;
    }
    (m, out)
}

/// Without a correction the shared chain is the separate filters sample for sample:
/// detectors, Lmax, Lmin and Lpeak are bit-identical to a reference built from
/// independent filters, and Leq and the per-second energies (chunked sums) agree to
/// 10⁻¹² relative.
#[test]
fn one_chain_matches_separate_paths() {
    for fs in [48_000.0, 96_000.0] {
        let x = stepped_noise(fs, 6.0);
        let r = reference(&x, fs, None);
        let (m, secs) = run_meter(&x, fs, None);
        for (wi, w) in [Weighting::A, Weighting::C, Weighting::Z]
            .into_iter()
            .enumerate()
        {
            for (ti, t) in TIME_WEIGHTINGS.into_iter().enumerate() {
                let got = m.levels_of(w, t, PeakWeighting::C);
                let want = r.levels[wi][ti];
                assert_eq!(got.level.to_bits(), want.level.to_bits(), "{w:?} {t:?}");
                assert_eq!(got.lmax.to_bits(), want.lmax.to_bits(), "{w:?} {t:?}");
                assert_eq!(got.lmin.to_bits(), want.lmin.to_bits(), "{w:?} {t:?}");
                assert_eq!(got.lpeak.to_bits(), want.lpeak.to_bits(), "{w:?} {t:?}");
            }
            let e = m.leq[wi].energy();
            assert!((e / r.leq[wi] - 1.0).abs() < 1e-12, "{fs} {w:?} Leq");
        }
        assert_eq!(secs.len(), r.seconds.len());
        for (s, want) in secs.iter().zip(&r.seconds) {
            assert_eq!(s.measured, 1.0);
            for (w, (g, v)) in s.energy.iter().zip(want).enumerate() {
                assert!((g / v - 1.0).abs() < 1e-12, "{fs} w{w}");
            }
        }
        // Each second's LCpeak and LAFmax: the same filters and detector, bit for bit.
        for (k, (s, want)) in secs.iter().zip(&r.maxima).enumerate() {
            assert_eq!(s.c_peak_sq.to_bits(), want[0].to_bits(), "{fs} second {k}");
            assert_eq!(s.af_max_ms.to_bits(), want[1].to_bits(), "{fs} second {k}");
        }
        // Loud and quiet stretches alternate every 0.7 s: the maxima follow them.
        let lcpeak: Vec<f64> = secs.iter().map(Second::lcpeak_dbfs).collect();
        assert!(
            lcpeak.iter().all(|l| l.is_finite() && *l > -20.0),
            "{lcpeak:?}"
        );
    }
}

/// With a mic-curve correction the f32 convolution is the only difference from an f64
/// reference: every level, the Leq and each second's A/C/Z level within 0.001 dB.
#[test]
fn corrected_chain_matches_f64_reference() {
    let fs = 48_000.0;
    let h = mic_correction(fs);
    let x = stepped_noise(fs, 4.0);
    let r = reference(&x, fs, Some(&h));
    let (m, secs) = run_meter(&x, fs, Some(&h));
    let mut worst = 0.0f64;
    for (wi, w) in [Weighting::A, Weighting::C, Weighting::Z]
        .into_iter()
        .enumerate()
    {
        for (ti, t) in TIME_WEIGHTINGS.into_iter().enumerate() {
            let got = m.levels_of(w, t, PeakWeighting::C);
            let want = r.levels[wi][ti];
            for (g, v) in [
                (got.level, want.level),
                (got.lmax, want.lmax),
                (got.lmin, want.lmin),
                (got.leq, want.leq),
            ] {
                worst = worst.max((g - v).abs());
            }
            assert_eq!(got.lpeak.to_bits(), want.lpeak.to_bits(), "{w:?} {t:?}");
        }
    }
    assert_eq!(secs.len(), r.seconds.len());
    for (s, want) in secs.iter().zip(&r.seconds) {
        for (g, v) in s.energy.iter().zip(want) {
            worst = worst.max((power_dbfs(*g) - power_dbfs(*v)).abs());
        }
    }
    eprintln!("corrected chain vs f64 reference: max |Δ| = {worst:.2e} dB");
    assert!(worst < 1e-3, "{worst}");
}

/// The per-second A and C levels of steady sines at the IEC 61672-1 Table 3 frequencies
/// (rounded to whole hertz, so a second holds whole cycles) follow the analytic
/// weighting of Annex E (checked against `tools/refgen` in [`crate::weighting`]) within
/// the filters' design bound, with and without a (flat) correction in the path.
#[test]
fn per_second_weighting_follows_iec_table() {
    let g = GoldenSet::load("weighting_iec61672").expect("golden");
    let f = g.f64("exact_hz").expect("f");
    for w in [Weighting::A, Weighting::C] {
        for fs in [48_000.0, 96_000.0] {
            let bound = if fs < 90_000.0 { 0.13 } else { 0.002 };
            for flat in [false, true] {
                let mut worst = 0.0f64;
                for fi in f.iter().map(|f| f.round()) {
                    if !(20.0..=16_000.0).contains(&fi) {
                        continue;
                    }
                    let mut m = meter(fs, w, TimeWeighting::Fast, PeakWeighting::C);
                    if flat {
                        m.set_correction(Some(&[1.0]));
                    }
                    let mut out = Vec::new();
                    m.process(&sine(fi, 0.5, fs, 3 * fs as usize), |s| out.push(s));
                    // The last second: the filters have settled and the partition
                    // delay has passed.
                    let l = out[2].level_dbfs(w) - power_dbfs(0.125);
                    let err = l - w.analytic_db(fi);
                    worst = worst.max(err.abs());
                    assert!(
                        err.abs() <= bound + 1e-3,
                        "{w:?} {fs} flat {flat} {fi} Hz: {err:.4} dB"
                    );
                }
                eprintln!("{w:?} {fs} correction {flat}: max |per-second − Annex E| {worst:.4} dB");
            }
        }
    }
}

/// The compensated total over plain chunk sums stays exact over 48 h at 48 kHz:
/// chunk energies built from samples k/2²⁰ (squares exact in f64) are checked against
/// the exact integer total.
#[test]
fn chunked_leq_exact_over_48_hours() {
    let fs = 48_000.0;
    let chunks = (48.0 * 3600.0 * fs / CHUNK as f64) as u64;
    let mut leq = Leq::new(fs);
    let mut exact: u128 = 0;
    let mut s = 0x2545_f491_4f6c_dd1du64;
    let unit = 2f64.powi(-40);
    for i in 0..chunks {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        // Loud for the first hours, then 60 dB quieter: the small sums must survive
        // the large total.
        let k = if i < chunks / 8 { s >> 44 } else { s >> 54 };
        // 256 samples of k/2²⁰: Σx² = 256·k²·2⁻⁴⁰, exact in f64.
        let k2 = u128::from(k) * u128::from(k) * CHUNK as u128;
        exact += k2;
        leq.push_energy(k2 as f64 * unit, CHUNK as u64);
    }
    let want = exact as f64 * unit;
    let rel = (leq.energy() / want - 1.0).abs();
    eprintln!("48 h compensated total: relative error {rel:.1e}");
    assert!(rel < 1e-15, "{rel}");
    assert_eq!(leq.samples(), chunks * CHUNK as u64);
}
