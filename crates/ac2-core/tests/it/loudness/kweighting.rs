//! K-weighting: Tables 1 and 2 at 48 kHz, the per-rate derivation against refgen, the
//! response held to the 48 kHz curve, and the rate rule.

use ac2_core::loudness::{KWeighting, LoudnessMeter, TABLE_1, TABLE_2, Unsupported, supports_rate};

use super::{RATES, set};

fn flat(k: &KWeighting) -> [[f64; 5]; 2] {
    [k.pre, k.rlb].map(|c| [c.b[0], c.b[1], c.b[2], c.a[0], c.a[1]])
}

#[test]
fn coefficients_at_48k_equal_tables_1_and_2() {
    let k = flat(&KWeighting::new(48000.0));
    for (got, want) in k.iter().zip([TABLE_1, TABLE_2]) {
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() <= 1e-14, "{g:e} vs table {w:e}");
        }
    }
    // The RLB numerator is the table's unnormalised [1, −2, 1] exactly at 48 kHz (g = 1).
    assert_eq!(k[1][..3], [1.0, -2.0, 1.0]);
}

#[test]
fn coefficients_match_golden_at_every_rate() {
    let g = set("loudness_kweighting");
    let got: Vec<f64> = RATES
        .iter()
        .flat_map(|&fs| flat(&KWeighting::new(f64::from(fs))).into_iter().flatten())
        .collect();
    g.assert_f64("coefficients", &got);
    let tables = g.f64("tables").expect("tables");
    assert_eq!(tables, [TABLE_1, TABLE_2].concat(), "tables as printed");
}

#[test]
fn response_matches_golden() {
    let g = set("loudness_kweighting");
    let freqs = g.f64("freqs").expect("freqs");
    let got: Vec<f64> = RATES
        .iter()
        .flat_map(|&fs| {
            let fs = f64::from(fs);
            let k = KWeighting::new(fs);
            freqs
                .iter()
                .map(move |&f| k.magnitude_db(f, fs))
                .collect::<Vec<_>>()
        })
        .collect();
    g.assert_f64("response_db", &got);
}

/// Every rate follows the 48 kHz curve within ±0.01 dB from 20 Hz to 19.2 kHz.
#[test]
fn response_follows_48k_within_a_hundredth_of_a_db() {
    let k48 = KWeighting::new(48000.0);
    let n = 2000;
    for fs in RATES.map(f64::from) {
        let k = KWeighting::new(fs);
        let mut worst = (0.0f64, 0.0);
        for i in 0..=n {
            let f = 20.0 * (19200.0f64 / 20.0).powf(f64::from(i) / f64::from(n));
            let d = k.magnitude_db(f, fs) - k48.magnitude_db(f, 48000.0);
            if d.abs() > worst.0.abs() {
                worst = (d, f);
            }
        }
        assert!(
            worst.0.abs() <= 0.01,
            "{fs} Hz: {:+.4} dB at {:.0} Hz",
            worst.0,
            worst.1
        );
    }
}

#[test]
fn loudness_runs_at_44k1_to_192k_multiples_of_100_hz_only() {
    for fs in RATES {
        assert!(supports_rate(f64::from(fs)), "{fs}");
        assert!(LoudnessMeter::new(f64::from(fs), 2).is_ok());
    }
    for fs in [32000.0, 47952.0, 384000.0, 44100.5, 22050.0, 0.0, f64::NAN] {
        assert!(!supports_rate(fs), "{fs}");
        let err = LoudnessMeter::new(fs, 2).expect_err("refused");
        assert!(matches!(err, Unsupported::Rate(_)));
    }
    let msg = LoudnessMeter::new(32000.0, 2)
        .expect_err("refused")
        .to_string();
    assert!(
        msg.starts_with("loudness needs a rate of 44.1 to 192 kHz, a multiple of 100 Hz"),
        "{msg}"
    );
    for ch in [0, 3] {
        assert_eq!(
            LoudnessMeter::new(48000.0, ch).expect_err("refused"),
            Unsupported::Channels(ch)
        );
    }
}
