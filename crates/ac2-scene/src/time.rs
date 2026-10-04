//! Frame age and freshness (decision 2a), from times the caller provides.
//!
//! Frames carry `capture_wall_ns` in the daemon's clock. The client keeps an estimate of
//! `daemon − client` from keepalives (`ka.daemon_wall_ns`); the age of a frame is then
//! `client_now + offset − capture`. Nothing here reads a clock.

use ac2_proto::units::WallNs;

/// A frame older than this is STALE: its trace dims and shows its age. Strictly greater:
/// a frame exactly 1 s old is still fresh.
pub const STALE_AFTER_S: f64 = 1.0;

/// No keepalive for longer than this → DAEMON NOT RESPONDING. Keepalives come every
/// 250 ms; 1.5 s is six missed ones, the same span as the stimulus dead-man timeout.
pub const DAEMON_SILENT_AFTER_S: f64 = 1.5;

/// Estimated `daemon clock − client clock`, nanoseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockOffset(pub i64);

/// The client's "now" expressed in the daemon's clock.
pub fn daemon_now(client_now: WallNs, offset: ClockOffset) -> i128 {
    i128::from(client_now.0) + i128::from(offset.0)
}

/// Seconds between `at` (daemon clock) and `client_now` (+ offset). Clamped at zero: a
/// negative age is offset-estimate error, not a frame from the future.
pub fn age_s(at: WallNs, client_now: WallNs, offset: ClockOffset) -> f64 {
    let ns = daemon_now(client_now, offset) - i128::from(at.0);
    (ns.max(0) as f64) / 1e9
}

/// `y`, `m` (1–12), `d` of a day count from 1970-01-01 (civil-from-days, proleptic
/// Gregorian).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The local time of day of `at` (`4:01`), with its date when that is not `today`'s local
/// day (`2 Oct 4:01`). `offset_s` gives the local UTC offset (s) in force at a wall time;
/// both times are on the same clock.
pub fn local_clock(at: WallNs, today: WallNs, offset_s: impl Fn(WallNs) -> i32) -> String {
    let local = |t: WallNs| (t.0 / 1_000_000_000) as i64 + i64::from(offset_s(t));
    let (a, t) = (local(at), local(today));
    let tod = a.rem_euclid(86_400);
    let hm = format!("{}:{:02}", tod / 3600, (tod % 3600) / 60);
    if a.div_euclid(86_400) == t.div_euclid(86_400) {
        hm
    } else {
        let (_, m, d) = civil(a.div_euclid(86_400));
        format!("{d} {} {hm}", MONTHS[(m - 1) as usize])
    }
}

/// Freshness of displayed data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Freshness {
    Fresh { age_s: f64 },
    Stale { age_s: f64 },
}

impl Freshness {
    pub fn from_age(age_s: f64) -> Self {
        if age_s > STALE_AFTER_S {
            Self::Stale { age_s }
        } else {
            Self::Fresh { age_s }
        }
    }

    pub fn is_stale(&self) -> bool {
        matches!(self, Self::Stale { .. })
    }

    pub fn age_s(&self) -> f64 {
        match *self {
            Self::Fresh { age_s } | Self::Stale { age_s } => age_s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    /// The time of day in local time; the date only on another day; the offset in force at
    /// each instant (a log across a daylight-saving change).
    #[test]
    fn local_clock_names_the_day_only_when_it_is_another() {
        // 2026-10-03 04:01:30 UTC.
        let at = WallNs((20_729 * 86_400 + 4 * 3600 + 90) * S);
        let utc = |_: WallNs| 0;
        assert_eq!(local_clock(at, WallNs(at.0 + 3600 * S), utc), "4:01");
        assert_eq!(
            local_clock(at, WallNs(at.0 + 86_400 * S), utc),
            "3 Oct 4:01"
        );
        // UTC+3: 7:01, and the next UTC day starts at 21:00 local of the same day.
        let plus3 = |_: WallNs| 3 * 3600;
        assert_eq!(local_clock(at, WallNs(at.0 + 10 * 3600 * S), plus3), "7:01");
        assert_eq!(
            local_clock(at, WallNs(at.0 + 20 * 3600 * S), plus3),
            "3 Oct 7:01"
        );
    }

    #[test]
    fn age_uses_offset() {
        let cap = WallNs(1_000 * S);
        // Client clock 2 s behind the daemon: client_now 999.5 s is daemon 1001.5 s.
        let now = WallNs(999 * S + S / 2);
        let off = ClockOffset(2 * S as i64);
        assert!((age_s(cap, now, off) - 1.5).abs() < 1e-12);
        // Without the offset the same frame would look like it came from the future.
        assert_eq!(age_s(cap, now, ClockOffset(0)), 0.0);
    }

    #[test]
    fn stale_threshold_is_strict() {
        assert!(!Freshness::from_age(0.999).is_stale());
        assert!(!Freshness::from_age(1.0).is_stale());
        assert!(Freshness::from_age(1.001).is_stale());
        let cap = WallNs(10 * S);
        let off = ClockOffset(-(S as i64) / 4);
        assert!(!Freshness::from_age(age_s(cap, WallNs(11 * S + S / 4), off)).is_stale());
        assert!(
            Freshness::from_age(age_s(cap, WallNs(11 * S + S / 4 + 1_000_000), off)).is_stale()
        );
    }
}
