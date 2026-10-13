use std::sync::Arc;

use crate::backend::DuplexRequest;
use crate::block::{BlockProducer, BlockStamp};
use crate::clock::FrameCounterClock;
use crate::events::{BackendEvents, EventLatch};
use crate::load::LoadTimer;
use crate::output::{OutputRenderer, OutputStamp};
use crate::rng::Rng;

use super::analog::{Reverb, SINC_HALF, SINC_PHASES, shape, sinc_table};
use super::config::{FakeConfig, FakeFault, InputContent, OutputFault, output_faults, signature};

/// Counters of the simulated device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FakeStats {
    /// Callbacks run.
    pub blocks: u64,
    /// Analog frames simulated (including frames lost to xruns).
    pub device_frames: u64,
    /// Frames the DAC received.
    pub dac_frames: u64,
    /// Output frames dropped by faults.
    pub output_frames_dropped: u64,
    /// Output frames played twice by faults.
    pub output_frames_repeated: u64,
    /// Reads of DAC samples that were not (yet / any more) available and read as zero. Non
    /// zero means a path delay is too short for the scripted drops or drift.
    pub dac_underreads: u64,
}

/// Steps a fake device; the stand-in for the audio callback.
#[derive(Debug)]
pub struct FakeDriver {
    pub(super) sim: Box<Sim>,
}

impl FakeDriver {
    /// Runs one callback.
    pub fn step(&mut self) {
        self.sim.step();
    }

    /// Runs `blocks` callbacks.
    pub fn run_blocks(&mut self, blocks: u64) {
        for _ in 0..blocks {
            self.sim.step();
        }
    }

    /// Runs enough callbacks to cover `seconds` of device time.
    pub fn run_seconds(&mut self, seconds: f64) {
        let frames = (seconds * f64::from(self.sim.cfg.sample_rate))
            .ceil()
            .max(0.0) as u64;
        self.run_blocks(frames.div_ceil(u64::from(self.sim.cfg.block_frames)));
    }

    /// Captured blocks the stream's reader has not taken yet. A test that steps the
    /// device faster than real time waits for this to reach zero between steps: a real
    /// device cannot outrun its reader, it would overflow, and a loaded machine must not
    /// turn into lost audio a test never asked for.
    pub fn capture_queued(&self) -> usize {
        self.sim.producer.blocks_queued()
    }

    /// Callbacks run so far.
    pub fn blocks(&self) -> u64 {
        self.sim.stats.blocks
    }

    /// Device counters.
    pub fn stats(&self) -> FakeStats {
        self.sim.stats
    }
}

struct PathRt {
    output: usize,
    input: usize,
    delay: f64,
    fir: Box<[f32]>,
    noise_rms: f64,
    /// `[c2, c3, …]`; empty = linear.
    poly: Box<[f64]>,
    rng: Rng,
    reverb: Option<Reverb>,
}

pub(super) struct Sim {
    cfg: FakeConfig,
    producer: BlockProducer,
    renderer: OutputRenderer,
    clock: FrameCounterClock,
    latch: EventLatch,
    load: LoadTimer,
    pub(super) events: Arc<BackendEvents>,
    input_map: Box<[usize]>,
    stream_outputs: usize,
    device_inputs: usize,
    device_outputs: usize,
    raw_counter: u32,
    block_faults: Box<[FakeFault]>,
    output_faults: Box<[OutputFault]>,
    output_fault_cursor: usize,
    /// DAC history, `device_outputs` interleaved channels.
    dac: Box<[f32]>,
    dac_mask: u64,
    out_buf: Box<[f32]>,
    in_buf: Box<[f32]>,
    paths: Box<[PathRt]>,
    input_rngs: Box<[Rng]>,
    drift_ratio: Option<f64>,
    sinc: Box<[f32]>,
    stats: FakeStats,
}

impl std::fmt::Debug for Sim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sim")
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl Sim {
    pub(super) fn new(
        cfg: FakeConfig,
        req: &DuplexRequest,
        producer: BlockProducer,
        renderer: OutputRenderer,
        events: Arc<BackendEvents>,
    ) -> Self {
        let block = cfg.block_frames as usize;
        let device_inputs = usize::from(cfg.inputs);
        let device_outputs = usize::from(cfg.outputs).max(1);
        let output_faults = output_faults(&cfg.faults);
        let repeats: u64 = output_faults
            .iter()
            .filter(|f| f.repeat)
            .map(|f| u64::from(f.frames))
            .sum();
        let max_delay = cfg.paths.iter().map(|p| p.delay_frames).max().unwrap_or(0);
        let max_fir = cfg.paths.iter().map(|p| p.fir.len()).max().unwrap_or(1);
        let drift_margin =
            (cfg.drift_ppm.abs() * 1e-6 * cfg.drift_horizon_seconds * f64::from(cfg.sample_rate))
                .ceil() as u64;
        let dac_frames = (u64::from(max_delay)
            + max_fir as u64
            + 2 * SINC_HALF as u64
            + 4 * block as u64
            + repeats
            + drift_margin)
            .next_power_of_two();
        let mut seeds = Rng::new(cfg.seed);
        let paths = cfg
            .paths
            .iter()
            .map(|p| PathRt {
                output: usize::from(p.output),
                input: usize::from(p.input),
                delay: f64::from(p.delay_frames),
                fir: p.fir.clone().into_boxed_slice(),
                noise_rms: f64::from(p.noise_rms),
                poly: p.nonlinearity.clone().into_boxed_slice(),
                rng: Rng::new(seeds.next_u64()),
                reverb: p.reverb.map(|r| Reverb::new(r, f64::from(cfg.sample_rate))),
            })
            .collect();
        let input_rngs = (0..device_inputs)
            .map(|_| Rng::new(seeds.next_u64()))
            .collect();
        let block_faults = cfg
            .faults
            .iter()
            .copied()
            .filter(|f| {
                !matches!(
                    f,
                    FakeFault::DropOutputFrames { .. } | FakeFault::RepeatOutputFrames { .. }
                )
            })
            .collect();
        let drift_ratio = (cfg.drift_ppm != 0.0).then_some(1.0 + cfg.drift_ppm * 1e-6);
        let latch = EventLatch::new(Arc::clone(&events));
        Self {
            producer,
            renderer,
            clock: FrameCounterClock::new(),
            latch,
            load: LoadTimer::new(Arc::clone(&events), cfg.sample_rate),
            events,
            input_map: req.input_map.iter().map(|&c| usize::from(c)).collect(),
            stream_outputs: usize::from(req.output_channels),
            device_inputs,
            device_outputs,
            raw_counter: cfg.counter_origin,
            block_faults,
            output_faults: output_faults.into_boxed_slice(),
            output_fault_cursor: 0,
            dac: vec![0.0; dac_frames as usize * device_outputs].into_boxed_slice(),
            dac_mask: dac_frames - 1,
            out_buf: vec![0.0; block * usize::from(req.output_channels).max(1)].into_boxed_slice(),
            in_buf: vec![0.0; block * device_inputs].into_boxed_slice(),
            paths,
            input_rngs,
            drift_ratio,
            sinc: if cfg.drift_ppm != 0.0 {
                sinc_table()
            } else {
                Box::new([])
            },
            stats: FakeStats::default(),
            cfg,
        }
    }

    /// One callback. Allocation-free.
    fn step(&mut self) {
        let started = self.load.start();
        let block_no = self.stats.blocks;
        let frames = self.cfg.block_frames;
        for i in 0..self.block_faults.len() {
            match self.block_faults[i] {
                FakeFault::XrunFlag { at_block } if at_block == block_no => self.events.xrun(),
                FakeFault::ConfigChange { at_block } if at_block == block_no => {
                    self.events.config_change();
                }
                FakeFault::Xrun {
                    at_block,
                    lost_frames,
                } if at_block == block_no => {
                    self.events.xrun();
                    self.lose_frames(lost_frames);
                }
                _ => {}
            }
        }

        let (start, mut flags) = self.clock.advance(self.raw_counter, frames);
        debug_assert_eq!(
            start, self.stats.device_frames,
            "output index is device time"
        );
        flags |= self.latch.take();
        let capture_ns = self.cfg.ns(start);
        let callback_ns = self.cfg.ns(start + u64::from(frames));
        let playback_ns = callback_ns + self.cfg.ns(u64::from(frames));

        let out_ch = self.stream_outputs;
        let out_buf = &mut self.out_buf;
        self.renderer.render(
            OutputStamp {
                start_sample: start,
                frames,
                flags,
                callback_ns,
                playback_ns: Some(playback_ns),
            },
            out_ch,
            |f, c, v| out_buf[f * out_ch + c] = v,
        );
        for f in 0..frames as usize {
            self.dac_push(start + f as u64, Some(f));
        }
        self.capture(start, frames as usize, true);

        let (map, inputs, buf) = (&self.input_map, self.device_inputs, &self.in_buf);
        self.producer.push_with(
            BlockStamp {
                start_sample: start,
                frames,
                flags,
                callback_ns,
                capture_ns: Some(capture_ns),
            },
            |f, c| buf[f * inputs + map[c]],
        );
        self.raw_counter = self.raw_counter.wrapping_add(frames);
        self.stats.blocks += 1;
        self.load.finish(started, frames);
    }

    /// Frames that pass while no callback runs: the DAC plays silence, the ADC's samples
    /// are lost, the counter moves on.
    fn lose_frames(&mut self, lost: u32) {
        let start = self.stats.device_frames;
        for i in 0..u64::from(lost) {
            self.dac_push(start + i, None);
            self.capture(start + i, 1, false);
        }
        self.raw_counter = self.raw_counter.wrapping_add(lost);
    }

    /// Hands output frame `index` to the DAC: frame `frame` of `out_buf`, or silence.
    fn dac_push(&mut self, index: u64, frame: Option<usize>) {
        while let Some(f) = self.output_faults.get(self.output_fault_cursor) {
            if f.at + u64::from(f.frames) <= index {
                self.output_fault_cursor += 1;
            } else {
                break;
            }
        }
        let fault = self.output_faults.get(self.output_fault_cursor).copied();
        let inside = fault.is_some_and(|f| index >= f.at);
        if inside && fault.is_some_and(|f| !f.repeat) {
            self.stats.output_frames_dropped += 1;
            return;
        }
        let w = self.stats.dac_frames;
        let base = ((w & self.dac_mask) as usize) * self.device_outputs;
        for c in 0..self.device_outputs {
            self.dac[base + c] = match frame {
                Some(f) if c < self.stream_outputs => self.out_buf[f * self.stream_outputs + c],
                _ => 0.0,
            };
        }
        self.stats.dac_frames += 1;
        if let Some(f) = fault
            && f.repeat
            && index == f.at + u64::from(f.frames) - 1
        {
            // Replay the last `frames` DAC frames once more.
            let k = u64::from(f.frames);
            for _ in 0..k {
                let w = self.stats.dac_frames;
                let dst = ((w & self.dac_mask) as usize) * self.device_outputs;
                let src = (((w - k) & self.dac_mask) as usize) * self.device_outputs;
                for c in 0..self.device_outputs {
                    self.dac[dst + c] = self.dac[src + c];
                }
                self.stats.dac_frames += 1;
            }
            self.stats.output_frames_repeated += k;
        }
    }

    /// One DAC sample at integer position `pos` (zero before the stream started).
    #[inline]
    fn dac_at(&mut self, channel: usize, pos: i64) -> f32 {
        if pos < 0 {
            return 0.0;
        }
        let pos = pos as u64;
        let written = self.stats.dac_frames;
        if pos >= written || written - pos > self.dac_mask + 1 {
            self.stats.dac_underreads += 1;
            return 0.0;
        }
        self.dac[((pos & self.dac_mask) as usize) * self.device_outputs + channel]
    }

    /// DAC signal at fractional position `pos` (band-limited interpolation).
    #[inline]
    fn dac_interp(&mut self, channel: usize, pos: f64) -> f32 {
        let base = pos.floor();
        let mut phase = ((pos - base) * SINC_PHASES as f64).round() as usize;
        let mut i0 = base as i64;
        if phase == SINC_PHASES {
            phase = 0;
            i0 += 1;
        }
        let taps = 2 * SINC_HALF;
        let mut acc = 0.0f32;
        for k in 0..taps {
            let coef = self.sinc[phase * taps + k];
            acc += coef * self.dac_at(channel, i0 - (SINC_HALF as i64 - 1) + k as i64);
        }
        acc
    }

    /// Simulates `frames` ADC frames starting at device index `start`; writes them into
    /// `in_buf` when `keep`.
    fn capture(&mut self, start: u64, frames: usize, keep: bool) {
        let inputs = self.device_inputs;
        for f in 0..frames {
            let n = start + f as u64;
            for i in 0..inputs {
                let base = match self.cfg.input_content {
                    InputContent::Silence => 0.0,
                    InputContent::Signature => signature(i as u16, n),
                };
                let noise = if self.cfg.input_noise_rms > 0.0 {
                    (f64::from(self.cfg.input_noise_rms) * self.input_rngs[i].gaussian()) as f32
                } else {
                    0.0
                };
                if keep {
                    self.in_buf[f * inputs + i] = base + noise;
                }
            }
            for p in 0..self.paths.len() {
                let v = self.path_sample(p, n);
                if keep {
                    let input = self.paths[p].input;
                    self.in_buf[f * inputs + input] += v;
                }
            }
        }
        self.stats.device_frames = self.stats.device_frames.max(start + frames as u64);
    }

    fn path_sample(&mut self, p: usize, n: u64) -> f32 {
        let (output, delay, fir_len, noise_rms) = {
            let path = &self.paths[p];
            (path.output, path.delay, path.fir.len(), path.noise_rms)
        };
        let mut acc = 0.0f32;
        match self.drift_ratio {
            None => {
                let pos = n as i64 - delay as i64;
                for t in 0..fir_len {
                    let coef = self.paths[p].fir[t];
                    let x = self.dac_at(output, pos - t as i64);
                    acc += coef * shape(&self.paths[p].poly, x);
                }
            }
            Some(ratio) => {
                let pos = n as f64 * ratio - delay;
                for t in 0..fir_len {
                    let coef = self.paths[p].fir[t];
                    let x = self.dac_interp(output, pos - t as f64);
                    acc += coef * shape(&self.paths[p].poly, x);
                }
            }
        }
        if let Some(r) = self.paths[p].reverb.as_mut() {
            acc += r.tick(acc);
        }
        if noise_rms > 0.0 {
            acc += (noise_rms * self.paths[p].rng.gaussian()) as f32;
        }
        acc
    }
}
