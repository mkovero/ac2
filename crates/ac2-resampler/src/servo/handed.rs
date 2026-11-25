//! The positions the servo has handed out: where the next block continues, and anchors to
//! look up where an earlier output sample was taken from (audio measures the handed
//! output, not the estimate).

use std::collections::VecDeque;

use super::fix;

/// Enough anchors, one per quarter second at most, to cover two minutes: a delay tracker
/// reports on windows seconds old, never minutes.
const ANCHORS: usize = 512;

#[derive(Clone, Copy, Debug)]
struct Anchor {
    /// Output index as 64.64 media position.
    at: i128,
    /// Device position handed for it, 64.64.
    pos: i128,
    /// The block's step (input samples per output).
    step: f64,
}

#[derive(Clone, Debug)]
pub(super) struct Handed {
    /// The output index the next contiguous block starts at, and its device position.
    next: Option<(u64, i128)>,
    anchors: VecDeque<Anchor>,
    /// Media samples between thinned anchors.
    spacing: i128,
    /// Whether the last block's step was clamped.
    pub clamped: bool,
    /// The step's correction on top of the rate (a deviation from 1), low-passed.
    pub correction: f64,
}

impl Handed {
    pub fn new(rate: u32) -> Self {
        Self {
            next: None,
            anchors: VecDeque::with_capacity(ANCHORS),
            spacing: fix::from_int(i128::from(rate / 4)),
            clamped: false,
            correction: 0.0,
        }
    }

    /// Forgets everything handed: the device positions no longer mean the same samples.
    pub fn clear(&mut self) {
        self.next = None;
        self.anchors.clear();
        self.clamped = false;
        self.correction = 0.0;
    }

    /// The device position a block starting at `first` continues from, if it continues the
    /// last one.
    pub fn continuing(&self, first: u64) -> Option<i128> {
        self.next.filter(|&(i, _)| i == first).map(|(_, p)| p)
    }

    /// Records a block handed out at `first`, `pos`, `step`; `next` is the position after it.
    pub fn record(
        &mut self,
        first: u64,
        len: usize,
        pos: i128,
        step: f64,
        next: i128,
        restarted: bool,
    ) {
        let at = fix::from_int(i128::from(first));
        if restarted {
            // Anchors before a non-contiguous block may overlap it in output time.
            self.anchors.clear();
        }
        if self
            .anchors
            .back()
            .is_none_or(|a| at - a.at >= self.spacing)
        {
            if self.anchors.len() == ANCHORS {
                self.anchors.pop_front();
            }
            self.anchors.push_back(Anchor { at, pos, step });
        }
        self.next = Some((first.saturating_add(len as u64), next));
    }

    /// The device position (64.64) handed for media position `at`, if it was handed.
    pub fn position_at(&self, at: i128) -> Option<i128> {
        let (end, _) = self.next?;
        if at >= fix::from_int(i128::from(end)) {
            return None;
        }
        let a = self.anchors.iter().rev().find(|a| a.at <= at)?;
        // Later blocks' steps differ from this one's by the servo's rate changes over at most
        // a quarter second: nanoseconds.
        Some(a.pos + fix::from_f64(fix::to_f64(at - a.at) * a.step))
    }
}
