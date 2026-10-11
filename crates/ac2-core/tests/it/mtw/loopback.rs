//! MTW loopback with synthetic signals whose answers are analytic: transfer and coherence.

use super::*;

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
    let (meas, floor, _) = coherence_vs_model(&m, &f, lo, 20_000.0, 0.0);
    println!("misaligned: mean γ² {meas:.3}, floor model {floor:.3}");
    assert!(
        meas < 1.3 * floor,
        "misaligned mean γ² {meas} vs floor {floor}"
    );
}

fn engine_ppo(sr: f64, averaging: Averaging, ppo: u32) -> Mtw {
    Mtw::new(MtwConfig {
        sample_rate_hz: sr,
        ladder: Ladder::Standard,
        averaging,
        depth: DepthPolicy::EqualConfidence,
        grid: LogGrid::covering(ppo, 20.0, 20_000.0),
        delay_samples: 0.0,
    })
    .expect("engine")
}

/// |ΣGxy|² / (ΣGxx·ΣGyy) over a single-stage column's bins: the column-sum estimator, which
/// cancels wherever the transfer phase turns across the column.
fn column_sum_coherence(m: &Mtw, b: &StageBins) -> f64 {
    let sp = m.stage_spectra(b.stage).expect("spectra");
    let xx: f64 = sp.gxx[b.lo..b.hi].iter().sum();
    let yy: f64 = sp.gyy[b.lo..b.hi].iter().sum();
    let xy: Complex64 = sp.gxy[b.lo..b.hi].iter().sum();
    xy.norm_sqr() / (xx * yy)
}

/// `y = x` delayed by 1 ms, left in the path (alignment 0): the phase turns by 360°·f·1 ms,
/// so across a 1/3-octave column above a few kHz it turns more than a full cycle. Every bin
/// is still coherent (the window misalignment is 48 of 4096 samples), so γ² stays ≈ 1 on
/// every valid column at 3 and at 48 points per octave. The column-sum estimator is shown
/// to collapse on the same averages, so the test is sensitive to the definition.
#[test]
fn coherence_does_not_drop_with_phase_turning_across_a_column() {
    let sr = 48_000.0;
    let delay = 48;
    for ppo in [3u32, 12, 48] {
        let mut m = engine_ppo(sr, Averaging::Fifo { blocks: 4 }, ppo);
        let n = samples_to_fill(&m);
        let lead = 1_000;
        let s = Noise(31).vec(n + 2 * lead, 0.2);
        let x = s[lead..lead + n].to_vec();
        let y = delayed(&s, lead, delay, n);
        push_chunked(&mut m, 0, &x, &y);
        let f = m.frame();
        let (mut min_coh, mut min_sum, mut count) = (1.0f64, 1.0f64, 0);
        for i in valid(&f) {
            min_coh = min_coh.min(f.coherence[i]);
            if let ColumnSource::Stage(b) = f.columns[i].source {
                min_sum = min_sum.min(column_sum_coherence(&m, &b));
            }
            count += 1;
        }
        println!("{ppo} ppo: {count} cols, γ² min {min_coh:.4}, column-sum min {min_sum:.4}");
        assert!(count > 9 * ppo as usize, "{count}");
        assert!(min_coh > 0.97, "{ppo} ppo: γ² {min_coh}");
        if ppo == 3 {
            assert!(
                min_sum < 0.2,
                "column-sum estimator {min_sum}: test not sensitive"
            );
        }
    }
}

/// `y = x + n` with uncorrelated noise at 0 dB SNR (γ² = 0.5) and a 1 ms delay left in the
/// path: the mean γ² over each stage's band, pooled over four noise seeds, matches
/// γ² + (1 − γ²)²/N_eff at 3 and at 48 points per octave alike, so coherence does not depend
/// on resolution. A 3-ppo band holds only 5–9 columns, hence the pooling.
#[test]
fn partial_coherence_is_independent_of_resolution() {
    let sr = 48_000.0;
    let gamma2 = 0.5;
    let delay = 48;
    let seeds = [41u64, 141, 241, 341];
    let mut by_ppo = Vec::new();
    for ppo in [3u32, 48] {
        // [band] → (Σ measured, Σ model) over seeds.
        let mut bands = [(0.0, 0.0); 3];
        for seed in seeds {
            let mut m = engine_ppo(sr, Averaging::Fifo { blocks: 16 }, ppo);
            let n = samples_to_fill(&m);
            let lead = 1_000;
            let s = Noise(seed).vec(n + 2 * lead, 0.2);
            let x = s[lead..lead + n].to_vec();
            let e = Noise(seed + 1).vec(n, 0.2);
            let y: Vec<f64> = delayed(&s, lead, delay, n)
                .iter()
                .zip(&e)
                .map(|(a, b)| a + b)
                .collect();
            push_chunked(&mut m, 0, &x, &y);
            let f = m.frame();
            let l = m.layout();
            let ranges = [
                (25.0, l.crossovers[1].lo_hz),
                (l.crossovers[1].hi_hz, l.crossovers[0].lo_hz),
                (l.crossovers[0].hi_hz, 20_000.0),
            ];
            for (acc, (lo, hi)) in bands.iter_mut().zip(ranges) {
                let (meas, model, cols) = coherence_vs_model(&m, &f, lo, hi, gamma2);
                assert!(cols > 0);
                acc.0 += meas / seeds.len() as f64;
                acc.1 += model / seeds.len() as f64;
            }
        }
        for (b, (meas, model)) in bands.iter().enumerate() {
            println!("{ppo} ppo band {b}: γ² {meas:.4} model {model:.4}");
            assert!(
                (meas - model).abs() < 0.03,
                "{ppo} ppo band {b}: {meas} vs {model}"
            );
        }
        by_ppo.push(bands);
    }
    for ((coarse, _), (fine, _)) in by_ppo[0].iter().zip(&by_ppo[1]) {
        assert!(
            (coarse - fine).abs() < 0.03,
            "3 ppo {coarse} vs 48 ppo {fine}"
        );
    }
}

/// Mean γ̂² over a frequency range, and the mean of the model expectation of each column.
///
/// One bin of N_eff averages reads γ² + (1 − γ²)²/N_eff (first-order bias of the
/// coherence estimator). The column's Gxx-weighted mean over K independent bins tends to
/// γ² + (1 − γ²)/N_eff as K grows: a bin whose reference read strong also reads more
/// coherent, and the weight favours it. A Monte Carlo of the estimator puts K bins
/// 1 − 1/K of the way between the two. K is the model's N_eff over the column's bins
/// divided by its N_eff per bin, as adjacent Hann bins are partly correlated.
fn coherence_vs_model(m: &Mtw, f: &MtwFrame, lo: f64, hi: f64, gamma2: f64) -> (f64, f64, usize) {
    let stage_model = |b: &StageBins| {
        let n = m.eff_avg(b.stage, 1);
        let k = m.eff_avg(b.stage, b.count()) / n;
        let one = gamma2 + (1.0 - gamma2).powi(2) / n;
        let many = gamma2 + (1.0 - gamma2) / n;
        one + (1.0 - 1.0 / k) * (many - one)
    };
    let idx: Vec<usize> = valid(f)
        .filter(|&i| f.freq_hz[i] >= lo && f.freq_hz[i] < hi)
        .collect();
    let meas = idx.iter().map(|&i| f.coherence[i]).sum::<f64>() / idx.len() as f64;
    let model = idx
        .iter()
        .map(|&i| match &f.columns[i].source {
            ColumnSource::Stage(b) => stage_model(b),
            ColumnSource::Blend {
                deep,
                shallow,
                shallow_weight: w,
            } => (1.0 - w) * stage_model(deep) + w * stage_model(shallow),
            ColumnSource::None => f64::NAN,
        })
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
            let (meas, model, cols) = coherence_vs_model(&m, &f, lo, hi, gamma2);
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
/// (block overlap and weighting) and per display column (a mean of per-bin γ̂², which keeps
/// the per-bin floor however many bins the column holds).
#[test]
fn effective_averages_model_predicts_coherence_floor() {
    check_coherence_floor(DepthPolicy::EqualConfidence);
}

/// The same check with the fast-LF cap: the capped stages' higher floor is what the
/// reported effective averages predict. Uncapped stages keep the tolerances above. The
/// capped deep stages hold only 2–2.5 effective averages of heavily overlapped blocks,
/// where the measured floor falls further below the variance-equivalent 1/N_eff (measured
/// 6–9 % low for FIFO, about 20 % for exponential); the model never reads low, so a capped
/// stage's reported floor is conservative and the bound is widened on the low side only.
#[test]
fn fast_lf_coherence_floor_matches_model() {
    check_coherence_floor(DepthPolicy::fast_lf());
}

fn check_coherence_floor(depth: DepthPolicy) {
    let sr = 48_000.0;
    let seeds = [21u64, 121, 221, 321];
    for averaging in [
        Averaging::Fifo { blocks: 8 },
        Averaging::Exponential {
            time_constant_s: 0.15,
        },
    ] {
        let (bin_tol, col_tol, capped_low_tol) = match averaging {
            Averaging::Fifo { .. } => (0.05, 0.08, 0.12),
            // Unequal block weights pull the coherence floor below the variance-equivalent
            // 1/N_eff (measured 4–13 % low); the model stays a variance statement.
            Averaging::Exponential { .. } => (0.15, 0.15, 0.25),
        };
        let capped: Vec<bool> = engine_with(sr, averaging, depth, 0)
            .stage_depths()
            .iter()
            .map(|d| d.capped)
            .collect();
        // Relative error must lie in (−low, high).
        let within = |meas: f64, model: f64, tol: f64, stage: usize| {
            let low = if capped[stage] { capped_low_tol } else { tol };
            let r = meas / model - 1.0;
            r < tol && r > -low
        };
        // [stage] → (Σ measured, Σ model, count) per bin; [band] → per column.
        let mut per_bin = [(0.0, 0.0, 0usize); 3];
        let mut per_col = [(0.0, 0.0); 3];
        for seed in seeds {
            let mut m = engine_with(sr, averaging, depth, 0);
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
                let (meas, model, _) = coherence_vs_model(&m, &f, lo, hi, 0.0);
                acc.0 += meas / seeds.len() as f64;
                acc.1 += model / seeds.len() as f64;
            }
        }
        for (s, (meas, model, k)) in per_bin.iter().enumerate() {
            let (meas, model) = (meas / *k as f64, model / *k as f64);
            println!(
                "{depth:?} {averaging:?} stage {s} per bin: floor {meas:.4} model {model:.4} \
                 ({k} bins)"
            );
            assert!(
                within(meas, model, bin_tol, s),
                "stage {s}: {meas} vs {model}"
            );
        }
        for (b, (meas, model)) in per_col.iter().enumerate() {
            println!(
                "{depth:?} {averaging:?} band {b} per column: floor {meas:.4} model {model:.4}"
            );
            // Band b is served by stage 2 − b.
            assert!(
                within(*meas, *model, col_tol, 2 - b),
                "band {b}: {meas} vs {model}"
            );
        }
    }
}

/// White noise with γ² = 0.5: the mean coherence half an octave either side of each
/// crossover, pooled over four noise seeds, differs by no more than 0.04, and the blend
/// sits between its neighbours within the same tolerance. Equal confidence gives every
/// stage the same per-bin N_eff, so the per-bin bias does not step; what remains is the Gxx
/// weight's bias, which grows with the bins per column, and the deeper stage's columns hold
/// more bins than the shallower stage's just above (model ≈ −0.005, measured ≈ −0.017).
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
            let (below, mb, _) = coherence_vs_model(&m, &f, c.lo_hz / half, c.lo_hz, 0.5);
            let (inside, _, _) = coherence_vs_model(&m, &f, c.lo_hz, c.hi_hz, 0.5);
            let (above, ma, _) = coherence_vs_model(&m, &f, c.hi_hz, c.hi_hz * half, 0.5);
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
