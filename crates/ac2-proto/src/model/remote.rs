//! Remote inputs (`docs/design/remote-inputs.md`): one rig measuring with another rig's
//! input, the trust between the two rigs and what this rig can say about their clocks.

use serde::{Deserialize, Serialize};

use super::{CalEntry, CalStatus, Clocks, InputSetup, MicCurveRef};
use crate::units::{DaemonIncarnation, RigId, Seconds, SessionEpoch, WallNs};

/// Session channel of remote input slot 0. A remote input is numbered from here rather than
/// after the device's channels, so a device with another channel count never renumbers it
/// under the measurements, panes and recordings that name it.
pub const REMOTE_BASE: u16 = 0x8000;

/// The remote slot of session channel `ch`; `None` for a channel of this rig's device.
pub fn remote_slot(ch: u16) -> Option<u16> {
    ch.checked_sub(REMOTE_BASE)
}

/// An input of another rig captured into this session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteInput {
    /// Its session channel is `REMOTE_BASE + slot` (slot < 0x8000); a slot is never reused
    /// in a session.
    pub slot: u16,
    /// The rig, by id.
    pub rig: RigId,
    /// The rig's name, kept for display while it is unreachable.
    pub rig_name: String,
    /// Its device input channel (zero-based), as that rig numbers it.
    pub channel: u16,
}

impl RemoteInput {
    /// Its session channel.
    pub fn session_channel(&self) -> u16 {
        REMOTE_BASE + self.slot
    }
}

/// Another rig this rig may take audio from (`rig.peers`), as the operator's app paired
/// them: the app held both and gave each the other's key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigPeer {
    /// Its identity.
    pub rig: RigId,
    /// Its name, for display.
    pub name: String,
    /// Its ctrl endpoint (`tcp://host:port`).
    pub ctrl: String,
    /// Its data endpoint (`tcp://host:port`).
    pub data: String,
    /// Its server key as the app pinned it (Z85, 40 characters); never replaced silently.
    pub key: String,
}

/// A host's stamp clock on its system clock (`CLOCK_REALTIME`), fitted over the last minute
/// of paired reads: `system = system_ns + (stamp − epoch_stamp_ns) · (1 + ppm·10⁻⁶)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StampMap {
    /// The stamp the line is anchored at, ns on the stamp clock.
    pub epoch_stamp_ns: u64,
    /// System time at that stamp, ns since the Unix epoch.
    pub system_ns: u64,
    /// The system clock's rate against the stamp clock, ppm.
    pub ppm: f64,
}

impl StampMap {
    /// System time of `stamp_ns`, ns since the Unix epoch.
    pub fn system_of(&self, stamp_ns: u64) -> f64 {
        let dx = stamp_ns as f64 - self.epoch_stamp_ns as f64;
        self.system_ns as f64 + dx * (1.0 + self.ppm * 1e-6)
    }

    /// Stamp-clock time of `system_ns`, ns.
    pub fn stamp_of(&self, system_ns: f64) -> f64 {
        let dy = system_ns - self.system_ns as f64;
        self.epoch_stamp_ns as f64 + dy / (1.0 + self.ppm * 1e-6)
    }
}

/// One run of a rig's capture: a new one on every session reopen or daemon start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamIncarnation {
    /// The daemon process.
    pub daemon: DaemonIncarnation,
    /// Its session.
    pub epoch: SessionEpoch,
}

/// What a rig tells a rig admitted to it (`source.state`): its clocks, and what its session
/// captures with each input's setup and calibration. Nothing else of its state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceState {
    /// The rig.
    pub rig: RigId,
    /// Its name.
    pub name: String,
    /// Its clocks, with its stamp map.
    pub clocks: Clocks,
    /// Its session; `None` while it has none.
    pub session: Option<SourceSession>,
}

/// A source rig's session as a rig taking its audio sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSession {
    /// The capture's run.
    pub incarnation: StreamIncarnation,
    /// Rate, Hz.
    pub sample_rate_hz: u32,
    /// Device period, frames.
    pub buffer_frames: u32,
    /// It replays a recording: its sample index is the file's, no device clock behind it.
    pub replaying: bool,
    /// Its backend stamps each block's capture time (JACK, Core Audio, fake, replay).
    pub capture_stamps: bool,
    /// The device inputs it captures, each with what is on it.
    pub inputs: Vec<SourceInput>,
}

/// A captured input of a source rig.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceInput {
    /// Device input channel (zero-based).
    pub channel: u16,
    /// What is on it, as that rig declares it; `None`: undeclared.
    pub setup: Option<InputSetup>,
    /// Which calibration applies there.
    pub cal_status: CalStatus,
    /// The sensitivity calibration applied; `None`: dBFS.
    pub calibration: Option<CalEntry>,
    /// The mic curve applied, with its points.
    pub curve: Option<SourceCurve>,
}

/// A mic curve with its points, so a rig taking the input corrects as its own rig does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCurve {
    /// Which curve.
    pub reference: MicCurveRef,
    /// Point frequencies, Hz, ascending.
    pub freq: Vec<f64>,
    /// Gain at each, dB.
    pub gain_db: Vec<f64>,
}

/// Remote inputs as this rig runs them: the rigs it may take audio from, each remote rig of
/// the session and each remote input.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteState {
    /// The rigs paired with this one (`rig.peers`).
    pub peers: Vec<RigPeer>,
    /// The rigs the open session takes inputs from.
    pub rigs: Vec<RemoteRig>,
    /// The open session's remote inputs, in its `remote_inputs` order.
    pub inputs: Vec<RemoteInputStatus>,
    /// How long audio meeting a remote input waits for it; `None` without a remote input or
    /// before the source rig's period is known.
    pub link_offset: Option<Seconds>,
}

/// A rig the session takes inputs from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteRig {
    /// The rig.
    pub rig: RigId,
    /// Its name.
    pub name: String,
    /// The connection to it.
    pub link: RemoteLink,
    /// The clocks between the two rigs.
    pub clock: PeerClock,
    /// Audio received from it, bytes per second over the last few seconds.
    pub bytes_per_s: f64,
    /// The data window this rig grants it, bytes.
    pub window: u32,
}

/// The connection to a remote rig.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteLink {
    /// This rig is not paired with it (no `rig.peers` entry): pair the rigs from the app.
    NotPaired,
    /// This rig listens only on its own computer: it has no key to show another rig.
    NotNetworked,
    /// Connecting.
    Connecting,
    /// Connected and answering.
    Up,
    /// Not answering since `since`.
    Unreachable {
        /// When it last answered (or when connecting started).
        since: WallNs,
    },
    /// It refused this rig's key: it has not admitted this rig.
    NotAdmitted,
    /// It answers with another key than the one paired: pair the rigs again from the app.
    KeyChanged,
    /// Another rig answers at the paired address.
    OtherRig {
        /// The rig that answered.
        found: RigId,
    },
}

/// A remote input as this rig runs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteInputStatus {
    /// Its slot (session channel `REMOTE_BASE + slot`).
    pub slot: u16,
    /// Its audio.
    pub audio: RemoteAudio,
    /// How its samples are placed against this rig's; `None` before the first frame.
    pub anchor: Option<Anchor>,
    /// Frames that arrived after their samples were needed, since the stream started.
    pub late_frames: u64,
    /// Of those, the ones in the last hour.
    pub late_frames_hour: u32,
    /// Times the map between the two rigs' sample indices slipped since the stream started.
    pub slips: u32,
    /// The last slip, samples (positive: the remote samples moved later).
    pub last_slip: Option<i64>,
    /// How early remote audio arrived before it was needed, the least over the last minute.
    pub arrival_margin: Option<Seconds>,
    /// What is on it, as its rig declares it.
    pub setup: Option<InputSetup>,
    /// Which calibration its rig applies.
    pub cal_status: CalStatus,
    /// The sensitivity calibration its rig applies.
    pub calibration: Option<CalEntry>,
    /// The mic curve its rig applies.
    pub curve: Option<MicCurveRef>,
}

/// Where a remote input's audio stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteAudio {
    /// Its rig has not answered yet.
    Waiting,
    /// The input is on this rig (a session file loaded on its own source): no audio.
    OnThisRig,
    /// Its rig has no session.
    NoSession,
    /// Its rig's session does not capture that input.
    NotCaptured,
    /// Its rig replays a recording: no device clock to pair with.
    Replaying,
    /// Its rig runs another rate.
    RateMismatch {
        /// That rig's rate, Hz.
        theirs: u32,
        /// This rig's rate, Hz.
        ours: u32,
    },
    /// Subscribed; no frame yet.
    Starting,
    /// Audio arrives.
    Streaming,
}

/// How a remote input's samples are placed against this rig's (§4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Anchor {
    /// On system time: samples captured at the same system time are paired.
    SystemTime,
    /// On arrival: a block is due a link offset after it arrives.
    Arrival {
        /// Why not on system time.
        reason: ArrivalReason,
    },
}

/// Why a remote input is anchored on arrival.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArrivalReason {
    /// A system clock is not stated synced.
    SystemClock,
    /// The source stamps no capture times.
    NoCaptureStamps,
    /// A stamp map is not fitted yet.
    NoStampMap,
    /// Placed on system time its audio would fall outside the link offset of its arrival:
    /// the two system clocks disagree by about this much (positive: the source's ahead).
    ClocksDisagree {
        /// The disagreement.
        by: Seconds,
    },
}

/// The clocks between this rig and a remote rig (§3.1), as the evidence shows them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerClock {
    /// The relation.
    pub relation: PeerRelation,
    /// The remote rig's sample clock against this rig's, through both system clocks, ppm
    /// over the longest window measured (positive: the remote rig runs fast).
    pub ppm: Option<f64>,
    /// The audio that figure spans.
    pub span: Option<Seconds>,
    /// When the relation last changed.
    pub last_change: Option<WallNs>,
}

/// The relation between two rigs' clocks. Never *locked*: stamps resolve about 2 ppm a
/// minute, which two free crystals can meet.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerRelation {
    /// Less than a minute of stream since it started or re-anchored.
    Measuring,
    /// Both system clocks synced, both rigs' own clock lines within ±2 ppm, and the cross-rig
    /// rate within ±2 ppm over a minute and ±3 ppm over 40 s.
    Agree,
    /// The rates differ beyond those bounds (held until a minute clean).
    NotLocked {
        /// Why.
        reason: NotLockedReason,
    },
    /// The evidence cannot say.
    CantTell {
        /// Why.
        reason: CantTellReason,
    },
}

/// Why two rigs read not locked.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotLockedReason {
    /// The two sample clocks differ, seen through the two system clocks.
    CrossRate {
        /// Remote against this rig, ppm.
        ppm: f64,
        /// Over this window.
        over: Seconds,
    },
    /// One rig's interface runs off its own disciplined host clock.
    OwnClock {
        /// Which rig.
        rig: RigId,
        /// Its rate against its system clock, ppm.
        ppm: f64,
    },
}

/// Why the relation cannot be told.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CantTellReason {
    /// A rig's system clock is not synced.
    SystemClockUnsynced {
        /// Which rig.
        rig: RigId,
    },
    /// A rig's OS states nothing about its system clock (macOS, Windows).
    SystemClockNotStated {
        /// Which rig.
        rig: RigId,
    },
    /// A rig's sample clock is measured against its own crystal, not its system clock.
    NotAgainstSystemClock {
        /// Which rig.
        rig: RigId,
    },
    /// The source stamps no capture times (ASIO).
    NoCaptureStamps,
    /// The two system clocks disagree by more than the link offset allows.
    ClocksDisagree {
        /// By about this much (positive: the source's ahead).
        by: Seconds,
    },
    /// No audio from the remote rig.
    NoAudio,
}
