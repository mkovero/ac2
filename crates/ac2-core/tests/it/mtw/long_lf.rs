//! MTW long-LF ladder: the fourth stage against an analytic system below the standard
//! ladder's reach.

use super::*;

/// A sub-like response (a resonance at 22 Hz, a cut at 45 Hz) on a grid down to 7.8 Hz:
/// magnitude within 0.1 dB and phase within 1° wherever coherence is high, including the
/// columns only the fourth stage resolves and the blend into the third.
#[test]
fn long_lf_resolves_the_bottom_octaves() {
    let sr = 48_000.0;
    let delay = 100;
    let grid = LogGrid {
        ppo: 48,
        k_min: -7 * 48,
        k_max: 5 * 48 - 1,
    };
    let mut m = Mtw::new(MtwConfig {
        sample_rate_hz: sr,
        ladder: Ladder::LongLf,
        averaging: Averaging::Fifo { blocks: 16 },
        depth: DepthPolicy::EqualConfidence,
        grid,
        delay_samples: delay as f64,
    })
    .expect("engine");
    let n = samples_to_fill(&m);
    let filters = [
        Biquad::peaking(sr, 22.0, 3.0, 6.0),
        Biquad::peaking(sr, 45.0, 4.0, -6.0),
    ];
    let x = Noise(3).vec(n, 0.1);
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
    let edge = m.layout().stages[3].validity_edge_hz();
    let cross = m.layout().crossovers[2];
    let (mut worst_db, mut worst_deg, mut deep, mut blends) = (0.0f64, 0.0f64, 0, 0);
    for i in valid(&f) {
        if f.coherence[i] < 0.99 || f.freq_hz[i] < edge || f.freq_hz[i] > 2.0 * cross.hi_hz {
            continue;
        }
        let e = expected_column(&m, &f.columns[i].source, h);
        worst_db = worst_db.max((f.magnitude_db[i] - 20.0 * e.norm().log10()).abs());
        worst_deg = worst_deg.max(wrap_deg(f.phase_deg[i] - e.arg().to_degrees()).abs());
        if f.freq_hz[i] < cross.lo_hz {
            deep += 1;
        } else if matches!(f.columns[i].source, ColumnSource::Blend { .. }) {
            blends += 1;
        }
    }
    println!(
        "long LF: {deep} fourth-stage columns, {blends} blended, worst {worst_db:.4} dB \
         {worst_deg:.3}°; {:.1} s of signal",
        n as f64 / sr
    );
    // Two octaves from the validity edge (16.9 Hz) to the crossover (67.6 Hz).
    assert!(deep >= 90, "{deep}");
    assert!(blends >= 12, "{blends}");
    assert!(worst_db < 0.1, "{worst_db} dB");
    assert!(worst_deg < 1.0, "{worst_deg}°");
}
