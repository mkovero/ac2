//! MTW engine: golden comparisons against scipy-built references and loopback tests with
//! synthetic signals whose answers are analytic.

use ac2_core::grid::LogGrid;
use ac2_core::mtw::{
    Averaging, ColumnSource, Ladder, Layout, Mtw, MtwConfig, MtwFrame, PairDecimator, SampleGate,
    StageAveraging, StageBins, Validity,
};
use ac2_testkit::golden::GoldenSet;
use num_complex::Complex64;
use std::f64::consts::PI;

// ---------------------------------------------------------------------------------------
// Signal helpers
// ---------------------------------------------------------------------------------------

/// Deterministic Gaussian noise (splitmix64 + Box–Muller).
struct Noise(u64);

impl Noise {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
    fn vec(&mut self, n: usize, rms: f64) -> Vec<f64> {
        (0..n).map(|_| rms * self.gauss()).collect()
    }
}

/// RBJ cookbook peaking EQ.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
}

impl Biquad {
    fn peaking(fs: f64, f0: f64, q: f64, gain_db: f64) -> Self {
        let a_lin = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f0 / fs;
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha / a_lin;
        Self {
            b: [
                (1.0 + alpha * a_lin) / a0,
                -2.0 * w0.cos() / a0,
                (1.0 - alpha * a_lin) / a0,
            ],
            a: [1.0, -2.0 * w0.cos() / a0, (1.0 - alpha / a_lin) / a0],
        }
    }
    fn filter(&self, x: &[f64]) -> Vec<f64> {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        x.iter()
            .map(|&v| {
                let y = self.b[0] * v + self.b[1] * x1 + self.b[2] * x2
                    - self.a[1] * y1
                    - self.a[2] * y2;
                (x2, x1, y2, y1) = (x1, v, y1, y);
                y
            })
            .collect()
    }
    fn response(&self, fs: f64, f: f64) -> Complex64 {
        let z1 = Complex64::from_polar(1.0, -2.0 * PI * f / fs);
        let z2 = z1 * z1;
        (self.b[0] + self.b[1] * z1 + self.b[2] * z2)
            / (self.a[0] + self.a[1] * z1 + self.a[2] * z2)
    }
}

/// `y[n] = s[n − d]` (zero before the start), for either sign of `d`, over `[0, len)`, where
/// `s` is indexed from `-lead`.
fn delayed(s: &[f64], lead: usize, d: i64, len: usize) -> Vec<f64> {
    (0..len as i64)
        .map(|n| {
            let i = n - d + lead as i64;
            if i >= 0 && (i as usize) < s.len() {
                s[i as usize]
            } else {
                0.0
            }
        })
        .collect()
}

fn grid48() -> LogGrid {
    LogGrid::covering(48, 20.0, 20_000.0)
}

fn engine(sr: f64, averaging: Averaging, delay: i64) -> Mtw {
    Mtw::new(MtwConfig {
        sample_rate_hz: sr,
        ladder: Ladder::Standard,
        averaging,
        grid: grid48(),
        delay_samples: delay,
    })
    .expect("engine")
}

/// Full-rate samples needed for every stage to fill its FIFO.
fn samples_to_fill(m: &Mtw) -> usize {
    m.layout()
        .stages
        .iter()
        .zip(m.stage_averaging())
        .map(|(s, a)| {
            let blocks = match a {
                StageAveraging::Fifo { blocks } => blocks,
                StageAveraging::Exponential { .. } => 1,
            };
            (s.nfft + (blocks - 1) * s.hop) * s.factor + s.filter_len()
        })
        .max()
        .expect("stages")
}

/// Push in irregular chunks (the result must not depend on chunking).
fn push_chunked(m: &mut Mtw, start: u64, x: &[f64], y: &[f64]) {
    let sizes = [997usize, 64, 4096, 1, 333, 12_000, 256];
    let mut i = 0;
    let mut c = 0;
    while i < x.len() {
        let e = (i + sizes[c % sizes.len()]).min(x.len());
        m.push(start + i as u64, &x[i..e], &y[i..e], SampleGate::Accept)
            .expect("push");
        i = e;
        c += 1;
    }
}

fn bins_freqs(m: &Mtw, b: &StageBins) -> Vec<f64> {
    let df = m.layout().stages[b.stage].bin_hz;
    (b.lo..b.hi).map(|k| k as f64 * df).collect()
}

/// Mean of `h` over a column's bins, blended as the engine blends.
fn expected_column(m: &Mtw, src: &ColumnSource, h: impl Fn(f64) -> Complex64) -> Complex64 {
    let mean = |b: &StageBins| {
        let f = bins_freqs(m, b);
        f.iter().map(|&f| h(f)).sum::<Complex64>() / f.len() as f64
    };
    match src {
        ColumnSource::Stage(b) => mean(b),
        ColumnSource::Blend {
            deep,
            shallow,
            shallow_weight: w,
        } => mean(deep) * (1.0 - w) + mean(shallow) * *w,
        ColumnSource::None => Complex64::new(f64::NAN, f64::NAN),
    }
}

fn valid(f: &MtwFrame) -> impl Iterator<Item = usize> + '_ {
    (0..f.freq_hz.len()).filter(|&i| f.columns[i].validity == Validity::Valid)
}

fn wrap_deg(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

// ---------------------------------------------------------------------------------------
// Golden
// ---------------------------------------------------------------------------------------

/// Single-stage configuration with the golden set's Welch parameters reproduces scipy's
/// Pxx, Pyy, Pxy, H1 and coherence; frame columns equal the ratio of summed golden spectra.
#[test]
fn golden_transfer_h1_biquad_delay() {
    let g = GoldenSet::load("transfer_h1_biquad_delay").expect("golden");
    let fs = g.parameter("fs_hz").and_then(|v| v.as_f64()).expect("fs");
    let nperseg = g
        .parameter("nperseg")
        .and_then(|v| v.as_u64())
        .expect("nperseg") as usize;
    let noverlap = g
        .parameter("noverlap")
        .and_then(|v| v.as_u64())
        .expect("noverlap") as usize;
    let x = g.f64("x").expect("x");
    let y = g.f64("y").expect("y");
    let mut m = Mtw::new(MtwConfig {
        sample_rate_hz: fs,
        ladder: Ladder::Single {
            nfft: nperseg,
            hop: nperseg - noverlap,
        },
        averaging: Averaging::Fifo { blocks: 10_000 },
        grid: grid48(),
        delay_samples: 0,
    })
    .expect("engine");
    push_chunked(&mut m, 0, &x, &y);
    let s = m.stage_spectra(0).expect("spectra");
    assert_eq!(s.blocks as f64, g.scalar("n_segments").expect("segments"));
    g.assert_f64("pxx", &s.gxx);
    g.assert_f64("pyy", &s.gyy);
    g.assert_c128("pxy", &s.gxy);
    let h1: Vec<Complex64> = s.gxy.iter().zip(&s.gxx).map(|(a, b)| a / b).collect();
    let coh: Vec<f64> = (0..s.gxx.len())
        .map(|k| s.gxy[k].norm_sqr() / (s.gxx[k] * s.gyy[k]))
        .collect();
    g.assert_c128("h1", &h1);
    g.assert_f64("coherence", &coh);

    let pxx = g.f64("pxx").expect("pxx");
    let pyy = g.f64("pyy").expect("pyy");
    let pxy = g.c128("pxy").expect("pxy");
    let f = m.frame();
    let mut checked = 0;
    for i in valid(&f) {
        let ColumnSource::Stage(b) = f.columns[i].source else {
            panic!("single stage never blends");
        };
        let sxx: f64 = pxx[b.lo..b.hi].iter().sum();
        let syy: f64 = pyy[b.lo..b.hi].iter().sum();
        let sxy: Complex64 = pxy[b.lo..b.hi].iter().sum();
        let h = sxy / sxx;
        assert!((f.h1[i] - h).norm() <= 1e-9 * h.norm(), "col {i}");
        let c = sxy.norm_sqr() / (sxx * syy);
        assert!((f.coherence[i] - c).abs() <= 1e-9, "col {i}");
        checked += 1;
    }
    // 93.75 Hz bins on a 1/48-octave grid: columns below κ·Δf ≈ 6.5 kHz thin out.
    assert!(checked > 100, "{checked}");
    assert!(f.columns.iter().any(|c| c.validity == Validity::Thinned));
}

/// Full ladder at 48 kHz: decimator taps, warm-up discard, phase and per-stage framing match
/// a pipeline built from scipy primitives, with alignment applied first.
#[test]
fn golden_mtw_stage_pipeline_48k() {
    let g = GoldenSet::load("mtw_stage_pipeline_48k").expect("golden");
    let delay = g
        .parameter("delay_samples")
        .and_then(|v| v.as_i64())
        .expect("delay");
    let x = g.f64("x").expect("x");
    let y = g.f64("y").expect("y");
    let mut m = engine(48_000.0, Averaging::Fifo { blocks: 10_000 }, delay);
    for s in 1..3 {
        let d = m.layout().stages[s].decimator.as_ref().expect("decimator");
        g.assert_f64(&format!("stage{s}_taps"), &d.taps);
        let fp = g.scalar(&format!("stage{s}_passband_hz")).expect("fp");
        let fstop = g.scalar(&format!("stage{s}_stopband_hz")).expect("fstop");
        assert!((d.passband_hz - fp).abs() < 1e-9 && (d.stopband_hz - fstop).abs() < 1e-9);
    }
    push_chunked(&mut m, 0, &x, &y);
    for s in 0..2 {
        let sp = m.stage_spectra(s).expect("stage spectra");
        assert_eq!(
            sp.blocks as f64,
            g.scalar(&format!("stage{s}_blocks")).expect("blocks"),
            "stage {s}"
        );
        g.assert_f64(&format!("stage{s}_freq_hz"), &sp.freq_hz);
        g.assert_f64(&format!("stage{s}_gxx"), &sp.gxx);
        g.assert_f64(&format!("stage{s}_gyy"), &sp.gyy);
        g.assert_c128(&format!("stage{s}_gxy"), &sp.gxy);
    }
    // Stage 2 needs 4096 × 12 samples for one block; 30 000 is not enough.
    assert!(m.stage_spectra(2).is_none());
}

// ---------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------

#[test]
fn stage_table() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let m = engine(sr, Averaging::Fifo { blocks: 8 }, 0);
        let l: &Layout = m.layout();
        println!("sr {sr}");
        for ((s, a), c) in l
            .stages
            .iter()
            .zip(m.stage_averaging())
            .zip(std::iter::once(None).chain(l.crossovers.iter().map(Some)))
        {
            let fifo = match a {
                StageAveraging::Fifo { blocks } => blocks,
                StageAveraging::Exponential { .. } => 0,
            };
            println!(
                "  stage {} M={:2} rate={:8.1} df={:7.4} win={:7.1} ms hop={:6.1} ms ov={:5.1}% \
                 taps={:3} served<= {:8.1} Hz  blend-into-shallower {:?}  fifo={} neff={:.2}",
                s.index,
                s.factor,
                s.rate_hz,
                s.bin_hz,
                1e3 * s.window_s(),
                1e3 * s.hop_s(),
                100.0 * s.overlap(),
                s.filter_len(),
                s.served_hi_hz,
                c.map(|c| (c.lo_hz.round(), c.hi_hz.round())),
                fifo,
                m.steady_eff_avg(s.index),
            );
            assert!(s.served_hi_hz <= 0.45 * s.rate_hz);
        }
        // Depth matched across stages in the model.
        let n0 = m.steady_eff_avg(0);
        for s in 1..l.stages.len() {
            let n = m.steady_eff_avg(s);
            assert!(
                n >= n0 * (1.0 - 1e-9) && n < n0 * 1.25,
                "stage {s}: {n} vs {n0}"
            );
        }
    }
}

/// Every decimator meets its passband and stopband specification at the common rates.
#[test]
fn decimator_passband_and_stopband() {
    for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        let l = Layout::new(sr, Ladder::Standard).expect("layout");
        for s in &l.stages[1..] {
            let d = s.decimator.as_ref().expect("decimator");
            let mut ripple: f64 = 0.0;
            for i in 0..=400 {
                let f = d.passband_hz * i as f64 / 400.0;
                ripple = ripple.max((20.0 * d.response(f).norm().log10()).abs());
            }
            let mut stop = f64::NEG_INFINITY;
            for i in 0..=4000 {
                let f = d.stopband_hz + (sr / 2.0 - d.stopband_hz) * i as f64 / 4000.0;
                stop = stop.max(20.0 * d.response(f).norm().log10());
            }
            println!(
                "sr {sr} stage {} taps {} ripple {ripple:.2e} dB stopband {stop:.1} dB",
                s.index,
                d.len()
            );
            assert!(
                ripple < 1e-3,
                "sr {sr} stage {}: ripple {ripple} dB",
                s.index
            );
            assert!(
                stop < -90.0,
                "sr {sr} stage {}: stopband {stop} dB",
                s.index
            );

            // End to end: a tone that would alias into the served band is gone; a tone in
            // the passband comes through at unity.
            let tone = |f: f64| -> f64 {
                let n = 40 * d.len() + 64 * s.factor;
                let x: Vec<f64> = (0..n)
                    .map(|i| (2.0 * PI * f * i as f64 / sr).sin())
                    .collect();
                let mut dec = PairDecimator::new(d.taps.clone(), s.factor);
                let (mut ox, mut oy) = (Vec::new(), Vec::new());
                dec.push(&x, &x, &mut ox, &mut oy);
                assert_eq!(ox, oy);
                // Least-squares fit of a sinusoid at the folded frequency: exact amplitude
                // without leakage from a non-integer number of periods.
                let w = 2.0 * PI * f * s.factor as f64 / sr;
                let (mut scc, mut sss, mut scs, mut syc, mut sys) = (0.0, 0.0, 0.0, 0.0, 0.0);
                for (j, v) in ox.iter().enumerate() {
                    let (sn, cs) = (w * j as f64).sin_cos();
                    scc += cs * cs;
                    sss += sn * sn;
                    scs += sn * cs;
                    syc += v * cs;
                    sys += v * sn;
                }
                let det = scc * sss - scs * scs;
                let a = (syc * sss - sys * scs) / det;
                let b = (sys * scc - syc * scs) / det;
                a.hypot(b)
            };
            let alias = s.rate_hz - 0.5 * s.served_hi_hz;
            let a_db = 20.0 * tone(alias).log10();
            assert!(a_db < -88.0, "alias tone {alias} Hz: {a_db} dB");
            let p_db = 20.0 * tone(0.5 * s.served_hi_hz).log10();
            assert!(p_db.abs() < 0.01, "pass tone: {p_db} dB");
        }
    }
}

// ---------------------------------------------------------------------------------------
// Loopback
// ---------------------------------------------------------------------------------------

/// Known biquad cascade with an aligned delay: magnitude within 0.1 dB and phase within 1°
/// wherever coherence is high, across all three stages and both crossover blends.
#[test]
fn loopback_biquad_cascade() {
    let sr = 48_000.0;
    let delay = 100;
    let mut m = engine(sr, Averaging::Fifo { blocks: 16 }, delay);
    let n = samples_to_fill(&m);
    let filters = [
        Biquad::peaking(sr, 80.0, 1.4, 6.0),
        Biquad::peaking(sr, 1_000.0, 2.0, -6.0),
        Biquad::peaking(sr, 8_000.0, 2.0, 4.0),
    ];
    let x = Noise(1).vec(n, 0.1);
    let mut c = x.clone();
    for f in &filters {
        c = f.filter(&c);
    }
    let y = delayed(&c, 0, delay, n);
    push_chunked(&mut m, 0, &x, &y);
    let f = m.frame();
    let h = |fr: f64| {
        filters
            .iter()
            .map(|b| b.response(sr, fr))
            .product::<Complex64>()
    };
    let (mut worst_db, mut worst_deg, mut count, mut blends) = (0.0f64, 0.0f64, 0, 0);
    for i in valid(&f) {
        if f.coherence[i] < 0.99 {
            continue;
        }
        let e = expected_column(&m, &f.columns[i].source, h);
        let db = (f.magnitude_db[i] - 20.0 * e.norm().log10()).abs();
        let deg = wrap_deg(f.phase_deg[i] - e.arg().to_degrees()).abs();
        if db > 0.05 || deg > 0.5 {
            println!(
                "  {:.1} Hz {db:.4} dB {deg:.3}° γ² {:.5} eff {:.1} {:?}",
                f.freq_hz[i], f.coherence[i], f.eff_avg[i], f.columns[i].source
            );
        }
        worst_db = worst_db.max(db);
        worst_deg = worst_deg.max(deg);
        count += 1;
        if matches!(f.columns[i].source, ColumnSource::Blend { .. }) {
            blends += 1;
        }
    }
    println!("biquad: {count} columns ({blends} blended), worst {worst_db:.4} dB {worst_deg:.3}°");
    assert!(count > 380, "{count}");
    assert!(blends >= 30, "{blends}");
    assert!(worst_db < 0.1, "{worst_db} dB");
    assert!(worst_deg < 1.0, "{worst_deg}°");
    assert!(f.last_block_end.is_some_and(|e| e <= n as u64));
}

/// Pure delays of either sign, small and large (±0.2 s at 96 kHz), set as the alignment:
/// magnitude flat, phase zero, coherence ≈ 1 on every valid column. A wrong delay is shown
/// to collapse coherence so the test is sensitive.
#[test]
fn loopback_pure_delays() {
    let sr = 96_000.0;
    for delay in [0i64, 37, -37, 19_200, -19_200] {
        let mut m = engine(sr, Averaging::Fifo { blocks: 2 }, delay);
        let n = samples_to_fill(&m);
        let lead = 20_000;
        let s = Noise(7).vec(n + 2 * lead, 0.2);
        let x = s[lead..lead + n].to_vec();
        let y = delayed(&s, lead, delay, n);
        push_chunked(&mut m, 1_000_000, &x, &y);
        let f = m.frame();
        let mut worst_db = 0.0f64;
        let mut worst_deg = 0.0f64;
        let mut min_coh = 1.0f64;
        let mut count = 0;
        for i in valid(&f) {
            worst_db = worst_db.max(f.magnitude_db[i].abs());
            worst_deg = worst_deg.max(f.phase_deg[i].abs());
            min_coh = min_coh.min(f.coherence[i]);
            count += 1;
        }
        println!("delay {delay}: {count} cols, {worst_db:.2e} dB {worst_deg:.2e}° γ²min {min_coh}");
        assert!(count > 380);
        assert!(
            worst_db < 1e-6 && worst_deg < 1e-5 && min_coh > 1.0 - 1e-9,
            "delay {delay}"
        );
    }
    // Misaligned by 0.2 s: the full-rate stage (42.7 ms window) drops to the uncorrelated
    // floor 1/N_eff. Deeper windows still partly overlap and keep some coherence.
    let mut m = engine(sr, Averaging::Fifo { blocks: 2 }, 0);
    let n = samples_to_fill(&m);
    let s = Noise(7).vec(n + 40_000, 0.2);
    let x = s[20_000..20_000 + n].to_vec();
    let y = delayed(&s, 20_000, 19_200, n);
    push_chunked(&mut m, 0, &x, &y);
    let f = m.frame();
    let lo = m.layout().crossovers[0].hi_hz;
    let (meas, floor, _) = coherence_vs_model(&f, lo, 20_000.0, 0.0);
    println!("misaligned: mean γ² {meas:.3}, floor model {floor:.3}");
    assert!(
        meas < 1.3 * floor,
        "misaligned mean γ² {meas} vs floor {floor}"
    );
}

/// Mean γ̂² over a frequency range, and the mean of the model expectation
/// γ² + (1 − γ²)² / N_eff (first-order bias of the coherence estimator).
fn coherence_vs_model(f: &MtwFrame, lo: f64, hi: f64, gamma2: f64) -> (f64, f64, usize) {
    let idx: Vec<usize> = valid(f)
        .filter(|&i| f.freq_hz[i] >= lo && f.freq_hz[i] < hi)
        .collect();
    let meas = idx.iter().map(|&i| f.coherence[i]).sum::<f64>() / idx.len() as f64;
    let model = idx
        .iter()
        .map(|&i| gamma2 + (1.0 - gamma2).powi(2) / f.eff_avg[i])
        .sum::<f64>()
        / idx.len() as f64;
    (meas, model, idx.len())
}

/// Inputs whose *aligned* pair stream is `(s[p], s[p] + e[p])` for `p = 0, 1, …`, for any
/// delay: `x[i] = s[i − n0 + D]`, `y[n] = s[n − n0] + e[n − n0]` with `n0 = max(0, D)`.
/// Samples the pairs never use are filled with unrelated noise, so reading them would show.
fn aligned_stream(
    delay: i64,
    pairs: usize,
    seed: u64,
    signal: f64,
    noise: f64,
) -> (Vec<f64>, Vec<f64>) {
    let len = pairs + delay.unsigned_abs() as usize;
    let s = Noise(seed).vec(pairs, signal);
    let mut e = Noise(seed + 1);
    let mut junk = Noise(seed + 2);
    let n0 = delay.max(0);
    let x = (0..len as i64)
        .map(|i| {
            let p = i - n0 + delay;
            if (0..pairs as i64).contains(&p) {
                s[p as usize]
            } else {
                junk.gauss()
            }
        })
        .collect();
    let y = (0..len as i64)
        .map(|n| {
            let p = n - n0;
            if (0..pairs as i64).contains(&p) {
                s[p as usize] + noise * e.gauss()
            } else {
                junk.gauss()
            }
        })
        .collect();
    (x, y)
}

/// `y = x + n` with uncorrelated white noise at 0 dB SNR: γ² = SNR/(1 + SNR) = 0.5 plus the
/// estimator bias (1 − γ²)²/N_eff from the reported model. Regression "large delay doesn't
/// bias coherence": with ±0.2 s at 96 kHz set as the alignment, the engine sees the same pair
/// stream as with no delay and must produce the same H1 and γ² bit for bit.
#[test]
fn partial_coherence_matches_theory_and_large_delay_does_not_bias_it() {
    let sr = 96_000.0;
    let gamma2 = 0.5;
    let reference = {
        let mut m = engine(sr, Averaging::Fifo { blocks: 16 }, 0);
        let pairs = samples_to_fill(&m);
        let (x, y) = aligned_stream(0, pairs, 11, 0.2, 0.2);
        push_chunked(&mut m, 0, &x, &y);
        let f = m.frame();
        let l = m.layout();
        for (lo, hi) in [
            (25.0, l.crossovers[1].lo_hz),
            (l.crossovers[1].hi_hz, l.crossovers[0].lo_hz),
            (l.crossovers[0].hi_hz, 20_000.0),
        ] {
            let (meas, model, cols) = coherence_vs_model(&f, lo, hi, gamma2);
            println!("{lo:.0}-{hi:.0} Hz: γ² {meas:.4} model {model:.4} ({cols} cols)");
            assert!((meas - model).abs() < 0.03, "{lo}-{hi}: {meas} vs {model}");
        }
        (f, pairs)
    };
    let (f0, pairs) = reference;
    for delay in [19_200i64, -19_200, 1] {
        let mut m = engine(sr, Averaging::Fifo { blocks: 16 }, delay);
        let (x, y) = aligned_stream(delay, pairs, 11, 0.2, 0.2);
        push_chunked(&mut m, 5_000_000, &x, &y);
        let f = m.frame();
        let same = f.columns == f0.columns
            && f.h1
                .iter()
                .zip(&f0.h1)
                .all(|(a, b)| a.re.to_bits() == b.re.to_bits() && a.im.to_bits() == b.im.to_bits())
            && f.coherence
                .iter()
                .zip(&f0.coherence)
                .all(|(a, b)| a.to_bits() == b.to_bits());
        assert!(same, "delay {delay} changed the estimate");
        assert_eq!(
            f.last_block_end
                .map(|e| e as i64 - 5_000_000 - delay.max(0)),
            f0.last_block_end.map(|e| e as i64)
        );
    }
}

/// Uncorrelated inputs: mean γ̂² equals 1/N_eff of the reported model, for FIFO and
/// exponential averaging, pooled over four noise seeds. Checked per bin of each stage
/// (block overlap and weighting) and per display column (adding adjacent-bin correlation).
#[test]
fn effective_averages_model_predicts_coherence_floor() {
    let sr = 48_000.0;
    let seeds = [21u64, 121, 221, 321];
    for averaging in [
        Averaging::Fifo { blocks: 8 },
        Averaging::Exponential {
            time_constant_s: 0.15,
        },
    ] {
        let (bin_tol, col_tol) = match averaging {
            Averaging::Fifo { .. } => (0.05, 0.08),
            // Unequal block weights pull the coherence floor below the variance-equivalent
            // 1/N_eff (measured 4–13 % low); the model stays a variance statement.
            Averaging::Exponential { .. } => (0.15, 0.15),
        };
        // [stage] → (Σ measured, Σ model, count) per bin; [band] → per column.
        let mut per_bin = [(0.0, 0.0, 0usize); 3];
        let mut per_col = [(0.0, 0.0); 3];
        for seed in seeds {
            let mut m = engine(sr, averaging, 0);
            let n = match averaging {
                Averaging::Fifo { .. } => samples_to_fill(&m),
                Averaging::Exponential { .. } => 48_000 * 6,
            };
            let x = Noise(seed).vec(n, 0.3);
            let y = Noise(seed + 1).vec(n, 0.3);
            push_chunked(&mut m, 0, &x, &y);
            for (s, acc) in per_bin.iter_mut().enumerate() {
                let sp = m.stage_spectra(s).expect("spectra");
                let top =
                    (m.layout().stages[s].served_hi_hz / m.layout().stages[s].bin_hz) as usize;
                for k in 1..=top {
                    acc.0 += sp.gxy[k].norm_sqr() / (sp.gxx[k] * sp.gyy[k]);
                    acc.1 += 1.0 / m.eff_avg(s, 1);
                    acc.2 += 1;
                }
            }
            let f = m.frame();
            let l = m.layout();
            let bands = [
                (25.0, l.crossovers[1].lo_hz),
                (l.crossovers[1].hi_hz, l.crossovers[0].lo_hz),
                (l.crossovers[0].hi_hz, 20_000.0),
            ];
            for (acc, (lo, hi)) in per_col.iter_mut().zip(bands) {
                let (meas, model, _) = coherence_vs_model(&f, lo, hi, 0.0);
                acc.0 += meas / seeds.len() as f64;
                acc.1 += model / seeds.len() as f64;
            }
        }
        for (s, (meas, model, k)) in per_bin.iter().enumerate() {
            let (meas, model) = (meas / *k as f64, model / *k as f64);
            println!(
                "{averaging:?} stage {s} per bin: floor {meas:.4} model {model:.4} ({k} bins)"
            );
            assert!(
                (meas / model - 1.0).abs() < bin_tol,
                "stage {s}: {meas} vs {model}"
            );
        }
        for (b, (meas, model)) in per_col.iter().enumerate() {
            println!("{averaging:?} band {b} per column: floor {meas:.4} model {model:.4}");
            assert!(
                (meas / model - 1.0).abs() < col_tol,
                "band {b}: {meas} vs {model}"
            );
        }
    }
}

/// White noise with γ² = 0.5: the mean coherence half an octave either side of each
/// crossover, pooled over four noise seeds, differs by no more than 0.04, and the blend
/// sits between its neighbours within the same tolerance. The residual step expected from
/// the bin count dropping across the crossover is ≈ 0.25·(1/N_above − 1/N_below) ≈ 0.01.
#[test]
fn no_coherence_step_at_crossovers() {
    let sr = 48_000.0;
    let half = 2f64.sqrt();
    let seeds = [31u64, 131, 231, 331];
    // Per crossover: summed (below, blend, above) means and their models.
    let mut acc = [[0.0f64; 5]; 2];
    for seed in seeds {
        let mut m = engine(sr, Averaging::Fifo { blocks: 16 }, 0);
        let n = samples_to_fill(&m);
        let x = Noise(seed).vec(n, 0.2);
        let mut w = Noise(seed + 1);
        let y: Vec<f64> = x.iter().map(|v| v + 0.2 * w.gauss()).collect();
        push_chunked(&mut m, 0, &x, &y);
        let f = m.frame();
        for (a, c) in acc.iter_mut().zip(&m.layout().crossovers) {
            let (below, mb, _) = coherence_vs_model(&f, c.lo_hz / half, c.lo_hz, 0.5);
            let (inside, _, _) = coherence_vs_model(&f, c.lo_hz, c.hi_hz, 0.5);
            let (above, ma, _) = coherence_vs_model(&f, c.hi_hz, c.hi_hz * half, 0.5);
            for (s, v) in a.iter_mut().zip([below, inside, above, mb, ma]) {
                *s += v / seeds.len() as f64;
            }
        }
    }
    for (c, a) in [812, 203].iter().zip(acc) {
        let [below, inside, above, mb, ma] = a;
        println!(
            "crossover {c} Hz: below {below:.4} (model {mb:.4}) blend {inside:.4} \
             above {above:.4} (model {ma:.4}) step {:+.4} (model {:+.4})",
            above - below,
            ma - mb
        );
        assert!((above - below).abs() < 0.04, "step {}", above - below);
        assert!((inside - 0.5 * (above + below)).abs() < 0.04);
    }
}

// ---------------------------------------------------------------------------------------
// Stream behaviour
// ---------------------------------------------------------------------------------------

fn same_frame(a: &MtwFrame, b: &MtwFrame) -> bool {
    a.columns == b.columns
        && a.last_block_end == b.last_block_end
        && a.h1
            .iter()
            .zip(&b.h1)
            .all(|(p, q)| (p.re.to_bits(), p.im.to_bits()) == (q.re.to_bits(), q.im.to_bits()))
        && a.coherence
            .iter()
            .zip(&b.coherence)
            .all(|(p, q)| p.to_bits() == q.to_bits())
}

/// The block grid belongs to the stream: chunking and planar vs interleaved input give the
/// same frame bit for bit.
#[test]
fn chunking_and_layout_do_not_matter() {
    let sr = 48_000.0;
    let n = 120_000;
    let x = Noise(41).vec(n, 0.2);
    let y = Biquad::peaking(sr, 300.0, 1.0, 3.0).filter(&x);
    let mut a = engine(sr, Averaging::Fifo { blocks: 4 }, -5);
    a.push(0, &x, &y, SampleGate::Accept).expect("push");
    let mut b = engine(sr, Averaging::Fifo { blocks: 4 }, -5);
    push_chunked(&mut b, 0, &x, &y);
    let mut c = engine(sr, Averaging::Fifo { blocks: 4 }, -5);
    let inter: Vec<f32> = x
        .iter()
        .zip(&y)
        .flat_map(|(&r, &m)| [0.0f32, m as f32, r as f32])
        .collect();
    let xf: Vec<f32> = x.iter().map(|&v| v as f32).collect();
    let yf: Vec<f32> = y.iter().map(|&v| v as f32).collect();
    for (k, chunk) in inter.chunks(3 * 1000).enumerate() {
        c.push_interleaved((k * 1000) as u64, chunk, 3, 2, 1, SampleGate::Accept)
            .expect("push");
    }
    let mut d = engine(sr, Averaging::Fifo { blocks: 4 }, -5);
    d.push(0, &xf, &yf, SampleGate::Accept).expect("push");
    assert!(same_frame(&a.frame(), &b.frame()));
    assert!(same_frame(&c.frame(), &d.frame()));
}

/// Samples marked `Reject` never reach the averages: a glitch inside a rejected span gives
/// exactly the frame of a clean signal with the same span rejected, for either delay sign.
#[test]
fn rejected_samples_never_reach_the_averages() {
    let sr = 48_000.0;
    for delay in [300i64, -300] {
        let n = 260_000;
        let x = Noise(51).vec(n, 0.2);
        let y = Biquad::peaking(sr, 500.0, 1.0, 6.0).filter(&x);
        let mut gx = x.clone();
        let mut gy = y.clone();
        let (g0, g1) = (100_000usize, 101_000usize);
        for i in g0..g1 {
            gx[i] = 0.99;
            gy[i] = -0.99 * (i % 7) as f64;
        }
        let run = |x: &[f64], y: &[f64]| {
            let mut m = engine(sr, Averaging::Fifo { blocks: 64 }, delay);
            let mut rejected = 0;
            for (lo, hi) in [(0, g0), (g0, g1), (g1, n)] {
                let gate = if lo == g0 {
                    SampleGate::Reject
                } else {
                    SampleGate::Accept
                };
                let mut i = lo;
                while i < hi {
                    let e = (i + 777).min(hi);
                    rejected += m
                        .push(i as u64, &x[i..e], &y[i..e], gate)
                        .expect("push")
                        .blocks_rejected;
                    i = e;
                }
            }
            (m.frame(), rejected)
        };
        let (clean, rc) = run(&x, &y);
        let (glitch, rg) = run(&gx, &gy);
        assert!(rc > 0 && rc == rg);
        assert!(same_frame(&clean, &glitch), "delay {delay}");
        // And the rejection is not vacuous: without it the glitch shows.
        let mut m = engine(sr, Averaging::Fifo { blocks: 64 }, delay);
        m.push(0, &gx, &gy, SampleGate::Accept).expect("push");
        assert!(!same_frame(&clean, &m.frame()));
    }
}

#[test]
fn freeze_reset_and_restart() {
    let sr = 48_000.0;
    let x = Noise(61).vec(200_000, 0.2);
    let y = x.clone();
    let mut m = engine(sr, Averaging::Fifo { blocks: 4 }, 0);
    m.push(0, &x[..100_000], &y[..100_000], SampleGate::Accept)
        .expect("push");
    let before = m.frame();
    m.set_frozen(true);
    let out = m
        .push(100_000, &x[100_000..], &y[100_000..], SampleGate::Accept)
        .expect("push");
    assert!(out.blocks_frozen > 0 && out.blocks_accumulated == 0);
    assert!(same_frame(&before, &m.frame()));
    m.set_frozen(false);
    m.reset_averages();
    assert!(m.frame().columns.iter().all(|c| matches!(
        c.validity,
        Validity::Settling | Validity::Thinned | Validity::OutOfBand
    )));
    // A gap in the sample index restarts the ladder.
    let out = m
        .push(300_000, &x[..10], &y[..10], SampleGate::Accept)
        .expect("push");
    assert!(out.restarted);
    assert!(m.push(0, &x[..3], &y[..2], SampleGate::Accept).is_err());
}

#[test]
fn columns_thin_and_stop_at_band_edge() {
    let sr = 44_100.0;
    let mut m = engine(sr, Averaging::Fifo { blocks: 2 }, 0);
    let n = samples_to_fill(&m);
    let x = Noise(71).vec(n, 0.2);
    push_chunked(&mut m, 0, &x, &x);
    let f = m.frame();
    let edge = m.layout().stages.last().expect("stage").validity_edge_hz();
    for (i, c) in f.columns.iter().enumerate() {
        let fc = f.freq_hz[i];
        if fc > 0.45 * sr {
            assert_eq!(c.validity, Validity::OutOfBand, "{fc}");
        } else if fc > edge * 1.01 {
            assert_eq!(c.validity, Validity::Valid, "{fc}");
        }
        if c.validity == Validity::Thinned {
            assert!(fc < edge && f.h1[i].re.is_nan());
        }
    }
    let thinned = f
        .columns
        .iter()
        .filter(|c| c.validity == Validity::Thinned)
        .count();
    assert!(thinned > 20, "{thinned}");
}
