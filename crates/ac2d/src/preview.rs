//! Device preview: meters of every input of a device before a session opens on it, so the
//! operator can see which input carries the loopback and which the mics.
//!
//! The preview opens the device for capture only. Its request has no output channels and a
//! silent output source, so there is no output stream at all: nothing can be emitted,
//! whatever else happens in the daemon. It lives until it is stopped, replaced, a session
//! opens, or it is not renewed in time ([`PREVIEW_EXPIRY`]).
//!
//! A renewal checks the stream is still delivering: a stream the host ended or that has
//! delivered nothing for [`PREVIEW_STALL`] is reopened instead of renewed, so the meters
//! never stay blank while the dialog keeps asking for them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_audio::{Backend, DeviceSelector, DuplexRequest, MaxLevel, OutputSource};
use ac2_proto::ErrorCode;
use ac2_proto::ProtoError;
use ac2_proto::cal;
use ac2_proto::frame::{FrameData, PreviewLevelsFrame, PreviewLevelsMeta};
use ac2_proto::model::{
    BackendKind, Ballistic, DeviceId, InputSetup, Pair, Preview as WirePreview, PreviewInput,
};
use ac2_proto::topic::Topic;
use ac2_proto::units::Rev;

use crate::burst::BurstDetector;
use crate::fanout::pop_block;
use crate::jobs::meters::{meter_period, meter_stamp};
use crate::jobs::programme::{Captured, Programme, SharedPolicy};
use crate::jobs::{Emitter, JobEnv, LevelsMeter, Meters};
use crate::session::audio_err;
use crate::util::perr;

/// A preview not renewed within this closes.
pub(crate) const PREVIEW_EXPIRY: Duration = Duration::from_secs(5);
/// A preview that delivered no audio for this long is dead: a client reads its meters as
/// stale after one second already.
pub(crate) const PREVIEW_STALL: Duration = Duration::from_secs(2);
/// No block yet.
const NEVER: u64 = u64::MAX;
/// Most inputs metered (the frame header lists one channel per column).
const MAX_PREVIEW_INPUTS: u16 = 64;
const STOP_TIMEOUT: Duration = Duration::from_millis(300);

/// A running preview.
pub(crate) struct Preview {
    pub(crate) backend: BackendKind,
    pub(crate) device: DeviceId,
    pub(crate) channels: u16,
    /// What every frame says about the device and its declared inputs.
    pub(crate) meta: PreviewLevelsMeta,
    pub(crate) sample_rate: u32,
    pub(crate) deadline: Instant,
    opened: Instant,
    /// Milliseconds after `opened` of the newest block, or [`NEVER`].
    last_block_ms: Arc<AtomicU64>,
    /// The host ended the stream.
    ended: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Preview {
    /// Opens `device` of `backend` for capture of every input and starts metering.
    pub(crate) fn open(
        backend: &dyn Backend,
        (kind, device): (BackendKind, DeviceId),
        declared: Vec<PreviewInput>,
        pairs: Vec<Pair>,
        max_level: MaxLevel,
        env: JobEnv,
        policy: SharedPolicy,
    ) -> Result<Self, ProtoError> {
        let caps = backend
            .enumerate()
            .map_err(audio_err)?
            .into_iter()
            .find(|d| d.id.0 == device.0)
            .ok_or_else(|| {
                perr(
                    ErrorCode::NotFound,
                    format!("no {kind:?} device {:?}", device.0),
                )
            })?;
        let inputs = caps
            .input
            .as_ref()
            .map_or(0, |i| i.max_channels)
            .min(MAX_PREVIEW_INPUTS);
        if inputs == 0 {
            return Err(perr(
                ErrorCode::Invalid,
                format!("{} has no inputs to meter", caps.name),
            ));
        }
        let sel = DeviceSelector::Id(ac2_audio::DeviceId(device.0.clone()));
        let mut req = DuplexRequest::new((0..inputs).collect(), 0, max_level);
        req.input_device = sel.clone();
        req.output_device = sel;
        // Capture only: no output channels, and silence even if a host opened one anyway.
        req.output = OutputSource::Silence;
        let mut stream = backend.open(req).map_err(audio_err)?;
        let sample_rate = stream.negotiated().sample_rate;
        let channels = stream.negotiated().input_channels;
        let mut bursts = BurstDetector::new(sample_rate, crate::burst::label(stream.negotiated()));
        tracing::info!(
            "preview of {:?} {:?}: {channels} inputs @ {sample_rate} Hz (capture only)",
            kind,
            device.0
        );
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let opened = Instant::now();
        let last_block_ms = Arc::new(AtomicU64::new(NEVER));
        let ended_flag = Arc::new(AtomicBool::new(false));
        let (lb, ef) = (Arc::clone(&last_block_ms), Arc::clone(&ended_flag));
        // The declaration follows the rules of `session.inputs`, over the device's inputs.
        let rows: Vec<InputSetup> = declared
            .iter()
            .map(|d| InputSetup {
                channel: d.channel,
                kind: d.kind.clone(),
                pair: None,
            })
            .collect();
        for (i, r) in rows.iter().enumerate() {
            if rows[..i].iter().any(|o| o.channel == r.channel) {
                let _ = stream.stop(STOP_TIMEOUT);
                return Err(perr(
                    ErrorCode::Invalid,
                    format!("input {} listed twice", u32::from(r.channel) + 1),
                ));
            }
        }
        let captured = |ch: u16| ch < channels;
        if let Err(e) = cal::check_pairs(&rows, &pairs, Some(&captured)) {
            let _ = stream.stop(STOP_TIMEOUT);
            return Err(perr(ErrorCode::Invalid, e));
        }
        let meta = PreviewLevelsMeta {
            backend: kind,
            device: device.clone(),
            channels: (0..channels).collect(),
            inputs: declared,
            pairs,
            ballistic: Ballistic::default(),
            pair_levels: Vec::new(),
        };
        let frame_meta = meta.clone();
        // The declared pairs stay as the dialog declared them; the pairs of a session do
        // not apply to a device that is only being looked at.
        let mut programme = Programme::new(
            Captured {
                idx: (0..usize::from(channels)).collect(),
                channels: meta.channels.clone(),
                device: device.clone(),
                sample_rate,
            },
            &meta.pairs,
            false,
            policy,
            crate::util::wall_ns(),
        );
        let thread = std::thread::Builder::new()
            .name("ac2d-preview".into())
            .spawn(move || {
                let em = match Emitter::connect(env) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!("preview: cannot reach the I/O thread: {e}");
                        let _ = stream.stop(STOP_TIMEOUT);
                        return;
                    }
                };
                let mut levels = LevelsMeter::new(
                    (0..usize::from(channels)).collect(),
                    frame_meta.channels.clone(),
                    sample_rate,
                );
                let mut wall = 0;
                let mut ended = false;
                // One wakeup per meter frame: the capture ring holds seconds, so nothing is
                // gained by draining it more often than the meters are published.
                let mut next = Instant::now();
                while !s.load(Ordering::Acquire) {
                    while let Some(b) = pop_block(&mut stream) {
                        let since =
                            u64::try_from(opened.elapsed().as_millis()).unwrap_or(NEVER - 1);
                        lb.store(since, Ordering::Release);
                        bursts.observe(Instant::now(), b.frames);
                        levels.push(&b);
                        programme.push(&b);
                        wall = b.wall_ns;
                    }
                    let stamp = meter_stamp(&levels, Rev(0), wall);
                    if let Some(Meters {
                        peak, rms, clip, ..
                    }) = levels.take_meters()
                    {
                        let r = programme.take(em.wants(Topic::PreviewGonio));
                        if em.wants(Topic::PreviewLevels) {
                            let mut meta = frame_meta.clone();
                            meta.ballistic = r.ballistic;
                            meta.pair_levels = r.pair_levels;
                            em.send(
                                stamp,
                                FrameData::PreviewLevels(PreviewLevelsFrame {
                                    meta,
                                    peak,
                                    rms,
                                    clip,
                                    ppm: r.ppm,
                                    true_peak: r.true_peak,
                                    max: r.max,
                                    overs: r.overs,
                                }),
                            );
                        }
                        if let Some(g) = r.gonio {
                            em.send(stamp, FrameData::PreviewGonio(g));
                        }
                    }
                    if !ended && stream.events().ended {
                        ended = true;
                        ef.store(true, Ordering::Release);
                        tracing::warn!("preview: the host ended the stream");
                    }
                    next += meter_period();
                    let now = Instant::now();
                    if next <= now {
                        next = now + meter_period();
                    }
                    std::thread::sleep(next - now);
                }
                let outcome = stream.stop(STOP_TIMEOUT);
                tracing::info!("preview stopped: {outcome:?}");
            })
            .map_err(|e| {
                perr(
                    ErrorCode::Internal,
                    format!("cannot start the preview: {e}"),
                )
            })?;
        Ok(Self {
            backend: kind,
            device,
            channels,
            meta,
            sample_rate,
            deadline: Instant::now() + PREVIEW_EXPIRY,
            opened,
            last_block_ms,
            ended: ended_flag,
            stop,
            thread: Some(thread),
        })
    }

    /// Why the stream is no use any more, if it is not: the host ended it, or no audio for
    /// [`PREVIEW_STALL`].
    pub(crate) fn dead(&self, now: Instant) -> Option<&'static str> {
        if self.ended.load(Ordering::Acquire) {
            return Some("the host ended the stream");
        }
        let since_open = now.saturating_duration_since(self.opened);
        let quiet = match self.last_block_ms.load(Ordering::Acquire) {
            NEVER => since_open,
            ms => since_open.saturating_sub(Duration::from_millis(ms)),
        };
        (quiet >= PREVIEW_STALL).then_some("no audio arrived")
    }

    /// Keeps the preview open for another [`PREVIEW_EXPIRY`].
    pub(crate) fn renew(&mut self) {
        self.deadline = Instant::now() + PREVIEW_EXPIRY;
    }

    /// The reply of `session.preview`.
    pub(crate) fn wire(&self) -> WirePreview {
        WirePreview {
            backend: self.backend,
            device: self.device.clone(),
            channels: self.channels,
            sample_rate_hz: self.sample_rate,
            expires_in_ms: u32::try_from(PREVIEW_EXPIRY.as_millis()).unwrap_or(u32::MAX),
        }
    }

    /// Stops metering and closes the device.
    pub(crate) fn close(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.stop_inner();
    }
}
