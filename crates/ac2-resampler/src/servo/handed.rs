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

    /// Records a block handed out at `first` from `pos`; `next` is the position after it.
    pub fn record(&mut self, first: u64, len: usize, pos: i128, next: i128, restarted: bool) {
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
            self.anchors.push_back(Anchor { at, pos });
        }
        self.next = Some((first.saturating_add(len as u64), next));
    }

    /// The device position (64.64) handed for media position `at`, if it was handed.
    pub fn position_at(&self, at: i128) -> Option<i128> {
        let (end, end_pos) = self.next?;
        let end = fix::from_int(i128::from(end));
        if at >= end {
            return None;
        }
        let i = self
            .anchors
            .partition_point(|a| a.at <= at)
            .checked_sub(1)?;
        let a = self.anchors[i];
        let (b_at, b_pos) = self
            .anchors
            .get(i + 1)
            .map_or((end, end_pos), |b| (b.at, b.pos));
        // Between two exact handed positions at most a quarter second apart the handed
        // positions bend only as the step changes: the chord errs by (span²/8) times the
        // step's rate of change: 1.6 ns for a step changing 0.2 ppm a second, whatever the
        // block length.
        let span = fix::to_f64(b_at - a.at);
        let frac = fix::to_f64(at - a.at) / span;
        Some(a.pos + fix::from_f64(fix::to_f64(b_pos - a.pos) * frac))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blocks of 32 outputs, far shorter than the anchor spacing, with the step's deviation
    /// changing up to 1 ppm a second (five times what the servo hands): every looked-up
    /// position is the handed one within the chord error, 0.75e-3 samples; extrapolating
    /// from the anchor's own step would miss by ten times that.
    #[test]
    fn short_blocks_with_a_changing_step_are_placed_exactly() {
        let rate = 96_000u32;
        let mut h = Handed::new(rate);
        let first0 = 1u64 << 40;
        let mut pos = fix::from_int(1 << 35) + fix::from_f64(0.375);
        let mut exact = Vec::new();
        let len = 32usize;
        for b in 0..30_000u64 {
            let t = b as f64 * len as f64 / f64::from(rate);
            let dev = 7e-6 + 1e-6 * t.sin();
            let step = fix::from_f64(1.0 + dev);
            let first = first0 + b * len as u64;
            let next = pos + step * len as i128;
            h.record(first, len, pos, next, b == 0);
            exact.push((first, pos, step));
            pos = next;
        }
        let mut worst = 0.0f64;
        for &(first, p, step) in exact.iter().step_by(7) {
            for i in [0i128, 5, 31] {
                let at = fix::from_int(i128::from(first) + i);
                let got = h.position_at(at).expect("handed within the anchors' reach");
                worst = worst.max(fix::to_f64(got - (p + step * i)).abs());
            }
        }
        assert!(worst <= 1e-3, "worst {worst:e} samples");
    }
}
