//! What the adapter needs from the HAL, as a trait: [`CahalSource`] is `cahal` on macOS,
//! the tests' scripted devices are the other implementation.

use std::sync::Arc;
use std::time::Duration;

use cahal::{
    ClockSource, Cycle, DeviceProperty, Error, FrameRange, IoConfig, IoStats, Latency, RateRange,
    Scope, StreamLayout, Transport,
};

/// Everything the adapter reads about one device, in one go.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DevDesc {
    pub(crate) uid: String,
    pub(crate) name: String,
    /// The HAL's object id now: a replugged device usually comes back under a new one.
    pub(crate) object_id: u32,
    pub(crate) transport: Transport,
    pub(crate) input: StreamLayout,
    pub(crate) output: StreamLayout,
    pub(crate) input_names: Vec<Option<String>>,
    pub(crate) output_names: Vec<Option<String>>,
    pub(crate) nominal_rate: f64,
    pub(crate) rates: Vec<RateRange>,
    pub(crate) buffer_frames: u32,
    pub(crate) buffer_range: FrameRange,
    /// The largest cycle, when cycles vary in size.
    pub(crate) variable_buffer: Option<u32>,
    pub(crate) input_latency: Latency,
    pub(crate) output_latency: Latency,
    pub(crate) clock_sources: Vec<ClockSource>,
    /// The process holding the device in hog mode, if any.
    pub(crate) hog_pid: Option<i32>,
}

/// Microphone consent as the source reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Consent {
    Authorized,
    Denied,
    NotDetermined,
}

/// The per-cycle closure an IOProc runs.
pub(crate) type CycleFn = Box<dyn FnMut(Cycle<'_, '_>) + Send>;
/// Called on the HAL's notification thread when a watched property changes; must not
/// block.
pub(crate) type Notify = Arc<dyn Fn(Notice) + Send + Sync>;

/// A property change a running stream cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Notice {
    Device(DeviceProperty),
    /// coreaudiod restarted: every device object is gone.
    ServiceRestarted,
}

/// A started IOProc; dropping it stops it, after which its closure is never called again.
pub(crate) trait Running: Send + Sync {
    fn stats(&self) -> IoStats;
    fn wait_first_cycle(&self, timeout: Duration) -> Result<(), Error>;
}

/// The HAL, as the adapter uses it.
pub(crate) trait HalSource: Send + Sync {
    /// UIDs of every device.
    fn uids(&self) -> Result<Vec<String>, Error>;
    /// The default device's UID for `scope`.
    fn default_uid(&self, scope: Scope) -> Result<Option<String>, Error>;
    /// The device's object id, `None` when it is not there. Cheap: reads one property.
    fn object_id(&self, uid: &str) -> Result<Option<u32>, Error>;
    /// Everything about the device; `Error::NotFound` when it is not there.
    fn describe(&self, uid: &str) -> Result<DevDesc, Error>;
    fn set_rate(&self, uid: &str, hz: f64) -> Result<(), Error>;
    fn set_buffer(&self, uid: &str, frames: u32) -> Result<(), Error>;
    /// Starts an IOProc on the device with `f`, and listeners that call `notify`.
    fn start(
        &self,
        uid: &str,
        config: IoConfig,
        f: CycleFn,
        notify: Notify,
    ) -> Result<Box<dyn Running>, Error>;
    fn microphone(&self) -> Consent;
    /// Asks for the microphone, waiting up to `timeout`.
    fn request_microphone(&self, timeout: Duration) -> Consent;
}

#[cfg(target_os = "macos")]
pub(crate) use mac::CahalSource;

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use cahal::{Device, IoProc, Listener, System, SystemProperty};

    /// The real HAL, through `cahal`.
    #[derive(Debug, Default)]
    pub(crate) struct CahalSource;

    fn device(uid: &str) -> Result<Device, Error> {
        System::device_by_uid(uid)?.ok_or(Error::NotFound)
    }

    struct Run {
        io: Option<IoProc>,
        _listeners: Vec<Listener>,
    }

    impl Running for Run {
        fn stats(&self) -> IoStats {
            self.io.as_ref().map(IoProc::stats).unwrap_or_default()
        }
        fn wait_first_cycle(&self, timeout: Duration) -> Result<(), Error> {
            match &self.io {
                Some(io) => io.wait_first_cycle(timeout),
                None => Err(Error::StartTimedOut),
            }
        }
    }

    impl Drop for Run {
        fn drop(&mut self) {
            if let Some(io) = self.io.take() {
                let _ = io.stop();
            }
        }
    }

    impl HalSource for CahalSource {
        fn uids(&self) -> Result<Vec<String>, Error> {
            Ok(System::devices()?
                .into_iter()
                .filter_map(|d| d.uid().ok())
                .collect())
        }

        fn default_uid(&self, scope: Scope) -> Result<Option<String>, Error> {
            System::default_device(scope)?.map(Device::uid).transpose()
        }

        fn object_id(&self, uid: &str) -> Result<Option<u32>, Error> {
            Ok(System::device_by_uid(uid)?.map(Device::object_id))
        }

        fn describe(&self, uid: &str) -> Result<DevDesc, Error> {
            let d = device(uid)?;
            let info = d.info()?;
            let names = |s: Scope, l: &StreamLayout| {
                if l.channels() == 0 {
                    Vec::new()
                } else {
                    d.channel_names(s).unwrap_or_default()
                }
            };
            Ok(DevDesc {
                input_names: names(Scope::Input, &info.input),
                output_names: names(Scope::Output, &info.output),
                uid: info.uid,
                name: info.name,
                object_id: d.object_id(),
                transport: info.transport,
                input: info.input,
                output: info.output,
                nominal_rate: d.nominal_rate()?,
                rates: d.available_rates().unwrap_or_default(),
                buffer_frames: d.buffer_frames()?,
                buffer_range: d.buffer_frame_range()?,
                variable_buffer: d.variable_buffer_frames(),
                input_latency: d.latency(Scope::Input).unwrap_or_default(),
                output_latency: d.latency(Scope::Output).unwrap_or_default(),
                clock_sources: d.clock_sources().unwrap_or_default(),
                hog_pid: d.hog_pid(),
            })
        }

        fn set_rate(&self, uid: &str, hz: f64) -> Result<(), Error> {
            device(uid)?.set_nominal_rate(hz)
        }

        fn set_buffer(&self, uid: &str, frames: u32) -> Result<(), Error> {
            device(uid)?.set_buffer_frames(frames)
        }

        fn start(
            &self,
            uid: &str,
            config: IoConfig,
            f: CycleFn,
            notify: Notify,
        ) -> Result<Box<dyn Running>, Error> {
            let d = device(uid)?;
            let n = Arc::clone(&notify);
            let device_listener = d.listen(
                &[
                    DeviceProperty::Alive,
                    DeviceProperty::NominalRate,
                    DeviceProperty::BufferFrameSize,
                    DeviceProperty::Latency,
                    DeviceProperty::StreamConfiguration,
                    DeviceProperty::ClockSource,
                    DeviceProperty::IoStoppedAbnormally,
                ],
                move |p| n(Notice::Device(p)),
            )?;
            let system_listener = System::listen(&[SystemProperty::ServiceRestarted], move |_| {
                notify(Notice::ServiceRestarted)
            })?;
            let io = d.start_io(config, f)?;
            Ok(Box::new(Run {
                io: Some(io),
                _listeners: vec![device_listener, system_listener],
            }))
        }

        fn microphone(&self) -> Consent {
            consent(cahal::consent::microphone())
        }

        fn request_microphone(&self, timeout: Duration) -> Consent {
            consent(cahal::consent::request_microphone(timeout).authorization)
        }
    }

    fn consent(a: cahal::consent::Authorization) -> Consent {
        use cahal::consent::Authorization as A;
        match a {
            A::Authorized => Consent::Authorized,
            A::Denied | A::Restricted => Consent::Denied,
            A::NotDetermined => Consent::NotDetermined,
        }
    }
}
