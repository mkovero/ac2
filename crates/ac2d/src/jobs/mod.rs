//! Measurement jobs: one thread each, fed by the capture fan-out, publishing frames into the
//! I/O thread's latest slots.
//!
//! A job's lifetime follows commands (`meas.start/stop`, session open/close), never
//! subscriptions. Subscriptions only decide which optional derivations are computed (the
//! live IR view). Every job publishes at most at the publish rate; between frames it only
//! accumulates.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::frame::{
    ClipFlags, Frame, FrameData, FrameStamp, LevelsFrame, LevelsMeta, ProtectionFlags,
};
use ac2_proto::grid::GridId;
use ac2_proto::topic::Topic;
use ac2_proto::units::{DaemonIncarnation, MeasId, Rev, SampleIndex, SessionEpoch, WallNs};
use ac2_zmq::Context;

use crate::fanout::{Batch, Block, JobFeed};
use crate::io::Interest;
use crate::outbox::Outbox;

pub(crate) mod finder;
pub(crate) mod meters;
pub(crate) mod rta;
pub(crate) mod spectrum;
pub(crate) mod spl;
pub(crate) mod sweep;
pub(crate) mod timing;
pub(crate) mod transfer;

/// Per-topic sequence numbers; they survive job restarts so `seq` keeps increasing per
/// topic for the whole incarnation.
#[derive(Debug, Default)]
pub(crate) struct Seqs(Mutex<HashMap<Topic, u64>>);

impl Seqs {
    pub(crate) fn next(&self, t: Topic) -> u64 {
        let mut m = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let s = m.entry(t).or_insert(0);
        *s += 1;
        *s
    }
}

/// What every job needs from the daemon.
#[derive(Clone, Debug)]
pub(crate) struct JobEnv {
    pub(crate) ctx: Context,
    pub(crate) endpoint: String,
    pub(crate) incarnation: DaemonIncarnation,
    pub(crate) epoch: SessionEpoch,
    pub(crate) seqs: Arc<Seqs>,
    pub(crate) interest: Arc<Interest>,
    pub(crate) fps: u32,
}

/// Header fields a job decides.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StampArgs {
    /// Newest sample index in the frame.
    pub(crate) audio_sample: u64,
    pub(crate) config_rev: Rev,
    pub(crate) applied_at: u64,
    pub(crate) wall_ns: u64,
    pub(crate) grid_id: Option<GridId>,
    pub(crate) protection: ProtectionFlags,
}

/// The newest `tf` / `spec` / `rta` frame a job published, for `trace.capture`: a capture
/// stores what clients were shown, never a separately computed result — for a transfer
/// function the same frame before display smoothing, so a stored trace can be re-smoothed;
/// for a spectrum the same result before smoothing with every bin, not its display columns.
pub(crate) type LatestFrame = Arc<Mutex<Option<Frame>>>;

/// A job's way out.
pub(crate) struct Emitter {
    outbox: Outbox,
    env: JobEnv,
    latest: LatestFrame,
}

impl Emitter {
    /// An emitter for a thread that is not a job (the preview).
    pub(crate) fn connect(env: JobEnv) -> std::io::Result<Self> {
        let outbox = Outbox::connect(&env.ctx, &env.endpoint, 64)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(Self {
            outbox,
            env,
            latest: Arc::new(Mutex::new(None)),
        })
    }

    pub(crate) fn wants(&self, t: Topic) -> bool {
        self.env.interest.wants(&t.to_bytes())
    }

    pub(crate) fn send(&self, s: StampArgs, data: FrameData) {
        self.publish(s, data, None);
    }

    /// Publishes `data`, keeping `capture` (the same result before display smoothing) as
    /// what `trace.capture` stores.
    pub(crate) fn send_with_capture(&self, s: StampArgs, data: FrameData, capture: FrameData) {
        self.publish(s, data, Some((capture, s.grid_id)));
    }

    /// Publishes `data`, keeping `capture` — the same result at full resolution, on
    /// `capture_grid` — as what `trace.capture` stores.
    pub(crate) fn send_with_capture_on(
        &self,
        s: StampArgs,
        data: FrameData,
        capture: FrameData,
        capture_grid: GridId,
    ) {
        self.publish(s, data, Some((capture, Some(capture_grid))));
    }

    fn publish(&self, s: StampArgs, data: FrameData, capture: Option<(FrameData, Option<GridId>)>) {
        let topic = data.topic();
        let mut frame = Frame {
            stamp: FrameStamp {
                seq: self.env.seqs.next(topic),
                audio_sample: SampleIndex(s.audio_sample),
                session_epoch: self.env.epoch,
                daemon_incarnation: self.env.incarnation,
                config_rev: s.config_rev,
                config_applied_at: SampleIndex(s.applied_at),
                capture_wall_ns: WallNs(s.wall_ns),
                grid_id: s.grid_id,
                protection: s.protection,
            },
            data,
        };
        let parts = match ac2_proto::encode_frame(&frame) {
            Ok(parts) => parts,
            Err(e) => {
                tracing::error!("{topic}: frame not encodable: {e}");
                return;
            }
        };
        // The capture slot is filled before the frame leaves: a client that has seen this
        // frame and asks for a capture must get this result (or a newer one), never the
        // one before it.
        if let Some((c, grid)) = capture {
            frame.data = c;
            frame.stamp.grid_id = grid;
        }
        if matches!(
            frame.data,
            FrameData::Tf(_) | FrameData::Spec(_) | FrameData::Rta(_)
        ) {
            *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(frame);
        }
        self.outbox.frame(&parts);
    }
}

/// Commands to a running job.
#[derive(Clone, Debug)]
pub(crate) enum JobCmd {
    /// Run the delay finder now; the result goes to control under `token`.
    Find {
        token: u64,
        band: crate::conv::FindBand,
        observation: Option<f64>,
    },
    /// Delay tracking on or off.
    Track { enabled: bool },
    /// New alignment delay (transfer); `rev` is the commit that set it. `resume`: the
    /// operator inserted or typed it, which resolves an ambiguous finding.
    SetDelay {
        samples: i64,
        seconds: f64,
        rev: Rev,
        resume: bool,
    },
    /// Freeze or unfreeze.
    Freeze(bool),
    /// Clear averages.
    Reset,
    /// New display smoothing; `rev` is the commit that set it. Averaging goes on.
    Smoothing { change: SmoothingChange, rev: Rev },
    /// New weightings and Leq windows of an SPL meter on the same input; `rev` is the commit
    /// that set them. The meter's interval, its log and its windows carry on.
    Spl {
        config: Box<ac2_proto::model::SplConfig>,
        rev: Rev,
    },
    /// The input's calibration or mic curve changed (on the measurement input of a
    /// transfer function only the curve matters).
    Cal(Box<crate::calstore::InputCal>),
}

/// A measurement's new display smoothing, by kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SmoothingChange {
    Transfer(Option<ac2_proto::model::Smoothing>),
    Spectrum(Option<ac2_proto::model::SmoothingFraction>),
}

/// What a job thread receives: captured audio and commands on one channel, so a job sleeps
/// in one blocking receive until either arrives.
pub(crate) enum JobMsg {
    /// Blocks of one hand-off, oldest first.
    Blocks(Batch),
    Cmd(JobCmd),
    Stop,
}

/// One analysis.
pub(crate) trait Analysis: Send {
    /// Accumulates one captured block.
    fn push(&mut self, b: &Block);
    /// Applies a command.
    fn command(&mut self, c: JobCmd);
    /// Publishes the current result.
    fn emit(&mut self, e: &Emitter);
}

/// A running job thread.
pub(crate) struct JobHandle {
    tx: Sender<JobMsg>,
    thread: Option<JoinHandle<()>>,
    latest: LatestFrame,
    /// Id of this job at the fan-out.
    pub(crate) fanout_id: u64,
}

impl JobHandle {
    pub(crate) fn send(&self, c: JobCmd) {
        let _ = self.tx.send(JobMsg::Cmd(c));
    }

    /// The newest curve frame this job published.
    pub(crate) fn latest(&self) -> Option<Frame> {
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn stop_inner(&mut self) {
        let _ = self.tx.send(JobMsg::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// Starts `analysis` on its own thread. Returns the handle and the feed the fan-out sends
/// captured audio through.
pub(crate) fn spawn(
    name: String,
    env: JobEnv,
    fanout_id: u64,
    mut analysis: Box<dyn Analysis>,
) -> std::io::Result<(JobHandle, JobFeed)> {
    let (tx, rx) = std::sync::mpsc::channel::<JobMsg>();
    let queued = Arc::new(AtomicU64::new(0));
    let q = Arc::clone(&queued);
    let latest: LatestFrame = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&latest);
    let thread = std::thread::Builder::new()
        .name(name.clone())
        .spawn(move || {
            let outbox = match Outbox::connect(&env.ctx, &env.endpoint, 64) {
                Ok(o) => o,
                Err(e) => {
                    tracing::error!("{name}: cannot reach the I/O thread: {e}");
                    return;
                }
            };
            let em = Emitter {
                outbox,
                env,
                latest: slot,
            };
            run(&mut *analysis, &rx, &q, &em);
        })?;
    Ok((
        JobHandle {
            tx: tx.clone(),
            thread: Some(thread),
            latest,
            fanout_id,
        },
        JobFeed { tx, queued },
    ))
}

/// Messages handled per wakeup before the job looks at its publish schedule again.
const DRAIN_MAX: usize = 64;

fn run(a: &mut dyn Analysis, rx: &Receiver<JobMsg>, queued: &AtomicU64, em: &Emitter) {
    let period = Duration::from_secs_f64(1.0 / f64::from(em.env.fps.max(1)));
    // Audio arrives once per fan-out hand-off, about one publish period apart give or take
    // scheduling jitter. A frame may go this much early, so jitter neither costs a separate
    // wakeup just to publish nor skips a hand-off's worth of results.
    let slack = period / 4;
    let mut last_emit: Option<Instant> = None;
    let mut dirty = false;
    loop {
        // Nothing to publish: sleep until audio or a command arrives. Something pending
        // but published too recently: sleep no longer than until it is due.
        let first = if dirty {
            let due = last_emit.map_or_else(Instant::now, |t| t + period - slack);
            match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        };
        let mut next = first;
        let mut handled = 0;
        while let Some(m) = next.take() {
            match m {
                JobMsg::Blocks(batch) => {
                    let mut frames = 0u64;
                    for b in batch.iter() {
                        a.push(b);
                        frames += u64::from(b.frames);
                    }
                    queued.fetch_sub(frames, Ordering::AcqRel);
                }
                JobMsg::Cmd(c) => a.command(c),
                JobMsg::Stop => return,
            }
            dirty = true;
            handled += 1;
            if handled < DRAIN_MAX {
                next = match rx.try_recv() {
                    Ok(m) => Some(m),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                };
            }
        }
        if dirty && last_emit.is_none_or(|t| t.elapsed() + slack >= period) {
            a.emit(em);
            last_emit = Some(Instant::now());
            dirty = false;
        }
    }
}

/// Clip threshold for meters, matching the protection default (−0.1 dBFS sample peak).
const CLIP_PEAK: f32 = 0.988_553_1;

/// RMS integration time of the input meters. A frame interval (~16 ms) is far too short for
/// noise stimuli: pink noise over 16 ms swings by several dB, so the RMS is an exponential
/// mean square with a 300 ms time constant (VU-like ballistics) while peak stays per interval.
const METER_RMS_TAU_S: f64 = 0.3;

/// One interval's meters.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Meters {
    pub(crate) meta: LevelsMeta,
    pub(crate) peak: Vec<f32>,
    pub(crate) rms: Vec<f32>,
    pub(crate) clip: Vec<ClipFlags>,
}

/// Input meters of a job's channels: per-interval peak and clip, 300 ms integrated RMS.
#[derive(Debug)]
pub(crate) struct LevelsMeter {
    /// Block channel index per meter.
    idx: Vec<usize>,
    /// Device input per meter.
    channels: Vec<u16>,
    peak: Vec<f32>,
    /// Exponentially integrated mean square per meter; `None` until the first block.
    ms: Vec<Option<f64>>,
    frames: u64,
    sample_rate: f64,
    clipped: Vec<bool>,
    hold_until: Vec<u64>,
    hold: u64,
    end: u64,
}

impl LevelsMeter {
    pub(crate) fn new(idx: Vec<usize>, channels: Vec<u16>, sample_rate: u32) -> Self {
        let n = idx.len();
        Self {
            idx,
            channels,
            peak: vec![0.0; n],
            ms: vec![None; n],
            frames: 0,
            sample_rate: f64::from(sample_rate),
            clipped: vec![false; n],
            hold_until: vec![0; n],
            // A clip indicator stays lit for a second so a single clipped block is seen.
            hold: u64::from(sample_rate),
            end: 0,
        }
    }

    pub(crate) fn push(&mut self, b: &Block) {
        let frames = f64::from(b.frames.max(1));
        let alpha = 1.0 - (-frames / (METER_RMS_TAU_S * self.sample_rate)).exp();
        for (m, &ch) in self.idx.iter().enumerate() {
            let st = b.stats.get(ch).copied().unwrap_or_default();
            let p = self.peak[m].max(st.peak);
            self.peak[m] = p;
            let block_ms = st.sum_sq / frames;
            self.ms[m] = Some(match self.ms[m] {
                Some(prev) => prev + alpha * (block_ms - prev),
                None => block_ms,
            });
            if p >= CLIP_PEAK {
                self.clipped[m] = true;
                self.hold_until[m] = b.end_sample() + self.hold;
            }
        }
        self.frames += u64::from(b.frames);
        self.end = b.end_sample();
    }

    /// The interval's meters, then a new interval; `None` if nothing was captured.
    pub(crate) fn take(&mut self, meas: MeasId) -> Option<LevelsFrame> {
        let m = self.take_meters()?;
        Some(LevelsFrame {
            meas,
            meta: m.meta,
            peak: m.peak,
            rms: m.rms,
            clip: m.clip,
        })
    }

    /// One past the newest sample metered.
    pub(crate) fn end(&self) -> u64 {
        self.end
    }

    /// [`Self::take`] without a measurement.
    pub(crate) fn take_meters(&mut self) -> Option<Meters> {
        if self.frames == 0 {
            return None;
        }
        let n = self.idx.len();
        let mut peak = Vec::with_capacity(n);
        let mut rms = Vec::with_capacity(n);
        let mut clip = Vec::with_capacity(n);
        for m in 0..n {
            peak.push((20.0 * f64::from(self.peak[m]).log10()) as f32);
            let r = self.ms[m].unwrap_or(0.0).sqrt();
            rms.push(ac2_core::spectrum::rms_dbfs(r) as f32);
            let mut c = ClipFlags::NONE;
            if self.clipped[m] {
                c = c.with(ClipFlags::CLIP);
            }
            if self.hold_until[m] > self.end {
                c = c.with(ClipFlags::HELD);
            }
            clip.push(c);
            self.peak[m] = 0.0;
            self.clipped[m] = false;
        }
        self.frames = 0;
        Some(Meters {
            meta: LevelsMeta {
                channels: self.channels.clone(),
            },
            peak,
            rms,
            clip,
        })
    }

    pub(crate) fn any_clip_held(&self) -> bool {
        self.hold_until.iter().any(|&h| h > self.end)
    }
}

/// Block channel index of device input `input` in a session capturing `input_map`.
pub(crate) fn block_index(input_map: &[u16], input: u16) -> Option<usize> {
    input_map.iter().position(|&c| c == input)
}

/// Converts a block's channel to f64 into `out`.
pub(crate) fn channel_f64(b: &Block, ch: usize, out: &mut Vec<f64>) {
    out.clear();
    let n = usize::from(b.channels).max(1);
    out.extend(b.data.iter().skip(ch).step_by(n).map(|v| f64::from(*v)));
}

#[cfg(test)]
mod levels_tests {
    use super::*;
    use crate::fanout::Block;
    use ac2_audio::BlockFlags;
    use ac2_proto::units::MeasId;

    fn sine_block(start: u64, frames: u32, amp: f32, fs: f64) -> Block {
        let data: Vec<f32> = (0..frames)
            .map(|i| {
                let t = (start + u64::from(i)) as f64 / fs;
                amp * (2.0 * std::f64::consts::PI * 997.0 * t).sin() as f32
            })
            .collect();
        Block::new(start, frames, 1, BlockFlags::NONE, 0, data)
    }

    /// A steady −20 dBFS sine reads −20 dBFS RMS, and short frame intervals don't change the
    /// reading: the RMS integrates over 300 ms, not over one interval.
    #[test]
    fn rms_integrates_across_intervals() {
        let fs = 48_000.0;
        let amp = 0.1_f32; // −20 dBFS (0 dBFS = full-scale sine)
        let mut m = LevelsMeter::new(vec![0], vec![0], 48_000);
        let mut start = 0;
        let mut last = f32::NAN;
        for _ in 0..200 {
            m.push(&sine_block(start, 256, amp, fs));
            start += 256;
            if let Some(f) = m.take(MeasId(1)) {
                last = f.rms[0];
            }
        }
        assert!((last + 20.0).abs() < 0.05, "rms {last}");
    }
}
