//! A binding to the Core Audio HAL (macOS): devices, their properties and property
//! listeners, and one duplex IOProc per device whose input and output share an exact sample
//! index.
//!
//! The HAL calls an IOProc once per cycle with the input buffers it read and the output
//! buffers to fill, both stamped on the device's one sample-time line. [`Device::start_io`]
//! turns those stamps into one index per cycle ([`CycleTime::index`]) through
//! [`Indexer`]: a jump in the sample time is the exact number of frames lost
//! ([`Gap::Forward`]).
//!
//! ```no_run
//! # #[cfg(target_os = "macos")]
//! # fn main() -> Result<(), cahal::Error> {
//! use cahal::{IoConfig, Scope, System};
//! let dev = System::default_device(Scope::Input)?.ok_or(cahal::Error::NotFound)?;
//! println!("{} at {} Hz", dev.info()?.name, dev.nominal_rate()?);
//! let io = dev.start_io(IoConfig::new(true, false), |cycle| {
//!     // Real-time thread: no allocation, locks or system calls here.
//!     let _first = cycle.input.channel(0).get(0);
//! })?;
//! std::thread::sleep(std::time::Duration::from_secs(1));
//! println!("{:?}", io.stats());
//! io.stop()
//! # }
//! # #[cfg(not(target_os = "macos"))]
//! # fn main() {}
//! ```
//!
//! On other systems the crate holds only its pure part: the data types, [`Indexer`] and
//! [`ChannelTable`] with the cycle views, so code that maps them can be tested anywhere.

// The pure part has no unsafe code; the HAL calls live in four files (`property`,
// `ioproc`, `listener`, `consent`).
#[forbid(unsafe_code)]
mod index;
#[forbid(unsafe_code)]
mod layout;
#[forbid(unsafe_code)]
mod types;

#[cfg(target_os = "macos")]
mod device;
#[cfg(target_os = "macos")]
mod ioproc;
#[cfg(target_os = "macos")]
mod listener;
#[cfg(target_os = "macos")]
mod property;
#[cfg(target_os = "macos")]
mod system;

#[cfg(all(target_os = "macos", feature = "consent"))]
pub mod consent;

pub use index::Indexer;
pub use layout::{Channel, ChannelLoc, ChannelMut, ChannelTable, Cycle, Input, Output};
pub use types::{
    ClockSource, CycleTime, DeviceInfo, DeviceProperty, Error, FourCc, FrameRange, Gap, IoConfig,
    IoStats, Latency, RateRange, Scope, StreamFormat, StreamInfo, StreamLayout, StreamSet,
    SystemProperty, Transport,
};

#[cfg(target_os = "macos")]
pub use device::Device;
#[cfg(target_os = "macos")]
pub use ioproc::IoProc;
#[cfg(target_os = "macos")]
pub use listener::Listener;
#[cfg(target_os = "macos")]
pub use system::System;

/// Converts host-clock ticks (`mHostTime`) to nanoseconds with a factor read once, so a
/// callback converts with arithmetic alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostClock {
    ns_per_tick: f64,
}

impl HostClock {
    /// A clock that ticks `ticks_per_second` times a second.
    pub fn new(ticks_per_second: f64) -> Self {
        Self {
            ns_per_tick: if ticks_per_second > 0.0 {
                1e9 / ticks_per_second
            } else {
                1.0
            },
        }
    }

    /// The machine's host clock (`AudioGetHostClockFrequency`).
    #[cfg(target_os = "macos")]
    pub fn host() -> Self {
        Self::new(property::host_ticks_per_second())
    }

    /// `ticks` in nanoseconds.
    #[inline]
    pub fn ns(&self, ticks: u64) -> u64 {
        (ticks as f64 * self.ns_per_tick) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_clock_scales_ticks() {
        // Apple silicon: 24 MHz.
        let c = HostClock::new(24_000_000.0);
        assert_eq!(c.ns(24_000_000), 1_000_000_000);
        assert_eq!(HostClock::new(1e9).ns(123), 123);
    }
}
