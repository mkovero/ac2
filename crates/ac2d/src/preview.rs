//! Device preview: meters of every input of a device before a session opens on it, so the
//! operator can see which input carries the loopback and which the mics.
//!
//! The preview opens the device for capture only. Its request has no output channels and a
//! silent output source, so there is no output stream at all: nothing can be emitted,
//! whatever else happens in the daemon. It lives until it is stopped, replaced, a session
//! opens, or it is not renewed in time ([`PREVIEW_EXPIRY`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_audio::{Backend, DeviceSelector, DuplexRequest, MaxLevel, OutputSource};
use ac2_proto::ErrorCode;
use ac2_proto::ProtoError;
use ac2_proto::frame::{FrameData, PreviewLevelsFrame, PreviewLevelsMeta};
use ac2_proto::model::{BackendKind, DeviceId, Preview as WirePreview};
use ac2_proto::topic::Topic;
use ac2_proto::units::Rev;

use crate::fanout::pop_block;
use crate::jobs::meters::{meter_period, meter_stamp};
use crate::jobs::{Emitter, JobEnv, LevelsMeter, Meters};
use crate::session::audio_err;
use crate::util::perr;

/// A preview not renewed within this closes.
pub(crate) const PREVIEW_EXPIRY: Duration = Duration::from_secs(5);
/// Most inputs metered (the frame header lists one channel per column).
const MAX_PREVIEW_INPUTS: u16 = 64;
const STOP_TIMEOUT: Duration = Duration::from_millis(300);

/// A running preview.
pub(crate) struct Preview {
    pub(crate) backend: BackendKind,
    pub(crate) device: DeviceId,
    pub(crate) channels: u16,
    pub(crate) sample_rate: u32,
    pub(crate) deadline: Instant,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Preview {
    /// Opens `device` of `backend` for capture of every input and starts metering.
    pub(crate) fn open(
        backend: &dyn Backend,
        kind: BackendKind,
        device: DeviceId,
        max_level: MaxLevel,
        env: JobEnv,
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
        tracing::info!(
            "preview of {:?} {:?}: {channels} inputs @ {sample_rate} Hz (capture only)",
            kind,
            device.0
        );
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let meta = PreviewLevelsMeta {
            backend: kind,
            device: device.clone(),
            channels: (0..channels).collect(),
        };
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
                    meta.channels.clone(),
                    sample_rate,
                );
                let mut wall = 0;
                let mut last: Option<Instant> = None;
                let mut ended = false;
                while !s.load(Ordering::Acquire) {
                    let mut got = 0;
                    while got < 64 {
                        let Some(b) = pop_block(&mut stream) else {
                            break;
                        };
                        levels.push(&b);
                        wall = b.wall_ns;
                        got += 1;
                    }
                    if last.is_none_or(|t| t.elapsed() >= meter_period()) {
                        last = Some(Instant::now());
                        let stamp = meter_stamp(&levels, Rev(0), wall);
                        if let Some(Meters {
                            peak, rms, clip, ..
                        }) = levels.take_meters()
                            && em.wants(Topic::PreviewLevels)
                        {
                            em.send(
                                stamp,
                                FrameData::PreviewLevels(PreviewLevelsFrame {
                                    meta: meta.clone(),
                                    peak,
                                    rms,
                                    clip,
                                }),
                            );
                        }
                    }
                    if !ended && stream.events().ended {
                        ended = true;
                        tracing::warn!("preview: the host ended the stream");
                    }
                    std::thread::sleep(Duration::from_millis(5));
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
            sample_rate,
            deadline: Instant::now() + PREVIEW_EXPIRY,
            stop,
            thread: Some(thread),
        })
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
