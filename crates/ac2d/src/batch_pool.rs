//! The fan-out's batches and blocks, reused once every job has let go of them.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::fanout::{Batch, Block};

/// Batches handed out and the spare blocks they return, so steady operation allocates
/// nothing.
#[derive(Default)]
pub(crate) struct Pool {
    /// Oldest first; jobs consume in order, so the front is released first.
    in_flight: VecDeque<Batch>,
    pub(crate) spare_blocks: Vec<Block>,
    spare_batches: Vec<Vec<Block>>,
}

/// Batches kept for reuse at most; more in flight than this are left to be freed.
const POOL_BATCHES: usize = 256;

impl Pool {
    /// Takes back every batch no job holds any more.
    pub(crate) fn reclaim(&mut self) {
        while let Some(front) = self.in_flight.front_mut() {
            let Some(v) = Arc::get_mut(front) else {
                break;
            };
            self.spare_blocks.append(v);
            if let Some(b) = self.in_flight.pop_front().and_then(Arc::into_inner) {
                self.spare_batches.push(b);
            }
        }
    }

    pub(crate) fn batch(&mut self) -> Vec<Block> {
        self.spare_batches.pop().unwrap_or_default()
    }

    /// Keeps `batch` for reuse once every job is done with it.
    pub(crate) fn hand_out(&mut self, batch: Batch) {
        if self.in_flight.len() < POOL_BATCHES {
            self.in_flight.push_back(batch);
        }
    }

    /// A batch no job saw: its blocks are spare at once.
    pub(crate) fn give_back(&mut self, mut batch: Vec<Block>) {
        self.spare_blocks.append(&mut batch);
        self.spare_batches.push(batch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_audio::BlockFlags;

    #[test]
    fn a_released_batch_returns_its_blocks_to_the_pool() {
        let mut pool = Pool::default();
        let mut v = pool.batch();
        v.push(Block::new(0, 1, 1, BlockFlags::NONE, 0, vec![0.5]));
        let batch: Batch = Arc::new(v);
        let held = Arc::clone(&batch);
        pool.hand_out(batch);
        pool.reclaim();
        assert!(pool.spare_blocks.is_empty(), "a job still holds the batch");
        drop(held);
        pool.reclaim();
        assert_eq!(pool.spare_blocks.len(), 1);
        assert_eq!(pool.spare_batches.len(), 1);
        assert!(pool.in_flight.is_empty());
    }
}
