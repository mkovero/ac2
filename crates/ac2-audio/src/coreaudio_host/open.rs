//! Opening a stream: devices, consent, rate and buffer, one IOProc per device, and the
//! watcher that turns property changes into stream events.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use cahal::{DeviceProperty, IoConfig, Scope};

use super::caps::{self, device_id, uid_of};
use super::engine::{Capture, Engine, Render, Shared};
use super::source::{Consent, DevDesc, HalSource, Notice, Notify, Running};
use crate::backend::{
    BackendKind, ClockRelation, Delivery, DeviceSelector, Direction, DuplexRequest, IndexExactness,
    Negotiated, SampleFormat, StaticLatency, short_buffer_frames,
};
use crate::error::{AudioError, Operation, Unavailability, Unsupported};
use crate::events::{BackendEvents, EventLatch, ResetReason};
use crate::stream::{DuplexStream, Plumbing, StreamParts};

pub(crate) fn backend_err(operation: Operation, e: impl std::fmt::Display) -> AudioError {
    AudioError::Backend {
        backend: BackendKind::CoreAudio,
        operation,
        detail: e.to_string(),
    }
}

fn unavailable(reason: Unavailability) -> AudioError {
    AudioError::Unavailable {
        backend: BackendKind::CoreAudio,
        reason,
    }
}

/// The UID a selector names for `direction`.
pub(crate) fn resolve(
    source: &dyn HalSource,
    selector: &DeviceSelector,
    direction: Direction,
) -> Result<String, AudioError> {
    let not_found = || AudioError::DeviceNotFound {
        direction,
        selector: selector.to_string(),
    };
    match selector {
        DeviceSelector::Default => {
            let scope = match direction {
                Direction::Input => Scope::Input,
                Direction::Output => Scope::Output,
            };
            source
                .default_uid(scope)
                .map_err(|e| backend_err(Operation::Open, e))?
                .ok_or_else(not_found)
        }
        DeviceSelector::Id(id) => uid_of(id).map(str::to_owned).ok_or_else(not_found),
    }
}

fn describe(
    source: &dyn HalSource,
    uid: &str,
    direction: Direction,
    selector: &DeviceSelector,
) -> Result<DevDesc, AudioError> {
    match source.describe(uid) {
        Ok(d) => Ok(d),
        Err(cahal::Error::NotFound) => Err(AudioError::DeviceNotFound {
            direction,
            selector: selector.to_string(),
        }),
        Err(e) => Err(backend_err(Operation::Open, e)),
    }
}

/// The microphone check before any capture starts: without a grant, the HAL blocks the
/// start on a prompt instead of failing it.
fn check_consent(source: &dyn HalSource, wait: Duration) -> Result<(), AudioError> {
    let answer = match source.microphone() {
        Consent::NotDetermined => source.request_microphone(wait),
        c => c,
    };
    match answer {
        Consent::Authorized => Ok(()),
        Consent::Denied => Err(unavailable(Unavailability::MicrophoneDenied)),
        Consent::NotDetermined => Err(unavailable(Unavailability::MicrophonePending)),
    }
}

fn check_hog(d: &DevDesc) -> Result<(), AudioError> {
    match d.hog_pid {
        Some(pid) if pid != std::process::id() as i32 => Err(unavailable(Unavailability::Host(
            format!("{} is in exclusive use (hog mode) by process {pid}", d.name),
        ))),
        _ => Ok(()),
    }
}

fn check_float(d: &DevDesc, direction: Direction) -> Result<(), AudioError> {
    let layout = match direction {
        Direction::Input => &d.input,
        Direction::Output => &d.output,
    };
    if layout.all_float32() {
        Ok(())
    } else {
        Err(Unsupported::SampleFormat { direction }.into())
    }
}

/// Sets `rate` on the device unless it runs at it already: the rate is device-wide and
/// other applications see it, so an unrequested rate is never forced.
fn set_rate(source: &dyn HalSource, d: &DevDesc, rate: u32) -> Result<(), AudioError> {
    if (d.nominal_rate - f64::from(rate)).abs() < 0.5 {
        return Ok(());
    }
    let offered = || {
        d.rates
            .iter()
            .map(|r| {
                if r.min == r.max {
                    format!("{} Hz", r.min)
                } else {
                    format!("{}–{} Hz", r.min, r.max)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let hz = f64::from(rate);
    if !d
        .rates
        .iter()
        .any(|r| r.min - 0.5 <= hz && hz <= r.max + 0.5)
    {
        return Err(Unsupported::SampleRate {
            requested: rate,
            offered: offered(),
        }
        .into());
    }
    source.set_rate(&d.uid, hz).map_err(|_| {
        AudioError::from(Unsupported::SampleRate {
            requested: rate,
            offered: offered(),
        })
    })
}

fn set_buffer(source: &dyn HalSource, d: &DevDesc, frames: u32) -> Result<(), AudioError> {
    let r = d.buffer_range;
    if frames < r.min || frames > r.max {
        return Err(Unsupported::BufferFrames {
            requested: frames,
            fixed: (r.min == r.max).then_some(r.min),
        }
        .into());
    }
    if d.buffer_frames == frames {
        return Ok(());
    }
    source
        .set_buffer(&d.uid, frames)
        .map_err(|e| backend_err(Operation::Open, e))
}

/// How long the open waits for the first cycle: the backstop for a start that returned
/// but never calls back.
fn first_cycle_wait(rate: u32, frames: u32) -> Duration {
    let periods = Duration::from_secs_f64(4.0 * f64::from(frames) / f64::from(rate.max(1)));
    periods.max(Duration::from_secs(2))
}

/// Flags the listeners set for the watcher.
#[derive(Debug, Default)]
struct Pending {
    end: AtomicBool,
    reset: AtomicBool,
    config: AtomicBool,
    latency: AtomicBool,
    rate: AtomicBool,
    /// Bumped on every notice, so the watcher knows to look.
    seq: AtomicU32,
}

fn notifier(
    pending: &Arc<Pending>,
    watcher: &Arc<std::sync::OnceLock<std::thread::Thread>>,
) -> Notify {
    let (p, w) = (Arc::clone(pending), Arc::clone(watcher));
    Arc::new(move |n: Notice| {
        let flag = match n {
            Notice::ServiceRestarted
            | Notice::Device(DeviceProperty::Alive | DeviceProperty::IoStoppedAbnormally) => &p.end,
            Notice::Device(DeviceProperty::NominalRate) => &p.rate,
            Notice::Device(DeviceProperty::StreamConfiguration | DeviceProperty::ClockSource) => {
                &p.reset
            }
            Notice::Device(DeviceProperty::BufferFrameSize) => &p.config,
            Notice::Device(DeviceProperty::Latency) => &p.latency,
            Notice::Device(DeviceProperty::ProcessorOverload) => return,
        };
        flag.store(true, Ordering::Release);
        p.seq.fetch_add(1, Ordering::Release);
        if let Some(t) = w.get() {
            t.unpark();
        }
    })
}

/// What the watcher compares against.
struct Watch {
    source: Arc<dyn HalSource>,
    uids: Vec<String>,
    rate: u32,
    latency: StaticLatency,
    events: Arc<BackendEvents>,
    shared: Vec<Arc<Shared>>,
    pending: Arc<Pending>,
    quit: Arc<AtomicBool>,
}

impl Watch {
    /// Runs off the HAL's threads: reads properties, ends or resets the stream.
    fn run(self, runs: Arc<Vec<Box<dyn Running>>>) {
        let mut last_out_minus_in: Vec<Option<u32>> =
            self.shared.iter().map(|s| s.out_minus_in()).collect();
        let mut latency = self.latency.clone();
        // A panicked closure stays panicked: the end is reported once, on the change.
        let mut panicked = false;
        while !self.quit.load(Ordering::Acquire) {
            std::thread::park_timeout(Duration::from_millis(100));
            if self.quit.load(Ordering::Acquire) {
                break;
            }
            let p = &self.pending;
            if !panicked && runs.iter().any(|r| r.stats().panicked) {
                panicked = true;
                self.events.end();
            }
            // The device died, its IO stopped abnormally, or coreaudiod restarted (every
            // object is gone): the recovery reopens by UID.
            if p.end.swap(false, Ordering::AcqRel) {
                self.events.end();
            }
            // Streams or clock source changed under the stream: its settings did.
            if p.reset.swap(false, Ordering::AcqRel) {
                self.events.reset(ResetReason::ResetRequest);
            }
            if p.rate.swap(false, Ordering::AcqRel) {
                let moved = self.uids.iter().any(|u| match self.source.describe(u) {
                    Ok(d) => (d.nominal_rate - f64::from(self.rate)).abs() >= 0.5,
                    Err(_) => true,
                });
                if moved {
                    self.events.reset(ResetReason::RateChange);
                }
            }
            if p.config.swap(false, Ordering::AcqRel) {
                self.events.config_change();
            }
            let now: Vec<Option<u32>> = self.shared.iter().map(|s| s.out_minus_in()).collect();
            let diff_moved = now != last_out_minus_in;
            if p.latency.swap(false, Ordering::AcqRel) || diff_moved {
                last_out_minus_in = now;
                if let Some(l) = self.restate()
                    && l != latency
                {
                    latency = l;
                    self.events.config_change();
                }
            }
        }
    }

    /// The stated latency now, for a one-device stream.
    fn restate(&self) -> Option<StaticLatency> {
        let [uid] = self.uids.as_slice() else {
            return None;
        };
        let d = self.source.describe(uid).ok()?;
        Some(caps::stated_latency(
            d.buffer_frames,
            &d.input_latency,
            &d.output_latency,
            self.shared.first()?.out_minus_in(),
        ))
    }
}

/// Keeps the IOProcs and the watcher; dropping it stops the watcher, then every IOProc.
struct Guard {
    runs: Option<Arc<Vec<Box<dyn Running>>>>,
    watcher: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some((quit, h)) = self.watcher.take() {
            quit.store(true, Ordering::Release);
            h.thread().unpark();
            let _ = h.join();
        }
        // The watcher's clone is gone: this drops (stops) each IOProc.
        drop(self.runs.take());
    }
}

/// Opens `request` on the HAL `source`. `consent_wait` bounds a microphone request.
pub(crate) fn open_stream(
    source: Arc<dyn HalSource>,
    mut request: DuplexRequest,
    consent_wait: Duration,
) -> Result<DuplexStream, AudioError> {
    request.validate()?;
    let src = &*source;
    let in_uid = resolve(src, &request.input_device, Direction::Input)?;
    let in_dev = describe(src, &in_uid, Direction::Input, &request.input_device)?;
    let device_inputs = in_dev.input.channels();
    if device_inputs == 0 {
        return Err(AudioError::DeviceNotFound {
            direction: Direction::Input,
            selector: request.input_device.to_string(),
        });
    }
    let device_inputs = u16::try_from(device_inputs).unwrap_or(u16::MAX);
    if let Some(&ch) = request.input_map.iter().find(|&&c| c >= device_inputs) {
        return Err(Unsupported::InputChannel {
            channel: ch,
            available: device_inputs,
        }
        .into());
    }
    check_float(&in_dev, Direction::Input)?;

    // Output: none, the input device itself (one IOProc, one clock), or another device.
    let out_dev = if request.output_channels == 0 {
        None
    } else {
        let same = match &request.output_device {
            // Intended: a default output follows an input device that plays, even when the
            // system's default output is another device. One device, one IOProc, one clock
            // is the point of the backend (H1); another device has to be named.
            DeviceSelector::Default => in_dev.output.channels() > 0,
            DeviceSelector::Id(id) => uid_of(id) == Some(in_uid.as_str()),
        };
        if same {
            Some(in_dev.clone())
        } else {
            let uid = resolve(src, &request.output_device, Direction::Output)?;
            Some(describe(
                src,
                &uid,
                Direction::Output,
                &request.output_device,
            )?)
        }
    };
    let one_device = out_dev.as_ref().is_none_or(|o| o.uid == in_dev.uid);
    if let Some(o) = &out_dev {
        let available = u16::try_from(o.output.channels()).unwrap_or(u16::MAX);
        if request.output_channels > available {
            return Err(Unsupported::OutputChannels {
                requested: request.output_channels,
                available,
            }
            .into());
        }
        check_float(o, Direction::Output)?;
    }
    check_hog(&in_dev)?;
    if let Some(o) = &out_dev {
        check_hog(o)?;
    }
    check_consent(src, consent_wait)?;

    let rate = request
        .sample_rate
        .unwrap_or(in_dev.nominal_rate.round() as u32);
    set_rate(src, &in_dev, rate)?;
    if let Some(o) = out_dev.as_ref().filter(|_| !one_device) {
        set_rate(src, o, rate)?;
    }
    let frames = request
        .buffer_frames
        .unwrap_or_else(|| short_buffer_frames(rate, Some(range(&in_dev))));
    set_buffer(src, &in_dev, frames)?;
    if let Some(o) = out_dev.as_ref().filter(|_| !one_device) {
        set_buffer(src, o, frames)?;
    }

    // Variable-size devices may deliver short cycles: size the header ring for them.
    let min_block = (frames / 4).clamp(16.min(frames), frames.max(1)) as usize;
    let plumbing = Plumbing::new(&mut request, rate, min_block);
    let events = Arc::clone(&plumbing.events);
    let pending = Arc::new(Pending::default());
    let watcher_thread = Arc::new(std::sync::OnceLock::new());
    let notify = notifier(&pending, &watcher_thread);
    let capture = Capture {
        producer: plumbing.producer,
        map: request.input_map.iter().map(|&c| usize::from(c)).collect(),
    };
    let render = out_dev.as_ref().map(|_| Render {
        renderer: plumbing.renderer,
    });
    let engine = |capture: Option<Capture>, render: Option<Render>, shared: &Arc<Shared>| Engine {
        capture,
        render,
        latch: EventLatch::new(Arc::clone(&events)),
        events: Arc::clone(&events),
        shared: Arc::clone(shared),
    };
    let start = |uid: &str, config: IoConfig, mut e: Engine| {
        src.start(
            uid,
            config,
            Box::new(move |c| e.cycle(c)),
            Arc::clone(&notify),
        )
        .map_err(|e| backend_err(Operation::Start, e))
    };

    let mut runs: Vec<Box<dyn Running>> = Vec::new();
    let mut shared = Vec::new();
    if one_device {
        let s = Arc::new(Shared::new());
        let config = IoConfig {
            input: true,
            output: render.is_some(),
        };
        runs.push(start(&in_uid, config, engine(Some(capture), render, &s))?);
        shared.push(s);
    } else {
        // Two devices, two clocks: each IOProc indexes its own side.
        let (si, so) = (Arc::new(Shared::new()), Arc::new(Shared::new()));
        let out_uid = out_dev.as_ref().map(|o| o.uid.clone()).unwrap_or_default();
        runs.push(start(
            &out_uid,
            IoConfig {
                input: false,
                output: true,
            },
            engine(None, render, &so),
        )?);
        runs.push(start(
            &in_uid,
            IoConfig {
                input: true,
                output: false,
            },
            engine(Some(capture), None, &si),
        )?);
        shared.push(si);
        shared.push(so);
    }
    let wait = first_cycle_wait(rate, frames);
    for r in &runs {
        r.wait_first_cycle(wait)
            .map_err(|e| backend_err(Operation::Start, format!("{}: {e}", in_dev.name)))?;
    }

    // What the devices run at now that the IOProcs are started.
    let in_now = src.describe(&in_uid).unwrap_or_else(|_| in_dev.clone());
    let actual = in_now.buffer_frames;
    let out_now = match &out_dev {
        Some(o) if !one_device => src.describe(&o.uid).unwrap_or_else(|_| o.clone()),
        Some(_) => in_now.clone(),
        None => in_now.clone(),
    };
    let latency = if one_device {
        caps::stated_latency(
            actual,
            &in_now.input_latency,
            &in_now.output_latency,
            shared[0].out_minus_in(),
        )
    } else {
        // Each side states its own part; their sum means nothing across two clocks.
        match caps::stated_latency(actual, &in_now.input_latency, &out_now.output_latency, None) {
            StaticLatency::Frames { input, .. } => StaticLatency::Frames {
                input,
                output: out_now.buffer_frames
                    + out_now.output_latency.safety_offset
                    + out_now.output_latency.device
                    + out_now.output_latency.streams.first().copied().unwrap_or(0),
                driver_buffering: None,
            },
            other => other,
        }
    };
    let estimated = shared.iter().any(|s| s.estimated.load(Ordering::Relaxed));
    let clock_source = in_now
        .clock_sources
        .iter()
        .find(|c| c.current)
        .map(|c| c.name.clone());
    let output_part = match &latency {
        StaticLatency::Frames { output, .. } => *output,
        _ => 0,
    };
    let negotiated = Negotiated {
        backend: BackendKind::CoreAudio,
        input_device: device_id(&in_uid),
        output_device: out_dev
            .as_ref()
            .map_or(crate::backend::DeviceId(String::new()), |o| {
                device_id(&o.uid)
            }),
        sample_rate: rate,
        input_channels: request.input_map.len() as u16,
        device_input_channels: device_inputs,
        output_channels: request.output_channels,
        device_output_channels: out_dev.as_ref().map_or(0, |o| {
            u16::try_from(o.output.channels()).unwrap_or(u16::MAX)
        }),
        buffer_frames: in_now.variable_buffer.is_none().then_some(actual),
        input_format: SampleFormat::F32,
        output_format: out_dev.as_ref().map(|_| SampleFormat::F32),
        clock: if one_device && out_dev.is_some() {
            ClockRelation::SingleCallback
        } else {
            ClockRelation::Unknown
        },
        index: if estimated {
            IndexExactness::Estimated
        } else {
            IndexExactness::Exact
        },
        latency: latency.clone(),
        clock_source,
        delivery: Delivery::Device,
    };
    // The last rendered cycle leaves after the output part of the latency (its buffer,
    // safety offset and converter) has played.
    let drain = Duration::from_secs_f64(f64::from(output_part.max(actual)) / f64::from(rate))
        + Duration::from_millis(20);

    let runs = Arc::new(runs);
    let quit = Arc::new(AtomicBool::new(false));
    let mut uids = vec![in_uid.clone()];
    if let Some(o) = out_dev.as_ref().filter(|_| !one_device) {
        uids.push(o.uid.clone());
    }
    let watch = Watch {
        source: Arc::clone(&source),
        uids,
        rate,
        latency,
        events: Arc::clone(&events),
        shared,
        pending,
        quit: Arc::clone(&quit),
    };
    let watcher = std::thread::Builder::new()
        .name("ac2-ca-watch".into())
        .spawn({
            let runs = Arc::clone(&runs);
            move || watch.run(runs)
        })
        .ok()
        .map(|h| {
            let _ = watcher_thread.set(h.thread().clone());
            (quit, h)
        });
    Ok(DuplexStream::new(StreamParts {
        negotiated,
        capture: plumbing.consumer,
        output: plumbing.output,
        events,
        drain,
        guard: Box::new(Guard {
            runs: Some(runs),
            watcher,
        }),
        patch: None,
    }))
}

fn range(d: &DevDesc) -> crate::backend::FrameRange {
    crate::backend::FrameRange {
        min: d.buffer_range.min,
        max: d.buffer_range.max,
    }
}
