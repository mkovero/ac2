//! The ASIO [`Backend`]: which driver this process holds, listing, opening and the control
//! panel, written against [`DriverSource`] so the same code runs on the installed drivers
//! (through `azo`) and on the scripted drivers of the tests.
//!
//! A process runs one ASIO driver at a time. The driver is held by the open stream, or, with
//! no stream open, by the last control panel call: a panel that is a window of the driver's
//! own closes when the driver is unloaded, so it stays loaded until a stream or a listing
//! needs the process's one driver.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use super::engine::Slot;
use super::open::{
    AsioDriver, Step, device_caps, first_switches_wait, initialise, not_found, one_device,
    open_stream, refusal, unloaded_caps,
};
use crate::backend::{
    Backend, BackendKind, DeviceCaps, DeviceId, DeviceSelector, DuplexRequest, Presence,
};
use crate::error::{AudioError, Operation, Unavailability};
use crate::stream::DuplexStream;

/// Where drivers come from: the registry and COM on Windows, a script in the tests.
pub(crate) trait DriverSource: Send + Sync + fmt::Debug {
    /// The installed drivers' names, in registry order; `Unavailable(NoAsioDriver)` when
    /// there are none.
    fn installed(&self) -> Result<Vec<String>, AudioError>;
    /// Creates an instance of driver `name`, not yet initialised.
    fn create(&self, name: &str) -> Result<Arc<dyn AsioDriver>, AudioError>;
    /// Where this source's driver callbacks find the running stream.
    fn slot(&self) -> &'static Slot;
}

/// The loaded driver and what holds it.
struct Loaded {
    id: DeviceId,
    drv: Arc<dyn AsioDriver>,
    /// How it listed when loaded (a stream's driver cannot be asked again while it runs).
    caps: Option<DeviceCaps>,
    /// A stream runs on it; otherwise only a control panel holds it.
    streaming: bool,
}

#[derive(Default)]
struct Inner {
    loaded: Mutex<Option<Loaded>>,
    /// Serialises loading drivers: listing loads each one in turn.
    loading: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The ASIO backend over the drivers of `S`.
pub(crate) struct AsioHost<S> {
    source: S,
    inner: Arc<Inner>,
}

impl<S: fmt::Debug> fmt::Debug for AsioHost<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let held = lock(&self.inner.loaded)
            .as_ref()
            .map(|l| (l.id.clone(), l.streaming));
        f.debug_struct("AsioHost")
            .field("source", &self.source)
            .field("held", &held)
            .finish()
    }
}

fn busy_here(held: &DeviceId) -> AudioError {
    AudioError::Unavailable {
        backend: BackendKind::Asio,
        reason: Unavailability::Host(format!(
            "{held} is open in this process; ASIO runs one driver at a time"
        )),
    }
}

impl<S: DriverSource> AsioHost<S> {
    pub(crate) fn new(source: S) -> Self {
        Self {
            source,
            inner: Arc::new(Inner::default()),
        }
    }

    /// Creates and initialises driver `name`.
    fn load(&self, name: &str) -> Result<Arc<dyn AsioDriver>, AudioError> {
        let drv = self.source.create(name)?;
        initialise(&*drv, name)?;
        Ok(drv)
    }

    /// The installed driver `device` names: its index and name.
    fn resolve(&self, device: Option<&DeviceId>) -> Result<(usize, String), AudioError> {
        let names = self.source.installed()?;
        match device {
            None => Ok((0, names[0].clone())),
            Some(id) => names
                .iter()
                .position(|n| *n == id.0)
                .map(|i| (i, names[i].clone()))
                .ok_or_else(|| not_found(id)),
        }
    }
}

impl<S: DriverSource> Backend for AsioHost<S> {
    fn kind(&self) -> BackendKind {
        BackendKind::Asio
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let names = self.source.installed()?;
        let _loading = lock(&self.inner.loading);
        let mut loaded = lock(&self.inner.loaded);
        if let Some(l) = loaded.as_mut()
            && l.caps.is_none()
            && !l.streaming
        {
            l.caps = device_caps(&*l.drv, &l.id.0, names.first() == Some(&l.id.0)).ok();
        }
        let held = loaded.as_ref().map(|l| (l.id.clone(), l.caps.clone()));
        drop(loaded);
        let mut out = Vec::with_capacity(names.len());
        for (i, id) in names.iter().enumerate() {
            let caps = match &held {
                Some((h, Some(caps))) if h.0 == *id => caps.clone(),
                Some((h, _)) => unloaded_caps(id, &h.0),
                None => match self.load(id) {
                    Ok(drv) => device_caps(&*drv, id, i == 0)?,
                    Err(e) => {
                        let mut c = unloaded_caps(id, "");
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
        let present = match (self.source.installed(), device) {
            (Ok(names), DeviceSelector::Default) => !names.is_empty(),
            (Ok(names), DeviceSelector::Id(id)) => names.contains(&id.0),
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
        // An ASIO driver's channels are all physical: nothing else can route into it.
        crate::backend::check_spare_inputs(crate::BackendKind::Asio, &request, u16::MAX, 0)?;
        let want = one_device(&request.input_device, &request.output_device)?;
        let (index, name) = self.resolve(want.as_ref())?;
        let id = DeviceId(name);
        let _loading = lock(&self.inner.loading);
        // A driver loaded for its control panel is reused when it is this one (its panel
        // stays open), and unloaded when it is another.
        let panel = {
            let mut loaded = lock(&self.inner.loaded);
            match loaded.take() {
                Some(l) if l.streaming => {
                    let held = l.id.clone();
                    *loaded = Some(l);
                    return Err(busy_here(&held));
                }
                Some(l) if l.id == id => Some(l.drv),
                _ => None,
            }
        };
        let drv = match panel {
            Some(d) => d,
            None => self.load(&id.0)?,
        };
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
        *lock(&self.inner.loaded) = Some(Loaded {
            id: id.clone(),
            drv: Arc::clone(&drv),
            caps,
            streaming: true,
        });
        let inner = Arc::clone(&self.inner);
        let release = Box::new(move || {
            let mut loaded = lock(&inner.loaded);
            if loaded.as_ref().is_some_and(|l| l.streaming) {
                *loaded = None;
            }
        });
        open_stream(drv, self.source.slot(), id, request, wait, release).inspect_err(|_| {
            *lock(&self.inner.loaded) = None;
        })
    }

    /// On the stream's own driver while a stream runs on it; otherwise the driver is loaded
    /// (unloading one loaded for an earlier panel) and stays loaded after the call.
    fn control_panel(&self, device: &DeviceSelector) -> Result<(), AudioError> {
        let want = match device {
            DeviceSelector::Default => None,
            DeviceSelector::Id(id) => Some(id.clone()),
        };
        let drv = {
            let _loading = lock(&self.inner.loading);
            let mut loaded = lock(&self.inner.loaded);
            let reuse = match (loaded.as_ref(), &want) {
                (Some(l), None) => Some(l),
                (Some(l), Some(id)) if l.id == *id => Some(l),
                (Some(l), Some(_)) if l.streaming => return Err(busy_here(&l.id)),
                _ => None,
            };
            match reuse {
                Some(l) => (l.id.clone(), Arc::clone(&l.drv)),
                None => {
                    *loaded = None;
                    drop(loaded);
                    let (_, name) = self.resolve(want.as_ref())?;
                    let drv = self.load(&name)?;
                    *lock(&self.inner.loaded) = Some(Loaded {
                        id: DeviceId(name.clone()),
                        drv: Arc::clone(&drv),
                        caps: None,
                        streaming: false,
                    });
                    (DeviceId(name), drv)
                }
            }
        };
        // No lock is held while the panel shows: a modal one returns only when it closes.
        let (id, drv) = drv;
        drv.control_panel()
            .map_err(|e| refusal(&id.0, Step::Other(Operation::Open), e))
    }
}
