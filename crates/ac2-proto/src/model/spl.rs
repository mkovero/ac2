//! SPL log: Leq window and peak states, alarms, log rows and pages, history.

use serde::{Deserialize, Serialize};

use super::{LeqJudgement, LeqWindow, LevelScale, PeakQuantity, PositionCorrection, Weighting};
use crate::units::{Db, DbSpl, Dbfs, Hz, MeasId, Seconds, WallNs};

/// The per-second log of an SPL meter and the state of its Leq windows
/// (`docs/design/leq.md`). Changes only when a window's judgement changes, the windows
/// change or the log starts; values arrive in `leq` frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplLog {
    /// SPL measurement.
    pub meas: MeasId,
    /// Wall time of the oldest second held; `None` before the first.
    pub started_at: Option<WallNs>,
    /// Each configured window's state, in configuration order.
    pub windows: Vec<LeqWindowState>,
    /// The peak limits' states.
    pub peaks: PeakStates,
    /// Over and recovered events, oldest first (the newest [`SplLog::MAX_ALARMS`]).
    pub alarms: Vec<LeqAlarm>,
}

/// State of a peak limit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeqPeakState {
    /// Current judgement (`no_limit` without a limit).
    pub judgement: LeqJudgement,
    /// When the judgement began.
    pub since: WallNs,
}

/// The states of an SPL meter's peak limits.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeakStates {
    /// LCpeak.
    pub lcpeak: LeqPeakState,
    /// LAFmax.
    pub lafmax: LeqPeakState,
}

impl PeakStates {
    /// The state of `q`.
    pub fn get(&self, q: PeakQuantity) -> LeqPeakState {
        match q {
            PeakQuantity::LcPeak => self.lcpeak,
            PeakQuantity::LafMax => self.lafmax,
        }
    }

    /// The state of `q`, to change.
    pub fn get_mut(&mut self, q: PeakQuantity) -> &mut LeqPeakState {
        match q {
            PeakQuantity::LcPeak => &mut self.lcpeak,
            PeakQuantity::LafMax => &mut self.lafmax,
        }
    }
}

impl SplLog {
    /// Alarms kept.
    pub const MAX_ALARMS: usize = 100;
}

/// State of one Leq window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeqWindowState {
    /// Window length.
    pub duration: Seconds,
    /// Window weighting.
    pub weighting: Weighting,
    /// Current judgement.
    pub judgement: LeqJudgement,
    /// When the judgement began.
    pub since: WallNs,
}

/// What happened to a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeqAlarmKind {
    /// The window went over its limit.
    Over,
    /// The window came back to or below its limit.
    Recovered,
}

/// What an alarm is about.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AlarmSubject {
    /// A rolling Leq window.
    Window {
        /// Window length.
        duration: Seconds,
        /// Window weighting.
        weighting: Weighting,
    },
    /// A peak limit.
    Peak {
        /// LCpeak or LAFmax.
        quantity: PeakQuantity,
    },
    /// A 1/3-octave band of a band window of the band meter (its limit at the mic).
    Band {
        /// Window length.
        duration: Seconds,
        /// Window weighting.
        weighting: Weighting,
        /// Nominal centre.
        nominal: Hz,
    },
    /// The band meter's predicted LAeq window at the transfer's place.
    Predicted,
}

impl AlarmSubject {
    /// A window's length; `None` for a peak limit.
    pub fn duration(&self) -> Option<Seconds> {
        match self {
            AlarmSubject::Window { duration, .. } => Some(*duration),
            AlarmSubject::Peak { .. } | AlarmSubject::Band { .. } | AlarmSubject::Predicted => None,
        }
    }
}

/// A window or a peak limit going over its limit, or recovering.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeqAlarm {
    /// When (wall time of the end of the second that decided it).
    pub at: WallNs,
    /// The window or the peak limit.
    pub subject: AlarmSubject,
    /// Over or recovered.
    pub kind: LeqAlarmKind,
    /// The level judged then (a window's Leq, a peak limit's highest second within its
    /// hold), with `position` added.
    pub level: DbSpl,
    /// Its limit.
    pub limit: DbSpl,
    /// The measuring-position correction included in `level` (dB), if any.
    pub position: Option<Db>,
}

/// What the wall time of a log row rests on (`docs/design/protocol-next.md` §7): an
/// annotation, never the row's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogClock {
    /// The OS held the system clock synchronised.
    Synced,
    /// The OS said the clock was not synchronised.
    Unsynced,
    /// Logged unsynced, re-stamped from the boot-clock distance to the point where the
    /// clock became synchronised in the same run.
    Derived,
    /// The OS states nothing about its clock (macOS, Windows, a failed `adjtimex`).
    NotStated,
}

impl LogClock {
    /// Whether a wall time stamped so may measure the distance across a reboot: a
    /// desktop OS keeps its clock by network time without saying so, and a standalone
    /// meter runs on Linux, which states it.
    pub fn trusted(self) -> bool {
        !matches!(self, Self::Unsynced)
    }
}

/// How the seconds between a run's first row and the row before it are known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogGap {
    /// Measured on the boot clock or a trusted wall clock: the index counts them.
    Known,
    /// Not known (a reboot with an untrusted clock, a lost high-water mark): the index
    /// moves on by one and no window spans it.
    Unknown,
}

/// One second of an SPL meter's log.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplLogRow {
    /// The row's key: an index into the log that only rises; within a run it advances by
    /// the seconds the audio clock counted.
    pub second: u64,
    /// The run (one stretch of the meter on one open stream) the row is of, rising.
    pub run: u32,
    /// Wall time of the second's first sample, as read (an annotation).
    pub start: WallNs,
    /// What `start` rests on.
    pub clock: LogClock,
    /// On a run's first row, how the distance to the row before is known; `None` on the
    /// others.
    pub gap: Option<LogGap>,
    /// Time measured within the second (< 1 s next to a capture gap).
    pub measured: Seconds,
    /// LAeq over the measured time.
    pub laeq: Dbfs,
    /// LCeq over the measured time.
    pub lceq: Dbfs,
    /// LZeq over the measured time.
    pub lzeq: Dbfs,
    /// Highest C-weighted peak of the second.
    pub lcpeak: Dbfs,
    /// Highest A-weighted Fast level of the second.
    pub lafmax: Dbfs,
    /// Sensitivity in force (dB SPL of 0 dBFS); `None` uncalibrated.
    pub sensitivity: Option<Db>,
    /// Measuring-position correction in force: not in the levels (a row is what was
    /// measured); `None` without one or uncalibrated.
    pub position: Option<PositionCorrection>,
}

/// Which of an SPL meter's logs `spl.log_get` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplLogWhich {
    /// The log the meter is writing.
    Current,
    /// The log `spl.log_new` ended last (kept in memory until the next one).
    Previous,
}

/// Rows of an SPL meter's log (`spl.log_get`). Rows are numbered from the first second the
/// meter logged; the oldest are dropped after [`SplLogPage::RETAINED_ROWS`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplLogPage {
    /// SPL measurement.
    pub meas: MeasId,
    /// When the log started: a log whose rows may differ from what a client cached (a new
    /// log, a high-water mark lost) starts anew with a new one, and a downloader that sees
    /// it change mid-download starts over.
    pub started_at: WallNs,
    /// Number of the first row returned.
    pub from: u64,
    /// Rows logged so far (one past the newest row's number).
    pub total: u64,
    /// The rows.
    pub rows: Vec<SplLogRow>,
}

impl SplLogPage {
    /// Rows a meter keeps: 48 h.
    pub const RETAINED_ROWS: usize = 48 * 3600;
    /// Most rows one reply carries.
    pub const MAX_ROWS: u32 = 20_000;
}

/// Which seconds of an SPL meter's log `spl.history_get` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistorySpan {
    /// The newest `seconds` (at most [`SplHistory::MAX_POINTS`], else refused), a point a
    /// second.
    Newest {
        /// Seconds wanted.
        seconds: u32,
    },
    /// The seconds whose index lies in `from ..= to`, anywhere in the log, a point per
    /// `step_s` seconds: buckets of the whole multiples of `step_s` on the index
    /// (`second / step_s`), each window's highest Leq in the bucket and over when it was
    /// over in any second of it (a limit is about the worst moment). At most
    /// [`SplHistory::MAX_POINTS`] buckets, else refused.
    Range {
        /// First second index (inclusive).
        from: u64,
        /// Last second index (inclusive).
        to: u64,
        /// Seconds per point, ≥ 1 (1: each second as the frames carried it).
        step_s: u32,
    },
}

/// Each Leq window of an SPL meter second by second (or bucket by bucket), as its `leq`
/// frames carried them (`spl.history_get`): what a client that was not connected missed, or
/// any span of the log looked back at, computed by the daemon from the meter's current log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplHistory {
    /// SPL measurement.
    pub meas: MeasId,
    /// The windows the series are of: the meter's, in configuration order.
    pub windows: Vec<LeqWindow>,
    /// Unit of `leq`: the meter's as of the newest second returned (the series start after
    /// the last change of unit).
    pub scale: LevelScale,
    /// Seconds per point: 1, or the `step_s` of a [`HistorySpan::Range`].
    pub step_s: u32,
    /// Index of the oldest second the log holds; `None` for an empty log.
    pub first_second: Option<u64>,
    /// Index of the newest second the log holds; `None` for an empty log.
    pub last_second: Option<u64>,
    /// Wall start of the oldest second held, as logged (an annotation of `first_second`).
    pub log_start: Option<WallNs>,
    /// Wall end of the newest second held, as logged (an annotation of `last_second`).
    pub log_until: Option<WallNs>,
    /// Index of each second (of each bucket's last row, with `step_s` > 1), oldest first.
    /// A second without a row (nothing measured) has none, as no frame was sent for it; a
    /// bucket without any row neither.
    pub second: Vec<u64>,
    /// Wall end of each point's second, as logged (an annotation beside `second`).
    pub at: Vec<WallNs>,
    /// Per window (as `windows`), its Leq at each point of `at` in `scale` (the highest in
    /// the bucket); NaN when nothing was measured in the window.
    pub leq: Vec<Vec<f32>>,
    /// Per window, whether it was over its limit at each point of `at` (in any second of
    /// the bucket).
    pub over: Vec<Vec<bool>>,
}

impl SplHistory {
    /// Most points a reply carries, either span: 4 h a second at a time, which with every
    /// window a meter may have still fits one ctrl message.
    pub const MAX_POINTS: u32 = 4 * 3600;
}
