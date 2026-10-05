//! Sample-indexed multichannel capture blocks and the wait-free transport that carries them
//! from the audio callback to one consumer thread.
//!
//! The transport is two single-producer single-consumer rings, both written only by the
//! callback thread: interleaved `f32` samples first, then the header that describes them.
//! The header push is a release store sequenced after the sample commit on the same thread,
//! so a consumer that pops a header always finds all of that block's samples committed.
//! A block is written whole or not at all. Channels therefore never shift relative to each
//! other, and a lost block is always visible as a flag on the next block that gets through.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Per-block condition bits.
///
/// A bit always describes what happened *before* this block (since the previous block that
/// reached the consumer). A consumer that sees any bit of [`BlockFlags::BREAKS_CONTINUITY`]
/// must not splice this block onto the previous one: averages reset, correlations re-acquire.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlockFlags(u32);

impl BlockFlags {
    /// No condition.
    pub const NONE: Self = Self(0);
    /// First block of the stream. Nothing precedes it, so there is nothing to splice onto.
    pub const FIRST: Self = Self(1);
    /// The backend reported an xrun since the previous block. On JACK a late cycle can keep
    /// the frame counter contiguous although audio was lost at the converter, so an xrun
    /// breaks continuity on its own. The notification is asynchronous and may land one
    /// block late; consumers should treat the preceding block as suspect too.
    pub const XRUN: Self = Self(1 << 1);
    /// The sample index does not continue the previous block (exact or estimated gap, or a
    /// counter that went backwards).
    pub const DISCONTINUITY: Self = Self(1 << 2);
    /// One or more blocks were dropped because the consumer fell behind.
    pub const OVERFLOW: Self = Self(1 << 3);
    /// Sample rate, buffer size, device or routing changed since the previous block.
    pub const CONFIG_CHANGE: Self = Self(1 << 4);
    /// The size of the gap is estimated from host timestamps, not read from an exact frame
    /// counter. Only set together with [`BlockFlags::DISCONTINUITY`].
    pub const GAP_ESTIMATED: Self = Self(1 << 5);

    /// Every bit after which samples must not be treated as contiguous with the previous
    /// block.
    pub const BREAKS_CONTINUITY: Self =
        Self(Self::XRUN.0 | Self::DISCONTINUITY.0 | Self::OVERFLOW.0 | Self::CONFIG_CHANGE.0);

    const NAMES: [(Self, &'static str); 6] = [
        (Self::FIRST, "FIRST"),
        (Self::XRUN, "XRUN"),
        (Self::DISCONTINUITY, "DISCONTINUITY"),
        (Self::OVERFLOW, "OVERFLOW"),
        (Self::CONFIG_CHANGE, "CONFIG_CHANGE"),
        (Self::GAP_ESTIMATED, "GAP_ESTIMATED"),
    ];

    /// Raw bits, for wire formats.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Flags from raw bits; unknown bits are dropped.
    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits & 0x3f)
    }

    /// True when every bit of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// True when any bit of `other` is set.
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// True when no bit is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// True when this block must not be spliced onto the previous one.
    pub const fn breaks_continuity(self) -> bool {
        self.intersects(Self::BREAKS_CONTINUITY)
    }
}

impl std::ops::BitOr for BlockFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for BlockFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl fmt::Debug for BlockFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("NONE");
        }
        let mut first = true;
        for (flag, name) in Self::NAMES {
            if self.contains(flag) {
                if !first {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        Ok(())
    }
}

/// Timing and indexing of one block, as produced by a callback. The transport adds the
/// channel count when it turns this into a [`BlockHeader`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockStamp {
    /// Absolute index of the first frame since stream start.
    pub start_sample: u64,
    /// Frames in the block.
    pub frames: u32,
    /// Condition bits from the clock and event latch.
    pub flags: BlockFlags,
    /// When the callback ran, in ns on the backend's monotonic clock.
    pub callback_ns: u64,
    /// When the first frame was sampled by the ADC, on the same clock, if the backend has an
    /// estimate.
    pub capture_ns: Option<u64>,
}

/// Describes one block of interleaved capture samples.
///
/// Times are nanoseconds on the backend's own monotonic clock; only differences between
/// headers of one stream are meaningful.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Absolute index of the first frame since stream start.
    pub start_sample: u64,
    /// Frames in the block.
    pub frames: u32,
    /// Interleaved channels per frame (the request's input map length).
    pub channels: u16,
    /// Condition bits; see [`BlockFlags`].
    pub flags: BlockFlags,
    /// When the callback ran.
    pub callback_ns: u64,
    /// When the first frame was sampled, if the backend estimates it.
    pub capture_ns: Option<u64>,
}

impl BlockHeader {
    /// Number of interleaved samples (`frames * channels`).
    pub fn samples(&self) -> usize {
        self.frames as usize * usize::from(self.channels)
    }

    /// Index one past the last frame.
    pub fn end_sample(&self) -> u64 {
        self.start_sample + u64::from(self.frames)
    }
}

/// Transport statistics. Relaxed atomics: they are counters, never used to order memory.
#[derive(Debug, Default)]
pub struct TransportCounters {
    blocks_pushed: AtomicU64,
    blocks_dropped: AtomicU64,
    frames_dropped: AtomicU64,
}

/// Plain copy of [`TransportCounters`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportStats {
    /// Blocks that reached the ring.
    pub blocks_pushed: u64,
    /// Blocks discarded because the ring was full.
    pub blocks_dropped: u64,
    /// Frames in the discarded blocks.
    pub frames_dropped: u64,
}

impl TransportCounters {
    /// Current values.
    pub fn snapshot(&self) -> TransportStats {
        TransportStats {
            blocks_pushed: self.blocks_pushed.load(Ordering::Relaxed),
            blocks_dropped: self.blocks_dropped.load(Ordering::Relaxed),
            frames_dropped: self.frames_dropped.load(Ordering::Relaxed),
        }
    }
}

/// Result of [`BlockProducer::push_with`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// The block is in the ring.
    Pushed,
    /// The ring was full; the block was discarded whole and the next pushed block carries
    /// `OVERFLOW | DISCONTINUITY`.
    Dropped,
    /// A zero-frame block; nothing to carry.
    Empty,
}

/// Callback-side writer. Never allocates, locks or blocks.
#[derive(Debug)]
pub struct BlockProducer {
    headers: rtrb::Producer<BlockHeader>,
    samples: rtrb::Producer<f32>,
    channels: u16,
    carry: BlockFlags,
    counters: Arc<TransportCounters>,
}

/// Consumer-side reader.
#[derive(Debug)]
pub struct BlockConsumer {
    headers: rtrb::Consumer<BlockHeader>,
    samples: rtrb::Consumer<f32>,
    channels: u16,
    counters: Arc<TransportCounters>,
}

/// Creates a transport for `channels` interleaved channels holding `capacity_frames`
/// frames.
///
/// The header ring is sized for blocks no smaller than `min_block_frames`. Size it from the
/// smallest plausible block: hosts with variable callback sizes would otherwise run out of
/// header slots long before sample space.
pub fn transport(
    channels: u16,
    capacity_frames: usize,
    min_block_frames: usize,
) -> (BlockProducer, BlockConsumer) {
    let per_frame = usize::from(channels.max(1));
    let header_slots = capacity_frames.div_ceil(min_block_frames.max(1)).max(4);
    let (hp, hc) = rtrb::RingBuffer::new(header_slots);
    let (sp, sc) = rtrb::RingBuffer::new(capacity_frames.max(1) * per_frame);
    let counters = Arc::new(TransportCounters::default());
    (
        BlockProducer {
            headers: hp,
            samples: sp,
            channels,
            carry: BlockFlags::NONE,
            counters: Arc::clone(&counters),
        },
        BlockConsumer {
            headers: hc,
            samples: sc,
            channels,
            counters,
        },
    )
}

impl BlockProducer {
    /// Interleaved channels per frame.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Whether a block of `frames` frames would fit now. Only a source that can wait (a
    /// replay) asks; a device callback pushes and lets a full ring drop the block.
    pub(crate) fn has_room(&self, frames: u32) -> bool {
        self.headers.slots() > 0
            && self.samples.slots() >= frames as usize * usize::from(self.channels.max(1))
    }

    fn drop_block(&mut self, frames: u32) -> PushOutcome {
        self.carry |= BlockFlags::OVERFLOW | BlockFlags::DISCONTINUITY;
        self.counters.blocks_dropped.fetch_add(1, Ordering::Relaxed);
        self.counters
            .frames_dropped
            .fetch_add(u64::from(frames), Ordering::Relaxed);
        PushOutcome::Dropped
    }

    /// Pushes one block. `sample(frame, channel)` supplies each value; it is called exactly
    /// `frames * channels` times in interleaved order, and not at all if the block is
    /// dropped.
    #[inline]
    pub fn push_with(
        &mut self,
        stamp: BlockStamp,
        mut sample: impl FnMut(usize, usize) -> f32,
    ) -> PushOutcome {
        if stamp.frames == 0 {
            return PushOutcome::Empty;
        }
        let header = BlockHeader {
            start_sample: stamp.start_sample,
            frames: stamp.frames,
            channels: self.channels,
            flags: stamp.flags | self.carry,
            callback_ns: stamp.callback_ns,
            capture_ns: stamp.capture_ns,
        };
        let n = header.samples();
        if self.headers.slots() == 0 || self.samples.slots() < n {
            return self.drop_block(stamp.frames);
        }
        let Ok(mut chunk) = self.samples.write_chunk(n) else {
            // Cannot happen after the slots() check with a single producer; a drop is the
            // only safe reaction on the audio thread.
            return self.drop_block(stamp.frames);
        };
        let per_frame = usize::from(self.channels.max(1));
        let (a, b) = chunk.as_mut_slices();
        let (mut frame, mut ch) = (0usize, 0usize);
        for slot in a.iter_mut().chain(b.iter_mut()) {
            *slot = sample(frame, ch);
            ch += 1;
            if ch == per_frame {
                ch = 0;
                frame += 1;
            }
        }
        chunk.commit_all();
        if self.headers.push(header).is_err() {
            // Also unreachable (slots() > 0 was checked). The samples are committed without
            // a header; the consumer would mis-frame every following block, so this must
            // never be silent. It cannot happen with one producer.
            return self.drop_block(stamp.frames);
        }
        self.carry = BlockFlags::NONE;
        self.counters.blocks_pushed.fetch_add(1, Ordering::Relaxed);
        PushOutcome::Pushed
    }
}

impl BlockConsumer {
    /// Interleaved channels per frame.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Transport statistics.
    pub fn stats(&self) -> TransportStats {
        self.counters.snapshot()
    }

    /// Complete blocks waiting.
    pub fn blocks_available(&self) -> usize {
        self.headers.slots()
    }

    /// Pops one block and hands its samples to `f` as up to two slices (the ring may wrap
    /// inside a block), without copying. Returns `None` when no complete block is waiting.
    pub fn pop_with<R>(&mut self, f: impl FnOnce(&BlockHeader, &[f32], &[f32]) -> R) -> Option<R> {
        let header = self.headers.pop().ok()?;
        let chunk = self
            .samples
            .read_chunk(header.samples())
            .unwrap_or_else(|_| {
                // Ordering guarantees make this impossible; continuing would mis-frame every
                // later block, so it is a hard invariant violation.
                panic!("block header popped before its samples were committed")
            });
        let (a, b) = chunk.as_slices();
        let r = f(&header, a, b);
        chunk.commit_all();
        Some(r)
    }

    /// Pops one block into `out` (cleared, then filled with `frames * channels`
    /// interleaved samples).
    pub fn pop_into(&mut self, out: &mut Vec<f32>) -> Option<BlockHeader> {
        self.pop_with(|h, a, b| {
            out.clear();
            out.extend_from_slice(a);
            out.extend_from_slice(b);
            *h
        })
    }

    /// Discards everything queued; returns the number of blocks discarded.
    pub fn discard_all(&mut self) -> usize {
        let mut n = 0;
        while self.pop_with(|_, _, _| ()).is_some() {
            n += 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(start: u64, frames: u32) -> BlockStamp {
        BlockStamp {
            start_sample: start,
            frames,
            flags: BlockFlags::NONE,
            callback_ns: 0,
            capture_ns: None,
        }
    }

    #[test]
    fn flags_mask_and_debug() {
        assert!(BlockFlags::XRUN.breaks_continuity());
        assert!(BlockFlags::OVERFLOW.breaks_continuity());
        assert!(BlockFlags::CONFIG_CHANGE.breaks_continuity());
        assert!(BlockFlags::DISCONTINUITY.breaks_continuity());
        assert!(!BlockFlags::FIRST.breaks_continuity());
        assert!(!BlockFlags::GAP_ESTIMATED.breaks_continuity());
        let f = BlockFlags::DISCONTINUITY | BlockFlags::GAP_ESTIMATED;
        assert_eq!(format!("{f:?}"), "DISCONTINUITY|GAP_ESTIMATED");
        assert_eq!(BlockFlags::from_bits_truncate(f.bits() | 0x8000), f);
    }

    #[test]
    fn interleaving_and_alignment_survive_ring_wrap() {
        let (mut p, mut c) = transport(3, 10, 4);
        let mut out = Vec::new();
        for round in 0..20u64 {
            let start = round * 4;
            let value = |f: usize, ch: usize| (start as f32 + f as f32) * 10.0 + ch as f32;
            assert_eq!(p.push_with(stamp(start, 4), value), PushOutcome::Pushed);
            let h = c.pop_into(&mut out).expect("block");
            assert_eq!((h.start_sample, h.channels, out.len()), (start, 3, 12));
            for f in 0..4 {
                for ch in 0..3 {
                    assert_eq!(out[f * 3 + ch], value(f, ch));
                }
            }
        }
    }

    #[test]
    fn full_ring_drops_whole_block_and_flags_the_next() {
        let (mut p, mut c) = transport(2, 8, 4);
        assert_eq!(p.push_with(stamp(0, 4), |_, _| 1.0), PushOutcome::Pushed);
        assert_eq!(p.push_with(stamp(4, 4), |_, _| 2.0), PushOutcome::Pushed);
        assert_eq!(p.push_with(stamp(8, 4), |_, _| 3.0), PushOutcome::Dropped);
        // A second drop in a row still flags only once, on the next delivered block.
        assert_eq!(p.push_with(stamp(12, 4), |_, _| 3.5), PushOutcome::Dropped);
        let mut out = Vec::new();
        assert_eq!(c.pop_into(&mut out).map(|h| h.start_sample), Some(0));
        assert_eq!(p.push_with(stamp(16, 4), |_, _| 4.0), PushOutcome::Pushed);
        let h = c.pop_into(&mut out).expect("second");
        assert_eq!(h.start_sample, 4);
        assert!(h.flags.is_empty());
        let h = c.pop_into(&mut out).expect("third");
        assert_eq!(h.start_sample, 16);
        assert!(
            h.flags
                .contains(BlockFlags::OVERFLOW | BlockFlags::DISCONTINUITY)
        );
        assert!(out.iter().all(|&s| s == 4.0));
        let stats = c.stats();
        assert_eq!(
            (
                stats.blocks_pushed,
                stats.blocks_dropped,
                stats.frames_dropped
            ),
            (3, 2, 8)
        );
        // The flag is consumed by the block that carried it.
        assert_eq!(p.push_with(stamp(20, 4), |_, _| 5.0), PushOutcome::Pushed);
        assert!(c.pop_into(&mut out).expect("fourth").flags.is_empty());
    }

    #[test]
    fn oversized_and_empty_blocks() {
        let (mut p, mut c) = transport(1, 8, 4);
        assert_eq!(p.push_with(stamp(0, 0), |_, _| 0.0), PushOutcome::Empty);
        assert_eq!(p.push_with(stamp(0, 9), |_, _| 0.0), PushOutcome::Dropped);
        assert_eq!(c.blocks_available(), 0);
        assert_eq!(c.discard_all(), 0);
    }

    #[test]
    fn zero_copy_pop_sees_both_halves_of_a_wrapped_block() {
        let (mut p, mut c) = transport(1, 6, 4);
        p.push_with(stamp(0, 4), |f, _| f as f32);
        c.discard_all();
        p.push_with(stamp(4, 4), |f, _| 10.0 + f as f32);
        let joined = c
            .pop_with(|_, a, b| {
                assert!(!b.is_empty(), "block must straddle the ring end");
                a.iter().chain(b).copied().collect::<Vec<_>>()
            })
            .expect("block");
        assert_eq!(joined, vec![10.0, 11.0, 12.0, 13.0]);
    }

    #[test]
    fn cross_thread_transfer_is_lossless_when_consumer_keeps_up() {
        const BLOCKS: u64 = 2000;
        let (mut p, mut c) = transport(2, 4096, 64);
        let producer = std::thread::spawn(move || {
            let mut start = 0u64;
            while start < BLOCKS * 64 {
                let s = start;
                let r = p.push_with(stamp(s, 64), |f, ch| {
                    (s + f as u64) as f32 + ch as f32 * 0.5
                });
                if r == PushOutcome::Pushed {
                    start += 64;
                } else {
                    // A drop here would lose data; the test's consumer always catches up,
                    // but the producer must not assume that, so it retries the same block.
                    std::thread::yield_now();
                }
            }
        });
        let mut out = Vec::new();
        let mut expect = 0u64;
        while expect < BLOCKS * 64 {
            match c.pop_into(&mut out) {
                Some(h) => {
                    assert_eq!(h.start_sample, expect);
                    for f in 0..64usize {
                        assert_eq!(out[f * 2], (expect + f as u64) as f32);
                        assert_eq!(out[f * 2 + 1], (expect + f as u64) as f32 + 0.5);
                    }
                    expect += 64;
                }
                None => std::thread::yield_now(),
            }
        }
        producer.join().expect("producer");
    }
}
