//! One channel's input history and the pull of output samples at caller-given positions.

use std::sync::Arc;

use crate::{Error, Kernel, Pos, Step};

/// Re-times one channel: input is pushed as it arrives, output is pulled at positions in
/// input-sample time. Input index 0 is the first sample pushed after creation or
/// [`reset`](Self::reset).
///
/// # Threading
///
/// One thread pushes and pulls, and it is not the audio callback: the callback exchanges
/// blocks with that thread through its own queues. `push` and `pull` do not allocate, so the
/// thread may run at real-time priority.
///
/// The kernel is shared through an `Arc`. Dropping its last clone — the last `Resampler` built
/// on it, once the caller's own clone is gone — frees the table (0.9 MB for
/// [`Kernel::measurement`]) on the dropping thread; keep a clone where freeing is allowed.
#[derive(Clone, Debug)]
pub struct Resampler {
    kernel: Arc<Kernel>,
    /// Two copies of a ring of `capacity` samples, sample `n` at `n % capacity` and at
    /// `n % capacity + capacity`, so any run of up to `capacity` retained samples is one
    /// contiguous slice and the kernel's inner loop never wraps.
    ring: Vec<f32>,
    /// A power of two, so a sample index maps to its slot with a mask.
    capacity: usize,
    /// Input samples pushed since creation or reset: the index of the next one.
    pushed: u64,
}

impl Resampler {
    /// The most input samples `max_block + max_backlog + taps` may ask to retain: 2²⁴, almost
    /// three minutes at 96 kHz. A backlog that long is a consumer that has stopped, not jitter
    /// to ride out, and the ring (two f32 copies, rounded up to a power of two) stays within
    /// 128 MiB.
    pub const MAX_RETAINED: usize = 1 << 24;

    /// A resampler retaining at least `max_block + max_backlog + kernel.taps()` input
    /// samples.
    ///
    /// The guarantee, in terms of the *frontier* — the first position the input does not yet
    /// cover, `pushed() − lookahead()`: pushes totalling up to `max_block` samples between two
    /// pulls never drop input the second pull needs, as long as it starts no more than
    /// `max_backlog` input samples before the frontier as it stood before those pushes. A
    /// caller that always pulls everything [`available`](Self::available) sets
    /// `max_backlog = 0`; one that leaves up to `B` samples' worth of output unpulled, or
    /// re-anchors its position up to `B` samples backwards, sets `max_backlog ≥ B`.
    ///
    /// Refused with [`Error::RetentionTooLarge`] above [`Self::MAX_RETAINED`].
    pub fn new(kernel: Arc<Kernel>, max_block: usize, max_backlog: usize) -> Result<Self, Error> {
        let too_large = Error::RetentionTooLarge {
            max_block,
            max_backlog,
        };
        // A pull at `frontier − max_backlog` reads from `history()` before it; after
        // `max_block` more samples the newest is `max_block + lookahead() − 1` past the
        // frontier: `max_block + max_backlog + taps − 1` samples in all.
        let needed = max_block
            .checked_add(max_backlog)
            .and_then(|n| n.checked_add(kernel.taps()))
            .filter(|&n| n <= Self::MAX_RETAINED)
            .ok_or(too_large)?;
        let capacity = needed.next_power_of_two();
        Ok(Self {
            kernel,
            ring: vec![0.0; 2 * capacity],
            capacity,
            pushed: 0,
        })
    }

    pub fn kernel(&self) -> &Kernel {
        &self.kernel
    }

    /// The index the next pushed sample gets.
    pub fn pushed(&self) -> u64 {
        self.pushed
    }

    /// The oldest input index still retained.
    pub fn oldest(&self) -> u64 {
        self.pushed.saturating_sub(self.capacity as u64)
    }

    /// Appends input. A block longer than the retained history keeps only its tail.
    pub fn push(&mut self, input: &[f32]) {
        let skip = input.len().saturating_sub(self.capacity);
        let mut at = ((self.pushed + skip as u64) & (self.capacity as u64 - 1)) as usize;
        for &x in &input[skip..] {
            self.ring[at] = x;
            self.ring[at + self.capacity] = x;
            at = if at + 1 == self.capacity { 0 } else { at + 1 };
        }
        self.pushed += input.len() as u64;
    }

    /// Forgets all input; the next pushed sample is index 0. Call after a gap in the input:
    /// the kernel spans 2·half-length samples, and an output near a gap would mix samples
    /// from both sides of it as if they were continuous.
    pub fn reset(&mut self) {
        self.pushed = 0;
    }

    /// How many outputs from `pos` on, `step` apart, the input pushed so far covers: a
    /// [`pull`](Self::pull) of up to that many succeeds. Refused with
    /// [`Error::BeforeHistory`] when the first output needs input from before the oldest
    /// retained sample or before index 0 — the same check `pull` makes.
    pub fn available(&self, pos: Pos, step: Step) -> Result<usize, Error> {
        let oldest = self.oldest();
        if pos
            .index()
            .checked_sub(self.kernel.history())
            .is_none_or(|first| first < oldest)
        {
            return Err(Error::BeforeHistory { pos, oldest });
        }
        // The first index whose lookahead reaches past the input.
        let Some(end) = self.pushed.checked_sub(self.kernel.lookahead()) else {
            return Ok(0);
        };
        let end = Pos::from_index(end).raw();
        if pos.raw() >= end {
            return Ok(0);
        }
        let n = (end - pos.raw() - 1) / step.raw() + 1;
        Ok(usize::try_from(n).unwrap_or(usize::MAX))
    }

    /// Writes `out.len()` outputs at `pos`, `pos + step`, …, and returns the position after
    /// the last. Fails, writing nothing, when [`available`](Self::available) fails or covers
    /// fewer than `out.len()` outputs. An empty `out` is checked the same way: it returns
    /// `pos` exactly when `available` succeeds.
    pub fn pull(&self, pos: Pos, step: Step, out: &mut [f32]) -> Result<Pos, Error> {
        let available = self.available(pos, step)?;
        if out.len() > available {
            return Err(Error::InputShort {
                requested: out.len(),
                available,
            });
        }
        let history = self.kernel.history();
        let taps = self.kernel.taps();
        let mut p = pos;
        for o in out.iter_mut() {
            let first = p.index() - history;
            let at = (first & (self.capacity as u64 - 1)) as usize;
            let frac = p.frac_bits();
            *o = if frac == 0 {
                // The kernel at an integer position is a unit impulse; reading the sample
                // directly keeps it bit-exact for every value (a sum would turn −0 into +0).
                self.ring[at + history as usize]
            } else {
                self.kernel.apply(&self.ring[at..at + taps], frac) as f32
            };
            p = p.stepped(step);
        }
        Ok(p)
    }
}
