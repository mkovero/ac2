//! One channel's input history and the pull of output samples at caller-given positions.

use std::sync::Arc;

use crate::{Error, Kernel, Pos, Step};

/// Re-times one channel: input is pushed as it arrives, output is pulled at positions in
/// input-sample time. Input index 0 is the first sample pushed after creation or
/// [`reset`](Self::reset).
///
/// `push` and `pull` do not allocate.
#[derive(Clone, Debug)]
pub struct Resampler {
    kernel: Arc<Kernel>,
    /// Two copies of a ring of `capacity` samples, sample `n` at `n % capacity` and at
    /// `n % capacity + capacity`, so any run of up to `capacity` retained samples is one
    /// contiguous slice and the kernel's inner loop never wraps.
    ring: Vec<f32>,
    capacity: usize,
    /// Input samples pushed since creation or reset: the index of the next one.
    pushed: u64,
}

impl Resampler {
    /// A resampler retaining at least `max_block + kernel.taps()` input samples: enough that
    /// pushing up to `max_block` samples between pulls that take all
    /// [`available`](Self::available) output never drops history a later pull needs.
    pub fn new(kernel: Arc<Kernel>, max_block: usize) -> Self {
        let capacity = (max_block + kernel.taps() + 1).next_power_of_two();
        Self {
            kernel,
            ring: vec![0.0; 2 * capacity],
            capacity,
            pushed: 0,
        }
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

    /// How many outputs from `pos` on, `step` apart, the input pushed so far covers.
    pub fn available(&self, pos: Pos, step: Step) -> usize {
        let Some(last) = self.pushed.checked_sub(1 + self.kernel.lookahead()) else {
            return 0;
        };
        // The first position whose lookahead reaches past the input.
        let end = Pos::from_index(last + 1).raw();
        if pos.raw() >= end {
            return 0;
        }
        let n = (end - pos.raw() - 1) / step.raw() + 1;
        usize::try_from(n).unwrap_or(usize::MAX)
    }

    /// Writes `out.len()` outputs at `pos`, `pos + step`, …, and returns the position after
    /// the last. Fails, writing nothing, when the input does not yet reach the last output or
    /// no longer reaches back to the first.
    pub fn pull(&self, pos: Pos, step: Step, out: &mut [f32]) -> Result<Pos, Error> {
        let available = self.available(pos, step);
        if out.len() > available {
            return Err(Error::InputShort {
                requested: out.len(),
                available,
            });
        }
        let history = self.kernel.history();
        let oldest = self.oldest();
        if pos.index() < oldest + history {
            return Err(Error::BeforeHistory { pos, oldest });
        }
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
            p = p.advanced(step, 1);
        }
        Ok(p)
    }
}
