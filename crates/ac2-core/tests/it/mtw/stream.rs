//! MTW stream behaviour: chunking, rejection, reset and column thinning.

use super::*;

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
fn reset_and_restart() {
    let sr = 48_000.0;
    let x = Noise(61).vec(200_000, 0.2);
    let y = x.clone();
    let mut m = engine(sr, Averaging::Fifo { blocks: 4 }, 0);
    m.push(0, &x[..100_000], &y[..100_000], SampleGate::Accept)
        .expect("push");
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
