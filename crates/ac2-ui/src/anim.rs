//! Spring animation for navigation only (zoom, pan). Measurement values, traces and faults
//! are never animated (PLAN §4.4): they are drawn as received.

use ac2_scene::view::FreqRange;

/// Critically damped spring: the fastest approach without overshoot, so a zoom never
/// swings past the range the operator asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spring {
    pub pos: f64,
    pub vel: f64,
}

/// Angular frequency: ~0.25 s to settle within 1 %.
pub const OMEGA: f64 = 26.0;

impl Spring {
    pub fn at(pos: f64) -> Self {
        Self { pos, vel: 0.0 }
    }

    /// Advances by `dt` seconds towards `target` (exact solution of the critically damped
    /// oscillator, stable for any `dt`).
    pub fn step(&mut self, target: f64, dt: f64) {
        let x0 = self.pos - target;
        let v0 = self.vel;
        let e = (-OMEGA * dt).exp();
        let c = v0 + OMEGA * x0;
        self.pos = target + (x0 + c * dt) * e;
        self.vel = (v0 - OMEGA * c * dt) * e;
    }

    pub fn settled(&self, target: f64, eps: f64) -> bool {
        (self.pos - target).abs() < eps && self.vel.abs() < eps * OMEGA
    }
}

/// Animated log-frequency axis: springs on `ln lo` and `ln hi`, so zooming feels the same
/// at every frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FreqNav {
    pub target: FreqRange,
    lo: Spring,
    hi: Spring,
}

/// Settled when both edges are within this many nepers (≈ 0.01 %).
const SETTLE: f64 = 1e-4;

impl FreqNav {
    pub fn new(r: FreqRange) -> Self {
        Self {
            target: r,
            lo: Spring::at(r.lo.ln()),
            hi: Spring::at(r.hi.ln()),
        }
    }

    /// The range drawn now.
    pub fn current(&self) -> FreqRange {
        if self.settled() {
            return self.target;
        }
        FreqRange {
            lo: self.lo.pos.exp(),
            hi: self.hi.pos.exp(),
        }
    }

    pub fn set_target(&mut self, r: FreqRange) {
        self.target = r;
    }

    /// Jumps to the target (tests, reduced motion).
    pub fn snap(&mut self) {
        *self = Self::new(self.target);
    }

    pub fn step(&mut self, dt: f64) {
        self.lo.step(self.target.lo.ln(), dt);
        self.hi.step(self.target.hi.ln(), dt);
        if self.settled() {
            self.snap();
        }
    }

    pub fn settled(&self) -> bool {
        self.lo.settled(self.target.lo.ln(), SETTLE) && self.hi.settled(self.target.hi.ln(), SETTLE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges_without_overshoot() {
        let mut s = Spring::at(0.0);
        let mut max: f64 = 0.0;
        for _ in 0..120 {
            s.step(1.0, 1.0 / 120.0);
            max = max.max(s.pos);
        }
        assert!(max <= 1.0 + 1e-12, "overshoot {max}");
        assert!(s.settled(1.0, 1e-3), "{s:?}");
    }

    #[test]
    fn step_size_does_not_matter() {
        let (mut a, mut b) = (Spring::at(0.0), Spring::at(0.0));
        a.step(1.0, 0.1);
        for _ in 0..10 {
            b.step(1.0, 0.01);
        }
        assert!((a.pos - b.pos).abs() < 1e-9);
    }

    #[test]
    fn nav_reaches_target_and_snaps() {
        let mut n = FreqNav::new(FreqRange::default());
        let z = FreqRange::default().zoom(1000.0, 4.0);
        n.set_target(z);
        n.step(0.016);
        let mid = n.current();
        assert!(mid.lo > 20.0 && mid.lo < z.lo, "{mid:?}");
        for _ in 0..200 {
            n.step(0.016);
        }
        assert!(n.settled());
        assert_eq!(n.current(), z);
    }
}
