//! Simulated duplex streams: a generator writes the output stream, the loopback capture is
//! that stream delayed by a (possibly time-varying) offset plus noise.

use super::*;
use crate::generator::{
    BandLimit, Generator, GeneratorConfig, LevelControl, Rng, Signal, dbfs_to_rms,
};

const FS: f64 = 48_000.0;

/// Unoptimised builds instantiate the FFT generics at opt-level 0 (≈15 ms per 2^16
/// transform), so debug runs simulate shorter spans; release runs the full spans.
fn by_build(debug: usize, release: usize) -> usize {
    if cfg!(debug_assertions) {
        debug
    } else {
        release
    }
}
const STIMULUS_DBFS: f64 = -20.0;

type OffsetFn = Box<dyn Fn(u64) -> f64>;

struct Sim {
    generator: Generator,
    level: LevelControl,
    out: Vec<f32>,
    out_base: u64,
    cap: Vec<f32>,
    cap_base: u64,
    offset: OffsetFn,
    noise: Rng,
    noise_rms: f64,
    /// Output index at which the generator fades out, and at which it fades back in.
    stop_at: Option<u64>,
    restart_at: Option<u64>,
}

impl Sim {
    fn new(signal: Signal, snr_db: f64, offset: OffsetFn) -> Self {
        let generator = Generator::new(&GeneratorConfig {
            signal,
            sample_rate: FS,
            seed: 42,
            band: BandLimit::NONE,
            level_dbfs: STIMULUS_DBFS,
            ceiling_dbfs: -10.0,
        })
        .expect("generator");
        let level = generator.level_control();
        Self {
            generator,
            level,
            out: Vec::new(),
            out_base: 0,
            cap: Vec::new(),
            cap_base: 0,
            offset,
            noise: Rng::new(7),
            noise_rms: dbfs_to_rms(STIMULUS_DBFS - snr_db),
            stop_at: None,
            restart_at: None,
        }
    }

    fn out_end(&self) -> u64 {
        self.out_base + self.out.len() as u64
    }

    fn cap_end(&self) -> u64 {
        self.cap_base + self.cap.len() as u64
    }

    fn render_out_to(&mut self, end: u64) {
        let mut block = [0.0f32; 1024];
        while self.out_end() < end {
            let at = self.out_end();
            if self.stop_at.is_some_and(|s| at >= s) && self.restart_at.is_none_or(|r| at < r) {
                self.level.fade_out();
            } else {
                self.level.set_muted(false);
            }
            self.generator.fill(&mut block);
            self.out.extend_from_slice(&block);
        }
    }

    fn out_at(&self, i: i64) -> f32 {
        if i < self.out_base as i64 {
            0.0
        } else {
            self.out[(i - self.out_base as i64) as usize]
        }
    }

    fn render_cap_to(&mut self, end: u64) {
        while self.cap_end() < end {
            let c = self.cap_end();
            let src = c as f64 - (self.offset)(c);
            let i = src.floor();
            let frac = (src - i) as f32;
            self.render_out_to((i as i64 + 2).max(0) as u64);
            let a = self.out_at(i as i64);
            let b = self.out_at(i as i64 + 1);
            let noise = self.noise.uniform_unit_rms() * self.noise_rms;
            self.cap.push(a + (b - a) * frac + noise as f32);
        }
    }

    fn window(&mut self, cfg: &TimingConfig, start: u64, range: LagRange) -> (Vec<f32>, Vec<f32>) {
        let w = cfg.window;
        self.render_cap_to(start + w as u64);
        let r0 = range.reference_start(start);
        let len = range.reference_len(w);
        self.render_out_to((r0 + len as i64).max(0) as u64);
        let reference = (0..len as i64).map(|j| self.out_at(r0 + j)).collect();
        let s = (start - self.cap_base) as usize;
        let capture = self.cap[s..s + w].to_vec();
        // Keep a few seconds of history so long runs stay small.
        let keep = 3 * FS as u64;
        if start > self.cap_base + 2 * keep {
            let drop = (start - keep - self.cap_base) as usize;
            self.cap.drain(..drop);
            self.cap_base += drop as u64;
        }
        let out_keep_from = (r0 - 2 * FS as i64).max(0) as u64;
        if out_keep_from > self.out_base + 2 * keep {
            let drop = (out_keep_from - self.out_base) as usize;
            self.out.drain(..drop);
            self.out_base += drop as u64;
        }
        (capture, reference)
    }
}

struct Record {
    start: u64,
    state: TimingState,
    events: Vec<TimingEvent>,
    measurement: WindowMeasurement,
}

/// Runs `windows` hops starting at capture index `first`.
fn run(sim: &mut Sim, mon: &mut LoopbackTiming, first: u64, windows: usize) -> Vec<Record> {
    let cfg = *mon.config();
    (0..windows)
        .map(|k| {
            let start = first + (k * cfg.hop) as u64;
            let range = mon.search_range();
            let (capture, reference) = sim.window(&cfg, start, range);
            let (measurement, ev) = mon
                .process_window(start, &capture, &reference, range)
                .expect("window");
            Record {
                start,
                state: mon.tracker().state(),
                events: ev.iter().copied().collect(),
                measurement,
            }
        })
        .collect()
}

fn events(rec: &[Record]) -> Vec<TimingEvent> {
    rec.iter().flat_map(|r| r.events.iter().copied()).collect()
}

fn jumps(rec: &[Record]) -> Vec<(i64, i64)> {
    events(rec)
        .iter()
        .filter_map(|e| match e {
            TimingEvent::Jump { from, to, .. } => Some((*from, *to)),
            _ => None,
        })
        .collect()
}

fn constant(d: f64) -> OffsetFn {
    Box::new(move |_| d)
}

fn step_offset(d0: f64, delta: f64, at: u64) -> OffsetFn {
    Box::new(move |c| if c < at { d0 } else { d0 + delta })
}

mod drift;
mod jumps;
mod lock;
mod stimulus;
mod sweeps;
