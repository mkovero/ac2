//! Measurement jobs: one thread each, fed by the capture fan-out, publishing frames into the
//! I/O thread's latest slots.
//!
//! A job's lifetime follows commands (`meas.start/stop`, session open/close), never
//! subscriptions. Subscriptions decide what is built and sent: a result nobody receives is
//! neither formed nor encoded, and one that has not changed since it was last sent is only
//! re-sent now and then to keep it fresh ([`Pace`]). Every job publishes at most at the
//! publish rate; between frames it only accumulates.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::frame::{
    BinsFrame, ClipFlags, Frame, FrameData, FrameStamp, IrFrame, LevelsFrame, LevelsMeta,
    ProtectionFlags,
};
use ac2_proto::grid::GridId;
use ac2_proto::topic::Topic;
use ac2_proto::units::{DaemonIncarnation, MeasId, Rev, SampleIndex, SessionEpoch, WallNs};
use ac2_zmq::Context;

use crate::cadence::Cadence;
use crate::fanout::{Batch, Block, JobFeed, Queue};
use crate::io::Interest;
use crate::outbox::{FrameTx, Outbox};

#[cfg(test)]
pub(crate) use pace::REFRESH;
pub(crate) use pace::{Due, Flush, Pace};

mod bins;
pub(crate) mod cross_rig;
pub(crate) mod dynamics;
pub(crate) mod finder;
mod loudness;
pub(crate) mod math;
pub(crate) mod meters;
mod pace;
#[cfg(test)]
mod pace_tests;
pub(crate) mod programme;
pub(crate) mod record;
pub(crate) mod rta;
pub(crate) mod spectrum;
pub(crate) mod spl;
pub(crate) mod sweep;
pub(crate) mod tf_smooth;
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
    /// Where frames go to the I/O thread.
    pub(crate) frames: FrameTx,
}

/// The measurement a job runs: its id, its configuration at `config_rev`, and the session
/// epoch it starts in.
pub(crate) struct JobCtx<C> {
    pub(crate) meas: MeasId,
    pub(crate) cfg: C,
    pub(crate) config_rev: Rev,
    pub(crate) epoch: SessionEpoch,
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
    asked: RefCell<Asked>,
    /// The config rev of the last frame sent.
    last_rev: std::cell::Cell<Option<Rev>>,
}

/// Which of the topics this thread asks about have a subscriber, as of `generation` of the
/// daemon's [`Interest`]: a job asks about the same few topics every block, and the answer
/// changes only when a subscription does.
#[derive(Debug, Default)]
struct Asked {
    generation: u64,
    topics: Vec<(Topic, bool)>,
}

impl Emitter {
    /// An emitter for a thread that is not a job (the preview).
    pub(crate) fn connect(env: JobEnv) -> std::io::Result<Self> {
        let outbox = Outbox::connect(&env.ctx, &env.endpoint, 64)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(Self::new(outbox, env))
    }

    fn new(outbox: Outbox, env: JobEnv) -> Self {
        Self {
            outbox,
            env,
            asked: RefCell::default(),
            last_rev: std::cell::Cell::default(),
        }
    }

    pub(crate) fn wants(&self, t: Topic) -> bool {
        let mut asked = self.asked.borrow_mut();
        // Read before the subscriptions are: an answer kept under this count is then never
        // older than the count says.
        let generation = self.env.interest.generation();
        if asked.generation != generation {
            asked.generation = generation;
            asked.topics.clear();
        }
        if let Some(&(_, wanted)) = asked.topics.iter().find(|(k, _)| *k == t) {
            return wanted;
        }
        let wanted = self.env.interest.wants(&t.to_bytes());
        asked.topics.push((t, wanted));
        wanted
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

    /// Publishes `data`, quantised here and encoded by the I/O thread; `false` if it did
    /// not reach the I/O thread (channel full).
    pub(crate) fn send(&self, s: StampArgs, data: FrameData) -> bool {
        let topic = data.topic();
        let frame = self.frame(s, self.env.seqs.next(topic), data);
        self.last_rev.set(Some(s.config_rev));
        match ac2_proto::CodedFrame::from_frame(&frame) {
            Ok(coded) => {
                let sent = self.env.frames.send(coded);
                if sent {
                    self.outbox.wake();
                }
                sent
            }
            Err(e) => {
                tracing::error!("{topic}: frame not encodable: {e}");
                false
            }
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
    /// The generator started or stopped: a find places it in the audio it reads.
    GeneratorEdge(finder::GeneratorEdge),
    /// New alignment delay (transfer); `rev` is the commit that set it. `resume`: the
    /// operator inserted or typed it, which resolves an ambiguous finding.
    SetDelay {
        samples: f64,
        seconds: f64,
        /// What the operator's nudges added to the arrival, samples.
        nudged_samples: f64,
        rev: Rev,
        resume: bool,
    },
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
    /// The sine the generator fires (Hz), or that it fires none: a spectrum reads its
    /// harmonics.
    Tone(Option<f64>),
    /// The clocks between the rigs of a cross-rig transfer function, judged anew.
    Clocks(cross_rig::ClockGate),
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
    /// Form the current result for a math trace's operand and answer on the channel.
    Capture(SyncSender<Option<Live>>),
    /// As `Capture`, with every analysis stage's bins of the same averages and stamp.
    CaptureBins(SyncSender<Option<bins::OperandBins>>),
    /// Form the current result with its impulse response for `trace.capture`.
    CaptureTrace(SyncSender<Option<Captured>>),
    /// Send the final frames and end; with a channel, also answer there with the result a
    /// `trace.capture` would take at that moment.
    Stop(Option<SyncSender<Option<Stopped>>>),
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
    /// The impulse response the IR pane would draw of the result [`Analysis::capture`]
    /// returns, formed from the same state; `None` for analyses without one, or before
    /// there is one (a transfer job that has accepted no block with a reference yet).
    fn capture_ir(&mut self) -> Option<IrFrame> {
        None
    }
    /// Every analysis stage's unquantised bins, from the averages [`Analysis::capture`]
    /// read when called right after it; `None` for an analysis that keeps no stage bins (one
    /// not at the `bins` resolution), which a math then cannot combine per bin.
    fn stage_bins(&mut self) -> Option<Vec<BinsFrame>> {
        None
    }
    /// Per column of [`Analysis::capture`]'s result, the frequency its estimate belongs to
    /// (`ac2_core::mtw::Mtw::column_hz`), where that is not the column centre; `None` for
    /// an analysis whose columns are their centres' or that does not know.
    fn column_hz(&self) -> Option<Arc<[f64]>> {
        None
    }
    /// Frames of further audio without which the job can form nothing new, so it sleeps
    /// through the hand-offs until that much is queued and takes them together; commands and
    /// stop wake it at once. `None`: every hand-off may change the result, so the job wakes
    /// for each.
    fn frames_needed(&self) -> Option<u64> {
        None
    }
    /// Samples between results when they come on a grid of whole device periods about a
    /// display frame apart (a live spectrum's frame hop): the fan-out then wakes for the
    /// block that completes each, so a result goes out within a period of it, and the
    /// publish cadence lets each through. `None`: results come as they come, at most at
    /// the publish rate.
    fn frame_hop(&self) -> Option<u64> {
        None
    }
    /// A count that advances whenever the result [`Analysis::capture`] returns may have
    /// changed (the job's own `Pace` generation): math traces read it through a [`Probe`]
    /// to form and publish a new result only when an operand has one. `None` for analyses
    /// a math trace cannot use as an operand.
    fn result_generation(&self) -> Option<u64>;
    /// On stop, the frames that outlive the job: a transfer sends its `tf`, its impulse
    /// response and (at the `bins` resolution) every stage's bins from the same averages
    /// under one stamp, past its publish schedule and whether or not anyone subscribes, so
    /// a view opened or a client connected after the stop gets the final result. Returns
    /// them as sent, in that order.
    fn final_frames(&mut self, _e: &Emitter) -> Vec<Frame> {
        Vec::new()
    }
}

/// Longest `trace.capture` waits for a job to form its result. The request wakes the job like
/// any message and is answered after at most one drain of queued audio, so only a stuck job
/// takes this long.
const CAPTURE_WAIT: Duration = Duration::from_secs(1);

/// A job's current result as a math takes it (through a [`Probe`]).
#[derive(Debug)]
pub(crate) struct Live {
    pub(crate) frame: Frame,
    /// [`Analysis::column_hz`].
    pub(crate) column_hz: Option<Arc<[f64]>>,
}

/// A job's result as `trace.capture` stores it.
#[derive(Clone, Debug)]
pub(crate) struct Captured {
    /// The curve result.
    pub(crate) frame: Frame,
    /// Its impulse response, when the job forms one.
    pub(crate) ir: Option<IrFrame>,
}

/// What a job leaves as it stops: the result a capture takes, and the final frames it sent
/// (none for a job that sends none: its views keep its last frames).
#[derive(Clone, Debug)]
pub(crate) struct Stopped {
    pub(crate) captured: Captured,
    pub(crate) finals: Vec<Frame>,
    /// The config rev of the last frame it sent before the stop.
    pub(crate) last_rev: Option<Rev>,
}

/// A running job thread.
pub(crate) struct JobHandle {
    tx: Sender<JobMsg>,
    thread: Option<JoinHandle<()>>,
    /// The analysis's [`Analysis::result_generation`], as of its last drain.
    results: Arc<AtomicU64>,
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

    /// The job's current curve result with its impulse response, formed on request:
    /// results are built for clients only while someone subscribes, so a capture cannot
    /// rely on a published frame. It is the newest state, so it is never older than any
    /// frame a client has seen.
    pub(crate) fn capture(&self) -> Option<Captured> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.tx.send(JobMsg::CaptureTrace(tx)).ok()?;
        self.wake();
        rx.recv_timeout(CAPTURE_WAIT).ok().flatten()
    }

    /// A handle another job can ask for this job's current result with.
    pub(crate) fn probe(&self) -> Option<Probe> {
        Some(Probe {
            tx: self.tx.clone(),
            thread: self.thread.as_ref()?.thread().clone(),
            results: Arc::clone(&self.results),
        })
    }

    /// Stops the job and returns the result a `trace.capture` would have taken as it
    /// stopped, formed after its final frames from the same state, so a capture of the
    /// stopped measurement stores what its views show. `None` when it had none yet.
    pub(crate) fn stop_keeping(mut self) -> Option<Stopped> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.stop_inner(Some(tx));
        rx.try_recv().ok().flatten()
    }

    fn stop_inner(&mut self, keep: Option<SyncSender<Option<Stopped>>>) {
        let _ = self.tx.send(JobMsg::Stop(keep));
        self.wake();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.stop_inner(None);
        }
    }
}

/// Asks a running job for its current result, as `trace.capture` does, from another thread
/// (a math trace asking its live operands). A probe of a stopped job answers nothing.
#[derive(Clone, Debug)]
pub(crate) struct Probe {
    tx: Sender<JobMsg>,
    thread: std::thread::Thread,
    results: Arc<AtomicU64>,
}

impl Probe {
    /// Sends the request; the answer arrives on the returned channel (nothing if the job
    /// has stopped).
    pub(crate) fn request(&self) -> Option<Receiver<Option<Live>>> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.tx.send(JobMsg::Capture(tx)).ok()?;
        self.thread.unpark();
        Some(rx)
    }

    /// The job's result generation as of its last drain of audio and commands: unchanged,
    /// it has nothing new to answer a request with.
    pub(crate) fn results(&self) -> u64 {
        self.results.load(Ordering::Acquire)
    }
}

/// The probes of running transfer, spectrum and RTA jobs by measurement, kept by control as
/// jobs start and stop, read by math traces when they publish.
#[derive(Debug, Default)]
pub(crate) struct Probes(Mutex<HashMap<MeasId, Probe>>);

impl Probes {
    pub(crate) fn set(&self, meas: MeasId, p: Option<Probe>) {
        let mut m = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match p {
            Some(p) => {
                m.insert(meas, p);
            }
            None => {
                m.remove(&meas);
            }
        }
    }

    pub(crate) fn get(&self, meas: MeasId) -> Option<Probe> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&meas)
            .cloned()
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
    let results = Arc::new(AtomicU64::new(0));
    let r = Arc::clone(&results);
    let per_period = analysis.frame_hop();
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
            let em = Emitter::new(outbox, env);
            run(&mut *analysis, &rx, &q, &em, &r);
        })?;
    let feed = JobFeed {
        tx: tx.clone(),
        queue,
        thread: thread.thread().clone(),
        per_period,
    };
    Ok((
        JobHandle {
            tx,
            thread: Some(thread),
            results,
            fanout_id,
        },
        feed,
    ))
}

/// Messages handled per wakeup before the job looks at its publish schedule again.
const DRAIN_MAX: usize = 64;

fn run(
    a: &mut dyn Analysis,
    rx: &Receiver<JobMsg>,
    queue: &Queue,
    em: &Emitter,
    results: &AtomicU64,
) {
    let publish = Duration::from_secs_f64(1.0 / f64::from(em.env.fps.max(1)));
    // Results on a device-period grid are already spaced by the analysis, a whole number
    // of periods near the publish period and so up to a period short of it; a cadence of
    // the publish period would hold every few back. Three quarters of it lets each through
    // and still stops a burst after a stall (and a lower publish rate passes every few).
    let period = if a.frame_hop().is_some() {
        publish.mul_f64(0.75)
    } else {
        publish
    };
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
    // The result generation the last emit carried.
    let mut emitted: Option<u64> = None;
    // When to emit again without new input (a frame held back by a rate or a refresh).
    let mut retry: Option<Instant> = None;
    // One past the newest sample taken in.
    let mut taken: Option<u64> = None;
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
                    taken = batch.last().map(|b| b.end_sample()).or(taken);
                    queue.frames.fetch_sub(frames, Ordering::AcqRel);
                    true
                }
                JobMsg::Cmd(c) => {
                    a.command(c);
                    true
                }
                JobMsg::Capture(reply) => {
                    let f = a.capture().map(|(s, d)| Live {
                        frame: em.frame(s, 0, d),
                        column_hz: a.column_hz(),
                    });
                    let _ = reply.send(f);
                    false
                }
                JobMsg::CaptureBins(reply) => {
                    // Nothing comes between the two reads, so the stages are of the same
                    // averages as the result.
                    let c = a.capture().map(|(s, d)| bins::OperandBins {
                        tf: Live {
                            frame: em.frame(s, 0, d),
                            column_hz: a.column_hz(),
                        },
                        stages: a.stage_bins(),
                    });
                    let _ = reply.send(c);
                    false
                }
                JobMsg::CaptureTrace(reply) => {
                    let _ = reply.send(captured(a, em));
                    false
                }
                JobMsg::Stop(keep) => {
                    // Only a `meas.stop` leaves a result: a job ended for a delete, a
                    // restart or the audio closing leaves nothing for the views to draw.
                    if let Some(reply) = keep {
                        let last_rev = em.last_rev.get();
                        let finals = a.final_frames(em);
                        let _ = reply.send(captured(a, em).map(|captured| Stopped {
                            captured,
                            finals,
                            last_rev,
                        }));
                    }
                    return;
                }
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
        if let Some(g) = a.result_generation() {
            results.store(g, Ordering::Release);
        }
        if let Some(end) = taken
            && let Some(n) = a.frames_needed().filter(|&n| n > 0)
        {
            queue.due.store(end.saturating_add(n), Ordering::Release);
        }
        let now = Instant::now();
        if changed || retry.is_some_and(|t| now >= t) {
            // A result slower than the frame rate (a long-window spectrum) goes on the
            // hand-off that formed it, not on the next slot of the grid.
            let generation = a.result_generation();
            let new_result = generation.is_some() && generation != emitted;
            match cadence.ready_at() {
                Some(t) if now < t && !(new_result && cadence.is_ready_for_result(now)) => {
                    // Too soon after the previous frame: emit at the slot (or on the
                    // hand-off around it).
                    retry = Some(retry.filter(|_| !changed).map_or(t, |r| r.max(t)));
                }
                _ => {
                    if new_result {
                        cadence.take_result(now);
                        emitted = generation;
                    } else {
                        cadence.take(now);
                    }
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
/// The analysis's current result with its impulse response, as `trace.capture` stores it.
fn captured(a: &mut dyn Analysis, em: &Emitter) -> Option<Captured> {
    a.capture().map(|(s, d)| Captured {
        frame: em.frame(s, 0, d),
        ir: a.capture_ir(),
    })
}

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
    /// A remote sample of the meter's column went missing this interval: the zeros standing
    /// in for it are no signal, so the interval has no reading.
    missing: Vec<bool>,
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
            missing: vec![false; n],
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
            if !b.marks.gaps(ch).is_empty() {
                // The RMS integrates from the next whole block rather than through zeros.
                self.missing[m] = true;
                self.ms[m] = None;
                continue;
            }
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
        self.missing.fill(false);
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
            if std::mem::take(&mut self.missing[m]) {
                // NaN, not −∞: −∞ is digital silence, a reading.
                peak.push(f32::NAN);
                rms.push(f32::NAN);
            } else {
                peak.push((20.0 * f64::from(self.peak[m]).log10()) as f32);
                let r = self.ms[m].unwrap_or(0.0).sqrt();
                rms.push(ac2_core::spectrum::rms_dbfs(r) as f32);
            }
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
