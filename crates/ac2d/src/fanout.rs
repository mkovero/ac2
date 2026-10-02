//! Capture fan-out: the one consumer of the stream's capture ring.
//!
//! The audio callback only writes the wait-free ring (no allocation there). This thread pops
//! whole blocks, wraps each in one shared [`Block`] and hands it to every attached job over
//! a bounded channel. A job that falls behind loses blocks (its channel is full); it sees the
//! gap as a jump in `start_sample` and restarts its averages, it never splices. The
//! fan-out also keeps per-input running levels (for `cal.spl`) and watches for configuration
//! changes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::time::Duration;

use ac2_audio::{BlockFlags, DuplexStream};
use ac2_proto::units::SessionEpoch;

use crate::control::ControlMsg;
use crate::util::wall_ns;

/// One captured block shared by all jobs.
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
    pub(crate) data: Box<[f32]>,
}

impl Block {
    pub(crate) fn end_sample(&self) -> u64 {
        self.start_sample + u64::from(self.frames)
    }

    /// Copies block channel `ch` into `out`.
    pub(crate) fn channel_into(&self, ch: usize, out: &mut Vec<f32>) {
        out.clear();
        let n = usize::from(self.channels);
        out.extend(self.data.iter().skip(ch).step_by(n.max(1)).copied());
    }
}

/// Per-input running mean square, f64 bits; read by `cal.spl`. Two time constants: the
/// 1 s one is the reading, the 0.2 s one tells whether the level has been steady long
/// enough for the slow one to have settled.
#[derive(Debug)]
pub(crate) struct InputMeters {
    ms: Box<[AtomicU64]>,
    fast: Box<[AtomicU64]>,
}

/// Time constants of [`InputMeters`], s.
const METER_SLOW_S: f64 = 1.0;
const METER_FAST_S: f64 = 0.2;

impl InputMeters {
    fn new(channels: usize) -> Self {
        Self {
            ms: (0..channels).map(|_| AtomicU64::new(0)).collect(),
            fast: (0..channels).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    /// Mean square of block channel `ch` (τ = 1 s) and its fast companion (τ = 0.2 s).
    pub(crate) fn mean_square(&self, ch: usize) -> Option<(f64, f64)> {
        let load = |a: &AtomicU64| f64::from_bits(a.load(Ordering::Acquire));
        Some((load(self.ms.get(ch)?), load(self.fast.get(ch)?)))
    }
}

pub(crate) enum FanoutMsg {
    Attach(u64, SyncSender<Arc<Block>>),
    Detach(u64),
    Stop,
}

/// Running fan-out thread.
pub(crate) struct Fanout {
    tx: Sender<FanoutMsg>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub(crate) meters: Arc<InputMeters>,
    /// One past the newest captured sample.
    pub(crate) latest: Arc<AtomicU64>,
}

/// Time to wait for the final fade-out before tearing the stream down regardless.
const STOP_TIMEOUT: Duration = Duration::from_millis(300);

impl Fanout {
    pub(crate) fn spawn(
        stream: DuplexStream,
        epoch: SessionEpoch,
        to_control: Sender<ControlMsg>,
    ) -> std::io::Result<Self> {
        let channels = usize::from(stream.negotiated().input_channels);
        let rate = f64::from(stream.negotiated().sample_rate);
        let (tx, rx) = std::sync::mpsc::channel();
        let meters = Arc::new(InputMeters::new(channels));
        let latest = Arc::new(AtomicU64::new(0));
        let m = Arc::clone(&meters);
        let l = Arc::clone(&latest);
        let thread = std::thread::Builder::new()
            .name("ac2d-fanout".into())
            .spawn(move || run(stream, &rx, epoch, &to_control, &m, &l, rate))?;
        Ok(Self {
            tx,
            thread: Some(thread),
            meters,
            latest,
        })
    }

    pub(crate) fn attach(&self, id: u64, tx: SyncSender<Arc<Block>>) {
        let _ = self.tx.send(FanoutMsg::Attach(id, tx));
    }

    pub(crate) fn detach(&self, id: u64) {
        let _ = self.tx.send(FanoutMsg::Detach(id));
    }

    /// Fades out, stops the stream and joins the thread.
    pub(crate) fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        let _ = self.tx.send(FanoutMsg::Stop);
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

fn run(
    mut stream: DuplexStream,
    rx: &Receiver<FanoutMsg>,
    epoch: SessionEpoch,
    to_control: &Sender<ControlMsg>,
    meters: &InputMeters,
    latest: &AtomicU64,
    rate: f64,
) {
    let mut jobs: BTreeMap<u64, SyncSender<Arc<Block>>> = BTreeMap::new();
    let mut ms: Vec<(f64, f64)> = vec![(0.0, 0.0); meters.ms.len()];
    let mut ended_reported = false;
    loop {
        loop {
            match rx.try_recv() {
                Ok(FanoutMsg::Attach(id, tx)) => {
                    jobs.insert(id, tx);
                }
                Ok(FanoutMsg::Detach(id)) => {
                    jobs.remove(&id);
                }
                Ok(FanoutMsg::Stop) | Err(TryRecvError::Disconnected) => {
                    let outcome = stream.stop(STOP_TIMEOUT);
                    tracing::info!("audio stream stopped: {outcome:?}");
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        let mut got = 0usize;
        while got < 64 {
            let Some(block) = stream.capture().pop_with(|h, a, b| {
                let mut data = Vec::with_capacity(a.len() + b.len());
                data.extend_from_slice(a);
                data.extend_from_slice(b);
                Block {
                    start_sample: h.start_sample,
                    frames: h.frames,
                    channels: h.channels,
                    flags: h.flags,
                    wall_ns: wall_ns(),
                    data: data.into_boxed_slice(),
                }
            }) else {
                break;
            };
            got += 1;
            if block.flags.contains(BlockFlags::CONFIG_CHANGE) {
                tracing::warn!(
                    "audio configuration changed at sample {}",
                    block.start_sample
                );
                let _ = to_control.send(ControlMsg::DeviceChanged { epoch });
            }
            if block.flags.breaks_continuity() {
                tracing::warn!(
                    "capture discontinuity at sample {} ({:?})",
                    block.start_sample,
                    block.flags
                );
            }
            // Running per-input mean square, τ = 1 s and 0.2 s.
            let n = usize::from(block.channels).max(1);
            let alpha = |tau: f64| 1.0 - (-f64::from(block.frames) / (rate * tau)).exp();
            let (a_slow, a_fast) = (alpha(METER_SLOW_S), alpha(METER_FAST_S));
            for (ch, m) in ms.iter_mut().enumerate().take(n) {
                let frames = block.data.len() / n;
                if frames == 0 {
                    continue;
                }
                let sq: f64 = block
                    .data
                    .iter()
                    .skip(ch)
                    .step_by(n)
                    .map(|v| f64::from(*v) * f64::from(*v))
                    .sum::<f64>()
                    / frames as f64;
                m.0 += a_slow * (sq - m.0);
                m.1 += a_fast * (sq - m.1);
                meters.ms[ch].store(m.0.to_bits(), Ordering::Release);
                meters.fast[ch].store(m.1.to_bits(), Ordering::Release);
            }
            latest.store(block.end_sample(), Ordering::Release);
            let block = Arc::new(block);
            jobs.retain(|id, tx| match tx.try_send(Arc::clone(&block)) {
                Ok(()) | Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => {
                    tracing::debug!("job {id} detached");
                    false
                }
            });
        }
        // Output timing records are not used yet; keep the ring from overflowing.
        while stream.pop_output_tick().is_some() {}
        if !ended_reported && stream.events().ended {
            ended_reported = true;
            tracing::error!("audio stream ended by the host");
            let _ = to_control.send(ControlMsg::DeviceChanged { epoch });
        }
        if got == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
