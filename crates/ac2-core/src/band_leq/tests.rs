use super::*;
use crate::leq::Headroom;
use std::f64::consts::TAU;

/// The bands of the decree's low-frequency table.
const LF: [usize; LF_BANDS] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

/// Deterministic xorshift for test sequences.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn close_db(a: f64, b: f64, tol: f64) -> bool {
    (a.is_nan() && b.is_nan()) || (a - b).abs() <= tol
}

/// Energy and measured time of band `band` over the newest `len` seconds of `h`.
fn sums(h: &[BandSecond], band: usize, len: usize) -> (f64, f64) {
    let from = h.len().saturating_sub(len);
    h[from..]
        .iter()
        .fold((0.0, 0.0), |(e, m), s| (e + s.energy[band], m + s.measured))
}

/// Brute-force headroom: a window filling for at least the horizon spends its budget
/// exactly when held to the end of the fill; else the newest `N − h` seconds plus `h` new.
fn brute_allowed(h: &[BandSecond], band: usize, n: u32, horizon: u32, limit: f64) -> Option<f64> {
    let rem = (n as usize).saturating_sub(h.len());
    let x = if rem >= horizon as usize {
        let (e, m) = sums(h, band, n as usize);
        let r = rem as f64;
        (limit * (m + r) - e) / r
    } else {
        let keep = n.saturating_sub(horizon) as usize;
        if keep == 0 {
            return Some(limit);
        }
        let (e, m) = sums(h, band, keep);
        let hh = f64::from(horizon);
        (limit * (m + hh) - e) / hh
    };
    (x > 0.0).then_some(x)
}

fn brute_recover(h: &[BandSecond], band: usize, n: u32, horizon: u32, limit: f64) -> Option<u32> {
    (horizon.min(n)..=n).find(|&t| {
        let (e, m) = sums(h, band, (n - t) as usize);
        let tt = f64::from(t);
        (e + tt * limit) / (m + tt) <= limit * (1.0 + 1e-12)
    })
}

/// Random band seconds over 100 dB of dynamics, each band its own, with gaps and partial
/// seconds: every band's value, headroom and recovery time equals the brute-force sums,
/// through ring wrap-around and the periodic exact recompute.
#[test]
fn band_windows_match_brute_force_with_gaps() {
    let n = 90;
    let bands = 5;
    for horizon in [5, 60] {
        let mut rng = Rng(0x2545_f491_4f6c_dd1d ^ u64::from(horizon));
        let mut w = BandWindows::new(n, &(0..bands).collect::<Vec<_>>(), Weighting::Z, horizon);
        assert_eq!(w.windows().len(), bands);
        let mut hist = Vec::new();
        for step in 0..700 {
            let u = rng.next();
            let s = if u < 0.05 {
                BandSecond::GAP
            } else {
                let base = if (step / 120) % 2 == 0 { -10.0 } else { -90.0 };
                let mut levels = [f64::NEG_INFINITY; BANDS];
                for l in levels.iter_mut().take(bands) {
                    *l = base + 20.0 * rng.next();
                }
                let m = if u < 0.1 {
                    0.25 + 0.5 * rng.next()
                } else {
                    1.0
                };
                BandSecond::from_levels(&levels, m)
            };
            w.push(s, Period::Day);
            hist.push(s);
            let limit = mean_square(-25.0);
            for b in 0..bands {
                let got = w.windows().value(b);
                let (e, m) = sums(&hist, b, n as usize);
                let want = if m > 0.0 { power_dbfs(e / m) } else { f64::NAN };
                assert!(
                    close_db(got.leq_dbfs, want, 1e-6),
                    "step {step} band {b}: {} vs {want}",
                    got.leq_dbfs
                );
                assert!((got.measured - m).abs() < 1e-9, "step {step} band {b}");
                assert_eq!(got.elapsed, hist.len().min(n as usize) as u32);
                match (
                    w.windows().headroom(b, limit),
                    brute_allowed(&hist, b, n, horizon, limit),
                ) {
                    (Headroom::Allowed { ms }, Some(want)) => assert!(
                        close_db(power_dbfs(ms), power_dbfs(want), 1e-6),
                        "step {step} band {b}: allowed {ms} vs {want}"
                    ),
                    (Headroom::CannotRecover { recover_s }, None) => assert_eq!(
                        Some(recover_s),
                        brute_recover(&hist, b, n, horizon, limit),
                        "step {step} band {b}"
                    ),
                    (got, want) => panic!("step {step} band {b}: {got:?} vs {want:?}"),
                }
            }
        }
    }
}

/// The decree's night table (Liite 2 Taulukko 2) on the first eleven bands.
fn decree() -> BandLimits {
    let night = [
        74.0, 64.0, 56.0, 49.0, 44.0, 42.0, 40.0, 38.0, 36.0, 34.0, 32.0,
    ];
    let mut n = [None; BANDS];
    for (o, l) in n.iter_mut().zip(night) {
        *o = Some(l);
    }
    BandLimits::night_and_offset_day(n, 5.0)
}

#[test]
fn day_limits_are_five_db_above_night() {
    let l = decree();
    assert_eq!(l.of(Period::Night)[0], Some(74.0));
    assert_eq!(l.of(Period::Day)[0], Some(79.0));
    assert_eq!(l.of(Period::Day)[10], Some(37.0));
    assert_eq!(l.of(Period::Day)[11], None);
}

#[test]
fn period_boundaries() {
    let hms = |h: u32, m: u32, s: u32| h * 3600 + m * 60 + s;
    assert_eq!(Period::at(hms(21, 59, 59)), Period::Day);
    assert_eq!(Period::at(hms(22, 0, 0)), Period::Night);
    assert_eq!(Period::at(0), Period::Night);
    assert_eq!(Period::at(hms(6, 59, 59)), Period::Night);
    assert_eq!(Period::at(hms(7, 0, 0)), Period::Day);
    assert_eq!(Period::at(hms(12, 0, 0)), Period::Day);
    assert_eq!(Period::at(hms(23, 59, 59)), Period::Night);
    assert_eq!(Period::at(86_400), Period::Night, "a leap second");
}

/// An hour window is judged by the night set from the first night second it holds until
/// the last one has left it: from 22:00:00 on, and after 07:00 until 08:00.
#[test]
fn a_window_holding_any_night_second_is_judged_at_night() {
    let n = 3600u32;
    let mut w = BandWindows::new(n, &LF, Weighting::Z, 60);
    let start = 21 * 3600; // 21:00
    let s = BandSecond::from_levels(&[50.0 - 120.0; BANDS], 1.0);
    for k in 0..3600 {
        w.push(s, Period::at(start + k));
    }
    assert_eq!(w.period(), Period::Day, "21:00–21:59:59 is day");
    assert_eq!(
        w.period_after_horizon(Period::at(start + 3600 + 59)),
        Period::Night,
        "the horizon ends at 22:00:59"
    );
    w.push(s, Period::at(22 * 3600));
    assert_eq!(w.period(), Period::Night, "22:00:00 is in the window");

    // Morning: night until 07:00, then an hour until the last night second has left.
    let mut w = BandWindows::new(n, &LF, Weighting::Z, 60);
    let six = 6 * 3600;
    for k in 0..3600 {
        w.push(s, Period::at(six + k));
    }
    for k in 0..3599 {
        w.push(s, Period::at(7 * 3600 + k));
        assert_eq!(w.period(), Period::Night, "07:00 + {k} s");
    }
    assert_eq!(
        w.period_after_horizon(Period::Day),
        Period::Day,
        "06:59:59 leaves within the horizon"
    );
    w.push(s, Period::at(7 * 3600 + 3599));
    assert_eq!(w.period(), Period::Day, "07:59:59: no night second left");
}

/// Each second carries the period of its own local start, so a clock change inside the
/// window (here the autumn one, 04:00 back to 03:00, inside the night) neither shortens
/// nor lengthens it: the window is the newest 3600 seconds as measured.
#[test]
fn a_clock_change_moves_no_window() {
    let mut w = BandWindows::new(3600, &[0], Weighting::Z, 60);
    let mut local = 3 * 3600 + 30 * 60; // 03:30
    for k in 0..7200u32 {
        if k == 1800 {
            local -= 3600; // 04:00 → 03:00
        }
        let mut levels = [f64::NEG_INFINITY; BANDS];
        levels[0] = if k < 3600 { -20.0 } else { -40.0 };
        w.push(BandSecond::from_levels(&levels, 1.0), Period::at(local));
        local += 1;
    }
    assert!(close_db(w.windows().value(0).leq_dbfs, -40.0, 1e-9));
    assert_eq!(w.period(), Period::Night);
}

#[test]
fn judged_per_band_against_the_set_in_force_and_the_worst_named() {
    let mut w = BandWindows::new(60, &LF, Weighting::Z, 10);
    let limits = decree();
    // Sensitivity 120 dB SPL at 0 dBFS; 63 Hz (band 5) at 47 dB SPL: over the night 42, under
    // the day 47 + margin; 100 Hz (band 7) at 37: near the night 38.
    let offset = 120.0;
    let mut levels = [f64::NEG_INFINITY; BANDS];
    levels[5] = 47.0 - offset;
    levels[7] = 37.0 - offset;
    for k in 0..60 {
        w.push(
            BandSecond::from_levels(&levels, 1.0),
            Period::at(23 * 3600 + k),
        );
    }
    let mut out = [BandState {
        value: w.windows().value(0),
        level_db: f64::NAN,
        limit_db: None,
        verdict: None,
        headroom: None,
    }; LF_BANDS];
    w.judge(&limits, offset, 3.0, Period::Night, &mut out);
    assert_eq!(out[5].limit_db, Some(42.0));
    assert_eq!(out[5].verdict.map(|v| v.judgement), Some(Judgement::Over));
    assert_eq!(out[7].verdict.map(|v| v.judgement), Some(Judgement::Near));
    assert_eq!(out[0].verdict.map(|v| v.judgement), Some(Judgement::Ok));
    assert_eq!(worst_band(&out), Some(5));
    // A silent band: the 50 measured silent seconds that stay over a 10 s horizon allow
    // six times the limit's power for those 10 s.
    let Some(Headroom::Allowed { ms }) = out[0].headroom else {
        panic!("{:?}", out[0].headroom)
    };
    assert!(close_db(
        power_dbfs(ms) + offset,
        74.0 + 10.0 * 6f64.log10(),
        1e-9
    ));
    assert!(matches!(
        out[5].headroom,
        Some(Headroom::CannotRecover { .. })
    ));
    // By day the 63 Hz band is near (47 against 47), the 100 Hz one ok (37 against 43).
    let mut day = BandWindows::new(60, &LF, Weighting::Z, 10);
    for k in 0..60 {
        day.push(
            BandSecond::from_levels(&levels, 1.0),
            Period::at(12 * 3600 + k),
        );
    }
    day.judge(&limits, offset, 3.0, Period::Day, &mut out);
    assert_eq!(out[5].limit_db, Some(47.0));
    assert_eq!(out[5].verdict.map(|v| v.judgement), Some(Judgement::Near));
    assert_eq!(out[7].verdict.map(|v| v.judgement), Some(Judgement::Ok));
    assert_eq!(worst_band(&out), Some(5));
    // Ahead of the night: the headroom is against the night limits already.
    day.judge(&limits, offset, 3.0, Period::Night, &mut out);
    assert!(matches!(
        out[5].headroom,
        Some(Headroom::CannotRecover { .. })
    ));
}

/// A correction of K dB for part of the period raises the energy of those seconds: the
/// window's level is the energy average of L + K.
#[test]
fn correction_applies_for_the_seconds_it_is_in_force() {
    let mut w = BandWindows::new(100, &[0], Weighting::Z, 10);
    let mut levels = [f64::NEG_INFINITY; BANDS];
    levels[0] = -40.0;
    let s = BandSecond::from_levels(&levels, 1.0);
    for k in 0..100 {
        let s = if k < 25 { s.corrected(6.0) } else { s };
        w.push(s, Period::Day);
    }
    let want = -40.0 + 10.0 * (0.25 * 10f64.powf(0.6) + 0.75).log10();
    assert!(close_db(w.windows().value(0).leq_dbfs, want, 1e-9));
}

#[test]
fn transfer_background_rules() {
    // ≥ 10 dB above the background: the difference as is.
    assert_eq!(
        BandTransfer::measure(90.0, 50.0, Some(40.0)),
        BandTransfer::Clean {
            attenuation_db: 40.0
        }
    );
    // 3 … 10 dB: background energy subtracted. 46 over 40: 10·lg(10^4.6 − 10^4).
    let BandTransfer::Corrected {
        attenuation_db,
        margin_db,
    } = BandTransfer::measure(90.0, 46.0, Some(40.0))
    else {
        panic!()
    };
    let signal = 10.0 * (10f64.powf(4.6) - 1e4).log10();
    assert!((attenuation_db - (90.0 - signal)).abs() < 1e-9);
    assert!((margin_db - 6.0).abs() < 1e-12);
    assert!(
        attenuation_db > 44.0 && attenuation_db < 45.3,
        "{attenuation_db}"
    );
    // Exactly 3 dB is still corrected (the signal ≈ the background), 2.9 is not.
    assert!(matches!(
        BandTransfer::measure(90.0, 43.0, Some(40.0)),
        BandTransfer::Corrected { .. }
    ));
    assert_eq!(
        BandTransfer::measure(90.0, 42.9, Some(40.0)),
        BandTransfer::Unusable { at_least_db: 50.0 }
    );
    assert_eq!(
        BandTransfer::measure(90.0, 38.0, Some(40.0)).conservative_db(),
        Some(50.0)
    );
    assert_eq!(
        BandTransfer::measure(90.0, 38.0, Some(40.0)).attenuation_db(),
        None
    );
    // Without a background, unchecked; without a level, missing.
    assert_eq!(
        BandTransfer::measure(90.0, 60.0, None),
        BandTransfer::Unchecked {
            attenuation_db: 30.0
        }
    );
    assert_eq!(
        BandTransfer::measure(f64::NEG_INFINITY, 60.0, None),
        BandTransfer::Missing
    );
    assert_eq!(
        BandTransfer::measure(90.0, 60.0, Some(f64::NAN)),
        BandTransfer::Missing
    );
}

#[test]
fn foh_limits_are_dwelling_limits_plus_attenuation() {
    let foh = [95.0; BANDS];
    let mut dwelling = [55.0; BANDS];
    dwelling[3] = 41.0; // 1 dB over a 40 dB background: unusable, at least 55
    let bg = [40.0; BANDS];
    let t = Transfer::measure(&foh, &dwelling, Some(&bg));
    let l = t.foh_limits(&decree());
    assert_eq!(l.night[0], Some(74.0 + 40.0));
    assert_eq!(l.day[0], Some(79.0 + 40.0));
    assert_eq!(l.night[3], Some(49.0 + 55.0), "the bound, never more");
    assert_eq!(l.night[11], None);
}

/// One band at the FOH: the dwelling level is the band level less the attenuation, plus
/// the A weighting at the band centre; bands add as energies; an unusable band only in the
/// upper bound.
#[test]
fn predicted_dwelling_laeq() {
    let mut foh = [f64::NAN; BANDS];
    let mut dwelling = [f64::NAN; BANDS];
    let bg = [0.0; BANDS];
    // 63 Hz: 30 dB down, clean. 125 Hz: 40 dB down, clean. 1 kHz: unusable, ≥ 60 dB.
    for (b, att) in [(5usize, 30.0), (8, 40.0)] {
        foh[b] = 100.0;
        dwelling[b] = 100.0 - att;
    }
    foh[17] = 100.0;
    dwelling[17] = 41.0;
    let bg = {
        let mut b = bg;
        b[17] = 40.0;
        b
    };
    let t = Transfer::measure(&foh, &dwelling, Some(&bg));
    assert_eq!(t.bands()[0], BandTransfer::Missing);
    let mut levels = [f64::NEG_INFINITY; BANDS];
    levels[5] = -20.0;
    levels[8] = -20.0;
    levels[17] = -20.0;
    let p = t.predict(&BandSecond::from_levels(&levels, 1.0));
    let a = |b: usize| Weighting::A.analytic_db(centre_hz(b));
    let e = |l: f64| 10f64.powf(l / 10.0);
    let est = 10.0 * (e(-50.0 + a(5)) + e(-60.0 + a(8))).log10();
    let most = 10.0 * (e(-50.0 + a(5)) + e(-60.0 + a(8)) + e(-80.0 + a(17))).log10();
    assert!(close_db(
        power_dbfs(p.energy[PredictedSecond::ESTIMATE]),
        est,
        1e-9
    ));
    assert!(close_db(
        power_dbfs(p.energy[PredictedSecond::AT_MOST]),
        most,
        1e-9
    ));
    assert!((a(5) + 26.2).abs() < 0.1, "A at 63 Hz ≈ −26.2 dB: {}", a(5));
    // A rolling window over the prediction, judged like any window.
    let mut r = RollingLeq::<PredictedSecond>::of_channels(
        [
            (3600, PredictedSecond::ESTIMATE),
            (3600, PredictedSecond::AT_MOST),
        ],
        60,
    );
    r.push(p);
    assert!(close_db(r.value(0).leq_dbfs, est, 1e-9));
}

fn sine(f: f64, amp: f64, fs: f64, n: usize, phase: f64) -> impl Iterator<Item = f64> {
    (0..n).map(move |i| amp * (TAU * f * i as f64 / fs + phase).sin())
}

/// A steady sine at a band's exact centre reads its own level (`10·lg(2·A²/2)`) in that
/// band, within the class 1 pass-band tolerance (±0.4 dB), and far less in the others.
#[test]
fn sine_at_band_centre_reads_its_level() {
    let fs = 48_000.0;
    for band in [0usize, 5, 10, 17, 27] {
        let mut bi = BandIntegrator::new(fs).expect("bank");
        assert_eq!(bi.bands().count(), BANDS);
        assert!(bi.bands().all(|b| b.meets_class1));
        let x: Vec<f64> = sine(centre_hz(band), 0.5, fs, 3 * 48_000, 0.3).collect();
        let mut out = Vec::new();
        bi.process(&x, |s| out.push(s));
        assert_eq!(out.len(), 3);
        let want = power_dbfs(0.125);
        // The first second holds the filters' rise; the following ones are steady.
        for s in &out[1..] {
            assert_eq!(s.measured, 1.0);
            let got = s.level_dbfs(band);
            assert!((got - want).abs() < 0.4, "band {band}: {got} vs {want}");
            for (b, _) in NOMINAL_HZ.iter().enumerate() {
                if b.abs_diff(band) >= 2 {
                    assert!(s.level_dbfs(b) < want - 30.0, "band {band} leaks into {b}");
                }
            }
        }
    }
}

/// Pink-ish signal: one sine per band at its centre, all of the same amplitude (equal power
/// per 1/3 octave, as pink noise), random phases. Every band reads the one sine's level and
/// the bands add up to the total power.
#[test]
fn equal_power_per_band_reads_flat() {
    let fs = 48_000.0;
    let mut rng = Rng(0xdead_beef_cafe_f00d);
    let phases: Vec<f64> = (0..BANDS).map(|_| TAU * rng.next()).collect();
    let amp = 0.05;
    let n = 3 * 48_000;
    let mut x = vec![0.0; n];
    for (b, ph) in phases.iter().enumerate() {
        for (y, v) in x.iter_mut().zip(sine(centre_hz(b), amp, fs, n, *ph)) {
            *y += v;
        }
    }
    let mut bi = BandIntegrator::new(fs).expect("bank");
    let mut out = Vec::new();
    // In odd blocks, as audio arrives.
    for chunk in x.chunks(1013) {
        bi.process(chunk, |s| out.push(s));
    }
    assert_eq!(out.len(), 3);
    let want = power_dbfs(amp * amp / 2.0);
    let s = out[2];
    for b in 0..BANDS {
        let got = s.level_dbfs(b);
        assert!((got - want).abs() < 0.4, "band {b}: {got} vs {want}");
    }
    let total: f64 = s.energy.iter().sum();
    let want_total = power_dbfs(BANDS as f64 * amp * amp / 2.0);
    assert!((power_dbfs(total) - want_total).abs() < 0.3);
}

/// A gap advances the grid without energy or measured time: a second half measured
/// reads the level over its measured half, and lost seconds come out empty.
#[test]
fn skip_makes_partial_seconds() {
    let fs = 48_000.0;
    let mut bi = BandIntegrator::new(fs).expect("bank");
    let x: Vec<f64> = sine(centre_hz(17), 0.5, fs, 48_000 + 24_000, 0.0).collect();
    let mut out = Vec::new();
    bi.process(&x, |s| out.push(s));
    bi.skip(24_000 + 48_000, |s| out.push(s));
    assert_eq!(out.len(), 3);
    assert_eq!(out[1].measured, 0.5);
    assert!((out[1].level_dbfs(17) - power_dbfs(0.125)).abs() < 0.4);
    assert_eq!(out[2], BandSecond::GAP);
    assert_eq!(bi.position(), 0);
}

#[test]
fn too_low_a_rate_is_refused() {
    assert!(BandIntegrator::new(16_000.0).is_err());
    assert!(BandIntegrator::new(44_100.0).is_ok());
}

#[test]
fn nominal_and_exact_centres_agree() {
    for (b, nom) in NOMINAL_HZ.iter().enumerate() {
        let exact = centre_hz(b);
        assert!((exact / nom - 1.0).abs() < 0.03, "{nom} vs {exact}");
    }
    assert_eq!(NOMINAL_HZ[LF_BANDS - 1], 200.0);
}

#[test]
fn logged_seconds_refill_by_wall_time_with_gaps_in_their_period() {
    const NS: u64 = 1_000_000_000;
    let lv = |db: f64| BandSecond::from_levels(&[db; BANDS], 1.0);
    // Rows at t = 100, 101, 103 s (102 missing), now = 104 s, window 3 s: slots 101…103.
    let rows = [
        (103 * NS, lv(-20.0), Period::Day),
        (101 * NS, lv(-30.0), Period::Day),
        (100 * NS, lv(-10.0), Period::Day),
    ];
    let placed = Placed::new(rows, 104 * NS, 3);
    let night_at_102 = |t: u64| {
        if t == 102 * NS {
            Period::Night
        } else {
            Period::Day
        }
    };
    let got: Vec<(BandSecond, Period)> = placed.seconds(night_at_102).collect();
    assert_eq!(got.len(), 3);
    assert_eq!(got[0], (lv(-30.0), Period::Day));
    assert_eq!(got[1], (BandSecond::GAP, Period::Night));
    assert_eq!(got[2], (lv(-20.0), Period::Day));

    let mut w = BandWindows::new(3, &LF, Weighting::Z, 1);
    w.refill(placed.seconds(night_at_102));
    // The gap second was a night second: the window is judged at night.
    assert_eq!(w.period(), Period::Night);
    let v = w.windows().value(0);
    assert_eq!(v.measured, 2.0);
    let want = power_dbfs((mean_square(-30.0) + mean_square(-20.0)) / 2.0);
    assert!(close_db(v.leq_dbfs, want, 1e-9), "{} {want}", v.leq_dbfs);

    // Nothing in the span: nothing pushed.
    let empty = Placed::new([(10 * NS, lv(0.0), Period::Day)], 104 * NS, 3);
    assert_eq!(empty.seconds(|_| Period::Day).count(), 0);
}

/// A weighted band window reads the unweighted band level plus the IEC 61672-1 weighting at
/// the band's exact mid-band frequency; its limit, judgement and allowed level are on that
/// weighted level.
#[test]
fn weighting_adds_the_analytic_weighting_at_the_mid_band() {
    let bands = [0, 5, 17, 27];
    let level = -40.0;
    let s = BandSecond::from_levels(&[level; BANDS], 1.0);
    for w in [Weighting::A, Weighting::C, Weighting::Z] {
        let mut win = BandWindows::new(30, &bands, w, 10);
        for _ in 0..30 {
            win.push(s, Period::Day);
        }
        let offset = 100.0;
        // Each band 1 dB under a limit put on its weighted level, margin 3 dB: near.
        let mut lim = [None; BANDS];
        for &b in &bands {
            lim[b] = Some(level + offset + w.analytic_db(centre_hz(b)) + 1.0);
        }
        let mut out = vec![
            BandState {
                value: win.windows().value(0),
                level_db: f64::NAN,
                limit_db: None,
                verdict: None,
                headroom: None,
            };
            bands.len()
        ];
        win.judge(&BandLimits::always(lim), offset, 3.0, Period::Day, &mut out);
        for (o, &b) in out.iter().zip(&bands) {
            let want = level + offset + w.analytic_db(centre_hz(b));
            assert!(close_db(o.level_db, want, 1e-9), "{w:?} band {b}");
            assert_eq!(o.verdict.map(|v| v.judgement), Some(Judgement::Near));
            // Full and steady 1 dB under the limit: the next 10 s may play at the level
            // that brings the 30 s window to the limit exactly.
            let Some(Headroom::Allowed { ms }) = o.headroom else {
                panic!("{:?}", o.headroom)
            };
            let lim_ms = mean_square(want + 1.0 - offset);
            let steady = mean_square(want - offset);
            let allowed = (30.0 * lim_ms - 20.0 * steady) / 10.0;
            assert!(
                close_db(power_dbfs(ms), power_dbfs(allowed), 1e-6),
                "{w:?} {b}"
            );
        }
    }
    assert!(close_db(weighting_db(Weighting::A, 17), 0.0, 1e-9));
    assert!(close_db(weighting_db(Weighting::A, 0), -50.45, 0.01));
    assert_eq!(weighting_db(Weighting::Z, 0), 0.0);
}

/// Only the bands selected have windows, in the order given, judged against their own
/// band's limit.
#[test]
fn a_selection_of_bands_is_windowed_and_judged() {
    let sel = [3, 9, 20];
    let mut w = BandWindows::new(10, &[3, 9, 20, BANDS], Weighting::Z, 5);
    assert_eq!(w.bands(), &sel);
    assert_eq!(w.windows().len(), 3);
    let mut levels = [f64::NEG_INFINITY; BANDS];
    for (k, &b) in sel.iter().enumerate() {
        levels[b] = -50.0 + 10.0 * k as f64;
    }
    for _ in 0..10 {
        w.push(BandSecond::from_levels(&levels, 1.0), Period::Day);
    }
    let mut lim = [None; BANDS];
    lim[9] = Some(35.0);
    let mut out = [BandState {
        value: w.windows().value(0),
        level_db: f64::NAN,
        limit_db: None,
        verdict: None,
        headroom: None,
    }; 3];
    w.judge(&BandLimits::always(lim), 80.0, 3.0, Period::Day, &mut out);
    for (k, o) in out.iter().enumerate() {
        assert!(close_db(o.level_db, 30.0 + 10.0 * k as f64, 1e-9));
    }
    assert_eq!(out[0].verdict, None);
    assert_eq!(out[1].limit_db, Some(35.0));
    assert_eq!(out[1].verdict.map(|v| v.judgement), Some(Judgement::Over));
    assert_eq!(out[2].verdict, None);
    assert_eq!(worst_band(&out), Some(1));
}

/// Two windows of different lengths and weightings over the same random seconds with gaps:
/// each band of each equals the brute-force sums over its own length, weighted.
#[test]
fn several_windows_match_brute_force() {
    let bands = [0, 3, 17];
    let specs = [(30u32, Weighting::A), (90, Weighting::C)];
    let mut wins: Vec<BandWindows> = specs
        .iter()
        .map(|&(n, w)| BandWindows::new(n, &bands, w, 10))
        .collect();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut hist = Vec::new();
    for step in 0..400 {
        let s = if rng.next() < 0.05 {
            BandSecond::GAP
        } else {
            let mut levels = [f64::NEG_INFINITY; BANDS];
            for &b in &bands {
                levels[b] = -60.0 + 40.0 * rng.next();
            }
            BandSecond::from_levels(&levels, 1.0)
        };
        hist.push(s);
        for win in &mut wins {
            win.push(s, Period::Day);
        }
        let lim = BandLimits::always([Some(70.0); BANDS]);
        for (win, &(n, wt)) in wins.iter().zip(&specs) {
            let mut out = vec![
                BandState {
                    value: win.windows().value(0),
                    level_db: f64::NAN,
                    limit_db: None,
                    verdict: None,
                    headroom: None,
                };
                bands.len()
            ];
            win.judge(&lim, 100.0, 3.0, Period::Day, &mut out);
            for (o, &b) in out.iter().zip(&bands) {
                let (e, m) = sums(&hist, b, n as usize);
                let g = wt.analytic_db(centre_hz(b));
                let want = if m > 0.0 {
                    power_dbfs(e / m) + 100.0 + g
                } else {
                    f64::NAN
                };
                assert!(
                    close_db(o.level_db, want, 1e-6),
                    "step {step} {n} s band {b}: {} vs {want}",
                    o.level_db
                );
                let brute = brute_allowed(&hist, b, n, 10, mean_square(70.0 - 100.0 - g));
                match (o.headroom, brute) {
                    (Some(Headroom::Allowed { ms }), Some(a)) => assert!(
                        close_db(power_dbfs(ms), power_dbfs(a) + g, 1e-6),
                        "step {step} {n} s band {b}"
                    ),
                    (Some(Headroom::CannotRecover { .. }), None) => {}
                    (got, want) => panic!("step {step} {n} s band {b}: {got:?} vs {want:?}"),
                }
            }
        }
    }
}
