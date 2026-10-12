//! Listing and opening one ASIO driver, written against [`AsioDriver`] so the same code runs
//! on a real driver (through `azo`) and on the scripted driver of the tests.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::engine::{Channel, Engine, Rt, RtDriver, Slot};
use super::format::Format;
use crate::backend::{
    BackendKind, ClockRelation, ClockSource, Delivery, DeviceCaps, DeviceId, Direction,
    DirectionCaps, DriverBuffering, DuplexRequest, FrameRange, IndexExactness, Negotiated,
    RateRange, StaticLatency,
};
use crate::error::{AudioError, Operation, Unavailability, Unsupported};
use crate::stream::{DuplexStream, Plumbing, StreamParts};

/// One channel as the driver describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChannelDesc {
    /// The driver's name for it ("Analog 1").
    pub(crate) name: String,
    /// `ASIOSampleType`.
    pub(crate) sample_type: i32,
}

/// `getBufferSize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BufferSizes {
    pub(crate) min: i32,
    pub(crate) max: i32,
    pub(crate) preferred: i32,
    /// Step between sizes; -1 for powers of two, 0 when only min and max are offered.
    pub(crate) granularity: i32,
}

impl BufferSizes {
    fn accepts(&self, frames: u32) -> bool {
        let Ok(f) = i32::try_from(frames) else {
            return false;
        };
        if f < self.min || f > self.max {
            return false;
        }
        match self.granularity {
            -1 => frames.is_power_of_two(),
            0 => f == self.min || f == self.max || f == self.preferred,
            g if g > 0 => (f - self.min) % g == 0,
            _ => f == self.preferred,
        }
    }

    fn describe(&self) -> String {
        match self.granularity {
            _ if self.min == self.max => format!("buffer fixed at {} frames", self.min),
            -1 => format!("buffer {}–{} frames in powers of two", self.min, self.max),
            g if g > 0 => format!("buffer {}–{} frames in steps of {g}", self.min, self.max),
            _ => format!("buffer {} or {} frames", self.min, self.max),
        }
    }
}

/// An `ASIOError` result code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AsioCode {
    /// `ASE_NotPresent`: the hardware is not there, or the call is not supported.
    NotPresent,
    /// `ASE_HWMalfunction`.
    HwMalfunction,
    /// `ASE_InvalidParameter`.
    InvalidParameter,
    /// `ASE_InvalidMode`.
    InvalidMode,
    /// `ASE_SPNotAdvancing`.
    SpNotAdvancing,
    /// `ASE_NoClock`.
    NoClock,
    /// `ASE_NoMemory`.
    NoMemory,
    /// Any other code.
    Other(i32),
}

impl AsioCode {
    pub(crate) fn from_raw(code: i32) -> Self {
        match code {
            -1000 => Self::NotPresent,
            -999 => Self::HwMalfunction,
            -998 => Self::InvalidParameter,
            -997 => Self::InvalidMode,
            -996 => Self::SpNotAdvancing,
            -995 => Self::NoClock,
            -994 => Self::NoMemory,
            c => Self::Other(c),
        }
    }

    fn name(self) -> String {
        match self {
            Self::NotPresent => "ASE_NotPresent".into(),
            Self::HwMalfunction => "ASE_HWMalfunction".into(),
            Self::InvalidParameter => "ASE_InvalidParameter".into(),
            Self::InvalidMode => "ASE_InvalidMode".into(),
            Self::SpNotAdvancing => "ASE_SPNotAdvancing".into(),
            Self::NoClock => "ASE_NoClock".into(),
            Self::NoMemory => "ASE_NoMemory".into(),
            Self::Other(c) => format!("ASIO error {c}"),
        }
    }
}

/// What a driver call returned on failure: its result code, when the call has one (`init`
/// returns only a yes or no), and the driver's own words (`getErrorMessage`), if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DriverError {
    pub(crate) code: Option<AsioCode>,
    pub(crate) message: String,
}

impl DriverError {
    /// A failure with no code, described by `message`.
    pub(crate) fn text(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = self.message.trim();
        match (self.code, msg.is_empty()) {
            (Some(c), true) => f.write_str(&c.name()),
            (Some(c), false) => write!(f, "{msg} ({})", c.name()),
            (None, true) => f.write_str("the driver gave no reason"),
            (None, false) => f.write_str(msg),
        }
    }
}

impl From<DriverError> for String {
    fn from(e: DriverError) -> Self {
        e.to_string()
    }
}

/// The step of loading or starting a driver that failed, for [`refusal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// `init`.
    Init,
    /// `createBuffers`.
    CreateBuffers,
    /// `start`.
    Start,
    /// Anything else, during `operation`.
    Other(Operation),
}

/// The error for a failed driver call, typed from the driver's result code alone (never
/// from its message text), so the operator reads what to do:
///
/// - `ASE_NotPresent` from any call: the interface is not there.
/// - `init` refused with a message from the driver: the driver could not reach its
///   hardware and says why (an unplugged interface is the common case). Refused without a
///   word: the driver is held elsewhere — single-client drivers refuse a second program
///   this way and have nothing to report about their hardware.
/// - `createBuffers` or `start` with `ASE_HWMalfunction` or `ASE_NoMemory`: the driver
///   loaded, but another program holds the hardware stream.
/// - Anything else: the driver's words, with the step.
pub(crate) fn refusal(driver: &str, step: Step, e: DriverError) -> AudioError {
    let unavailable = |reason| AudioError::Unavailable {
        backend: BackendKind::Asio,
        reason,
    };
    let not_present = |e: &DriverError| {
        unavailable(Unavailability::AsioHardwareNotPresent {
            driver: driver.into(),
            detail: e.message.trim().into(),
        })
    };
    let in_use = || {
        unavailable(Unavailability::AsioDriverInUse {
            driver: driver.into(),
        })
    };
    match (step, e.code) {
        (_, Some(AsioCode::NotPresent)) => not_present(&e),
        (Step::Init, None) if e.message.trim().is_empty() => in_use(),
        (Step::Init, _) => not_present(&e),
        (Step::CreateBuffers | Step::Start, Some(AsioCode::HwMalfunction | AsioCode::NoMemory)) => {
            in_use()
        }
        (Step::CreateBuffers, _) => backend_err(
            Operation::Open,
            format!("{driver} refused to make buffers: {e}"),
        ),
        (Step::Start, _) => backend_err(Operation::Start, format!("{driver}: {e}")),
        (Step::Other(op), _) => backend_err(op, format!("{driver}: {e}")),
    }
}

/// Initialises the driver `drv`, loaded as `name`.
pub(crate) fn initialise(drv: &dyn AsioDriver, name: &str) -> Result<(), AudioError> {
    drv.init().map_err(|e| refusal(name, Step::Init, e))
}

/// One clock source as the driver lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClockDesc {
    /// Its name ("Internal", "Word Clock").
    pub(crate) name: String,
    /// The driver runs on it now.
    pub(crate) current: bool,
}

/// The driver's clock sources, or none when it does not list them (many drivers answer
/// `ASE_NotPresent`). Names are trimmed; an empty one is numbered.
fn clock_sources(drv: &dyn AsioDriver) -> Vec<ClockSource> {
    drv.clock_sources()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(i, c)| ClockSource {
            name: match c.name.trim() {
                "" => format!("Clock {}", i + 1),
                n => n.to_owned(),
            },
            current: c.current,
        })
        .collect()
}

/// The control-side calls into one loaded driver. Every call but [`Self::init`] needs it
/// initialised.
pub(crate) trait AsioDriver: Send + Sync {
    /// `init`; on a refusal, the driver's `getErrorMessage`.
    fn init(&self) -> Result<(), DriverError>;
    /// `getClockSources`.
    fn clock_sources(&self) -> Result<Vec<ClockDesc>, DriverError>;
    /// `controlPanel`.
    fn control_panel(&self) -> Result<(), DriverError>;
    /// `getChannels`: inputs, outputs.
    fn channel_counts(&self) -> Result<(u16, u16), DriverError>;
    /// `getChannelInfo`.
    fn channel(&self, input: bool, index: u16) -> Result<ChannelDesc, DriverError>;
    /// `getBufferSize`.
    fn buffer_sizes(&self) -> Result<BufferSizes, DriverError>;
    /// `canSampleRate`.
    fn can_sample_rate(&self, hz: f64) -> bool;
    /// `getSampleRate`.
    fn sample_rate(&self) -> Result<f64, DriverError>;
    /// `setSampleRate`.
    fn set_sample_rate(&self, hz: f64) -> Result<(), DriverError>;
    /// `getLatencies`: input, output, in frames.
    fn latencies(&self) -> Result<(u32, u32), DriverError>;
    /// `future(kAsioGetInternalBufferSamples)` (ASIO 2.3): the driver's own buffering,
    /// input, output, in frames, valid once buffers exist. `None` when the driver does not
    /// answer (`ASE_NotPresent` from drivers before 2.3, or any other refusal).
    fn internal_buffering(&self) -> Option<(u32, u32)>;
    /// `future(kAsioCanReportOverload)`.
    fn can_report_overload(&self) -> bool;
    /// `createBuffers` for `channels` (input?, index) in order, with this backend's
    /// callbacks; the two halves of each channel's buffer, in the same order.
    fn create_buffers(
        &self,
        channels: &[(bool, u16)],
        frames: u32,
    ) -> Result<Vec<[*mut u8; 2]>, DriverError>;
    /// `start`.
    fn start(&self) -> Result<(), DriverError>;
    /// `stop`.
    fn stop(&self) -> Result<(), DriverError>;
    /// `disposeBuffers`.
    fn dispose_buffers(&self) -> Result<(), DriverError>;
    /// The calls a switch makes, for the callback's own use.
    fn rt(&self) -> Box<dyn RtDriver>;
}

/// Rates offered to `canSampleRate` when listing a driver.
pub(crate) const STANDARD_RATES: [u32; 11] = [
    22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000, 16_000,
];

pub(crate) fn backend_err(operation: Operation, detail: impl Into<String>) -> AudioError {
    AudioError::Backend {
        backend: BackendKind::Asio,
        operation,
        detail: detail.into(),
    }
}

/// The driver's channel names for one direction, or `None` when they cannot tell the
/// channels apart (empty or repeated names): the channels are then numbered, as on hosts
/// that name none.
fn channel_names(descs: &[ChannelDesc]) -> Option<Vec<String>> {
    let names: Vec<String> = descs.iter().map(|d| d.name.trim().to_owned()).collect();
    let mut seen = std::collections::BTreeSet::new();
    let distinct = names
        .iter()
        .all(|n| !n.is_empty() && seen.insert(n.as_str()));
    distinct.then_some(names)
}

/// Describes the loaded driver `drv`, listed as `id`. `first` marks the driver
/// [`DeviceSelector::Default`](crate::DeviceSelector::Default) opens.
pub(crate) fn device_caps(
    drv: &dyn AsioDriver,
    id: &str,
    first: bool,
) -> Result<DeviceCaps, AudioError> {
    let err = |e: DriverError| refusal(id, Step::Other(Operation::Enumerate), e);
    let (ins, outs) = drv.channel_counts().map_err(err)?;
    let sizes = drv.buffer_sizes().map_err(err)?;
    let current = drv.sample_rate().map_err(err)?;
    let current = current.round() as u32;
    let mut rates: Vec<RateRange> = STANDARD_RATES
        .iter()
        .copied()
        .filter(|&r| drv.can_sample_rate(f64::from(r)))
        .map(|r| RateRange { min: r, max: r })
        .collect();
    if !rates.iter().any(|r| r.contains(current)) {
        rates.push(RateRange {
            min: current,
            max: current,
        });
    }
    rates.sort_by_key(|r| r.min);
    let mut notes = vec![
        sizes.describe(),
        "rate and buffer size also change in the driver's control panel".into(),
    ];
    if !drv.can_report_overload() {
        notes.push("the driver cannot report overloads: 0 xruns proves nothing".into());
    }
    let mut dir = |input: bool, n: u16| -> Result<Option<DirectionCaps>, AudioError> {
        if n == 0 {
            return Ok(None);
        }
        let descs = (0..n)
            .map(|i| drv.channel(input, i))
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        let mut formats = Vec::new();
        for d in &descs {
            match Format::from_asio(d.sample_type) {
                Some(f) if !formats.contains(&f.sample_format()) => {
                    formats.push(f.sample_format());
                }
                Some(_) => {}
                None => notes.push(format!(
                    "{} {} is {}, which ac2 does not convert",
                    if input { "input" } else { "output" },
                    d.name,
                    Format::asio_name(d.sample_type)
                )),
            }
        }
        Ok(Some(DirectionCaps {
            max_channels: n,
            rates: rates.clone(),
            buffer_frames: Some(FrameRange {
                min: sizes.min.max(1) as u32,
                max: sizes.max.max(1) as u32,
            }),
            formats,
            default_rate: Some(current),
            default_buffer: Some(sizes.preferred.max(1) as u32),
            channel_names: channel_names(&descs),
            system_default: first,
        }))
    };
    let input = dir(true, ins)?;
    let output = dir(false, outs)?;
    let latency = match drv.latencies() {
        Ok((input, output)) => StaticLatency::Frames {
            input,
            output,
            driver_buffering: None,
        },
        Err(_) => StaticLatency::Unknown,
    };
    Ok(DeviceCaps {
        backend: BackendKind::Asio,
        host: "asio".into(),
        id: DeviceId(id.into()),
        name: id.into(),
        input,
        output,
        duplex_clock: ClockRelation::SingleCallback,
        index: IndexExactness::Exact,
        latency,
        clock_sources: clock_sources(drv),
        notes,
    })
}

/// A driver listed by name only: loading it would unload the one this process holds.
pub(crate) fn unloaded_caps(id: &str, held: &str) -> DeviceCaps {
    DeviceCaps {
        backend: BackendKind::Asio,
        host: "asio".into(),
        id: DeviceId(id.into()),
        name: id.into(),
        input: None,
        output: None,
        duplex_clock: ClockRelation::SingleCallback,
        index: IndexExactness::Exact,
        latency: StaticLatency::Unknown,
        clock_sources: Vec::new(),
        notes: vec![format!(
            "not loaded while {held} is open: a process runs one ASIO driver at a time"
        )],
    }
}

/// How long `open` waits for the first switches after `start()`, to state whether the
/// driver's position is usable before the stream is handed out.
pub(crate) fn first_switches_wait(rate: u32, frames: u32) -> Duration {
    let periods = Duration::from_secs_f64(4.0 * f64::from(frames) / f64::from(rate.max(1)));
    periods.max(Duration::from_millis(500))
}

/// Opens a stream on the loaded driver `drv`, listed as `id`, with its callbacks reaching
/// the stream through `slot`. Waits up to `first_switches` for two indexed switches, which
/// show whether the driver's position is usable. `release` runs once the stream's driver
/// buffers are disposed; on an error it is dropped unrun.
pub(crate) fn open_stream(
    drv: Arc<dyn AsioDriver>,
    slot: &'static Slot,
    id: DeviceId,
    mut request: DuplexRequest,
    first_switches: Duration,
    release: Box<dyn FnOnce() + Send>,
) -> Result<DuplexStream, AudioError> {
    request.validate()?;
    let err = |e: DriverError| refusal(&id.0, Step::Other(Operation::Open), e);
    let (ins, outs) = drv.channel_counts().map_err(err)?;
    if let Some(&ch) = request.input_map.iter().find(|&&c| c >= ins) {
        return Err(Unsupported::InputChannel {
            channel: ch,
            available: ins,
        }
        .into());
    }
    if request.output_channels > outs {
        return Err(Unsupported::OutputChannels {
            requested: request.output_channels,
            available: outs,
        }
        .into());
    }

    let current = drv.sample_rate().map_err(err)?;
    let rate = request.sample_rate.unwrap_or(current.round() as u32);
    if f64::from(rate) != current {
        if !drv.can_sample_rate(f64::from(rate)) {
            let offered: Vec<String> = STANDARD_RATES
                .iter()
                .filter(|&&r| drv.can_sample_rate(f64::from(r)))
                .map(|r| format!("{r} Hz"))
                .collect();
            return Err(Unsupported::SampleRate {
                requested: rate,
                offered: offered.join(", "),
            }
            .into());
        }
        drv.set_sample_rate(f64::from(rate)).map_err(err)?;
    }
    let sizes = drv.buffer_sizes().map_err(err)?;
    // The driver's preferred size is the one the operator set in its control panel.
    let frames = request
        .buffer_frames
        .unwrap_or(sizes.preferred.max(1) as u32);
    if !sizes.accepts(frames) {
        return Err(Unsupported::BufferFrames {
            requested: frames,
            fixed: (sizes.min == sizes.max).then_some(sizes.min.max(1) as u32),
        }
        .into());
    }

    // Buffers only for the channels used: each device input once, however often the map
    // repeats it, and the stream's outputs.
    let mut device_inputs: Vec<u16> = request.input_map.clone();
    device_inputs.sort_unstable();
    device_inputs.dedup();
    let mut wanted: Vec<(bool, u16)> = device_inputs.iter().map(|&c| (true, c)).collect();
    wanted.extend((0..request.output_channels).map(|c| (false, c)));
    let mut formats = Vec::with_capacity(wanted.len());
    for &(input, ch) in &wanted {
        let d = drv.channel(input, ch).map_err(err)?;
        let f = Format::from_asio(d.sample_type).ok_or_else(|| {
            backend_err(
                Operation::Open,
                format!(
                    "{} {} ({}) is {}, which ac2 does not convert",
                    if input { "input" } else { "output" },
                    ch + 1,
                    d.name,
                    Format::asio_name(d.sample_type)
                ),
            )
        })?;
        formats.push(f);
    }

    let plumbing = Plumbing::new(&mut request, rate, frames as usize);
    let bufs = drv
        .create_buffers(&wanted, frames)
        .map_err(|e| refusal(&id.0, Step::CreateBuffers, e))?;
    if bufs.len() != wanted.len() {
        let _ = drv.dispose_buffers();
        return Err(backend_err(
            Operation::Open,
            format!(
                "driver returned {} buffers for {} channels",
                bufs.len(),
                wanted.len()
            ),
        ));
    }
    let channel = |i: usize| Channel {
        bufs: bufs[i],
        format: formats[i],
    };
    let by_device: BTreeMap<u16, usize> = device_inputs
        .iter()
        .enumerate()
        .map(|(i, &c)| (c, i))
        .collect();
    let inputs: Vec<Channel> = request
        .input_map
        .iter()
        .map(|c| channel(by_device[c]))
        .collect();
    let outputs: Vec<Channel> = (device_inputs.len()..wanted.len()).map(channel).collect();
    let input_format = inputs[0].format.sample_format();
    let output_format = outputs.first().map(|c| c.format.sample_format());
    let latencies = drv.latencies().unwrap_or((0, 0));
    // Asked after `createBuffers`: the buffering a driver keeps may depend on the size.
    let driver_buffering = drv
        .internal_buffering()
        .map(|(input, output)| DriverBuffering { input, output });
    let clock_source = clock_sources(&*drv)
        .into_iter()
        .find(|c| c.current)
        .map(|c| c.name);

    let events = Arc::clone(&plumbing.events);
    let engine = Arc::new(Engine::new(
        Rt::new(
            inputs,
            outputs,
            frames,
            plumbing.producer,
            plumbing.renderer,
            rate,
            Arc::clone(&events),
            drv.rt(),
        ),
        Arc::clone(&events),
        Arc::clone(&plumbing.output.shared),
    ));

    slot.install(&engine);
    if let Err(e) = drv.start() {
        slot.clear();
        let _ = drv.dispose_buffers();
        return Err(refusal(&id.0, Step::Start, e));
    }
    engine.started();
    let deadline = Instant::now() + first_switches;
    while engine.indexed() < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }

    let negotiated = Negotiated {
        backend: BackendKind::Asio,
        input_device: id.clone(),
        output_device: id,
        sample_rate: rate,
        input_channels: request.input_map.len() as u16,
        device_input_channels: ins,
        output_channels: request.output_channels,
        device_output_channels: request.output_channels,
        buffer_frames: Some(frames),
        input_format,
        output_format,
        clock: ClockRelation::SingleCallback,
        index: if engine.estimated() {
            IndexExactness::Estimated
        } else {
            IndexExactness::Exact
        },
        latency: StaticLatency::Frames {
            input: latencies.0,
            output: latencies.1,
            driver_buffering,
        },
        clock_source,
        delivery: Delivery::Device,
    };
    // Output written in one switch leaves after the other half has played, then passes
    // the driver's output latency.
    let drain_frames = 2 * u64::from(frames) + u64::from(latencies.1);
    let drain = Duration::from_nanos(drain_frames * 1_000_000_000 / u64::from(rate))
        + Duration::from_millis(10);
    let guard = AsioGuard::new(drv, slot, engine, latencies, release);
    Ok(DuplexStream::new(StreamParts {
        negotiated,
        capture: plumbing.consumer,
        output: plumbing.output,
        events,
        drain,
        guard: Box::new(guard),
        patch: None,
    }))
}

/// Keeps a started driver; dropping it stops the driver, detaches the callbacks and
/// disposes the buffers, in that order.
struct AsioGuard {
    drv: Arc<dyn AsioDriver>,
    slot: &'static Slot,
    engine: Arc<Engine>,
    watcher: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl AsioGuard {
    fn new(
        drv: Arc<dyn AsioDriver>,
        slot: &'static Slot,
        engine: Arc<Engine>,
        latencies: (u32, u32),
        release: Box<dyn FnOnce() + Send>,
    ) -> Self {
        let quit = Arc::new(AtomicBool::new(false));
        let watcher = std::thread::Builder::new()
            .name("ac2-asio-latency".into())
            .spawn({
                let (drv, engine, quit) =
                    (Arc::clone(&drv), Arc::clone(&engine), Arc::clone(&quit));
                move || watch_latency(&*drv, &engine, &quit, latencies)
            })
            .ok()
            .map(|h| (quit, h));
        Self {
            drv,
            slot,
            engine,
            watcher,
            release: Some(release),
        }
    }
}

/// Re-reads the driver's latencies when it says they changed, off the callback (the
/// message may arrive on the driver's audio thread). A real change moves the loopback
/// delay, so it is a configuration change; a message with nothing changed (some drivers
/// send one on every routing change) is not.
fn watch_latency(drv: &dyn AsioDriver, engine: &Engine, quit: &AtomicBool, initial: (u32, u32)) {
    let mut last = initial;
    while !quit.load(Ordering::Acquire) {
        std::thread::park_timeout(Duration::from_millis(100));
        if engine.take_latency_change()
            && let Ok(now) = drv.latencies()
            && now != last
        {
            last = now;
            engine.events.config_change();
        }
    }
}

impl Drop for AsioGuard {
    fn drop(&mut self) {
        if let Some((quit, h)) = self.watcher.take() {
            quit.store(true, Ordering::Release);
            h.thread().unpark();
            let _ = h.join();
        }
        self.engine.stopping();
        let _ = self.drv.stop();
        self.slot.clear();
        let _ = self.drv.dispose_buffers();
        if let Some(r) = self.release.take() {
            r();
        }
    }
}

/// Both directions name one driver: `Default` on one side follows the other, and two
/// different drivers are refused.
pub(crate) fn one_device(
    input: &crate::DeviceSelector,
    output: &crate::DeviceSelector,
) -> Result<Option<DeviceId>, AudioError> {
    use crate::DeviceSelector as S;
    match (input, output) {
        (S::Default, S::Default) => Ok(None),
        (S::Id(i), S::Default) | (S::Default, S::Id(i)) => Ok(Some(i.clone())),
        (S::Id(i), S::Id(o)) if i == o => Ok(Some(i.clone())),
        (S::Id(i), S::Id(o)) => Err(Unsupported::SeparateDevices {
            backend: BackendKind::Asio,
            input: i.to_string(),
            output: o.to_string(),
        }
        .into()),
    }
}

/// The error for a selector naming no listed driver.
pub(crate) fn not_found(id: &DeviceId) -> AudioError {
    AudioError::DeviceNotFound {
        direction: Direction::Input,
        selector: id.to_string(),
    }
}
