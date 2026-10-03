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
        }
    }

    fn allowed(&self, spec: WindowSpec, horizon: u32, limit_ms: f64) -> Option<f64> {
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

    // Filling: 100 s at the limit in a 600 s window; after 60 s more the window holds 160 s,
    // so anything up to the limit itself keeps it there.
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
