use super::*;
use crate::spectrum::{Averaging, SpectrumAnalyzer, SpectrumConfig};
use std::f64::consts::TAU;

const FS: f64 = 48_000.0;
const WINDOWED: [Window; 3] = [Window::Hann, Window::BlackmanHarris4, Window::FlatTop];

/// Deterministic Gaussian noise (xorshift64* + Box–Muller); statistics only.
struct Noise(u64);
impl Noise {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

/// A sine of amplitude `a1` at `f0` plus harmonics `(order, amplitude, phase)` and white
/// noise of standard deviation `sigma`.
fn signal(len: usize, f0: f64, a1: f64, harmonics: &[(u32, f64, f64)], sigma: f64) -> Vec<f64> {
    let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
    (0..len)
        .map(|i| {
            let t = i as f64 / FS;
            let mut x = a1 * (TAU * f0 * t).sin();
            for &(k, a, phase) in harmonics {
                x += a * (TAU * k as f64 * f0 * t + phase).sin();
            }
            if sigma > 0.0 {
                x += sigma * noise.gauss();
            }
            x
        })
        .collect()
}

/// `frames` contiguous frames of `x` averaged by an analyzer, then read.
fn read(
    x: &[f64],
    n: usize,
    window: Window,
    f0: f64,
    max_order: u32,
) -> Result<Harmonics, Unreadable> {
    let frames = x.len() / n;
    let mut a = SpectrumAnalyzer::new(SpectrumConfig {
        fs: FS,
        n,
        hop: n,
        window,
        averaging: Averaging::Fifo { frames },
        peak_hold: None,
    })
    .expect("config");
    assert_eq!(a.push(x), frames);
    read_harmonics(&a.average().expect("frames"), window, f0, max_order)
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

/// Amplitude of a level in dB re an amplitude of 1.
fn amp(level_db: f64) -> f64 {
    10f64.powf(level_db / 20.0)
}

#[test]
fn on_bin_rectangular_reads_every_harmonic_to_minus_140_dbc() {
    let n = 16_384;
    let f0 = snap_to_bin(1000.0, FS, n);
    let a1 = 0.5;
    // Orders 2..=8 at −20, −40, … −140 dBc.
    let levels: Vec<(u32, f64)> = (2..=8).map(|k| (k, -20.0 * (k - 1) as f64)).collect();
    let harmonics: Vec<(u32, f64, f64)> = levels
        .iter()
        .map(|&(k, l)| (k, a1 * amp(l), 0.7 * k as f64))
        .collect();
    let x = signal(2 * n, f0, a1, &harmonics, 0.0);
    let h = read(&x, n, Window::Rectangular, f0, 8).expect("readable");

    assert!(h.on_bin);
    assert_eq!(h.leakage, 0.0);
    assert!(db(h.fundamental / (a1 * a1 / 2.0)).abs() < 0.01);
    assert_eq!(h.harmonics.len(), levels.len());
    for (got, &(k, level)) in h.harmonics.iter().zip(&levels) {
        assert_eq!(got.order, k);
        assert!(got.resolved, "order {k}: floor {}", db(got.floor));
        let dbc = db(got.power / h.fundamental);
        assert!((dbc - level).abs() < 0.01, "order {k}: {dbc} vs {level}");
    }
    let thd: f64 = levels.iter().map(|&(_, l)| amp(l).powi(2)).sum();
    assert!(db(h.thd / thd).abs() < 0.01, "thd {}", db(h.thd));
    assert!(db(h.thd_bound / thd).abs() < 0.01);
    // With no noise the residual is the harmonics.
    assert!(db(h.thd_n / thd).abs() < 0.01);
}

#[test]
fn off_bin_windows_read_fundamental_and_harmonics() {
    let n = 65_536;
    let a1 = 0.8;
    let bin = (1000.0 * n as f64 / FS).round();
    let mut freqs: Vec<f64> = [0.0, 0.25, 0.5]
        .iter()
        .map(|d| (bin + d) * FS / n as f64)
        .collect();
    freqs.push(1000.0);
    for window in WINDOWED {
        for &f0 in &freqs {
            let harmonics = [(2, a1 * amp(-60.0), 1.0), (3, a1 * amp(-60.0), 2.5)];
            let x = signal(2 * n, f0, a1, &harmonics, 0.0);
            let h = read(&x, n, window, f0, 10).expect("readable");
            let what = format!("{window:?} at {f0} Hz");

            assert_eq!(h.on_bin, f0 == freqs[0], "{what}");
            let err = db(h.fundamental / (a1 * a1 / 2.0));
            assert!(err.abs() < 0.05, "{what}: fundamental off by {err} dB");
            assert_eq!(h.harmonics.len(), 9, "{what}");
            for got in &h.harmonics[..2] {
                assert!(got.resolved, "{what}: order {}", got.order);
                let dbc = db(got.power / h.fundamental);
                assert!(
                    (dbc + 60.0).abs() < 0.1,
                    "{what}: order {} at {dbc}",
                    got.order
                );
            }
            // Orders with nothing there hold only the window's skirt, far below the two
            // real ones, whether or not it happens to clear its own floor.
            for got in &h.harmonics[2..] {
                assert!(
                    db(got.power / h.fundamental) < -110.0,
                    "{what}: order {}",
                    got.order
                );
            }
            assert!(
                (db(h.thd) - db(2.0 * amp(-60.0).powi(2))).abs() < 0.1,
                "{what}: thd {}",
                db(h.thd)
            );
            assert!(h.thd_bound >= h.thd, "{what}");
            // The standalone tone reading is the same number.
            let mut a = SpectrumAnalyzer::new(SpectrumConfig {
                fs: FS,
                n,
                hop: n,
                window,
                averaging: Averaging::Off,
                peak_hold: None,
            })
            .expect("config");
            a.push(&x[..n]);
            let p = tone_power(&a.average().expect("frame"), window, f0).expect("readable");
            assert!(db(p / h.fundamental).abs() < 1e-6, "{what}");
        }
    }
}

/// Between bins the residual cannot read below what the window leaks out of the lobe, and
/// `leakage` says how much that is.
#[test]
fn leakage_is_the_residual_of_a_pure_off_bin_sine() {
    let n = 16_384;
    let f0 = (341.0 + 0.5) * FS / n as f64;
    for window in WINDOWED {
        let x = signal(n, f0, 1.0, &[], 0.0);
        let h = read(&x, n, window, f0, 10).expect("readable");
        assert!(
            h.leakage > 0.0 && h.leakage < 2e-3,
            "{window:?}: {}",
            h.leakage
        );
        if window == Window::Hann {
            // Hann's skirt is far above anything else in an f64 frame.
            assert!(db(h.thd_n / h.leakage).abs() < 0.1, "{}", db(h.thd_n));
        } else {
            assert!(h.leakage < 1e-8, "{window:?}: {}", h.leakage);
        }
    }
}

#[test]
fn blackman_harris_resolves_minus_80_dbc_over_noise() {
    let n = 65_536;
    let (f0, a1) = (1000.0, 0.5);
    let harmonics = [
        (2, a1 * amp(-80.0), 0.3),
        (3, a1 * amp(-80.0), 1.3),
        (5, a1 * amp(-80.0), 2.3),
    ];
    // Noise 100 dB under the sine's amplitude: about −130 dBc in a lobe at this length.
    let x = signal(4 * n, f0, a1, &harmonics, a1 * 1e-5);
    let h = read(&x, n, Window::BlackmanHarris4, f0, 10).expect("readable");
    for got in &h.harmonics {
        let dbc = db(got.power / h.fundamental);
        if [2, 3, 5].contains(&got.order) {
            assert!(got.resolved, "order {}", got.order);
            assert!((dbc + 80.0).abs() < 0.1, "order {}: {dbc}", got.order);
        } else {
            assert!(!got.resolved, "order {} at {dbc}", got.order);
            // The floor is the noise the lobe's 8 bins hold: 2σ²·8/N.
            let noise = 2.0 * (1e-5f64).powi(2) * 2.0 * 8.0 / n as f64;
            let err = db(got.floor / h.fundamental / noise);
            assert!(
                err.abs() < 1.5,
                "order {}: floor off by {err} dB",
                got.order
            );
        }
    }
    assert!((db(h.thd) - db(3.0 * amp(-80.0).powi(2))).abs() < 0.1);
}

#[test]
fn hann_reports_a_harmonic_under_the_skirt_as_unresolved() {
    let n = 4096;
    // 12.3 bins: the second harmonic sits where the fundamental's skirt is still near
    // −75 dBc per bin.
    let f0 = 12.3 * FS / n as f64;
    let a1 = 0.5;
    let level = amp(-120.0).powi(2);
    let x = signal(n, f0, a1, &[(2, a1 * amp(-120.0), 0.4)], 0.0);
    let h = read(&x, n, Window::Hann, f0, 4).expect("readable");
    let second = h.harmonics[0];
    assert_eq!(second.order, 2);
    assert!(!second.resolved);
    assert!(second.floor / h.fundamental > 100.0 * level);
    assert_eq!(h.thd, 0.0);
    assert!(h.thd_bound > 100.0 * level);
    // The floor is the skirt under the lobe, not an arbitrary large number. This close to
    // the fundamental the skirt falls 36 dB per octave and the bins on the near side weigh
    // more, so the estimate errs high, the side that keeps a harmonic unresolved.
    let err = db(second.floor / second.power);
    assert!(
        (-1.0..6.0).contains(&err),
        "floor {} dBc, off by {err} dB",
        db(second.floor / h.fundamental)
    );
}

#[test]
fn thd_n_reads_white_noise_at_its_variance() {
    let n = 16_384;
    let (a1, sigma) = (1.0, 0.25f64);
    let expected = sigma * sigma / (a1 * a1 / 2.0);
    let cases = WINDOWED
        .iter()
        .map(|&w| (w, 1000.0))
        .chain([(Window::Rectangular, snap_to_bin(1000.0, FS, n))]);
    for (window, f0) in cases {
        let x = signal(16 * n, f0, a1, &[], sigma);
        let h = read(&x, n, window, f0, 10).expect("readable");
        let err = db(h.thd_n / expected);
        assert!(err.abs() < 0.3, "{window:?}: thd_n off by {err} dB");
        // Noise alone resolves no harmonic, and each floor is the noise in its lobe.
        for got in &h.harmonics {
            assert!(!got.resolved, "{window:?}: order {}", got.order);
            let bins = if h.on_bin {
                2 * window.lobe_half_width() - 1
            } else {
                2 * window.lobe_half_width()
            };
            let noise = sigma * sigma * 2.0 * bins as f64 / n as f64;
            let err = db(got.floor / noise);
            assert!(
                err.abs() < 1.5,
                "{window:?}: order {} floor off by {err} dB",
                got.order
            );
        }
        assert_eq!(h.thd, 0.0, "{window:?}");
        assert!(h.thd_bound > 0.0, "{window:?}");
    }
}

#[test]
fn rectangular_off_bin_is_unreadable() {
    let n = 4096;
    let x = signal(n, 1000.0, 0.5, &[], 0.0);
    assert_eq!(
        read(&x, n, Window::Rectangular, 1000.0, 10),
        Err(Unreadable::RectangularOffBin)
    );
    let on = snap_to_bin(1000.0, FS, n);
    let x = signal(n, on, 0.5, &[], 0.0);
    assert!(read(&x, n, Window::Rectangular, on, 10).is_ok());
}

#[test]
fn fundamental_too_low_for_the_fft_length_is_unreadable() {
    let n = 4096;
    let bin = FS / n as f64;
    for (window, bins) in [
        (Window::Hann, 5.0),
        (Window::Hann, 5.9),
        (Window::BlackmanHarris4, 9.5),
        (Window::FlatTop, 11.0),
        (Window::Rectangular, 3.0),
    ] {
        let f0 = bins * bin;
        let x = signal(n, f0, 0.5, &[], 0.0);
        assert_eq!(
            read(&x, n, window, f0, 10),
            Err(Unreadable::TooLow),
            "{window:?} at {bins} bins"
        );
    }
    // One step up and each has room.
    for (window, bins) in [
        (Window::Hann, 6.0),
        (Window::BlackmanHarris4, 10.5),
        (Window::FlatTop, 12.0),
        (Window::Rectangular, 4.0),
    ] {
        let f0 = bins * bin;
        let x = signal(n, f0, 0.5, &[], 0.0);
        assert!(read(&x, n, window, f0, 10).is_ok(), "{window:?} at {bins}");
    }
}

#[test]
fn harmonics_above_nyquist_are_absent() {
    let n = 4096;
    for window in WINDOWED {
        // 5 kHz: 10, 15 and 20 kHz are below 24 kHz, 25 kHz is not.
        let x = signal(n, 5000.0, 0.5, &[], 0.0);
        let h = read(&x, n, window, 5000.0, 10).expect("readable");
        let orders: Vec<u32> = h.harmonics.iter().map(|h| h.order).collect();
        assert_eq!(orders, [2, 3, 4], "{window:?}");
        // A harmonic whose lobe touches fs/2 is not read either: the fourth of 5996 Hz is
        // 1.4 bins under it, inside every one of these lobes.
        let x = signal(n, 5996.0, 0.5, &[], 0.0);
        let h = read(&x, n, window, 5996.0, 10).expect("readable");
        assert_eq!(h.harmonics.len(), 2, "{window:?}");
    }
    let x = signal(n, 1000.0, 0.5, &[], 0.0);
    let h = read(&x, n, Window::Hann, 1000.0, 3).expect("readable");
    assert_eq!(h.harmonics.len(), 2, "max order bounds the list");
    assert!(
        read(&x, n, Window::Hann, 1000.0, 1)
            .expect("readable")
            .harmonics
            .is_empty()
    );
}

#[test]
fn frequencies_outside_the_band_and_silence_are_unreadable() {
    let n = 4096;
    let x = signal(n, 1000.0, 0.5, &[], 0.0);
    for f in [
        0.0,
        -100.0,
        f64::NAN,
        f64::INFINITY,
        24_000.0,
        23_990.0,
        30_000.0,
    ] {
        assert_eq!(
            read(&x, n, Window::Hann, f, 10),
            Err(Unreadable::OutOfBand),
            "{f}"
        );
    }
    assert_eq!(
        read(&vec![0.0; n], n, Window::Hann, 1000.0, 10),
        Err(Unreadable::NoFundamental)
    );
}

#[test]
fn snap_to_bin_lands_on_the_nearest_interior_bin() {
    for (fs, n) in [(48_000.0, 4096usize), (44_100.0, 1 << 20), (96_000.0, 1001)] {
        let bin = fs / n as f64;
        let last = ((n - 1) / 2) as f64;
        for f in [1.0, 20.0, 997.0, 1000.0, 10_000.3, 0.45 * fs] {
            let s = snap_to_bin(f, fs, n);
            let k = s / bin;
            assert!((k - k.round()).abs() < 1e-6, "{f} at {fs}/{n}: bin {k}");
            assert!(k.round() >= 1.0 && k.round() <= last);
            if f > bin && f < last * bin {
                assert!((s - f).abs() <= bin / 2.0 * (1.0 + 1e-12), "{f} -> {s}");
            }
            assert!(is_on_bin(s, fs, n), "{f} at {fs}/{n}");
            assert_eq!(snap_to_bin(s, fs, n), s, "snapping twice");
        }
        // Never DC, never fs/2.
        assert_eq!(snap_to_bin(0.0, fs, n), bin);
        assert_eq!(snap_to_bin(-5.0, fs, n), bin);
        assert_eq!(snap_to_bin(fs, fs, n), last * fs / n as f64);
        assert!(snap_to_bin(fs / 2.0, fs, n) < fs / 2.0);
        assert!(!is_on_bin(1.5 * bin, fs, n));
        assert!(!is_on_bin((100.0 + 1e-6) * bin, fs, n));
    }
}
