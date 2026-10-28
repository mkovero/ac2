//! `Mtw::fill`: counted in accumulated blocks against each stage's depth.

use super::*;

/// Full-rate pairs after which every stage of a fresh stream holds its full FIFO.
fn pairs_to_full(m: &Mtw) -> usize {
    m.layout()
        .stages
        .iter()
        .zip(m.stage_averaging())
        .map(|(s, a)| s.pairs_for_blocks(a.full_blocks() as usize) as usize)
        .max()
        .expect("stages")
}

/// The ratio `fill` should report, worked out from the blocks each stage holds.
fn expected_fill(m: &Mtw) -> f64 {
    m.stage_averaging()
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let held = m.stage_spectra(i).map_or(0, |s| s.blocks) as f64;
            (held / a.full_blocks()).min(1.0)
        })
        .fold(1.0, f64::min)
}

fn noise(n: usize) -> (Vec<f64>, Vec<f64>) {
    (Noise(301).vec(n, 0.2), Noise(302).vec(n, 0.2))
}

/// From a restart, `fill` reaches 1 exactly when the last stage completes its FIFO, and a
/// gap in the stream (a restart) takes it back to 0.
#[test]
fn fill_after_restart() {
    let sr = 48_000.0;
    let mut m = engine(sr, Averaging::Fifo { blocks: 4 }, 0);
    assert_eq!(m.fill(), 0.0);
    let n = pairs_to_full(&m);
    let (x, y) = noise(n + 10_000);
    push_chunked(&mut m, 0, &x[..n - 1], &y[..n - 1]);
    let before = m.fill();
    assert!(before > 0.0 && before < 1.0, "{before}");
    assert_eq!(before, expected_fill(&m));
    m.push(n as u64 - 1, &x[n - 1..n], &y[n - 1..n], SampleGate::Accept)
        .expect("push");
    assert_eq!(m.fill(), 1.0);
    let out = m
        .push(n as u64 + 5_000, &x[n..], &y[n..], SampleGate::Accept)
        .expect("push");
    assert!(out.restarted);
    assert!(m.fill() < 1.0, "{}", m.fill());
    assert_eq!(m.fill(), expected_fill(&m));
}

/// After an averaging reset framing continues, so each stage is full `settle_s` plus one
/// hop later, sooner than its `fill_s` from a restart.
#[test]
fn fill_after_averaging_reset() {
    let sr = 48_000.0;
    let mut m = engine(sr, Averaging::Fifo { blocks: 4 }, 0);
    let n = pairs_to_full(&m);
    let depths = m.stage_depths();
    let refill = m
        .layout()
        .stages
        .iter()
        .zip(&depths)
        .map(|(s, d)| ((d.settle_s + s.hop_s()) * sr).round() as usize)
        .max()
        .expect("stages");
    let short = m
        .layout()
        .stages
        .iter()
        .zip(&depths)
        .map(|(s, d)| (d.settle_s * sr).round() as usize - s.factor)
        .max()
        .expect("stages");
    assert!(refill < n);
    let (x, y) = noise(n + refill);
    push_chunked(&mut m, 0, &x[..n], &y[..n]);
    assert_eq!(m.fill(), 1.0);
    m.reset_averages();
    assert_eq!(m.fill(), 0.0);
    push_chunked(&mut m, n as u64, &x[n..n + short], &y[n..n + short]);
    assert!(m.fill() < 1.0, "{}", m.fill());
    assert_eq!(m.fill(), expected_fill(&m));
    push_chunked(&mut m, (n + short) as u64, &x[n + short..], &y[n + short..]);
    assert_eq!(m.fill(), 1.0);
}

/// Rejected blocks add nothing: after as many samples as fill a clean stream, a stream with
/// a rejected stretch still reads below 1, and fills once enough accepted blocks arrive.
#[test]
fn fill_rejected_blocks_count_nothing() {
    let sr = 48_000.0;
    let mut m = engine(sr, Averaging::Fifo { blocks: 4 }, 0);
    let n = pairs_to_full(&m);
    let (x, y) = noise(3 * n);
    let r = n / 4;
    push_chunked(&mut m, 0, &x[..r], &y[..r]);
    m.push(
        r as u64,
        &x[r..r + 2_000],
        &y[r..r + 2_000],
        SampleGate::Reject,
    )
    .expect("push");
    push_chunked(
        &mut m,
        (r + 2_000) as u64,
        &x[r + 2_000..n],
        &y[r + 2_000..n],
    );
    assert!(m.fill() < 1.0, "{}", m.fill());
    assert_eq!(m.fill(), expected_fill(&m));
    push_chunked(&mut m, n as u64, &x[n..], &y[n..]);
    assert_eq!(m.fill(), 1.0);
}

/// A `FastLf`-capped stage is full at its capped depth: the capped engine reads 1 where the
/// equal-confidence engine, fed the same samples, still settles its deep stages.
#[test]
fn fill_fast_lf_capped_depth() {
    let sr = 48_000.0;
    let avg = Averaging::Fifo { blocks: 8 };
    let mut capped = engine_with(sr, avg, DepthPolicy::fast_lf(), 0);
    let mut equal = engine(sr, avg, 0);
    assert!(capped.stage_depths().iter().any(|d| d.capped));
    let n = pairs_to_full(&capped);
    assert!(n < pairs_to_full(&equal));
    let (x, y) = noise(n);
    push_chunked(&mut capped, 0, &x[..n - 1], &y[..n - 1]);
    assert!(capped.fill() < 1.0);
    capped
        .push(n as u64 - 1, &x[n - 1..], &y[n - 1..], SampleGate::Accept)
        .expect("push");
    assert_eq!(capped.fill(), 1.0);
    push_chunked(&mut equal, 0, &x, &y);
    assert!(equal.fill() < 1.0, "{}", equal.fill());
    assert_eq!(equal.fill(), expected_fill(&equal));
}

/// Exponential stages are full after `settle_blocks` blocks (three time constants), where
/// the start-up deficit of the weight has fallen to e⁻³.
#[test]
fn fill_exponential_stages() {
    let sr = 48_000.0;
    let mut m = engine(
        sr,
        Averaging::Exponential {
            time_constant_s: 0.5,
        },
        0,
    );
    let n = m
        .layout()
        .stages
        .iter()
        .zip(m.stage_averaging())
        .map(|(s, a)| s.pairs_for_blocks(a.full_blocks().ceil() as usize) as usize)
        .max()
        .expect("stages");
    let (x, y) = noise(n);
    let mut last = 0.0;
    let step = n / 40;
    let mut i = 0;
    while i < n - 1 {
        let e = (i + step).min(n - 1);
        m.push(i as u64, &x[i..e], &y[i..e], SampleGate::Accept)
            .expect("push");
        let f = m.fill();
        assert_eq!(f, expected_fill(&m));
        assert!(f >= last && f < 1.0, "{f} after {e}");
        last = f;
        i = e;
    }
    m.push(n as u64 - 1, &x[n - 1..], &y[n - 1..], SampleGate::Accept)
        .expect("push");
    assert_eq!(m.fill(), 1.0);
    for a in m.stage_averaging() {
        let StageAveraging::Exponential { alpha } = a else {
            panic!("exponential");
        };
        assert!((a.full_blocks() - 3.0 / -(1.0 - alpha).ln()).abs() < 1e-12);
    }
}

/// A delay step that starts some stages over drops `fill`; a step every stage keeps does not.
#[test]
fn fill_follows_delay_changes() {
    let sr = 48_000.0;
    let mut m = engine(sr, Averaging::Fifo { blocks: 4 }, 0);
    let n = pairs_to_full(&m);
    let (x, y) = noise(n);
    push_chunked(&mut m, 0, &x, &y);
    assert_eq!(m.fill(), 1.0);
    let kept = m.set_delay(0.25);
    assert_eq!(kept.kept, (1 << m.layout().stages.len()) - 1);
    assert_eq!(m.fill(), 1.0);
    let step = m.set_delay(4_000.0);
    assert_ne!(step.kept, (1 << m.layout().stages.len()) - 1);
    assert!(m.fill() < 1.0, "{}", m.fill());
    assert_eq!(m.fill(), expected_fill(&m));
}
