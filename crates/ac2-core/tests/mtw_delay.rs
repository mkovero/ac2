//! MTW delay changes without a full resettle, and sub-sample delays, against analytic
//! systems (`docs/design/delay-no-resettle.md`).
//!
//! The inputs are periodic and band-limited (period 2^19 samples, nothing at Nyquist), built
//! in the frequency domain: `Y_k = X_k · H(f_k)` with `H` a peaking biquad times an exact
//! fractional delay `e^{−j2πfτ/fs}`. Every sample of `y` is then exactly the system's
//! steady-state output, for any τ, so the expected transfer function at alignment `D` is
//! `H_eq(f) · e^{−j2πf(τ − D)/fs}` with no approximation in the test signal itself.

use ac2_core::grid::LogGrid;
use ac2_core::mtw::{
    Averaging, ColumnSource, DelayChange, DepthPolicy, Ladder, Mtw, MtwConfig, MtwFrame,
    SampleGate, Validity,
};
use num_complex::Complex64;
use realfft::RealFftPlanner;
use std::f64::consts::PI;

const FS: f64 = 48_000.0;
const PERIOD: usize = 1 << 19;

/// Deterministic Gaussian noise (splitmix64 + Box–Muller).
struct Noise(u64);

impl Noise {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
}

/// RBJ peaking EQ response at `f`.
fn peaking(f0: f64, q: f64, gain_db: f64, f: f64) -> Complex64 {
    let a_lin = 10f64.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * f0 / FS;
    let alpha = w0.sin() / (2.0 * q);
    let b = [1.0 + alpha * a_lin, -2.0 * w0.cos(), 1.0 - alpha * a_lin];
    let a = [1.0 + alpha / a_lin, -2.0 * w0.cos(), 1.0 - alpha / a_lin];
    let z1 = Complex64::from_polar(1.0, -2.0 * PI * f / FS);
    let z2 = z1 * z1;
    (b[0] + b[1] * z1 + b[2] * z2) / (a[0] + a[1] * z1 + a[2] * z2)
}

/// The system without its delay: two peaking filters, one on each side of the low crossover.
fn eq(f: f64) -> Complex64 {
    peaking(150.0, 1.4, 6.0, f) * peaking(3_000.0, 2.0, -6.0, f)
}

/// The system under test: a pure delay, or the two peaking filters and a delay.
#[derive(Clone, Copy, Debug)]
enum System {
    Delay,
    EqDelay,
}

impl System {
    /// Response without the delay.
    fn h(self, f: f64) -> Complex64 {
        match self {
            System::Delay => Complex64::new(1.0, 0.0),
            System::EqDelay => eq(f),
        }
    }
}

/// One period of reference and measurement for `y = sys * x` delayed by `tau` samples.
fn periodic_pair(sys: System, tau: f64, seed: u64) -> (Vec<f64>, Vec<f64>) {
    let mut planner = RealFftPlanner::<f64>::new();
    let inv = planner.plan_fft_inverse(PERIOD);
    let mut n = Noise(seed);
    let bins = PERIOD / 2 + 1;
    let mut xs: Vec<Complex64> = (0..bins)
        .map(|k| {
            if k == 0 || k == bins - 1 {
                Complex64::new(0.0, 0.0)
            } else {
                Complex64::new(n.gauss(), n.gauss())
            }
        })
        .collect();
    let mut ys: Vec<Complex64> = xs
        .iter()
        .enumerate()
        .map(|(k, x)| {
            let f = k as f64 * FS / PERIOD as f64;
            x * sys.h(f) * Complex64::from_polar(1.0, -2.0 * PI * f * tau / FS)
        })
        .collect();
    let scale = 0.1 / (PERIOD as f64).sqrt();
    let mut x = inv.make_output_vec();
    let mut y = inv.make_output_vec();
    inv.process(&mut xs, &mut x).expect("ifft");
    inv.process(&mut ys, &mut y).expect("ifft");
    x.iter_mut().for_each(|v| *v *= scale);
    y.iter_mut().for_each(|v| *v *= scale);
    (x, y)
}

/// A periodic pair streamed into an engine from input index `next`.
struct Feed {
    x: Vec<f64>,
    y: Vec<f64>,
    next: u64,
}

impl Feed {
    fn new(sys: System, tau: f64, seed: u64) -> Self {
        let (x, y) = periodic_pair(sys, tau, seed);
        Self { x, y, next: 0 }
    }

    /// Push `n` samples in irregular chunks.
    fn push(&mut self, m: &mut Mtw, n: usize) -> (usize, usize) {
        let sizes = [997usize, 64, 4096, 1, 333, 12_000, 256];
        let (mut acc, mut rej) = (0, 0);
        let mut done = 0;
        let mut c = 0;
        let (mut bx, mut by) = (Vec::new(), Vec::new());
        while done < n {
            let len = sizes[c % sizes.len()].min(n - done);
            bx.clear();
            by.clear();
            for i in 0..len {
                let p = ((self.next + i as u64) % PERIOD as u64) as usize;
                bx.push(self.x[p]);
                by.push(self.y[p]);
            }
            let o = m
                .push(self.next, &bx, &by, SampleGate::Accept)
                .expect("push");
            acc += o.blocks_accumulated;
            rej += o.blocks_rejected;
            assert!(!o.restarted);
            self.next += len as u64;
            done += len;
            c += 1;
        }
        (acc, rej)
    }
}

fn engine(averaging: Averaging, delay: f64) -> Mtw {
    Mtw::new(MtwConfig {
        sample_rate_hz: FS,
        ladder: Ladder::Standard,
        averaging,
        depth: DepthPolicy::EqualConfidence,
        grid: LogGrid::covering(48, 20.0, 20_000.0),
        delay_samples: delay,
    })
    .expect("engine")
}

/// Full-rate samples until every stage's FIFO is full.
fn fill(m: &Mtw) -> usize {
    m.stage_depths()
        .iter()
        .map(|d| (d.fill_s * FS).ceil() as usize)
        .max()
        .expect("stages")
        + 1
}

/// Errors of one stage's per-bin H1 against the analytic system at the residual delay
/// `tau − D`, over the bins the stage serves (DC excluded).
#[derive(Debug, Default)]
struct BinErr {
    /// Worst |phase error|, degrees.
    deg: f64,
    /// Worst |magnitude error|, dB.
    db: f64,
    /// Mean magnitude error, dB (a bias shows here; scatter averages out).
    mean_db: f64,
    /// RMS phase error, degrees.
    rms_deg: f64,
}

fn stage_err(m: &Mtw, s: usize, sys: System, residual: f64) -> BinErr {
    let spec = &m.layout().stages[s];
    let mut h = Vec::new();
    assert!(m.stage_h1_into(s, &mut h), "stage {s} holds blocks");
    let top = (spec.served_hi_hz / spec.bin_hz).floor() as usize;
    let mut e = BinErr::default();
    let mut n = 0.0;
    for (k, v) in h.iter().enumerate().take(top + 1).skip(1) {
        let f = k as f64 * spec.bin_hz;
        let want = sys.h(f) * Complex64::from_polar(1.0, -2.0 * PI * f * residual / FS);
        let r = v / want;
        let deg = r.arg().to_degrees();
        let db = 20.0 * r.norm().log10();
        e.deg = e.deg.max(deg.abs());
        e.db = e.db.max(db.abs());
        e.mean_db += db;
        e.rms_deg += deg * deg;
        n += 1.0;
    }
    e.mean_db /= n;
    e.rms_deg = (e.rms_deg / n).sqrt();
    e
}

/// Worst per-bin errors over every stage.
fn worst_err(m: &Mtw, sys: System, residual: f64) -> (f64, f64) {
    (0..m.layout().stages.len())
        .map(|s| stage_err(m, s, sys, residual))
        .fold((0.0, 0.0), |(d, b), e| (d.max(e.deg), b.max(e.db)))
}

/// Settling and valid columns, and the lowest coherence among the valid ones.
fn columns(f: &MtwFrame) -> (usize, usize, f64) {
    let mut r = (0, 0, 1.0f64);
    for (c, g) in f.columns.iter().zip(&f.coherence) {
        match c.validity {
            Validity::Settling => r.0 += 1,
            Validity::Valid => {
                r.1 += 1;
                r.2 = r.2.min(*g);
            }
            _ => {}
        }
    }
    r
}

/// `Σ w[n]·w[n+r] / Σ w²` for the stage window (Hann, 4096).
fn hann_rho(r: usize) -> f64 {
    let n = 4096;
    let w: Vec<f64> = (0..n)
        .map(|i| (PI * i as f64 / n as f64).sin().powi(2))
        .collect();
    let e2: f64 = w.iter().map(|v| v * v).sum();
    w.iter().zip(&w[r..]).map(|(a, b)| a * b).sum::<f64>() / e2
}

/// A pure-delay system with a fractional delay, aligned exactly: the whole-sample part in
/// the time domain, the fraction as a rotation of each block's cross-spectrum. Per bin of
/// every stage up to its served band (0.45·fs at full rate) the phase is flat within 0.1°
/// and the magnitude within 0.01 dB: what remains is the ≤ ½-sample offset of the two
/// windows, a scatter of a few hundredths of a degree. Aligned to the nearest whole sample
/// instead, the fraction's linear phase stays (0.3 samples: 48.6° at 0.45·fs).
#[test]
fn fractional_delay_is_flat_to_045_of_each_stage_rate() {
    let sys = System::Delay;
    for tau in [37.3, -12.7, 480.5, 0.25] {
        let mut feed = Feed::new(sys, tau, 3);
        let mut m = engine(Averaging::Fifo { blocks: 8 }, tau);
        let n = fill(&m);
        feed.push(&mut m, n);
        let (deg, db) = worst_err(&m, sys, 0.0);
        let (settling, valid, coh) = columns(&m.frame());
        println!("τ {tau}: per bin {deg:.4}° {db:.5} dB; {valid} columns, γ²min {coh:.6}");
        assert!(deg < 0.1 && db < 0.01, "τ {tau}: {deg}° {db} dB");
        assert!(settling == 0 && valid > 380 && coh > 0.9999);
    }
    let mut feed = Feed::new(sys, 37.3, 3);
    let mut m = engine(Averaging::Fifo { blocks: 8 }, 37.0);
    let n = fill(&m);
    feed.push(&mut m, n);
    let (deg, _) = worst_err(&m, sys, 0.0);
    println!("τ 37.3 aligned at 37: {deg:.1}°");
    assert!((deg - 0.3 * 0.45 * 360.0).abs() < 0.5);
    // ... and it is exactly the fraction's phase.
    assert!(worst_err(&m, sys, 0.3).0 < 0.1);
}

/// Nudges of ±1 and ±0.3 samples: every stage keeps its averages and the frame moves at
/// once, with no sample pushed. Each bin's H1 is the old one times `e^{j2πfΔ/fs}` to
/// rounding, so a pure delay reads its new residual per bin within 0.1° and 0.01 dB, and
/// no column goes back to settling. Fresh blocks at the new alignment then take over.
#[test]
fn nudges_move_the_curve_at_once_without_settling() {
    let tau = 480.0;
    let sys = System::Delay;
    for averaging in [
        Averaging::Fifo { blocks: 8 },
        Averaging::Exponential {
            time_constant_s: 0.3,
        },
    ] {
        let mut feed = Feed::new(sys, tau, 5);
        let mut m = engine(averaging, tau);
        let n = fill(&m);
        feed.push(&mut m, n);
        let mut d = tau;
        for step in [1.0, -1.0, -1.0, 0.3, -0.3, -0.3, 1.3] {
            let before: Vec<Vec<Complex64>> = (0..3)
                .map(|s| {
                    let mut h = Vec::new();
                    assert!(m.stage_h1_into(s, &mut h));
                    h
                })
                .collect();
            d += step;
            let change = m.set_delay(d);
            assert_eq!(
                change,
                DelayChange {
                    restarted: false,
                    kept: 0b111
                },
                "{averaging:?} step {step}"
            );
            for (s, old) in before.iter().enumerate() {
                let spec = &m.layout().stages[s];
                let mut h = Vec::new();
                assert!(m.stage_h1_into(s, &mut h));
                for (k, (a, b)) in h.iter().zip(old).enumerate() {
                    let rot = Complex64::from_polar(
                        1.0,
                        2.0 * PI * k as f64 * step / (spec.factor * spec.nfft) as f64,
                    );
                    assert!((a - b * rot).norm() <= 1e-12 * b.norm().max(1e-12));
                }
            }
            let (deg, db) = worst_err(&m, sys, tau - d);
            let (settling, valid, coh) = columns(&m.frame());
            println!(
                "{averaging:?} D {d:.1} at once: {deg:.4}° {db:.5} dB, {valid} columns, γ²min {coh:.6}"
            );
            assert!(deg < 0.1 && db < 0.01);
            // A residual delay turns the phase across a wide column's bins, so its γ² is a
            // little below 1 however the curve was reached.
            assert!(settling == 0 && valid > 380 && coh > 0.9995);
            feed.push(&mut m, 30_000);
            let (deg, db) = worst_err(&m, sys, tau - d);
            assert!(deg < 0.1 && db < 0.01 && columns(&m.frame()).0 == 0);
        }
    }
}

/// The same with a system that is not a pure delay: right after a 1-sample and a 0.3-sample
/// nudge the per-bin scatter against the analytic response is no larger than a fresh
/// measurement's settled at the new delay (no bias added), and the mean magnitude error
/// stays within 0.002 dB.
#[test]
fn nudged_eq_system_reads_like_a_fresh_measurement() {
    let tau = 480.0;
    let sys = System::EqDelay;
    let averaging = Averaging::Fifo { blocks: 8 };
    let mut feed = Feed::new(sys, tau, 21);
    let mut m = engine(averaging, tau);
    let n = fill(&m);
    feed.push(&mut m, n);
    for d in [481.0, 480.7] {
        m.set_delay(d);
        let mut fresh = engine(averaging, d);
        let mut ffeed = Feed::new(sys, tau, 21);
        let n = 2 * fill(&fresh);
        ffeed.push(&mut fresh, n);
        for s in 0..3 {
            let a = stage_err(&m, s, sys, tau - d);
            let b = stage_err(&fresh, s, sys, tau - d);
            println!(
                "D {d} stage {s}: nudged rms {:.4}° mean {:.5} dB; fresh rms {:.4}° mean {:.5} dB",
                a.rms_deg, a.mean_db, b.rms_deg, b.mean_db
            );
            assert!(a.rms_deg < 1.5 * b.rms_deg + 0.01, "stage {s}");
            assert!(
                a.mean_db.abs() < 0.002 && b.mean_db.abs() < 0.002,
                "stage {s}"
            );
        }
    }
}

/// A whole-sample change splices the reference: the blocks straddling the splice are
/// dropped, never averaged as a mix of two alignments; a change of the fraction alone needs
/// no splice and drops nothing.
#[test]
fn whole_sample_changes_drop_straddling_blocks() {
    let sys = System::Delay;
    let mut feed = Feed::new(sys, 100.0, 7);
    let mut m = engine(Averaging::Fifo { blocks: 8 }, 100.0);
    let n = fill(&m);
    feed.push(&mut m, n);
    m.set_delay(100.4);
    let (acc, rej) = feed.push(&mut m, 60_000);
    assert!(
        acc > 0 && rej == 0,
        "fraction only: {acc} accumulated, {rej} rejected"
    );
    m.set_delay(101.4);
    let (acc, rej) = feed.push(&mut m, 60_000);
    println!("whole-sample change: {acc} accumulated, {rej} rejected");
    // Blocks holding the splice: 2 at full rate (50 % overlap), 4 at 75 %, 8 at 87.5 %.
    assert_eq!(rej, 14);
    let (deg, db) = worst_err(&m, sys, -1.4);
    assert!(deg < 0.1 && db < 0.01, "{deg}° {db} dB");
}

/// Correcting a delay 300 samples off: the held full-rate blocks are misaligned beyond what
/// that stage keeps (113 samples), so only it starts over; the 12 kHz and 4 kHz stages (452
/// and 1356) keep their averages. Their held blocks carry the window correlation of their
/// old misalignment (75 and 25 stage samples) as a magnitude bias, within 0.01 dB of the
/// model. Beyond every stage's limit the ladder restarts.
#[test]
fn large_changes_reset_only_the_stages_that_cannot_keep() {
    let tau = 2_000.0;
    let sys = System::Delay;
    let mut feed = Feed::new(sys, tau, 9);
    let mut m = engine(Averaging::Fifo { blocks: 8 }, tau - 300.0);
    assert_eq!(
        (0..3).map(|s| m.keep_lag(s)).collect::<Vec<_>>(),
        vec![113.0, 452.0, 1356.0]
    );
    let n = fill(&m);
    feed.push(&mut m, n);
    let change = m.set_delay(tau);
    assert_eq!(
        change,
        DelayChange {
            restarted: false,
            kept: 0b110
        }
    );
    let f = m.frame();
    // The columns the full-rate stage serves (alone or in its blend) settle; the rest stay.
    for i in 0..f.freq_hz.len() {
        let full_rate = match f.columns[i].source {
            ColumnSource::Stage(b) => b.stage == 0,
            ColumnSource::Blend { shallow, .. } => shallow.stage == 0,
            ColumnSource::None => false,
        };
        let settling = f.columns[i].validity == Validity::Settling;
        assert_eq!(settling, full_rate, "{:.0} Hz", f.freq_hz[i]);
    }
    for (s, r) in [(1, 75), (2, 25)] {
        let e = stage_err(&m, s, sys, 0.0);
        let model = 20.0 * hann_rho(r).log10();
        println!(
            "stage {s} kept: mean {:.4} dB (model {model:.4}), rms {:.3}°",
            e.mean_db, e.rms_deg
        );
        assert!((e.mean_db - model).abs() < 0.01, "stage {s}");
    }
    // The full-rate stage refills in its own fill time; the kept stages' old blocks leave
    // in theirs (plus the blocks dropped at the splice).
    let n = 2 * fill(&m);
    feed.push(&mut m, n);
    let (deg, db) = worst_err(&m, sys, 0.0);
    assert!(deg < 0.1 && db < 0.01 && columns(&m.frame()).0 == 0);
    // 2000 samples: beyond the deepest stage's limit.
    assert!(m.set_delay(tau + 2_000.0).restarted);
    assert!(columns(&m.frame()).1 == 0);
}

/// The keep limit is a bound: correcting a delay by exactly the full-rate stage's 113
/// samples keeps every stage, and the held full-rate blocks then read with the bias the
/// window correlation predicts (−0.045 dB) and a mean γ² of at least
/// KEEP_MIN_WINDOW_CORRELATION² (0.990) less the estimator's spread; 114 samples resets the
/// stage.
#[test]
fn keep_limit_bounds_the_bias() {
    let tau = 1_000.0;
    let sys = System::Delay;
    for (off, kept) in [(113usize, 0b111u32), (114, 0b110)] {
        let mut feed = Feed::new(sys, tau, 13);
        let mut m = engine(Averaging::Fifo { blocks: 8 }, tau - off as f64);
        let n = fill(&m);
        feed.push(&mut m, n);
        let change = m.set_delay(tau);
        assert_eq!(change.kept, kept, "{off}");
        if kept & 1 == 0 {
            continue;
        }
        let e = stage_err(&m, 0, sys, 0.0);
        let model = 20.0 * hann_rho(off).log10();
        let f = m.frame();
        let full: Vec<f64> = (0..f.freq_hz.len())
            .filter(|&i| {
                f.columns[i].validity == Validity::Valid
                    && matches!(f.columns[i].source, ColumnSource::Stage(b) if b.stage == 0)
            })
            .map(|i| f.coherence[i])
            .collect();
        let mean_coh = full.iter().sum::<f64>() / full.len() as f64;
        println!(
            "corrected by {off}: mean {:.4} dB (model {model:.4}), rms {:.3}°, mean γ² {mean_coh:.4}",
            e.mean_db, e.rms_deg
        );
        assert!((e.mean_db - model).abs() < 0.01 && model > -0.05);
        assert!(mean_coh > 0.985);
    }
}

/// The held blocks' alignments are tracked until they leave the average: two quick
/// 100-sample steps reset the full-rate stage (its oldest blocks are then 200 samples off),
/// but once its average has turned over (FIFO) or decayed (exponential), another 100-sample
/// step keeps it again.
#[test]
fn held_alignments_are_tracked_until_they_leave_the_average() {
    let sys = System::Delay;
    for averaging in [
        Averaging::Fifo { blocks: 8 },
        Averaging::Exponential {
            time_constant_s: 0.2,
        },
    ] {
        let mut feed = Feed::new(sys, 5_000.0, 17);
        let mut m = engine(averaging, 4_700.0);
        let n = fill(&m);
        feed.push(&mut m, n);
        assert_eq!(m.set_delay(4_800.0).kept, 0b111, "{averaging:?}");
        feed.push(&mut m, 2_000);
        assert_eq!(m.set_delay(4_900.0).kept, 0b110, "{averaging:?}");
        feed.push(&mut m, (2.0 * FS) as usize);
        assert_eq!(m.set_delay(5_000.0).kept, 0b111, "{averaging:?}");
        let n = 2 * fill(&m);
        feed.push(&mut m, n);
        let (deg, db) = worst_err(&m, sys, 0.0);
        assert!(deg < 0.1 && db < 0.01, "{averaging:?}: {deg}° {db} dB");
    }
}
