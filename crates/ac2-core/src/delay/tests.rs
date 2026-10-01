use super::candidates::{local_maxima, median, parabolic};
use super::estimator::{
    Ffts, GridShape, MeasSpectra, Model, Pair, Pulse, Reg, Tile, band_hi, band_weight,
    segment_starts,
};
use super::*;

const FS: f64 = 48_000.0;
const H1: Reg = Reg {
    estimator: Estimator::RegularisedH1,
    eps: 0.01,
};

/// Deterministic uniform noise in [−1, 1) (xorshift64*).
fn noise(n: usize, mut s: u64) -> Vec<f64> {
    (0..n)
        .map(|_| {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            let v = s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
            v as f64 / (1u64 << 52) as f64 - 1.0
        })
        .collect()
}

#[test]
fn median_odd_and_even() {
    assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
    assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), 2.5);
    assert_eq!(median(&mut []), 0.0);
}

#[test]
fn band_edges_are_minus_6_db() {
    let (lo, hi) = (2000.0, band_hi(16_000.0, FS));
    assert!((band_weight(lo, lo, hi) - 0.5).abs() < 1e-12);
    assert!((band_weight(hi, lo, hi) - 0.5).abs() < 1e-12);
    assert!((band_weight(5000.0, lo, hi) - 1.0).abs() < 1e-12);
    assert_eq!(band_weight(lo / 2f64.sqrt(), lo, hi), 0.0);
    assert_eq!(band_weight(0.0, lo, hi), 0.0);
    // the taper of the upper edge ends at or below Nyquist
    let hi_clip = band_hi(30_000.0, FS);
    assert!((hi_clip * 2f64.sqrt() - FS / 2.0).abs() < 1e-9);
    assert_eq!(band_weight(FS / 2.0, lo, hi_clip), 0.0);
}

#[test]
fn segment_grid_is_centred() {
    let mut s = Vec::new();
    segment_starts(12_000, 4096, 2048, &mut s);
    assert_eq!(s, vec![880, 2928, 4976, 7024]);
    segment_starts(4000, 4096, 2048, &mut s);
    assert!(s.is_empty());
}

#[test]
fn peaks_and_vertex() {
    let mut out = Vec::new();
    local_maxima(&[3.0, 1.0, 2.0, 2.0, 1.0, 0.0, 5.0], &mut out);
    // on a flat top only the last sample counts (≥ left, > right)
    assert_eq!(out, vec![0, 3, 6]);
    assert_eq!(parabolic(1.0, 2.0, 1.0), 0.0);
    assert!((parabolic(0.0, 1.0, 1.0) - 0.5).abs() < 1e-12);
    assert_eq!(parabolic(1.0, 0.0, 1.0), 0.0, "not concave");
}

#[test]
fn band_classes() {
    assert_eq!(BandClass::FullRange.segment(FS), 4096);
    assert_eq!(BandClass::FullRange.segment(96_000.0), 8192);
    assert_eq!(BandClass::Sub.segment(44_100.0), 32768);
    assert_eq!(
        Band::Custom {
            lo_hz: 1500.0,
            hi_hz: 12_000.0
        }
        .class(),
        BandClass::FullRange
    );
    assert_eq!(
        Band::Custom {
            lo_hz: 40.0,
            hi_hz: 150.0
        }
        .class(),
        BandClass::Sub
    );
    assert!((BandClass::Mid.tolerance_samples(FS) - 2.4).abs() < 1e-12);
    assert!((BandClass::Sub.tolerance_samples(FS) - 4.8).abs() < 1e-12);
    let cfg = FinderConfig::new(FS, Band::Sub);
    assert_eq!(
        cfg.search,
        SearchRange {
            min: -48_000,
            max: 48_000
        }
    );
    assert_eq!(cfg.observation_len(), 192_000);
    assert_eq!(cfg.min_observation_len(), 65_536);
}

#[test]
fn window_overlap_and_unit_delay_normalisation() {
    let mut ffts = Ffts::new();
    let (lo, hi) = Band::FullRange.edges(FS);
    let g = GridShape::new(&mut ffts, 4096, 2048, FS, lo, hi);
    assert!((g.rho(0) - 1.0).abs() < 1e-12);
    assert!((g.rho(100) - g.rho(-100)).abs() < 1e-12);
    assert!(g.rho(4096).abs() < 1e-9);
    // meas = ref delayed by 300 samples: |h| ≈ 1 at the lag, from a tile offset by 200
    let d = 300usize;
    let r = noise(20_000, 7);
    let m: Vec<f64> = r[2000 - d..2000 - d + 12_000].to_vec();
    let mut y = MeasSpectra::default();
    y.compute(&mut ffts, &g, &m);
    let pair = Pair {
        r: &r,
        a: 0,
        m: &m,
        b: 2000,
    };
    let mut tile = Tile::default();
    tile.compute(&mut ffts, &g, &y, pair, 100, H1);
    assert_eq!(tile.k, 4);
    let h = tile.at(&g, 200);
    // white excitation: Gxx ≈ mean, so the regularisation costs 1/(1 + ε)
    assert!((h.norm() - 1.0 / 1.01).abs() < 0.02, "{h}");
    assert!(h.arg().abs() < 0.05, "in phase: {h}");
    assert!(tile.at(&g, 250).norm() < 0.1);
}

#[test]
fn oversampled_model_matches_the_integer_pulse() {
    let mut ffts = Ffts::new();
    let (lo, hi) = Band::Mid.edges(FS);
    let g = GridShape::new(&mut ffts, 16_384, 4096, FS, lo, hi);
    let p = Pulse::new(&mut ffts, &g, None, H1);
    assert!(
        (p.width - 20.0).abs() < 0.5,
        "nominal mid width {}",
        p.width
    );
    let model = Model::new(&mut ffts, &p.pw, 40.0);
    assert!((model.abs_at(0.0) - 1.0).abs() < 1e-9);
    for k in -40..=40 {
        assert!(
            (model.abs_at(k as f64) - p.env_at(k)).abs() < 1e-6,
            "k = {k}: {} vs {}",
            model.abs_at(k as f64),
            p.env_at(k)
        );
    }
    assert_eq!(model.abs_at(41.0), 0.0, "outside the grid");
    // zero phase at the peak, half amplitude at half the width
    assert!(model.at(0.0).im.abs() < 1e-9);
    assert!((model.abs_at(p.width / 2.0) - 0.5).abs() < 0.01);
}

#[test]
fn stream_cuts_back_to_back_windows_and_restarts_on_a_gap() {
    let mut cfg = FinderConfig::new(FS, Band::FullRange);
    cfg.search = SearchRange {
        min: -2400,
        max: 2400,
    };
    let x: Vec<f32> = noise(80_000, 3).iter().map(|&v| (0.1 * v) as f32).collect();
    let mut s = DelayStream::new(cfg).expect("config");
    let push = |s: &mut DelayStream, from: usize, to: usize, base: u64| {
        let mut windows = Vec::new();
        for c in (from..to).step_by(1000) {
            let blk = Block {
                start: base + c as u64,
                samples: &x[c..(c + 1000).min(to)],
            };
            s.push_ref(blk);
            s.push_meas(blk);
            while let Some(r) = s.poll().expect("config") {
                assert!(
                    r.accepted().is_some_and(|a| a.delay == 0),
                    "{:?}",
                    r.outcome
                );
                windows.push(r.meas_window);
            }
        }
        windows
    };
    let windows = push(&mut s, 0, 30_000, 0);
    // the first window starts where its largest lag has ref, the next one follows it
    assert_eq!(windows, vec![2400..14_400, 14_400..26_400]);
    // a jump in the indices is a discontinuity: both streams restart there
    let windows = push(&mut s, 30_000, 60_000, 1_000_000);
    assert_eq!(windows, vec![1_032_400..1_044_400, 1_044_400..1_056_400]);
}
