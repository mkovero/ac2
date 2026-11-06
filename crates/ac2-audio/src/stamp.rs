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
}

/// The longest the two disciplined reads around a stamp-clock read may lie apart. Read
/// back to back they are a few hundred ns apart; more means the thread was preempted in
/// between, and the midpoint no longer says when the stamp clock was read.
#[cfg(target_os = "linux")]
const MAX_BRACKET_NS: u64 = 20_000;

/// `CLOCK_MONOTONIC`, ns.
#[cfg(target_os = "linux")]
fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writes one timespec through a valid pointer; a vDSO read, no syscall.
    #[allow(unsafe_code)]
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts)
    };
    (ts.tv_sec.max(0) as u64) * 1_000_000_000 + ts.tv_nsec.max(0) as u64
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
}
