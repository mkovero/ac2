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

#[test]
fn fixture_acceptance_table() {
    let runs = fixture_runs();
    assert_eq!(runs.len(), 16, "the §13 table has 16 cases");
    let mut failed = Vec::new();
    for run in runs {
        let ok = passes(run);
        let (st, why) = status(&run.result);
        let listed: Vec<String> = match &run.result.outcome {
            Outcome::Ambiguous { ranked, .. } => ranked
                .iter()
                .map(|a| format!("{:.2}", a.delay_frac))
                .collect(),
            _ => vec![],
        };
        println!(
            "{} {}/{:<26} expect {:<20} got {st} {why:?} first={:?} listed={listed:?} {} \
             unc={:.3} [{:.0} ms]",
            if ok { "PASS" } else { "FAIL" },
            run.set.trim_start_matches("delay_finder_"),
            run.case,
            format!("{:?}", run.expect),
            rule_pick(&run.result, -12.0).map(|a| (a.delay_frac * 1000.0).round() / 1000.0),
            fmt_conf(&run.result.confidence),
            rule_pick(&run.result, -12.0).map_or(f64::NAN, |a| a.uncertainty),
            run.elapsed.as_secs_f64() * 1e3,
        );
        if !ok {
            failed.push(format!("{}/{}", run.set, run.case));
        }
    }
    assert!(failed.is_empty(), "fixture cases failed: {failed:?}");
}

// ---------------------------------------------------------------------------------------
// Cross-check against the Python reference prototype
// ---------------------------------------------------------------------------------------

/// `tools/experiments/q1/finder.py` (`find_delay` with the band defaults) on the same
/// fixture inputs, in f64. Generated by running the prototype over the golden sets.
#[derive(Debug)]
struct Py {
    set: &'static str,
    case: &'static str,
    status: &'static str,
    reasons: &'static [&'static str],
    first: f64,
    strongest: f64,
    listed: &'static [f64],
    psr: f64,
    psr_acq: f64,
    snr: f64,
    excited: f64,
    width: f64,
    unc: f64,
    n_cands: usize,
}

#[rustfmt::skip]
const PY: &[Py] = &[
    Py { set: "delay_finder_full_music", case: "music_refl_louder", status: "accepted", reasons: &[], first: 55.495506749539345, strongest: 355.50126062977597, listed: &[55.495506749539345, 355.50126062977597], psr: 37.83986100833738, psr_acq: 29.623900330741616, snr: 20.090891187767134, excited: 0.9795214701151483, width: 3.9165835476039117, unc: 0.1, n_cands: 2 },
    Py { set: "delay_finder_full_narrow", case: "narrow_excitation", status: "no_estimate", reasons: &["insufficient_excitation"], first: 100.01108211349947, strongest: 100.01108211349947, listed: &[], psr: 26.986130348011393, psr_acq: 20.582816044759564, snr: 34.33635960499979, excited: 0.4136417834966184, width: 16.337117321519464, unc: 0.1, n_cands: 1 },
    Py { set: "delay_finder_full_periodic", case: "periodic_wrap", status: "no_estimate", reasons: &["periodic_excitation_too_short"], first: 87.997580865891, strongest: 480.00013674147203, listed: &[], psr: 41.40574715385122, psr_acq: 25.547259705757483, snr: 15.674185178554056, excited: 0.9997485182530932, width: 3.8416313468345145, unc: 0.1, n_cands: 2 },
    Py { set: "delay_finder_full_pink", case: "single_frac", status: "accepted", reasons: &[], first: 137.3728468863501, strongest: 137.3728468863501, listed: &[137.3728468863501], psr: 45.83713982384961, psr_acq: 26.70604204491863, snr: 31.16555612293258, excited: 0.999601374482577, width: 3.844021732949657, unc: 0.1, n_cands: 1 },
    Py { set: "delay_finder_full_pink", case: "negative_frac", status: "accepted", reasons: &[], first: -611.5974050897105, strongest: -611.5974050897105, listed: &[-611.5974050897105], psr: 45.71061217580414, psr_acq: 27.47785335470435, snr: 31.25428316085275, excited: 0.9997381503724427, width: 3.8432749883333654, unc: 0.1, n_cands: 1 },
    Py { set: "delay_finder_full_pink", case: "refl_louder_inverted", status: "accepted", reasons: &[], first: 250.2558708135748, strongest: 370.2480553906221, listed: &[250.2558708135748, 370.2480553906221], psr: 44.70476845227668, psr_acq: 26.11413619996049, snr: 25.814322622379212, excited: 0.9996677148091386, width: 3.8437057280527283, unc: 0.1, n_cands: 2 },
    Py { set: "delay_finder_full_pink", case: "refl_louder_far", status: "accepted", reasons: &[], first: -80.49893050694027, strongest: 879.5028508988262, listed: &[-80.49893050694027, 879.5028508988262], psr: 39.930849919702034, psr_acq: 26.318827240300084, snr: 13.959088081552187, excited: 0.9998782727123946, width: 3.8433686707531844, unc: 0.1, n_cands: 2 },
    Py { set: "delay_finder_full_pink", case: "direct_buried", status: "accepted", reasons: &[], first: 543.9997896287068, strongest: 543.9997896287068, listed: &[543.9997896287068], psr: 45.77780952711491, psr_acq: 27.60260558282924, snr: 30.176208092539234, excited: 0.9997580180619913, width: 3.8434318702424113, unc: 0.1, n_cands: 2 },
    Py { set: "delay_finder_full_pink", case: "borderline", status: "ambiguous", reasons: &["borderline_level"], first: 500.00157297313217, strongest: 500.00157297313217, listed: &[500.00157297313217, 299.9948851942627], psr: 45.22488683348229, psr_acq: 26.969241653347858, snr: 26.709281900700503, excited: 0.9997580180619913, width: 3.8434917706326575, unc: 0.1, n_cands: 2 },
    Py { set: "delay_finder_full_pink", case: "close_interfering", status: "ambiguous", reasons: &["close_arrivals"], first: -200.39423722488618, strongest: -195.41013843257906, listed: &[-200.39423722488618, -195.41013843257906], psr: 43.97036745274523, psr_acq: 25.72280884226422, snr: 31.112028446445635, excited: 0.9998149602086855, width: 3.843859434266728, unc: 0.1, n_cands: 2 },
    Py { set: "delay_finder_full_pink", case: "room", status: "accepted", reasons: &[], first: 1020.6000803007887, strongest: 1164.6089807520862, listed: &[1020.6000803007887, 1164.6089807520862], psr: 33.15972818656469, psr_acq: 20.526869487530682, snr: 11.301271593840205, excited: 0.9997580481256539, width: 3.843646221623899, unc: 0.1, n_cands: 6 },
    Py { set: "delay_finder_full_pink", case: "large_negative", status: "accepted", reasons: &[], first: -35040.30062439476, strongest: -35040.30062439476, listed: &[-35040.30062439476], psr: 43.68828257169486, psr_acq: 19.903356380582984, snr: 19.500461224994947, excited: 0.9997073724751276, width: 3.849657011761146, unc: 0.1, n_cands: 2 },
    Py { set: "delay_finder_full_pink", case: "low_snr", status: "no_estimate", reasons: &["low_psr"], first: 89.76243387906334, strongest: 89.76243387906334, listed: &[], psr: 4.5920722096518976, psr_acq: 6.4461314102698895, snr: -4.41920190607084, excited: 0.999601374482577, width: 3.8440386029945284, unc: 0.15150042660256585, n_cands: 1 },
    Py { set: "delay_finder_mid_pink", case: "mid_single_frac", status: "accepted", reasons: &[], first: -1234.6087230548042, strongest: -1234.6087230548042, listed: &[-1234.6087230548042], psr: 41.848874465768226, psr_acq: 23.08126025006354, snr: 31.521108057820904, excited: 1.000280901393055, width: 20.051820607834003, unc: 0.1, n_cands: 1 },
    Py { set: "delay_finder_mid_pink", case: "mid_refl_louder_inverted", status: "accepted", reasons: &[], first: 700.1814587900753, strongest: 940.3229452316118, listed: &[700.1814587900753, 940.3229452316118], psr: 38.79580125920635, psr_acq: 21.829802324819273, snr: 26.162813232856035, excited: 1.0000138091800514, width: 20.043592131032593, unc: 0.1, n_cands: 2 },
    Py { set: "delay_finder_sub_pink", case: "sub_refl_louder", status: "accepted", reasons: &[], first: -600.3820738308987, strongest: 1078.7484529228186, listed: &[-600.3820738308987, 1078.7484529228186], psr: 35.159307866975645, psr_acq: 18.031705855805697, snr: 21.503145009713045, excited: 1.0008451164945056, width: 536.4030875294503, unc: 1.1589070243299304, n_cands: 2 },
];

/// Same decisions and reasons as the prototype; delays within the band tolerance (§9) of
/// it; confidence figures close. The inputs here are f32 (the API type), the prototype
/// ran in f64, so last-digit agreement is not expected.
#[test]
fn matches_python_prototype() {
    let runs = fixture_runs();
    let mut bad = Vec::new();
    let (mut worst_first, mut worst_strong) = (0.0f64, 0.0f64);
    for py in PY {
        let run = runs
            .iter()
            .find(|r| r.set == py.set && r.case == py.case)
            .expect("case present");
        let r = &run.result;
        let (st, why) = status(r);
        let pick = rule_pick(r, -12.0).expect("a candidate");
        let strong = strongest(r).expect("a candidate");
        let c = &r.confidence;
        let d_first = pick.delay_frac - py.first;
        let d_strong = strong.delay_frac - py.strongest;
        worst_first = worst_first.max(d_first.abs());
        worst_strong = worst_strong.max(d_strong.abs());
        let ranked: Vec<f64> = match &r.outcome {
            Outcome::Ambiguous { ranked, .. } => ranked.iter().map(|a| a.delay_frac).collect(),
            Outcome::Accepted { first, strongest } => {
                let mut v = vec![first.delay_frac];
                if strongest.delay != first.delay {
                    v.push(strongest.delay_frac);
                }
                v
            }
            Outcome::NoEstimate { .. } => vec![],
        };
        println!(
            "{}/{:<26} Δfirst={:+.4} Δstrongest={:+.4} Δpsr={:+.2} Δacq={:+.2} Δsnr={:+.2} \
             Δexc={:+.4} Δw={:+.3} Δunc={:+.3} cands {}/{}",
            py.set.trim_start_matches("delay_finder_"),
            py.case,
            d_first,
            d_strong,
            c.psr_db - py.psr,
            c.psr_acq_db - py.psr_acq,
            c.band_snr_db - py.snr,
            c.excited_fraction - py.excited,
            c.pulse_width - py.width,
            pick.uncertainty - py.unc,
            r.candidates.len(),
            py.n_cands,
        );
        let mut problems = Vec::new();
        if st != py.status || why != py.reasons {
            problems.push(format!(
                "status {st} {why:?} vs {} {:?}",
                py.status, py.reasons
            ));
        }
        if d_first.abs() > run.tol || d_strong.abs() > run.tol {
            problems.push(format!(
                "delay Δ {d_first:+.3}/{d_strong:+.3} > tol {}",
                run.tol
            ));
        }
        // with a 30 dB-class floor, 0.5 dB is several times the f32 input noise
        let close_db = |a: f64, b: f64| (a - b).abs() <= 0.5;
        if !close_db(c.psr_db, py.psr) || !close_db(c.psr_acq_db, py.psr_acq) {
            problems.push("psr".into());
        }
        if !close_db(c.band_snr_db, py.snr) {
            problems.push("band snr".into());
        }
        if (c.excited_fraction - py.excited).abs() > 0.01 || (c.pulse_width - py.width).abs() > 0.05
        {
            problems.push("excitation / pulse width".into());
        }
        if (pick.uncertainty - py.unc).abs() > 0.05 * py.unc.max(0.1) {
            problems.push("uncertainty".into());
        }
        if r.candidates.len() != py.n_cands {
            problems.push("candidate count".into());
        }
        if ranked.len() != py.listed.len()
            || ranked
                .iter()
                .zip(py.listed)
                .any(|(a, b)| (a - b).abs() > run.tol)
        {
            problems.push(format!("listed {ranked:?} vs {:?}", py.listed));
        }
        if !problems.is_empty() {
            bad.push(format!("{}/{}: {}", py.set, py.case, problems.join("; ")));
        }
    }
    println!("max |Δ| first {worst_first:.2e} samples, strongest {worst_strong:.2e} samples");
    assert!(
        bad.is_empty(),
        "differs from the prototype:\n{}",
        bad.join("\n")
    );
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

#[test]
fn index_invariance_and_sign() {
    let (r, m) = scene(60_000, &[(321.4, 1.0)], -30.0, 11);
    let cfg = full_cfg(2400);
    let (rb, mb) = full_window(&r, &m, 20_000, 2400);
    let base = find(rb, mb, &cfg).expect("config");
    let first = *base.accepted().expect("accepted");
    assert!((first.delay_frac - 321.4).abs() < 0.1, "{first:?}");
    assert_eq!(first.delay, 321);

    // shifting both block starts by the same amount changes nothing
    let shift = 7_000_000_123u64;
    let moved = find(
        Block {
            start: rb.start + shift,
            ..rb
        },
        Block {
            start: mb.start + shift,
            ..mb
        },
        &cfg,
    )
    .expect("config");
    assert_eq!(moved.outcome, base.outcome);
    assert_eq!(moved.meas_window, mb.start + shift..mb.end_index() + shift);

    // swapping the roles of ref and meas negates a single-path delay
    let swapped = find(
        Block {
            start: 20_000 - 2400,
            samples: &m[20_000 - 2400..32_000 + 2400],
        },
        Block {
            start: 20_000,
            samples: &r[20_000..32_000],
        },
        &cfg,
    )
    .expect("config");
    let neg = swapped.accepted().expect("accepted when swapped");
    assert!((neg.delay_frac + 321.4).abs() < 0.1, "{neg:?}");
}

trait EndIndex {
    fn end_index(&self) -> u64;
}

impl EndIndex for Block<'_> {
    fn end_index(&self) -> u64 {
        self.start + self.samples.len() as u64
    }
}

fn blk(samples: &[f32], start: u64) -> Block<'_> {
    Block { start, samples }
}

fn no_estimate(r: &FinderResult) -> &[NoEstimateReason] {
    match &r.outcome {
        Outcome::NoEstimate { reasons } => reasons,
        other => panic!("expected NoEstimate, got {other:?}"),
    }
}

#[test]
fn refusal_paths() {
    let cfg = full_cfg(2400);
    let (r, m) = scene(40_000, &[(100.0, 1.0)], -30.0, 5);
    let zeros = vec![0.0f32; 20_000];

    let res = find(blk(&zeros, 0), blk(&m[0..12_000], 2400), &cfg).expect("config");
    assert_eq!(no_estimate(&res), &[NoEstimateReason::NoReference]);
    let res = find(blk(&r[0..16_800], 0), blk(&zeros[0..12_000], 2400), &cfg).expect("config");
    assert_eq!(no_estimate(&res), &[NoEstimateReason::NoSignal]);
    let res = find(blk(&zeros, 0), blk(&zeros, 0), &cfg).expect("config");
    assert_eq!(
        no_estimate(&res),
        &[NoEstimateReason::NoReference, NoEstimateReason::NoSignal]
    );
    // N₂ = 8192 at 48 kHz full range
    let res = find(
        blk(&r[0..12_000], 0),
        blk(&m[2400..2400 + 8000], 2400),
        &cfg,
    )
    .expect("config");
    assert_eq!(no_estimate(&res), &[NoEstimateReason::ObservationTooShort]);
    // ref far away from every lag of the search range
    let res = find(blk(&r[0..12_000], 0), blk(&m[20_000..32_000], 20_000), &cfg).expect("config");
    assert_eq!(no_estimate(&res), &[NoEstimateReason::InsufficientOverlap]);
    // given periodic excitation: P ≤ span + tail
    let mut pcfg = cfg;
    pcfg.excitation_period = Some(8192);
    let (rb, mb) = full_window(&r, &m, 10_000, 2400);
    let res = find(rb, mb, &pcfg).expect("config");
    assert!(
        no_estimate(&res).contains(&NoEstimateReason::PeriodicExcitation { period: 8192 }),
        "{res:?}"
    );
    assert_eq!(res.confidence.period, Some(8192));
    // invalid configuration is an error, not a result
    let mut bad = cfg;
    bad.search = SearchRange { min: 5, max: -5 };
    assert!(find(rb, mb, &bad).is_err());
}

#[test]
fn ambiguous_lists_rule_pick_first() {
    // direct at −12.3 dB: within ±2 dB of the threshold → BorderlineLevel
    let (r, m) = scene(60_000, &[(300.0, db(-12.3)), (500.0, 1.0)], -30.0, 21);
    let (rb, mb) = full_window(&r, &m, 20_000, 2400);
    let res = find(rb, mb, &full_cfg(2400)).expect("config");
    match &res.outcome {
        Outcome::Ambiguous {
            reasons,
            ranked,
            strongest,
        } => {
            assert!(
                reasons.contains(&AmbiguityReason::BorderlineLevel),
                "{reasons:?}"
            );
            assert!(ranked.len() <= 3 && !ranked.is_empty());
            assert!((strongest.delay_frac - 500.0).abs() < 0.5);
            assert!(ranked.iter().any(|a| (a.delay_frac - 300.0).abs() < 1.0));
            assert_eq!(res.pick(), ranked.first());
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

/// Filter by a zero-phase magnitude response.
fn shape(x: &[f64], mag: impl Fn(f64) -> f64) -> Vec<f64> {
    let n = x.len();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf = x.to_vec();
    let mut spec = fwd.make_output_vec();
    fwd.process(&mut buf, &mut spec).expect("fft");
    for (k, s) in spec.iter_mut().enumerate() {
        *s *= mag(k as f64 * FS / n as f64);
    }
    let mut out = vec![0.0; n];
    inv.process(&mut spec, &mut out).expect("ifft");
    out.iter().map(|v| v / n as f64).collect()
}

#[test]
fn auto_band_picks_from_the_excitation() {
    let cfg = full_cfg(2400);
    let n = 40_000;
    let blocks = |r: &[f32], m: &[f32]| -> (Vec<f32>, Vec<f32>) {
        (
            r[5000 - 2400..5000 + 24_000 + 2400].to_vec(),
            m[5000..5000 + 24_000].to_vec(),
        )
    };
    let run = |r: &[f32], m: &[f32]| {
        let (rb, mb) = blocks(r, m);
        find_auto(
            Block {
                start: 5000 - 2400,
                samples: &rb,
            },
            Block {
                start: 5000,
                samples: &mb,
            },
            &cfg,
        )
        .expect("config")
    };
    // pink: full range answers first
    let (r, m) = scene(n, &[(100.0, 1.0)], -30.0, 31);
    let res = run(&r, &m);
    assert_eq!(res.band, Band::FullRange);
    assert!(
        res.accepted().is_some_and(|a| a.delay == 100),
        "{:?}",
        res.outcome
    );
    // excitation rolling off above 1.2 kHz (8th-order), ref with a white floor 70 dB down:
    // full range refuses, mid answers
    let lp = |f: f64| 1.0 / (1.0 + (f / 1200.0).powi(16)).sqrt();
    let x = shape(&pink(n, 0.1, 32), lp);
    let mut m = paths(&x, &[(100.0, 1.0)]);
    for (v, e) in m.iter_mut().zip(shape(&pink(n, 0.003, 33), lp)) {
        *v += e;
    }
    let mut floor = Noise(34);
    let r: Vec<f64> = x
        .iter()
        .map(|v| v + 0.1 * db(-70.0) * floor.gauss())
        .collect();
    let full = {
        let (rb, mb) = blocks(&to_f32(&r), &to_f32(&m));
        find(
            Block {
                start: 5000 - 2400,
                samples: &rb,
            },
            Block {
                start: 5000,
                samples: &mb,
            },
            &cfg,
        )
        .expect("config")
    };
    println!(
        "full band on 1.2 kHz excitation: {:?} {:?}",
        full.outcome, full.confidence
    );
    assert!(!no_estimate(&full).is_empty());
    let res = run(&to_f32(&r), &to_f32(&m));
    assert_eq!(res.band, Band::Mid);
    assert!(
        res.accepted().is_some_and(|a| a.delay == 100),
        "{:?}",
        res.outcome
    );
}

// ---------------------------------------------------------------------------------------
// Tracking
// ---------------------------------------------------------------------------------------

/// Push a scene through a [`DelayStream`] in 1024-sample blocks; return each window's
/// result.
fn stream_results(cfg: FinderConfig, r: &[f32], m: &[f32]) -> Vec<FinderResult> {
    let mut s = DelayStream::new(cfg).expect("config");
    let mut out = Vec::new();
    for (i, (rc, mc)) in r.chunks(1024).zip(m.chunks(1024)).enumerate() {
        let start = (i * 1024) as u64;
        s.push_ref(Block { start, samples: rc });
        s.push_meas(Block { start, samples: mc });
        while let Some(res) = s.poll().expect("config") {
            out.push(res);
        }
    }
    out
}

fn track(results: &[FinderResult], agreement: Agreement) -> Vec<Option<i64>> {
    let mut t = Tracker::new(agreement);
    results.iter().map(|r| t.observe(r)).collect()
}

#[test]
fn tracking_full_locks_in_two_windows_and_relocks_after_a_step() {
    let cfg = full_cfg(2400);
    let lm = cfg.observation_len();
    assert_eq!(lm, 12_000);
    let n_win = 6;
    // windows start at 2400 (the first index whose largest lag has ref)
    let change = 2400 + 3 * lm;
    let n = 2400 + n_win * lm + 2400 + 1024;
    let x = pink(n, 0.1, 99);
    let (d1, d2) = (412.3, 1187.8);
    let m1 = paths(&x, &[(d1, db(-6.0)), (d1 + 300.0, -1.0)]);
    let m2 = paths(&x, &[(d2, db(-6.0)), (d2 + 300.0, -1.0)]);
    let noise = pink(n, 0.1 * db(-30.0), 1234);
    let m: Vec<f64> = (0..n)
        .map(|i| if i < change { m1[i] } else { m2[i] } + noise[i])
        .collect();
    let results = stream_results(cfg, &to_f32(&x), &to_f32(&m));
    assert!(results.len() >= n_win, "{} windows", results.len());
    for w in results.windows(2) {
        assert_eq!(w[0].meas_window.end, w[1].meas_window.start, "back to back");
    }
    assert_eq!(results[3].meas_window.start, change as u64);
    let moves = track(&results, Agreement::for_band(Band::FullRange, FS));
    println!("full tracking moves: {moves:?}");
    assert_eq!(moves[0], None);
    assert_eq!(moves[1], Some(412), "lock after two windows");
    assert_eq!(moves[2], None);
    assert_eq!(
        moves[3], None,
        "first window after the step is only pending"
    );
    assert_eq!(moves[4], Some(1188), "relock two windows after the step");
}

#[test]
fn tracking_never_moves_on_ambiguous_results() {
    let cfg = full_cfg(2400);
    let n = 2400 + 4 * 12_000 + 2400 + 1024;
    let (r, m) = scene(n, &[(300.0, db(-12.3)), (500.0, 1.0)], -30.0, 77);
    let results = stream_results(cfg, &r, &m);
    assert!(results.len() >= 4);
    for res in &results {
        assert!(
            matches!(res.outcome, Outcome::Ambiguous { .. }),
            "{:?}",
            res.outcome
        );
    }
    let moves = track(&results, Agreement::for_band(Band::FullRange, FS));
    assert!(moves.iter().all(Option::is_none), "{moves:?}");
}

/// A hand-made result for rule tests.
fn result(outcome: Outcome, window: std::ops::Range<u64>) -> FinderResult {
    FinderResult {
        outcome,
        candidates: vec![],
        confidence: Confidence {
            psr_db: 40.0,
            psr_acq_db: 25.0,
            band_snr_db: 30.0,
            excited_fraction: 1.0,
            pulse_width: 4.0,
            nominal_width: 4.0,
            period: None,
            refinement_window: None,
        },
        band: Band::FullRange,
        meas_window: window,
    }
}

fn arrival(d: f64) -> Arrival {
    Arrival {
        delay: d.round() as i64,
        delay_frac: d,
        level_db: 0.0,
        phase_deg: 0.0,
        uncertainty: 0.1,
        misfit: 0.0,
        refined: true,
    }
}

fn accepted(d: f64, k: u64) -> FinderResult {
    result(
        Outcome::Accepted {
            first: arrival(d),
            strongest: arrival(d),
        },
        k * 100..(k + 1) * 100,
    )
}

#[test]
fn tracker_rules() {
    let full = Agreement::for_band(Band::FullRange, FS);
    let mut t = Tracker::new(full);
    // an ambiguous window between two agreeing ones clears the pending result
    assert_eq!(t.observe(&accepted(1000.2, 0)), None);
    let amb = result(
        Outcome::Ambiguous {
            reasons: vec![AmbiguityReason::CloseArrivals],
            ranked: [arrival(1000.2)].into_iter().collect(),
            strongest: arrival(1000.2),
        },
        100..200,
    );
    assert_eq!(t.observe(&amb), None);
    assert_eq!(t.observe(&accepted(1000.2, 2)), None);
    // overlapping windows are ignored
    assert_eq!(
        t.observe(&result(
            Outcome::Accepted {
                first: arrival(1000.0),
                strongest: arrival(1000.0)
            },
            250..350
        )),
        None
    );
    assert_eq!(t.observe(&accepted(1000.9, 3)), Some(1001));
    assert_eq!(t.held(), Some(1001));
    // a refusal clears pending too
    assert_eq!(t.observe(&accepted(1000.9, 4)), None);
    assert_eq!(
        t.observe(&result(
            Outcome::NoEstimate {
                reasons: vec![NoEstimateReason::LowPsr]
            },
            500..600
        )),
        None
    );
    assert_eq!(t.observe(&accepted(2000.0, 6)), None);
    assert_eq!(t.held(), Some(1001));
    // ±1 sample in the full band: 2.2 samples apart never agree
    assert_eq!(t.observe(&accepted(2002.2, 7)), None);
    assert_eq!(t.observe(&accepted(2000.0, 8)), None);
    t.reset();
    assert_eq!(t.observe(&accepted(2000.0, 9)), None);
    assert_eq!(t.observe(&accepted(2000.4, 10)), Some(2000));
}

#[test]
fn tracker_sub_agreement_is_a_tenth_of_a_millisecond() {
    let sub = Agreement::for_band(Band::Sub, FS);
    assert!((sub.samples - 4.8).abs() < 1e-12);
    let mut t = Tracker::new(sub);
    assert_eq!(t.observe(&accepted(1000.0, 0)), None);
    assert_eq!(t.observe(&accepted(1004.7, 1)), Some(1005));
    assert_eq!(t.observe(&accepted(1000.0, 2)), None);
    assert_eq!(
        t.observe(&accepted(1004.9, 3)),
        None,
        "4.9 samples > 0.1 ms"
    );
    // the same 4.7-sample spread never agrees in the full band
    let mut f = Tracker::new(Agreement::for_band(Band::FullRange, FS));
    assert_eq!(f.observe(&accepted(1000.0, 0)), None);
    assert_eq!(f.observe(&accepted(1004.7, 1)), None);
}

/// Sub band through the stream at the default 4 s observation: lock after two windows (8 s).
#[test]
fn tracking_sub_locks_in_two_windows() {
    let mut cfg = FinderConfig::new(FS, Band::Sub);
    cfg.search = SearchRange {
        min: -9600,
        max: 9600,
    };
    let lm = cfg.observation_len();
    assert_eq!(lm, 192_000);
    let n = 9600 + 2 * lm + 9600 + 1024;
    let (r, m) = scene(n, &[(-600.3, 1.0)], -40.0, 4242);
    let results = stream_results(cfg, &r, &m);
    assert_eq!(results.len(), 2);
    for res in &results {
        let a = res.accepted().expect("accepted");
        assert!((a.delay_frac + 600.3).abs() <= 4.8, "{a:?}");
    }
    let moves = track(&results, Agreement::for_band(Band::Sub, FS));
    assert_eq!(moves[0], None);
    assert!(
        matches!(moves[1], Some(d) if (d + 600).abs() <= 5),
        "{moves:?}"
    );
}

// ---------------------------------------------------------------------------------------
// Timing (prints only)
// ---------------------------------------------------------------------------------------

#[test]
fn timing_print() {
    let mode = if cfg!(debug_assertions) {
        "debug build (ac2-core unoptimised; FFT deps at opt-level 2)"
    } else {
        "release build"
    };
    for (band, label) in [(Band::FullRange, "full"), (Band::Sub, "sub")] {
        let cfg = FinderConfig::new(FS, band); // ±1 s search, default observation
        let lm = cfg.observation_len();
        let span = cfg.search.max as usize;
        let n = lm + 2 * span;
        let (r, m) = scene(n, &[(123.4, 1.0)], -30.0, 8);
        let mut scratch = FinderScratch::for_config(&cfg).expect("config");
        let mut times = Vec::new();
        let mut last = None;
        // a second call shows the cost with plans and buffers reused; skipped in debug
        // builds to keep the test run short
        let calls = if cfg!(debug_assertions) { 1 } else { 2 };
        for _ in 0..calls {
            let t0 = Instant::now();
            let res = find_with(
                &mut scratch,
                Block {
                    start: 0,
                    samples: &r,
                },
                Block {
                    start: span as u64,
                    samples: &m[span..span + lm],
                },
                &cfg,
            )
            .expect("config");
            times.push(t0.elapsed().as_secs_f64() * 1e3);
            last = Some(res);
        }
        let res = last.expect("ran");
        println!(
            "timing {label}: obs {:.2} s, search ±1 s: {:.0} ms first call, {} reused \
             scratch [{mode}] -> {:?}",
            lm as f64 / FS,
            times[0],
            times
                .get(1)
                .map_or("-".to_owned(), |t| format!("{t:.0} ms")),
            status(&res),
        );
    }
}
