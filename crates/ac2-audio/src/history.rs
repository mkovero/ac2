//! Generator history: the last few seconds of samples written to one output channel,
//! indexed by output sample index.
//!
//! The loopback timing monitor correlates this against the loopback capture (Q3). It is
//! written by the output callback after each block is rendered (after fades and the level
//! limit, so it holds what was actually handed to the device) and read from any thread.
//!
//! Lock-free scheme: samples are stored as `AtomicU32` bit patterns, so concurrent access is
//! never undefined behaviour; a sequence check detects reads that raced with an overwrite.
//! The writer first announces the end index it is about to reach (`claimed_end`), then
//! writes samples, then publishes `written_end`. A reader copies samples and afterwards
//! checks `claimed_end`: if the writer may have reached into the copied range, the read is
//! rejected rather than returned torn.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

use thiserror::Error;

/// Why a history read could not be served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum HistoryError {
    /// Part of the range has not been rendered yet.
    #[error("samples up to {requested_end} requested, only {written_end} written")]
    NotYetWritten {
        /// One past the last requested index.
        requested_end: u64,
        /// One past the last written index.
        written_end: u64,
    },
    /// Part of the range has already been overwritten.
    #[error("sample {requested_start} requested, oldest available is {oldest_available}")]
    Overwritten {
        /// First requested index.
        requested_start: u64,
        /// Oldest index still held.
        oldest_available: u64,
    },
}

#[derive(Debug)]
struct Shared {
    samples: Box<[AtomicU32]>,
    mask: u64,
    claimed_end: AtomicU64,
    written_end: AtomicU64,
}

impl Shared {
    fn capacity(&self) -> u64 {
        self.mask + 1
    }
}

/// Creates a history holding at least `min_frames` samples (rounded up to a power of two).
pub(crate) fn history(min_frames: usize) -> (HistoryWriter, HistoryReader) {
    let cap = min_frames.max(2).next_power_of_two();
    let shared = Arc::new(Shared {
        samples: (0..cap).map(|_| AtomicU32::new(0)).collect(),
        mask: cap as u64 - 1,
        claimed_end: AtomicU64::new(0),
        written_end: AtomicU64::new(0),
    });
    (
        HistoryWriter {
            shared: Arc::clone(&shared),
            next: 0,
        },
        HistoryReader { shared },
    )
}

/// Output-callback side. Never allocates or blocks.
#[derive(Debug)]
pub(crate) struct HistoryWriter {
    shared: Arc<Shared>,
    next: u64,
}

impl HistoryWriter {
    /// Writes `samples` at output indices `start..start + samples.len()`.
    ///
    /// Indices skipped since the previous write (an output-side gap) are filled with zeros:
    /// nothing was rendered for them, and a correlation must not pick up stale samples from
    /// an earlier lap of the ring. Writes are expected in increasing order; a write that goes
    /// backwards is treated as a restart at `start`.
    #[inline]
    pub(crate) fn write(&mut self, start: u64, samples: &[f32]) {
        let end = start + samples.len() as u64;
        let s = &*self.shared;
        s.claimed_end.store(end, Ordering::Relaxed);
        fence(Ordering::Release);
        if start > self.next {
            let fill_from = self.next.max(start.saturating_sub(s.capacity()));
            for i in fill_from..start {
                s.samples[(i & s.mask) as usize].store(0, Ordering::Relaxed);
            }
        }
        for (i, v) in (start..).zip(samples) {
            s.samples[(i & s.mask) as usize].store(v.to_bits(), Ordering::Relaxed);
        }
        s.written_end.store(end, Ordering::Release);
        self.next = end;
    }
}

/// Read side; cheap to clone and share between threads.
#[derive(Debug, Clone)]
pub struct HistoryReader {
    shared: Arc<Shared>,
}

impl HistoryReader {
    /// Number of samples held.
    pub fn capacity(&self) -> u64 {
        self.shared.capacity()
    }

    /// One past the newest written output index.
    pub fn written_end(&self) -> u64 {
        self.shared.written_end.load(Ordering::Acquire)
    }

    /// Copies output indices `start..start + out.len()` into `out`.
    pub fn read(&self, start: u64, out: &mut [f32]) -> Result<(), HistoryError> {
        let s = &*self.shared;
        let end = start + out.len() as u64;
        let written_end = s.written_end.load(Ordering::Acquire);
        if end > written_end {
            return Err(HistoryError::NotYetWritten {
                requested_end: end,
                written_end,
            });
        }
        let oldest = written_end.saturating_sub(s.capacity());
        if start < oldest {
            return Err(HistoryError::Overwritten {
                requested_start: start,
                oldest_available: oldest,
            });
        }
        for (i, o) in (start..).zip(out.iter_mut()) {
            *o = f32::from_bits(s.samples[(i & s.mask) as usize].load(Ordering::Relaxed));
        }
        // Pairs with the writer's release fence: if any copied value came from a write that
        // started after this read began, `claimed_end` now shows it.
        fence(Ordering::Acquire);
        let claimed = s.claimed_end.load(Ordering::Relaxed);
        let oldest_now = claimed.saturating_sub(s.capacity());
        if start < oldest_now {
            return Err(HistoryError::Overwritten {
                requested_start: start,
                oldest_available: oldest_now,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_by_index_and_rejects_out_of_range() {
        let (mut w, r) = history(8);
        assert_eq!(r.capacity(), 8);
        w.write(0, &[0.0, 1.0, 2.0, 3.0, 4.0]);
        w.write(5, &[5.0, 6.0, 7.0, 8.0, 9.0]);
        let mut out = [0.0; 4];
        r.read(6, &mut out).expect("in range");
        assert_eq!(out, [6.0, 7.0, 8.0, 9.0]);
        assert_eq!(
            r.read(7, &mut out),
            Err(HistoryError::NotYetWritten {
                requested_end: 11,
                written_end: 10
            })
        );
        assert_eq!(
            r.read(1, &mut out),
            Err(HistoryError::Overwritten {
                requested_start: 1,
                oldest_available: 2
            })
        );
    }

    #[test]
    fn gap_is_zero_filled() {
        let (mut w, r) = history(16);
        w.write(0, &[1.0; 4]);
        w.write(8, &[2.0; 4]);
        let mut out = [9.0; 12];
        r.read(0, &mut out).expect("in range");
        assert_eq!(&out[..4], &[1.0; 4]);
        assert_eq!(&out[4..8], &[0.0; 4]);
        assert_eq!(&out[8..], &[2.0; 4]);
    }

    #[test]
    fn concurrent_reads_are_never_torn() {
        // Each sample's value is its own index, so a torn read is detectable.
        let (mut w, r) = history(1024);
        let writer = std::thread::spawn(move || {
            let mut block = [0.0f32; 64];
            for b in 0..20_000u64 {
                for (i, v) in block.iter_mut().enumerate() {
                    *v = ((b * 64 + i as u64) % 1_000_000) as f32;
                }
                w.write(b * 64, &block);
            }
        });
        let mut out = vec![0.0f32; 512];
        let mut ok = 0;
        while !writer.is_finished() {
            let end = r.written_end();
            let start = end.saturating_sub(700);
            if r.read(start, &mut out).is_ok() {
                for (i, v) in out.iter().enumerate() {
                    assert_eq!(*v, ((start + i as u64) % 1_000_000) as f32);
                }
                ok += 1;
            }
        }
        writer.join().expect("writer");
        assert!(ok > 0);
    }
}
