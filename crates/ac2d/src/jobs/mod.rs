//! Measurement jobs: one thread each, fed by the capture fan-out, publishing frames into the
//! I/O thread's latest slots.
//!
//! A job's lifetime follows commands (`meas.start/stop`, session open/close), never
//! subscriptions. Subscriptions decide what is built and sent: a result nobody receives is
//! neither formed nor encoded, and one that has not changed since it was last sent is only
//! re-sent now and then to keep it fresh ([`Pace`]). Every job publishes at most at the
//! publish rate; between frames it only accumulates.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
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

use crate::cadence::Cadence;
use crate::fanout::{Batch, Block, JobFeed, Queue};
use crate::io::Interest;
use crate::outbox::Outbox;

pub(crate) mod finder;
pub(crate) mod meters;
#[cfg(test)]
mod pace_tests;
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

/// A job's way out.
pub(crate) struct Emitter {
    outbox: Outbox,
    env: JobEnv,
}

impl Emitter {
    /// An emitter for a thread that is not a job (the preview).
    pub(crate) fn connect(env: JobEnv) -> std::io::Result<Self> {
        let outbox = Outbox::connect(&env.ctx, &env.endpoint, 64)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(Self { outbox, env })
    }

    pub(crate) fn wants(&self, t: Topic) -> bool {
        self.env.interest.wants(&t.to_bytes())
    }

    /// The frame `data` stamped with `s` and sequence number `seq`.
    fn frame(&self, s: StampArgs, seq: u64, data: FrameData) -> Frame {
        Frame {
            stamp: FrameStamp {
                seq,
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
        }
    }

    /// Publishes `data`; `false` if it did not reach the I/O thread (pipe full).
    pub(crate) fn send(&self, s: StampArgs, data: FrameData) -> bool {
        let topic = data.topic();
        let frame = self.frame(s, self.env.seqs.next(topic), data);
        match ac2_proto::encode_frame(&frame) {
            Ok(parts) => self.outbox.frame(parts),
            Err(e) => {
                tracing::error!("{topic}: frame not encodable: {e}");
                false
            }
        }
    }
}

/// Longest a client goes without a frame of a result that has not changed (frozen, settled,
/// or gated by protection). A client marks a topic STALE after a second without a new frame
/// (`ac2_client::data::STALE_AFTER`): re-sending an unchanged result four times a second
/// keeps a live but steady measurement fresh with room for a late or dropped frame, while
/// only a stopped stream (no audio, so no blocks and no frames at all) goes STALE.
pub(crate) const REFRESH: Duration = Duration::from_millis(250);

/// What a [`Pace`] says about one result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Due {
    /// Build and send it now.
    Send,
    /// Nothing to send: nobody receives it, or it is unchanged and was sent recently.
    Skip,
    /// Changed, or due a refresh, but not yet; ask again at this instant.
    Later(Instant),
}

/// When one topic of a job is sent: on a new result (at most every `min_period` on
/// average, on a [`Cadence`] so jittery wakeups do not cost rate), on a change of its
/// protection flags, and every [`REFRESH`] while unchanged as long as audio keeps coming —
/// and only while someone subscribes to it.
#[derive(Debug)]
pub(crate) struct Pace {
    cadence: Cadence,
    sent: Option<Sent>,
}

#[derive(Clone, Copy, Debug)]
struct Sent {
    generation: u64,
    prot: ProtectionFlags,
    audio_sample: u64,
    at: Instant,
}

impl Pace {
    /// Sends a new result at most every `min_period` (zero: whenever the job emits).
    pub(crate) fn new(min_period: Duration) -> Self {
        Self {
            // A job emits on hand-offs a fraction of `min_period` apart; a quarter period of
            // lead lets the hand-off just before a slot take it instead of the one after.
            cadence: Cadence::new(min_period, min_period / 4),
            sent: None,
        }
    }

    /// Whether `topic`'s result of generation `generation` (a count the job advances whenever
    /// the result may have changed), stamped `stamp`, goes out at `now`. A `Send` is taken
    /// to be sent.
    pub(crate) fn due(
        &mut self,
        e: &Emitter,
        topic: Topic,
        generation: u64,
        stamp: &StampArgs,
        now: Instant,
    ) -> Due {
        if !e.wants(topic) {
            // A new subscriber gets the result on the next emit, whatever it was before.
            self.unsent();
            return Due::Skip;
        }
        let due = match self.sent {
            None => Due::Send,
            Some(s) => {
                if s.generation != generation || s.prot != stamp.protection {
                    match self.cadence.ready_at() {
                        Some(t) if now < t => Due::Later(t),
                        _ => Due::Send,
                    }
                } else if stamp.audio_sample <= s.audio_sample {
                    // Nothing new at all: the stream has stopped, and the client's STALE
                    // must say so.
                    Due::Skip
                } else if now.saturating_duration_since(s.at) >= REFRESH {
                    Due::Send
                } else {
                    // The refresh carries the newest stamp even after the last block.
                    Due::Later(s.at + REFRESH)
                }
            }
        };
        if due == Due::Send {
            self.cadence.take(now);
            self.sent = Some(Sent {
                generation,
                prot: stamp.protection,
                audio_sample: stamp.audio_sample,
                at: now,
            });
        }
        due
    }

    /// The result taken as sent did not go out: send it on the next emit.
    pub(crate) fn unsent(&mut self) {
        self.sent = None;
        self.cadence.reset();
    }
}

/// Whether `emit` sent everything it had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flush {
    Done,
    /// Something was held back (by its rate, or until its refresh); emit again at this
    /// instant even without new audio, so the newest result goes out after the last block.
    Pending(Instant),
}

impl Flush {
    /// [`Flush::Pending`] if `due` said to ask again.
    pub(crate) fn from_due(due: Due) -> Self {
        match due {
            Due::Later(t) => Self::Pending(t),
            Due::Send | Due::Skip => Self::Done,
        }
    }

    /// Pending if either is, at the sooner instant.
    pub(crate) fn and(self, o: Flush) -> Flush {
        match (self, o) {
            (Flush::Pending(a), Flush::Pending(b)) => Flush::Pending(a.min(b)),
            (Flush::Pending(t), Flush::Done) | (Flush::Done, Flush::Pending(t)) => {
                Flush::Pending(t)
            }
            (Flush::Done, Flush::Done) => Flush::Done,
        }
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
        samples: f64,
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
    /// Form the current result for a capture and answer on the channel.
    Capture(SyncSender<Option<Frame>>),
    Stop,
}

/// One analysis.
pub(crate) trait Analysis: Send {
    /// Accumulates one captured block.
    fn push(&mut self, b: &Block);
    /// Applies a command.
    fn command(&mut self, c: JobCmd);
    /// Publishes what is due of the current result.
    fn emit(&mut self, e: &Emitter) -> Flush;
    /// The current `tf` / `spec` / `rta` result for `trace.capture`, as its next frame would
    /// carry it but before display smoothing (a stored trace is re-smoothed); `None` for
    /// analyses without one, or before the first result.
    fn capture(&mut self) -> Option<(StampArgs, FrameData)>;
    /// Frames of further audio without which the job can form nothing new, so it sleeps
    /// through the hand-offs until that much is queued and takes them together; commands and
    /// stop wake it at once. `None`: every hand-off may change the result, so the job wakes
    /// for each.
    fn frames_needed(&self) -> Option<u64> {
        None
    }
}

/// Longest `trace.capture` waits for a job to form its result. The request wakes the job like
/// any message and is answered after at most one drain of queued audio, so only a stuck job
/// takes this long.
const CAPTURE_WAIT: Duration = Duration::from_secs(1);

/// A running job thread.
pub(crate) struct JobHandle {
    tx: Sender<JobMsg>,
    thread: Option<JoinHandle<()>>,
    /// Id of this job at the fan-out.
    pub(crate) fanout_id: u64,
}

impl JobHandle {
    pub(crate) fn send(&self, c: JobCmd) {
        let _ = self.tx.send(JobMsg::Cmd(c));
        self.wake();
    }

    /// Ends an [`Analysis::frames_needed`] sleep: anything but audio is handled at once.
    fn wake(&self) {
        if let Some(t) = &self.thread {
            t.thread().unpark();
        }
    }

    /// The job's current curve result, formed on request: results are built for clients
    /// only while someone subscribes, so a capture cannot rely on a published frame. It is
    /// the newest state, so it is never older than any frame a client has seen.
    pub(crate) fn capture(&self) -> Option<Frame> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.tx.send(JobMsg::Capture(tx)).ok()?;
        self.wake();
        rx.recv_timeout(CAPTURE_WAIT).ok().flatten()
    }

    fn stop_inner(&mut self) {
        let _ = self.tx.send(JobMsg::Stop);
        self.wake();
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
    let queue = Arc::new(Queue::default());
    let q = Arc::clone(&queue);
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
            let em = Emitter { outbox, env };
            run(&mut *analysis, &rx, &q, &em);
        })?;
    let feed = JobFeed {
        tx: tx.clone(),
        queue,
        thread: thread.thread().clone(),
    };
    Ok((
        JobHandle {
            tx,
            thread: Some(thread),
            fanout_id,
        },
        feed,
    ))
}

/// Messages handled per wakeup before the job looks at its publish schedule again.
const DRAIN_MAX: usize = 64;

fn run(a: &mut dyn Analysis, rx: &Receiver<JobMsg>, queue: &Queue, em: &Emitter) {
    let period = Duration::from_secs_f64(1.0 / f64::from(em.env.fps.max(1)));
    // Audio arrives once per fan-out hand-off, a publish period apart or less, give or take
    // scheduling jitter. A frame may go a quarter period early, so a hand-off just before
    // its slot takes it rather than the one after.
    let mut cadence = Cadence::new(period, period / 4);
    // A frame held back is due at a known instant, but audio usually arrives within a
    // period of it, and an emit on that hand-off carries newer results than one on a timer
    // would. The timer is for when audio stops: it waits this much past the instant, so it
    // seldom fires while audio flows.
    let grace = period;
    let mut changed = false;
    // When to emit again without new input (a frame held back by a rate or a refresh).
    let mut retry: Option<Instant> = None;
    loop {
        // Sleep until audio or a command arrives, or until a frame held back is due.
        let first = if let Some(at) = retry {
            let wait = (at + grace).saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
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
            // A capture only reads the result; it leaves nothing new to publish.
            changed |= match m {
                JobMsg::Blocks(batch) => {
                    let mut frames = 0u64;
                    for b in batch.iter() {
                        a.push(b);
                        frames += u64::from(b.frames);
                    }
                    queue.frames.fetch_sub(frames, Ordering::AcqRel);
                    true
                }
                JobMsg::Cmd(c) => {
                    a.command(c);
                    true
                }
                JobMsg::Capture(reply) => {
                    let f = a.capture().map(|(s, d)| em.frame(s, 0, d));
                    let _ = reply.send(f);
                    false
                }
                JobMsg::Stop => return,
            };
            handled += 1;
            if handled < DRAIN_MAX {
                next = match rx.try_recv() {
                    Ok(m) => Some(m),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                };
            }
        }
        let now = Instant::now();
        if changed || retry.is_some_and(|t| now >= t) {
            match cadence.ready_at() {
                Some(t) if now < t => {
                    // Too soon after the previous frame: emit at the slot (or on the
                    // hand-off around it).
                    retry = Some(retry.filter(|_| !changed).map_or(t, |r| r.max(t)));
                }
                _ => {
                    cadence.take(now);
                    changed = false;
                    retry = match a.emit(em) {
                        Flush::Done => None,
                        Flush::Pending(at) => Some(at),
                    };
                }
            }
        }
        if retry.is_none()
            && !changed
            && let Some(n) = a.frames_needed().filter(|&n| n > 0)
        {
            park_until_queued(queue, n);
        }
    }
}

/// Longest a job sleeps for audio it is waiting for; only a stopped stream lets it run out.
const PARK_MAX: Duration = Duration::from_secs(1);

/// Sleeps until `n` frames are queued. Parked rather than blocked in a receive, the thread
/// is not woken by each hand-off (a send wakes only a waiting receiver): the fan-out unparks
/// it once enough has arrived, and `JobHandle` for anything else.
fn park_until_queued(queue: &Queue, n: u64) {
    // Announced before the queue is looked at, and the fan-out adds to the queue before it
    // reads this, so one of the two always sees the other: a hand-off is never missed.
    queue.wake_at.store(n, Ordering::SeqCst);
    if queue.frames.load(Ordering::SeqCst) < n {
        std::thread::park_timeout(PARK_MAX);
    }
    queue.wake_at.store(0, Ordering::SeqCst);
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

    /// Sends the interval's meters of measurement `meas` (stamped `s`, without a grid) when
    /// someone receives them; a new interval starts either way.
    pub(crate) fn send(&mut self, e: &Emitter, meas: MeasId, s: StampArgs) {
        let topic = Topic::Data {
            meas,
            stream: ac2_proto::topic::Stream::Levels,
        };
        if !e.wants(topic) {
            self.reset_interval();
            return;
        }
        if let Some(l) = self.take(meas) {
            e.send(StampArgs { grid_id: None, ..s }, FrameData::Levels(l));
        }
    }

    /// Starts a new interval without reading the old one.
    fn reset_interval(&mut self) {
        self.peak.fill(0.0);
        self.clipped.fill(false);
        self.frames = 0;
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
