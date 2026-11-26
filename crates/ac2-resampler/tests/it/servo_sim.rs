//! A deterministic rig for the clock servo: a drifting device crystal, stamps with white
//! noise and Gauss–Markov wander, and a delay tracker that measures the servo's own output
//! against the truth plus a path delay.

use ac2_resampler::servo::{AudioUse, MediaTime, Servo, ServoConfig, StampUse, Status};
use ac2_resampler::{Pos, Step};

pub const FS: u32 = 96_000;
/// One block every 0.1 s of media time, with one stamp per block.
pub const BLOCK: usize = 9_600;
pub const BLOCK_S: f64 = 0.1;
/// Media time of block 0: a whole second far from the epoch, so absolute times are large.
pub const START_NS: u64 = 1_700_000_000_000_000_000;
pub const START_INDEX: u64 = 1_700_000_000 * FS as u64;

/// splitmix64 and Box–Muller: enough for noise, seeded per test.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    pub fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

/// What the truth and the evidence look like.
pub struct Sim {
    pub seed: u64,
    pub secs: f64,
    pub ppm: f64,
    pub drift_ppm_per_h: f64,
    pub rate_walk_ppm_per_h: f64,
    pub wander_sd_s: f64,
    pub wander_tau_s: f64,
    /// White stamp noise as the sd of a one-second mean.
    pub white_s: f64,
    /// One block in this many is stamped, its stamp carrying the white noise of a mean over
    /// those blocks (a caller feeding means).
    pub stamp_every: usize,
    /// While `.0 ≤ t < .0 + .1` no stamps arrive; the device keeps running.
    pub stamp_gaps: Vec<(f64, f64)>,
    /// Each stamp with probability `.0` errs by a further normal error of sd `.1` seconds.
    pub stamp_tail: Option<(f64, f64)>,
    /// The first stamp at or after `.0` seconds errs by a further `.1` seconds.
    pub stamp_spike: Option<(f64, f64)>,
    /// The device's first sample index: above 2³² so no 32-bit shortcut survives.
    pub start_input: u64,
    /// The delay tracker's own sd, passed to the servo as `sigma_s`.
    pub audio_sigma_s: f64,
    /// The path's own white jitter per observation, seconds (not passed to the servo).
    pub path_jitter_s: f64,
    /// Path delay at time t (s), seconds; `None` while no audio is measured.
    pub path: Box<dyn Fn(f64) -> Option<f64>>,
    /// At these times the input has a gap of `.1` seconds and the device renumbers its
    /// samples; the caller calls `reset_phase`.
    pub gaps: Vec<(f64, f64)>,
    /// From `.0` seconds on, every stamp reads `.1` seconds late.
    pub stamp_step: Option<(f64, f64)>,
    /// The first audio observation at or after this time is flagged as a restart.
    pub restart_at: Option<f64>,
    /// From `.0` seconds on, the device runs `.1` ppm faster.
    pub rate_step: Option<(f64, f64)>,
    pub cfg: ServoConfig,
}

impl Sim {
    pub fn stamps_only(seed: u64, secs: f64) -> Self {
        Self {
            seed,
            secs,
            ppm: 7.0,
            drift_ppm_per_h: 0.25,
            rate_walk_ppm_per_h: 0.1,
            wander_sd_s: 15.5e-6,
            wander_tau_s: 100.0,
            white_s: 5e-6,
            stamp_every: 1,
            stamp_gaps: Vec::new(),
            stamp_tail: None,
            stamp_spike: None,
            start_input: (1 << 33) + 12_345,
            audio_sigma_s: 0.3e-6,
            path_jitter_s: 0.0,
            path: Box::new(|_| None),
            gaps: Vec::new(),
            stamp_step: None,
            restart_at: None,
            rate_step: None,
            cfg: ServoConfig::default(),
        }
    }
}

/// One block of the run.
#[derive(Clone, Copy, Debug)]
pub struct Rec {
    pub t: f64,
    /// Handed position minus the true device position, seconds; `None` while acquiring.
    pub err_s: Option<f64>,
    pub pos: Option<Pos>,
    pub step: Option<Step>,
    pub true_ppm: f64,
    pub status: Status,
    pub audio: Option<AudioUse>,
    pub stamp: Option<StampUse>,
}

pub struct Run {
    pub recs: Vec<Rec>,
}

/// A device position: whole samples and a fraction, so its precision does not decay with
/// the index.
#[derive(Clone, Copy, Debug)]
pub struct DevPos {
    pub whole: u64,
    pub frac: f64,
}

impl DevPos {
    fn add(self, d: f64) -> Self {
        let s = self.frac + d;
        let w = s.floor();
        Self {
            whole: (self.whole as i64 + w as i64) as u64,
            frac: s - w,
        }
    }

    /// `pos − self`, samples.
    pub fn to(self, pos: Pos) -> f64 {
        (pos.index() as i64 - self.whole as i64) as f64 + pos.fraction() - self.frac
    }
}

/// The true device position of each block start, kept for the end-to-end tone.
pub struct Truth {
    pub x: Vec<DevPos>,
    /// Device samples renumber at a gap: reported index = true index − offset.
    pub offset: Vec<u64>,
}

pub fn run(sim: &Sim, servo: &mut Servo) -> (Run, Truth) {
    let mut rng = Rng::new(sim.seed);
    // The tail draws from its own stream, so a run with it differs from one without only
    // by the tail.
    let mut tail_rng = Rng::new(sim.seed ^ 0x7A11);
    let fs = f64::from(FS);
    let n = (sim.secs / BLOCK_S).round() as usize;
    let a = (-BLOCK_S / sim.wander_tau_s).exp();
    let mut wander = sim.wander_sd_s * rng.normal();
    let mut walk = 0.0;
    let walk_step = sim.rate_walk_ppm_per_h * 1e-6 * (BLOCK_S / 3600.0).sqrt();
    let white = sim.white_s / (BLOCK_S * sim.stamp_every as f64).sqrt();
    let mut x = DevPos {
        whole: sim.start_input,
        frac: 0.678,
    };
    let mut spike = sim.stamp_spike;
    let mut offset = 0u64;
    let mut gaps = sim.gaps.iter().copied().peekable();
    let mut gap_until = f64::NEG_INFINITY;
    let mut handed: Vec<Option<Pos>> = Vec::with_capacity(n);
    let mut truth = Truth {
        x: Vec::with_capacity(n),
        offset: Vec::with_capacity(n),
    };
    let mut recs = Vec::with_capacity(n);
    let mut restart_at = sim.restart_at;
    for k in 0..n {
        let t = k as f64 * BLOCK_S;
        let stepped = sim
            .rate_step
            .filter(|&(at, _)| t >= at)
            .map_or(0.0, |s| s.1);
        let rate = (sim.ppm + stepped + sim.drift_ppm_per_h * t / 3600.0) * 1e-6 + walk;
        let ns = START_NS + k as u64 * 100_000_000;
        if gaps.peek().is_some_and(|&(at, _)| t >= at) {
            let (_, len) = gaps.next().unwrap_or_default();
            gap_until = t + len;
            servo.reset_phase();
            // The restarted device numbers from about where it is, minus a large jump.
            offset = x.whole - 1_000;
        }
        truth.x.push(x);
        truth.offset.push(offset);
        let withheld = sim
            .stamp_gaps
            .iter()
            .any(|&(at, len)| (at..at + len).contains(&t));
        let mut stamp_use = None;
        if t >= gap_until && !withheld && k % sim.stamp_every == 0 {
            // The block's first whole sample, captured `frac` samples after the block start.
            let n_true = x.whole + 1;
            let lead = (1.0 - x.frac) / (fs * (1.0 + rate));
            let late = sim
                .stamp_step
                .filter(|&(at, _)| t >= at)
                .map_or(0.0, |s| s.1);
            let mut err = wander + white * rng.normal() + late;
            if let Some((p, sd)) = sim.stamp_tail {
                let extra = sd * tail_rng.normal();
                if tail_rng.uniform() < p {
                    err += extra;
                }
            }
            if let Some((_, e)) = spike.filter(|&(at, _)| t >= at) {
                err += e;
                spike = None;
            }
            let stamp = ns as i64 + ((lead + err) * 1e9).round() as i64;
            stamp_use =
                Some(servo.observe_stamp(n_true - offset, MediaTime::from_nanos(stamp as u64)));
        }
        let mut audio = None;
        if k >= 5 && k % 10 == 0 {
            let back = k - 5;
            let tb = back as f64 * BLOCK_S;
            if let (Some(path), Some(Some(h))) = ((sim.path)(tb), handed.get(back))
                && truth.offset[back] == offset
            {
                let xb = truth.x[back];
                let real = DevPos {
                    whole: xb.whole - offset,
                    frac: xb.frac,
                };
                let behind = -real.to(*h);
                let delay = path
                    + behind / fs
                    + sim.audio_sigma_s * rng.normal()
                    + sim.path_jitter_s * rng.normal();
                let at = MediaTime::from_nanos(START_NS + back as u64 * 100_000_000);
                let restart = restart_at.is_some_and(|r| tb >= r);
                if restart {
                    restart_at = None;
                }
                audio = Some(servo.observe_audio(delay, sim.audio_sigma_s, at, restart));
            }
        }
        let first = START_INDEX + (k * BLOCK) as u64;
        let (err_s, step, pos) = match servo.block(first, BLOCK) {
            Ok((pos, step)) => {
                let real = DevPos {
                    whole: x.whole - offset,
                    frac: x.frac,
                };
                (Some(real.to(pos) / fs), Some(step), Some(pos))
            }
            Err(_) => (None, None, None),
        };
        handed.push(pos);
        recs.push(Rec {
            t,
            err_s,
            pos,
            step,
            true_ppm: rate * 1e6,
            status: servo.status(MediaTime::from_nanos(ns)),
            audio,
            stamp: stamp_use,
        });
        x = x.add(BLOCK as f64 * (1.0 + rate));
        wander = a * wander + sim.wander_sd_s * (1.0 - a * a).sqrt() * rng.normal();
        walk += walk_step * rng.normal();
    }
    (Run { recs }, truth)
}

/// Mean and sd.
pub fn mean_sd(v: &[f64]) -> (f64, f64) {
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    (mean, var.sqrt())
}

impl Run {
    /// Errors (s) from `from` seconds on, every block handed.
    pub fn errors(&self, from: f64) -> Vec<f64> {
        self.recs
            .iter()
            .filter(|r| r.t >= from)
            .filter_map(|r| r.err_s)
            .collect()
    }

    /// Errors minus their centred moving average over `window` seconds: what the phase does
    /// faster than that.
    pub fn high_passed(&self, from: f64, window: f64) -> Vec<f64> {
        let e: Vec<(f64, f64)> = self
            .recs
            .iter()
            .filter(|r| r.t >= from)
            .filter_map(|r| r.err_s.map(|e| (r.t, e)))
            .collect();
        let half = (window / BLOCK_S / 2.0) as usize;
        let mut prefix = vec![0.0];
        for &(_, v) in &e {
            prefix.push(prefix.last().copied().unwrap_or(0.0) + v);
        }
        (half..e.len().saturating_sub(half))
            .map(|i| e[i].1 - (prefix[i + half + 1] - prefix[i - half]) / (2 * half + 1) as f64)
            .collect()
    }

    /// Rate error (ppm) from `from` seconds on.
    pub fn rate_errors(&self, from: f64) -> Vec<f64> {
        self.recs
            .iter()
            .filter(|r| r.t >= from)
            .filter_map(|r| r.status.rate_ppm.map(|p| p - r.true_ppm))
            .collect()
    }

    /// The largest change of the step's deviation over one second, ppm, from `from` on.
    pub fn step_change_per_s(&self, from: f64) -> f64 {
        let per = (1.0 / BLOCK_S) as usize;
        let s: Vec<f64> = self
            .recs
            .iter()
            .filter(|r| r.t >= from)
            .filter_map(|r| r.step.map(|s| (s.get() - 1.0) * 1e6))
            .collect();
        s.windows(per + 1)
            .map(|w| (w[per] - w[0]).abs())
            .fold(0.0, f64::max)
    }

    /// Peak-to-peak of the errors from `from` on, seconds.
    pub fn peak_to_peak(&self, from: f64) -> f64 {
        let e = self.errors(from);
        let (lo, hi) = e
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), &v| {
                (l.min(v), h.max(v))
            });
        hi - lo
    }

    /// Time (s) of the first block handed at or after `from`.
    pub fn first_block_after(&self, from: f64) -> Option<f64> {
        self.recs
            .iter()
            .find(|r| r.t >= from && r.err_s.is_some())
            .map(|r| r.t)
    }
}
