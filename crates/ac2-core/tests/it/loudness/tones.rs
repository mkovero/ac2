//! Tones at 40 Hz, 1 kHz and 10 kHz read the same at every rate; the −18 dBFS calibration
//! tone reads −18.0 LUFS; a mono source weighs its one channel 1.

use super::{RATES, assert_near, param, run, set};

const PARITY_LU: f64 = 0.01;

#[test]
fn tones_read_the_same_integrated_loudness_at_every_rate() {
    let g = set("loudness_tones");
    let tol = param(&g, "rate_tolerance_lu").as_f64().expect("tolerance");
    for f in ["40", "1000", "10000"] {
        let prog = param(&g, &format!("tones/{f}"));
        let at48 = run(prog, 48000).integrated.expect("integrated");
        for fs in RATES {
            let got = run(prog, fs).integrated.expect("integrated");
            let reference = g
                .scalar(&format!("tone{f}_{fs}_integrated"))
                .expect("scalar");
            assert_near(
                &format!("{f} Hz tone at {fs} Hz vs reference"),
                got,
                reference,
                PARITY_LU,
            );
            assert_near(&format!("{f} Hz tone at {fs} Hz vs 48 kHz"), got, at48, tol);
        }
    }
}

#[test]
fn calibration_tone_reads_minus_18_lufs() {
    let g = set("loudness_tones");
    let c = param(&g, "calibration");
    let want = c["published"].as_f64().expect("published");
    let tol = c["tolerance_lu"].as_f64().expect("tolerance");
    let r = run(&c["programme"], 48000);
    for (what, v) in [
        ("integrated", r.integrated.expect("integrated")),
        ("momentary", r.momentary),
        ("short_term", r.short_term),
    ] {
        assert_near(&format!("calibration {what}"), v, want, tol);
        let reference = g.scalar(&format!("calibration_{what}")).expect("scalar");
        assert_near(
            &format!("calibration {what} vs reference"),
            v,
            reference,
            PARITY_LU,
        );
    }
    // One decimal on screen: −18.0.
    assert_eq!(format!("{:.1}", r.integrated.expect("integrated")), "-18.0");
}

#[test]
fn mono_source_reads_3_lu_under_the_pair() {
    let g = set("loudness_tones");
    for case in ["case1", "case3"] {
        let r = run(param(&g, &format!("mono/{case}")), 48000);
        let got = r.integrated.expect("integrated");
        let reference = g
            .scalar(&format!("mono_{case}_integrated"))
            .expect("scalar");
        assert_near(
            &format!("mono {case} vs reference"),
            got,
            reference,
            PARITY_LU,
        );
        assert_near(&format!("mono {case}"), got, -23.0 - 3.0103, 0.1);
    }
}
