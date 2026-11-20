//! The streaming analysis of a dynamics run.
//!
//! Three things run over the audio as it arrives, each at a fixed lag behind the newest
//! sample, so what they decide depends on sample positions only, never on how the audio was
//! cut into blocks:
//!
//! 1. the edge finder, on the reference's one-period envelope `|R₁|`: the programme's start,
//!    the linearity check's 6 dB edge (which fixes the programme's period grid), then every
//!    burst start and end, each checked against the grid and the programme, and a guard that
//!    the reference holds its level between edges (a capture gap or a foreign signal fails it);
//! 2. the rest watch, during each rest: the gain of every input once per millisecond or so,
//!    and from `rest_min` on, whether the medians of its last two 1 s windows agree — the
//!    release the programme source waits for;
//! 3. once a rest is over, that step's full analysis (`step::StepJob`) on the raw audio kept
//!    since just before its burst, run `WORK_PER_SAMPLE` samples per input for each sample
//!    fed (so no block carries a whole step) and finished at once in the run's last block; a
//!    level's rows are finished when its last step is.

mod dsp;
mod pacing;
mod step;
mod watch;

#[cfg(test)]
pub(super) use step::THUMP_MIN_PERIODS;

use std::collections::VecDeque;

use num_complex::Complex64;

use self::dsp::{Boxcar, Mixers, Signal, centred_at, median};
use self::pacing::Job;
#[cfg(test)]
pub(super) use self::pacing::WORK_PER_SAMPLE;
use self::step::{Bounds, Ctx, Partial};
use super::{DynamicsError, DynamicsResult, InputResult, Onset, Phase, Programme, Rests, StepInfo};

/// The reference must carry the programme within this long of the first sample fed, s.
pub const NO_REFERENCE_SECONDS: f64 = 5.0;

/// The reference counts as present once its envelope is above this, peak re full scale
/// (−80 dBFS): well under any pilot worth testing with, well over an interface's noise in a
/// one-period band.
const PRESENT_PEAK: f64 = 1e-4;

/// The gain at the pilot and 6 dB below it must agree this closely, dB.
pub const PILOT_GATE_DB: f64 = 0.1;

/// The linearity check's step must read 6 dB in the reference within this, dB.
const CHECK_STEP_TOLERANCE_DB: f64 = 0.5;

/// Longest processor delay searched for, s.
pub const MAX_DELAY_SECONDS: f64 = 0.1;

/// A rest is flat when the medians of its last two windows of this length agree within
/// [`FLAT_DB`] on every input.
pub const FLAT_WINDOW_SECONDS: f64 = 1.0;

/// See [`FLAT_WINDOW_SECONDS`], dB.
pub const FLAT_DB: f64 = 0.05;

/// The rest watch reads the gain about this often, s.
const WATCH_STEP_SECONDS: f64 = 0.001;

/// And tests for flatness about this often, s.
const WATCH_EVAL_SECONDS: f64 = 0.025;

/// Between edges the reference envelope must stay within this ratio of its level (±3 dB):
/// a capture gap reads as a drop to silence, a foreign signal as a jump.
const GUARD: (f64, f64) = (0.707, 1.414);

/// The raw audio from a sample on, by absolute index.
#[derive(Debug)]
struct Store {
    base: u64,
    refr: Vec<f32>,
    inputs: Vec<Vec<f32>>,
}

impl Store {
    fn end(&self) -> u64 {
        self.base + self.refr.len() as u64
    }

    fn reference(&self) -> Signal<'_> {
        Signal {
            x: &self.refr,
            abs0: self.base,
        }
    }

    /// Input `i` shifted by `d` samples: sample `v` of the result is sample `v + d` of the
    /// input. Valid from `base − d` on.
    fn input(&self, i: usize, d: i64) -> Signal<'_> {
        let skip = d.max(0) as usize;
        Signal {
            x: &self.inputs[i][skip.min(self.inputs[i].len())..],
            abs0: (self.base as i64 + skip as i64 - d) as u64,
        }
    }

    fn trim(&mut self, keep_from: u64) {
        if keep_from <= self.base {
            return;
        }
        let k = ((keep_from - self.base) as usize).min(self.refr.len());
        self.refr.drain(..k);
        for x in &mut self.inputs {
            x.drain(..k);
        }
        self.base += k as u64;
    }
}

/// Where the edge finder is.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Seg {
    /// No programme yet.
    Waiting,
    /// The check's low half, from `s0` (where the reference appeared); its level once read.
    CheckLow {
        s0: u64,
        a_low: Option<f64>,
    },
    CheckHigh,
    Burst(usize),
    Rest(usize),
}

/// The period grid, fixed by the linearity check's 6 dB edge.
#[derive(Debug, Clone, Copy)]
struct Grid {
    /// The edge's midpoint in `|R₁|`, fractional sample.
    x6: f64,
    /// Its first sample at `pilot`.
    e6: u64,
    /// `|R₁|` at `pilot` (peak amplitude).
    pilot: f64,
    /// Whether `pilot` has been read (it is twice the low half's level until then).
    pilot_read: bool,
}

/// A step's edges as found.
#[derive(Debug, Clone, Copy)]
struct Found {
    s: u64,
    e: Option<u64>,
    end: Option<u64>,
    recovered: Option<bool>,
}

/// What the linearity check gave each input.
#[derive(Debug, Clone)]
struct Cal {
    shift: Vec<i64>,
    g0: Vec<f64>,
}

impl Cal {
    fn shift_max(&self) -> u64 {
        self.shift
            .iter()
            .map(|&d| d.max(0) as u64)
            .max()
            .unwrap_or(0)
    }

    fn shift_abs(&self) -> u64 {
        self.shift
            .iter()
            .map(|d| d.unsigned_abs())
            .max()
            .unwrap_or(0)
    }
}

/// The gain of one rest, read as it arrives.
#[derive(Debug)]
struct Watch {
    step: usize,
    e: u64,
    next: u64,
    /// Per input, one gain reading per watch step, dB re `g₀`.
    g: Vec<Vec<f64>>,
}

/// Work that runs once enough audio has arrived.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Task {
    Check,
    WatchPoint,
    WatchEnd,
    /// The step's audio is all in: its analysis may run.
    Step(usize),
}

/// Analyses a dynamics run as its audio arrives: feed the reference and every measured input
/// block by block ([`Analyser::push`]), publish [`Analyser::released`] to the programme's
/// [`super::Release`], and take the result with [`Analyser::finish`] once the phase is
/// [`Phase::Done`]. The result does not depend on how the audio is cut into blocks.
#[derive(Debug)]
pub struct Analyser {
    prog: Programme,
    fs: f64,
    p: u64,
    mx: Mixers,
    store: Store,
    /// Absolute index of the next sample fed.
    pos: u64,
    error: Option<DynamicsError>,

    // Edge finder.
    seg: Seg,
    /// The next envelope sample (centre) to compute.
    c: u64,
    /// Full-weight running sum of the reference's carrier mix over `[c − P/2, c + P/2]`.
    sum: Complex64,
    since_sync: u32,
    prev_env: f64,
    /// Envelope samples awaiting the guard, which looks back far enough that an edge
    /// nearby has been found.
    lagged: VecDeque<(u64, f64)>,
    /// The last four periods of envelope, for refining an edge.
    hist: VecDeque<(u64, f64)>,
    /// A coarse edge crossing waiting for two periods after it: midpoint, rising.
    pending: Option<(f64, bool)>,
    grid: Option<Grid>,
    /// Every edge found: first sample at the new level, and the reference level after it.
    edges: Vec<(u64, f64)>,
    steps: Vec<Found>,

    cal: Option<Cal>,
    watch: Option<Watch>,
    released: u32,
    /// Steps whose rests are decided (recovered or not).
    watched: usize,
    /// Steps whose audio is all in.
    queued: usize,
    /// Steps analysed so far.
    analysed: usize,
    job: Option<Job>,
    /// The current level's partials, per input.
    level: Vec<Vec<Partial>>,
    infos: Vec<StepInfo>,
    results: Vec<InputResult>,
    done: bool,
    /// Per input and step: the thump's peak and RMS over their floors, dB, before any gate.
    #[cfg(test)]
    pub(super) thump_excess: Vec<Vec<[[f64; 2]; 2]>>,
    /// Samples of step analysis the last push ran.
    #[cfg(test)]
    pub(super) last_work: u64,
}

/// The mean time of the step in `|X₁|` around `at`: the centroid of its rise over two
/// periods either side.
fn rise_centroid(x: Signal<'_>, at: u64, p: u64, mx: &Mixers) -> f64 {
    let from = at - 2 * p;
    let mut b = Boxcar::new(x, mx, from);
    let first = b.value(x, mx).norm();
    let mut prev = first;
    let mut moment = 0.0;
    for u in from + 1..=at + 2 * p {
        b.advance(x, mx);
        let v = b.value(x, mx).norm();
        moment += (u as f64 - 0.5) * (v - prev);
        prev = v;
    }
    moment / (prev - first)
}

impl Analyser {
    /// An analyser of `programme` with `inputs` measured inputs (1 or more).
    pub fn new(programme: &Programme, inputs: usize) -> Self {
        let p = u64::from(programme.period);
        Self {
            prog: programme.clone(),
            fs: programme.sample_rate,
            p,
            mx: Mixers::new(p as usize),
            store: Store {
                base: 0,
                refr: Vec::new(),
                inputs: vec![Vec::new(); inputs],
            },
            pos: 0,
            error: None,
            seg: Seg::Waiting,
            c: p / 2,
            sum: Complex64::new(0.0, 0.0),
            since_sync: u32::MAX,
            prev_env: 0.0,
            lagged: VecDeque::new(),
            hist: VecDeque::new(),
            pending: None,
            grid: None,
            edges: Vec::new(),
            steps: Vec::new(),
            cal: None,
            watch: None,
            released: 0,
            watched: 0,
            queued: 0,
            analysed: 0,
            job: None,
            level: vec![Vec::new(); inputs],
            infos: Vec::new(),
            results: vec![
                InputResult {
                    delay_samples: 0.0,
                    g0_db: 0.0,
                    rows: Vec::new(),
                    envelopes: Vec::new(),
                };
                inputs
            ],
            done: false,
            #[cfg(test)]
            thump_excess: vec![Vec::new(); inputs],
            #[cfg(test)]
            last_work: 0,
        }
    }

    /// Feeds the next block: the reference and each measured input, equal lengths,
    /// contiguous with the previous block. Audio after [`Phase::Done`] is ignored; after an
    /// error, every call returns it again.
    ///
    /// # Panics
    ///
    /// If the number of inputs differs from [`Analyser::new`]'s or a block's length from the
    /// reference's.
    pub fn push(&mut self, reference: &[f32], inputs: &[&[f32]]) -> Result<(), DynamicsError> {
        assert_eq!(inputs.len(), self.store.inputs.len(), "one block per input");
        assert!(
            inputs.iter().all(|x| x.len() == reference.len()),
            "blocks of equal length"
        );
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        if self.done {
            return Ok(());
        }
        self.store.refr.extend_from_slice(reference);
        for (s, x) in self.store.inputs.iter_mut().zip(inputs) {
            s.extend_from_slice(x);
        }
        self.pos += reference.len() as u64;
        let r = self.advance();
        if let Err(e) = &r {
            self.error = Some(e.clone());
            // Nothing more is analysed: the raw audio can go.
            self.job = None;
            self.store.trim(self.store.end());
            return r;
        }
        self.work(reference.len() as u64);
        Ok(())
    }

    /// Samples of raw audio kept.
    #[cfg(test)]
    pub(super) fn retained(&self) -> usize {
        self.store.refr.len()
    }

    /// Rests found recovered so far (1-based count, monotonic): what the source may end.
    pub fn released(&self) -> u32 {
        self.released
    }

    /// Where the run is.
    pub fn phase(&self) -> Phase {
        if self.done {
            return Phase::Done;
        }
        match self.seg {
            Seg::Waiting => Phase::Waiting,
            Seg::CheckLow { .. } | Seg::CheckHigh => Phase::Check,
            Seg::Burst(i) => Phase::Burst(i as u32),
            Seg::Rest(i) => Phase::Rest(i as u32),
        }
    }

    /// The result, once [`Phase::Done`]; before it, the run ended early and this is
    /// [`DynamicsError::Mismatch`].
    pub fn finish(self) -> Result<DynamicsResult, DynamicsError> {
        if let Some(e) = self.error {
            return Err(e);
        }
        if !self.done {
            return Err(match self.seg {
                Seg::Waiting => DynamicsError::NoReference,
                _ => DynamicsError::Mismatch(format!(
                    "the capture ended at step {} of {}",
                    self.queued + 1,
                    self.prog.steps()
                )),
            });
        }
        Ok(DynamicsResult {
            steps: self.infos,
            inputs: self.results,
        })
    }

    // -----------------------------------------------------------------------------------
    // Scheduling

    fn h(&self) -> u64 {
        self.p / 2
    }

    fn secs(&self, s: f64) -> u64 {
        (s * self.fs).round() as u64
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            mx: &self.mx,
            fs: self.fs,
            carrier_hz: self.prog.carrier_hz(),
        }
    }

    /// Samples a step's analysis needs past its rest's end.
    fn step_lookahead(&self, cal: &Cal) -> u64 {
        self.ctx().margin() + cal.shift_max() + 1
    }

    /// The pending task that becomes possible first, and the number of samples fed it
    /// needs. Runs in that order, interleaved with the edge finder, so every decision is
    /// taken at the same sample whatever the block size.
    fn next_task(&self) -> Option<(u64, Task)> {
        let grid = self.grid?;
        let b0 = grid.e6 + u64::from(self.prog.check_periods) * self.p;
        let Some(cal) = &self.cal else {
            return Some((
                b0 + self.h() + 2 + self.secs(MAX_DELAY_SECONDS),
                Task::Check,
            ));
        };
        if self.queued == self.prog.steps() {
            return None;
        }
        let watch = self.watch.as_ref().map(|w| {
            if self
                .watch_limit(w)
                .is_some_and(|l| w.next + self.h() + cal.shift_max() >= l)
            {
                (0, Task::WatchEnd)
            } else {
                // The edge finder has looked past the point's window, and refined any edge
                // there, by then.
                (w.next + 3 * self.p + cal.shift_max() + 3, Task::WatchPoint)
            }
        });
        let step = self.steps.get(self.queued).and_then(|f| {
            f.recovered?;
            Some((f.end? + self.step_lookahead(cal), Task::Step(self.queued)))
        });
        match (watch, step) {
            (Some(w), Some(s)) => Some(if s.0 < w.0 { s } else { w }),
            (w, s) => w.or(s),
        }
    }

    fn advance(&mut self) -> Result<(), DynamicsError> {
        loop {
            self.start_watch();
            let next = self.next_task().filter(|t| t.0 <= self.pos);
            let target = next.map_or(self.pos, |t| t.0.max(self.c));
            if self.find_edges(target)? {
                continue;
            }
            match next {
                None => return Ok(()),
                Some((_, Task::Check)) => self.run_check()?,
                Some((_, Task::WatchPoint)) => self.watch_point(),
                Some((_, Task::WatchEnd)) => self.watch_end(),
                Some((_, Task::Step(i))) => self.queued = i + 1,
            }
            if self.queued == self.prog.steps() {
                // The run's audio is all in; what follows is not the programme's.
                return Ok(());
            }
        }
    }

    // -----------------------------------------------------------------------------------
    // Edge finder

    /// Envelope samples until `target` samples have been fed (the centre `c` needs sample
    /// `c + P/2`). Returns `true` after finding an edge, so the schedule is redone.
    fn find_edges(&mut self, target: u64) -> Result<bool, DynamicsError> {
        let h = self.h();
        let keep = (4 * self.p + 8) as usize;
        // The guard looks back past the two periods an edge waits for its refinement.
        let lag = 3 * self.p + 4;
        while self.c + h < target.min(self.pos) {
            let env = self.env_next();
            let c = self.c;
            self.c += 1;
            self.hist.push_back((c, env));
            if self.hist.len() > keep {
                self.hist.pop_front();
            }
            let found = self.edge_step(c, env)?;
            self.prev_env = env;
            self.lagged.push_back((c, env));
            while let Some(&(lc, le)) = self.lagged.front() {
                if lc + lag > c {
                    break;
                }
                self.lagged.pop_front();
                self.guard(lc, le)?;
            }
            if matches!(self.seg, Seg::Waiting) && self.c > self.store.base + 64 * self.p {
                // Nothing before the programme is needed.
                self.store.trim(self.c - 2 * self.p);
            }
            if found {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `2|R₁|` at `self.c`, from the running sum.
    fn env_next(&mut self) -> f64 {
        let h = self.h();
        let c = self.c;
        let r = self.store.reference();
        let term = |v: u64| self.mx.carrier(v) * r.get(v);
        if self.since_sync >= 4096 {
            self.sum = (c - h..=c + h).map(term).sum();
            self.since_sync = 0;
        } else {
            self.sum += term(c + h) - term(c - h - 1);
            self.since_sync += 1;
        }
        ((self.sum - 0.5 * (term(c - h) + term(c + h))) * (2.0 / self.p as f64)).norm()
    }

    /// Reference level of burst `i` (peak amplitude).
    fn burst_ref(&self, grid: &Grid, i: usize) -> f64 {
        grid.pilot * 10f64.powf((self.prog.step_level_dbfs(i) - self.prog.pilot_dbfs) / 20.0)
    }

    /// Where between `c − 1` and `c` the envelope crossed `th` in the given direction.
    fn crossed(&self, c: u64, env: f64, th: f64, up: bool) -> Option<f64> {
        let prev = self.prev_env;
        let hit = if up {
            prev < th && env >= th
        } else {
            prev > th && env <= th
        };
        hit.then(|| (c - 1) as f64 + (th - prev) / (env - prev))
    }

    fn mismatch(&self, what: String) -> DynamicsError {
        DynamicsError::Mismatch(what)
    }

    /// A found midpoint `x` against the edge expected at first sample `at`.
    fn check_edge(&self, grid: &Grid, x: f64, at: u64, what: &str) -> Result<(), DynamicsError> {
        let off = x - (at as f64 + grid.x6 - grid.e6 as f64);
        if off.abs() > 1.0 {
            return Err(self.mismatch(format!("{what} is {off:+.1} samples off the programme")));
        }
        Ok(())
    }

    /// The crossing of the exact midpoint between the levels two periods either side of a
    /// coarse crossing `xc`. A zero-onset step's centred envelope is nearly flat at its
    /// midpoint (both ends of the window sit on zero crossings of the carrier), so a
    /// threshold from the nominal levels can land samples off; the midpoint of the levels
    /// actually there is crossed exactly at the edge.
    fn refine(&self, xc: f64, up: bool) -> f64 {
        let i0 = xc.round() as u64;
        let p = self.p;
        let at = |u: u64| self.hist.iter().find(|q| q.0 == u).map(|q| q.1);
        let (Some(a), Some(b)) = (at(i0 - 2 * p), at(i0 + 2 * p)) else {
            return xc;
        };
        let th = (a + b) / 2.0;
        let mut prev: Option<(u64, f64)> = None;
        for &(u, v) in self.hist.iter().filter(|q| q.0 + p >= i0 && q.0 <= i0 + p) {
            if let Some((pu, pv)) = prev {
                let hit = if up {
                    pv < th && v >= th
                } else {
                    pv > th && v <= th
                };
                if hit {
                    return pu as f64 + (th - pv) / (v - pv);
                }
            }
            prev = Some((u, v));
        }
        xc
    }

    fn edge_step(&mut self, c: u64, env: f64) -> Result<bool, DynamicsError> {
        let p = self.p;
        if let Some((xc, up)) = self.pending {
            if c < xc.round() as u64 + 2 * p {
                return Ok(false);
            }
            self.pending = None;
            let x = self.refine(xc, up);
            self.commit(x)?;
            return Ok(true);
        }
        match self.seg {
            Seg::Waiting => {
                if env > PRESENT_PEAK {
                    self.seg = Seg::CheckLow { s0: c, a_low: None };
                    // The check needs its whole low half.
                    self.store.trim(c.saturating_sub(4 * p));
                } else if c >= self.secs(NO_REFERENCE_SECONDS) {
                    return Err(DynamicsError::NoReference);
                }
            }
            Seg::CheckLow { s0, a_low: None } => {
                if env < PRESENT_PEAK / 2.0 {
                    self.seg = Seg::Waiting;
                } else if c >= s0 + self.secs(0.1) {
                    self.seg = Seg::CheckLow {
                        s0,
                        a_low: Some(env),
                    };
                }
            }
            Seg::CheckLow {
                s0,
                a_low: Some(a_low),
            } => {
                let check = u64::from(self.prog.check_periods) * p;
                if let Some(x) = self.crossed(c, env, 1.5 * a_low, true) {
                    self.pending = Some((x, true));
                } else if c > s0 + check + self.secs(0.1) {
                    return Err(
                        self.mismatch("no 6 dB step where the linearity check should end".into())
                    );
                }
            }
            Seg::CheckHigh => {
                let Some(mut grid) = self.grid else {
                    return Ok(false);
                };
                if !grid.pilot_read && c >= grid.e6 + self.secs(0.1) {
                    let a_low = grid.pilot / 2.0;
                    let step = 20.0 * (env / a_low).log10();
                    if (step - 20.0 * 2f64.log10()).abs() > CHECK_STEP_TOLERANCE_DB {
                        return Err(self.mismatch(format!(
                            "the linearity check steps {step:.2} dB, not 6 dB"
                        )));
                    }
                    grid.pilot = env;
                    grid.pilot_read = true;
                    self.grid = Some(grid);
                    if let Some(last) = self.edges.last_mut() {
                        last.1 = env;
                    }
                }
                let b0 = grid.e6 + u64::from(self.prog.check_periods) * p;
                let b = self.burst_ref(&grid, 0);
                if let Some(x) = self.crossed(c, env, (grid.pilot + b) / 2.0, true) {
                    self.pending = Some((x, true));
                } else if c > b0 + p + 2 {
                    return Err(self.mismatch("the first burst did not start".into()));
                }
            }
            Seg::Burst(i) => {
                let Some(grid) = self.grid else {
                    return Ok(false);
                };
                let e = self.steps[i].s + u64::from(self.prog.step_periods(i)) * p;
                let b = self.burst_ref(&grid, i);
                if let Some(x) = self.crossed(c, env, (grid.pilot + b) / 2.0, false) {
                    self.pending = Some((x, false));
                } else if c > e + p + 2 {
                    return Err(self.mismatch(format!(
                        "step {}'s burst did not end after {} periods",
                        i + 1,
                        self.prog.step_periods(i)
                    )));
                }
            }
            Seg::Rest(i) => {
                if i + 1 >= self.prog.steps() {
                    return Ok(false);
                }
                let (Some(grid), Some(e)) = (self.grid, self.steps[i].e) else {
                    return Ok(false);
                };
                let b = self.burst_ref(&grid, i + 1);
                let latest = match &self.prog.rests {
                    Rests::Replay(r) => e + u64::from(r[i]) * p,
                    Rests::Adaptive => e + u64::from(self.prog.rest_max_periods) * p,
                };
                if let Some(x) = self.crossed(c, env, (grid.pilot + b) / 2.0, true) {
                    self.pending = Some((x, true));
                } else if c > latest + p + 2 {
                    return Err(self.mismatch(format!("step {} did not start", i + 2)));
                }
            }
        }
        Ok(false)
    }

    /// Takes a refined edge midpoint `x` as the edge the current segment waits for, or
    /// refuses it.
    fn commit(&mut self, x: f64) -> Result<(), DynamicsError> {
        let p = self.p;
        match self.seg {
            Seg::CheckLow {
                s0,
                a_low: Some(a_low),
            } => {
                let check = u64::from(self.prog.check_periods) * p;
                // The step's midpoint in a centred one-period envelope: on the zero sample
                // itself for a zero onset, half a sample before the first new sample for a
                // peak onset.
                let lead = match self.prog.onset {
                    Onset::Zero => 0.0,
                    Onset::Peak => 0.5,
                };
                let e6 = (x + lead).round() as u64;
                let start = e6 as i64 - check as i64;
                let s0 = s0 as i64;
                // The programme fades in over the generator's ramp, inside which the
                // reference first shows.
                let ramp = self.secs(crate::generator::RAMP_SECONDS + 0.01) as i64;
                if s0 < start - p as i64 || s0 > start + ramp {
                    return Err(self.mismatch(format!(
                        "the linearity check's step is {:.3} s after the reference started, not {:.3} s",
                        (e6 as i64 - s0) as f64 / self.fs,
                        check as f64 / self.fs
                    )));
                }
                self.grid = Some(Grid {
                    x6: x,
                    e6,
                    pilot: 2.0 * a_low,
                    pilot_read: false,
                });
                self.edges.push((e6, 2.0 * a_low));
                self.seg = Seg::CheckHigh;
            }
            Seg::CheckHigh => {
                let Some(grid) = self.grid else {
                    return Ok(());
                };
                let b0 = grid.e6 + u64::from(self.prog.check_periods) * p;
                self.check_edge(&grid, x, b0, "the first burst's start")?;
                let b = self.burst_ref(&grid, 0);
                self.begin_burst(b0, b);
            }
            Seg::Burst(i) => {
                let Some(grid) = self.grid else {
                    return Ok(());
                };
                let e = self.steps[i].s + u64::from(self.prog.step_periods(i)) * p;
                self.check_edge(&grid, x, e, &format!("step {}'s end", i + 1))?;
                self.steps[i].e = Some(e);
                self.edges.push((e, grid.pilot));
                self.seg = Seg::Rest(i);
            }
            Seg::Rest(i) => {
                let (Some(grid), Some(e)) = (self.grid, self.steps[i].e) else {
                    return Ok(());
                };
                let what = format!("step {}'s start", i + 2);
                let next = match &self.prog.rests {
                    Rests::Replay(r) => {
                        let at = e + u64::from(r[i]) * p;
                        self.check_edge(&grid, x, at, &what)?;
                        at
                    }
                    Rests::Adaptive => {
                        let k = ((x - grid.x6) / p as f64).round().max(0.0) as u64;
                        let at = grid.e6 + k * p;
                        self.check_edge(&grid, x, at, &what)?;
                        let rest = at.saturating_sub(e) / p;
                        if at < e
                            || rest < u64::from(self.prog.rest_min_periods)
                            || rest > u64::from(self.prog.rest_max_periods)
                        {
                            return Err(self.mismatch(format!(
                                "the rest before {what} lasts {:.3} s",
                                (at as f64 - e as f64) / self.fs
                            )));
                        }
                        at
                    }
                };
                self.steps[i].end = Some(next);
                let b = self.burst_ref(&grid, i + 1);
                self.begin_burst(next, b);
            }
            Seg::Waiting | Seg::CheckLow { a_low: None, .. } => {}
        }
        Ok(())
    }

    fn begin_burst(&mut self, s: u64, level: f64) {
        self.edges.push((s, level));
        self.steps.push(Found {
            s,
            e: None,
            end: None,
            recovered: None,
        });
        self.seg = Seg::Burst(self.steps.len() - 1);
    }

    /// The reference holds its level between edges.
    fn guard(&self, c: u64, env: f64) -> Result<(), DynamicsError> {
        let (level, from) = match (self.seg, self.edges.iter().rev().find(|x| x.0 <= c)) {
            (Seg::Waiting, _) => return Ok(()),
            (Seg::CheckLow { s0, a_low }, _) => match a_low {
                Some(a) => (a, s0 + self.secs(0.1)),
                None => return Ok(()),
            },
            (_, Some(&(at, level))) => (level, at),
            (_, None) => match self.seg_low_level() {
                Some(v) => v,
                None => return Ok(()),
            },
        };
        if c < from
            || self
                .edges
                .iter()
                .any(|&(at, _)| c + self.p + 1 >= at && c <= at + self.p + 1)
        {
            return Ok(());
        }
        // The last rest may fade out once released, from its shortest length on.
        if let Some(e) = self.last_rest_start()
            && c >= e + u64::from(self.prog.rest_min_periods) * self.p
        {
            return Ok(());
        }
        let ratio = env / level;
        if !(GUARD.0..=GUARD.1).contains(&ratio) {
            return Err(self.mismatch(format!(
                "the reference left its level by {:+.1} dB at {:.3} s (a capture gap?)",
                20.0 * ratio.log10(),
                c as f64 / self.fs
            )));
        }
        Ok(())
    }

    /// Before the 6 dB edge (found after the low half started): the low half's level.
    fn seg_low_level(&self) -> Option<(f64, u64)> {
        let grid = self.grid?;
        Some((grid.pilot / 2.0, 0))
    }

    fn last_rest_start(&self) -> Option<u64> {
        (self.steps.len() == self.prog.steps())
            .then(|| self.steps.last().and_then(|f| f.e))
            .flatten()
    }

    // -----------------------------------------------------------------------------------
    // Linearity check

    fn run_check(&mut self) -> Result<(), DynamicsError> {
        let Some(grid) = self.grid else {
            return Ok(());
        };
        let p = self.p;
        let h = self.h();
        let b0 = grid.e6 + u64::from(self.prog.check_periods) * p;
        let fs = self.fs;
        let sec = |s: f64| (s * fs).round() as u64;
        let r = self.store.reference();
        let n = self.store.inputs.len();
        let (mut delay, mut shift, mut g0) = (vec![0.0; n], vec![0i64; n], vec![0.0; n]);
        let mut worst: Option<(f64, usize)> = None;
        for i in 0..n {
            let y = self.store.input(i, 0);
            let level = |from: u64, to: u64| {
                let mut v: Vec<f64> = (from..to)
                    .step_by(p as usize)
                    .map(|u| centred_at(y, u, &self.mx).norm())
                    .collect();
                median(&mut v)
            };
            let yl = level(grid.e6 - sec(1.5), grid.e6 - sec(0.5));
            let yh = level(grid.e6 + sec(0.5), grid.e6 + sec(1.5));
            // Where the input's step is, roughly: its midpoint crossing.
            let th = (yl + yh) / 2.0;
            let from = grid.e6 - p - 1;
            let mut b = Boxcar::new(y, &self.mx, from);
            let mut prev = b.value(y, &self.mx).norm();
            let mut x_y = None;
            for u in from + 1..=grid.e6 + sec(MAX_DELAY_SECONDS) {
                b.advance(y, &self.mx);
                let v = b.value(y, &self.mx).norm();
                if prev < th && v >= th {
                    x_y = Some((u - 1) as f64 + (th - prev) / (v - prev));
                    break;
                }
                prev = v;
            }
            // The delay is the difference of the two steps' mean times (the centroids of
            // their envelopes' rise). A delay shifts the envelope's magnitude whole, so the
            // centroid moves with it exactly; a midpoint crossing would not do: a zero-onset
            // step's envelope is nearly flat at its midpoint, and any small level change
            // there (an even-order distortion's DC stepping with the level) moves the
            // crossing by samples.
            let dl = x_y.map_or(0.0, |x| {
                let i0 = grid.x6.round() as u64;
                rise_centroid(y, x.round() as u64, p, &self.mx) - rise_centroid(r, i0, p, &self.mx)
            });
            let d = dl.round() as i64;
            let ys = self.store.input(i, d);
            let gain = |from: u64, to: u64| {
                let mut rb = Boxcar::new(r, &self.mx, from);
                let mut yb = Boxcar::new(ys, &self.mx, from);
                let mut v = Vec::with_capacity((to - from) as usize);
                for _ in from..to {
                    v.push(
                        20.0 * (yb.value(ys, &self.mx).norm() / rb.value(r, &self.mx).norm())
                            .log10(),
                    );
                    rb.advance(r, &self.mx);
                    yb.advance(ys, &self.mx);
                }
                median(&mut v)
            };
            let low = gain(grid.e6 - sec(1.0), grid.e6 - p - h);
            let high = gain(b0 - sec(1.0), b0 - p - h);
            let diff = if (high - low).is_finite() {
                high - low
            } else {
                f64::INFINITY
            };
            if diff.abs() > PILOT_GATE_DB && worst.is_none_or(|w| diff.abs() > w.0.abs()) {
                worst = Some((diff, i));
            }
            delay[i] = dl;
            shift[i] = d;
            g0[i] = high;
        }
        if let Some((diff_db, input)) = worst {
            return Err(DynamicsError::PilotNotBelowThreshold { diff_db, input });
        }
        for (res, (&dl, &g)) in self.results.iter_mut().zip(delay.iter().zip(&g0)) {
            res.delay_samples = dl;
            res.g0_db = g;
        }
        let cal = Cal { shift, g0 };
        let keep = self.ctx().pre() + self.ctx().margin() + cal.shift_abs() + p;
        self.cal = Some(cal);
        self.store.trim(b0.saturating_sub(keep));
        Ok(())
    }

    // -----------------------------------------------------------------------------------
    // Steps
}
