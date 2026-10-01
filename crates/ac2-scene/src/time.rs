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
