//! The open audio session: one duplex stream, its generator handle and the capture fan-out.

use std::sync::mpsc::Sender;

use ac2_audio::{
    AudioError, Backend, DuplexRequest, GeneratorHandle, HistoryReader, HistoryRequest, MaxLevel,
    OutputSource, generator,
};
use ac2_proto::ErrorCode;
use ac2_proto::ProtoError;
use ac2_proto::model::{OpenSession, SessionConfig};
use ac2_proto::units::{SessionEpoch, WallNs};

use crate::control::ControlMsg;
use crate::conv;
use crate::fanout::Fanout;
use crate::util::{perr, wall_ns};

/// A running session.
pub(crate) struct Runtime {
    pub(crate) epoch: SessionEpoch,
    pub(crate) open: OpenSession,
    pub(crate) sample_rate: u32,
    /// Device input channel per block channel.
    pub(crate) input_map: Vec<u16>,
    pub(crate) output_channels: u16,
    /// Output channels the generator is routed to (fixed per stream).
    pub(crate) routes: Vec<u16>,
    pub(crate) gen_handle: GeneratorHandle,
    pub(crate) history: Option<HistoryReader>,
    pub(crate) fanout: Fanout,
}

pub(crate) fn audio_err(e: AudioError) -> ProtoError {
    let code = match &e {
        AudioError::DeviceNotFound { .. } => ErrorCode::NotFound,
        AudioError::Unsupported(_) | AudioError::Unavailable { .. } => ErrorCode::Unsupported,
        AudioError::InvalidRequest(_) => ErrorCode::Invalid,
        AudioError::Backend { .. } => ErrorCode::Internal,
    };
    perr(code, e.to_string())
}

/// Checks a session configuration without a device.
pub(crate) fn validate(cfg: &SessionConfig) -> Result<(), ProtoError> {
    if cfg.input_channels.is_empty() {
        return Err(perr(ErrorCode::Invalid, "no input channels"));
    }
    for (i, c) in cfg.input_channels.iter().enumerate() {
        if cfg.input_channels[..i].contains(c) {
            return Err(perr(ErrorCode::Invalid, format!("input {c} listed twice")));
        }
    }
    if let Some(l) = cfg.loopback {
        if !cfg.input_channels.contains(&l.input) {
            return Err(perr(
                ErrorCode::Invalid,
                format!("loopback input {} is not captured", l.input),
            ));
        }
        if l.output >= cfg.output_channels {
            return Err(perr(
                ErrorCode::Invalid,
                format!(
                    "loopback output {} is not an output of the stream",
                    l.output
                ),
            ));
        }
    }
    Ok(())
}

impl Runtime {
    /// Opens the stream with the generator routed to `routes`; silent until a source is set
    /// and started.
    pub(crate) fn open(
        backend: &dyn Backend,
        cfg: &SessionConfig,
        routes: &[u16],
        max_level: MaxLevel,
        epoch: SessionEpoch,
        to_control: Sender<ControlMsg>,
    ) -> Result<Self, ProtoError> {
        validate(cfg)?;
        let (gen_handle, port) =
            generator(routes.to_vec()).map_err(|e| perr(ErrorCode::Invalid, e.to_string()))?;
        let req = DuplexRequest {
            input_device: conv::device_selector(&cfg.input_device),
            output_device: conv::device_selector(&cfg.output_device),
            input_map: cfg.input_channels.clone(),
            output_channels: cfg.output_channels,
            sample_rate: cfg.sample_rate_hz,
            buffer_frames: cfg.buffer_frames,
            ring_seconds: DuplexRequest::DEFAULT_RING_SECONDS,
            output: OutputSource::Generator(port),
            max_level,
            history: cfg.loopback.map(|l| HistoryRequest::channel(l.output)),
        };
        let stream = backend.open(req).map_err(audio_err)?;
        let n = stream.negotiated().clone();
        let history = stream.history().cloned();
        let open = OpenSession {
            config: cfg.clone(),
            backend: conv::backend_kind(backend.kind()),
            input_device: ac2_proto::model::DeviceId(n.input_device.0.clone()),
            output_device: ac2_proto::model::DeviceId(n.output_device.0.clone()),
            sample_rate_hz: n.sample_rate,
            buffer_frames: n.buffer_frames.or(cfg.buffer_frames).unwrap_or(0),
            clock: conv::clock(n.clock),
            opened_at: WallNs(wall_ns()),
        };
        tracing::info!(
            "session open: {} in / {} out @ {} Hz on {:?}/{:?} ({:?}), epoch {}",
            n.input_channels,
            n.output_channels,
            n.sample_rate,
            n.input_device.0,
            n.output_device.0,
            n.clock,
            epoch.0
        );
        let fanout = Fanout::spawn(stream, epoch, to_control)
            .map_err(|e| perr(ErrorCode::Internal, format!("cannot start fan-out: {e}")))?;
        Ok(Self {
            epoch,
            open,
            sample_rate: n.sample_rate,
            input_map: cfg.input_channels.clone(),
            output_channels: cfg.output_channels,
            routes: routes.to_vec(),
            gen_handle,
            history,
            fanout,
        })
    }

    /// Fades out, stops the stream and its threads.
    pub(crate) fn close(self) {
        self.gen_handle.stop();
        self.fanout.stop();
        tracing::info!("session epoch {} closed", self.epoch.0);
    }
}
