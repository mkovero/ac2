//! `ac2 selftest duplex`: opens a duplex stream on a real `ac2-audio` backend in this
//! process (no daemon) and reports what the device delivered, pass or fail with named
//! reasons, in a form a tester can paste.
//!
//! The output is silent unless an emit level is given. Silently the test still sees device
//! open, channel counts, block continuity, xruns, callback timing and both directions'
//! clocks against the host clock. Output→input timing needs a stimulus on a loopback cable,
//! so it is measured only with emission, by the daemon's own loopback timing monitor.
//!
//! Emission goes through the production output path: the `ac2-core` pink-noise generator
//! below [`CEILING_DBFS`] (refused above it), the stream's sample-peak limit derived from
//! that ceiling, fades on every start and stop, and a stop that waits for the fade-out.

mod loopback;
mod stats;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ac2_audio::{
    Backend, BackendKind, ClockRelation, DeviceSelector, DuplexRequest, DuplexStream,
    EventSnapshot, GeneratorHandle, HistoryRequest, IndexExactness, MaxLevel, OutputSource,
    SampleFormat, SignalSource, StopOutcome, TransportStats,
};
use ac2_core::generator::{BandLimit, Generator, GeneratorConfig, GeneratorError, Signal};
use serde::Serialize;

pub use loopback::LoopbackReport;
pub use stats::{CaptureStats, ClockRate, OutputCallbacks, Spread};

use loopback::LoopbackCheck;
use stats::StreamStats;

/// Highest stimulus level the self-test emits, dBFS RMS. A loopback cable needs far less
/// than a measurement does: the timing monitor locks on a stimulus 60 dB below this.
pub const CEILING_DBFS: f64 = -20.0;

/// Largest deviation of a device clock from its negotiated rate, measured against the host
/// clock, that passes. Converter crystals are within ±100 ppm; a device running at another
/// rate than it reports (44.1 vs 48 kHz is 8.8 %) or a host resampling behind ac2's back is
/// far outside.
pub const RATE_TOLERANCE_PPM: f64 = 1000.0;

/// Shortest audio span on which the clock rate is judged, s: below it the callback-time
/// scatter dominates the slope.
pub const RATE_MIN_SPAN_S: f64 = 2.0;

/// The stream to test.
#[derive(Clone, Debug, PartialEq)]
pub struct DuplexSpec {
    /// Capture device.
    pub input_device: DeviceSelector,
    /// Playback device.
    pub output_device: DeviceSelector,
    /// Zero-based device inputs to capture.
    pub input_map: Vec<u16>,
    /// Output channels to open (silent unless [`Emit`] routes the stimulus to one).
    pub output_channels: u16,
    /// Sample rate; `None` takes the device's default.
    pub sample_rate: Option<u32>,
    /// Callback size; `None` takes the backend's choice.
    pub buffer_frames: Option<u32>,
}

/// Stimulus on a loopback cable, for output→input timing and drift.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Emit {
    /// Pink-noise level, dBFS RMS; at most [`CEILING_DBFS`].
    pub level_dbfs: f64,
    /// Zero-based output carrying the stimulus (the cable's start).
    pub loopback_out: u16,
    /// Zero-based device input the cable returns on; must be captured.
    pub loopback_in: u16,
}

/// Why a self-test could not run at all.
#[derive(Debug, Clone, PartialEq)]
pub enum SetupError {
    /// The emit level is above [`CEILING_DBFS`] or cannot be generated.
    Level(String),
    /// The loopback input is not among the captured inputs.
    LoopbackInNotCaptured(u16),
    /// The loopback output is not among the opened outputs.
    LoopbackOutNotOpened(u16),
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Level(m) => f.write_str(m),
            Self::LoopbackInNotCaptured(c) => {
                write!(
                    f,
                    "loopback input {} is not among the captured inputs",
                    c + 1
                )
            }
            Self::LoopbackOutNotOpened(c) => {
                write!(
                    f,
                    "loopback output {} is not among the opened outputs",
                    c + 1
                )
            }
        }
    }
}

impl std::error::Error for SetupError {}

/// A named reason the self-test failed.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum Failure {
    /// The backend refused to open the stream.
    OpenFailed {
        /// The backend's error.
        error: String,
    },
    /// The run was interrupted before its duration.
    Interrupted,
    /// Not one capture block arrived.
    NoAudio,
    /// Capture blocks stopped arriving: far fewer frames than the time passed.
    Stalled {
        /// Frames received.
        frames: u64,
        /// Frames the wall-clock time since the first block would hold.
        expected: u64,
    },
    /// Output streams were opened but no output callback ran.
    NoOutputCallbacks,
    /// The host reported xruns.
    Xruns {
        /// Blocks flagged, or host notifications, whichever is more.
        count: u64,
    },
    /// The sample index skipped frames.
    IndexGaps {
        /// Gaps.
        gaps: u64,
        /// Frames skipped.
        frames: u64,
    },
    /// The sample index went backwards.
    IndexRegressions {
        /// Blocks.
        count: u64,
    },
    /// This test fell behind and blocks were dropped (the machine is overloaded).
    Overflow {
        /// Blocks dropped.
        blocks: u64,
    },
    /// The host changed the stream's configuration.
    ConfigChanged {
        /// Changes reported.
        count: u64,
    },
    /// The host reported stream errors.
    DeviceErrors {
        /// Errors reported.
        count: u64,
    },
    /// The stream ended on its own (device gone, server stopped).
    StreamEnded,
    /// The capture clock is off its negotiated rate.
    InputRate {
        /// Deviation, ppm.
        ppm: f64,
    },
    /// The output clock is off its negotiated rate.
    OutputRate {
        /// Deviation, ppm.
        ppm: f64,
    },
    /// The output path limited samples: a level computation upstream is wrong.
    LevelLimited {
        /// Samples limited.
        samples: u64,
    },
    /// The output did not report silence after the fade-out.
    FadeOutTimedOut,
    /// The stimulus never showed on the loopback input.
    NoLoopbackLock,
    /// The output→input offset changed during the run.
    LoopbackJumped {
        /// Jumps.
        count: u64,
        /// The first: offset before, samples.
        from: i64,
        /// The first: offset after, samples.
        to: i64,
    },
    /// The loopback lost its lock while the stimulus played.
    LoopbackLost {
        /// Times.
        count: u64,
    },
    /// Output and input are on different clocks.
    ClockDrift {
        /// Drift, ppm.
        ppm: f64,
    },
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenFailed { error } => write!(f, "the stream did not open: {error}"),
            Self::Interrupted => f.write_str("interrupted before the run ended"),
            Self::NoAudio => f.write_str("no capture block arrived"),
            Self::Stalled { frames, expected } => {
                write!(
                    f,
                    "capture stalled: {frames} frames where {expected} were due"
                )
            }
            Self::NoOutputCallbacks => f.write_str("the output never ran a callback"),
            Self::Xruns { count } => write!(f, "{count} xruns reported by the host"),
            Self::IndexGaps { gaps, frames } => {
                write!(f, "{gaps} sample-index gaps, {frames} frames lost")
            }
            Self::IndexRegressions { count } => {
                write!(f, "the sample index went backwards {count} times")
            }
            Self::Overflow { blocks } => {
                write!(
                    f,
                    "{blocks} blocks dropped: this machine fell behind the device"
                )
            }
            Self::ConfigChanged { count } => {
                write!(f, "the host changed the stream configuration {count} times")
            }
            Self::DeviceErrors { count } => write!(f, "{count} stream errors from the host"),
            Self::StreamEnded => f.write_str("the stream ended on its own"),
            Self::InputRate { ppm } => write!(
                f,
                "capture clock {ppm:+.0} ppm off its rate (limit ±{RATE_TOLERANCE_PPM:.0})"
            ),
            Self::OutputRate { ppm } => write!(
                f,
                "output clock {ppm:+.0} ppm off its rate (limit ±{RATE_TOLERANCE_PPM:.0})"
            ),
            Self::LevelLimited { samples } => {
                write!(f, "{samples} output samples hit the level limit")
            }
            Self::FadeOutTimedOut => f.write_str("the output did not confirm its fade-out"),
            Self::NoLoopbackLock => {
                f.write_str("the stimulus never locked on the loopback input (cable, channels?)")
            }
            Self::LoopbackJumped { count, from, to } => write!(
                f,
                "the output→input offset jumped {count} times (first {from} → {to} samples)"
            ),
            Self::LoopbackLost { count } => write!(f, "the loopback lock was lost {count} times"),
            Self::ClockDrift { ppm } => write!(
                f,
                "output and input drift {ppm:+.2} ppm: they are not on one clock"
            ),
        }
    }
}

/// What was opened.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Opened {
    /// Backend.
    pub backend: BackendKind,
    /// Capture device id.
    pub input_device: String,
    /// Playback device id.
    pub output_device: String,
    /// Stream rate, Hz.
    pub sample_rate: u32,
    /// Captured inputs, zero-based device channels.
    pub inputs: Vec<u16>,
    /// Channels the capture device has.
    pub device_inputs: u16,
    /// Output channels of the stream.
    pub outputs: u16,
    /// Channels the playback device was opened with.
    pub device_outputs: u16,
    /// Callback size when fixed by the host.
    pub buffer_frames: Option<u32>,
    /// Capture sample format.
    pub input_format: SampleFormat,
    /// Playback sample format.
    pub output_format: Option<SampleFormat>,
    /// How the directions relate in time.
    pub clock: ClockRelation,
    /// How the capture index is obtained.
    pub index: IndexExactness,
}

/// The whole result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DuplexReport {
    /// Passed: no failure.
    pub pass: bool,
    /// Every failure, in the order checked.
    pub failures: Vec<Failure>,
    /// Seconds the stream ran.
    pub seconds: f64,
    /// The stimulus, if one played.
    pub emit: Option<Emit>,
    /// What was opened; `None` if opening failed.
    pub opened: Option<Opened>,
    /// Capture side.
    pub capture: Option<CaptureStats>,
    /// Output side.
    pub output: Option<OutputCallbacks>,
    /// Host notifications.
    pub host_events: Option<EventSnapshot>,
    /// Capture transport counters.
    pub transport: Option<TransportStats>,
    /// Output samples the level limit acted on.
    pub limited_samples: u64,
    /// How the stream stopped.
    pub stop: Option<StopOutcome>,
    /// Output→input timing (emission only).
    pub loopback: Option<LoopbackReport>,
}

impl DuplexReport {
    fn open_failed(error: String, emit: Option<Emit>) -> Self {
        Self {
            pass: false,
            failures: vec![Failure::OpenFailed { error }],
            seconds: 0.0,
            emit,
            opened: None,
            capture: None,
            output: None,
            host_events: None,
            transport: None,
            limited_samples: 0,
            stop: None,
            loopback: None,
        }
    }
}

/// The `ac2-core` generator as the output path's signal source.
struct Stimulus(Generator);

impl SignalSource for Stimulus {
    fn fill(&mut self, out: &mut [f32]) {
        self.0.fill(out);
    }
}

fn level_error(e: GeneratorError) -> SetupError {
    SetupError::Level(match e {
        GeneratorError::AboveCeiling {
            requested_dbfs,
            ceiling_dbfs,
        } => format!(
            "{requested_dbfs:.1} dBFS is above the self-test maximum of {ceiling_dbfs:.0} dBFS"
        ),
        other => other.to_string(),
    })
}

/// A running self-test stream.
pub struct DuplexSession {
    stream: DuplexStream,
    generator: Option<GeneratorHandle>,
    emit: Option<Emit>,
    opened: Opened,
    stats: StreamStats,
    loopback: Option<LoopbackCheck>,
    buf: Vec<f32>,
    started: Instant,
    first_block: Option<Instant>,
}

impl std::fmt::Debug for DuplexSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuplexSession")
            .field("opened", &self.opened)
            .finish_non_exhaustive()
    }
}

/// Opening ended in a report (the backend refused) rather than a session.
pub type Opening = Result<DuplexSession, Box<DuplexReport>>;

impl DuplexSession {
    /// Opens `spec` on `backend`; with `emit`, starts the stimulus on the loopback output.
    /// A setup that cannot work is refused before anything is opened.
    pub fn open(
        backend: &dyn Backend,
        spec: &DuplexSpec,
        emit: Option<Emit>,
    ) -> Result<Opening, SetupError> {
        let loopback_idx = match emit {
            Some(e) => {
                if e.loopback_out >= spec.output_channels {
                    return Err(SetupError::LoopbackOutNotOpened(e.loopback_out));
                }
                Some(
                    spec.input_map
                        .iter()
                        .position(|&c| c == e.loopback_in)
                        .ok_or(SetupError::LoopbackInNotCaptured(e.loopback_in))?,
                )
            }
            None => None,
        };
        // The peak limit is the daemon's: the self-test ceiling times the largest crest any
        // generator signal is allowed.
        let max_level = MaxLevel::from_peak_db(ac2_core::generator::peak_limit_db(CEILING_DBFS))
            .map_err(|e| SetupError::Level(e.to_string()))?;
        let mut request =
            DuplexRequest::new(spec.input_map.clone(), spec.output_channels, max_level);
        request.input_device = spec.input_device.clone();
        request.output_device = spec.output_device.clone();
        request.sample_rate = spec.sample_rate;
        request.buffer_frames = spec.buffer_frames;
        let mut handle = None;
        if let Some(e) = emit {
            check_level(e.level_dbfs)?;
            let (h, port) = ac2_audio::generator([e.loopback_out])
                .map_err(|err| SetupError::Level(err.to_string()))?;
            request.output = OutputSource::Generator(port);
            request.history = Some(HistoryRequest::channel(e.loopback_out));
            handle = Some(h);
        }
        let stream = match backend.open(request) {
            Ok(s) => s,
            Err(err) => {
                return Ok(Err(Box::new(DuplexReport::open_failed(
                    err.to_string(),
                    emit,
                ))));
            }
        };
        let n = stream.negotiated().clone();
        if let (Some(h), Some(e)) = (handle.as_mut(), emit) {
            let g = Generator::new(&generator_config(e.level_dbfs, f64::from(n.sample_rate)))
                .map_err(level_error)?;
            if h.set_source(Box::new(Stimulus(g))).is_err() {
                return Err(SetupError::Level(
                    "the generator did not take its source".into(),
                ));
            }
            h.start();
        }
        let loopback = match (loopback_idx, stream.history()) {
            (Some(idx), Some(history)) => {
                Some(LoopbackCheck::new(n.sample_rate, idx, history.clone()))
            }
            _ => None,
        };
        let opened = Opened {
            backend: n.backend,
            input_device: n.input_device.0.clone(),
            output_device: n.output_device.0.clone(),
            sample_rate: n.sample_rate,
            inputs: spec.input_map.clone(),
            device_inputs: n.device_input_channels,
            outputs: n.output_channels,
            device_outputs: n.device_output_channels,
            buffer_frames: n.buffer_frames,
            input_format: n.input_format,
            output_format: n.output_format,
            clock: n.clock,
            index: n.index,
        };
        Ok(Ok(Self {
            stats: StreamStats::new(n.sample_rate, n.input_channels),
            stream,
            generator: handle,
            emit,
            opened,
            loopback,
            buf: Vec::new(),
            started: Instant::now(),
            first_block: None,
        }))
    }

    /// What was opened.
    pub fn opened(&self) -> &Opened {
        &self.opened
    }

    /// Takes every waiting capture block and output tick; returns whether there was any.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Some(h) = self.stream.capture().pop_into(&mut self.buf) {
            self.first_block.get_or_insert_with(Instant::now);
            self.stats.add_block(&h, &self.buf);
            if let Some(l) = self.loopback.as_mut() {
                l.add_block(&h, &self.buf);
            }
            any = true;
        }
        while let Some(t) = self.stream.pop_output_tick() {
            self.stats.add_tick(&t);
            any = true;
        }
        any
    }

    /// Starts the final fade-out; the stream keeps running until [`Self::finish`].
    pub fn begin_stop(&self) {
        if let Some(g) = &self.generator {
            g.stop();
        }
        self.stream.begin_stop();
    }

    /// Stops the stream (after its fade-out) and judges the run.
    pub fn finish(mut self, interrupted: bool) -> DuplexReport {
        self.begin_stop();
        self.poll();
        let seconds = self.started.elapsed().as_secs_f64();
        let since_first = self.first_block.map(|t| t.elapsed().as_secs_f64());
        let events = self.stream.events();
        let transport = self.stream.transport_stats();
        let limited = self.stream.output_stats().limited_samples;
        let rate = f64::from(self.opened.sample_rate);
        let outputs = self.opened.outputs;
        let frames = self.stats.frames();
        let stop = self.stream.stop(DuplexStream::DEFAULT_STOP_TIMEOUT);
        let (capture, output) = self.stats.finish();
        let loopback = self.loopback.map(LoopbackCheck::finish);

        let mut failures = Vec::new();
        if interrupted {
            failures.push(Failure::Interrupted);
        }
        if capture.blocks == 0 {
            failures.push(Failure::NoAudio);
        } else if let Some(s) = since_first.filter(|&s| s > 1.0) {
            // Half the due frames: callback scheduling and the last partial block never
            // cost that much; a device that stopped delivering does.
            let expected = (s * rate) as u64;
            if frames < expected / 2 {
                failures.push(Failure::Stalled { frames, expected });
            }
        }
        if outputs > 0 && output.callbacks == 0 {
            failures.push(Failure::NoOutputCallbacks);
        }
        let xruns = capture.xrun_blocks.max(events.xruns);
        if xruns > 0 {
            failures.push(Failure::Xruns { count: xruns });
        }
        if capture.index_gaps > 0 {
            failures.push(Failure::IndexGaps {
                gaps: capture.index_gaps,
                frames: capture.gap_frames,
            });
        }
        if capture.index_regressions > 0 {
            failures.push(Failure::IndexRegressions {
                count: capture.index_regressions,
            });
        }
        if transport.blocks_dropped > 0 || capture.overflow_blocks > 0 {
            failures.push(Failure::Overflow {
                blocks: transport.blocks_dropped.max(capture.overflow_blocks),
            });
        }
        let changes = capture.config_change_blocks.max(events.config_changes);
        if changes > 0 {
            failures.push(Failure::ConfigChanged { count: changes });
        }
        if events.errors > 0 {
            failures.push(Failure::DeviceErrors {
                count: events.errors,
            });
        }
        if events.ended {
            failures.push(Failure::StreamEnded);
        }
        let judged = |r: &Option<ClockRate>| {
            r.filter(|r| r.span_s >= RATE_MIN_SPAN_S && r.ppm.abs() > RATE_TOLERANCE_PPM)
                .map(|r| r.ppm)
        };
        if let Some(ppm) = judged(&capture.rate) {
            failures.push(Failure::InputRate { ppm });
        }
        if let Some(ppm) = judged(&output.rate) {
            failures.push(Failure::OutputRate { ppm });
        }
        if limited > 0 {
            failures.push(Failure::LevelLimited { samples: limited });
        }
        if stop == StopOutcome::TimedOut {
            failures.push(Failure::FadeOutTimedOut);
        }
        if let Some(l) = &loopback {
            if l.locks == 0 {
                failures.push(Failure::NoLoopbackLock);
            }
            if let Some(&(from, to)) = l.jumps.first() {
                failures.push(Failure::LoopbackJumped {
                    count: l.jumps.len() as u64,
                    from,
                    to,
                });
            }
            if l.lost > 0 {
                failures.push(Failure::LoopbackLost { count: l.lost });
            }
            if l.drift_warning
                && let Some(ppm) = l.drift_ppm
            {
                failures.push(Failure::ClockDrift { ppm });
            }
        }
        DuplexReport {
            pass: failures.is_empty(),
            failures,
            seconds,
            emit: self.emit,
            opened: Some(self.opened),
            capture: Some(capture),
            output: Some(output),
            host_events: Some(events),
            transport: Some(transport),
            limited_samples: limited,
            stop: Some(stop),
            loopback,
        }
    }
}

/// Refuses an emit level above [`CEILING_DBFS`] or one the generator cannot make, before
/// any device is touched. The rate is a placeholder: the stimulus is built at the
/// negotiated rate, and the level checks do not depend on it.
pub fn check_level(level_dbfs: f64) -> Result<(), SetupError> {
    Generator::new(&generator_config(level_dbfs, 48_000.0))
        .map(drop)
        .map_err(level_error)
}

fn generator_config(level_dbfs: f64, sample_rate: f64) -> GeneratorConfig {
    GeneratorConfig {
        signal: Signal::Pink,
        sample_rate,
        seed: 0x5e1f_7e57,
        band: BandLimit::NONE,
        level_dbfs,
        ceiling_dbfs: CEILING_DBFS,
    }
}

/// Runs `spec` for `duration` of wall-clock time, or until `stop` is set (reported as
/// interrupted), then fades out and judges the run.
pub fn run(
    backend: &dyn Backend,
    spec: &DuplexSpec,
    emit: Option<Emit>,
    duration: Duration,
    stop: &AtomicBool,
) -> Result<DuplexReport, SetupError> {
    let mut session = match DuplexSession::open(backend, spec, emit)? {
        Ok(s) => s,
        Err(report) => return Ok(*report),
    };
    let end = Instant::now() + duration;
    let mut interrupted = false;
    while Instant::now() < end {
        if stop.load(Ordering::Acquire) {
            interrupted = true;
            break;
        }
        if !session.poll() {
            // The capture ring holds two seconds: a few milliseconds' nap loses nothing.
            std::thread::sleep(Duration::from_millis(3));
        }
    }
    Ok(session.finish(interrupted))
}

#[cfg(test)]
mod tests;
