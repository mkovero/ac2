use super::*;
use ac2_testkit::golden::GoldenSet;
use std::f64::consts::{PI, TAU};

fn curve(pts: &[(f64, f64)]) -> MicCurve {
    MicCurve::from_points(pts).expect("valid curve")
}

#[test]
fn parse_layouts() {
    // REW .frd: header text, three columns.
    let frd = b"* Measurement mic M30 #1234\n* Freq(Hz) SPL(dB) Phase(degrees)\n20 -1.5 10\n1000 0.0 0\n20000 2.25 -30\n";
    let c = MicCurve::parse(frd).expect("frd");
    assert_eq!(c.freqs(), &[20.0, 1000.0, 20000.0]);
    assert_eq!(c.gains(), &[-1.5, 0.0, 2.25]);

    // miniDSP UMIK: quoted vendor line, tabs, CRLF, BOM.
    let umik = "\u{feff}\"Sens Factor =-1.378dB, SERNO: 7001234\"\r\n10.054\t-3.2\r\n1000.0\t0\r\n";
    let c = MicCurve::parse(umik.as_bytes()).expect("umik");
    assert_eq!(c.len(), 2);
    assert!((c.f_lo() - 10.054).abs() < 1e-12);

    // CSV with a header row, and European semicolon + decimal comma.
    let c = MicCurve::parse(b"freq,db\n20,1.5\n100, -0.25\n").expect("csv");
    assert_eq!(c.gains(), &[1.5, -0.25]);
    let c = MicCurve::parse(b"Frequenz;Pegel\n20,5;-1,25\n1000;0\n").expect("semicolon");
    assert_eq!(c.freqs(), &[20.5, 1000.0]);
    assert_eq!(c.gains(), &[-1.25, 0.0]);

    // Lone CR line ends, comments anywhere, Latin-1 bytes in a comment.
    let mut b = b"# Kalibrierung \xb0C\r20 1\r; mid\r2000 -1\r# end\r".to_vec();
    b.extend_from_slice(b"// trailing\r");
    let c = MicCurve::parse(&b).expect("cr");
    assert_eq!(c.len(), 2);
}

#[test]
fn parse_errors_name_their_line() {
    use MicCurveFileError as E;
    let cases: [(&[u8], E); 8] = [
        (b"# only text\n", E::TooFewPoints { found: 0 }),
        (b"20 1\n", E::TooFewPoints { found: 1 }),
        (b"20 1\n100\n", E::MissingGain { line: 2 }),
        (b"20 1\n100 x\n", E::BadNumber { line: 2 }),
        (b"0 1\n100 0\n", E::NonPositiveFrequency { line: 1 }),
        (b"20 nan\n100 0\n", E::NonFinite { line: 1 }),
        (b"20 1\n100 41\n", E::GainOutOfRange { line: 2 }),
        (b"20 1\n\n# c\n20 0\n", E::NotAscending { line: 4 }),
    ];
    for (text, want) in cases {
        assert_eq!(
            MicCurve::parse(text),
            Err(want),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
    let many: String = (1..=MAX_POINTS + 5).map(|i| format!("{i} 0\n")).collect();
    assert_eq!(
        MicCurve::parse(many.as_bytes()),
        Err(E::TooManyPoints {
            found: MAX_POINTS + 5
        })
    );
    assert_eq!(E::NotAscending { line: 4 }.line(), Some(4));
    assert!(E::GainOutOfRange { line: 2 }.to_string().contains("line 2"));
    assert_eq!(
        MicCurve::from_points(&[(100.0, 0.0), (50.0, 0.0)]),
        Err(E::NotAscending { line: 2 })
    );
}

#[test]
fn interpolation_is_log_linear_and_held_outside() {
    let c = curve(&[(100.0, -2.0), (1000.0, 0.0), (10_000.0, 4.0)]);
    assert_eq!(c.gain_db(1000.0), 0.0);
    // Geometric midpoint of a segment is the arithmetic mean of its ends.
    assert!((c.gain_db(10f64.powf(2.5)) - -1.0).abs() < 1e-12);
    assert!((c.gain_db(10f64.powf(3.25)) - 1.0).abs() < 1e-12);
    for f in [0.0, 1.0, 99.9, f64::NAN] {
        assert_eq!(c.gain_db(f), -2.0, "{f}");
    }
    assert_eq!(c.gain_db(20_000.0), 4.0);
    assert_eq!(c.gain_db(f64::INFINITY), 4.0);
}

#[test]
fn normalisation_band_average_and_subtract() {
    let c = curve(&[(100.0, -2.0), (1000.0, 1.0), (10_000.0, 4.0)]);
    let n = c.normalised(1000.0);
    assert_eq!(n.db(1000.0), 0.0);
    assert!((n.db(10_000.0) - 3.0).abs() < 1e-12);
    // A constant curve's band average is the constant.
    let flat = curve(&[(10.0, 2.5), (20_000.0, 2.5)]).normalised(250.0);
    assert_eq!(flat.db(5000.0), 0.0);
    assert!(flat.band_db(700.0, 1400.0).abs() < 1e-12);
    let k = curve(&[(10.0, -1.5), (20_000.0, -1.5)]).normalised(15.0);
    assert!(k.band_db(100.0, 200.0).abs() < 1e-12);
    // Power average lies between the band's extremes and is not their arithmetic mean
    // when the correction varies.
    let b = n.band_db(2000.0, 8000.0);
    assert!(b > n.db(2000.0) && b < n.db(8000.0), "{b}");
    let mut v = [10.0f32, f32::NAN, 0.0];
    n.subtract(&[1000.0, 2000.0, 10_000.0], &mut v);
    assert_eq!(v[0], 10.0);
    assert!(v[1].is_nan());
    assert!((v[2] - -3.0).abs() < 1e-6);
}

#[test]
fn golden_display_correction() {
    let g = GoldenSet::load("calibration_mic_curve").expect("golden");
    let f = g.f64("curve_freq_hz").expect("f");
    let db = g.f64("curve_gain_db").expect("db");
    let pts: Vec<(f64, f64)> = f.iter().copied().zip(db.iter().copied()).collect();
    let n = curve(&pts).normalised(g.scalar("f_norm_hz").expect("fn"));
    let t = g.f64("test_freq_hz").expect("t");
    let sub: Vec<f64> = t.iter().map(|&f| n.db(f)).collect();
    g.assert_f64("subtract_db", &sub);
    let third = 10f64.powf(0.05);
    let band: Vec<f64> = t.iter().map(|&f| n.band_db(f / third, f * third)).collect();
    g.assert_f64("band_subtract_db", &band);
}

/// FIR magnitude against the analog model, 31.5 Hz … min(16 kHz, 0.4·fs), per rate.
#[test]
fn golden_fir_magnitude_per_rate() {
    let g = GoldenSet::load("calibration_mic_curve").expect("golden");
    let f = g.f64("curve_freq_hz").expect("f");
    let db = g.f64("curve_gain_db").expect("db");
    let pts: Vec<(f64, f64)> = f.iter().copied().zip(db.iter().copied()).collect();
    let n = curve(&pts).normalised(1000.0);
    let t = g.f64("test_freq_hz").expect("t");
    let want = g.f64("subtract_db").expect("want");
    for fs in [44_100.0, 48_000.0, 96_000.0] {
        let h = n.design_fir(fs);
        assert_eq!(h.len(), fir_len(fs));
        assert_eq!(h.len() / fir_partition(fs), 64);
        assert!((dtft(&h, 1000.0, fs).norm() - 1.0).abs() < 1e-12);
        let mut worst: f64 = 0.0;
        for (&fq, &w) in t.iter().zip(&want) {
            if fq < 31.0 || fq > 16_000.0f64.min(0.4 * fs) {
                continue;
            }
            let got = 20.0 * dtft(&h, fq, fs).norm().log10();
            let err = got - -w;
            worst = worst.max(err.abs());
            assert!(
                err.abs() <= 0.1,
                "{fs} Hz, {fq:.1} Hz: {got:.3} vs {:.3}",
                -w
            );
        }
        eprintln!("fs {fs}: max |FIR − analog| = {worst:.4} dB (31.5 Hz – 16 kHz)");
    }
}

#[test]
fn flat_curve_designs_a_unit_impulse() {
    let n = curve(&[(20.0, 3.0), (20_000.0, 3.0)]).normalised(1000.0);
    let h = n.design_fir(48_000.0);
    assert!((h[0] - 1.0).abs() < 1e-12, "{}", h[0]);
    assert!(h[1..].iter().all(|v| v.abs() < 1e-12));
}

/// RBJ peaking biquad (b, a), a0 = 1. Poles and zeros lie inside the unit circle, so it is
/// minimum phase, and so is its inverse.
fn peaking(fs: f64, f0: f64, gain_db: f64, q: f64) -> ([f64; 3], [f64; 3]) {
    let a = 10f64.powf(gain_db / 40.0);
    let w0 = TAU * f0 / fs;
    let alpha = w0.sin() / (2.0 * q);
    let a0 = 1.0 + alpha / a;
    (
        [
            (1.0 + alpha * a) / a0,
            -2.0 * w0.cos() / a0,
            (1.0 - alpha * a) / a0,
        ],
        [1.0, -2.0 * w0.cos() / a0, (1.0 - alpha / a) / a0],
    )
}

fn biquad_db(b: &[f64; 3], a: &[f64; 3], f: f64, fs: f64) -> f64 {
    let z = Complex64::from_polar(1.0, -TAU * f / fs);
    let num = b[0] + b[1] * z + b[2] * z * z;
    let den = a[0] + a[1] * z + a[2] * z * z;
    20.0 * (num / den).norm().log10()
}

/// The minimum-phase filter with a given magnitude is unique: designing from the magnitude
/// of a minimum-phase biquad must give the impulse response of its inverse.
#[test]
fn min_phase_design_reproduces_inverse_biquad() {
    let fs = 48_000.0;
    let (b, a) = peaking(fs, 3000.0, 6.0, 1.5);
    let pts: Vec<(f64, f64)> = (0..)
        .map(|k| 10.0 * 2f64.powf(k as f64 / 96.0))
        .take_while(|&f| f < 0.5 * fs)
        .map(|f| (f, biquad_db(&b, &a, f, fs)))
        .collect();
    let n = curve(&pts).normalised(1000.0);
    let h = n.design_fir(fs);
    // Inverse biquad (swap numerator and denominator), scaled to 0 dB at 1 kHz.
    let g = 10f64.powf(biquad_db(&b, &a, 1000.0, fs) / 20.0);
    let (nb, na) = (a, b);
    let mut want = vec![0.0; h.len()];
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    for (i, w) in want.iter_mut().enumerate() {
        let x = if i == 0 { g } else { 0.0 };
        let y = (nb[0] * x + nb[1] * x1 + nb[2] * x2 - na[1] * y1 - na[2] * y2) / na[0];
        (x2, x1, y2, y1) = (x1, x, y1, y);
        *w = y;
    }
    let peak = want.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let err = h
        .iter()
        .zip(&want)
        .fold(0.0f64, |m, (x, y)| m.max((x - y).abs()));
    eprintln!(
        "min-phase design vs inverse biquad: max |Δh| / peak = {:.2e}",
        err / peak
    );
    assert!(err / peak < 2e-3, "{}", err / peak);
    // Minimum phase: the energy is at the start (no pre-ringing, no bulk delay).
    let total: f64 = h.iter().map(|v| v * v).sum();
    let first: f64 = h[..48].iter().map(|v| v * v).sum();
    assert!(first / total > 0.99, "{}", first / total);
    let _ = PI;
}

#[test]
fn partitioned_convolution_equals_direct() {
    let mut s = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    };
    for (len, part) in [
        (1usize, 256),
        (255, 256),
        (256, 256),
        (257, 256),
        (1000, 64),
        (4096, 512),
        // Long enough for a tail of long partitions: 2, 8 and 4 of them.
        (600, 64),
        (4100, 64),
        (5000, 64),
    ] {
        let h: Vec<f64> = (0..len).map(|_| rnd()).collect();
        let x: Vec<f64> = (0..12_000).map(|_| rnd()).collect();
        let mut fir = PartitionedFir::new(&h, part);
        let lat = fir.latency();
        let mut y = vec![0.0; x.len()];
        // Uneven chunks: the block boundary must not matter.
        let mut i = 0;
        for c in [1usize, 7, 300, 255, 1024, 13].iter().cycle() {
            if i >= x.len() {
                break;
            }
            let e = (i + c).min(x.len());
            fir.process(&x[i..e], &mut y[i..e]);
            i = e;
        }
        let want: Vec<f64> = (0..x.len())
            .map(|n| {
                if n < lat {
                    0.0
                } else {
                    let m = n - lat;
                    (0..=m.min(len - 1)).map(|k| h[k] * x[m - k]).sum()
                }
            })
            .collect();
        // The convolution runs in f32: its round-off scales with the output's RMS, and
        // 2·10⁻⁶ of it (−114 dB) bounds every sample with margin.
        let rms = (want.iter().map(|v| v * v).sum::<f64>() / want.len() as f64).sqrt();
        let mut err2 = 0.0;
        for (n, (yn, w)) in y.iter().zip(&want).enumerate() {
            assert!(
                (yn - w).abs() < 2e-6 * rms,
                "len {len} n {n}: {yn} vs {w} (rms {rms})"
            );
            err2 += (yn - w) * (yn - w);
        }
        let rel_db = 10.0 * (err2 / want.len() as f64 / (rms * rms)).log10();
        eprintln!("len {len} part {part}: f32 round-off {rel_db:.1} dB re output RMS");
        assert!(rel_db < -125.0, "len {len}: {rel_db} dB");
        fir.reset();
        let mut z = vec![1.0; 10];
        fir.process(&[0.0; 10], &mut z);
        assert!(z.iter().all(|v| *v == 0.0));
    }
}
