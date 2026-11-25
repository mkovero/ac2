//! What the SPL logs take from the host's clocks (`docs/design/protocol-next.md` §7): a
//! boot clock that counts suspend, the boot's id, whether an RTC set the clock at boot, the
//! time zone, and the system clock's status as the meter jobs read it.
//!
//! `Instant` stops during suspend on Linux and macOS, so a laptop that sleeps for hours and
//! reopens its stream in the same process would see seconds; the boot clock sees hours.

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use ac2_proto::model::{LogClock, SystemClock, TimeZone};

use crate::config::LocalClock;

/// Milliseconds since boot, suspend included: `CLOCK_BOOTTIME` on Linux.
#[cfg(target_os = "linux")]
pub(crate) fn boot_ms() -> u64 {
    timespec_ms(libc::CLOCK_BOOTTIME)
}

/// Milliseconds since boot, suspend included: Apple's `CLOCK_MONOTONIC`, which keeps
/// counting while the system sleeps (the clock `mach_continuous_time` reads).
#[cfg(target_os = "macos")]
pub(crate) fn boot_ms() -> u64 {
    timespec_ms(libc::CLOCK_MONOTONIC)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn timespec_ms(clock: libc::clockid_t) -> u64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `t` is a valid, writable timespec; the call writes only within it.
    #[allow(unsafe_code)]
    let r = unsafe { libc::clock_gettime(clock, &mut t) };
    if r != 0 {
        return 0;
    }
    let secs = u64::try_from(t.tv_sec).unwrap_or(0);
    let ns = u64::try_from(t.tv_nsec).unwrap_or(0);
    secs * 1000 + ns / 1_000_000
}

/// Milliseconds since boot, suspend included: the interrupt time (100 ns units), which
/// counts sleep and hibernation.
#[cfg(windows)]
pub(crate) fn boot_ms() -> u64 {
    let mut t = 0u64;
    // SAFETY: `t` is a valid, writable u64; the call writes only it.
    #[allow(unsafe_code)]
    unsafe {
        windows_sys::Win32::System::WindowsProgramming::QueryInterruptTime(&mut t);
    }
    t / 10_000
}

/// Any other OS: the process's monotonic clock, which may stop during suspend.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) fn boot_ms() -> u64 {
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    let ms = T0
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis();
    u64::try_from(ms).unwrap_or(u64::MAX)
}

/// The boot's id: the kernel's on Linux; elsewhere one made per daemon process, so a new
/// process counts as a new boot there (its boot clock readings are not compared with an
/// earlier process's).
pub(crate) fn boot_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        #[cfg(target_os = "linux")]
        if let Ok(s) = std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
            let s = s.trim();
            if !s.is_empty() {
                return s.to_owned();
            }
        }
        format!("process-{:016x}", crate::util::random_u64())
    })
    .clone()
}

/// Whether the kernel set the system clock from an RTC at boot: some
/// `/sys/class/rtc/rtc*/hctosys` reads 1.
#[cfg(target_os = "linux")]
pub(crate) fn rtc() -> bool {
    std::fs::read_dir("/sys/class/rtc").is_ok_and(|rd| {
        rd.flatten().any(|e| {
            std::fs::read_to_string(e.path().join("hctosys")).is_ok_and(|s| s.trim() == "1")
        })
    })
}

/// macOS and Windows hosts keep their clock in a battery-backed RTC.
#[cfg(not(target_os = "linux"))]
pub(crate) fn rtc() -> bool {
    true
}

/// Whether a `TZ` value names an IANA zone (`Europe/Helsinki`, `UTC`) rather than a POSIX
/// rule (`EET-2EEST,M3.5.0/3,M10.5.0/4`), which carries offsets as digits.
fn iana_name(tz: &str) -> Option<&str> {
    let n = tz.strip_prefix(':').unwrap_or(tz);
    if n.is_empty() || n.starts_with(['/', '<']) {
        return None;
    }
    (n.starts_with("Etc/") || !n.contains(|c: char| c.is_ascii_digit() || c == ',')).then_some(n)
}

/// The host's zone now under `local`: the IANA name when the host has one (`TZ` when it
/// names one, else the system's), nil when it gives only a POSIX rule; the offset in force.
pub(crate) fn zone(local: LocalClock, wall_ns: u64) -> TimeZone {
    let utc_offset_s = local.offset_s(wall_ns);
    let name = match local {
        LocalClock::FixedOffset { .. } => None,
        LocalClock::Host => match std::env::var("TZ") {
            Ok(tz) if !tz.is_empty() => iana_name(&tz).map(str::to_owned),
            _ => iana_time_zone::get_timezone().ok(),
        },
    };
    TimeZone { name, utc_offset_s }
}

/// A log row's clock for the system clock's status.
pub(crate) fn log_clock(c: SystemClock) -> LogClock {
    match c {
        SystemClock::Synced { .. } => LogClock::Synced,
        SystemClock::Unsynced { .. } => LogClock::Unsynced,
        SystemClock::NotStated => LogClock::NotStated,
    }
}

/// What the meter jobs read of the host's clocks once a second: the control thread writes
/// it when it reads the clocks, the jobs read it without a lock.
#[derive(Debug)]
pub(crate) struct HostClock {
    system: AtomicU8,
    /// An RTC set the clock at boot (read once, at start).
    pub(crate) rtc: bool,
    /// The boot's id.
    pub(crate) boot_id: String,
    /// Local time, for the band limits' day and night.
    pub(crate) local: LocalClock,
}

/// The handle jobs hold.
pub(crate) type SharedHostClock = Arc<HostClock>;

fn clock_code(c: LogClock) -> u8 {
    match c {
        LogClock::Synced => 0,
        LogClock::Unsynced => 1,
        LogClock::Derived => 2,
        LogClock::NotStated => 3,
    }
}

impl HostClock {
    /// The clocks as read now.
    pub(crate) fn new(system: SystemClock, rtc: bool, local: LocalClock) -> Self {
        Self {
            system: AtomicU8::new(clock_code(log_clock(system))),
            rtc,
            boot_id: boot_id(),
            local,
        }
    }

    /// The system clock's status as last read.
    pub(crate) fn clock(&self) -> LogClock {
        match self.system.load(Ordering::Relaxed) {
            0 => LogClock::Synced,
            1 => LogClock::Unsynced,
            2 => LogClock::Derived,
            _ => LogClock::NotStated,
        }
    }

    /// Takes a new reading of the system clock's status.
    pub(crate) fn set_system(&self, c: SystemClock) {
        self.system
            .store(clock_code(log_clock(c)), Ordering::Relaxed);
    }

    /// Whether the time of day is unknown: the clock unsynced on a host whose clock no RTC
    /// set at boot (a meter cut every night restarts its clock from the last save and falls
    /// behind by each night's off time).
    pub(crate) fn time_of_day_unknown(&self) -> bool {
        self.clock() == LogClock::Unsynced && !self.rtc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tz_names_a_zone_or_a_rule() {
        assert_eq!(iana_name("Europe/Helsinki"), Some("Europe/Helsinki"));
        assert_eq!(
            iana_name(":America/Port-au-Prince"),
            Some("America/Port-au-Prince")
        );
        assert_eq!(iana_name("UTC"), Some("UTC"));
        assert_eq!(iana_name("Etc/GMT+2"), Some("Etc/GMT+2"));
        assert_eq!(iana_name("EET-2EEST,M3.5.0/3,M10.5.0/4"), None);
        assert_eq!(iana_name("/usr/share/zoneinfo/UTC"), None);
    }

    #[test]
    fn the_boot_clock_runs_and_the_boot_has_one_id() {
        let a = boot_ms();
        assert!(a > 0);
        assert!(boot_ms() >= a);
        assert_eq!(boot_id(), boot_id());
    }

    #[test]
    fn the_time_of_day_is_unknown_only_unsynced_without_an_rtc() {
        let unsynced = SystemClock::Unsynced { maxerror_us: 1 };
        let h = HostClock::new(unsynced, false, LocalClock::Host);
        assert!(h.time_of_day_unknown());
        h.set_system(SystemClock::Synced { maxerror_us: 1 });
        assert!(!h.time_of_day_unknown());
        assert!(!HostClock::new(unsynced, true, LocalClock::Host).time_of_day_unknown());
        let ns = HostClock::new(SystemClock::NotStated, false, LocalClock::Host);
        assert!(!ns.time_of_day_unknown());
    }
}
