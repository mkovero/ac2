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
    /// The capture device's sample clock against the system clock (every measurement is
    /// timed on it); `None` while no audio I/O is open.
    pub capture: Option<DeviceClock>,
}

/// A device's sample clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceClock {
    /// The device.
    pub device: DeviceId,
    /// Its rate against the system clock.
    pub rate: SampleClock,
}

/// A sample clock's rate against the system clock, from the host stamps of the captured
/// blocks over the last minute.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SampleClock {
    /// Less than a minute of stamps since the audio opened: no figure yet.
    Measuring,
    /// Measured.
    Measured {
        /// Deviation from the nominal rate, ppm; positive = the device runs fast against
        /// the system clock.
        ppm: f64,
        /// Audio the fit spans.
        span: Seconds,
    },
    /// The backend stamps blocks by their sample index (a replayed recording): there is no
    /// device clock to measure.
    Synthetic,
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
