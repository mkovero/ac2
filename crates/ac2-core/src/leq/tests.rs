use super::*;
use crate::weighting::Weighting;
use std::f64::consts::TAU;

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

fn steady(level_dbfs: f64) -> Second {
    Second::from_levels([level_dbfs; 3], 1.0)
}

/// Brute-force reference over the whole history (newest last).
struct Brute<'a>(&'a [Second]);

impl Brute<'_> {
    /// Energy and measured time over the newest `len` slots of weighting `w`.
    fn sums(&self, w: Weighting, len: usize) -> (f64, f64) {
        let h = self.0;
        let from = h.len().saturating_sub(len);
        h[from..]
            .iter()
            .fold((0.0, 0.0), |(e, m), s| (e + s.energy(w), m + s.measured))
    }

    fn value(&self, spec: WindowSpec) -> WindowValue {
        let (e, m) = self.sums(spec.weighting, spec.seconds as usize);
        WindowValue {
            leq_dbfs: if m > 0.0 { power_dbfs(e / m) } else { f64::NAN },
            elapsed: self.0.len().min(spec.seconds as usize) as u32,
            measured: m,
            energy: e,
            seconds: spec.seconds,
        }
    }

    /// The level for the next horizon that leaves the window at the limit after it; a
    /// window filling for at least the horizon spends its budget exactly when held to the
    /// end of the fill.
    fn allowed(&self, spec: WindowSpec, horizon: u32, limit_ms: f64) -> Option<f64> {
        let rem = (spec.seconds as usize).saturating_sub(self.0.len());
        if rem >= horizon as usize {
            let (e, m) = self.sums(spec.weighting, spec.seconds as usize);
            let r = rem as f64;
            let x = (limit_ms * (m + r) - e) / r;
            return (x > 0.0).then_some(x);
        }
        let keep = spec.seconds.saturating_sub(horizon) as usize;
        if keep == 0 {
            return Some(limit_ms);
        }
        let (e, m) = self.sums(spec.weighting, keep);
        let h = f64::from(horizon);
        let x = (limit_ms * (m + h) - e) / h;
        (x > 0.0).then_some(x)
    }

    /// Smallest t in [horizon, N] after which, playing `level_ms`, the window is at or
    /// below the limit, by simulating every t.
    fn recover(&self, spec: WindowSpec, horizon: u32, limit_ms: f64, level_ms: f64) -> Option<u32> {
        let n = spec.seconds;
        (horizon.min(n)..=n).find(|&t| {
            let (e, m) = self.sums(spec.weighting, (n - t) as usize);
            let tt = f64::from(t);
            (e + tt * level_ms) / (m + tt) <= limit_ms * (1.0 + 1e-12)
        })
    }
}

fn close_db(a: f64, b: f64, tol: f64) -> bool {
    (a.is_nan() && b.is_nan()) || (a - b).abs() <= tol
}

/// Random seconds over 100 dB of dynamics with gaps and partial seconds, several windows
/// and two horizons: every value, headroom and recovery time equals the brute-force
/// sums over the history, through ring wrap-around and the periodic exact recompute.
#[test]
fn matches_brute_force_with_gaps() {
    let specs = [
        WindowSpec {
            seconds: 7,
            weighting: Weighting::A,
        },
        WindowSpec {
            seconds: 60,
            weighting: Weighting::C,
        },
        WindowSpec {
            seconds: 90,
            weighting: Weighting::Z,
        },
        WindowSpec {
            seconds: 240,
            weighting: Weighting::A,
        },
    ];
    for horizon in [5, 60] {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ u64::from(horizon));
        let mut r = RollingLeq::new(&specs, horizon);
        assert_eq!(r.capacity(), 240);
        let mut hist = Vec::new();
        for step in 0..1500 {
            let u = rng.next();
            let s = if u < 0.05 {
                Second::GAP
            } else {
                // Loud passages every few hundred seconds, then quiet.
                let base = if (step / 150) % 2 == 0 { -10.0 } else { -90.0 };
                let l = base + 20.0 * rng.next();
                let m = if u < 0.1 {
                    0.25 + 0.5 * rng.next()
                } else {
                    1.0
                };
                Second::from_levels([l, l + 3.0 * rng.next(), l + 6.0 * rng.next()], m)
            };
            r.push(s);
            hist.push(s);
            let b = Brute(&hist);
            for (i, &spec) in specs.iter().enumerate() {
                let got = r.value(i);
                let want = b.value(spec);
                assert_eq!(got.elapsed, want.elapsed, "step {step} window {i}");
                assert!(
                    (got.measured - want.measured).abs() < 1e-9,
                    "step {step} window {i}: measured {} vs {}",
                    got.measured,
                    want.measured
                );
                assert!(
                    close_db(got.leq_dbfs, want.leq_dbfs, 1e-6),
                    "step {step} window {i}: {} vs {}",
                    got.leq_dbfs,
                    want.leq_dbfs
                );
                let limit_ms = mean_square(-25.0);
                match (r.headroom(i, limit_ms), b.allowed(spec, horizon, limit_ms)) {
                    (Headroom::Allowed { ms }, Some(want)) => assert!(
                        close_db(power_dbfs(ms), power_dbfs(want), 1e-6),
                        "step {step} window {i}: allowed {ms} vs {want}"
                    ),
                    (Headroom::CannotRecover { recover_s }, None) => assert_eq!(
                        Some(recover_s),
                        b.recover(spec, horizon, limit_ms, limit_ms),
                        "step {step} window {i}"
                    ),
                    (got, want) => panic!("step {step} window {i}: {got:?} vs {want:?}"),
                }
                if step % 37 == 0 {
                    let level = mean_square(-40.0);
                    assert_eq!(
                        r.recover_time(i, limit_ms, level),
                        b.recover(spec, horizon, limit_ms, level),
                        "step {step} window {i}: recovery at −40"
                    );
                }
            }
        }
    }
}

/// A loud stretch followed by an hour at −120 dBFS: the running sums keep no residue of the
/// loud seconds once they have left (exact recompute; Neumaier between recomputes).
#[test]
fn no_drift_after_loud_then_quiet() {
    let spec = WindowSpec {
        seconds: 300,
        weighting: Weighting::Z,
    };
    let mut r = RollingLeq::new(&[spec], 60);
    for _ in 0..1000 {
        r.push(steady(0.0));
    }
    for k in 0..3600 {
        r.push(steady(-120.0));
        if k >= 300 {
            let v = r.value(0);
            assert!((v.leq_dbfs + 120.0).abs() < 1e-9, "{k}: {}", v.leq_dbfs);
        }
    }
}

#[test]
fn steady_level_and_filling() {
    let spec = WindowSpec {
        seconds: 300,
        weighting: Weighting::A,
    };
    let mut r = RollingLeq::new(&[spec], 60);
    let v = r.value(0);
    assert!(v.leq_dbfs.is_nan());
    assert_eq!((v.elapsed, v.measured), (0, 0.0));
    for k in 1..=450u32 {
        r.push(steady(-23.0));
        let v = r.value(0);
        assert!((v.leq_dbfs + 23.0).abs() < 1e-12);
        assert_eq!(v.elapsed, k.min(300));
        assert!(!v.incomplete());
    }
}

/// 30 s at L₁ then 30 s at L₂: the 60 s Leq is the energy mean, 10·lg((10^(L₁/10) +
/// 10^(L₂/10)) / 2); the 30 s window holds L₂ alone.
#[test]
fn step_change() {
    let specs = [
        WindowSpec {
            seconds: 60,
            weighting: Weighting::A,
        },
        WindowSpec {
            seconds: 30,
            weighting: Weighting::A,
        },
    ];
    let mut r = RollingLeq::new(&specs, 10);
    for _ in 0..30 {
        r.push(steady(-6.0));
    }
    for _ in 0..30 {
        r.push(steady(-16.0));
    }
    let want = 10.0 * ((10f64.powf(-0.6) + 10f64.powf(-1.6)) / 2.0).log10();
    assert!((r.value(0).leq_dbfs - want).abs() < 1e-12);
    assert!((want + 8.596).abs() < 1e-3);
    assert!((r.value(1).leq_dbfs + 16.0).abs() < 1e-12);
}

/// A gap is not silence: 50 s measured at L and 10 s lost reads L over 50 measured seconds,
/// flagged incomplete.
#[test]
fn gaps_are_not_silence() {
    let spec = WindowSpec {
        seconds: 60,
        weighting: Weighting::Z,
    };
    let mut r = RollingLeq::new(&[spec], 60);
    for k in 0..60 {
        r.push(if (20..30).contains(&k) {
            Second::GAP
        } else {
            steady(-30.0)
        });
    }
    let v = r.value(0);
    assert!((v.leq_dbfs + 30.0).abs() < 1e-12);
    assert_eq!(v.elapsed, 60);
    assert!((v.measured - 50.0).abs() < 1e-12);
    assert!(v.incomplete());
    // Once the gap has left the window it is complete again.
    for _ in 0..30 {
        r.push(steady(-30.0));
    }
    assert!(!r.value(0).incomplete());
}

/// Headroom, analytic: a full 600 s window 3 dB under the limit keeps 540 s at P/2 over a
/// 60 s horizon, so x = (600·P − 270·P) / 60 = 5.5·P (+7.40 dB); playing x for 60 s lands the
/// window exactly on the limit.
#[test]
fn headroom_lands_on_the_limit() {
    let spec = WindowSpec {
        seconds: 600,
        weighting: Weighting::A,
    };
    let limit = -20.0;
    let p = mean_square(limit);
    let mut r = RollingLeq::new(&[spec], 60);
    for _ in 0..600 {
        r.push(Second::from_levels([limit - 10.0 * 2f64.log10(); 3], 1.0));
    }
    let Headroom::Allowed { ms } = r.headroom(0, p) else {
        panic!("{:?}", r.headroom(0, p))
    };
    assert!((ms / p - 5.5).abs() < 1e-9, "{}", ms / p);
    let x = power_dbfs(ms);
    assert!((x - limit - 10.0 * 5.5f64.log10()).abs() < 1e-9);
    for _ in 0..60 {
        r.push(Second::from_levels([x; 3], 1.0));
    }
    assert!((r.value(0).leq_dbfs - limit).abs() < 1e-9);

    // Filling: 100 s at the limit in a 600 s window; the 500 s left to fill have a budget of
    // 500 s at the limit, so the limit itself, held to the end, lands on it.
    let mut r = RollingLeq::new(&[spec], 60);
    for _ in 0..100 {
        r.push(steady(limit));
    }
    let Headroom::Allowed { ms } = r.headroom(0, p) else {
        panic!()
    };
    assert!((ms / p - 1.0).abs() < 1e-12);

    // A window no longer than the horizon is replaced within it: the limit itself.
    let short = WindowSpec {
        seconds: 30,
        weighting: Weighting::A,
    };
    let mut r = RollingLeq::new(&[short], 60);
    for _ in 0..30 {
        r.push(steady(limit + 20.0));
    }
    assert_eq!(r.headroom(0, p), Headroom::Allowed { ms: p });
}

/// Over and not recoverable within the horizon: 300 s at +10 dB re the limit, then 300 s of
/// silence, in a 600 s window. At the limit the newest j slots must average ≤ P: the 300
/// silent ones plus k loud with 10·k ≤ 300 + k, k ≤ 33, so recovery takes 600 − 333 = 267 s;
/// simulated, 266 s at the limit is still over and 267 s is not.
#[test]
fn cannot_recover_and_time_to_recover() {
    let spec = WindowSpec {
        seconds: 600,
        weighting: Weighting::A,
    };
    let limit = -20.0;
    let p = mean_square(limit);
    let mut r = RollingLeq::new(&[spec], 60);
    for _ in 0..300 {
        r.push(steady(limit + 10.0));
    }
    for _ in 0..300 {
        r.push(steady(-200.0));
    }
    assert!(r.value(0).leq_dbfs > limit);
    assert_eq!(r.headroom(0, p), Headroom::CannotRecover { recover_s: 267 });
    assert_eq!(r.headroom(0, p).allowed_dbfs(), None);
    for (t, over) in [(266, true), (267, false)] {
        let mut s = r.clone();
        for _ in 0..t {
            s.push(steady(limit));
        }
        assert_eq!(s.value(0).leq_dbfs > limit + 1e-9, over, "{t}");
    }
    // Twice the limit for 300 s over the 300 silent ones averages exactly at it; four
    // times never gets there.
    assert_eq!(r.recover_time(0, p, p * 2.0), Some(300));
    assert_eq!(r.recover_time(0, p, p * 4.0), None);
    // Quieter recovers sooner.
    assert!(r.recover_time(0, p, p / 10.0).expect("recovers") < 267);
}

#[test]
fn clear_starts_over() {
    let spec = WindowSpec {
        seconds: 10,
        weighting: Weighting::C,
    };
    let mut r = RollingLeq::new(&[spec], 60);
    for _ in 0..25 {
        r.push(steady(-3.0));
    }
    r.clear();
    assert_eq!(r.pushed(), 0);
    assert!(r.value(0).leq_dbfs.is_nan());
    r.push(steady(-40.0));
    assert!((r.value(0).leq_dbfs + 40.0).abs() < 1e-12);
    assert_eq!(r.value(0).elapsed, 1);
}

#[test]
fn judgement_at_display_resolution() {
    assert_eq!(
        judge(99.96, 100.0, 3.0),
        Some(Judgement::Near),
        "shows 100.0"
    );
    assert_eq!(
        judge(100.06, 100.0, 3.0),
        Some(Judgement::Over),
        "shows 100.1"
    );
    assert_eq!(judge(97.04, 100.0, 3.0), Some(Judgement::Ok), "shows 97.0");
    assert_eq!(
        judge(97.06, 100.0, 3.0),
        Some(Judgement::Near),
        "shows 97.1"
    );
    assert_eq!(judge(90.0, 100.0, 0.0), Some(Judgement::Ok));
    assert_eq!(judge(f64::NEG_INFINITY, 100.0, 3.0), Some(Judgement::Ok));
    assert_eq!(judge(f64::NAN, 100.0, 3.0), None);
}

#[test]
fn second_from_levels_round_trips() {
    let s = Second::from_levels([-20.0, -17.5, f64::NEG_INFINITY], 0.5);
    assert!((s.level_dbfs(Weighting::A) + 20.0).abs() < 1e-12);
    assert!((s.level_dbfs(Weighting::C) + 17.5).abs() < 1e-12);
    assert_eq!(s.level_dbfs(Weighting::Z), f64::NEG_INFINITY);
    assert!(Second::GAP.level_dbfs(Weighting::A).is_nan());
}

fn sine(f: f64, amp: f64, fs: f64, from: usize, n: usize) -> Vec<f64> {
    (from..from + n)
        .map(|i| amp * (TAU * f * i as f64 / fs).sin())
        .collect()
}

/// A steady 1 kHz sine at −20 dBFS in blocks of any size: each second reads −20 dBFS in Z
/// (whole cycles per second, exact) and in A and C (0 dB at 1 kHz by definition).
#[test]
fn steady_tone_reads_its_level_per_second() {
    for fs in [44_100.0, 48_000.0, 96_000.0] {
        let mut g = SecondIntegrator::new(fs).expect("rate");
        let amp = 0.1; // −20 dBFS
        let mut out = Vec::new();
        let total = (3.0 * fs) as usize + 100;
        let mut at = 0;
        for size in [64, 1000, 4096, 333].iter().cycle() {
            if at >= total {
                break;
            }
            let n = (*size).min(total - at);
            g.process(&sine(1000.0, amp, fs, at, n), |s| out.push(s));
            at += n;
        }
        assert_eq!(out.len(), 3, "{fs}");
        assert_eq!(g.position(), 100);
        for (k, s) in out.iter().enumerate() {
            assert_eq!(s.measured, 1.0);
            let z = s.level_dbfs(Weighting::Z);
            assert!((z + 20.0).abs() < 1e-9, "{fs} s{k}: Z {z}");
            // The filters start from rest: their transient is in the first milliseconds.
            let tol = if k == 0 { 0.01 } else { 0.005 };
            for w in [Weighting::A, Weighting::C] {
                let l = s.level_dbfs(w);
                assert!((l + 20.0).abs() < tol, "{fs} s{k}: {w:?} {l}");
            }
        }
    }
}

/// Lost samples move the second grid without energy or measured time: 0.5 s measured,
/// 1.25 s lost, 0.75 s measured gives seconds measured 0.5 and 0.25, and the level over the
/// measured part is still the tone's.
#[test]
fn skip_makes_partial_seconds() {
    let fs = 48_000.0;
    let mut g = SecondIntegrator::new(fs).expect("rate");
    let mut out = Vec::new();
    g.process(&sine(1000.0, 0.1, fs, 0, 24_000), |s| out.push(s));
    g.skip(60_000, |s| out.push(s));
    g.process(&sine(1000.0, 0.1, fs, 84_000, 36_000), |s| out.push(s));
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].measured, 0.5);
    assert_eq!(out[1].measured, 0.25);
    for s in &out {
        assert!((s.level_dbfs(Weighting::Z) + 20.0).abs() < 1e-6);
    }
    assert_eq!(g.position(), 24_000);
    // Whole lost seconds come out as gaps.
    let mut out = Vec::new();
    g.skip(24_000 + 2 * 48_000, |s| out.push(s));
    assert_eq!(out.len(), 3);
    assert_eq!(out[1], Second::GAP);
    assert_eq!(out[2], Second::GAP);
}

/// A flat correction (a single unit tap) changes nothing but delays the path.
#[test]
fn correction_in_the_path() {
    let fs = 48_000.0;
    let mut g = SecondIntegrator::new(fs).expect("rate");
    g.set_correction(Some(&[1.0]));
    assert!(g.has_correction());
    let mut out = Vec::new();
    g.process(&sine(1000.0, 0.1, fs, 0, 2 * 48_000), |s| out.push(s));
    assert_eq!(out.len(), 2);
    assert!((out[1].level_dbfs(Weighting::Z) + 20.0).abs() < 1e-6);
    g.set_correction(None);
    assert!(!g.has_correction());
}

/// The total over a whole log with gaps and partial seconds, grown at the newest end and
/// trimmed at the oldest: the running total equals the brute-force energy average over the
/// seconds held, over their measured time (gaps are not silence).
#[test]
fn log_total_matches_brute_force_with_gaps_and_trimming() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut t = LogTotal::default();
    let mut held: std::collections::VecDeque<Second> = std::collections::VecDeque::new();
    assert!(t.level_dbfs(Weighting::A).is_nan());
    for step in 0..4000 {
        let u = rng.next();
        let s = if u < 0.05 {
            Second::GAP
        } else {
            let base = if (step / 300) % 2 == 0 { -5.0 } else { -95.0 };
            let l = base + 20.0 * rng.next();
            let m = if u < 0.1 { 0.2 + 0.6 * rng.next() } else { 1.0 };
            Second::from_levels([l, l + 3.0, l + 6.0], m)
        };
        t.add(&s);
        held.push_back(s);
        // Past 1000 seconds the oldest go, as a log at its retention.
        if held.len() > 1000 {
            let o = held.pop_front().expect("held");
            t.remove(&o);
        }
        if step % 7 == 0 {
            let (mut m, mut e) = (0.0, [0.0; 3]);
            for x in &held {
                m += x.measured;
                for (k, ek) in e.iter_mut().enumerate() {
                    *ek += x.energy[k];
                }
            }
            assert!((t.measured() - m).abs() < 1e-9, "step {step}");
            for (k, w) in WEIGHTINGS.into_iter().enumerate() {
                let want = power_dbfs(e[k] / m);
                assert!(
                    close_db(t.level_dbfs(w), want, 1e-6),
                    "step {step} {w:?}: {} vs {want}",
                    t.level_dbfs(w)
                );
            }
            let exact = LogTotal::of(&held);
            assert!(close_db(
                exact.level_dbfs(Weighting::A),
                t.level_dbfs(Weighting::A),
                1e-6
            ));
        }
    }
}

/// A partial second weighs by its measured time: a 0.25 s second at −10 dBFS and a whole
/// second at −30 dBFS average over 1.25 s, never over the 2 s of wall time.
#[test]
fn log_total_is_over_measured_time() {
    let a = Second::from_levels([-10.0; 3], 0.25);
    let b = Second::from_levels([-30.0; 3], 1.0);
    let t = LogTotal::of([&a, &Second::GAP, &b]);
    assert!((t.measured() - 1.25).abs() < 1e-12);
    let want = 10.0 * ((0.25 * 10f64.powf(-1.0) + 10f64.powf(-3.0)) / 1.25).log10();
    assert!((t.level_dbfs(Weighting::A) - want).abs() < 1e-9);
}

fn window(seconds: u32) -> WindowSpec {
    WindowSpec {
        seconds,
        weighting: Weighting::A,
    }
}

/// Judges every window of `r` against a limit `limit` dBFS (no offset), margin 3 dB.
fn verdicts(r: &RollingLeq, limit: f64) -> Vec<Verdict> {
    (0..r.specs().count())
        .map(|i| judge_window(&r.value(i), 0.0, limit, 3.0).expect("a value"))
        .collect()
}

const OVER: Verdict = Verdict {
    judgement: Judgement::Over,
    on_course: false,
};
const ON_COURSE: Verdict = Verdict {
    judgement: Judgement::Near,
    on_course: true,
};
const OK: Verdict = Verdict {
    judgement: Judgement::Ok,
    on_course: false,
};
const NEAR: Verdict = Verdict {
    judgement: Judgement::Near,
    on_course: false,
};

/// A fresh log, 10 dB over the limit steadily: ten times the limit's power spends a
/// window's budget in a tenth of its length. The 1 min window is on course at once; at 6 s its
/// energy equals the budget (60·P·s: the least level shows the limit itself, not over it) and
/// at 7 s it is over. The 60 min one spends its budget at 6 min and is over once the least
/// level shows above the limit at 0.1 dB: 6 min 5 s (3650·P·s, +0.06 dB; 6 min 4 s is
/// +0.048, shown as the limit). "Over in" counts down to the budget.
#[test]
fn a_fresh_log_10_db_over_spends_its_budget_in_a_tenth() {
    let limit = -30.0;
    let p = mean_square(limit);
    let mut r = RollingLeq::new(&[window(60), window(3600)], 60);
    r.push(steady(limit + 10.0));
    assert_eq!(verdicts(&r, limit), [ON_COURSE, ON_COURSE]);
    let v = r.value(0);
    assert!((v.leq_dbfs - limit - 10.0).abs() < 1e-9, "the Leq so far");
    assert!((v.over_in(p).expect("on course") - 5.0).abs() < 1e-9);
    assert!((r.value(1).over_in(p).expect("on course") - 359.0).abs() < 1e-6);
    for k in 2..=3600u32 {
        r.push(steady(limit + 10.0));
        let want = |t: u32| if k >= t { OVER } else { ON_COURSE };
        assert_eq!(verdicts(&r, limit), [want(7), want(365)], "after {k} s");
        if k < 6 {
            let t = r.value(0).over_in(p).expect("on course");
            assert!((t - f64::from(6 - k)).abs() < 1e-9, "{k}: {t}");
        }
    }
    // The 60 s budget spent at 6 s: the least level reaches the limit there.
    let mut r = RollingLeq::new(&[window(60)], 60);
    for _ in 0..6 {
        r.push(steady(limit + 10.0));
    }
    assert!((r.value(0).least_dbfs() - limit).abs() < 1e-9);
    assert_eq!(r.value(0).over_in(p), Some(0.0));
}

/// Quiet, then loud: a filling window ok while quiet, on course once the Leq so far passes
/// the limit, over only when the energy passes the whole window's budget.
#[test]
fn quiet_then_loud_while_filling() {
    let limit = -30.0;
    let mut r = RollingLeq::new(&[window(600)], 60);
    for _ in 0..300 {
        r.push(steady(limit - 20.0));
    }
    assert_eq!(verdicts(&r, limit), [OK]);
    // 300 s at P/100 = 3·P·s, then 100·P a second: the Leq so far is 103/301·P after one (ok),
    // within the margin after two, at the limit after three, above it after four; the
    // budget (600·P·s) is spent after 6 (603: shown at the limit) and passed after 7.
    let mut seen = Vec::new();
    for _ in 0..7 {
        r.push(steady(limit + 20.0));
        seen.push(verdicts(&r, limit)[0]);
    }
    assert_eq!(
        seen,
        [OK, NEAR, NEAR, ON_COURSE, ON_COURSE, ON_COURSE, OVER]
    );
}

/// Gaps: they earn no budget and spend none, as they add nothing to the Leq. 30 s lost and
/// 30 s measured at the limit in a 120 s window: the budget is P over the 30 measured
/// seconds and the 60 to come; the Leq so far is at the limit (near, not on course), and
/// 1 dB more for the rest ends over it.
#[test]
fn gaps_neither_earn_nor_spend_budget() {
    let limit = -30.0;
    let p = mean_square(limit);
    let mut r = RollingLeq::new(&[window(120)], 10);
    for k in 0..60 {
        r.push(if k < 30 { Second::GAP } else { steady(limit) });
    }
    let v = r.value(0);
    assert!(v.incomplete() && v.filling());
    assert_eq!(v.remaining(), 60);
    assert!((v.least_dbfs() - (limit + 10.0 * (30.0f64 / 90.0).log10())).abs() < 1e-9);
    assert_eq!(verdicts(&r, limit), [NEAR]);
    assert_eq!(
        v.over_in(p),
        None,
        "at the limit the budget lasts the window"
    );
    let Headroom::Allowed { ms } = r.headroom(0, p) else {
        panic!("{:?}", r.headroom(0, p))
    };
    assert!((ms / p - 1.0).abs() < 1e-12, "the limit for the 60 s left");
    for _ in 0..60 {
        r.push(steady(limit + 1.0));
    }
    assert_eq!(verdicts(&r, limit)[0].judgement, Judgement::Over);
}

/// Once full, the budget rule is the rolling one: the same judgement as [`judge`] on the
/// Leq, whatever the history; while filling, over only with the budget spent.
#[test]
fn full_windows_judged_on_their_leq() {
    let mut rng = Rng(0x5eed);
    let limit = -30.0;
    let mut r = RollingLeq::new(&[window(30), window(90)], 10);
    for _ in 0..400 {
        let l = limit - 8.0 + 14.0 * rng.next();
        r.push(if rng.next() < 0.05 {
            Second::GAP
        } else {
            steady(l)
        });
        for i in 0..2 {
            let v = r.value(i);
            let got = judge_window(&v, 0.0, limit, 3.0);
            if v.filling() {
                if got.is_some_and(|g| g.judgement == Judgement::Over) {
                    assert!(round_tenth(v.least_dbfs()) > limit);
                }
            } else {
                assert_eq!(got.map(|g| g.judgement), judge(v.leq_dbfs, limit, 3.0));
                assert!(!got.is_some_and(|g| g.on_course));
                assert_eq!(v.least_dbfs(), v.leq_dbfs);
            }
        }
    }
}

/// Over while filling is a certainty: a window judged over stays over until full whatever
/// is played (silence here), and its headroom says it cannot recover.
#[test]
fn over_while_filling_stays_over_until_full() {
    let limit = -30.0;
    let p = mean_square(limit);
    let mut r = RollingLeq::new(&[window(300)], 60);
    for _ in 0..40 {
        r.push(steady(limit + 10.0));
    }
    assert_eq!(verdicts(&r, limit), [OVER]);
    assert!(matches!(r.headroom(0, p), Headroom::CannotRecover { .. }));
    for _ in 40..300 {
        r.push(steady(-200.0));
        assert_eq!(verdicts(&r, limit), [OVER]);
    }
    // Full and sliding: back under once enough loud seconds have left.
    for _ in 0..60 {
        r.push(steady(-200.0));
    }
    assert_eq!(verdicts(&r, limit), [OK]);
}

/// A log with every kind of hole: seconds lost while running, short pauses (shorter than
/// the longest window) and a long one, partial seconds; wall times jitter a little.
fn holey_log() -> Vec<(u64, Second)> {
    let t0 = 1_790_000_000 * NS;
    let mut rng = Rng(0x1eb_0011);
    let mut rows = Vec::new();
    let mut t = t0;
    for k in 0..4000u64 {
        t += match k {
            0 => 0,
            // Pauses: 40 s, 80 s, 200 s (shorter than the 300 s window), 400 s (longer).
            700 => 41 * NS,
            1500 => 81 * NS,
            1530 => 201 * NS,
            2600 => 401 * NS,
            _ if k % 211 == 0 => 2 * NS,
            _ => NS,
        };
        let loud = if (1200..1400).contains(&k) { 20.0 } else { 0.0 };
        let level = -40.0 + 25.0 * rng.next() + loud;
        let measured = if k % 53 == 0 { 0.4 } else { 1.0 };
        let jitter = (rng.next() * 0.02 * NS as f64) as u64;
        rows.push((
            t + jitter,
            Second::from_levels([level, level + 3.0, level + 5.0], measured),
        ));
    }
    rows
}

fn replay_specs() -> Vec<WindowSpec> {
    [
        (10, Weighting::A),
        (60, Weighting::C),
        (120, Weighting::A),
        (300, Weighting::Z),
    ]
    .map(|(seconds, weighting)| WindowSpec { seconds, weighting })
    .to_vec()
}

/// The job as the daemon runs it: started on the log at the first row and after every
/// stretch without rows (the meter stopped), refilling its windows from the rows before;
/// otherwise every second pushed, a lost one as a gap.
fn job_values(rows: &[(u64, Second)], specs: &[WindowSpec]) -> Vec<Vec<WindowValue>> {
    let mut ring = RollingLeq::new(specs, 30);
    let mut out = Vec::new();
    for (i, &(start, s)) in rows.iter().enumerate() {
        let missing = (i > 0).then(|| {
            let step = (start - rows[i - 1].0 + NS / 2) / NS;
            step.max(1) - 1
        });
        if missing != Some(0) {
            ring.refill(rows[..i].iter().rev().copied(), start);
        }
        ring.push(s);
        out.push((0..specs.len()).map(|w| ring.value(w)).collect());
    }
    out
}

fn same_value(a: &WindowValue, b: &WindowValue) -> bool {
    a.elapsed == b.elapsed
        && a.seconds == b.seconds
        && (a.measured - b.measured).abs() < 1e-9
        && close_db(a.leq_dbfs, b.leq_dbfs, 1e-9)
        && close_db(a.least_dbfs(), b.least_dbfs(), 1e-9)
}

/// Replayed from the log, every window reads what the job computed, second by second,
/// through lost seconds, pauses shorter than the windows (refilled, their elapsed time
/// counted from the oldest row in span) and a longer one (empty windows).
#[test]
fn replay_matches_the_job() {
    let rows = holey_log();
    let specs = replay_specs();
    let job = job_values(&rows, &specs);
    let mut r = LogReplay::new(&specs, 30, true);
    for (i, &(start, s)) in rows.iter().enumerate() {
        r.push(start, s);
        assert!(r.settled());
        for (w, want) in job[i].iter().enumerate() {
            let got = r.windows().value(w);
            assert!(
                same_value(&got, want),
                "row {i} window {w}: {got:?} vs {want:?}"
            );
            assert_eq!(
                judge_window(&got, 0.0, -20.0, 3.0),
                judge_window(want, 0.0, -20.0, 3.0),
                "row {i} window {w}"
            );
        }
    }
    // After the 400 s pause every window fills again from nothing.
    assert!(job[2600].iter().all(|v| v.elapsed == 1));
    // The 200 s pause 30 s after an 80 s one: the 300 s window's span starts in the 80 s
    // pause, so it counts as filling from the oldest row in it.
    assert!(job[1530][3].elapsed < 300, "{:?}", job[1530][3]);
    assert!(job[1530][3].filling());
}

/// Begun part way through a log, the replay is settled (and then equal to the job) once
/// the longest window holds only rows it was given.
#[test]
fn replay_from_the_middle_settles_after_the_longest_window() {
    let rows = holey_log();
    let specs = replay_specs();
    let job = job_values(&rows, &specs);
    let from = 2000;
    let mut r = LogReplay::new(&specs, 30, false);
    let mut settled_at = None;
    for (i, &(start, s)) in rows.iter().enumerate().skip(from) {
        r.push(start, s);
        if !r.settled() {
            continue;
        }
        settled_at.get_or_insert(i);
        for (w, want) in job[i].iter().enumerate() {
            let got = r.windows().value(w);
            assert!(
                same_value(&got, want),
                "row {i} window {w}: {got:?} vs {want:?}"
            );
        }
    }
    let at = settled_at.expect("settles");
    assert!(at - from <= 300, "{at}");
}
