//! What a daemon can say about its clocks (`docs/design/multi-rig.md` §4): what it measures
//! and what the OS states, never anything inferred from them.

use serde::{Deserialize, Serialize};

use super::DeviceId;
use crate::units::Seconds;

/// The host's clocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clocks {
    /// The system clock, as the OS states it.
    pub system: SystemClock,
    /// The capture device's sample clock (every measurement is timed on it); `None` while
    /// no audio I/O is open.
    pub capture: Option<DeviceClock>,
}

/// A device's sample clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceClock {
    /// The device.
    pub device: DeviceId,
    /// Its rate.
    pub rate: SampleClock,
}

/// A sample clock's rate, from the host stamps of the captured blocks over the last
/// minute, said against the clock it was measured against.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SampleClock {
    /// Less than a minute of stamps since the audio opened: no figure yet.
    Measuring,
    /// Measured.
    Measured {
        /// Deviation from the nominal rate, ppm; positive = the device runs fast against
        /// `against`.
        ppm: f64,
        /// Audio the fit spans.
        span: Seconds,
        /// The clock the figure is against.
        against: ClockBasis,
    },
    /// The backend stamps blocks by their sample index (a replayed recording): there is no
    /// device clock to measure.
    Synthetic,
}

/// What a sample clock's measured rate is against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockBasis {
    /// The disciplined system clock: the backend's stamp clock was read beside it over the
    /// same minute and its own rate taken out (Linux `CLOCK_MONOTONIC`).
    SystemClock,
    /// The computer's own clock, undisciplined: the backend stamps on a clock it cannot
    /// read beside a disciplined one (macOS host time, the Windows performance counter),
    /// so the figure includes that clock's own error.
    HostCrystal,
}

/// The system clock's status as the OS states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SystemClock {
    /// The kernel holds the clock synchronised (Linux `adjtimex`: `STA_UNSYNC` clear).
    Synced {
        /// The kernel's maximum error estimate, µs: a bound that grows between updates of
        /// the discipline daemon, not the clock's actual offset.
        maxerror_us: u32,
    },
    /// The kernel says the clock is not synchronised (`STA_UNSYNC` set).
    Unsynced {
        /// The kernel's maximum error estimate, µs.
        maxerror_us: u32,
    },
    /// The OS states nothing (macOS, Windows, or the call failed).
    NotStated,
}

impl Default for Clocks {
    fn default() -> Self {
        Self {
            system: SystemClock::NotStated,
            capture: None,
        }
    }
}
