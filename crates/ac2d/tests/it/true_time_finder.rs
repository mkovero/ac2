//! The fakes' shared signal ([`TrueTimeNoise`]) through the delay finder as the transfer job
//! runs it (full range, the default search): the daemon tests of rigs on different clocks
//! measure their delays on it, so it must excite the finder's band and place a delay.

use ac2_audio::fake::TrueTimeNoise;
use ac2_core::delay::{Band, Block, DelayStream, FinderConfig, Outcome};

const FS: u32 = 48_000;
/// A fractional delay, so the finder's sub-sample placement is checked too.
const DELAY: f64 = 137.37;

#[test]
fn the_finder_accepts_the_shared_signal_and_places_its_delay() {
    // Any instant: the signal is a function of absolute true time.
    let origin_ns = 1_764_000_000_123_456_789u64;
    // `at` reads `origin_ns + t_ns` for `t_ns` of either sign.
    let period_ns = 1e9 / f64::from(FS);
    let mut g = TrueTimeNoise::new(FS);
    let mut at = |t_ns: f64| {
        let w = t_ns.floor();
        g.at(origin_ns.saturating_add_signed(w as i64), t_ns - w)
    };
    let n = 8 * FS as usize;
    // The measurement hears the reference's signal DELAY samples later.
    let r: Vec<f32> = (0..n).map(|i| at(i as f64 * period_ns)).collect();
    let m: Vec<f32> = (0..n).map(|i| at((i as f64 - DELAY) * period_ns)).collect();

    let mut s =
        DelayStream::new(FinderConfig::new(f64::from(FS), Band::FullRange)).expect("config");
    let mut results = Vec::new();
    for (i, (rc, mc)) in r.chunks(1024).zip(m.chunks(1024)).enumerate() {
        let start = (i * 1024) as u64;
        s.push_ref(Block { start, samples: rc });
        s.push_meas(Block { start, samples: mc });
        while let Some(res) = s.poll().expect("config") {
            results.push(res);
        }
    }
    assert!(results.len() >= 2, "{} windows", results.len());
    for res in &results {
        let Outcome::Accepted { first, .. } = &res.outcome else {
            panic!("not accepted: {:?}", res.outcome);
        };
        let excited = res.confidence.excited_fraction;
        println!(
            "window {:?}: delay {:.3}, excited {excited:.3}, psr {:.1} dB",
            res.meas_window, first.delay_frac, res.confidence.psr_db
        );
        assert!(excited >= 0.8, "excited fraction {excited:.3}");
        assert!(
            (first.delay_frac - DELAY).abs() < 0.1,
            "delay {:.3} against {DELAY}",
            first.delay_frac
        );
    }
}
