//! Raw capture files (`docs/design/raw-capture.md`): the captured samples of chosen inputs,
//! exactly as the device delivered them, in a 32-bit float WAV / RF64 file ([`wav`]), and
//! beside it a JSON sidecar ([`Sidecar`]) that says what the samples are: the device and
//! rate, each channel's input, name and role, where the file sits in the session's sample
//! clock, every discontinuity at its sample, what the operator changed while recording
//! (measurements, generator, inputs, calibrations) and which software wrote it.
//!
//! `<dir>/<name>.wav` and `<dir>/<name>.ac2rec.json`. The sidecar is written when the
//! recording starts (without an end) and replaced atomically when it ends; a sidecar
//! without an end is a recording that never finished ([`recover`]).

use std::fs;
use std::path::{Path, PathBuf};

use ac2_proto::event::Patch;
use ac2_proto::model::{
    BackendKind, CalEntry, CalKey, ClockRelation, DeviceId, DiscontinuityCause, Generator,
    InputSetup, LoopbackRoute, Measurement, RecordingEnd, RecordingFile,
};
use ac2_proto::units::{ClientId, MeasId, SampleIndex, Seconds, SessionEpoch, WallNs};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod wav;

pub use wav::{WavError, WavInfo, WavReader, WavWriter};

/// `format` of every sidecar.
pub const FORMAT: &str = "ac2-raw-capture";
/// The one sidecar version this build reads and writes.
pub const VERSION: u32 = 1;
/// Sidecar file name suffix.
pub const SIDECAR_SUFFIX: &str = ".ac2rec.json";
/// Audio file name suffix.
pub const AUDIO_SUFFIX: &str = ".wav";

/// Software that wrote a recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Software {
    /// `ac2d <version>`.
    pub ac2: String,
    /// Build id (git describe).
    pub build: String,
    /// Protocol version of the types quoted in this sidecar.
    pub protocol: u16,
}

/// One channel of the audio file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedChannel {
    /// Device input (zero-based).
    pub input: u16,
    /// The device's name for it, if it has one.
    pub name: Option<String>,
    /// The mic set on the input when recording started.
    pub mic: Option<String>,
    /// What the session and its measurements used it for when recording started.
    pub roles: Vec<ChannelRole>,
}

/// What a recorded input was used for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChannelRole {
    /// The session's reference loopback return.
    Loopback,
    /// Reference input of a transfer measurement.
    Reference {
        /// The measurement's name.
        measurement: String,
    },
    /// Measurement input of a transfer measurement.
    Measured {
        /// The measurement's name.
        measurement: String,
    },
    /// Input of a spectrum, RTA or SPL measurement.
    Analysed {
        /// The measurement's name.
        measurement: String,
    },
}

/// The audio file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioFile {
    /// File name, beside the sidecar.
    pub file: String,
    /// Sample rate, Hz.
    pub sample_rate: u32,
    /// Channels in file order.
    pub channels: Vec<RecordedChannel>,
}

/// The device and session that were recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedDevice {
    /// Backend.
    pub backend: BackendKind,
    /// Capture device.
    pub input_device: DeviceId,
    /// Playback device.
    pub output_device: DeviceId,
    /// Device period, frames.
    pub buffer_frames: u32,
    /// Clock relation of capture and playback.
    pub clock: ClockRelation,
    /// Session epoch.
    pub session_epoch: SessionEpoch,
    /// The session's reference loopback.
    pub loopback: Option<LoopbackRoute>,
}

/// A point of the recording on both clocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mark {
    /// Session sample index.
    pub session_sample: SampleIndex,
    /// Daemon wall clock, Unix ns.
    pub wall_ns: WallNs,
    /// The same, as UTC text.
    pub utc: String,
}

impl Mark {
    /// A mark at `session_sample`, `wall_ns`.
    pub fn new(session_sample: u64, wall_ns: u64) -> Self {
        Self {
            session_sample: SampleIndex(session_sample),
            wall_ns: WallNs(wall_ns),
            utc: crate::spl_log::utc_iso(wall_ns),
        }
    }
}

/// How the recording ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct End {
    /// One past the last recorded session sample, and when the recording ended.
    pub at: Mark,
    /// Frames in the audio file.
    pub frames: u64,
    /// Why it ended.
    pub reason: RecordingEnd,
}

/// The bounds the operator set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Most audio.
    pub max_duration: Seconds,
    /// Largest file.
    pub max_bytes: Option<u64>,
}

/// Daemon state that decides what the recorded inputs meant when recording started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Initial {
    /// Measurements.
    pub measurements: Vec<Measurement>,
    /// Generator.
    pub generator: Generator,
    /// Input setup (mic names, curves).
    pub inputs: Vec<InputSetup>,
    /// Sensitivity calibrations of the recorded inputs on this device.
    pub calibrations: Vec<CalEntry>,
}

/// What changed during the recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum TimelineChange {
    /// A measurement's new full value (configuration, delay, running, frozen), or its
    /// deletion.
    Measurement(Box<Patch<Measurement, MeasId>>),
    /// The generator's new state (armed, firing, signal, level).
    Generator(Generator),
    /// The whole input setup.
    Inputs(Vec<InputSetup>),
    /// A sensitivity calibration.
    Calibration(Patch<CalEntry, CalKey>),
}

/// One entry of the configuration timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineEntry {
    /// Newest captured session sample when the daemon committed the change: the change
    /// reaches the analyses with the next block they take, within one hand-off.
    pub at_sample: SampleIndex,
    /// The audio file frame of `at_sample` (where it is not recorded: the next one that is).
    pub frame: u64,
    /// When.
    pub wall_ns: WallNs,
    /// What.
    pub change: TimelineChange,
}

/// A point where the recorded audio is not contiguous.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discontinuity {
    /// Audio file frame of the first sample after it.
    pub frame: u64,
    /// Its session sample.
    pub session_sample: SampleIndex,
    /// Session samples missing before it (0: the device reported trouble but its counter
    /// went on).
    pub lost_frames: u64,
    /// `lost_frames` is estimated from host timestamps, not counted.
    pub estimated: bool,
    /// What happened.
    pub causes: Vec<DiscontinuityCause>,
}

/// The sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sidecar {
    /// Always [`FORMAT`].
    pub format: String,
    /// Always [`VERSION`].
    pub version: u32,
    /// Writer.
    pub software: Software,
    /// The audio file.
    pub audio: AudioFile,
    /// The device.
    pub device: RecordedDevice,
    /// Where the file's first frame sits.
    pub start: Mark,
    /// `None` while recording (or if the writer died: see [`recover`]).
    pub end: Option<End>,
    /// Bounds.
    pub limits: Limits,
    /// Who started it.
    pub started_by: ClientId,
    /// State at the start.
    pub initial: Initial,
    /// Changes, oldest first.
    pub timeline: Vec<TimelineEntry>,
    /// Discontinuities, oldest first.
    pub discontinuities: Vec<Discontinuity>,
}

/// An unusable recording.
#[derive(Debug, Error)]
pub enum RawError {
    /// No such recording.
    #[error("no recording {0}")]
    NotFound(String),
    /// A name that is not a plain file stem.
    #[error("{0:?} is not a recording name (letters, digits, space, - _ .)")]
    BadName(String),
    /// A recording of that name exists.
    #[error("a recording named {0:?} exists already")]
    Exists(String),
    /// The sidecar is not one this build reads.
    #[error("{path}: {msg}")]
    Sidecar {
        /// Path.
        path: PathBuf,
        /// What is wrong.
        msg: String,
    },
    /// The audio file.
    #[error("{path}: {err}")]
    Audio {
        /// Path.
        path: PathBuf,
        /// What is wrong.
        err: WavError,
    },
    /// The file system refused.
    #[error("{path}: {msg}")]
    Io {
        /// Path.
        path: PathBuf,
        /// What it said.
        msg: String,
    },
}

/// Checks a recording name: one plain file stem, like a session name.
pub fn validate_name(name: &str) -> Result<(), RawError> {
    crate::session::validate_name(name).map_err(|_| RawError::BadName(name.to_owned()))
}

/// Audio file of recording `name` in `dir`.
pub fn audio_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{AUDIO_SUFFIX}"))
}

/// Sidecar of recording `name` in `dir`.
pub fn sidecar_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{SIDECAR_SUFFIX}"))
}

/// The recording a path names: `(directory, name)` from either of its two files.
pub fn split_path(path: &Path) -> Option<(PathBuf, String)> {
    let dir = path.parent()?.to_path_buf();
    let file = path.file_name()?.to_str()?;
    let name = file
        .strip_suffix(SIDECAR_SUFFIX)
        .or_else(|| file.strip_suffix(AUDIO_SUFFIX))?;
    (!name.is_empty()).then(|| (dir, name.to_owned()))
}

/// Writes `s` as the sidecar of recording `name` in `dir`, atomically.
pub fn write_sidecar(dir: &Path, name: &str, s: &Sidecar) -> Result<(), RawError> {
    let path = sidecar_path(dir, name);
    let bytes = serde_json::to_vec_pretty(s).map_err(|e| RawError::Sidecar {
        path: path.clone(),
        msg: e.to_string(),
    })?;
    crate::session::write_atomic(&path, &bytes).map_err(|e| RawError::Io {
        path,
        msg: e.to_string(),
    })
}

/// Reads the sidecar of recording `name` in `dir`.
pub fn read_sidecar(dir: &Path, name: &str) -> Result<Sidecar, RawError> {
    let path = sidecar_path(dir, name);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(RawError::NotFound(name.to_owned()));
        }
        Err(e) => {
            return Err(RawError::Io {
                path,
                msg: e.to_string(),
            });
        }
    };
    let bad = |msg: String| RawError::Sidecar {
        path: path.clone(),
        msg,
    };
    // The format and version are checked before the rest, so a newer file says so rather
    // than failing on a field this build does not know.
    let probe: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| bad(e.to_string()))?;
    if probe.get("format").and_then(|v| v.as_str()) != Some(FORMAT) {
        return Err(bad(format!("not an {FORMAT} sidecar")));
    }
    let version = probe.get("version").and_then(serde_json::Value::as_u64);
    if version != Some(u64::from(VERSION)) {
        return Err(bad(format!(
            "sidecar version {version:?}; this build reads version {VERSION} only"
        )));
    }
    serde_json::from_slice(&bytes).map_err(|e| bad(e.to_string()))
}

/// A `rec.list` row.
pub fn listing(dir: &Path, name: &str, s: &Sidecar) -> RecordingFile {
    RecordingFile {
        name: name.to_owned(),
        path: dir.join(&s.audio.file).to_string_lossy().into_owned(),
        sample_rate_hz: s.audio.sample_rate,
        inputs: s.audio.channels.iter().map(|c| c.input).collect(),
        frames: s.end.as_ref().map_or(0, |e| e.frames),
        started_at: s.start.wall_ns,
        discontinuities: u32::try_from(s.discontinuities.len()).unwrap_or(u32::MAX),
        end: s.end.as_ref().map(|e| e.reason.clone()),
    }
}

/// Every readable recording in `dir`, oldest first; unreadable sidecars are skipped.
pub fn list(dir: &Path) -> Result<Vec<(String, Sidecar)>, RawError> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(RawError::Io {
                path: dir.to_owned(),
                msg: e.to_string(),
            });
        }
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let file = e.file_name();
        let Some(name) = file.to_str().and_then(|f| f.strip_suffix(SIDECAR_SUFFIX)) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if let Ok(s) = read_sidecar(dir, name) {
            out.push((name.to_owned(), s));
        }
    }
    out.sort_by(|a, b| {
        a.1.start
            .wall_ns
            .0
            .cmp(&b.1.start.wall_ns.0)
            .then(a.0.cmp(&b.0))
    });
    Ok(out)
}

/// Finishes every recording in `dir` whose sidecar has no end and whose audio file has
/// not been written for `quiet`: its writer died. (A file written more recently may belong
/// to another daemon sharing the directory, still recording.) The audio file keeps what
/// reached the disk; the sidecar ends it as [`RecordingEnd::Interrupted`] at its last
/// whole frame. Returns the names finished.
pub fn recover(
    dir: &Path,
    now_wall_ns: u64,
    quiet: std::time::Duration,
) -> Vec<(String, Result<u64, RawError>)> {
    let Ok(all) = list(dir) else {
        return Vec::new();
    };
    let idle = |s: &Sidecar| {
        fs::metadata(dir.join(&s.audio.file))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_none_or(|age| age >= quiet)
    };
    all.into_iter()
        .filter(|(_, s)| s.end.is_none() && idle(s))
        .map(|(name, mut s)| {
            let r = (|| {
                let path = dir.join(&s.audio.file);
                let info = wav::repair(&path).map_err(|err| RawError::Audio {
                    path: path.clone(),
                    err,
                })?;
                let end_sample = session_sample_of(&s, info.frames);
                s.discontinuities.retain(|d| d.frame < info.frames);
                s.end = Some(End {
                    at: Mark::new(end_sample, now_wall_ns),
                    frames: info.frames,
                    reason: RecordingEnd::Interrupted,
                });
                write_sidecar(dir, &name, &s)?;
                Ok(info.frames)
            })();
            (name, r)
        })
        .collect()
}

/// Session sample of audio file frame `frame` (one past the last frame for the end).
pub fn session_sample_of(s: &Sidecar, frame: u64) -> u64 {
    let lost: u64 = s
        .discontinuities
        .iter()
        .filter(|d| d.frame <= frame)
        .map(|d| d.lost_frames)
        .sum();
    s.start.session_sample.0 + frame + lost
}

/// Audio file frame of session sample `sample`: where it was not recorded (lost in a gap),
/// the first recorded frame after it.
pub fn frame_of(start: u64, discontinuities: &[Discontinuity], sample: u64) -> u64 {
    if sample < start {
        return 0;
    }
    let mut frame_base = 0u64;
    let mut sample_base = start;
    let mut next_frame = None;
    for d in discontinuities {
        if d.session_sample.0 > sample {
            next_frame = Some(d.frame);
            break;
        }
        frame_base = d.frame;
        sample_base = d.session_sample.0;
    }
    let f = frame_base + (sample - sample_base);
    // A sample lost in the gap before the next discontinuity maps to the frame after it.
    next_frame.map_or(f, |n| f.min(n))
}

#[cfg(test)]
mod tests;
