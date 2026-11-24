//! Each stage's own bins: the drawn ranges the ladder fixes, and the per-bin H1 and γ² the
//! averages hold.

use super::*;
use ac2_core::mtw::DrawnBins;

fn layout(sr: f64, ladder: Ladder) -> Vec<DrawnBins> {
    Layout::new(sr, ladder).expect("layout").drawn_bins()
}

/// `(first, count, hop)` per stage.
fn summary(d: &[DrawnBins]) -> Vec<(usize, usize, u64)> {
    d.iter().map(|b| (b.first, b.count, b.hop)).collect()
}

/// The ladder table of docs/design/bin-streams.md, from the layout's formulas: stage 0 from
/// the top of the blend below it to the served band, the deepest from bin 1, every other
/// between the blends on either side.
#[test]
fn drawn_ranges_follow_the_ladder() {
    assert_eq!(
        summary(&layout(48_000.0, Ladder::Standard)),
        [(88, 1756, 2048), (88, 189, 4096), (1, 207, 6144)]
    );
    assert_eq!(
        summary(&layout(48_000.0, Ladder::LongLf)),
        [
            (88, 1756, 2048),
            (88, 189, 4096),
            (88, 120, 6144),
            (1, 276, 12_288)
        ]
    );
    let hi = layout(192_000.0, Ladder::Standard);
    assert_eq!(
        summary(&hi),
        [
            (88, 1756, 2048),
            (88, 1020, 4 * 16 * 256),
            (1, 207, 48 * 512)
        ]
    );
    assert_eq!(hi[1].factor, 16);
    assert!((hi[1].bin_hz - 12_000.0 / 4096.0).abs() < 1e-12);
    assert!((hi[1].rate_hz - 12_000.0).abs() < 1e-9);
    let one = layout(
        48_000.0,
        Ladder::Single {
            nfft: 1024,
            hop: 512,
        },
    );
    assert_eq!(summary(&one), [(1, 460, 512)]);
}

/// No drawn bin lies in a blend, and the bins just outside a stage's range do (or lie beyond
/// the served band): the stages' drawn ranges and the blends tile the band.
#[test]
fn drawn_ranges_stop_at_the_blends() {
    for (sr, ladder) in [
        (48_000.0, Ladder::Standard),
        (48_000.0, Ladder::LongLf),
        (96_000.0, Ladder::Standard),
        (192_000.0, Ladder::LongLf),
    ] {
        let l = Layout::new(sr, ladder).expect("layout");
        let in_blend = |f: f64| l.crossovers.iter().any(|c| f > c.lo_hz && f < c.hi_hz);
        for d in l.drawn_bins() {
            let f = |k: usize| k as f64 * d.bin_hz;
            for k in d.range() {
                assert!(!in_blend(f(k)), "{sr} stage {} bin {k}", d.stage);
            }
            if d.stage > 0 {
                assert!(in_blend(f(d.range().end)), "{sr} stage {} top", d.stage);
            } else {
                assert!(f(d.range().end) > l.stages[0].served_hi_hz);
            }
            if d.first > 1 {
                assert!(in_blend(f(d.first - 1)), "{sr} stage {} bottom", d.stage);
            }
        }
    }
}

fn fine_engine(ladder: Ladder) -> Mtw {
    Mtw::new(MtwConfig {
        sample_rate_hz: 48_000.0,
        ladder,
        averaging: Averaging::Fifo { blocks: 4 },
        depth: DepthPolicy::EqualConfidence,
        grid: LogGrid::covering(96, 10.0, 20_000.0),
        delay_samples: 0.0,
    })
    .expect("engine")
}

/// A stage's bins read as a 1/96 column holding that one bin alone does: the column sums
/// one bin, so its H1 and γ² are the bin's.
#[test]
fn a_bin_reads_as_the_column_holding_it_alone() {
    let mut m = fine_engine(Ladder::LongLf);
    let (mut h1, mut coh) = (Vec::new(), Vec::new());
    for d in m.layout().drawn_bins() {
        assert_eq!(m.stage_bins_into(d.stage, d.range(), &mut h1, &mut coh), 0);
        assert_eq!(h1.len(), d.count);
        assert!(h1.iter().all(|h| h.re.is_nan()) && coh.iter().all(|c| c.is_nan()));
    }
    let n = 6 * 48_000;
    let x = Noise(11).vec(n, 0.1);
    let eq = Biquad::peaking(48_000.0, 300.0, 2.0, 6.0);
    let mut y = eq.filter(&x);
    let noise = Noise(12).vec(n, 0.01);
    for (v, e) in y.iter_mut().zip(&noise) {
        *v += e;
    }
    push_chunked(&mut m, 0, &x, &y);
    let f = m.frame();
    let drawn = m.layout().drawn_bins();
    let mut seen = vec![0usize; drawn.len()];
    for (i, c) in f.columns.iter().enumerate() {
        let ColumnSource::Stage(b) = c.source else {
            continue;
        };
        let d = drawn[b.stage];
        if b.count() != 1 || !d.range().contains(&b.lo) || c.validity != Validity::Valid {
            continue;
        }
        let held = m.stage_bins_into(b.stage, b.lo..b.hi, &mut h1, &mut coh);
        assert_eq!(held, m.stage_held(b.stage));
        assert!(held > 0);
        assert!(
            (h1[0] - f.h1[i]).norm() <= 1e-12 * f.h1[i].norm(),
            "column {i}"
        );
        assert!((coh[0] - f.coherence[i]).abs() < 1e-12, "column {i}");
        seen[b.stage] += 1;
    }
    assert!(seen.iter().all(|&s| s > 0), "{seen:?}");
    // Ends of the newest blocks: the deeper the stage, the longer its window and decimator
    // reach back, so its newest block ends no later than the full-rate stage's.
    let ends: Vec<u64> = (0..drawn.len())
        .map(|s| m.stage_last_block_end(s).expect("a block"))
        .collect();
    assert!(ends.iter().all(|&e| e <= n as u64), "{ends:?}");
    assert_eq!(m.stage_last_block_end(drawn.len()), None);
}

/// Where the reference has no energy H1 has no value, and γ² neither.
#[test]
fn a_silent_reference_leaves_nan() {
    let mut m = fine_engine(Ladder::Standard);
    let n = 2 * 48_000;
    let x = vec![0.0; n];
    let y = Noise(3).vec(n, 0.1);
    push_chunked(&mut m, 0, &x, &y);
    let d = m.layout().drawn_bins()[0];
    let (mut h1, mut coh) = (Vec::new(), Vec::new());
    assert!(m.stage_bins_into(0, d.range(), &mut h1, &mut coh) > 0);
    assert!(h1.iter().all(|h| h.re.is_nan() && h.im.is_nan()));
    assert!(coh.iter().all(|c| c.is_nan()));
}

/// Where the measurement has no energy the column holding a bin reads no measurement, so the
/// bin has no H1 and no γ² either.
#[test]
fn a_silent_measurement_leaves_nan() {
    let mut m = fine_engine(Ladder::Standard);
    let n = 2 * 48_000;
    let x = Noise(3).vec(n, 0.1);
    let y = vec![0.0; n];
    push_chunked(&mut m, 0, &x, &y);
    let f = m.frame();
    assert!(f.columns.iter().all(|c| c.validity != Validity::Valid));
    let d = m.layout().drawn_bins()[0];
    let (mut h1, mut coh) = (Vec::new(), Vec::new());
    assert!(m.stage_bins_into(0, d.range(), &mut h1, &mut coh) > 0);
    assert!(h1.iter().all(|h| h.re.is_nan() && h.im.is_nan()));
    assert!(coh.iter().all(|c| c.is_nan()));
}

/// A column's phase turns with the delay at the frequency `column_hz` gives, as each bin's
/// phase is referred at its own frequency: averages kept through a change of delay by `Δ`
/// (each bin turned at its own frequency) read, turned back by `Δ` at that frequency, as
/// before, exactly where the column holds one bin. At the column centre they would miss by
/// up to half a column's worth of turn.
#[test]
fn a_column_turns_with_the_delay_at_its_bins() {
    let (d, delta) = (48usize, 40usize);
    let n = 6 * 48_000;
    // The measurement is the reference `d` samples later.
    let s = Noise(21).vec(n + d, 0.1);
    let (reference, late_by_d) = (s[d..].to_vec(), s[..n].to_vec());
    let mut m = Mtw::new(MtwConfig {
        delay_samples: d as f64,
        ..fine_engine(Ladder::LongLf).config().clone()
    })
    .expect("engine");
    push_chunked(&mut m, 0, &reference, &late_by_d);
    let at = m.frame();
    assert!(!m.set_delay((d + delta) as f64).restarted);
    let turned = m.frame();
    let hz = m.column_hz();
    assert_eq!(hz.len(), at.h1.len());
    let tau = delta as f64 / 48_000.0;
    let miss = |f: f64, i: usize| {
        let back = Complex64::from_polar(1.0, -std::f64::consts::TAU * f * tau);
        (turned.h1[i] * back / at.h1[i]).arg().to_degrees().abs()
    };
    let (mut alone, mut centre_miss) = (0, 0f64);
    for (i, c) in turned.columns.iter().enumerate() {
        let ColumnSource::Stage(b) = c.source else {
            continue;
        };
        if b.count() != 1
            || c.validity != Validity::Valid
            || at.columns[i].validity != Validity::Valid
        {
            continue;
        }
        let bin = b.lo as f64 * m.layout().stages[b.stage].bin_hz;
        assert!((hz[i] - bin).abs() < 1e-9, "column {i}: {} {bin}", hz[i]);
        assert!(miss(hz[i], i) < 1e-6, "column {i}: {}°", miss(hz[i], i));
        centre_miss = centre_miss.max(miss(turned.freq_hz[i], i));
        alone += 1;
    }
    assert!(alone > 100, "{alone}");
    assert!(centre_miss > 1.0, "{centre_miss}°");
    // A column of several bins lies within them.
    for (i, c) in turned.columns.iter().enumerate() {
        if let ColumnSource::Stage(b) = c.source {
            let bin_hz = m.layout().stages[b.stage].bin_hz;
            let (lo, hi) = (b.lo as f64 * bin_hz, (b.hi - 1) as f64 * bin_hz);
            assert!(lo <= hz[i] && hz[i] <= hi, "column {i}");
        }
    }
}
