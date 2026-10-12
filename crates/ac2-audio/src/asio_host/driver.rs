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
use std::sync::{Arc, Mutex, MutexGuard};

use azo::driver::{CallbackHandle, Driver, Metadata, Proxy};
use azo::dto::{ChannelId, Granularity};
use azo::future::CanReportOverload;
use azo::sys::{Bool, Callbacks, MessageSelector, SampleRate, Time, TimeInfoFlags};
use azo::utils::Host;

use super::engine::{RtDriver, Slot, TimeInfo, answer};
use super::open::{
    AsioDriver, BufferSizes, ChannelDesc, backend_err, device_caps, first_switches_wait, not_found,
    one_device, open_stream, unloaded_caps,
};
use crate::backend::{
    Backend, BackendKind, DeviceCaps, DeviceId, DeviceSelector, DuplexRequest, Presence,
};
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

/// A loaded, initialised driver, reached through azo's apartment.
struct AzoDriver {
    proxy: Proxy,
}

impl AzoDriver {
    fn error(&self, e: azo::Error) -> String {
        let msg = self.proxy.last_error();
        let msg = msg.to_string_lossy();
        if msg.trim().is_empty() {
            e.to_string()
        } else {
            format!("{e}: {}", msg.trim())
        }
    }
}

impl AsioDriver for AzoDriver {
    fn channel_counts(&self) -> Result<(u16, u16), String> {
        let c = self.proxy.channel_counts().map_err(|e| self.error(e))?;
        let n = |v: c_long| u16::try_from(v.max(0)).unwrap_or(u16::MAX);
        Ok((n(c.in_), n(c.out)))
    }

    fn channel(&self, input: bool, index: u16) -> Result<ChannelDesc, String> {
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

    fn buffer_sizes(&self) -> Result<BufferSizes, String> {
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

    fn sample_rate(&self) -> Result<f64, String> {
        self.proxy.get_sample_rate().map_err(|e| self.error(e))
    }

    fn set_sample_rate(&self, hz: f64) -> Result<(), String> {
        self.proxy.set_sample_rate(hz).map_err(|e| self.error(e))
    }

    fn latencies(&self) -> Result<(u32, u32), String> {
        let l = self.proxy.latencies().map_err(|e| self.error(e))?;
        Ok((l.in_.max(0) as u32, l.out.max(0) as u32))
    }

    fn can_report_overload(&self) -> bool {
        self.proxy.future::<CanReportOverload>(&mut ()).is_ok()
    }

    fn create_buffers(
        &self,
        channels: &[(bool, u16)],
        frames: u32,
    ) -> Result<Vec<[*mut u8; 2]>, String> {
        let ids: Vec<ChannelId> = channels
            .iter()
            .map(|&(input, index)| ChannelId {
                input,
                index: c_long::from(index),
            })
            .collect();
        let frames = c_long::try_from(frames).map_err(|e| e.to_string())?;
        // SAFETY: CALLBACKS is static; the stream guard disposes the buffers before the
        // stream's pointers into them are dropped.
        #[allow(unsafe_code)]
        let bufs = unsafe { self.proxy.create_buffers(ids, frames, &raw const CALLBACKS) };
        bufs.map(|it| it.map(|[a, b]| [a.cast::<u8>(), b.cast::<u8>()]).collect())
            .map_err(|e| self.error(e))
    }

    fn start(&self) -> Result<(), String> {
        self.proxy.start().map_err(|e| self.error(e))
    }

    fn stop(&self) -> Result<(), String> {
        self.proxy.stop().map_err(|e| self.error(e))
    }

    fn dispose_buffers(&self) -> Result<(), String> {
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

/// The driver this process holds, and what it listed as when opened.
#[derive(Debug)]
struct Held {
    id: DeviceId,
    caps: Option<DeviceCaps>,
}

#[derive(Debug, Default)]
struct Inner {
    host: Mutex<Option<Arc<Host>>>,
    held: Mutex<Option<Held>>,
    /// Serialises loading drivers: listing loads each one in turn, and a process holds
    /// one driver at a time.
    loading: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The ASIO backend: installed ASIO drivers, one per device; input and output on the same
/// driver.
#[derive(Debug, Clone, Default)]
pub struct AsioBackend {
    inner: Arc<Inner>,
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
        Self::default()
    }

    fn host(&self) -> Result<Arc<Host>, AudioError> {
        let mut h = lock(&self.inner.host);
        if let Some(h) = h.as_ref() {
            return Ok(Arc::clone(h));
        }
        let host = Host::with_thread_name("ac2-asio")
            .map_err(|e| backend_err(Operation::Open, format!("ac2-asio thread: {e}")))?;
        *h = Some(Arc::clone(&host));
        Ok(host)
    }

    fn load(&self, meta: &Metadata) -> Result<AzoDriver, AudioError> {
        let name = meta.description.to_string_lossy();
        let proxy = self
            .host()?
            .create_driver(meta.clsid)
            .map_err(|e| backend_err(Operation::Open, format!("{name}: {e}")))?;
        if !proxy.init(None) {
            let msg = proxy.last_error();
            return Err(backend_err(
                Operation::Open,
                format!("{name} did not initialise: {}", msg.to_string_lossy()),
            ));
        }
        Ok(AzoDriver { proxy })
    }
}

/// Installed drivers, from `HKLM\SOFTWARE\ASIO`, in registry order.
fn installed() -> Result<Vec<Metadata>, AudioError> {
    match Metadata::enumerate() {
        Ok(m) if !m.is_empty() => Ok(m),
        _ => Err(AudioError::Unavailable {
            backend: BackendKind::Asio,
            reason: Unavailability::NoAsioDriver,
        }),
    }
}

fn id_of(m: &Metadata) -> String {
    m.description.to_string_lossy()
}

impl Backend for AsioBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Asio
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let metas = installed()?;
        let _loading = lock(&self.inner.loading);
        let held = lock(&self.inner.held)
            .as_ref()
            .map(|h| (h.id.clone(), h.caps.clone()));
        let mut out = Vec::with_capacity(metas.len());
        for (i, m) in metas.iter().enumerate() {
            let id = id_of(m);
            let caps = match &held {
                Some((h, Some(caps))) if h.0 == id => caps.clone(),
                Some((h, _)) => unloaded_caps(&id, &h.0),
                None => match self.load(m) {
                    Ok(drv) => device_caps(&drv, &id, i == 0)?,
                    Err(e) => {
                        let mut c = unloaded_caps(&id, "");
                        c.notes = vec![e.to_string()];
                        c
                    }
                },
            };
            out.push(caps);
        }
        Ok(out)
    }

    /// The registry only: loading a driver to look at it could take it from its control
    /// panel or another program.
    fn probe(&self, device: &DeviceSelector) -> Presence {
        let present = match (Metadata::enumerate(), device) {
            (Ok(m), DeviceSelector::Default) => !m.is_empty(),
            (Ok(m), DeviceSelector::Id(id)) => m.iter().any(|m| id_of(m) == id.0),
            (Err(_), _) => false,
        };
        if present {
            Presence::Present { generation: 0 }
        } else {
            Presence::Absent
        }
    }

    fn open(&self, request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        request.validate()?;
        let want = one_device(&request.input_device, &request.output_device)?;
        let metas = installed()?;
        let (index, meta) = match &want {
            None => (0, &metas[0]),
            Some(id) => metas
                .iter()
                .enumerate()
                .find(|(_, m)| id_of(m) == id.0)
                .ok_or_else(|| not_found(id))?,
        };
        let id = DeviceId(id_of(meta));
        let _loading = lock(&self.inner.loading);
        if let Some(h) = lock(&self.inner.held).as_ref() {
            return Err(AudioError::Unavailable {
                backend: BackendKind::Asio,
                reason: Unavailability::Host(format!(
                    "{} is open in this process; ASIO runs one driver at a time",
                    h.id
                )),
            });
        }
        let drv = Arc::new(self.load(meta)?);
        let caps = device_caps(&*drv, &id.0, index == 0).ok();
        let (rate, frames) =
            caps.as_ref()
                .and_then(|c| c.input.as_ref())
                .map_or((48_000, 1024), |d| {
                    (
                        d.default_rate.unwrap_or(48_000),
                        d.default_buffer.unwrap_or(1024),
                    )
                });
        let wait = first_switches_wait(
            request.sample_rate.unwrap_or(rate),
            request.buffer_frames.unwrap_or(frames),
        );
        *lock(&self.inner.held) = Some(Held {
            id: id.clone(),
            caps,
        });
        let inner = Arc::clone(&self.inner);
        let release = Box::new(move || *lock(&inner.held) = None);
        open_stream(drv, &SLOT, id, request, wait, release).inspect_err(|_| {
            *lock(&self.inner.held) = None;
        })
    }
}
