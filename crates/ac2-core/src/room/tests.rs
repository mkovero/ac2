//! Analytic rooms: exponentially decaying Gaussian noise (a diffuse field) with a direct
//! sound and stationary background noise, whose decay times, crossing point and energy
//! ratios are known in closed form; and the ensemble-mean band energy of such a field
//! through the band filters, whose decay is known exactly. Tolerances are in
//! `docs/design/room-metrics.md`.

use super::*;

const FS: f64 = 48_000.0;
const LN10: f64 = std::f64::consts::LN_10;

/// Gaussian noise from a fixed seed (xorshift64* and Box–Muller).
struct Rng(u64);

impl Rng {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let v = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((v >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gaussian(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

/// Background noise before the direct sound, s.
const PRE: f64 = 0.05;

/// A room: [`PRE`] of background noise, then a direct impulse of `direct` × the whole
/// reverberant energy, then reverberant noise whose energy falls 60 dB in `t60` s (in
/// `t60b` s once it has fallen `knee_db`), all over background noise `snr_db` below the
/// reverberation's starting energy per sample. `len` s in all.
#[derive(Clone, Copy)]
struct Room {
    t60: f64,
    knee_db: f64,
    t60b: f64,
    snr_db: f64,
    direct: f64,
    len: f64,
    seed: u64,
}

impl Room {
    fn new(t60: f64, snr_db: f64) -> Self {
        Self {
            t60,
            knee_db: f64::INFINITY,
            t60b: t60,
            snr_db,
            direct: 0.1,
            len: (t60 * 1.2).max(0.6),
            seed: 7,
        }
    }

    fn seed(self, seed: u64) -> Self {
        Self { seed, ..self }
    }

    /// Reverberant energy per sample `t` s after the direct sound, dB re its start.
    fn envelope_db(&self, t: f64) -> f64 {
        let knee_t = self.knee_db / 60.0 * self.t60;
        if t <= knee_t {
            -60.0 * t / self.t60
        } else {
            -self.knee_db - 60.0 * (t - knee_t) / self.t60b
        }
    }

    /// Whole reverberant energy re its energy per sample at the start (single slope).
    fn reverberant(&self) -> f64 {
        1.0 / (1.0 - (-6.0 * LN10 / self.t60 / FS).exp())
    }

    fn ir(&self) -> Vec<f64> {
        let mut rng = Rng(self.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let n = (self.len * FS) as usize;
        let d = (PRE * FS) as usize;
        let noise = 10f64.powf(-self.snr_db / 20.0);
        (0..n)
            .map(|i| {
                let mut v = noise * rng.gaussian();
                if i >= d {
                    let t = (i - d) as f64 / FS;
                    v += 10f64.powf(self.envelope_db(t) / 20.0) * rng.gaussian();
                }
                if i == d {
                    v += (self.direct * self.reverberant()).sqrt();
                }
                v
            })
            .collect()
    }

    /// The decay time the expected energy (no noise, no filter) gives: Schroeder integral
    /// of the envelope plus the direct sound, fitted as the analysis fits.
    fn expected(&self, d: Decay) -> f64 {
        let n = (self.t60.max(self.t60b) * 3.0 * FS) as usize;
        let mut e: Vec<f64> = (0..n)
            .map(|i| 10f64.powf(self.envelope_db(i as f64 / FS) / 10.0))
            .collect();
        e[0] += self.direct * self.reverberant();
        let edc = schroeder_db(&e, 0.0);
        let (top, bottom) = d.range_db();
        decay_time(&edc, FS, top, bottom).expect("expected decay")
    }

    /// Expected energy before / after `ms` (single slope): (early, late).
    fn split(&self, ms: f64) -> (f64, f64) {
        let k = 6.0 * LN10 / self.t60;
        let r = self.reverberant();
        let a = (-k * ms * 1e-3).exp();
        (self.direct * r + r * (1.0 - a), r * a)
    }

    fn clarity_db(&self, ms: f64) -> f64 {
        let (e, l) = self.split(ms);
        10.0 * (e / l).log10()
    }

    fn d50(&self) -> f64 {
        let (e, l) = self.split(50.0);
        e / (e + l)
    }

    fn analysed(&self) -> RoomAnalysis {
        analyse(&self.ir(), FS, -PRE, (20.0, 20_000.0))
    }
}

fn lo_hz(fm: f64) -> f64 {
    BandFraction::Octave.edges(fm).0
}

fn hi_hz(fm: f64) -> f64 {
    BandFraction::Octave.edges(fm).1
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b
}

fn value(m: &BandMetrics, what: &str, v: Metric) -> f64 {
    v.unwrap_or_else(|e| panic!("{:?} Hz {what} refused: {e:?}\n{m:#?}", m.centre_hz))
}

/// Broadband against the analytic room, three seeds each: EDT ±6 %, T20 ±2.5 %, T30 ±2 %,
/// C50 / C80 ±0.5 dB, D50 ±0.02.
#[test]
fn broadband_matches_the_analytic_room() {
    for t60 in [0.3, 0.8, 2.0] {
        for seed in 1..=3 {
            let room = Room::new(t60, 75.0).seed(seed);
            let b = room.analysed().broadband;
            assert!(b.centre_hz.is_none());
            for (d, tol) in [(Decay::Edt, 0.06), (Decay::T20, 0.025), (Decay::T30, 0.02)] {
                let got = value(&b, "decay", b.decay(d));
                let want = room.expected(d);
                assert!(rel(got, want) < tol, "T {t60} {d:?}: {got} vs {want}");
            }
            for (ms, got) in [(50.0, b.c50_db), (80.0, b.c80_db)] {
                let got = value(&b, "clarity", got);
                let want = room.clarity_db(ms);
                assert!((got - want).abs() < 0.5, "T {t60} C{ms}: {got} vs {want}");
            }
            let d50 = value(&b, "D50", b.d50);
            assert!(
                (d50 - room.d50()).abs() < 0.02,
                "D50 {d50} vs {}",
                room.d50()
            );
            // The direct sound is the onset: the trigger finds it.
            assert!(b.onset_s.abs() < 1.0 / FS, "onset {}", b.onset_s);
            assert!(b.curvature_pct.is_some_and(|c| c.abs() < 5.0));
        }
    }
}

/// A single band of one impulse response scatters (a few degrees of freedom per band: the
/// statistical uncertainty of any IR measurement), so the octave bands are judged on the
/// mean over 16 rooms and a bound per room. Mean error: T20 / T30 ±3 % from 500 Hz, ±8 %
/// below; EDT ±5 % / ±15 %; C80 ±0.5 dB / ±1.5 dB. Each room: T30 ±12 % from 1 kHz. Below
/// 500 Hz a room's EDT can come out short enough for the band filter to refuse it; the mean
/// is over the rooms that give one, at least 12 of 16.
#[test]
fn octave_bands_are_unbiased() {
    let seeds = 16;
    for t60 in [0.4, 1.2] {
        let base = Room::new(t60, 75.0);
        let rooms: Vec<RoomAnalysis> = (1..=seeds).map(|s| base.seed(s).analysed()).collect();
        let bands = rooms[0].octave.len();
        assert_eq!(bands, 8, "63 Hz … 8 kHz");
        for j in 0..bands {
            let fm = rooms[0].octave[j].centre_hz.expect("band");
            let high = fm > 400.0;
            let mean = |f: &dyn Fn(&BandMetrics) -> Option<f64>| {
                let v: Vec<f64> = rooms.iter().filter_map(|r| f(&r.octave[j])).collect();
                assert!(v.len() >= 12, "{fm} Hz: {} of {seeds} given", v.len());
                v.iter().sum::<f64>() / v.len() as f64
            };
            // Where B·T is small a single band's EDT scatters too widely for 16 rooms.
            let edt_checked = (hi_hz(fm) - lo_hz(fm)) * t60 >= 4.0 * MIN_BANDWIDTH_DECAY;
            for (d, lo_tol, hi_tol) in [
                (Decay::Edt, 0.15, 0.05),
                (Decay::T20, 0.08, 0.03),
                (Decay::T30, 0.08, 0.03),
            ] {
                if d == Decay::Edt && !edt_checked {
                    continue;
                }
                let want = base.expected(d);
                let got = mean(&|m| match m.decay(d) {
                    Err(Refusal::FilterLimited { .. }) if !high && d == Decay::Edt => None,
                    v => Some(value(m, "decay", v)),
                });
                let tol = if high { hi_tol } else { lo_tol };
                assert!(
                    rel(got, want) < tol,
                    "{fm} Hz T {t60} {d:?}: mean {got} vs {want}"
                );
            }
            let c80 = mean(&|m| Some(value(m, "C80", m.c80_db)));
            let tol = if high { 0.5 } else { 1.5 };
            assert!(
                (c80 - base.clarity_db(80.0)).abs() < tol,
                "{fm} Hz T {t60} C80: mean {c80} vs {}",
                base.clarity_db(80.0)
            );
            if fm > 900.0 {
                for r in &rooms {
                    let t = value(&r.octave[j], "T30", r.octave[j].t30_s);
                    assert!(rel(t, t60) < 0.12, "{fm} Hz T30 {t} vs {t60}");
                }
            }
        }
    }
}

/// Lundeby's truncation at several signal-to-noise ratios: the decay times stay within
/// 3 % (broadband) wherever they are given with 10 dB of range to spare, 5 % when the
/// noise is nearer than that, and the crossing point lies within 10 % of
/// where the decay meets the noise analytically.
#[test]
fn noise_is_truncated_and_corrected() {
    let t60 = 0.8;
    for snr in [70.0, 55.0, 46.0, 38.0] {
        let room = Room {
            len: 2.0,
            ..Room::new(t60, snr)
        };
        let b = room.analysed().broadband;
        let crossing = t60 * snr / 60.0;
        assert!(
            rel(b.truncation_s, crossing) < 0.1,
            "SNR {snr}: truncation {} vs {crossing}",
            b.truncation_s
        );
        for d in [Decay::Edt, Decay::T20, Decay::T30] {
            match b.decay(d) {
                Ok(t) => {
                    let tol = if snr >= d.needed_range_db() + 10.0 {
                        0.03
                    } else {
                        0.05
                    };
                    assert!(rel(t, room.expected(d)) < tol, "SNR {snr} {d:?}: {t}");
                }
                Err(Refusal::InsufficientRange {
                    range_db,
                    needed_db,
                }) => {
                    assert!(range_db < needed_db);
                    assert!(
                        snr < needed_db + 5.0,
                        "SNR {snr}: {d:?} refused at {range_db}"
                    );
                }
                Err(e) => panic!("SNR {snr} {d:?}: {e:?}"),
            }
        }
        // Without the correction the integral would bend down near the truncation point
        // and T30 would read short; with it T30 holds at 46 dB.
        if snr > 45.0 {
            assert!(b.t30_s.is_ok(), "SNR {snr}: {:?}", b.t30_s);
        }
    }
}

/// Refuse rather than mislead: T30 needs 45 dB of decay range, T20 35 dB, EDT and the
/// energy ratios 20 dB; silence has no decay.
#[test]
fn short_decay_ranges_are_refused() {
    let room = Room::new(0.8, 40.0);
    let b = room.analysed().broadband;
    assert!(b.t20_s.is_ok());
    assert!(matches!(
        b.t30_s,
        Err(Refusal::InsufficientRange { needed_db, .. }) if needed_db == 45.0
    ));
    let room = Room::new(0.8, 28.0);
    let b = room.analysed().broadband;
    assert!(b.edt_s.is_ok() && b.c80_db.is_ok());
    assert!(matches!(
        b.t20_s,
        Err(Refusal::InsufficientRange { needed_db, range_db }) if needed_db == 35.0 && range_db < 35.0
    ));
    let room = Room::new(0.8, 14.0);
    let b = room.analysed().broadband;
    for v in [b.edt_s, b.t20_s, b.t30_s, b.c50_db, b.c80_db, b.d50] {
        assert!(v.is_err(), "{b:#?}");
    }
    let a = analyse(&vec![0.0; 48_000], FS, 0.0, (20.0, 20_000.0));
    assert_eq!(a.broadband.edt_s, Err(Refusal::NoDecay));
    assert!(a.octave.iter().all(|m| m.t20_s == Err(Refusal::NoDecay)));
}

/// A double-slope decay (0.3 s for the first 25 dB, then 1.5 s): EDT follows the first
/// slope, T20 and T30 the mix, each within 6 % (EDT) / 4 % of the noise-free envelope's own
/// fit, and the curvature is the envelope's (31 %) within 8 points, above the 10 % that says the
/// decay is not straight.
#[test]
fn a_double_slope_decay_shows_curvature() {
    for seed in 1..=3 {
        let room = Room {
            knee_db: 25.0,
            t60b: 1.5,
            len: 3.0,
            ..Room::new(0.3, 75.0)
        }
        .seed(seed);
        let b = room.analysed().broadband;
        for d in [Decay::Edt, Decay::T20, Decay::T30] {
            let (got, want) = (value(&b, "decay", b.decay(d)), room.expected(d));
            let tol = if d == Decay::Edt { 0.06 } else { 0.04 };
            assert!(rel(got, want) < tol, "{d:?}: {got} vs {want}");
        }
        let want = 100.0 * (room.expected(Decay::T30) / room.expected(Decay::T20) - 1.0);
        assert!(want > CURVATURE_LIMIT_PCT, "{want}");
        assert!(
            b.curvature_pct.is_some_and(|c| (c - want).abs() < 8.0),
            "{:?} vs {want}",
            b.curvature_pct
        );
    }
}

/// Decay times of the ensemble-mean band energy of the room (envelope convolved with the
/// squared band filter response), forwards or backwards, from the onset the analysis uses.
fn filtered_expectation(t60: f64, fm: f64, reversed: bool, direct: f64) -> [Option<f64>; 3] {
    let (_, sections) = full_rate_band(BandFraction::Octave, fm, FS).expect("band");
    let mut imp = vec![0.0; (FS * 2.0) as usize];
    imp[0] = 1.0;
    let g2: Vec<f64> = filter(&imp, &sections, false, 0)
        .iter()
        .map(|v| v * v)
        .collect();
    let peak = g2.iter().copied().fold(0.0, f64::max);
    let glen = g2.iter().rposition(|v| *v > 1e-14 * peak).unwrap_or(0) + 1;
    let n = (t60 * 2.0 * FS) as usize + 2 * glen;
    let k = 6.0 * LN10 / t60;
    // Time zero at `glen`: room for the backwards filter's pre-ringing.
    let z = glen;
    let mut env: Vec<f64> = (0..n)
        .map(|i| {
            if i >= z {
                (-k * (i - z) as f64 / FS).exp()
            } else {
                0.0
            }
        })
        .collect();
    env[z] += direct / (1.0 - (-k / FS).exp());
    let e: Vec<f64> = (0..n)
        .map(|i| {
            if reversed {
                (0..glen.min(n - i)).map(|j| env[i + j] * g2[j]).sum()
            } else {
                (0..glen.min(i + 1)).map(|j| env[i - j] * g2[j]).sum()
            }
        })
        .collect();
    let (_, band) = from_onset(&e, reversed.then_some(z)).expect("onset");
    let edc = schroeder_db(&band, 0.0);
    [Decay::Edt, Decay::T20, Decay::T30].map(|d| {
        let (top, bottom) = d.range_db();
        decay_time(&edc, FS, top, bottom)
    })
}

/// Why the decay runs the band filter backwards, and where it stops reporting: at B·T = 8
/// a forward filter lengthens EDT by > 40 %; backwards with the pre-onset energy counted at
/// the onset, EDT is within 1 % (6 % with a direct sound as strong as the reverberation),
/// and T20 / T30 are exact down to B·T = 4 (the 63 Hz octave).
#[test]
fn backwards_filtering_keeps_the_decay() {
    let (lo, hi) = BandFraction::Octave.edges(62.5);
    let bw = hi - lo;
    let err = |r: Option<f64>, t: f64| (r.expect("decay") / t - 1.0).abs();
    let t = MIN_BANDWIDTH_DECAY / bw;
    let fwd = filtered_expectation(t, 62.5, false, 0.0);
    assert!(err(fwd[0], t) > 0.4, "forward EDT {:?}", fwd[0]);
    for (direct, tol) in [(0.0, 0.01), (0.1, 0.01), (1.0, 0.06)] {
        let r = filtered_expectation(t, 62.5, true, direct);
        // The direct sound barely moves the expected EDT of a least-squares fit.
        assert!(err(r[0], t) < tol, "D/R {direct}: EDT {:?} vs {t}", r[0]);
    }
    let t = 4.0 / bw;
    for direct in [0.0, 1.0] {
        let r = filtered_expectation(t, 62.5, true, direct);
        assert!(err(r[1], t) < 1e-3 && err(r[2], t) < 1e-3, "{r:?} vs {t}");
    }
}

/// A decay too short for a narrow band's filter is refused there, and still given where the
/// band is wide enough.
#[test]
fn short_decays_are_refused_in_narrow_bands() {
    let room = Room::new(0.12, 75.0);
    let a = room.analysed();
    let low = &a.octave[0];
    assert_eq!(low.centre_hz.map(f64::round), Some(63.0));
    // B·T ≈ 5: the energy moved before the onset leaves no early decay to fit.
    assert!(low.edt_s.is_err(), "{low:#?}");
    let top = a.octave.last().expect("8 kHz");
    assert!(top.edt_s.is_ok() && top.t30_s.is_ok());
    let third_low = &a.third[0];
    assert_eq!(third_low.centre_hz.map(f64::round), Some(50.0));
    assert!(matches!(
        third_low.t20_s,
        Err(Refusal::FilterLimited { .. })
    ));
}

/// Bands lie inside the excited range only; one-third octaves 50 Hz … 10 kHz at 48 kHz.
#[test]
fn bands_follow_the_excited_range() {
    let ir = Room::new(0.5, 70.0).ir();
    let a = analyse(&ir, FS, 0.0, (100.0, 5000.0));
    // The 125 Hz octave starts at 88 Hz, the 4 kHz one ends at 5.6 kHz: both outside.
    let centres: Vec<f64> = a.octave.iter().filter_map(|m| m.centre_hz).collect();
    assert_eq!(
        centres.iter().map(|f| f.round()).collect::<Vec<_>>(),
        [251.0, 501.0, 1000.0, 1995.0]
    );
    let a = analyse(&ir, FS, 0.0, (20.0, 20_000.0));
    assert_eq!(a.third.len(), 24);
}

/// The filter, Schroeder integral and decay fit against numpy/scipy
/// (`tools/refgen/sets/room.py`): a fixed IR through each octave band backwards in time,
/// the Schroeder curve truncated at a fixed point without correction, the decay times
/// fitted by `numpy.polyfit`, and C50 / C80 by window-before-filtering.
#[test]
fn filter_and_schroeder_match_refgen() {
    let g = ac2_testkit::golden::GoldenSet::load("room_schroeder_octaves").expect("golden");
    let ir = g.f64("ir").expect("ir");
    let fs = g.scalar("fs").expect("fs");
    let end = g.scalar("truncation_index").expect("end") as usize;
    let centres = g.f64("centres_hz").expect("centres");
    let start = onset(&ir.iter().map(|v| v * v).collect::<Vec<_>>(), TRIGGER_DB).expect("onset");
    assert_eq!(
        start as f64,
        g.scalar("broadband_onset_index").expect("onset")
    );
    let mut edc_points = Vec::new();
    let (mut edt, mut t20, mut t30, mut c50, mut c80) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for &fm in &centres {
        let (info, s) = full_rate_band(BandFraction::Octave, fm, fs).expect("band");
        assert_eq!(info.order, 3, "{fm}: the golden set uses order 3");
        let y = filter(&ir[..end], &s, true, 0);
        let e: Vec<f64> = y.iter().map(|v| v * v).collect();
        let on = onset(&e, TRIGGER_DB).expect("band onset");
        let edc = schroeder_db(&e[on..], 0.0);
        edc_points.extend((0..edc.len()).step_by(48).map(|i| edc[i]));
        let fit = |d: Decay| {
            let (top, bottom) = d.range_db();
            decay_time(&edc, fs, top, bottom).expect("decay")
        };
        edt.push(fit(Decay::Edt));
        t20.push(fit(Decay::T20));
        t30.push(fit(Decay::T30));
        let pad = (TAIL_PER_BANDWIDTH / (info.upper_hz - info.lower_hz) * fs).ceil() as usize;
        let [a, b, _] = ratios(&ir, fs, start, end, 0.0, Some((&s, pad)));
        c50.push(a.expect("C50"));
        c80.push(b.expect("C80"));
    }
    g.assert_f64("edc_db_every_ms", &edc_points);
    g.assert_f64("edt_s", &edt);
    g.assert_f64("t20_s", &t20);
    g.assert_f64("t30_s", &t30);
    g.assert_f64("c50_db", &c50);
    g.assert_f64("c80_db", &c80);
}
