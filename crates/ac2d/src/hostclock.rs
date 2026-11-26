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

/// Whether a `TZ` value names an IANA zone (`Europe/Helsinki`, `UTC`, `EST5EDT`) rather than
/// a POSIX rule (`EET-2EEST,M3.5.0/3,M10.5.0/4`, `EST5`), in the zone database's directories.
fn iana_name(tz: &str) -> Option<&str> {
    iana_name_in(tz, &zoneinfo_dirs())
}

/// Where the C library looks a `TZ` name up: `TZDIR`, else the usual places.
fn zoneinfo_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("TZDIR")
        .into_iter()
        .map(Into::into)
        .collect();
    dirs.extend(
        [
            "/usr/share/zoneinfo",
            "/var/db/timezone/zoneinfo",
            "/usr/lib/zoneinfo",
        ]
        .map(std::path::PathBuf::from),
    );
    dirs
}

/// [`iana_name`] against the zone files in `dirs`. The C library takes a value as a zone
/// when a zone file of that name exists, and a rule and a zone name can be spelled alike
/// (`EST5EDT` is both); a value naming no zone file is a rule, or no zone at all (`Foo`,
/// which the C library reads as UTC): the header then gives the offset, never the value.
fn iana_name_in<'a>(tz: &'a str, dirs: &[std::path::PathBuf]) -> Option<&'a str> {
    let n = tz.strip_prefix(':').unwrap_or(tz);
    let shaped = !n.is_empty()
        && !n.starts_with('/')
        && n.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '+' | '-'));
    if !shaped {
        return None;
    }
    dirs.iter().any(|d| d.join(n).is_file()).then_some(n)
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

    /// A zone database of its own: the tests never depend on the host's.
    fn db(zones: &[&str]) -> tempfile::TempDir {
        let db = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        for z in zones {
            let p = db.path().join(z);
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).unwrap_or_else(|e| panic!("{e}"));
            }
            std::fs::write(p, b"TZif").unwrap_or_else(|e| panic!("{e}"));
        }
        db
    }

    #[test]
    fn a_tz_names_a_zone_or_a_rule() {
        let db = db(&["Europe/Helsinki", "America/Port-au-Prince", "UTC"]);
        let dirs = [db.path().to_path_buf()];
        assert_eq!(
            iana_name_in("Europe/Helsinki", &dirs),
            Some("Europe/Helsinki")
        );
        assert_eq!(
            iana_name_in(":America/Port-au-Prince", &dirs),
            Some("America/Port-au-Prince")
        );
        assert_eq!(iana_name_in("UTC", &dirs), Some("UTC"));
        assert_eq!(iana_name_in("EET-2EEST,M3.5.0/3,M10.5.0/4", &dirs), None);
        assert_eq!(iana_name_in("/usr/share/zoneinfo/UTC", &dirs), None);
        assert_eq!(iana_name_in("<+0330>-3:30", &dirs), None);
    }

    /// Every name is a zone only when the database has a file of that name, as the C
    /// library reads it: the same spelling without one is a rule, and a name without
    /// digits and without a file is no zone at all.
    #[test]
    fn a_zone_name_is_found_in_the_database() {
        let db = db(&["EST5EDT", "Etc/GMT+5", "Etc/GMT-14", "Europe/Helsinki"]);
        let dirs = [db.path().to_path_buf()];
        for z in ["EST5EDT", "Etc/GMT+5", "Etc/GMT-14", "Europe/Helsinki"] {
            assert_eq!(iana_name_in(z, &dirs), Some(z));
        }
        assert_eq!(iana_name_in(":EST5EDT", &dirs), Some("EST5EDT"));
        assert_eq!(iana_name_in("EST5", &dirs), None);
        assert_eq!(iana_name_in("CST6CDT", &dirs), None);
        assert_eq!(iana_name_in("../EST5EDT", &dirs), None);
        assert_eq!(iana_name_in("Foo", &dirs), None);
        assert_eq!(iana_name_in("Europe/Atlantis", &dirs), None);
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
