//! The SPL job's side of the log's runs (`docs/design/protocol-next.md` §7): which second
//! index each completed second gets, when a run ends because the boot clock ran ahead of
//! the audio clock, and when the clock's becoming synchronised re-stamps a run's rows.

use ac2_proto::model::LogClock;

use crate::hostclock::{self, SharedHostClock};
use crate::leq_log::LeqLog;
use crate::leq_log::runs::{RunClock, RunStart};

/// How much further than the audio clock the boot clock may run between two blocks before
/// the run ends there: a suspend or stall the backend did not report as a discontinuity.
pub(super) const DRIFT_MS: u64 = 2000;

/// A run's second grid: the second starting at `sample` is index `index`.
#[derive(Debug, Clone, Copy)]
struct Base {
    run: u32,
    sample: u64,
    index: u64,
}

/// Where the job's seconds stand in the log.
pub(super) struct RunIndex {
    host: SharedHostClock,
    /// Samples per second of the meter's grid.
    per: u64,
    base: Option<Base>,
    /// Boot clock (ms) and end sample at the previous block.
    prev: Option<(u64, u64)>,
    /// The clock of the previous second, to see it become synchronised.
    last_clock: LogClock,
}

/// A completed second's place in the log.
#[derive(Debug, Clone, Copy)]
pub(super) struct Placed {
    pub(super) second: u64,
    pub(super) run: u32,
    pub(super) clock: LogClock,
}

impl RunIndex {
    pub(super) fn new(host: SharedHostClock, per: u64) -> Self {
        Self {
            last_clock: host.clock(),
            host,
            per,
            base: None,
            prev: None,
        }
    }

    /// Starts a run in `log`, its grid's second at `sample` (wall `wall` ns) taking the
    /// run's first index, which is returned.
    pub(super) fn begin(&mut self, log: &mut LeqLog, sample: u64, wall: u64) -> RunStart {
        let now = RunClock {
            boot_ms: hostclock::boot_ms(),
            boot_id: &self.host.boot_id,
            wall_ns: wall,
            clock: self.host.clock(),
        };
        let s = log.begin_run(&now);
        self.base = Some(Base {
            run: s.run,
            sample,
            index: s.first,
        });
        self.last_clock = now.clock;
        s
    }

    /// A block ending at `end_sample` arrived at boot-clock `boot_ms`: whether the boot
    /// clock ran more than [`DRIFT_MS`] further than the audio since the block before.
    pub(super) fn drifted(&mut self, boot_ms: u64, end_sample: u64, fs: f64) -> bool {
        let prev = self.prev.replace((boot_ms, end_sample));
        let Some((b0, s0)) = prev else {
            return false;
        };
        let audio_ms = (end_sample.saturating_sub(s0) as f64 / fs * 1000.0) as u64;
        boot_ms.saturating_sub(b0) > audio_ms + DRIFT_MS
    }

    /// The index of the second starting at `sample` in the open run.
    pub(super) fn index_of(&self, sample: u64) -> u64 {
        self.base.map_or(0, |b| {
            let k = (sample.saturating_sub(b.sample) + self.per / 2) / self.per.max(1);
            b.index + k
        })
    }

    /// The open run.
    pub(super) fn run(&self) -> u32 {
        self.base.map_or(0, |b| b.run)
    }

    /// The second starting at `sample` (wall `wall` ns) completed: issued in `log`, and,
    /// should the clock have become synchronised since the second before, the run's
    /// unsynced rows re-stamped from it.
    pub(super) fn second(&mut self, log: &mut LeqLog, sample: u64, wall: u64) -> Placed {
        let second = self.index_of(sample);
        let run = self.run();
        let clock = self.host.clock();
        if self.last_clock == LogClock::Unsynced && clock == LogClock::Synced {
            log.restamp(run, second, ac2_proto::units::WallNs(wall));
        }
        self.last_clock = clock;
        log.issue(run, second, hostclock::boot_ms());
        Placed { second, run, clock }
    }
}
