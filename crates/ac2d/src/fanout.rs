//! Capture fan-out: the one consumer of the stream's capture ring.
//!
//! The audio callback only writes the wait-free ring (no allocation, no syscall, so it
//! cannot wake anyone). This thread therefore wakes on a fixed hand-off period, pops every
//! whole block that arrived since, and hands the lot to every attached job as one shared
//! [`Batch`]: one wakeup per hand-off for the fan-out and for each job, however short the
//! device period. A job that falls behind by more than [`JOB_QUEUE_S`] of audio loses
//! batches; it sees the gap as a jump in `start_sample` and restarts its averages, it never
//! splices.
//!
//! Per-channel block statistics (sum of squares, peak) are computed once, when a block is
//! popped, and travel with it: the running levels for `cal.spl`, the session meters
//! published from here, and every job's own meters all read them instead of rescanning
//! the samples. The fan-out also watches for configuration changes.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ac2_audio::timer::{Sleeper, Waker};
use ac2_audio::{BlockFlags, DuplexStream};
use ac2_proto::units::{Rev, SessionEpoch};

use crate::burst::BurstDetector;
use crate::control::ControlMsg;
use crate::jobs::meters::{SessionLevels, meter_period};
use crate::jobs::{Emitter, JobEnv, JobMsg};
use crate::util::wall_ns;

/// Sum of squares and peak magnitude of one channel of one block.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ChanStats {
    pub(crate) sum_sq: f64,
    pub(crate) peak: f32,
}

/// One captured block.
#[derive(Debug)]
pub(crate) struct Block {
    /// Session sample index of the first frame.
    pub(crate) start_sample: u64,
    pub(crate) frames: u32,
    pub(crate) channels: u16,
    pub(crate) flags: BlockFlags,
    /// Daemon wall clock when the fan-out received the block (Unix ns).
    pub(crate) wall_ns: u64,
    /// Interleaved samples.
    pub(crate) data: Vec<f32>,
    /// Per block channel.
    pub(crate) stats: Vec<ChanStats>,
}

impl Block {
    /// A block of interleaved `data`, its statistics computed.
    #[cfg(test)]
    pub(crate) fn new(
        start_sample: u64,
        frames: u32,
        channels: u16,
        flags: BlockFlags,
        wall_ns: u64,
        data: Vec<f32>,
    ) -> Self {
        let mut b = Self {
            start_sample,
            frames,
            channels,
            flags,
            wall_ns,
            data,
            stats: Vec::new(),
        };
        b.compute_stats();
        b
    }

    pub(crate) fn end_sample(&self) -> u64 {
        self.start_sample + u64::from(self.frames)
    }

    /// Copies block channel `ch` into `out`.
    pub(crate) fn channel_into(&self, ch: usize, out: &mut Vec<f32>) {
        out.clear();
        let n = usize::from(self.channels);
        out.extend(self.data.iter().skip(ch).step_by(n.max(1)).copied());
    }

    /// One pass over the interleaved samples, all channels at once.
    fn compute_stats(&mut self) {
        let n = usize::from(self.channels).max(1);
        self.stats.clear();
        self.stats.resize(n, ChanStats::default());
        for frame in self.data.chunks_exact(n) {
            for (st, &v) in self.stats.iter_mut().zip(frame) {
                st.sum_sq += f64::from(v) * f64::from(v);
                st.peak = st.peak.max(v.abs());
            }
        }
    }
}

/// Blocks popped in one hand-off, shared by every job.
pub(crate) type Batch = Arc<Vec<Block>>;

/// Per-input running mean square, f64 bits; read by `cal.spl` and `cal.spl_electrical`.
/// Two time constants: the 1 s one is the reading, the 0.2 s one tells whether the level
/// has been steady long enough for the slow one to have settled. Beside them, where the
/// input last reached full scale: a clipped tone reads low, so a calibration taken from it
/// would be wrong by an unknown amount.
#[derive(Debug)]
pub(crate) struct InputMeters {
    ms: Box<[AtomicU64]>,
    fast: Box<[AtomicU64]>,
    /// One past the last sample of the newest block that reached [`CLIP_FULL_SCALE`]; 0 =
    /// never.
    clip_end: Box<[AtomicU64]>,
}

/// Sample magnitude counted as clipping: within 0.01 dB of full scale.
const CLIP_FULL_SCALE: f32 = 0.9989;

/// Time constants of [`InputMeters`], s.
const METER_SLOW_S: f64 = 1.0;
const METER_FAST_S: f64 = 0.2;

impl InputMeters {
    fn new(channels: usize) -> Self {
        Self {
            ms: (0..channels).map(|_| AtomicU64::new(0)).collect(),
            fast: (0..channels).map(|_| AtomicU64::new(0)).collect(),
            clip_end: (0..channels).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    /// One past the last sample of the newest block in which block channel `ch` reached
    /// full scale, `None` if it never did.
    pub(crate) fn clipped_until(&self, ch: usize) -> Option<u64> {
        let v = self.clip_end.get(ch)?.load(Ordering::Acquire);
        (v > 0).then_some(v)
    }

    /// Mean square of block channel `ch` (τ = 1 s) and its fast companion (τ = 0.2 s).
    pub(crate) fn mean_square(&self, ch: usize) -> Option<(f64, f64)> {
        let load = |a: &AtomicU64| f64::from_bits(a.load(Ordering::Acquire));
        Some((load(self.ms.get(ch)?), load(self.fast.get(ch)?)))
    }
}

/// The next whole captured block, stamped with the daemon's wall clock. `reuse` is a block
/// whose buffers may be refilled instead of allocating new ones.
pub(crate) fn pop_block_into(stream: &mut DuplexStream, reuse: Option<Block>) -> Option<Block> {
    stream.capture().pop_with(|h, a, b| {
        let mut block = reuse.unwrap_or_else(|| Block {
            start_sample: 0,
            frames: 0,
            channels: 0,
            flags: BlockFlags::NONE,
            wall_ns: 0,
            data: Vec::with_capacity(a.len() + b.len()),
            stats: Vec::new(),
        });
        block.start_sample = h.start_sample;
        block.frames = h.frames;
        block.channels = h.channels;
        block.flags = h.flags;
        block.wall_ns = wall_ns();
        block.data.clear();
        block.data.extend_from_slice(a);
        block.data.extend_from_slice(b);
        block.compute_stats();
        block
    })
}

/// [`pop_block_into`] with fresh buffers.
pub(crate) fn pop_block(stream: &mut DuplexStream) -> Option<Block> {
    pop_block_into(stream, None)
}

/// What a job and the fan-out share about the job's queue.
#[derive(Debug, Default)]
pub(crate) struct Queue {
    /// Frames sent and not yet consumed by the job.
    pub(crate) frames: AtomicU64,
    /// Queued frames at which a job sleeping through hand-offs is woken
    /// ([`crate::jobs::Analysis::frames_needed`]); 0 while it wakes for every hand-off.
    pub(crate) wake_at: AtomicU64,
    /// Frames this consumer may have queued; 0 = [`JOB_QUEUE_S`] of audio.
    pub(crate) limit: AtomicU64,
}

/// What the fan-out feeds one job through.
pub(crate) struct JobFeed {
    pub(crate) tx: Sender<JobMsg>,
    pub(crate) queue: Arc<Queue>,
    /// The job's thread, unparked once `queue.wake_at` frames are queued.
    pub(crate) thread: std::thread::Thread,
}

pub(crate) enum FanoutMsg {
    Attach(u64, JobFeed),
    /// Run this once every message sent before it is handled.
    Then(Box<dyn FnOnce() + Send>),
    Detach(u64),
    /// Publish the session input meters of a session capturing `input_map`, stamped with
    /// `config_rev`.
    Levels {
        env: JobEnv,
        input_map: Vec<u16>,
        config_rev: Rev,
    },
    Stop,
}

/// Running fan-out thread.
pub(crate) struct Fanout {
    tx: Sender<FanoutMsg>,
    /// Ends the thread's sleep between hand-offs, so a message is handled at once.
    waker: Waker,
    thread: Option<std::thread::JoinHandle<()>>,
    pub(crate) meters: Arc<InputMeters>,
    /// One past the newest captured sample.
    pub(crate) latest: Arc<AtomicU64>,
}

/// Time to wait for the final fade-out before tearing the stream down regardless.
const STOP_TIMEOUT: Duration = Duration::from_millis(300);

/// Audio a job may have queued. More means the job cannot keep up; it loses blocks and
/// restarts rather than lagging further. Counted in time, not blocks, so a short device
/// period does not shrink it.
pub(crate) const JOB_QUEUE_S: f64 = 2.0;

/// Most blocks popped in one hand-off: the capture ring holds about 2 s, so this only
/// bounds one pass after a long stall rather than limiting normal operation.
const MAX_BATCH: usize = 4096;

/// How often captured audio is handed on. Jobs publish at most `fps` times a second and
/// the session meters [`crate::jobs::meters::METER_FPS`] times; handing off at twice the
/// meter rate (or the publish rate, if faster) lets both publish on schedule while every
/// thread downstream wakes once per hand-off rather than once per device period. Never
/// shorter than one device period: waking between blocks finds nothing.
pub(crate) fn handoff_period(fps: u32, block_period: Option<Duration>) -> Duration {
    let publish = Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));
    let p = publish.min(meter_period() / 2);
    block_period.map_or(p, |b| p.max(b))
}

/// Discontinuity warnings at most this often; the rest are counted into a summary.
const DISCONTINUITY_WARN_EVERY: Duration = Duration::from_secs(10);

/// Rate-limited capture discontinuity warnings: a misbehaving device can break continuity
/// on every block, and a warning each time would flood the log.
#[derive(Default)]
struct Discontinuities {
    warned: Option<Instant>,
    /// Since the last warning: how many, and the newest.
    suppressed: u64,
    last: Option<(u64, BlockFlags)>,
}

impl Discontinuities {
    fn observe(&mut self, now: Instant, at: u64, flags: BlockFlags) {
        if self
            .warned
            .is_some_and(|t| now.duration_since(t) < DISCONTINUITY_WARN_EVERY)
        {
            self.suppressed += 1;
            self.last = Some((at, flags));
            return;
        }
        self.flush(now);
        tracing::warn!("capture discontinuity at sample {at} ({flags:?})");
        self.warned = Some(now);
    }

    /// Summarises suppressed warnings once the interval has passed.
    fn flush(&mut self, now: Instant) {
        if self.suppressed == 0
            || self
                .warned
                .is_some_and(|t| now.duration_since(t) < DISCONTINUITY_WARN_EVERY)
        {
            return;
        }
        if let Some((at, flags)) = self.last.take() {
            tracing::warn!(
                "{} more capture discontinuities in the last {} s, the newest at sample {at} \
                 ({flags:?})",
                self.suppressed,
                DISCONTINUITY_WARN_EVERY.as_secs()
            );
        }
        self.suppressed = 0;
        self.warned = Some(now);
    }
}

impl Fanout {
    /// A `lossless` stream (a replay, which can wait) is popped only as fast as every
    /// consumer takes its audio, so nobody ever loses a block; a device stream is popped
    /// whole at every hand-off, and a consumer that falls behind loses batches.
    pub(crate) fn spawn(
        stream: DuplexStream,
        epoch: SessionEpoch,
        to_control: Sender<ControlMsg>,
        fps: u32,
        lossless: bool,
    ) -> std::io::Result<Self> {
        let n = stream.negotiated();
        let channels = usize::from(n.input_channels);
        let rate = f64::from(n.sample_rate);
        let block_period = n
            .buffer_frames
            .filter(|_| rate > 0.0)
            .map(|f| Duration::from_secs_f64(f64::from(f) / rate));
        let bursts = BurstDetector::new(n.sample_rate, crate::burst::label(n));
        let (tx, rx) = std::sync::mpsc::channel();
        let (sleeper, waker) = ac2_audio::timer::sleeper()?;
        let meters = Arc::new(InputMeters::new(channels));
        let latest = Arc::new(AtomicU64::new(0));
        let m = Arc::clone(&meters);
        let l = Arc::clone(&latest);
        let cfg = Config {
            epoch,
            rate,
            sample_rate: n.sample_rate,
            handoff: handoff_period(fps, block_period),
            lossless: lossless.then(|| u64::from(n.buffer_frames.unwrap_or(4096).max(1))),
        };
        let thread = std::thread::Builder::new()
            .name("ac2d-fanout".into())
            .spawn(move || {
                let wait = Wait { rx, sleeper };
                run(stream, &wait, &cfg, &to_control, &m, &l, bursts);
            })?;
        Ok(Self {
            tx,
            waker,
            thread: Some(thread),
            meters,
            latest,
        })
    }

    fn send(&self, m: FanoutMsg) {
        let _ = self.tx.send(m);
        self.waker.wake();
    }

    pub(crate) fn attach(&self, id: u64, feed: JobFeed) {
        self.send(FanoutMsg::Attach(id, feed));
    }

    pub(crate) fn detach(&self, id: u64) {
        self.send(FanoutMsg::Detach(id));
    }

    /// Runs `f` on the fan-out thread after the messages sent before it (consumers
    /// attached by then receive every block popped after `f` ran).
    pub(crate) fn then(&self, f: Box<dyn FnOnce() + Send>) {
        self.send(FanoutMsg::Then(f));
    }

    /// Starts publishing the session input meters.
    pub(crate) fn start_levels(&self, env: JobEnv, input_map: Vec<u16>, config_rev: Rev) {
        self.send(FanoutMsg::Levels {
            env,
            input_map,
            config_rev,
        });
    }

    /// Fades out, stops the stream and joins the thread.
    pub(crate) fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.send(FanoutMsg::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Fanout {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// What the fan-out thread sleeps on between hand-offs: messages, and a timer that wakes it
/// on time (see [`ac2_audio::timer`]).
struct Wait {
    rx: Receiver<FanoutMsg>,
    sleeper: Sleeper,
}

impl Wait {
    /// The next message, or `None` once `deadline` has passed with none pending.
    /// Disconnection reads as [`FanoutMsg::Stop`].
    fn until(&self, deadline: Instant) -> Option<FanoutMsg> {
        loop {
            match self.rx.try_recv() {
                Ok(m) => return Some(m),
                Err(TryRecvError::Disconnected) => return Some(FanoutMsg::Stop),
                Err(TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                return None;
            }
            self.sleeper.sleep_until(deadline);
        }
    }
}

struct Config {
    epoch: SessionEpoch,
    rate: f64,
    sample_rate: u32,
    handoff: Duration,
    /// Lossless: the largest block the stream delivers, frames.
    lossless: Option<u64>,
}

/// Batches handed out and the spare blocks they return, so steady operation allocates
/// nothing.
#[derive(Default)]
struct Pool {
    /// Oldest first; jobs consume in order, so the front is released first.
    in_flight: VecDeque<Batch>,
    spare_blocks: Vec<Block>,
    spare_batches: Vec<Vec<Block>>,
}

/// Batches kept for reuse at most; more in flight than this are left to be freed.
const POOL_BATCHES: usize = 256;

impl Pool {
    /// Takes back every batch no job holds any more.
    fn reclaim(&mut self) {
        while let Some(front) = self.in_flight.front_mut() {
            let Some(v) = Arc::get_mut(front) else {
                break;
            };
            self.spare_blocks.append(v);
            if let Some(b) = self.in_flight.pop_front().and_then(Arc::into_inner) {
                self.spare_batches.push(b);
            }
        }
    }

    fn batch(&mut self) -> Vec<Block> {
        self.spare_batches.pop().unwrap_or_default()
    }

    /// Keeps `batch` for reuse once every job is done with it.
    fn hand_out(&mut self, batch: Batch) {
        if self.in_flight.len() < POOL_BATCHES {
            self.in_flight.push_back(batch);
        }
    }

    /// A batch no job saw: its blocks are spare at once.
    fn give_back(&mut self, mut batch: Vec<Block>) {
        self.spare_blocks.append(&mut batch);
        self.spare_batches.push(batch);
    }
}

fn run(
    mut stream: DuplexStream,
    wait: &Wait,
    cfg: &Config,
    to_control: &Sender<ControlMsg>,
    meters: &InputMeters,
    latest: &AtomicU64,
    mut bursts: BurstDetector,
) {
    let mut jobs: BTreeMap<u64, JobFeed> = BTreeMap::new();
    let mut ms: Vec<(f64, f64)> = vec![(0.0, 0.0); meters.ms.len()];
    let mut ended_reported = false;
    let mut levels: Option<(SessionLevels, Emitter)> = None;
    let mut gaps = Discontinuities::default();
    let mut pool = Pool::default();
    let queue_limit = (JOB_QUEUE_S * cfg.rate) as u64;
    // Meter frames are due every other hand-off; a frame may go half a hand-off ahead of
    // its slot, so a hand-off a little early takes it rather than the one after.
    let meter_early = cfg.handoff / 2;
    let mut next = Instant::now();
    loop {
        // Control messages wake the thread at once; otherwise it sleeps until the next
        // hand-off.
        while let Some(msg) = wait.until(next) {
            match msg {
                FanoutMsg::Attach(id, feed) => {
                    jobs.insert(id, feed);
                }
                FanoutMsg::Detach(id) => {
                    jobs.remove(&id);
                }
                FanoutMsg::Then(f) => f(),
                FanoutMsg::Levels {
                    env,
                    input_map,
                    config_rev,
                } => match Emitter::connect(env) {
                    Ok(em) => {
                        let l = SessionLevels::new(
                            &input_map,
                            cfg.sample_rate,
                            config_rev,
                            meter_early,
                        );
                        levels = Some((l, em));
                    }
                    Err(e) => tracing::error!("cannot start the session meters: {e}"),
                },
                FanoutMsg::Stop => {
                    let outcome = stream.stop(STOP_TIMEOUT);
                    tracing::info!("audio stream stopped: {outcome:?}");
                    return;
                }
            }
        }
        let now = Instant::now();
        next += cfg.handoff;
        if next <= now {
            // Overslept (a loaded machine): start a new schedule rather than catching up
            // with a run of back-to-back hand-offs.
            next = now + cfg.handoff;
        }

        pool.reclaim();
        let mut batch = pool.batch();
        let mut frames = 0u64;
        // Lossless: pop no more than the fullest consumer has room for, counting the next
        // block at the largest size the stream delivers.
        let room = cfg.lossless.and_then(|_| {
            jobs.values()
                .map(|f| {
                    consumer_limit(&f.queue, queue_limit)
                        .saturating_sub(f.queue.frames.load(Ordering::Acquire))
                })
                .min()
        });
        while batch.len() < MAX_BATCH {
            if let (Some(room), Some(block)) = (room, cfg.lossless)
                && frames + block > room
            {
                break;
            }
            let Some(block) = pop_block_into(&mut stream, pool.spare_blocks.pop()) else {
                break;
            };
            bursts.observe(now, block.frames);
            frames += u64::from(block.frames);
            if block.flags.contains(BlockFlags::CONFIG_CHANGE) {
                tracing::warn!(
                    "audio configuration changed at sample {}",
                    block.start_sample
                );
                let _ = to_control.send(ControlMsg::DeviceChanged { epoch: cfg.epoch });
            }
            if block.flags.breaks_continuity() {
                gaps.observe(now, block.start_sample, block.flags);
            }
            batch.push(block);
        }
        gaps.flush(now);
        if let Some(b) = batch.last() {
            latest.store(b.end_sample(), Ordering::Release);
        }
        stamp_arrivals(&mut batch, cfg.rate);
        for block in &batch {
            update_input_meters(meters, &mut ms, block, cfg.rate);
            if let Some((l, _)) = &mut levels {
                l.push(block);
            }
        }
        if batch.is_empty() || jobs.is_empty() {
            pool.give_back(batch);
        } else {
            let batch: Batch = Arc::new(batch);
            jobs.retain(|id, feed| {
                let q = feed.queue.frames.load(Ordering::Acquire);
                if q > 0 && q + frames > consumer_limit(&feed.queue, queue_limit) {
                    // The job is behind; it sees the gap and restarts.
                    return true;
                }
                let queued = feed.queue.frames.fetch_add(frames, Ordering::SeqCst) + frames;
                if feed.tx.send(JobMsg::Blocks(Arc::clone(&batch))).is_ok() {
                    let wake_at = feed.queue.wake_at.load(Ordering::SeqCst);
                    if wake_at != 0 && queued >= wake_at {
                        feed.thread.unpark();
                    }
                    true
                } else {
                    tracing::debug!("job {id} detached");
                    false
                }
            });
            pool.hand_out(batch);
        }
        if let Some((l, em)) = &mut levels {
            l.emit(em);
        }

        // Output timing records are not used yet; keep the ring from overflowing.
        while stream.pop_output_tick().is_some() {}
        if !ended_reported && stream.events().ended {
            ended_reported = true;
            tracing::error!("audio stream ended by the host");
            let _ = to_control.send(ControlMsg::DeviceChanged { epoch: cfg.epoch });
        }
    }
}

/// Frames `queue`'s consumer may have queued.
fn consumer_limit(queue: &Queue, default: u64) -> u64 {
    match queue.limit.load(Ordering::Relaxed) {
        0 => default,
        l => l,
    }
}

/// Wall clock of each block of a hand-off, as if it had been received the moment its last
/// sample was captured. All of them were popped at about the same instant, but the older
/// ones arrived earlier by the audio captured after them; the SPL log places seconds by
/// these stamps, so they must not jump by a hand-off period.
fn stamp_arrivals(batch: &mut [Block], rate: f64) {
    let Some(newest) = batch.last().map(|b| (b.end_sample(), b.wall_ns)) else {
        return;
    };
    if rate <= 0.0 {
        return;
    }
    let (end, wall) = newest;
    for b in batch.iter_mut() {
        let after_ns = (end - b.end_sample()) as f64 / rate * 1e9;
        b.wall_ns = wall.saturating_sub(after_ns as u64);
    }
}

/// Running per-input mean square, τ = 1 s and 0.2 s, and the clip marks.
fn update_input_meters(meters: &InputMeters, ms: &mut [(f64, f64)], block: &Block, rate: f64) {
    let n = usize::from(block.channels).max(1);
    let frames = block.data.len() / n;
    if frames == 0 {
        return;
    }
    let alpha = |tau: f64| 1.0 - (-f64::from(block.frames) / (rate * tau)).exp();
    let (a_slow, a_fast) = (alpha(METER_SLOW_S), alpha(METER_FAST_S));
    for (ch, (m, st)) in ms.iter_mut().zip(&block.stats).enumerate().take(n) {
        let sq = st.sum_sq / frames as f64;
        if st.peak >= CLIP_FULL_SCALE {
            meters.clip_end[ch].store(block.end_sample(), Ordering::Release);
        }
        m.0 += a_slow * (sq - m.0);
        m.1 += a_fast * (sq - m.1);
        meters.ms[ch].store(m.0.to_bits(), Ordering::Release);
        meters.fast[ch].store(m.1.to_bits(), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_match_a_per_channel_scan() {
        let data: Vec<f32> = (0..300)
            .map(|i| ((i * 37 % 101) as f32 - 50.0) / 51.0)
            .collect();
        let b = Block::new(0, 100, 3, BlockFlags::NONE, 0, data.clone());
        for ch in 0..3 {
            let (s, p) = data
                .iter()
                .skip(ch)
                .step_by(3)
                .fold((0.0f64, 0.0f32), |(s, p), v| {
                    (s + f64::from(*v) * f64::from(*v), p.max(v.abs()))
                });
            assert_eq!(b.stats[ch], ChanStats { sum_sq: s, peak: p });
        }
    }

    #[test]
    fn handoff_follows_the_faster_of_publish_and_meter_rates_but_not_below_a_block() {
        let ms = |d: Duration| (d.as_secs_f64() * 1000.0 * 10.0).round() / 10.0;
        assert_eq!(ms(handoff_period(60, None)), 16.7);
        assert_eq!(ms(handoff_period(30, None)), 16.7);
        assert_eq!(ms(handoff_period(120, None)), 8.3);
        let long_block = Duration::from_secs_f64(2048.0 / 48_000.0);
        assert_eq!(handoff_period(60, Some(long_block)), long_block);
    }

    #[test]
    fn older_blocks_of_a_hand_off_are_stamped_earlier_by_the_audio_after_them() {
        let mut batch: Vec<Block> = (0..3)
            .map(|i| {
                Block::new(
                    i * 480,
                    480,
                    1,
                    BlockFlags::NONE,
                    1_000_000_000,
                    vec![0.0; 480],
                )
            })
            .collect();
        stamp_arrivals(&mut batch, 48_000.0);
        let walls: Vec<u64> = batch.iter().map(|b| b.wall_ns).collect();
        assert_eq!(walls, [980_000_000, 990_000_000, 1_000_000_000]);
    }

    #[test]
    fn a_released_batch_returns_its_blocks_to_the_pool() {
        let mut pool = Pool::default();
        let mut v = pool.batch();
        v.push(Block::new(0, 1, 1, BlockFlags::NONE, 0, vec![0.5]));
        let batch: Batch = Arc::new(v);
        let held = Arc::clone(&batch);
        pool.hand_out(batch);
        pool.reclaim();
        assert!(pool.spare_blocks.is_empty(), "a job still holds the batch");
        drop(held);
        pool.reclaim();
        assert_eq!(pool.spare_blocks.len(), 1);
        assert_eq!(pool.spare_batches.len(), 1);
        assert!(pool.in_flight.is_empty());
    }
}
