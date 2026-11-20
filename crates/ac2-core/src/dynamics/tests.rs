//! Dynamics programme and analysis tests: the real programme generator drives a synthetic
//! processor, and the analysis must read back the processor's known constants.

use std::collections::VecDeque;
use std::sync::Arc;

use super::analyse::{NO_REFERENCE_SECONDS, THUMP_MIN_PERIODS};
use super::*;
use crate::generator::{BandLimit, Generator, GeneratorConfig, Signal};

// ---------------------------------------------------------------------------------------
// The synthetic processor

/// A feed-forward limiter: a peak detector holding the largest |x| of the last period, the
/// gain reduction it asks for (dB over threshold, ratio ∞) smoothed by one-pole attack and
/// release in dB, an optional second, slower detector whose charge holds the gain down after
/// a long burst (programme-dependent release), a pure delay, and an optional control-voltage
/// feedthrough (a step added to the output in proportion to the gain reduction: the thump).
/// With no feedthrough it is the ideal limiter: a time-varying gain and nothing else.
#[derive(Debug, Clone)]
struct Limiter {
    threshold: f64,
    a_att: f64,
    a_rel: f64,
    slow: Option<(f64, f64)>,
    cv: f64,
    /// Quadratic term after the gain: `y + c·y²`, whose second harmonic is `c·A/2` re a
    /// fundamental of amplitude `A`.
    square: f64,
    p: usize,
    /// Sliding maximum of |x| over the last period: (index, value), values decreasing.
    peaks: VecDeque<(u64, f64)>,
    n: u64,
    fast: f64,
    slow_gr: f64,
    line: VecDeque<f32>,
}

fn coef(tau: f64, fs: f64) -> f64 {
    1.0 - (-1.0 / (tau * fs)).exp()
}

impl Limiter {
    /// Threshold in dBFS (a sine whose peak is full scale reads 0 dBFS); time constants in
    /// s.
    fn new(prog: &Programme, threshold_dbfs: f64, attack: f64, release: f64) -> Self {
        let fs = prog.sample_rate;
        Self {
            threshold: 10f64.powf(threshold_dbfs / 20.0),
            a_att: coef(attack, fs),
            a_rel: coef(release, fs),
            slow: None,
            cv: 0.0,
            square: 0.0,
            p: prog.period as usize,
            peaks: VecDeque::new(),
            n: 0,
            fast: 0.0,
            slow_gr: 0.0,
            line: VecDeque::new(),
        }
    }

    fn slow(mut self, prog: &Programme, attack: f64, release: f64) -> Self {
        let fs = prog.sample_rate;
        self.slow = Some((coef(attack, fs), coef(release, fs)));
        self
    }

    fn delay(mut self, d: usize) -> Self {
        self.line = std::iter::repeat_n(0.0, d).collect();
        self
    }

    fn cv(mut self, per_db: f64) -> Self {
        self.cv = per_db;
        self
    }

    fn square(mut self, c: f64) -> Self {
        self.square = c;
        self
    }

    fn process(&mut self, x: &[f32]) -> Vec<f32> {
        x.iter()
            .map(|&v| {
                let a = f64::from(v).abs();
                while self.peaks.back().is_some_and(|q| q.1 <= a) {
                    self.peaks.pop_back();
                }
                self.peaks.push_back((self.n, a));
                while self
                    .peaks
                    .front()
                    .is_some_and(|q| q.0 + self.p as u64 <= self.n)
                {
                    self.peaks.pop_front();
                }
                self.n += 1;
                let peak = self.peaks.front().map_or(0.0, |q| q.1);
                let target = (20.0 * (peak / self.threshold).log10()).max(0.0);
                let k = if target > self.fast {
                    self.a_att
                } else {
                    self.a_rel
                };
                self.fast += (target - self.fast) * k;
                let mut gr = self.fast;
                if let Some((att, rel)) = self.slow {
                    let k = if target > self.slow_gr { att } else { rel };
                    self.slow_gr += (target - self.slow_gr) * k;
                    gr = gr.max(self.slow_gr);
                }
                let g = f64::from(v) * 10f64.powf(-gr / 20.0);
                let y = g + self.square * g * g + self.cv * gr;
                self.line.push_back(y as f32);
                self.line.pop_front().unwrap_or(0.0)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------------------
// Harness

const FS: f64 = 48_000.0;

fn config(
    carrier_hz: f64,
    onset: Onset,
    bursts: &[f64],
    durations: &[f64],
    rest_max: f64,
) -> ProgrammeConfig {
    ProgrammeConfig {
        carrier_hz,
        onset,
        pilot_dbfs: -40.0,
        bursts_dbfs: bursts.to_vec(),
        durations_s: durations.to_vec(),
        rest_max_s: rest_max,
        replay_rests_s: None,
    }
}

fn programme(cfg: &ProgrammeConfig, fs: f64) -> Programme {
    match Programme::new(cfg, fs) {
        Ok(p) => p,
        Err(e) => panic!("programme: {e}"),
    }
}

#[derive(Debug)]
struct Run {
    result: Result<DynamicsResult, DynamicsError>,
    phases: Vec<Phase>,
    released: Vec<u32>,
    reference: Vec<f32>,
    outputs: Vec<Vec<f32>>,
    thump_excess: Vec<Vec<[[f64; 2]; 2]>>,
}

/// Plays `played` through the processors in a closed loop (the analysis's releases end the
/// rests), analysing as `analysed`, after `lead` samples of silence (the loop's latency).
fn run_with(
    played: &Programme,
    analysed: &Programme,
    procs: &mut [Limiter],
    block: usize,
    lead: usize,
) -> Run {
    let release = Release::new();
    let mut generator = match Generator::new(&GeneratorConfig {
        signal: Signal::Programme {
            programme: Arc::new(played.clone()),
            release: release.clone(),
        },
        sample_rate: played.sample_rate,
        seed: 0,
        band: BandLimit::NONE,
        level_dbfs: played.highest_dbfs(),
        ceiling_dbfs: f64::INFINITY,
    }) {
        Ok(g) => g,
        Err(e) => panic!("generator: {e}"),
    };
    let mut an = Analyser::new(analysed, procs.len());
    let bound = lead + ((played.longest_seconds() + 1.0) * played.sample_rate) as usize;
    let mut run = Run {
        result: Err(DynamicsError::NoReference),
        phases: vec![an.phase()],
        released: vec![0],
        reference: Vec::new(),
        outputs: vec![Vec::new(); procs.len()],
        thump_excess: Vec::new(),
    };
    let mut buf = vec![0f32; block];
    let mut fed = 0;
    let mut error = None;
    while fed < bound && an.phase() != Phase::Done {
        if fed >= lead {
            generator.fill(&mut buf);
        } else {
            buf.fill(0.0);
        }
        let outs: Vec<Vec<f32>> = procs.iter_mut().map(|p| p.process(&buf)).collect();
        let views: Vec<&[f32]> = outs.iter().map(Vec::as_slice).collect();
        run.reference.extend_from_slice(&buf);
        for (o, x) in run.outputs.iter_mut().zip(&outs) {
            o.extend_from_slice(x);
        }
        fed += block;
        if let Err(e) = an.push(&buf, &views) {
            error = Some(e);
            break;
        }
        release.publish(an.released());
        run.released.push(an.released());
        if run.phases.last() != Some(&an.phase()) {
            run.phases.push(an.phase());
        }
    }
    run.thump_excess = an.thump_excess.clone();
    run.result = match error {
        Some(e) => Err(e),
        None => an.finish(),
    };
    run
}

fn run(prog: &Programme, procs: &mut [Limiter], block: usize) -> Run {
    run_with(prog, prog, procs, block, 777)
}

/// Feeds a recording in blocks of `block`: the result's debug text and the releases.
fn replay(
    prog: &Programme,
    reference: &[f32],
    outputs: &[Vec<f32>],
    block: usize,
) -> (String, u32) {
    let mut an = Analyser::new(prog, outputs.len());
    let mut i = 0;
    while i < reference.len() {
        let j = (i + block).min(reference.len());
        let views: Vec<&[f32]> = outputs.iter().map(|o| &o[i..j]).collect();
        if let Err(e) = an.push(&reference[i..j], &views) {
            return (format!("{:?}", Err::<DynamicsResult, _>(e)), an.released());
        }
        i = j;
    }
    let released = an.released();
    (format!("{:?}", an.finish()), released)
}

fn result(run: &Run) -> &DynamicsResult {
    match &run.result {
        Ok(r) => r,
        Err(e) => panic!("analysis failed: {e}"),
    }
}

fn secs(t: Time) -> f64 {
    match t {
        Time::Seconds(s) => s,
        other => panic!("expected a time, got {other:?}"),
    }
}

fn assert_near(what: &str, got: f64, want: f64, tol: f64) {
    assert!(
        (got - want).abs() <= tol,
        "{what}: {got} not within {tol} of {want}"
    );
}

/// No harmonic reads as a number.
fn assert_no_harmonics(row: &StepRow, what: &str) {
    for (w, h) in row.harmonics.iter().enumerate() {
        for r in h.h.iter().chain([&h.thd]) {
            assert!(
                !matches!(r, Reading::Db(_)),
                "{what}: window {w} reads {r:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Tests

const THRESHOLD: f64 = -20.0;
const BURST: f64 = -8.0;
const GR: f64 = BURST - THRESHOLD;

/// The ideal limiter (10 ms attack, 40 ms release, 3 samples of delay) and the same limiter
/// with control-voltage feedthrough, at 1 kHz, zero onset.
fn limiter_run(block: usize) -> (Programme, Run) {
    let prog = programme(
        &config(1000.0, Onset::Zero, &[BURST], &[0.02, 0.3], 4.0),
        FS,
    );
    let ideal = Limiter::new(&prog, THRESHOLD, 0.010, 0.040).delay(3);
    let cv = ideal.clone().cv(0.02);
    let distorting = ideal.clone().square(0.2);
    let r = run(&prog, &mut [ideal, cv, distorting], block);
    (prog, r)
}

#[test]
fn limiter_constants_read_back() {
    let (prog, run) = limiter_run(256);
    let res = result(&run);
    let p = f64::from(prog.period) / FS;
    assert_eq!(res.steps.len(), 2);
    assert_eq!(res.steps[0].burst_samples, 960);
    assert_eq!(res.steps[1].burst_samples, 14_400);
    for s in &res.steps {
        assert!(s.recovered, "{s:?}");
        assert!(
            s.rest_samples >= 2 * 48_000 && s.rest_samples < 4 * 48_000,
            "{s:?}"
        );
    }
    for input in &res.inputs {
        assert_near("delay", input.delay_samples, 3.0, 0.05);
        assert_near("g0", input.g0_db, 0.0, 0.01);
    }
    let ideal = &res.inputs[0];
    let long = &ideal.rows[1];
    assert!(long.settled);
    assert_near("gr_end", long.gr_end, GR, 0.05);
    assert_eq!(long.gr_settled, Some(long.gr_end));
    // The detector sees the new peak a quarter period in, and releases a quarter period
    // after the last one.
    assert_near("attack_tau", secs(long.attack_tau), 0.010 + p / 4.0, p);
    assert_near(
        "attack_2db",
        secs(long.attack_2db),
        0.010 * (GR / 2.0).ln() + p / 4.0,
        p,
    );
    assert_near(
        "settle_0_5db",
        secs(long.settle_0_5db),
        0.010 * (GR / 0.5).ln() + p / 4.0,
        p,
    );
    assert_near("release_tau", secs(long.release_tau), 0.040 + p / 4.0, p);
    assert_near(
        "recovery_2db",
        secs(long.recovery_2db),
        0.040 * (GR / 2.0).ln() + p / 4.0,
        p,
    );
    assert_near("drift", long.drift, 0.0, 0.01);
    // The first quarter period passes unreduced.
    let o = long.overshoot.unwrap_or_else(|| panic!("no overshoot"));
    // The detector charges for the quarter period up to the first peak: 0.2 dB.
    assert_near("overshoot", o.db, GR - 0.2, 0.1);
    assert_near("reference overshoot", o.reference_db, 0.0, 0.01);
    assert!(o.at < p, "{o:?}");
    // The 20 ms burst is cut off two time constants in.
    let short = &ideal.rows[0];
    assert_near(
        "short gr_end",
        short.gr_end,
        GR * (1.0 - (-2.0f64).exp()),
        0.5,
    );
    assert_eq!(short.gr_settled, Some(long.gr_end));
    // It came within 2 dB of the settled reduction (at 1.8 τ), never within 0.5 dB (3.2 τ).
    assert_near(
        "short attack_2db",
        secs(short.attack_2db),
        secs(long.attack_2db),
        p,
    );
    assert_eq!(short.settle_0_5db, Time::CantTell);

    // The ideal limiter reads under its floors everywhere.
    for (i, row) in ideal.rows.iter().enumerate() {
        assert_no_harmonics(row, &format!("ideal step {i}"));
        for t in &row.thump {
            assert_eq!(t.peak, Reading::BelowFloor, "ideal step {i}: {row:?}");
            assert_eq!(t.rms, Reading::BelowFloor, "ideal step {i}: {row:?}");
        }
    }
    // The feedthrough's step reads above its floor at both edges.
    let cv = &res.inputs[1].rows[1];
    for t in &cv.thump {
        assert!(
            matches!(t.peak, Reading::Db(v) if v > -12.0),
            "feedthrough: {:?}",
            cv.thump
        );
        assert!(
            matches!(t.rms, Reading::Db(_)),
            "feedthrough: {:?}",
            cv.thump
        );
    }

    // The quadratic's second harmonic at the settled level (peak 10^(−20/20)) and at the
    // pilot (10^(−40/20)), and nothing above it.
    let sq = &res.inputs[2].rows[1];
    let h2 = |w: usize| match sq.harmonics[w].h[0] {
        Reading::Db(v) => v,
        other => panic!("window {w}: {other:?}"),
    };
    assert_near(
        "settled h2",
        h2(1),
        20.0 * (0.2 * 0.1 / 2.0f64).log10(),
        0.1,
    );
    assert_near("rest h2", h2(3), 20.0 * (0.2 * 0.01 / 2.0f64).log10(), 0.1);
    assert!(
        sq.harmonics[1].h[1..]
            .iter()
            .all(|r| *r == Reading::BelowFloor),
        "{:?}",
        sq.harmonics
    );
    let settled_point = res.inputs[2].envelopes[1]
        .iter()
        .rfind(|q| q.edge == Edge::Start)
        .map_or(f32::NAN, |q| q.h_db[0]);
    assert_near("h2 point", f64::from(settled_point), -40.0, 0.1);

    // Envelopes: a quarter period apart for 50 ms around the start, NaN harmonics (the ideal
    // limiter is below its floor), gain reduction from 0 to settled.
    let env = &ideal.envelopes[1];
    let start: Vec<_> = env.iter().filter(|q| q.edge == Edge::Start).collect();
    assert_near("first point", start[0].t, -0.050, p / 4.0);
    assert_near("spacing", start[1].t - start[0].t, p / 4.0, 1.0 / FS);
    assert!(start.iter().all(|q| q.h_db.iter().all(|h| h.is_nan())));
    assert!(start[0].gr_db.abs() < 0.01);
    let last = start.last().map_or(0.0, |q| f64::from(q.gr_db));
    assert_near("settled point", last, GR, 0.05);
    let end: Vec<_> = env.iter().filter(|q| q.edge == Edge::End).collect();
    assert!(end.last().is_some_and(|q| q.gr_db.abs() < 0.05));
    assert!(
        env.windows(2)
            .all(|w| w[0].edge != w[1].edge || w[1].t > w[0].t)
    );
}

#[test]
fn a_level_under_threshold_reads_no_gain_reduction() {
    let prog = programme(&config(1000.0, Onset::Zero, &[-25.0], &[0.3], 2.0), FS);
    let ideal = Limiter::new(&prog, THRESHOLD, 0.010, 0.040);
    let run = run(&prog, &mut [ideal], 1024);
    let row = &result(&run).inputs[0].rows[0];
    assert!(row.settled);
    assert!(row.gr_end.abs() < 0.01, "{row:?}");
    for t in [
        row.attack_tau,
        row.attack_2db,
        row.settle_0_5db,
        row.release_tau,
        row.recovery_2db,
        row.recovery_0_5db,
    ] {
        assert_eq!(t, Time::NoGainReduction, "{row:?}");
    }
    // A gain that does not move leaves the floor exact: thump is readable, and nothing.
    for t in &row.thump {
        assert_eq!(
            *t,
            Thump {
                peak: Reading::BelowFloor,
                rms: Reading::BelowFloor
            }
        );
    }
    assert_no_harmonics(row, "no gain reduction");
}

#[test]
fn phases_advance_and_releases_only_grow() {
    let (_, run) = limiter_run(500);
    assert!(run.result.is_ok());
    assert_eq!(
        run.phases,
        [
            Phase::Waiting,
            Phase::Check,
            Phase::Burst(0),
            Phase::Rest(0),
            Phase::Burst(1),
            Phase::Rest(1),
            Phase::Done
        ]
    );
    assert!(run.released.windows(2).all(|w| w[1] >= w[0]));
    assert_eq!(run.released.last(), Some(&2));
}

#[test]
fn result_does_not_depend_on_block_size() {
    let (prog, run) = limiter_run(256);
    let want = format!("{:?}", run.result);
    for block in [1, 7, 331, 4099, run.reference.len()] {
        let (got, released) = replay(&prog, &run.reference, &run.outputs, block);
        assert_eq!(got, want, "block {block}");
        assert_eq!(released, 2, "block {block}");
    }
}

#[test]
fn replayed_rests_play_the_same_programme() {
    let (prog, first) = limiter_run(256);
    let res = result(&first);
    let mut cfg = config(1000.0, Onset::Zero, &[BURST], &[0.02, 0.3], 4.0);
    cfg.replay_rests_s = Some(
        res.steps
            .iter()
            .map(|s| s.rest_samples as f64 / FS)
            .collect(),
    );
    let replayed = programme(&cfg, FS);
    let ideal = Limiter::new(&prog, THRESHOLD, 0.010, 0.040).delay(3);
    let cv = ideal.clone().cv(0.02);
    let distorting = ideal.clone().square(0.2);
    let second = run(&replayed, &mut [ideal, cv, distorting], 256);
    let again = result(&second);
    assert_eq!(again.steps, res.steps);
    assert_eq!(format!("{:?}", again.inputs), format!("{:?}", res.inputs));
    // Replayed rests are not released by the analysis.
    assert_eq!(second.released.last(), Some(&0));
}

#[test]
fn a_long_burst_recovers_slower_than_a_short_one() {
    let prog = programme(
        &config(1000.0, Onset::Zero, &[BURST], &[0.03, 1.0], 8.0),
        FS,
    );
    let two_stage = Limiter::new(&prog, THRESHOLD, 0.002, 0.030).slow(&prog, 0.300, 0.400);
    let run = run(&prog, &mut [two_stage], 512);
    let res = result(&run);
    let rows = &res.inputs[0].rows;
    let (short, long) = (secs(rows[0].release_tau), secs(rows[1].release_tau));
    // After the short burst only the fast detector is charged; after the long one the slow
    // detector holds the gain down.
    assert_near("short release", short, 0.030, 0.010);
    assert!(long > 0.3 && long < 0.5, "long release {long}");
    assert!(res.steps.iter().all(|s| s.recovered), "{:?}", res.steps);
    assert!(
        res.steps[0].rest_samples < res.steps[1].rest_samples,
        "{:?}",
        res.steps
    );
}

#[test]
fn a_pilot_above_threshold_aborts() {
    let prog = programme(&config(1000.0, Onset::Zero, &[BURST], &[0.02], 2.0), FS);
    let fine = Limiter::new(&prog, THRESHOLD, 0.010, 0.040);
    // Between the pilot (−40) and 6 dB below it: the pilot is limited, the check's low half
    // is not.
    let limiting = Limiter::new(&prog, -43.0, 0.010, 0.040);
    let run = run(&prog, &mut [fine, limiting], 1000);
    match run.result {
        Err(DynamicsError::PilotNotBelowThreshold { diff_db, input }) => {
            assert_eq!(input, 1);
            assert_near("diff", diff_db, -3.0, 0.1);
        }
        other => panic!("{other:?}"),
    }
    // The gate is read once the check is over: the first burst has played meanwhile.
    assert!(!run.phases.contains(&Phase::Burst(1)), "{:?}", run.phases);
}

#[test]
fn edges_off_the_programme_abort() {
    let played = programme(
        &config(1000.0, Onset::Zero, &[BURST], &[0.02, 0.3], 2.0),
        FS,
    );
    let analysed = programme(
        &config(1000.0, Onset::Zero, &[BURST], &[0.02, 0.2], 2.0),
        FS,
    );
    let ideal = Limiter::new(&played, THRESHOLD, 0.010, 0.040);
    let run = run_with(&played, &analysed, &mut [ideal], 256, 0);
    assert!(
        matches!(&run.result, Err(DynamicsError::Mismatch(m)) if m.contains("step 2")),
        "{:?}",
        run.result
    );
}

#[test]
fn a_capture_gap_aborts() {
    let prog = programme(
        &config(1000.0, Onset::Zero, &[BURST], &[0.02, 0.3], 2.0),
        FS,
    );
    let ideal = Limiter::new(&prog, THRESHOLD, 0.010, 0.040);
    let mut good = run(&prog, &mut [ideal], 256);
    assert!(good.result.is_ok());
    // 10 ms of silence in the middle of the first rest.
    let at = 777 + (5.0 * FS) as usize;
    for x in good.reference[at..at + 480]
        .iter_mut()
        .chain(&mut good.outputs[0][at..at + 480])
    {
        *x = 0.0;
    }
    let (got, _) = replay(&prog, &good.reference, &good.outputs, 1024);
    assert!(
        got.contains("Mismatch") && got.contains("capture gap"),
        "{got}"
    );
}

#[test]
fn silence_is_no_reference() {
    let prog = programme(&config(1000.0, Onset::Zero, &[BURST], &[0.02], 2.0), FS);
    let mut an = Analyser::new(&prog, 1);
    let zeros = vec![0f32; 4800];
    let mut out = Ok(());
    for _ in 0..((NO_REFERENCE_SECONDS + 1.0) * 10.0) as usize {
        out = an.push(&zeros, &[&zeros]);
        if out.is_err() {
            break;
        }
    }
    assert_eq!(out, Err(DynamicsError::NoReference));
    assert_eq!(an.finish(), Err(DynamicsError::NoReference));
}

/// The smallest whole number of carrier periods of an edge's measured time constant from
/// which on the ideal limiter's thump reads at least 3 dB inside the 6 dB margin over its
/// floor, at 1 kHz and 8 kHz, zero and peak onset: the gate `THUMP_MIN_PERIODS` must be it.
#[test]
fn slow_thump_gate_is_the_smallest_safe_whole_period_count() {
    const FLOOR_MARGIN_DB: f64 = 6.0;
    const SAFETY_DB: f64 = 3.0;
    let halves: Vec<f64> = (1..=16).map(|k| f64::from(k) / 2.0).collect();
    // (case, measured τ in periods, worst of peak and RMS excess over the floor, dB)
    let mut seen: Vec<(String, f64, f64)> = Vec::new();
    for (carrier, fs) in [(1000.0, 48_000.0), (8000.0, 96_000.0)] {
        for onset in [Onset::Zero, Onset::Peak] {
            let prog = programme(&config(carrier, onset, &[BURST], &[0.3], 2.0), fs);
            let period = f64::from(prog.period) / fs;
            let mut procs: Vec<Limiter> = halves
                .iter()
                .map(|&n| Limiter::new(&prog, THRESHOLD, n * period, n * period))
                .collect();
            let run = run(&prog, &mut procs, 1024);
            let res = result(&run);
            for (i, input) in res.inputs.iter().enumerate() {
                let row = &input.rows[0];
                let ex = run.thump_excess[i][0];
                let what = format!("{carrier} Hz {onset:?} τ {} periods", halves[i]);
                for (edge, tau) in [(0, row.attack_tau), (1, row.release_tau)] {
                    let t = match tau {
                        Time::Seconds(t) => t / period,
                        Time::WithinPeriod => 0.0,
                        other => panic!("{what}: edge {edge} τ {other:?}"),
                    };
                    seen.push((
                        format!("{what} edge {edge}"),
                        t,
                        ex[edge][0].max(ex[edge][1]),
                    ));
                }
            }
        }
    }
    for s in &seen {
        eprintln!(
            "{}: measured τ {:.2} periods, excess {:.1} dB",
            s.0, s.1, s.2
        );
    }
    let safe = |n: u32| {
        seen.iter()
            .filter(|s| s.1 >= f64::from(n))
            .all(|s| s.2 <= FLOOR_MARGIN_DB - SAFETY_DB)
    };
    let pinned = (1..=8).find(|&n| (n..=8).all(safe));
    assert_eq!(pinned, Some(THUMP_MIN_PERIODS));
}

/// The field run that failed: 1 kHz at 96 kHz, a 1 ms (one-period) burst. Its one-period
/// envelope is a triangle with no plateau, so the programme is refused before it plays,
/// naming the shortest burst; the shortest allowed is found and read at either onset, at
/// 1 kHz and at the highest carrier.
#[test]
fn a_burst_shorter_than_three_periods_is_refused_and_three_are_read() {
    let fs = 96_000.0;
    let field = config(
        1000.0,
        Onset::Zero,
        &[-11.0, -8.0, -4.0, -1.0],
        &[0.001, 0.003],
        2.0,
    );
    assert_eq!(
        Programme::new(&field, fs),
        Err(ProgrammeError::ShortBurst {
            duration_s: 0.001,
            shortest_s: 0.003,
            carrier_hz: 1000.0,
        })
    );
    let msg = Programme::new(&field, fs).map_or_else(|e| e.to_string(), |_| String::new());
    assert!(msg.contains("1 ms") && msg.contains("3 ms"), "{msg}");
    // Two periods of the highest carrier (12 samples) are refused too.
    let top = fs / f64::from(MIN_PERIOD);
    assert!(matches!(
        Programme::new(&config(top, Onset::Zero, &[BURST], &[24.0 / fs], 2.0), fs),
        Err(ProgrammeError::ShortBurst { .. })
    ));

    for (carrier, d) in [(1000.0, 0.003), (top, 36.0 / fs)] {
        for onset in [Onset::Zero, Onset::Peak] {
            let prog = programme(&config(carrier, onset, &[-11.0, -1.0], &[d, 0.1], 2.0), fs);
            assert_eq!(prog.burst_periods[0], MIN_BURST_PERIODS);
            let lim = Limiter::new(&prog, THRESHOLD, 0.010, 0.040).delay(3);
            let run = run(&prog, &mut [lim], 256);
            let res = result(&run);
            let p = u64::from(prog.period);
            for (i, s) in res.steps.iter().enumerate() {
                let want = u64::from(prog.step_periods(i)) * p;
                assert_eq!(s.burst_samples, want, "{carrier} Hz {onset:?} step {i}");
            }
        }
    }
}

#[test]
fn default_durations_are_three_periods_or_longer() {
    assert_eq!(
        default_durations(1000.0),
        vec![0.003, 0.01, 0.03, 0.1, 0.3, 1.0, 3.0, 10.0]
    );
    assert_eq!(default_durations(8000.0), DEFAULT_DURATIONS.to_vec());
    assert_eq!(default_durations(100.0)[0], 0.03);
    // Every default at 1 kHz plays at every common rate.
    for fs in [44_100.0, 48_000.0, 88_200.0, 96_000.0, 192_000.0] {
        let cfg = config(
            1000.0,
            Onset::Peak,
            &[BURST],
            &default_durations(1000.0),
            2.0,
        );
        assert!(Programme::new(&cfg, fs).is_ok(), "{fs}");
    }
}

#[test]
fn step_analysis_is_spread_over_the_following_blocks() {
    // A 10 s burst and its rest is the longest step there is; analysing it in the block
    // that completes it held a 96 kHz daemon past its capture queue. Each block may carry
    // only its share, the steps behind may not pile up, and the run's last block
    // finishes the rest.
    let block = 256;
    let inputs = 2;
    let prog = programme(
        &config(
            1000.0,
            Onset::Zero,
            &[-4.0],
            &[0.003, 10.0, 0.01, 0.03],
            30.0,
        ),
        FS,
    );
    let lim = Limiter::new(&prog, THRESHOLD, 0.010, 0.040).delay(3);
    let mut procs = vec![lim; inputs];
    let release = Release::new();
    let mut generator = match Generator::new(&GeneratorConfig {
        signal: Signal::Programme {
            programme: Arc::new(prog.clone()),
            release: release.clone(),
        },
        sample_rate: FS,
        seed: 0,
        band: BandLimit::NONE,
        level_dbfs: prog.highest_dbfs(),
        ceiling_dbfs: f64::INFINITY,
    }) {
        Ok(g) => g,
        Err(e) => panic!("generator: {e}"),
    };
    let mut an = Analyser::new(&prog, inputs);
    let bound = ((prog.longest_seconds() + 1.0) * FS) as usize;
    let longest_step = ((10.0 + 30.0) * FS) as usize;
    let share = (block * inputs) as u64 * super::analyse::WORK_PER_SAMPLE;
    let (mut fed, mut busy, mut most_kept) = (0, 0, 0);
    let mut buf = vec![0f32; block];
    while fed < bound && an.phase() != Phase::Done {
        generator.fill(&mut buf);
        let outs: Vec<Vec<f32>> = procs.iter_mut().map(|p| p.process(&buf)).collect();
        let views: Vec<&[f32]> = outs.iter().map(Vec::as_slice).collect();
        if let Err(e) = an.push(&buf, &views) {
            panic!("analysis failed at {fed}: {e}");
        }
        fed += block;
        release.publish(an.released());
        if an.phase() != Phase::Done {
            assert!(
                an.last_work <= share,
                "{} samples of step analysis in one block at {fed}",
                an.last_work
            );
            busy += usize::from(an.last_work > 0);
            most_kept = most_kept.max(an.retained());
        }
    }
    assert_eq!(an.phase(), Phase::Done);
    assert!(busy > 0, "no step was analysed before the run's end");
    // The 10 s step's analysis runs through the short steps after it, keeping its audio
    // that much longer, but never a second long step's worth.
    assert!(
        most_kept < 2 * longest_step,
        "{most_kept} samples kept, a step is at most {longest_step}"
    );
    let r = match an.finish() {
        Ok(r) => r,
        Err(e) => panic!("analysis failed: {e}"),
    };
    assert_eq!(r.steps.len(), 4);
}
