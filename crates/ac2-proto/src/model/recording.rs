//! Raw capture files: record requests, recording runs and files, replay.

use serde::{Deserialize, Serialize};

use crate::units::{ClientId, SampleIndex, Seconds, SessionEpoch, WallNs};

/// What `rec.start` records, and where it stops on its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordRequest {
    /// Device inputs to record (zero-based), each captured by the open session; the file's
    /// channels follow this order.
    pub inputs: Vec<u16>,
    /// File name stem in the daemon's recording directory (letters, digits, ` `, `-`, `_`,
    /// `.`); `None` = `rec-<UTC date and time>`. An existing recording is never overwritten.
    pub name: Option<String>,
    /// The recording ends by itself after this much audio (at most
    /// [`RecordRequest::MAX_DURATION_S`]).
    pub max_duration: Seconds,
    /// … or once the file would grow past this many bytes.
    pub max_bytes: Option<u64>,
}

impl RecordRequest {
    /// Longest recording one `rec.start` may ask for: a day.
    pub const MAX_DURATION_S: f64 = 86_400.0;
}

/// Why a recording ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordingEnd {
    /// `rec.stop`.
    Stopped,
    /// The requested duration was reached.
    DurationLimit,
    /// The requested size was reached.
    SizeLimit,
    /// Writing failed (disk full, I/O error); everything before the failure is kept.
    WriteFailed {
        /// What the file system said.
        msg: String,
    },
    /// The audio session closed.
    SessionClosed,
    /// The audio session reopened (device or configuration change, `session.open`).
    SessionReopened,
    /// The session's audio stopped (the device stopped delivering or the host ended the
    /// stream); the file ends with the last audio received, and the audio after the outage,
    /// once the daemon reopens the session, is not spliced onto it.
    AudioStopped,
    /// The daemon shut down.
    DaemonShutdown,
    /// Found unfinished when the daemon started (it was killed or crashed while
    /// recording); the file was finalised from what had reached the disk.
    Interrupted,
}

/// What a recording is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordingStatus {
    /// Writing.
    Recording,
    /// Finalised: the file and its sidecar are complete.
    Ended {
        /// Why.
        reason: RecordingEnd,
    },
}

/// Why the captured audio is not contiguous at a point of a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscontinuityCause {
    /// The audio host reported an xrun.
    Xrun,
    /// The device's sample index jumped (audio lost before it reached the daemon).
    Gap,
    /// The daemon's capture ring overflowed.
    Overflow,
    /// The device's rate, buffer size or routing changed.
    ConfigChange,
    /// The recorder fell behind (the disk was too slow) and audio was dropped before it
    /// reached the file.
    RecorderBehind,
}

/// The latest recording (`rec.start`), mirrored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingRun {
    /// File name stem.
    pub name: String,
    /// Audio file on the daemon host; the sidecar is beside it (`<name>.ac2rec.json`).
    pub path: String,
    /// Device inputs recorded, in file channel order.
    pub inputs: Vec<u16>,
    /// Sample rate.
    pub sample_rate_hz: u32,
    /// Session epoch it records.
    pub session_epoch: SessionEpoch,
    /// Session sample of the file's first frame.
    pub start_sample: SampleIndex,
    /// When it started.
    pub started_at: WallNs,
    /// Client that started it.
    pub started_by: ClientId,
    /// Frames in the file so far (updated about once a second while recording).
    pub frames: u64,
    /// File size so far, bytes.
    pub bytes: u64,
    /// Discontinuities recorded so far.
    pub discontinuities: u32,
    /// Duration bound.
    pub max_duration: Seconds,
    /// Size bound.
    pub max_bytes: Option<u64>,
    /// Status.
    pub status: RecordingStatus,
}

impl RecordingRun {
    /// Still writing.
    pub fn active(&self) -> bool {
        self.status == RecordingStatus::Recording
    }
}

/// A recording in the daemon's recording directory (rows of `rec.list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingFile {
    /// File name stem.
    pub name: String,
    /// Audio file on the daemon host.
    pub path: String,
    /// Sample rate.
    pub sample_rate_hz: u32,
    /// Device inputs recorded, in file channel order.
    pub inputs: Vec<u16>,
    /// Frames in the file.
    pub frames: u64,
    /// When it started.
    pub started_at: WallNs,
    /// Discontinuities in it.
    pub discontinuities: u32,
    /// Why it ended; `None` while it is being written.
    pub end: Option<RecordingEnd>,
}

/// Which recording `session.replay` plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordingRef {
    /// A name in the daemon's recording directory.
    Name {
        /// Name (the file stem).
        name: String,
    },
    /// The audio file's or the sidecar's path on the daemon host (local transports only).
    Path {
        /// Path.
        path: String,
    },
}

/// How fast a replay runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPace {
    /// One second of audio per second, like the device that recorded it.
    Realtime,
    /// As fast as the running measurements take it; no audio is ever dropped.
    Fast,
}

/// The recording an open replay session plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayInfo {
    /// File name stem.
    pub name: String,
    /// Audio file on the daemon host.
    pub path: String,
    /// Frames in the file.
    pub frames: u64,
    /// One past the replay's last sample index: the frames plus the samples the recording
    /// lost in its dropouts (sample indices jump across them, as they did live).
    pub end_sample: SampleIndex,
    /// Pace.
    pub pace: ReplayPace,
    /// Session sample of the file's first frame in the recorded session (replay sample 0).
    pub recorded_start_sample: SampleIndex,
    /// When the recording started.
    pub recorded_at: WallNs,
}
