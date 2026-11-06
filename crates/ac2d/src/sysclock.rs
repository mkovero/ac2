//! The system clock's status as the OS states it (`docs/design/multi-rig.md` §4).
//!
//! Linux keeps the discipline daemon's verdict in the kernel: `adjtimex` with no mode bits
//! reads it without changing anything (and needs no privilege). The status word's
//! `STA_UNSYNC` bit says whether the clock is held synchronised; `maxerror` is the kernel's
//! bound on the error, which it grows by 500 µs every second until the discipline daemon
//! resets it, so it is reported as the bound it is and never as the clock's offset. macOS
//! and Windows state neither.

use ac2_proto::model::SystemClock;

/// `STA_UNSYNC` of the `adjtimex` status word (`<linux/timex.h>`).
const STA_UNSYNC: i32 = 0x0040;

/// The status for an `adjtimex` status word and `maxerror` (µs).
pub(crate) fn from_status(status: i32, maxerror_us: i64) -> SystemClock {
    let maxerror_us = u32::try_from(maxerror_us.max(0)).unwrap_or(u32::MAX);
    if status & STA_UNSYNC != 0 {
        SystemClock::Unsynced { maxerror_us }
    } else {
        SystemClock::Synced { maxerror_us }
    }
}

/// The system clock's status now.
#[cfg(target_os = "linux")]
pub(crate) fn read() -> SystemClock {
    // SAFETY: `timex` is plain old data, all zeroes a valid value; `modes == 0` makes the
    // call read-only, and the kernel writes only within the struct passed.
    #[allow(unsafe_code)]
    let (r, t) = unsafe {
        let mut t: libc::timex = std::mem::zeroed();
        let r = libc::adjtimex(&mut t);
        (r, t)
    };
    if r < 0 {
        return SystemClock::NotStated;
    }
    // `c_long` is 32 bits on 32-bit targets (a Pi's armhf), 64 elsewhere.
    #[allow(clippy::useless_conversion)]
    let maxerror = i64::from(t.maxerror);
    from_status(t.status, maxerror)
}

/// The system clock's status now: these OSes state none.
#[cfg(not(target_os = "linux"))]
pub(crate) fn read() -> SystemClock {
    SystemClock::NotStated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_word_says_synced_or_not_and_the_bound_is_kept_as_stated() {
        // What a PTP-disciplined chronyd host states: status 0x0, maxerror 3027 µs.
        assert_eq!(
            from_status(0x0, 3027),
            SystemClock::Synced { maxerror_us: 3027 }
        );
        // STA_UNSYNC with STA_PLL beside it: unsynced whatever else is set.
        assert_eq!(
            from_status(0x0041, 16_000_000),
            SystemClock::Unsynced {
                maxerror_us: 16_000_000
            }
        );
        // Other bits alone (STA_PLL | STA_NANO) leave it synced.
        assert_eq!(
            from_status(0x2001, 504),
            SystemClock::Synced { maxerror_us: 504 }
        );
        assert_eq!(
            from_status(0, -1),
            SystemClock::Synced { maxerror_us: 0 },
            "a negative bound is no bound"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_bit_is_the_kernels_and_reading_changes_nothing() {
        assert_eq!(STA_UNSYNC, libc::STA_UNSYNC);
        // Any host reads either way; the call must not fail for want of privilege.
        assert_ne!(read(), SystemClock::NotStated);
    }
}
