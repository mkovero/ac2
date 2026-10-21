use super::*;

const ANALOG: Realisation = Realisation::Analog;

fn h(s: Section, real: Realisation, f: f64) -> Complex64 {
    response(&[s], real, &[f]).expect("designs")[0]
}

fn db(s: Section, real: Realisation, f: f64) -> f64 {
    20.0 * h(s, real, f).norm().log10()
}

fn deg(s: Section, real: Realisation, f: f64) -> f64 {
    h(s, real, f).arg().to_degrees()
}

/// Bisection for the frequency in (lo, hi) where `g(f)` crosses `target` (monotonic there).
fn crossing(g: impl Fn(f64) -> f64, target: f64, lo: f64, hi: f64) -> f64 {
    let (mut a, mut b) = (lo.ln(), hi.ln());
    let up = g(hi) > g(lo);
    for _ in 0..200 {
        let m = 0.5 * (a + b);
        if (g(m.exp()) > target) == up {
            b = m;
        } else {
            a = m;
        }
    }
    (0.5 * (a + b)).exp()
}

fn peaking(f: f64, gain_db: f64, q: f64, q_measured: PeakQ) -> Section {
    Section::Peaking {
        f,
        gain_db,
        bw: Bandwidth::Q(q),
        q_measured,
    }
}

#[test]
fn peaking_half_gain_bandwidth_is_f0_over_q_at_every_gain() {
    for g in [3.0, -3.0, 12.0, -12.0] {
        let q = 2.0;
        let s = peaking(1000.0, g, q, PeakQ::HalfGain);
        assert!((db(s, ANALOG, 1000.0) - g).abs() < 1e-9, "peak {g}");
        let lo = crossing(|f| db(s, ANALOG, f), g / 2.0, 100.0, 1000.0);
        let hi = crossing(|f| db(s, ANALOG, f), g / 2.0, 1000.0, 10_000.0);
        assert!(((hi - lo) - 1000.0 / q).abs() < 1e-6, "{g}: {lo} {hi}");
        assert!((lo * hi - 1e6).abs() < 1e-3, "geometric about f0");
        // A decade away the section is flat.
        assert!(db(s, ANALOG, 20.0).abs() < 0.05 && db(s, ANALOG, 50_000.0).abs() < 0.05);
    }
}

#[test]
fn peaking_minus_3db_reading_puts_q3_between_the_points_3db_from_peak() {
    for g in [6.0, -6.0, 15.0, -15.0] {
        let q3 = 1.5;
        let s = peaking(1000.0, g, q3, PeakQ::Minus3dbFromPeak);
        assert!((db(s, ANALOG, 1000.0) - g).abs() < 1e-9);
        let target = g - (10.0 * 2f64.log10()).copysign(g);
        let lo = crossing(|f| db(s, ANALOG, f), target, 100.0, 1000.0);
        let hi = crossing(|f| db(s, ANALOG, f), target, 1000.0, 10_000.0);
        assert!(((hi - lo) - 1000.0 / q3).abs() < 1e-6, "{g}: {lo} {hi}");
    }
    for g in [3.0, -3.0] {
        let r = design(&peaking(1000.0, g, 1.0, PeakQ::Minus3dbFromPeak), ANALOG);
        assert_eq!(r, Err(Refusal::PeakTooSmallFor3db));
    }
}

#[test]
fn minus_3db_q_round_trips_through_the_cookbook_q() {
    for g in [4.0, -9.0, 24.0] {
        let qc = peak_cookbook_q(2.0, g, PeakQ::Minus3dbFromPeak)
            .expect("defined")
            .expect("a cookbook Q");
        assert!((peak_minus3db_q(qc, g).expect("defined") - 2.0).abs() < 1e-12);
    }
    assert_eq!(peak_cookbook_q(2.0, 6.0, PeakQ::BandPass), Ok(None));
}

#[test]
fn band_pass_family_half_gain_band_widens_as_root_k() {
    for g in [6.0, 18.0, -12.0] {
        let q = 2.0;
        let s = peaking(1000.0, g, q, PeakQ::BandPass);
        assert!((db(s, ANALOG, 1000.0) - g).abs() < 1e-9);
        let k = 10f64.powf(g.abs() / 20.0);
        let lo = crossing(|f| db(s, ANALOG, f), g / 2.0, 50.0, 1000.0);
        let hi = crossing(|f| db(s, ANALOG, f), g / 2.0, 1000.0, 20_000.0);
        assert!(((hi - lo) - 1000.0 * k.sqrt() / q).abs() < 1e-6, "{g}");
    }
}

#[test]
fn boost_times_equal_cut_is_flat() {
    for m in [PeakQ::HalfGain, PeakQ::Minus3dbFromPeak, PeakQ::BandPass] {
        let up = peaking(800.0, 9.0, 1.4, m);
        let down = peaking(800.0, -9.0, 1.4, m);
        let fs: Vec<f64> = (0..40).map(|i| 20.0 * 1.2f64.powi(i)).collect();
        let r = response(&[up, down], ANALOG, &fs).expect("designs");
        for (f, v) in fs.iter().zip(&r) {
            assert!(
                (v - Complex64::new(1.0, 0.0)).norm() < 1e-12,
                "{m:?} at {f}"
            );
        }
    }
}

fn hp(alignment: Alignment, order: u8) -> Section {
    Section::Pass {
        pass: Pass::High,
        f: 100.0,
        alignment,
        order,
    }
}

fn lp(alignment: Alignment, order: u8) -> Section {
    Section::Pass {
        pass: Pass::Low,
        f: 100.0,
        alignment,
        order,
    }
}

#[test]
fn butterworth_is_3db_down_at_f_with_n_times_6db_per_octave() {
    for n in 1..=8u8 {
        for s in [hp(Alignment::Butterworth, n), lp(Alignment::Butterworth, n)] {
            assert!(
                (db(s, ANALOG, 100.0) + 10.0 * 2f64.log10()).abs() < 1e-9,
                "{n}"
            );
        }
        let lo = db(hp(Alignment::Butterworth, n), ANALOG, 1.0);
        let lo2 = db(hp(Alignment::Butterworth, n), ANALOG, 2.0);
        assert!((lo2 - lo - 6.0206 * n as f64).abs() < 0.01, "{n}");
        let hi = db(lp(Alignment::Butterworth, n), ANALOG, 10_000.0);
        let hi2 = db(lp(Alignment::Butterworth, n), ANALOG, 20_000.0);
        assert!((hi - hi2 - 6.0206 * n as f64).abs() < 0.01, "{n}");
    }
}

#[test]
fn linkwitz_riley_is_6db_down_and_lr4_sums_flat_all_pass() {
    for n in [2u8, 4, 6, 8] {
        assert!((db(hp(Alignment::LinkwitzRiley, n), ANALOG, 100.0) + 6.0206).abs() < 1e-3);
        assert!((db(lp(Alignment::LinkwitzRiley, n), ANALOG, 100.0) + 6.0206).abs() < 1e-3);
    }
    for f in [10.0, 50.0, 100.0, 200.0, 1000.0] {
        let sum = h(hp(Alignment::LinkwitzRiley, 4), ANALOG, f)
            + h(lp(Alignment::LinkwitzRiley, 4), ANALOG, f);
        assert!((sum.norm() - 1.0).abs() < 1e-12, "flat at {f}");
        // The sum is the second-order all-pass of the Butterworth Q.
        let ap = h(
            Section::AllPass2 {
                f: 100.0,
                q: std::f64::consts::FRAC_1_SQRT_2,
            },
            ANALOG,
            f,
        );
        assert!((sum - ap).norm() < 1e-12, "all-pass at {f}");
    }
    for n in [1u8, 3, 5, 7, 9] {
        assert_eq!(
            design(&hp(Alignment::LinkwitzRiley, n), ANALOG),
            Err(Refusal::Order {
                order: n,
                alignment: Alignment::LinkwitzRiley
            })
        );
    }
    assert!(design(&hp(Alignment::Butterworth, 9), ANALOG).is_err());
}

#[test]
fn bessel_norms() {
    for n in 1..=8u8 {
        // −3 dB at f.
        let s = lp(Alignment::Bessel(BesselNorm::Minus3db), n);
        assert!(
            (db(s, ANALOG, 100.0) + 10.0 * 2f64.log10()).abs() < 1e-9,
            "{n}"
        );
        // Unit group delay at DC: f = 1/(2πτ), so τ = 1/(2π·100 Hz).
        let s = lp(Alignment::Bessel(BesselNorm::Delay), n);
        let df = 1e-3;
        let gd = -(h(s, ANALOG, df).arg() - h(s, ANALOG, 0.0).arg()) / (2.0 * PI * df);
        assert!((gd - 1.0 / (2.0 * PI * 100.0)).abs() < 1e-9, "{n}: {gd}");
        // Phase-normalised: far above f the LP's phase approaches the Butterworth's like
        // its asymptote (−n·90°), the same high-frequency magnitude as the Butterworth too.
        let bes = lp(Alignment::Bessel(BesselNorm::Phase), n);
        let but = lp(Alignment::Butterworth, n);
        let far = 1e6;
        assert!(
            (db(bes, ANALOG, far) - db(but, ANALOG, far)).abs() < 1e-3,
            "{n}"
        );
    }
}

#[test]
fn all_passes_are_flat_with_their_phase_at_f() {
    for f in [10.0, 100.0, 1000.0, 1e4] {
        assert!((h(Section::AllPass1 { f: 1000.0 }, ANALOG, f).norm() - 1.0).abs() < 1e-12);
        let ap2 = Section::AllPass2 { f: 1000.0, q: 0.9 };
        assert!((h(ap2, ANALOG, f).norm() - 1.0).abs() < 1e-12);
    }
    assert!((deg(Section::AllPass1 { f: 1000.0 }, ANALOG, 1000.0) + 90.0).abs() < 1e-9);
    assert!((deg(Section::AllPass2 { f: 1000.0, q: 0.9 }, ANALOG, 1000.0).abs() - 180.0) < 1e-9);
    // Unwrapped, the second-order all-pass goes 0 → −360° across the band.
    let fs: Vec<f64> = (0..200).map(|i| 1.0 * 1.1f64.powi(i)).collect();
    let r = response(&[Section::AllPass2 { f: 1000.0, q: 0.9 }], ANALOG, &fs).expect("ok");
    let mut ph = 0.0;
    let mut prev = r[0].arg();
    for v in &r[1..] {
        let mut d = v.arg() - prev;
        while d > PI {
            d -= 2.0 * PI;
        }
        while d < -PI {
            d += 2.0 * PI;
        }
        ph += d;
        prev = v.arg();
    }
    assert!((ph.to_degrees() + 360.0).abs() < 1.0, "{}", ph.to_degrees());
}

fn shelf(pass: Pass, gain_db: f64, order: ShelfOrder, f_at: ShelfFreq) -> Section {
    Section::Shelf {
        pass,
        f: 200.0,
        gain_db,
        order,
        f_at,
    }
}

#[test]
fn shelves_are_half_gain_at_the_midpoint_and_s1_is_monotonic() {
    for pass in [Pass::Low, Pass::High] {
        for g in [6.0, -10.0] {
            for order in [ShelfOrder::First, ShelfOrder::Second(ShelfSlope::S(1.0))] {
                let s = shelf(pass, g, order, ShelfFreq::Midpoint);
                assert!(
                    (db(s, ANALOG, 200.0) - g / 2.0).abs() < 1e-9,
                    "{pass:?} {g}"
                );
                let (shelf_side, flat_side) = match pass {
                    Pass::Low => (0.2, 200_000.0),
                    Pass::High => (200_000.0, 0.2),
                };
                assert!((db(s, ANALOG, shelf_side) - g).abs() < 0.01);
                assert!(db(s, ANALOG, flat_side).abs() < 0.01);
                let mut prev = db(s, ANALOG, 1.0);
                for i in 1..100 {
                    let v = db(s, ANALOG, 1.0 * 1.12f64.powi(i));
                    let step = (v - prev) * g.signum();
                    let ok = match pass {
                        Pass::Low => step <= 1e-12,
                        Pass::High => step >= -1e-12,
                    };
                    assert!(ok, "monotonic {pass:?} {g} {order:?}");
                    prev = v;
                }
            }
        }
    }
}

#[test]
fn shelf_corner_is_3db_from_flat_and_refused_below_3db() {
    for pass in [Pass::Low, Pass::High] {
        for g in [9.0, -12.0, 4.0] {
            let s = shelf(
                pass,
                g,
                ShelfOrder::Second(ShelfSlope::S(1.0)),
                ShelfFreq::Corner,
            );
            assert!(
                (db(s, ANALOG, 200.0) - CORNER_DB * g.signum()).abs() < 1e-9,
                "{g}"
            );
        }
        let s = shelf(pass, 2.5, ShelfOrder::First, ShelfFreq::Corner);
        assert_eq!(design(&s, ANALOG), Err(Refusal::ShelfTooSmallForCorner));
    }
}

#[test]
fn bandwidth_and_slope_conversions_round_trip() {
    for q in [0.3, 0.7, 1.41, 4.0, 30.0] {
        assert!((q_from_octaves(octaves_from_q(q)) - q).abs() < 1e-12 * q);
    }
    assert!((octaves_from_q(std::f64::consts::SQRT_2) - 1.0).abs() < 1e-6);
    for g in [3.0, -6.0, 15.0] {
        for s in [0.3, 0.8, 1.0] {
            let q = shelf_q_from_s(s, g).expect("in range");
            assert!((shelf_s_from_q(q, g) - s).abs() < 1e-12);
        }
    }
    // S = 1 is the Butterworth-like Q.
    assert!(
        (shelf_q_from_s(1.0, 6.0).expect("ok") - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12
    );
    // An octave entry designs the same filter as its Q.
    let a = peaking(1000.0, 5.0, q_from_octaves(0.7), PeakQ::HalfGain);
    let b = Section::Peaking {
        f: 1000.0,
        gain_db: 5.0,
        bw: Bandwidth::Octaves(0.7),
        q_measured: PeakQ::HalfGain,
    };
    assert_eq!(h(a, ANALOG, 1300.0), h(b, ANALOG, 1300.0));
}

/// Cookbook biquad coefficients written out, independently of the prototype path.
fn cookbook_peak(f0: f64, g: f64, q: f64, fs: f64) -> Section {
    let a = 10f64.powf(g / 40.0);
    let w0 = 2.0 * PI * f0 / fs;
    let al = w0.sin() / (2.0 * q);
    let a0 = 1.0 + al / a;
    Section::Biquad {
        b: [
            (1.0 + al * a) / a0,
            -2.0 * w0.cos() / a0,
            (1.0 - al * a) / a0,
        ],
        a: [-2.0 * w0.cos() / a0, (1.0 - al / a) / a0],
        rate: fs,
    }
}

#[test]
fn digital_matches_the_cookbook_and_the_analog_below_a_tenth_of_fs() {
    let fs = 48_000.0;
    let real = Realisation::Digital { rate: fs };
    for (f0, g, q) in [
        (1000.0, -6.0, 2.0),
        (3000.0, 12.0, 0.7),
        (16_000.0, 6.0, 1.0),
    ] {
        let s = peaking(f0, g, q, PeakQ::HalfGain);
        let cb = cookbook_peak(f0, g, q, fs);
        for f in [20.0, 200.0, 1000.0, 5000.0, 15_000.0, 23_000.0] {
            assert!(
                (h(s, real, f) - h(cb, ANALOG, f)).norm() < 1e-9,
                "{f0} at {f}"
            );
        }
    }
    let sections = [
        peaking(1000.0, -6.0, 2.0, PeakQ::HalfGain),
        hp(Alignment::LinkwitzRiley, 4),
        shelf(
            Pass::High,
            4.0,
            ShelfOrder::Second(ShelfSlope::S(0.8)),
            ShelfFreq::Midpoint,
        ),
        Section::Notch {
            f: 2000.0,
            bw: Bandwidth::Q(5.0),
        },
    ];
    for s in sections {
        for f in [20.0, 100.0, 500.0, 1000.0, 2500.0, 4800.0] {
            let d = (db(s, real, f) - db(s, ANALOG, f)).abs();
            assert!(d < 0.05, "{s:?} at {f}: {d}");
        }
    }
    // The digital peak at 16 kHz is cramped: narrower above f0 than the analog one.
    let s = peaking(16_000.0, 6.0, 1.0, PeakQ::HalfGain);
    assert!(db(s, real, 20_000.0) < db(s, ANALOG, 20_000.0) - 0.5);
}

#[test]
fn columns_at_or_above_half_the_rate_are_gaps() {
    let real = Realisation::Digital { rate: 48_000.0 };
    let r = response(
        &[Section::Gain { gain_db: 3.0 }],
        real,
        &[1000.0, 24_000.0, 30_000.0],
    )
    .expect("ok");
    assert!((r[0].norm() - 10f64.powf(3.0 / 20.0)).abs() < 1e-12);
    assert!(r[1].re.is_nan() && r[2].re.is_nan());
    assert_eq!(
        design(&peaking(24_000.0, 3.0, 1.0, PeakQ::HalfGain), real),
        Err(Refusal::FreqAboveNyquist {
            f: 24_000.0,
            nyquist: 24_000.0
        })
    );
}

#[test]
fn biquad_stability_and_ranges_are_refused() {
    let unstable = Section::Biquad {
        b: [1.0, 0.0, 0.0],
        a: [0.0, 1.0],
        rate: 48_000.0,
    };
    assert_eq!(design(&unstable, ANALOG), Err(Refusal::Unstable));
    let unstable = Section::Biquad {
        b: [1.0, 0.0, 0.0],
        a: [-2.0, 0.99],
        rate: 48_000.0,
    };
    assert_eq!(design(&unstable, ANALOG), Err(Refusal::Unstable));
    assert!(design(&cookbook_peak(1000.0, 3.0, 1.0, 48_000.0), ANALOG).is_ok());
    assert_eq!(
        design(&peaking(1000.0, 31.0, 1.0, PeakQ::HalfGain), ANALOG),
        Err(Refusal::GainRange { gain_db: 31.0 })
    );
    assert_eq!(
        design(&peaking(1000.0, 3.0, 101.0, PeakQ::HalfGain), ANALOG),
        Err(Refusal::QRange { q: 101.0 })
    );
    assert_eq!(
        design(&Section::Delay { seconds: -1e-3 }, ANALOG),
        Err(Refusal::DelayRange)
    );
    let err = response(
        &[Section::Polarity, peaking(-1.0, 1.0, 1.0, PeakQ::HalfGain)],
        ANALOG,
        &[100.0],
    )
    .expect_err("refused");
    assert_eq!(err.index, 1);
    assert_eq!(err.to_string(), "section 2: frequency must be positive");
}

#[test]
fn constant_sections_and_delay() {
    let r = response(
        &[
            Section::Polarity,
            Section::Gain { gain_db: -6.0 },
            Section::Delay { seconds: 1e-3 },
        ],
        ANALOG,
        &[250.0],
    )
    .expect("ok");
    // −1 · 10^(−6/20) · e^{−j2π·0.25} = j·0.501
    assert!((r[0] - Complex64::new(0.0, 10f64.powf(-6.0 / 20.0))).norm() < 1e-12);
}

#[test]
fn matches_refgen_eq_sections() {
    use ac2_testkit::GoldenSet;
    let gs = GoldenSet::load("eq_sections").expect("golden set");
    let f = gs.f64("freq_hz").expect("freq");
    let fs = gs.scalar("fs_hz").expect("fs");
    let dig = Realisation::Digital { rate: fs };
    let pass = |pass, f, alignment, order| Section::Pass {
        pass,
        f,
        alignment,
        order,
    };
    let cases: Vec<(&str, Section, Realisation)> = vec![
        (
            "bw_hp4_80_analog",
            pass(Pass::High, 80.0, Alignment::Butterworth, 4),
            ANALOG,
        ),
        (
            "bw_lp3_2k_digital",
            pass(Pass::Low, 2000.0, Alignment::Butterworth, 3),
            dig,
        ),
        (
            "lr4_lp_1k_digital",
            pass(Pass::Low, 1000.0, Alignment::LinkwitzRiley, 4),
            dig,
        ),
        (
            "bessel5_hp_100_phase_analog",
            pass(Pass::High, 100.0, Alignment::Bessel(BesselNorm::Phase), 5),
            ANALOG,
        ),
        (
            "bessel5_hp_100_delay_analog",
            pass(Pass::High, 100.0, Alignment::Bessel(BesselNorm::Delay), 5),
            ANALOG,
        ),
        (
            "bessel5_hp_100_mag_analog",
            pass(
                Pass::High,
                100.0,
                Alignment::Bessel(BesselNorm::Minus3db),
                5,
            ),
            ANALOG,
        ),
        (
            "bessel4_lp_3k_phase_digital",
            pass(Pass::Low, 3000.0, Alignment::Bessel(BesselNorm::Phase), 4),
            dig,
        ),
        (
            "peak_1k_m6_q2",
            peaking(1000.0, -6.0, 2.0, PeakQ::HalfGain),
            dig,
        ),
        (
            "peak_8k_p9_q07",
            peaking(8000.0, 9.0, 0.7, PeakQ::HalfGain),
            dig,
        ),
        (
            "ls_120_p6_s1",
            Section::Shelf {
                pass: Pass::Low,
                f: 120.0,
                gain_db: 6.0,
                order: ShelfOrder::Second(ShelfSlope::S(1.0)),
                f_at: ShelfFreq::Midpoint,
            },
            dig,
        ),
        (
            "hs_6k_m4_q09",
            Section::Shelf {
                pass: Pass::High,
                f: 6000.0,
                gain_db: -4.0,
                order: ShelfOrder::Second(ShelfSlope::Q(0.9)),
                f_at: ShelfFreq::Midpoint,
            },
            dig,
        ),
        (
            "notch_60_q8",
            Section::Notch {
                f: 60.0,
                bw: Bandwidth::Q(8.0),
            },
            dig,
        ),
        (
            "bp_500_q14",
            Section::BandPass {
                f: 500.0,
                bw: Bandwidth::Q(1.4),
            },
            dig,
        ),
        (
            "ap2_1k_q07",
            Section::AllPass2 {
                f: 1000.0,
                q: std::f64::consts::FRAC_1_SQRT_2,
            },
            dig,
        ),
        (
            "peak3db_2k_p12_q3_analog",
            peaking(2000.0, 12.0, 3.0, PeakQ::Minus3dbFromPeak),
            ANALOG,
        ),
    ];
    for (name, s, real) in cases {
        let got = response(&[s], real, &f).expect(name);
        gs.assert_c128(name, &got);
    }
    let qc = peak_cookbook_q(3.0, 12.0, PeakQ::Minus3dbFromPeak)
        .expect("defined")
        .expect("cookbook");
    assert!((qc - gs.scalar("peak3db_cookbook_q").expect("q")).abs() < 1e-9);
}
