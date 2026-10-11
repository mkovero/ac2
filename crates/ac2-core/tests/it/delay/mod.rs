//! Delay finder: the design-Q1 fixture acceptance table (§13), a cross-check against the
//! reference prototype `tools/experiments/q1/finder.py`, index/sign invariance, refusal
//! paths, tracking through the stream API, and a (non-failing) timing print.

use std::f64::consts::PI;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use ac2_core::delay::{
    Agreement, AmbiguityReason, Arrival, Band, Block, Confidence, DelayStream, FinderConfig,
    FinderResult, FinderScratch, NoEstimateReason, Outcome, SearchRange, Tracker, find, find_auto,
    find_with,
};
use ac2_testkit::golden::GoldenSet;
use realfft::RealFftPlanner;
use realfft::num_complex::Complex64;

const FS: f64 = 48_000.0;

const SETS: [&str; 6] = [
    "delay_finder_full_music",
    "delay_finder_full_narrow",
    "delay_finder_full_periodic",
    "delay_finder_full_pink",
    "delay_finder_mid_pink",
    "delay_finder_sub_pink",
];

mod fixtures;
mod scenes;
mod timing;
mod tracking;

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Accepted,
    Ambiguous,
    AcceptedOrAmbiguous,
    NoEstimate,
}

#[derive(Debug)]
struct CaseRun {
    set: String,
    case: String,
    expect: Expect,
    acceptable: Vec<f64>,
    span: Option<(f64, f64)>,
    reasons_any: Vec<String>,
    tol: f64,
    result: FinderResult,
    elapsed: Duration,
}

fn band_of(name: &str) -> Band {
    match name {
        "full" => Band::FullRange,
        "mid" => Band::Mid,
        "sub" => Band::Sub,
        other => panic!("unknown band {other}"),
    }
}

fn f64s(v: &serde_json::Value) -> Vec<f64> {
    v.as_array()
        .map(|a| a.iter().filter_map(serde_json::Value::as_f64).collect())
        .unwrap_or_default()
}

/// Every case of every set, run once per test binary.
fn fixture_runs() -> &'static [CaseRun] {
    static RUNS: OnceLock<Vec<CaseRun>> = OnceLock::new();
    RUNS.get_or_init(|| {
        let mut out = Vec::new();
        let mut scratch = FinderScratch::new();
        for set in SETS {
            let gs = GoldenSet::load(set).expect("golden set");
            let fs = gs.parameter("fs_hz").and_then(|v| v.as_f64()).expect("fs");
            let band = band_of(gs.parameter("band").and_then(|v| v.as_str()).expect("band"));
            let ref_start = gs
                .parameter("ref_start")
                .and_then(|v| v.as_f64())
                .expect("ref_start") as u64;
            let r: Vec<f32> = gs
                .f64("ref")
                .expect("ref")
                .iter()
                .map(|&v| v as f32)
                .collect();
            let cases = gs
                .parameter("cases")
                .and_then(|v| v.as_array())
                .expect("cases");
            for c in cases {
                let name = c["name"].as_str().expect("name").to_owned();
                let sc = |k: &str| gs.scalar(&format!("{name}.{k}")).expect("scalar");
                let m: Vec<f32> = gs
                    .f64(&format!("meas.{name}"))
                    .expect("meas")
                    .iter()
                    .map(|&v| v as f32)
                    .collect();
                let mut cfg = FinderConfig::new(fs, band);
                cfg.search = SearchRange {
                    min: sc("search_min") as i64,
                    max: sc("search_max") as i64,
                };
                let t0 = Instant::now();
                let result = find_with(
                    &mut scratch,
                    Block {
                        start: ref_start,
                        samples: &r,
                    },
                    Block {
                        start: sc("meas_start") as u64,
                        samples: &m,
                    },
                    &cfg,
                )
                .expect("valid config");
                let elapsed = t0.elapsed();
                let expect = match c["expect"].as_str().expect("expect") {
                    "accepted" => Expect::Accepted,
                    "ambiguous" => Expect::Ambiguous,
                    "accepted_or_ambiguous" => Expect::AcceptedOrAmbiguous,
                    "no_estimate" => Expect::NoEstimate,
                    other => panic!("unknown expectation {other}"),
                };
                let span = f64s(&c["span"]);
                out.push(CaseRun {
                    set: set.to_owned(),
                    case: name.clone(),
                    expect,
                    acceptable: f64s(&c["acceptable_first_delays"]),
                    span: (span.len() == 2).then(|| (span[0], span[1])),
                    reasons_any: c["reasons_any"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                    tol: sc("tol_samples"),
                    result,
                    elapsed,
                });
            }
        }
        out
    })
}

fn reason_name(r: NoEstimateReason) -> &'static str {
    match r {
        NoEstimateReason::NoReference => "no_reference",
        NoEstimateReason::NoSignal => "no_signal",
        NoEstimateReason::ObservationTooShort => "observation_too_short",
        NoEstimateReason::InsufficientOverlap => "insufficient_overlap",
        NoEstimateReason::InsufficientExcitation => "insufficient_excitation",
        NoEstimateReason::PeriodicExcitation { .. } => "periodic_excitation_too_short",
        NoEstimateReason::LowPsr => "low_psr",
        NoEstimateReason::LowPrecision => "low_precision",
        NoEstimateReason::PeakAtSearchEdge => "peak_at_search_edge",
        NoEstimateReason::LowBandSnr => "low_band_snr",
    }
}

fn ambiguity_name(r: AmbiguityReason) -> &'static str {
    match r {
        AmbiguityReason::BorderlineLevel => "borderline_level",
        AmbiguityReason::CloseArrivals => "close_arrivals",
        AmbiguityReason::MergedLobe => "merged_lobe",
        AmbiguityReason::OutsideRefinement => "outside_refinement",
    }
}

fn status(r: &FinderResult) -> (&'static str, Vec<&'static str>) {
    match &r.outcome {
        Outcome::Accepted { .. } => ("accepted", vec![]),
        Outcome::Ambiguous { reasons, .. } => (
            "ambiguous",
            reasons.iter().map(|&x| ambiguity_name(x)).collect(),
        ),
        Outcome::NoEstimate { reasons } => (
            "no_estimate",
            reasons.iter().map(|&x| reason_name(x)).collect(),
        ),
    }
}

/// The first-arrival rule pick from the candidate list (also for `NoEstimate`).
fn rule_pick(r: &FinderResult, threshold_db: f64) -> Option<Arrival> {
    r.candidates
        .iter()
        .copied()
        .find(|c| c.level_db >= threshold_db)
}

fn strongest(r: &FinderResult) -> Option<Arrival> {
    r.candidates
        .iter()
        .copied()
        .fold(None, |best: Option<Arrival>, c| match best {
            Some(b) if b.level_db >= c.level_db => Some(b),
            _ => Some(c),
        })
}

/// The expectation semantics of `parameters.expect_semantics` (same as check_fixtures.py).
fn passes(run: &CaseRun) -> bool {
    let near = |d: f64| run.acceptable.iter().any(|a| (d - a).abs() <= run.tol);
    match (&run.result.outcome, run.expect) {
        (Outcome::Accepted { first, .. }, Expect::Accepted) => near(first.delay_frac),
        (Outcome::Ambiguous { ranked, .. }, Expect::Ambiguous) => {
            ranked.iter().any(|c| near(c.delay_frac))
        }
        (Outcome::Ambiguous { .. }, Expect::AcceptedOrAmbiguous) => true,
        (Outcome::Accepted { first, .. }, Expect::AcceptedOrAmbiguous) => match run.span {
            Some((lo, hi)) => (lo..=hi).contains(&first.delay_frac),
            None => near(first.delay_frac),
        },
        (Outcome::NoEstimate { reasons }, Expect::NoEstimate) => reasons
            .iter()
            .any(|r| run.reasons_any.iter().any(|n| n == reason_name(*r))),
        _ => false,
    }
}

fn fmt_conf(c: &Confidence) -> String {
    format!(
        "psr={:.1} acq={:.1} snr={:.1} exc={:.3} w={:.2}",
        c.psr_db, c.psr_acq_db, c.band_snr_db, c.excited_fraction, c.pulse_width
    )
}

// ---------------------------------------------------------------------------------------
// Synthetic scenes
// ---------------------------------------------------------------------------------------

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

/// Pink noise (−3 dB/octave above 10 Hz) of `n` samples, RMS `rms`.
fn pink(n: usize, rms: f64, seed: u64) -> Vec<f64> {
    let mut g = Noise(seed);
    let mut x: Vec<f64> = (0..n).map(|_| g.gauss()).collect();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut spec = fwd.make_output_vec();
    fwd.process(&mut x, &mut spec).expect("fft");
    for (k, s) in spec.iter_mut().enumerate() {
        let f = k as f64 * FS / n as f64;
        *s *= if k == 0 {
            0.0
        } else {
            1.0 / f.max(10.0).sqrt()
        };
    }
    let last = spec.len() - 1;
    spec[last].im = 0.0;
    inv.process(&mut spec, &mut x).expect("ifft");
    let now = (x.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
    x.iter().map(|v| v * rms / now).collect()
}

/// Sum of exact band-limited (circular) fractional shifts of `x`: Σ g·x(i − d).
fn paths(x: &[f64], arrivals: &[(f64, f64)]) -> Vec<f64> {
    let n = x.len();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf = x.to_vec();
    let mut spec = fwd.make_output_vec();
    fwd.process(&mut buf, &mut spec).expect("fft");
    let mut acc = vec![Complex64::new(0.0, 0.0); spec.len()];
    for &(d, gain) in arrivals {
        for (k, (a, s)) in acc.iter_mut().zip(&spec).enumerate() {
            *a += s * Complex64::from_polar(gain, -2.0 * PI * k as f64 * d / n as f64);
        }
    }
    let last = acc.len() - 1;
    acc[0].im = 0.0;
    acc[last].im = 0.0;
    let mut out = vec![0.0; n];
    inv.process(&mut acc, &mut out).expect("ifft");
    out.iter().map(|v| v / n as f64).collect()
}

fn db(g: f64) -> f64 {
    10f64.powf(g / 20.0)
}

fn to_f32(v: &[f64]) -> Vec<f32> {
    v.iter().map(|&x| x as f32).collect()
}

/// A ref stream and a meas stream on the same clock (index i in both), meas = paths + pink
/// noise `noise_db` re the excitation.
fn scene(n: usize, arrivals: &[(f64, f64)], noise_db: f64, seed: u64) -> (Vec<f32>, Vec<f32>) {
    let x = pink(n, 0.1, seed);
    let mut m = paths(&x, arrivals);
    let noise = pink(n, 0.1 * db(noise_db), seed ^ 0x5A5A);
    for (v, e) in m.iter_mut().zip(&noise) {
        *v += e;
    }
    (to_f32(&x), to_f32(&m))
}

fn full_cfg(search: i64) -> FinderConfig {
    let mut cfg = FinderConfig::new(FS, Band::FullRange);
    cfg.search = SearchRange {
        min: -search,
        max: search,
    };
    cfg
}

/// One full-band window: meas [b, b+12000) against ref covering the search range.
fn full_window<'a>(r: &'a [f32], m: &'a [f32], b: usize, search: usize) -> (Block<'a>, Block<'a>) {
    (
        Block {
            start: (b - search) as u64,
            samples: &r[b - search..b + 12_000 + search],
        },
        Block {
            start: b as u64,
            samples: &m[b..b + 12_000],
        },
    )
}
