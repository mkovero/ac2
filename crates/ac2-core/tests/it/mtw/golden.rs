//! MTW golden comparisons against scipy-built references.

use super::*;

/// Single-stage configuration with the golden set's Welch parameters reproduces scipy's
/// Pxx, Pyy, Pxy, H1 and coherence; frame columns hold H1 as the ratio of summed golden
/// spectra and γ² as the Pxx-weighted mean of the golden per-bin coherence.
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
        depth: DepthPolicy::EqualConfidence,
        grid: grid48(),
        delay_samples: 0.0,
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
        let c = (b.lo..b.hi)
            .map(|k| pxy[k].norm_sqr() / pyy[k])
            .sum::<f64>()
            / sxx;
        assert!(syy > 0.0);
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
