//! The clocks entity (`docs/design/multi-rig.md` §4): the capture clock's rate, measured by
//! the fan-out ([`crate::capture_clock`]), beside the system clock's status, read here.

use std::time::{Duration, Instant};

use ac2_proto::Change;
use ac2_proto::model::{Clocks, DeviceClock, SampleClock, StampMap, SystemClock};
use ac2_proto::units::SessionEpoch;

use super::Control;

/// How often the system clock's status is read. The kernel's bound grows by half a
/// millisecond a second, so every read differs: reading more often would only commit more
/// often.
pub(super) const SYSTEM_EVERY: Duration = Duration::from_secs(5);

/// What the clocks entity is made from.
#[derive(Debug)]
pub(super) struct ClockWatch {
    system: SystemClock,
    next_system: Instant,
    /// The newest reading of the capture clock and its stamp map, with the stream they are
    /// of.
    sample: Option<(SessionEpoch, SampleClock, Option<StampMap>)>,
}

impl ClockWatch {
    /// Starting from the system clock as the initial state has it (read when the store was
    /// made): the next read is a period away.
    pub(super) fn new(system: SystemClock) -> Self {
        Self {
            system,
            next_system: Instant::now() + SYSTEM_EVERY,
            sample: None,
        }
    }
}

impl Control {
    /// The fan-out of `epoch` measured the capture clock.
    pub(super) fn sample_clock(
        &mut self,
        epoch: SessionEpoch,
        rate: SampleClock,
        map: Option<StampMap>,
    ) {
        self.clocks.sample = Some((epoch, rate, map));
    }

    /// When the system clock is read next.
    pub(super) fn clocks_due(&self) -> Instant {
        self.clocks.next_system
    }

    /// Reads the system clock when due, and commits the clocks when they changed.
    pub(super) fn clocks_tick(&mut self, now: Instant) {
        if now >= self.clocks.next_system {
            self.clocks.system = self.s.system_clock.read();
            self.clocks.next_system = now + SYSTEM_EVERY;
        }
        let capture = self.session.as_ref().map(|r| DeviceClock {
            device: r.open.input_device.clone(),
            rate: if r.open.replay.is_some() {
                SampleClock::Synthetic
            } else {
                match self.clocks.sample {
                    Some((e, rate, _)) if e == r.epoch => rate,
                    _ => SampleClock::Measuring,
                }
            },
        });
        let c = Clocks {
            system: self.clocks.system,
            capture,
            stamp_map: match (&self.session, self.clocks.sample) {
                (Some(r), Some((e, _, map))) if e == r.epoch && r.open.replay.is_none() => map,
                _ => None,
            },
        };
        if self.store.state().clocks != c {
            self.commit(Change::Clocks(c));
        }
    }
}
