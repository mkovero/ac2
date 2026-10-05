//! Signed integer alignment of a (reference, measurement) pair at full rate.
//!
//! With delay `D` (measurement late by `D` samples when positive), aligned pair `n` is
//! `(reference[n − D], measurement[n])`, indexed by the measurement sample index `n`. A
//! negative `D` needs reference samples from the future relative to the measurement, so the
//! measurement leg waits instead; both legs buffer and neither sign is a special case.
//!
//! The delay can change mid-stream ([`Aligner::splice`]): the pair stream stays contiguous
//! in `n` and pairs from the splice on take the reference at the new delay. A larger delay
//! needs reference samples older than the current pair, so the reference leg keeps a
//! history of `history` samples beyond what the current delay needs.

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub(crate) struct Aligner {
    delay: i64,
    /// Reference samples kept beyond what the current delay needs, so that a splice to a
    /// delay up to this much larger finds them.
    history: usize,
    /// The newest splice: measurement index of its first pair and the delay before it.
    /// Pairs below that index were made at the old delay.
    splice: Option<(i64, i64)>,
    reference: VecDeque<f64>,
    measurement: VecDeque<f64>,
    /// Input index of `reference[0]` / `measurement[0]`.
    ref_base: i64,
    meas_base: i64,
    /// Measurement index of the next pair to emit; `None` before the first input.
    next: Option<i64>,
    /// First pair's measurement index (pair-stream origin).
    origin: Option<i64>,
    /// Input index expected next.
    expected: Option<i64>,
}

impl Aligner {
    pub fn new(delay: i64, history: usize) -> Self {
        Self {
            delay,
            history,
            splice: None,
            reference: VecDeque::new(),
            measurement: VecDeque::new(),
            ref_base: 0,
            meas_base: 0,
            next: None,
            origin: None,
            expected: None,
        }
    }

    pub fn reset(&mut self, delay: i64) {
        *self = Self::new(delay, self.history);
    }

    /// Change the delay from the next pair on without restarting the pair stream; returns
    /// that pair's index in the pair stream (pairs before it used the old delay). `None` when
    /// the stream has not started, or when the new delay needs reference samples that are
    /// no longer kept: then only a restart can apply it.
    pub fn splice(&mut self, delay: i64) -> Option<u64> {
        let n = self.next?;
        let origin = self.origin?;
        if n - delay < self.ref_base {
            return None;
        }
        self.splice = Some((n, self.delay));
        self.delay = delay;
        Some((n - origin) as u64)
    }

    /// Input index the next push must start at to be contiguous.
    pub fn expected(&self) -> Option<i64> {
        self.expected
    }

    /// Measurement index of pair 0.
    pub fn origin(&self) -> Option<i64> {
        self.origin
    }

    /// Pair-stream interval touched by input samples `[lo, hi)` on either leg.
    pub fn pairs_touched(&self, lo: i64, hi: i64) -> Option<(u64, u64)> {
        let origin = self.origin?;
        // A reference sample feeds the pair `delay` later; near a splice it may feed a pair
        // at either delay.
        let (d_lo, d_hi) = match self.splice {
            Some((_, old)) => (self.delay.min(old), self.delay.max(old)),
            None => (self.delay, self.delay),
        };
        let a = lo.min(lo + d_lo) - origin;
        let b = hi.max(hi + d_hi) - origin;
        (b > 0).then(|| (a.max(0) as u64, b as u64))
    }

    /// Append input samples starting at input index `start` (must be contiguous) and emit
    /// every pair that is now complete into `out_x` (reference) / `out_y` (measurement).
    pub fn push(
        &mut self,
        start: i64,
        reference: impl Iterator<Item = f64>,
        measurement: impl Iterator<Item = f64>,
        out_x: &mut Vec<f64>,
        out_y: &mut Vec<f64>,
    ) {
        // Inputs from `start` on feed only pairs past the splice once neither leg reaches
        // back before it.
        if self
            .splice
            .is_some_and(|(at, old)| start >= at.max(at - old))
        {
            self.splice = None;
        }
        if self.next.is_none() {
            self.ref_base = start;
            self.meas_base = start;
            let first = start + self.delay.max(0);
            self.next = Some(first);
            self.origin = Some(first);
        }
        self.reference.extend(reference);
        self.measurement.extend(measurement);
        let end = self.meas_base + self.measurement.len() as i64;
        debug_assert_eq!(end, self.ref_base + self.reference.len() as i64);
        self.expected = Some(end);
        let Some(mut n) = self.next else { return };
        // Pair n needs measurement[n] and reference[n − D], both below `end`.
        let stop = end + self.delay.min(0);
        while n < stop {
            out_x.push(self.reference[(n - self.delay - self.ref_base) as usize]);
            out_y.push(self.measurement[(n - self.meas_base) as usize]);
            n += 1;
        }
        self.next = Some(n);
        // Drop samples no future pair needs.
        let drop_m = (n - self.meas_base).clamp(0, self.measurement.len() as i64) as usize;
        self.measurement.drain(..drop_m);
        self.meas_base += drop_m as i64;
        let drop_r = (n - self.delay - self.history as i64 - self.ref_base)
            .clamp(0, self.reference.len() as i64) as usize;
        self.reference.drain(..drop_r);
        self.ref_base += drop_r as i64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(delay: i64, n: usize, chunk: usize) -> (Vec<f64>, Vec<f64>, i64) {
        let r: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let m: Vec<f64> = (0..n).map(|i| 1000.0 + i as f64).collect();
        let mut a = Aligner::new(delay, 0);
        let (mut x, mut y) = (Vec::new(), Vec::new());
        let start = 50;
        for c in (0..n).step_by(chunk) {
            let e = (c + chunk).min(n);
            a.push(
                start + c as i64,
                r[c..e].iter().copied(),
                m[c..e].iter().copied(),
                &mut x,
                &mut y,
            );
        }
        (x, y, a.origin().expect("origin"))
    }

    #[test]
    fn positive_and_negative_delays_pair_correct_samples() {
        for delay in [0i64, 3, -3, 17, -17] {
            for chunk in [1, 4, 64] {
                let (x, y, origin) = run(delay, 100, chunk);
                assert_eq!(x.len(), 100 - delay.unsigned_abs() as usize);
                for (i, (xr, ym)) in x.iter().zip(&y).enumerate() {
                    let n = origin + i as i64; // measurement input index
                    assert_eq!(*ym, 1000.0 + (n - 50) as f64);
                    assert_eq!(*xr, (n - delay - 50) as f64, "delay {delay}");
                }
            }
        }
    }

    /// Splices up and down mid-stream: every pair before the splice uses the old delay and
    /// every pair from it on the new one, with no pair skipped or repeated, however the input
    /// is chunked.
    #[test]
    fn splice_switches_delay_at_the_next_pair() {
        let n = 400usize;
        let r: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let m: Vec<f64> = (0..n).map(|i| 1000.0 + i as f64).collect();
        for (d0, d1) in [(10i64, 13), (10, 7), (-5, 20), (20, -5), (0, 0)] {
            for chunk in [1usize, 7, 64] {
                let mut a = Aligner::new(d0, 32);
                let (mut x, mut y) = (Vec::new(), Vec::new());
                let mut at = None;
                for c in (0..n).step_by(chunk) {
                    if at.is_none() && c >= 150 {
                        at = Some(a.splice(d1).expect("history reaches"));
                    }
                    let e = (c + chunk).min(n);
                    a.push(
                        c as i64,
                        r[c..e].iter().copied(),
                        m[c..e].iter().copied(),
                        &mut x,
                        &mut y,
                    );
                }
                let at = at.expect("spliced") as usize;
                let origin = a.origin().expect("origin");
                for (i, (xr, ym)) in x.iter().zip(&y).enumerate() {
                    let nn = origin + i as i64;
                    let d = if i < at { d0 } else { d1 };
                    assert_eq!(*ym, 1000.0 + nn as f64, "{d0}->{d1} chunk {chunk}");
                    assert_eq!(*xr, (nn - d) as f64, "{d0}->{d1} chunk {chunk} pair {i}");
                }
            }
        }
        // Beyond the kept history the splice is refused.
        let mut a = Aligner::new(0, 8);
        let (mut x, mut y) = (Vec::new(), Vec::new());
        a.push(0, r.iter().copied(), m.iter().copied(), &mut x, &mut y);
        assert!(a.splice(9).is_none());
        assert!(a.splice(8).is_some());
        assert!(
            Aligner::new(0, 8).splice(1).is_none(),
            "nothing to splice before input"
        );
    }

    #[test]
    fn buffers_stay_bounded() {
        let mut a = Aligner::new(-500, 0);
        let (mut x, mut y) = (Vec::new(), Vec::new());
        for c in 0..100 {
            a.push(
                c * 64,
                (0..64).map(f64::from),
                (0..64).map(f64::from),
                &mut x,
                &mut y,
            );
            assert!(a.reference.len() <= 500 + 64 && a.measurement.len() <= 500 + 64);
        }
    }
}
