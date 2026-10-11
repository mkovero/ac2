//! MTW layout: the stage table and the decimators' specification.

use super::*;

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
