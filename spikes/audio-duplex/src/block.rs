//! Sample-indexed multichannel blocks and the wait-free transport that carries them from
//! the audio callback to a consumer thread.
//!
//! Transport is two SPSC rings written by the callback thread only: interleaved `f32`
//! samples first, then the header that describes them. A consumer that pops a header is
//! therefore guaranteed to find all of that block's samples already committed (the header
//! commit is a release store sequenced after the sample commit on the same thread). A block
//! is either written whole or not at all, so channels never shift relative to each other.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

/// Per-block condition bits. A set bit always describes the gap *before* this block, so a
/// consumer that sees any of [`BlockFlags::BREAKS_CONTINUITY`] must not splice this block
/// onto the previous one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct BlockFlags(u32);

impl BlockFlags {
    pub const NONE: Self = Self(0);
    /// First block of a stream.
    pub const FIRST: Self = Self(1);
    /// The backend reported an xrun since the previous block.
    pub const XRUN: Self = Self(1 << 1);
    /// The sample index does not follow the previous block exactly (estimated or exact gap).
    pub const DISCONTINUITY: Self = Self(1 << 2);
    /// One or more blocks were dropped because the consumer fell behind.
    pub const OVERFLOW: Self = Self(1 << 3);
    /// Buffer size, rate or routing changed since the previous block.
    pub const CONFIG_CHANGE: Self = Self(1 << 4);
    /// Gap size is an estimate from timestamps, not an exact sample counter.
    pub const GAP_ESTIMATED: Self = Self(1 << 5);

    pub const BREAKS_CONTINUITY: Self =
        Self(Self::XRUN.0 | Self::DISCONTINUITY.0 | Self::OVERFLOW.0 | Self::CONFIG_CHANGE.0);

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
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

/// Describes one block of interleaved capture samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct BlockHeader {
    /// Absolute index (since stream start) of the first frame in this block.
    pub start_sample: u64,
    pub frames: u32,
    pub channels: u16,
    pub flags: BlockFlags,
    /// When the callback ran, on the backend's monotonic clock (ns). Only differences
    /// between headers of the same stream are meaningful.
    pub callback_ns: u64,
    /// When the first frame hit the ADC, on the same clock, if the backend estimates it.
    pub capture_ns: Option<u64>,
}

impl BlockHeader {
    pub fn samples(&self) -> usize {
        self.frames as usize * self.channels as usize
    }

    pub fn end_sample(&self) -> u64 {
        self.start_sample + u64::from(self.frames)
    }
}

/// Counters shared between the RT side and observers. Relaxed is enough: they are
/// statistics, never used to order other memory.
#[derive(Debug, Default)]
pub struct TransportCounters {
    pub blocks_pushed: AtomicU64,
    pub blocks_dropped: AtomicU64,
    pub frames_dropped: AtomicU64,
}

/// What happened to a push.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    Pushed,
    /// Ring full; the block was discarded and the next pushed block carries OVERFLOW.
    Dropped,
}

/// RT-side writer. Never allocates, locks or blocks.
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

/// Create a transport sized for `capacity_frames` frames of `channels` channels, assuming
/// blocks no smaller than `min_block_frames` (bounds the header ring).
pub fn transport(
    channels: u16,
    capacity_frames: usize,
    min_block_frames: usize,
) -> (BlockProducer, BlockConsumer) {
    let channels_us = usize::from(channels.max(1));
    let header_slots = capacity_frames.div_ceil(min_block_frames.max(1)).max(4);
    let (hp, hc) = rtrb::RingBuffer::new(header_slots);
    let (sp, sc) = rtrb::RingBuffer::new(capacity_frames * channels_us);
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
    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn counters(&self) -> &Arc<TransportCounters> {
        &self.counters
    }

    /// Push one block. `sample(frame, channel)` supplies each output sample; it is called
    /// exactly `frames * channels` times in interleaved order. `header.channels` is
    /// overwritten with the transport's channel count.
    #[inline]
    pub fn push_with(
        &mut self,
        mut header: BlockHeader,
        mut sample: impl FnMut(usize, usize) -> f32,
    ) -> PushOutcome {
        header.channels = self.channels;
        let n = header.samples();
        if self.headers.slots() == 0 || self.samples.slots() < n {
            self.carry |= BlockFlags::OVERFLOW | BlockFlags::DISCONTINUITY;
            self.counters.blocks_dropped.fetch_add(1, Ordering::Relaxed);
            self.counters
                .frames_dropped
                .fetch_add(u64::from(header.frames), Ordering::Relaxed);
            return PushOutcome::Dropped;
        }
        let Ok(mut chunk) = self.samples.write_chunk(n) else {
            // Unreachable given the slots() check above (single producer), but treat any
            // refusal as a drop rather than panicking on the RT thread.
            self.carry |= BlockFlags::OVERFLOW | BlockFlags::DISCONTINUITY;
            self.counters.blocks_dropped.fetch_add(1, Ordering::Relaxed);
            return PushOutcome::Dropped;
        };
        let ch = usize::from(self.channels.max(1));
        let (a, b) = chunk.as_mut_slices();
        let (mut frame, mut c) = (0usize, 0usize);
        for slot in a.iter_mut().chain(b.iter_mut()) {
            *slot = sample(frame, c);
            c += 1;
            if c == ch {
                c = 0;
                frame += 1;
            }
        }
        chunk.commit_all();
        header.flags |= self.carry;
        self.carry = BlockFlags::NONE;
        // Cannot fail: slots() > 0 was checked and only this thread pushes.
        if self.headers.push(header).is_err() {
            self.carry |= BlockFlags::OVERFLOW | BlockFlags::DISCONTINUITY;
            return PushOutcome::Dropped;
        }
        self.counters.blocks_pushed.fetch_add(1, Ordering::Relaxed);
        PushOutcome::Pushed
    }
}

impl BlockConsumer {
    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn counters(&self) -> &Arc<TransportCounters> {
        &self.counters
    }

    /// Pop one block into `out` (cleared, then filled with `frames * channels` interleaved
    /// samples). Returns `None` if no complete block is available.
    pub fn pop_into(&mut self, out: &mut Vec<f32>) -> Option<BlockHeader> {
        let header = self.headers.pop().ok()?;
        let n = header.samples();
        out.clear();
        match self.samples.read_chunk(n) {
            Ok(chunk) => {
                let (a, b) = chunk.as_slices();
                out.extend_from_slice(a);
                out.extend_from_slice(b);
                chunk.commit_all();
            }
            Err(_) => {
                // Ordering guarantees make this impossible; surfacing it as a hard error
                // is better than returning a header with missing samples.
                unreachable!("header popped before its samples were committed");
            }
        }
        Some(header)
    }

    /// Discard everything queued.
    pub fn drain(&mut self) -> usize {
        let mut scratch = Vec::new();
        let mut n = 0;
        while self.pop_into(&mut scratch).is_some() {
            n += 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr(start: u64, frames: u32) -> BlockHeader {
        BlockHeader {
            start_sample: start,
            frames,
            channels: 0,
            flags: BlockFlags::NONE,
            callback_ns: 0,
            capture_ns: None,
        }
    }

    #[test]
    fn interleaved_order_and_channel_alignment_survive_wraparound() {
        let (mut p, mut c) = transport(3, 10, 4);
        let mut out = Vec::new();
        // Push/pop repeatedly so the sample ring wraps several times.
        for round in 0..20u64 {
            let start = round * 4;
            let r = p.push_with(hdr(start, 4), |f, ch| {
                (start as f32 + f as f32) * 10.0 + ch as f32
            });
            assert_eq!(r, PushOutcome::Pushed);
            let h = c.pop_into(&mut out).expect("block");
            assert_eq!(h.start_sample, start);
            assert_eq!(h.channels, 3);
            assert_eq!(out.len(), 12);
            for f in 0..4 {
                for ch in 0..3 {
                    assert_eq!(
                        out[f * 3 + ch],
                        (start as f32 + f as f32) * 10.0 + ch as f32
                    );
                }
            }
        }
    }

    #[test]
    fn full_ring_drops_whole_block_and_flags_next() {
        let (mut p, mut c) = transport(2, 8, 4);
        assert_eq!(p.push_with(hdr(0, 4), |_, _| 1.0), PushOutcome::Pushed);
        assert_eq!(p.push_with(hdr(4, 4), |_, _| 2.0), PushOutcome::Pushed);
        assert_eq!(p.push_with(hdr(8, 4), |_, _| 3.0), PushOutcome::Dropped);
        let mut out = Vec::new();
        assert!(c.pop_into(&mut out).is_some());
        assert_eq!(p.push_with(hdr(12, 4), |_, _| 4.0), PushOutcome::Pushed);
        let h = c.pop_into(&mut out).expect("second");
        assert_eq!(h.start_sample, 4);
        assert!(h.flags.is_empty());
        let h = c.pop_into(&mut out).expect("third");
        assert_eq!(h.start_sample, 12);
        assert!(
            h.flags
                .contains(BlockFlags::OVERFLOW | BlockFlags::DISCONTINUITY)
        );
        assert!(out.iter().all(|&s| s == 4.0));
        assert_eq!(c.counters().blocks_dropped.load(Ordering::Relaxed), 1);
        assert_eq!(c.counters().frames_dropped.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn cross_thread_transport_is_lossless_when_consumer_keeps_up() {
        let (mut p, mut c) = transport(2, 4096, 64);
        let producer = std::thread::spawn(move || {
            let mut start = 0u64;
            let mut pushed = 0;
            while pushed < 2000 {
                let s = start;
                if p.push_with(hdr(s, 64), |f, ch| (s + f as u64) as f32 + ch as f32 * 0.5)
                    == PushOutcome::Pushed
                {
                    pushed += 1;
                    start += 64;
                } else {
                    std::thread::yield_now();
                }
            }
        });
        let mut out = Vec::new();
        let mut expect = 0u64;
        let mut got = 0;
        while got < 2000 {
            if let Some(h) = c.pop_into(&mut out) {
                assert_eq!(h.start_sample, expect);
                for f in 0..64usize {
                    assert_eq!(out[f * 2], (expect + f as u64) as f32);
                    assert_eq!(out[f * 2 + 1], (expect + f as u64) as f32 + 0.5);
                }
                expect += 64;
                got += 1;
            } else {
                std::thread::yield_now();
            }
        }
        producer.join().expect("producer");
    }
}
