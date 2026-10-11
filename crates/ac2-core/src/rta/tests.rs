use super::class1::{CLASS1_OCTAVE, omega_fractional};
use super::half_band::{HALF_BAND_PASS, half_band_response};
use super::*;
use crate::grid::iec_octave_ratio;
use ac2_testkit::golden::GoldenSet;
use std::f64::consts::{PI, TAU};

const RATES: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];
const FRACTIONS: [BandFraction; 5] = [
    BandFraction::Octave,
    BandFraction::Third,
    BandFraction::Sixth,
    BandFraction::Twelfth,
    BandFraction::TwentyFourth,
];

#[test]
fn class1_mask_breakpoints() {
    let g = iec_octave_ratio();
    assert_eq!(class1_limits(1, 1.0), (-0.4, 0.4));
    let (lo, hi) = class1_limits(1, g.powf(0.375));
    assert!((lo + 0.4).abs() < 1e-12 && (hi - 1.4).abs() < 1e-9);
    let (lo, _) = class1_limits(1, g.powf(-1.0));
    assert!((lo - 16.6).abs() < 1e-9);
    assert_eq!(class1_limits(1, g.powf(5.0)), (70.0, f64::INFINITY));
    // Just outside the edge the stop-band minimum applies.
    let (lo, hi) = class1_limits(3, g.powf(1.0 / 6.0) * 1.0001);
    assert!(lo > 1.19 && hi.is_infinite());
    // Formula 9 for the one-third-octave image of the octave breakpoint Ω = G.
    let om = omega_fractional(3, g);
    let expect = 1.0 + (g.powf(1.0 / 6.0) - 1.0) / (g.sqrt() - 1.0) * (g - 1.0);
    assert!((om - expect).abs() < 1e-15);
}

/// Every band of every fraction at every rate meets the class 1 relative attenuation
/// limits at the Table 1 breakpoints (mapped by Formulas 9/10) and the edges are −3 dB.
#[test]
fn all_bands_meet_class1_at_breakpoints() {
    let g = iec_octave_ratio();
    for fs in RATES {
        for fr in FRACTIONS {
            let b = fr.b();
            let bank = OctaveFilterBank::new(fr, fs, 19.0, 21_000.0).expect("bank");
            let mut raised = 0;
            for (i, info) in bank.bands().enumerate() {
                assert!(info.meets_class1, "{fs} 1/{b} {:.1}", info.centre_hz);
                if info.order > BASE_ORDER {
                    raised += 1;
                }
                for (e, _, _) in CLASS1_OCTAVE {
                    for side in [1.0, -1.0] {
                        let oct = g.powf(e);
                        let om = if b == 1 {
                            oct
                        } else {
                            omega_fractional(b, oct)
                        };
                        let om = if side < 0.0 { 1.0 / om } else { om };
                        let f = info.centre_hz * om;
                        if f >= fs / 2.0 {
                            continue;
                        }
                        let att = -20.0 * bank.response(i, f).norm().log10();
                        // The band edge itself is checked below (−3 dB by design).
                        let (lo, hi) = class1_limits(b, om);
                        if (e - 0.5).abs() > 1e-12 {
                            assert!(
                                att >= lo && att <= hi,
                                "{fs} 1/{b} {:.1} Hz Ω={om:.4}: {att:.3} not in [{lo}, {hi}]",
                                info.centre_hz
                            );
                        }
                    }
                }
                for edge in [info.lower_hz, info.upper_hz] {
                    let att = -20.0 * bank.response(i, edge).norm().log10();
                    assert!((att - 3.0103).abs() < 1e-3, "edge {edge}: {att}");
                }
            }
            eprintln!(
                "{fs} Hz 1/{b}: {} bands, {raised} with raised order",
                bank.len()
            );
        }
    }
}

#[test]
fn bands_above_nyquist_are_not_built() {
    let bank = OctaveFilterBank::new(BandFraction::Third, 44_100.0, 19.0, 21_000.0).expect("bank");
    let last = bank.band(bank.len() - 1);
    assert!(last.upper_hz < 22_050.0);
    assert!(
        (last.centre_hz - 15_848.9).abs() < 1.0,
        "{}",
        last.centre_hz
    );
    let bank48 =
        OctaveFilterBank::new(BandFraction::Third, 48_000.0, 19.0, 21_000.0).expect("bank");
    assert_eq!(bank48.len(), 31);
    assert!(matches!(
        OctaveFilterBank::new(BandFraction::Octave, 8000.0, 5000.0, 8000.0),
        Err(RtaError::NoBands)
    ));
}

/// Cross-check of the band-pass design against scipy.signal.butter (bilinear, pre-warped
/// edges, same order) in the `rta_butterworth_bands` golden set.
#[test]
fn design_matches_scipy_butter() {
    let g = GoldenSet::load("rta_butterworth_bands").expect("golden");
    let spec = g.f64("band_spec").expect("spec");
    for (i, row) in spec.chunks(5).enumerate() {
        let [fs, _b, lower, upper, order] = [row[0], row[1], row[2], row[3], row[4]];
        let sos = butterworth_bandpass(lower, upper, fs, order as usize);
        let f = g.f64(&format!("band{i}_freq_hz")).expect("freq");
        let db: Vec<f64> = f
            .iter()
            .map(|&f| 20.0 * cascade_response(&sos, f, fs).norm().log10())
            .collect();
        g.assert_f64(&format!("band{i}_mag_db"), &db);
    }
}

/// Narrowest band at the highest rate, designed at the full rate: the impulse response
/// energy (white-noise power gain) of the running filter equals the integral of the
/// designed |H|², so f64 is numerically sound even for the highest-Q sections.
#[test]
fn low_band_numerics_full_rate() {
    let fs = 96_000.0;
    let fr = BandFraction::TwentyFourth;
    let fm = fr.centres(19.0, 21.0)[0];
    let (lo, hi) = fr.edges(fm);
    let mut sos = butterworth_bandpass(lo, hi, fs, 3);
    let n = (fs * 40.0) as usize;
    let mut energy = 0.0;
    for i in 0..n {
        let mut y = if i == 0 { 1.0 } else { 0.0 };
        for s in &mut sos {
            y = s.process_sample(y);
        }
        energy += y * y;
    }
    // Σh² = (2/fs)∫₀^{fs/2} |H(f)|² df; integrate on a fine grid around the band.
    let df = 1e-4;
    let (a, b) = (fm / 4.0, fm * 4.0);
    let steps = ((b - a) / df) as usize;
    let integral: f64 = (0..steps)
        .map(|k| {
            let f = a + (k as f64 + 0.5) * df;
            cascade_response(&sos, f, fs).norm_sqr()
        })
        .sum::<f64>()
        * df;
    let expected = 2.0 * integral / fs;
    let rel = (energy / expected - 1.0).abs();
    assert!(
        rel < 1e-6,
        "energy {energy:e} vs {expected:e} (rel {rel:e})"
    );
    // Noise bandwidth of an order-3 Butterworth band-pass is (π/6)/sin(π/6) · B.
    let nbw = integral / (hi - lo);
    assert!((nbw - (PI / 6.0) / (PI / 6.0).sin()).abs() < 1e-3, "{nbw}");
}

/// A steady sine at a band centre reads its mean square in that band and is attenuated
/// in the neighbours by at least the class 1 stop-band minimum.
#[test]
fn sine_band_power() {
    let fs = 48_000.0;
    let mut bank = OctaveFilterBank::new(BandFraction::Third, fs, 100.0, 10_000.0).expect("bank");
    let k = bank
        .bands()
        .position(|b| (b.centre_hz - 1000.0).abs() < 1e-6)
        .expect("1 kHz band");
    let amp = 0.5;
    let n = fs as usize;
    let x: Vec<f64> = (0..n)
        .map(|i| amp * (TAU * 1000.0 * i as f64 / fs).sin())
        .collect();
    bank.process(&x);
    bank.reset_powers();
    bank.process(&x);
    let mut p = vec![0.0; bank.len()];
    bank.band_powers(&mut p);
    let ms = amp * amp / 2.0;
    assert!((p[k] / ms - 1.0).abs() < 1e-3, "{} vs {ms}", p[k]);
    // Neighbour two bands away: Ω = G^(2/3) → beyond G^(1/2) octave-equivalent.
    let att2 = 10.0 * (ms / p[k + 2]).log10();
    assert!(att2 > 16.6, "{att2}");
}

/// Deterministic white noise in ±0.5 (xorshift).
fn noise(n: usize, seed: u64) -> Vec<f64> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        })
        .collect()
}

/// The bank without decimation: per band the lowest order whose full-rate cascade meets
/// class 1, each band run sample by sample through unit-gain sections.
fn reference_bands(fr: BandFraction, fs: f64, f_lo: f64, f_hi: f64) -> Vec<Vec<Biquad>> {
    fr.centres(f_lo, f_hi)
        .into_iter()
        .filter_map(|fm| {
            let (lo, hi) = fr.edges(fm);
            if hi >= fs / 2.0 {
                return None;
            }
            let mut sos = Vec::new();
            for order in BASE_ORDER..=MAX_ORDER {
                sos = butterworth_bandpass(lo, hi, fs, order);
                let mag = |f| cascade_response(&sos, f, fs).norm();
                if meets_class1(mag, fm, fr.b(), fs / 2.0) {
                    break;
                }
            }
            Some(sos)
        })
        .collect()
}

/// Mean square per band of `x[warm..]` after running `x[..warm]`.
fn reference_powers(bands: &mut [Vec<Biquad>], x: &[f64], warm: usize) -> Vec<f64> {
    bands
        .iter_mut()
        .map(|sos| {
            let mut e = 0.0;
            for (i, &v) in x.iter().enumerate() {
                let mut y = v;
                for s in sos.iter_mut() {
                    y = s.process_sample(y);
                }
                if i >= warm {
                    e += y * y;
                }
            }
            e / (x.len() - warm) as f64
        })
        .collect()
}

/// White-noise power gain (2/fs)∫|H|² df over fm/16..min(16 fm, fs/2) on a log grid.
fn power_gain(h: impl Fn(f64) -> Complex64, fm: f64, fs: f64) -> f64 {
    let (a, b) = (fm / 16.0, (fm * 16.0).min(fs / 2.0));
    let n = 20_000;
    let r = (b / a).ln();
    (0..n)
        .map(|k| {
            let f = a * (r * (k as f64 + 0.5) / n as f64).exp();
            h(f).norm_sqr() * f * r / n as f64
        })
        .sum::<f64>()
        * 2.0
        / fs
}

fn bank_powers(bank: &mut OctaveFilterBank, x: &[f64], warm: usize, block: usize) -> Vec<f64> {
    bank.reset();
    for c in x[..warm].chunks(block) {
        bank.process(c);
    }
    bank.reset_powers();
    for c in x[warm..].chunks(block) {
        bank.process(c);
    }
    let mut p = vec![0.0; bank.len()];
    bank.band_powers(&mut p);
    p
}

/// The half-band design: pass band flat, stop band at least the guaranteed attenuation,
/// and power-complementary (|H(f)|² + |H(fs/2 − f)|² = 1), which the two-branch all-pass
/// structure guarantees whatever the coefficients and so checks the branch wiring.
#[test]
fn half_band_design() {
    let c = half_band_coefs();
    let n = 20_000;
    let mut worst_pass = 0.0f64;
    let mut worst_stop = f64::NEG_INFINITY;
    for i in 0..=n {
        let f = 0.5 * f64::from(i) / f64::from(n);
        let h = half_band_response(&c, f, 1.0);
        let db = 20.0 * h.norm().log10();
        if f <= HALF_BAND_PASS {
            worst_pass = worst_pass.max(db.abs());
        }
        if f >= 0.5 - HALF_BAND_PASS {
            worst_stop = worst_stop.max(db);
        }
        let mirror = half_band_response(&c, 0.5 - f, 1.0).norm_sqr();
        assert!((h.norm_sqr() + mirror - 1.0).abs() < 1e-12, "{f}");
    }
    assert!(worst_pass < 1e-10, "pass-band deviation {worst_pass:e} dB");
    assert!(worst_stop < -HALF_BAND_STOP_DB, "stop band {worst_stop} dB");
}

/// The streaming decimator realises the designed response: a pass-band sine keeps its
/// mean square, a stop-band one comes out at least the stop-band attenuation down, and
/// splitting the input into odd-sized blocks changes nothing.
#[test]
fn half_band_streaming() {
    let c = half_band_coefs();
    let n = 1 << 16;
    for (f, pass) in [(0.1, true), (0.19, true), (0.31, false), (0.45, false)] {
        let x: Vec<f64> = (0..n).map(|i| (TAU * f * i as f64).sin()).collect();
        let mut whole = Vec::new();
        HalfBand::new(c).process(&x, &mut whole);
        let mut split = Vec::new();
        let mut hb = HalfBand::new(c);
        for chunk in x.chunks(37) {
            hb.process(chunk, &mut split);
        }
        assert_eq!(whole, split);
        assert_eq!(whole.len(), n / 2);
        let tail = &whole[n / 4..];
        let ms = tail.iter().map(|v| v * v).sum::<f64>() / tail.len() as f64;
        let db = 10.0 * (ms / 0.5).log10();
        let model = 20.0 * half_band_response(&c, f, 1.0).norm().log10();
        if pass {
            // The window holds a non-integer number of periods: ~10⁻⁴ dB of scatter.
            assert!(db.abs() < 1e-3 && model.abs() < 1e-10, "{f}: {db} dB");
        } else {
            assert!(
                db < -HALF_BAND_STOP_DB && model < -HALF_BAND_STOP_DB,
                "{f}: {db} dB"
            );
        }
    }
}

/// The measured band power of a steady sine equals ½|H|² from `response`, for bands at
/// decimated rates and at frequencies in the band, on the skirts and in the region that
/// aliases at the band's rate.
#[test]
fn decimated_bands_match_their_response() {
    let fs = 48_000.0;
    let mut bank = OctaveFilterBank::new(BandFraction::Third, fs, 20.0, 20_000.0).expect("bank");
    let picks: Vec<usize> = (0..bank.len())
        .filter(|&i| [4, 16].contains(&bank.band(i).decimation))
        .take(2)
        .chain(
            (0..bank.len())
                .filter(|&i| bank.band(i).decimation >= 64)
                .take(1),
        )
        .collect();
    assert_eq!(picks.len(), 3);
    for i in picks {
        let info = bank.band(i);
        let fs_k = fs / f64::from(info.decimation);
        let freqs = [
            info.centre_hz,
            info.upper_hz,
            info.lower_hz * 0.8,
            info.upper_hz * 1.5,
            0.45 * fs_k,
            0.55 * fs_k,
            fs_k - info.centre_hz,
        ];
        for f in freqs {
            let secs = (200.0 / (info.upper_hz - info.lower_hz)).max(2.0);
            let n = (fs * secs) as usize;
            let x: Vec<f64> = (0..n).map(|k| (TAU * f * k as f64 / fs).sin()).collect();
            let p = bank_powers(&mut bank, &x, n / 2, 1000)[i];
            let model = 0.5 * bank.response(i, f).norm_sqr();
            let (db, mdb) = (10.0 * p.log10(), 10.0 * model.log10());
            // Deep in the stop band the reading is the decaying start-up transient and
            // rounding, so only its being that far down is asserted.
            if mdb > -80.0 {
                assert!(
                    (db - mdb).abs() < 0.01,
                    "{:.1} Hz band, {f:.1} Hz: {db:.4} vs {mdb:.4} dB",
                    info.centre_hz
                );
            } else {
                assert!(
                    db < -77.0,
                    "{:.1} Hz band, {f:.1} Hz: {db:.2} dB",
                    info.centre_hz
                );
            }
        }
    }
}

/// The multirate bank against the same bands run at the full rate (the design without
/// decimation): white-noise power gain from the responses, for every fraction and rate,
/// and unit gain at every mid-band frequency.
#[test]
fn slow_noise_and_sine_gain_match_full_rate() {
    let mut worst = 0.0f64;
    for fs in RATES {
        for fr in FRACTIONS {
            let bank = OctaveFilterBank::new(fr, fs, 19.0, 21_000.0).expect("bank");
            let refb = reference_bands(fr, fs, 19.0, 21_000.0);
            assert_eq!(refb.len(), bank.len());
            for (i, sos) in refb.iter().enumerate() {
                let fm = bank.band(i).centre_hz;
                let gn = power_gain(|f| bank.response(i, f), fm, fs);
                let gr = power_gain(|f| cascade_response(sos, f, fs), fm, fs);
                let d = 10.0 * (gn / gr).log10();
                worst = worst.max(d.abs());
                assert!(d.abs() < 0.006, "{fs} 1/{} {fm:.1} Hz: {d:+.5} dB", fr.b());
                // Unit gain at f_m, or at the digital image of the analog centre for
                // bands whose order is raised near Nyquist (at the full rate either way).
                let centre = bank.response(i, fm).norm();
                let expect = cascade_response(sos, fm, fs).norm();
                assert!((centre - expect).abs() < 1e-9, "{fs} 1/{} {fm:.1}", fr.b());
                if bank.band(i).decimation > 1 {
                    assert!((centre - 1.0).abs() < 1e-9, "{fs} 1/{} {fm:.1}", fr.b());
                }
            }
        }
    }
    eprintln!("worst noise power gain difference {worst:.5} dB");
}

/// Streaming band levels of white noise against the full-rate bank. The two filter sets
/// differ in phase and delay, so their readings of one finite noise record differ by
/// estimation scatter as well as by the (smaller) difference in power gain.
#[test]
fn noise_levels_match_full_rate() {
    let fs = 48_000.0;
    for (fr, secs, tol) in [
        (BandFraction::Third, 10.0, 0.02),
        (BandFraction::TwentyFourth, 20.0, 0.05),
    ] {
        let mut bank = OctaveFilterBank::new(fr, fs, 20.0, 20_000.0).expect("bank");
        let mut refb = reference_bands(fr, fs, 20.0, 20_000.0);
        let warm = (fs * 4.0) as usize;
        let x = noise(warm + (fs * secs) as usize, 7);
        let pn = bank_powers(&mut bank, &x, warm, 1024);
        let pr = reference_powers(&mut refb, &x, warm);
        let worst = pn
            .iter()
            .zip(&pr)
            .map(|(a, b)| 10.0 * (a / b).log10())
            .fold(0.0f64, |w, d| if d.abs() > w.abs() { d } else { w });
        eprintln!("1/{}: worst noise level difference {worst:+.4} dB", fr.b());
        assert!(worst.abs() < tol, "1/{}: {worst} dB", fr.b());
    }
}

/// Decimation happens, and never makes a band slower: every band's mid-band group delay
/// (decimators included) is within the slack of the full-rate design's.
#[test]
fn decimation_keeps_latency() {
    for fs in RATES {
        for fr in FRACTIONS {
            let bank = OctaveFilterBank::new(fr, fs, 19.0, 21_000.0).expect("bank");
            for (i, info) in bank.bands().enumerate().filter(|(_, b)| b.decimation > 1) {
                let full = butterworth_bandpass(info.lower_hz, info.upper_hz, fs, BASE_ORDER);
                let reference = group_delay(|f| cascade_response(&full, f, fs), info.centre_hz);
                let delay = group_delay(|f| bank.response(i, f), info.centre_hz);
                assert!(
                    delay <= (1.0 + LATENCY_SLACK) * reference * (1.0 + 1e-9),
                    "{fs} 1/{} {:.1}: {delay} vs {reference}",
                    fr.b(),
                    info.centre_hz
                );
            }
            let full_rate = bank.bands().filter(|b| b.decimation == 1).count();
            eprintln!(
                "{fs} 1/{}: {full_rate} of {} bands at full rate",
                fr.b(),
                bank.len()
            );
            if fr.b() >= 3 {
                assert!(full_rate * 2 < bank.len(), "{fs} 1/{}", fr.b());
            }
        }
    }
}

/// Readings do not depend on how the input is split into blocks.
#[test]
fn block_size_independent() {
    let fs = 48_000.0;
    let x = noise(48_000, 3);
    let mut bank = OctaveFilterBank::new(BandFraction::Sixth, fs, 20.0, 20_000.0).expect("bank");
    let whole = bank_powers(&mut bank, &x, 0, x.len());
    for block in [1, 3, 256, 1000] {
        assert_eq!(bank_powers(&mut bank, &x, 0, block), whole, "block {block}");
    }
}

/// A band whose rate produced no output in an interval repeats its previous reading; right
/// after a reset it has none to repeat.
#[test]
fn short_interval_repeats_previous_power() {
    let fs = 48_000.0;
    let mut bank = OctaveFilterBank::new(BandFraction::Third, fs, 20.0, 20_000.0).expect("bank");
    let deep = (0..bank.len())
        .max_by_key(|&i| bank.band(i).decimation)
        .expect("band");
    let d = bank.band(deep).decimation as usize;
    assert!(d >= 64, "{d}");
    let x = noise(fs as usize, 5);
    let mut p = vec![0.0; bank.len()];
    bank.process(&x[..d / 2]);
    bank.band_powers(&mut p);
    assert!(p[deep].is_nan() && p[bank.len() - 1].is_finite());
    bank.process(&x[d / 2..]);
    bank.band_powers(&mut p);
    let before = p[deep];
    assert!(before > 0.0);
    bank.reset_powers();
    bank.process(&x[..d / 4]);
    bank.band_powers(&mut p);
    assert_eq!(p[deep], before);
}
