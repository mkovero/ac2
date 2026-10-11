use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::backend::{
    Backend, BackendKind, ClockRelation, Delivery, DeviceCaps, DeviceId, DeviceSelector, Direction,
    DirectionCaps, DuplexRequest, FrameRange, IndexExactness, Negotiated, Presence, RateRange,
    SampleFormat, StaticLatency,
};
use crate::error::{AudioError, Operation, Unsupported};
use crate::stream::{DuplexStream, Plumbing, StreamParts};

use super::config::{FakeConfig, FakeConfigError, FakeDrive, FakeEndpoints, Pace};
use super::outage::{Outage, OutageKind};
use super::sim::{FakeDriver, Sim};
use super::{FAKE_DEVICE_ID, FAKE_INPUT_ID, FAKE_OUTPUT_ID, FAKE_SPLIT_NAME as SPLIT_NAME};

/// The simulated device. Cloning shares the parked manual driver slot and the device's
/// availability.
#[derive(Debug, Clone)]
pub struct FakeBackend {
    config: FakeConfig,
    parked: Arc<Mutex<Option<FakeDriver>>>,
    outage: Arc<Outage>,
}

impl FakeBackend {
    /// A fake device with `config`.
    pub fn new(config: FakeConfig) -> Result<Self, FakeConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            parked: Arc::new(Mutex::new(None)),
            outage: Arc::new(Outage::new()),
        })
    }

    /// The device stops delivering without an error ([`OutageKind::Stall`]) for `lasts`, or
    /// until [`Self::restore`] when `None`. Acts on [`FakeDrive::Thread`] streams; a manual
    /// stream simply is not stepped.
    pub fn stall(&self, lasts: Option<Duration>) {
        self.outage.begin(OutageKind::Stall, lasts);
    }

    /// The device goes away with an error ([`OutageKind::Vanish`]) for `lasts`, or until
    /// [`Self::restore`] when `None`.
    pub fn vanish(&self, lasts: Option<Duration>) {
        self.outage.begin(OutageKind::Vanish, lasts);
    }

    /// Ends the outage: streams opened from now on deliver again.
    pub fn restore(&self) {
        self.outage.end();
    }

    /// The outage in force, if any.
    pub fn outage(&self) -> Option<OutageKind> {
        self.outage.active()
    }

    /// The device settings.
    pub fn config(&self) -> &FakeConfig {
        &self.config
    }

    /// The driver of the most recent [`Backend::open`] in [`FakeDrive::Manual`] mode.
    pub fn take_driver(&self) -> Option<FakeDriver> {
        self.parked.lock().ok().and_then(|mut slot| slot.take())
    }

    /// Opens a stream and returns its driver, regardless of [`FakeConfig::drive`].
    pub fn open_manual(
        &self,
        mut request: DuplexRequest,
    ) -> Result<(DuplexStream, FakeDriver), AudioError> {
        request.validate()?;
        self.outage.admit()?;
        let c = &self.config;
        for (selector, direction) in [
            (&request.input_device, Direction::Input),
            (&request.output_device, Direction::Output),
        ] {
            // A capture-only stream opens no output device, whatever it names (as cpal).
            let used = direction == Direction::Input || request.output_channels > 0;
            if used
                && let DeviceSelector::Id(id) = selector
                && id.0 != c.endpoints.id(direction)
            {
                return Err(AudioError::DeviceNotFound {
                    direction,
                    selector: selector.to_string(),
                });
            }
        }
        if let Some(r) = request.sample_rate.filter(|&r| r != c.sample_rate) {
            return Err(Unsupported::SampleRate {
                requested: r,
                offered: format!("{} Hz only", c.sample_rate),
            }
            .into());
        }
        if let Some(b) = request.buffer_frames.filter(|&b| b != c.block_frames) {
            return Err(Unsupported::BufferFrames {
                requested: b,
                fixed: Some(c.block_frames),
            }
            .into());
        }
        if let Some(&ch) = request.input_map.iter().find(|&&ch| ch >= c.inputs) {
            return Err(Unsupported::InputChannel {
                channel: ch,
                available: c.inputs,
            }
            .into());
        }
        if request.output_channels > c.outputs {
            return Err(Unsupported::OutputChannels {
                requested: request.output_channels,
                available: c.outputs,
            }
            .into());
        }

        let plumbing = Plumbing::new(&mut request, c.sample_rate, c.block_frames as usize);
        let negotiated = Negotiated {
            backend: BackendKind::Fake,
            input_device: DeviceId(c.endpoints.id(Direction::Input).into()),
            output_device: DeviceId(c.endpoints.id(Direction::Output).into()),
            sample_rate: c.sample_rate,
            input_channels: request.input_map.len() as u16,
            device_input_channels: c.inputs,
            output_channels: request.output_channels,
            device_output_channels: request.output_channels,
            buffer_frames: Some(c.block_frames),
            input_format: SampleFormat::F32,
            output_format: Some(SampleFormat::F32),
            // The simulation runs one callback either way; two devices are reported as a
            // host reports them, with no proof they share a clock.
            clock: match c.endpoints {
                FakeEndpoints::Duplex => ClockRelation::SingleCallback,
                FakeEndpoints::Split => ClockRelation::Unknown,
            },
            index: IndexExactness::Exact,
            latency: StaticLatency::Unknown,
            delivery: Delivery::Stepped,
        };
        let sim = Sim::new(
            c.clone(),
            &request,
            plumbing.producer,
            plumbing.renderer,
            Arc::clone(&plumbing.events),
        );
        let stream = DuplexStream::new(StreamParts {
            negotiated,
            capture: plumbing.consumer,
            output: plumbing.output,
            events: plumbing.events,
            // Simulated: nothing to drain in wall-clock time.
            drain: Duration::ZERO,
            guard: Box::new(()),
            patch: None,
        });
        Ok((stream, FakeDriver { sim: Box::new(sim) }))
    }
}

impl Backend for FakeBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Fake
    }

    fn probe(&self, device: &DeviceSelector) -> Presence {
        match device {
            DeviceSelector::Id(id) if id.0 != self.config.endpoints.id(Direction::Input) => {
                Presence::Absent
            }
            _ => self.outage.presence(),
        }
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let c = &self.config;
        let dir = |channels: u16, names: &Option<Vec<String>>| DirectionCaps {
            max_channels: channels,
            rates: vec![RateRange {
                min: c.sample_rate,
                max: c.sample_rate,
            }],
            buffer_frames: Some(FrameRange {
                min: c.block_frames,
                max: c.block_frames,
            }),
            formats: vec![SampleFormat::F32],
            default_rate: Some(c.sample_rate),
            default_buffer: Some(c.block_frames),
            channel_names: names.clone(),
            system_default: true,
        };
        let input = Some(dir(c.inputs, &c.input_names));
        let output = (c.outputs > 0).then(|| dir(c.outputs, &c.output_names));
        let device = |id: &str, name: &str, input, output, duplex_clock| DeviceCaps {
            backend: BackendKind::Fake,
            host: "fake".into(),
            id: DeviceId(id.into()),
            name: name.into(),
            input,
            output,
            duplex_clock,
            index: IndexExactness::Exact,
            latency: StaticLatency::Unknown,
            notes: Vec::new(),
        };
        Ok(match c.endpoints {
            FakeEndpoints::Duplex => vec![device(
                FAKE_DEVICE_ID,
                "simulated duplex device",
                input,
                output,
                ClockRelation::SingleCallback,
            )],
            FakeEndpoints::Split => vec![
                device(
                    FAKE_INPUT_ID,
                    SPLIT_NAME,
                    input,
                    None,
                    ClockRelation::Unknown,
                ),
                device(
                    FAKE_OUTPUT_ID,
                    SPLIT_NAME,
                    None,
                    output,
                    ClockRelation::Unknown,
                ),
            ],
        })
    }

    fn open(&self, request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        let (mut stream, driver) = self.open_manual(request)?;
        match self.config.drive {
            FakeDrive::Manual => {
                let mut slot = self.parked.lock().map_err(|_| AudioError::Backend {
                    backend: BackendKind::Fake,
                    operation: Operation::Open,
                    detail: "driver slot poisoned".into(),
                })?;
                *slot = Some(driver);
            }
            FakeDrive::Thread(pace) => {
                stream.set_delivery(Delivery::Device);
                let outage = Arc::clone(&self.outage);
                let guard = spawn_driver(driver, pace, &self.config, outage).map_err(|e| {
                    AudioError::Backend {
                        backend: BackendKind::Fake,
                        operation: Operation::Start,
                        detail: e.to_string(),
                    }
                })?;
                let drain =
                    Duration::from_nanos(self.config.ns(2 * u64::from(self.config.block_frames)));
                stream.set_guard(Box::new(guard), drain);
            }
        }
        Ok(stream)
    }
}

struct ThreadGuard {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ThreadGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            // A panicked driver thread has nothing left to clean up.
            let _ = t.join();
        }
    }
}

fn spawn_driver(
    mut driver: FakeDriver,
    pace: Pace,
    c: &FakeConfig,
    outage: Arc<Outage>,
) -> std::io::Result<ThreadGuard> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let limit = c.stop_after_blocks;
    let block = Duration::from_nanos(c.ns(u64::from(c.block_frames)));
    let (sleeper, _) = crate::timer::sleeper()?;
    // Read before the thread starts: an outage begun after the open hits this stream.
    let born = outage.begun.load(Ordering::Acquire);
    let thread = std::thread::Builder::new()
        .name("ac2-fake-audio".into())
        .spawn(move || {
            let t0 = Instant::now();
            let mut due = Duration::ZERO;
            let mut ended = false;
            while !stop_t.load(Ordering::Acquire) {
                if outage.begun.load(Ordering::Acquire) != born {
                    // The device was reset under this stream: it never delivers again; a
                    // vanished one is reported ended by the host, a stalled one says nothing.
                    if !ended && outage.kind.load(Ordering::Acquire) == 2 {
                        ended = true;
                        driver.sim.events.end();
                    }
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                if limit.is_some_and(|l| driver.blocks() >= l) {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                driver.step();
                if pace == Pace::Realtime {
                    due += block;
                    // A coalesced sleep would deliver the audio in lumps a real device
                    // never produces.
                    while Instant::now() < t0 + due {
                        sleeper.sleep_until(t0 + due);
                    }
                }
            }
        })?;
    Ok(ThreadGuard {
        stop,
        thread: Some(thread),
    })
}
