//! The clock a stream stamps its blocks on, read beside the host's disciplined clock.
//!
//! Block stamps ([`BlockHeader::callback_ns`](crate::BlockHeader), `capture_ns`) are on
//! the backend's own clock, which need not be the one the OS disciplines: JACK2 on Linux
//! stamps on `CLOCK_MONOTONIC_RAW`, the bare crystal, which a discipline daemon never
//! touches. A device's rate fitted from those stamps is its rate against the crystal. Pairs
//! of reads of both clocks, taken over the same minute, measure the crystal against the
//! disciplined clock, so the device's rate can be said against the latter.

use std::fmt;

/// One read of a stream's stamp clock with the disciplined clock read around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockPair {
    /// Now on the clock the stream's blocks are stamped on, ns.
    pub stamp_ns: u64,
    /// Now on the host's disciplined monotonic clock, ns (Linux `CLOCK_MONOTONIC`: slewed
    /// to the discipline daemon's frequency, never stepped).
    pub disciplined_ns: u64,
}

/// A stream's stamp clock, readable beside the host's disciplined clock. A stream has one
/// only where its backend can read the clock it stamps on and the OS has a disciplined
/// monotonic clock to read it against.
pub trait StampClock: Send + Sync + fmt::Debug {
    /// Both clocks now; `None` when this read is no good (the stream is gone, or the
    /// thread was preempted between the reads).
    fn pair(&self) -> Option<ClockPair>;

    /// The stamp clock alone, now, ns; `None` when the stream is gone. A caller that
    /// brackets the read with host clocks of its own choosing (see `bracket_host_clocks`)
    /// takes this rather than the disciplined pair.
    fn now_ns(&self) -> Option<u64>;
}

/// The longest the two disciplined reads around a stamp-clock read may lie apart. Read
/// back to back they are a few hundred ns apart; more means the thread was preempted in
/// between, and the midpoint no longer says when the stamp clock was read.
#[cfg(target_os = "linux")]
const MAX_BRACKET_NS: u64 = 20_000;

/// `CLOCK_MONOTONIC`, ns.
#[cfg(target_os = "linux")]
fn monotonic_ns() -> u64 {
    clock_ns(libc::CLOCK_MONOTONIC)
}

/// `clock` now, ns.
#[cfg(target_os = "linux")]
fn clock_ns(clock: libc::clockid_t) -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writes one timespec through a valid pointer; a vDSO read, no syscall.
    #[allow(unsafe_code)]
    unsafe {
        libc::clock_gettime(clock, &mut ts)
    };
    (ts.tv_sec.max(0) as u64) * 1_000_000_000 + ts.tv_nsec.max(0) as u64
}

/// One read of a stamp clock nested inside reads of the host's clocks, kept whatever the
/// bracket widths: a study of the clocks counts the preempted reads rather than losing them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostClocks {
    /// The stamp clock, ns.
    pub stamp_ns: u64,
    /// `CLOCK_MONOTONIC_RAW` read right after the stamp clock, ns: equal to the stamp to
    /// within the bracket when the stamp clock is the raw crystal.
    pub monotonic_raw_ns: u64,
    /// Midpoint of the `CLOCK_MONOTONIC` reads around the stamp read, ns.
    pub monotonic_ns: u64,
    /// Width of that bracket, ns.
    pub monotonic_bracket_ns: u64,
    /// Midpoint of the `CLOCK_REALTIME` reads around the monotonic ones, ns since the epoch.
    pub realtime_ns: u64,
    /// Width of that bracket, ns.
    pub realtime_bracket_ns: u64,
}

/// `clock` read inside `CLOCK_MONOTONIC` reads, those inside `CLOCK_REALTIME` reads, and
/// `CLOCK_MONOTONIC_RAW` right after it; `None` when the stamp clock cannot be read.
#[cfg(target_os = "linux")]
pub fn bracket_host_clocks(clock: &dyn StampClock) -> Option<HostClocks> {
    let real_a = clock_ns(libc::CLOCK_REALTIME);
    let mono_a = monotonic_ns();
    let stamp = clock.now_ns();
    let raw = clock_ns(libc::CLOCK_MONOTONIC_RAW);
    let mono_b = monotonic_ns();
    let real_b = clock_ns(libc::CLOCK_REALTIME);
    let stamp_ns = stamp?;
    // REALTIME can step backwards between the reads; the saturating width then says 0 and
    // the midpoint is the first read, which the caller's bracket filter cannot catch, so
    // a stepped pair shows as an outlier in the REALTIME map instead.
    let mid = |a: u64, b: u64| a + b.saturating_sub(a) / 2;
    Some(HostClocks {
        stamp_ns,
        monotonic_raw_ns: raw,
        monotonic_ns: mid(mono_a, mono_b),
        monotonic_bracket_ns: mono_b.saturating_sub(mono_a),
        realtime_ns: mid(real_a, real_b),
        realtime_bracket_ns: real_b.saturating_sub(real_a),
    })
}

/// `stamp_now` read between two reads of `CLOCK_MONOTONIC`, paired with their midpoint.
#[cfg(target_os = "linux")]
pub(crate) fn pair_with_monotonic(stamp_now: impl FnOnce() -> u64) -> Option<ClockPair> {
    let before = monotonic_ns();
    let stamp_ns = stamp_now();
    let after = monotonic_ns();
    (after.wrapping_sub(before) <= MAX_BRACKET_NS).then_some(ClockPair {
        stamp_ns,
        disciplined_ns: before + (after - before) / 2,
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// `CLOCK_MONOTONIC` against itself: the pair agrees to the bracket.
    #[test]
    fn a_pair_brackets_its_read() {
        let p = (0..100)
            .find_map(|_| pair_with_monotonic(monotonic_ns))
            .expect("one unpreempted read in a hundred");
        assert!(
            p.stamp_ns.abs_diff(p.disciplined_ns) <= MAX_BRACKET_NS,
            "{p:?}"
        );
    }

    /// A stamp clock on `CLOCK_MONOTONIC_RAW`, as JACK2's is.
    #[derive(Debug)]
    struct Raw;

    impl StampClock for Raw {
        fn pair(&self) -> Option<ClockPair> {
            pair_with_monotonic(|| clock_ns(libc::CLOCK_MONOTONIC_RAW))
        }
        fn now_ns(&self) -> Option<u64> {
            Some(clock_ns(libc::CLOCK_MONOTONIC_RAW))
        }
    }

    /// The raw read follows the stamp closely; the realtime bracket holds the monotonic one.
    #[test]
    fn host_clocks_nest_around_the_stamp() {
        let h = (0..100)
            .filter_map(|_| bracket_host_clocks(&Raw))
            .find(|h| h.monotonic_bracket_ns <= MAX_BRACKET_NS)
            .expect("one unpreempted read in a hundred");
        assert!(h.monotonic_raw_ns >= h.stamp_ns, "{h:?}");
        assert!(h.monotonic_raw_ns - h.stamp_ns <= MAX_BRACKET_NS, "{h:?}");
        assert!(h.realtime_bracket_ns >= h.monotonic_bracket_ns, "{h:?}");
    }
}
