//! The only code that touches COM: azo's driver proxies, the static callbacks the driver
//! calls, and the host clock read in them.
//!
//! Every control call goes to the driver through azo's single-threaded apartment, one
//! thread named `ac2-asio` that created the driver and pumps its window messages: drivers
//! are COM objects that expect their calls on the thread that loaded them, and some post
//! messages to it. The callbacks run on the driver's own audio thread and reach the
//! stream through [`SLOT`].

use std::ffi::{c_long, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use azo::Host;
use azo::driver::{CallbackHandle, Driver, Metadata, Proxy};
use azo::dto::{ChannelId, Granularity};
use azo::future::{CanReportOverload, GetInternalBufferSamples};
use azo::sys::{
    Bool, Callbacks, InternalBufferInfo, MessageSelector, SampleRate, Time, TimeInfoFlags,
};

use super::backend::{AsioHost, DriverSource};
use super::engine::{RtDriver, Slot, TimeInfo, answer};
use super::open::{
    AsioCode, AsioDriver, BufferSizes, ChannelDesc, ClockDesc, DriverError, backend_err,
};
use crate::backend::{Backend, BackendKind, DeviceCaps, DeviceSelector, DuplexRequest, Presence};
use crate::error::{AudioError, Operation, Unavailability};
use crate::stream::DuplexStream;

/// Where the callbacks find the running stream. One per process: ASIO runs one driver
/// per process, and the backend opens one stream on it at a time.
static SLOT: Slot = Slot::new();

static CALLBACKS: Callbacks = Callbacks {
    buffer_switch,
    sample_rate_did_change,
    asio_message,
    buffer_switch_time_info,
};

#[allow(unsafe_code)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
}

static QPC_HZ: AtomicU64 = AtomicU64::new(0);

/// The host clock in ns: the performance counter, which Windows audio stamps with too, read
/// in user mode without a system call.
fn now_ns() -> u64 {
    let hz = QPC_HZ.load(Ordering::Relaxed);
    if hz == 0 {
        return 0;
    }
    let mut count = 0i64;
    // SAFETY: writes one i64 through a valid pointer.
    #[allow(unsafe_code)]
    unsafe {
        QueryPerformanceCounter(&mut count)
    };
    (u128::from(count.max(0) as u64) * 1_000_000_000 / u128::from(hz)) as u64
}

#[allow(unsafe_code)]
unsafe extern "system" fn buffer_switch(index: c_long, _direct: Bool) {
    let now = now_ns();
    SLOT.with(|e| {
        if let Some(e) = e {
            e.switch(index, None, now);
        }
    });
}

#[allow(unsafe_code)]
unsafe extern "system" fn buffer_switch_time_info(
    params: *mut Time,
    index: c_long,
    _direct: Bool,
) -> *mut Time {
    let now = now_ns();
    // SAFETY: the driver passes a valid ASIOTime (or null) for the duration of the call.
    let time = unsafe { params.as_ref() }.map(|t| {
        let i = &t.time_info;
        TimeInfo {
            position: i
                .flags
                .contains(TimeInfoFlags::SAMPLE_POSITION_VALID)
                .then(|| i64::from(i.sample_position)),
            system_ns: i
                .flags
                .contains(TimeInfoFlags::SYSTEM_TIME_VALID)
                .then(|| i64::from(i.system_time).max(0) as u64),
        }
    });
    SLOT.with(|e| {
        if let Some(e) = e {
            e.switch(index, time, now);
        }
    });
    params
}

#[allow(unsafe_code)]
unsafe extern "system" fn sample_rate_did_change(rate: SampleRate) {
    SLOT.with(|e| {
        if let Some(e) = e {
            e.rate_changed(rate);
        }
    });
}

#[allow(unsafe_code)]
unsafe extern "system" fn asio_message(
    selector: MessageSelector,
    value: c_long,
    _message: *const c_void,
    _opt: *const f64,
) -> c_long {
    SLOT.with(|e| answer(e, selector.0, value))
}

/// A loaded driver, reached through azo's apartment.
struct AzoDriver {
    proxy: Proxy,
}

impl AzoDriver {
    /// The driver's result code with its own message, if it gave one.
    fn error(&self, e: azo::Error) -> DriverError {
        DriverError {
            code: Some(AsioCode::from_raw(e.code().0)),
            message: self.message(),
        }
    }

    fn message(&self) -> String {
        self.proxy.last_error().to_string_lossy().trim().to_owned()
    }
}

impl AsioDriver for AzoDriver {
    fn init(&self) -> Result<(), DriverError> {
        if self.proxy.init(None) {
            Ok(())
        } else {
            Err(DriverError::text(self.message()))
        }
    }

    fn clock_sources(&self) -> Result<Vec<ClockDesc>, DriverError> {
        let c = self.proxy.clock_sources().map_err(|e| self.error(e))?;
        Ok(c.into_iter()
            .map(|c| ClockDesc {
                name: c.name.to_string_lossy().into_owned(),
                current: c.is_current,
            })
            .collect())
    }

    fn control_panel(&self) -> Result<(), DriverError> {
        self.proxy.open_control_panel().map_err(|e| self.error(e))
    }

    fn channel_counts(&self) -> Result<(u16, u16), DriverError> {
        let c = self.proxy.channel_counts().map_err(|e| self.error(e))?;
        let n = |v: c_long| u16::try_from(v.max(0)).unwrap_or(u16::MAX);
        Ok((n(c.in_), n(c.out)))
    }

    fn channel(&self, input: bool, index: u16) -> Result<ChannelDesc, DriverError> {
        let info = self
            .proxy
            .channel_info(ChannelId {
                input,
                index: c_long::from(index),
            })
            .map_err(|e| self.error(e))?;
        Ok(ChannelDesc {
            name: info.name.to_string_lossy().trim().to_owned(),
            sample_type: info.sample_type.0,
        })
    }

    fn buffer_sizes(&self) -> Result<BufferSizes, DriverError> {
        let b = self.proxy.buffer_size().map_err(|e| self.error(e))?;
        Ok(BufferSizes {
            min: b.min,
            max: b.max,
            preferred: b.preferred,
            granularity: match b.granularity {
                None => 0,
                Some(Granularity::Exponential) => -1,
                Some(Granularity::Linear { step }) => step.get(),
            },
        })
    }

    fn can_sample_rate(&self, hz: f64) -> bool {
        self.proxy.can_sample_rate(hz).is_ok()
    }

    fn sample_rate(&self) -> Result<f64, DriverError> {
        self.proxy.get_sample_rate().map_err(|e| self.error(e))
    }

    fn set_sample_rate(&self, hz: f64) -> Result<(), DriverError> {
        self.proxy.set_sample_rate(hz).map_err(|e| self.error(e))
    }

    fn latencies(&self) -> Result<(u32, u32), DriverError> {
        let l = self.proxy.latencies().map_err(|e| self.error(e))?;
        Ok((l.in_.max(0) as u32, l.out.max(0) as u32))
    }

    fn can_report_overload(&self) -> bool {
        self.proxy.future::<CanReportOverload>(&mut ()).is_ok()
    }

    fn internal_buffering(&self) -> Option<(u32, u32)> {
        let mut info = InternalBufferInfo {
            input_samples: 0,
            output_samples: 0,
        };
        self.proxy
            .future::<GetInternalBufferSamples>(&mut info)
            .ok()?;
        Some((
            info.input_samples.max(0) as u32,
            info.output_samples.max(0) as u32,
        ))
    }

    fn create_buffers(
        &self,
        channels: &[(bool, u16)],
        frames: u32,
    ) -> Result<Vec<[*mut u8; 2]>, DriverError> {
        let ids: Vec<ChannelId> = channels
            .iter()
            .map(|&(input, index)| ChannelId {
                input,
                index: c_long::from(index),
            })
            .collect();
        let frames = c_long::try_from(frames).map_err(|e| DriverError::text(e.to_string()))?;
        // SAFETY: CALLBACKS is static; the stream guard disposes the buffers before the
        // stream's pointers into them are dropped.
        #[allow(unsafe_code)]
        let bufs = unsafe { self.proxy.create_buffers(ids, frames, &raw const CALLBACKS) };
        bufs.map(|it| it.map(|[a, b]| [a.cast::<u8>(), b.cast::<u8>()]).collect())
            .map_err(|e| self.error(e))
    }

    fn start(&self) -> Result<(), DriverError> {
        self.proxy.start().map_err(|e| self.error(e))
    }

    fn stop(&self) -> Result<(), DriverError> {
        self.proxy.stop().map_err(|e| self.error(e))
    }

    fn dispose_buffers(&self) -> Result<(), DriverError> {
        self.proxy.dispose_buffers().map_err(|e| self.error(e))
    }

    fn rt(&self) -> Box<dyn RtDriver> {
        // SAFETY: the handle lives in the stream's engine, which the callbacks stop using
        // (the slot is cleared) before the stream guard drops this driver.
        #[allow(unsafe_code)]
        let h = unsafe { self.proxy.callback_handle() };
        Box::new(AzoRt(h))
    }
}

struct AzoRt(CallbackHandle);

impl RtDriver for AzoRt {
    fn sample_position(&self) -> Option<i64> {
        self.0.sample_position().ok().map(|p| p.position)
    }

    fn output_ready(&self) -> bool {
        self.0.output_ready().is_ok()
    }
}

/// The drivers listed under `HKLM\SOFTWARE\ASIO`, created in the `ac2-asio` apartment.
#[derive(Debug, Default)]
struct AzoSource {
    host: Mutex<Option<Arc<Host>>>,
}

impl AzoSource {
    fn host(&self) -> Result<Arc<Host>, AudioError> {
        let mut h = self
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(h) = h.as_ref() {
            return Ok(Arc::clone(h));
        }
        let host = Host::with_thread_name("ac2-asio")
            .map_err(|e| backend_err(Operation::Open, format!("ac2-asio thread: {e}")))?;
        *h = Some(Arc::clone(&host));
        Ok(host)
    }
}

/// Installed drivers, in registry order.
fn installed() -> Result<Vec<Metadata>, AudioError> {
    match Metadata::enumerate() {
        Ok(m) if !m.is_empty() => Ok(m),
        _ => Err(AudioError::Unavailable {
            backend: BackendKind::Asio,
            reason: Unavailability::NoAsioDriver,
        }),
    }
}

impl DriverSource for AzoSource {
    fn installed(&self) -> Result<Vec<String>, AudioError> {
        Ok(installed()?
            .iter()
            .map(|m| m.description.to_string_lossy())
            .collect())
    }

    fn create(&self, name: &str) -> Result<Arc<dyn AsioDriver>, AudioError> {
        let metas = installed()?;
        let meta = metas
            .iter()
            .find(|m| m.description.to_string_lossy() == name)
            .ok_or_else(|| super::open::not_found(&crate::DeviceId(name.into())))?;
        let proxy = self
            .host()?
            .create_driver(&meta.clsid)
            .map_err(|e| backend_err(Operation::Open, format!("{name}: {e}")))?;
        Ok(Arc::new(AzoDriver { proxy }))
    }

    fn slot(&self) -> &'static Slot {
        &SLOT
    }
}

/// The ASIO backend: installed ASIO drivers, one per device; input and output on the same
/// driver.
#[derive(Debug, Clone)]
pub struct AsioBackend {
    host: Arc<AsioHost<AzoSource>>,
}

impl Default for AsioBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl AsioBackend {
    /// A backend; no driver is loaded until one is listed or opened.
    pub fn new() -> Self {
        let mut hz = 0i64;
        // SAFETY: writes one i64 through a valid pointer.
        #[allow(unsafe_code)]
        unsafe {
            QueryPerformanceFrequency(&mut hz)
        };
        QPC_HZ.store(hz.max(0) as u64, Ordering::Relaxed);
        Self {
            host: Arc::new(AsioHost::new(AzoSource::default())),
        }
    }
}

impl Backend for AsioBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Asio
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        self.host.enumerate()
    }

    fn probe(&self, device: &DeviceSelector) -> Presence {
        self.host.probe(device)
    }

    fn open(&self, request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        self.host.open(request)
    }

    fn control_panel(&self, device: &DeviceSelector) -> Result<(), AudioError> {
        self.host.control_panel(device)
    }
}
