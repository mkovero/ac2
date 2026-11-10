//! A host's stamp clock mapped onto its system clock: a line fitted over the last stretch
//! of paired reads. A block stamped on the stamp clock (a backend's raw crystal) is put on
//! system time through it, the one clock two hosts disciplined from one source share.
//!
//! A map that is a minute old would not do: a stamp clock runs ppm off its host's
//! disciplined clock (+8 ppm is 0.5 ms a minute), so the line is refitted as pairs arrive
//! and carries its own slope.

use std::collections::VecDeque;

/// System time as a line in stamp-clock time: `system = system_ns + (stamp − epoch) · (1 +
/// ppm·10⁻⁶)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StampLine {
    /// The stamp the line is anchored at, ns.
    pub epoch_stamp_ns: u64,
    /// System time at `epoch_stamp_ns`, ns since the Unix epoch.
    pub system_ns: u64,
    /// The system clock's rate against the stamp clock, ppm (positive: system time runs
    /// fast against the stamps).
    pub ppm: f64,
}

impl StampLine {
    /// System time of `stamp_ns`, ns since the Unix epoch.
    pub fn system_of(&self, stamp_ns: u64) -> f64 {
        let dx = stamp_ns as f64 - self.epoch_stamp_ns as f64;
        self.system_ns as f64 + dx * (1.0 + self.ppm * 1e-6)
    }

    /// Stamp-clock time of `system_ns` (ns since the Unix epoch), ns.
    pub fn stamp_of(&self, system_ns: f64) -> f64 {
        let dy = system_ns - self.system_ns as f64;
        self.epoch_stamp_ns as f64 + dy / (1.0 + self.ppm * 1e-6)
    }
}

/// The last `window` of pairs, a pair kept per `step`, fitted on demand.
#[derive(Clone, Debug)]
pub struct MapWindow {
    window_ns: u64,
    step_ns: u64,
    min_span_ns: u64,
    points: VecDeque<(u64, u64)>,
}

impl MapWindow {
    /// Pairs over `window_s` seconds of stamps, one kept per `step_s`; a line is given once
    /// the pairs span `min_span_s`.
    pub fn new(window_s: f64, step_s: f64, min_span_s: f64) -> Self {
        let ns = |s: f64| (s * 1e9).max(1.0) as u64;
        Self {
            window_ns: ns(window_s),
            step_ns: ns(step_s),
            min_span_ns: ns(min_span_s),
            points: VecDeque::new(),
        }
    }

    /// The stamp clock read `stamp_ns` while the system clock read `system_ns`.
    pub fn push(&mut self, stamp_ns: u64, system_ns: u64) {
        match self.points.back() {
            // Another stream's stamps (the clock went back): nothing before belongs to it.
            Some(&(last, _)) if stamp_ns < last => self.points.clear(),
            Some(&(last, _)) if stamp_ns - last < self.step_ns => return,
            _ => {}
        }
        self.points.push_back((stamp_ns, system_ns));
        while self
            .points
            .front()
            .is_some_and(|&(s, _)| stamp_ns - s > self.window_ns)
        {
            self.points.pop_front();
        }
    }

    /// Forgets every pair.
    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// The fitted line, anchored at the newest pair; `None` until the pairs span the least
    /// span asked for.
    pub fn line(&self) -> Option<StampLine> {
        let (&(s0, y0), &(s1, _)) = (self.points.front()?, self.points.back()?);
        if s1 - s0 < self.min_span_ns || self.points.len() < 3 {
            return None;
        }
        let n = self.points.len() as f64;
        let xy: Vec<(f64, f64)> = self
            .points
            .iter()
            .map(|&(s, y)| ((s - s0) as f64, y as f64 - y0 as f64))
            .collect();
        let mx = xy.iter().map(|p| p.0).sum::<f64>() / n;
        let my = xy.iter().map(|p| p.1).sum::<f64>() / n;
        let sxx: f64 = xy.iter().map(|p| (p.0 - mx) * (p.0 - mx)).sum();
        let sxy: f64 = xy.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        if sxx <= 0.0 {
            return None;
        }
        let slope = sxy / sxx;
        let x1 = (s1 - s0) as f64;
        let y1 = my + slope * (x1 - mx);
        Some(StampLine {
            epoch_stamp_ns: s1,
            system_ns: (y0 as f64 + y1).round().max(0.0) as u64,
            ppm: (slope - 1.0) * 1e6,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_follows_a_stamp_clock_off_its_system_clock() {
        let mut w = MapWindow::new(60.0, 0.25, 2.0);
        let origin = 1_700_000_000_000_000_000u64;
        // System time runs 8.2 ppm fast against the stamps, with ±3 µs of read jitter.
        for i in 0..400u64 {
            let s = 5_000_000_000 + i * 250_000_000;
            let j = ((i * 7919) % 7) as f64 - 3.0;
            let y = origin as f64 + (s - 5_000_000_000) as f64 * (1.0 + 8.2e-6) + j * 1000.0;
            w.push(s, y as u64);
        }
        let l = w.line().expect("fitted");
        assert!((l.ppm - 8.2).abs() < 0.05, "{l:?}");
        let s = l.epoch_stamp_ns;
        let truth = origin as f64 + (s - 5_000_000_000) as f64 * (1.0 + 8.2e-6);
        assert!((l.system_of(s) - truth).abs() < 2_000.0, "{l:?}");
        // System time near 1.7·10¹⁸ ns is held in an f64 to 256 ns, far below a sample.
        let back = l.stamp_of(l.system_of(s + 123_456));
        assert!((back - (s + 123_456) as f64).abs() <= 512.0, "{back}");
    }

    #[test]
    fn no_line_before_the_least_span_and_a_clock_going_back_starts_anew() {
        let mut w = MapWindow::new(60.0, 0.25, 2.0);
        for i in 0..5u64 {
            w.push(i * 250_000_000, 10 + i * 250_000_000);
        }
        assert!(w.line().is_none());
        for i in 5..20u64 {
            w.push(i * 250_000_000, 10 + i * 250_000_000);
        }
        assert!(w.line().is_some());
        w.push(1, 1);
        assert!(w.line().is_none());
    }
}
