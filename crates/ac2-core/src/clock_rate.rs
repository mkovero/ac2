//! A device's sample clock measured against the host clock, from the host time at which
//! blocks of known sample index arrive. Pure: fed `(sample, ns)` points, never a stream.
//!
//! [`RateFit`] is a least-squares line over every point it is given (a whole run);
//! [`RateWindow`] keeps the points of the last stretch of audio and fits those, so its
//! reading follows a clock that changes (temperature, a host clock being disciplined).
//!
//! A backend stamps blocks on its own host clock, which need not be the disciplined one
//! (JACK2 on Linux stamps on `CLOCK_MONOTONIC_RAW`, the bare crystal). The same window fed
//! pairs of reads (disciplined time as the index at 1 GHz nominal, stamp-clock time as the
//! stamp) measures the disciplined clock against the stamp clock, and
//! [`ClockRate::against`] takes that rate out of the device's.

use std::collections::VecDeque;

/// Least-squares line of host time against sample index, pooled over contiguous segments.
///
/// A device clock that runs at its nominal rate advances `1e9 / rate` ns per sample. Each
/// segment is fitted about its own mean, so an index gap of unknown or estimated size moves
/// no point off the line; only the slopes within segments are pooled.
#[derive(Clone, Copy, Debug, Default)]
pub struct RateFit {
    /// Index and time of the current segment's first point (the origin of its sums).
    origin: Option<(u64, u64)>,
    n: f64,
    sx: f64,
    sy: f64,
    sxx: f64,
    sxy: f64,
    /// Centred sums of closed segments.
    total_sxx: f64,
    total_sxy: f64,
    /// Samples spanned by all segments.
    span_samples: f64,
    seg_last_x: f64,
}

impl RateFit {
    /// One point: the frame at index `sample` was stamped `ns`. `breaks`: continuity broke
    /// before it (an xrun, an estimated gap), so it starts a new segment.
    pub fn add(&mut self, sample: u64, ns: u64, breaks: bool) {
        if breaks {
            self.close();
        }
        let (s0, t0) = *self.origin.get_or_insert((sample, ns));
        let x = sample.wrapping_sub(s0) as i64 as f64;
        let y = ns.wrapping_sub(t0) as i64 as f64;
        self.n += 1.0;
        self.sx += x;
        self.sy += y;
        self.sxx += x * x;
        self.sxy += x * y;
        self.seg_last_x = x;
    }

    fn close(&mut self) {
        if self.n >= 2.0 {
            self.total_sxx += self.sxx - self.sx * self.sx / self.n;
            self.total_sxy += self.sxy - self.sx * self.sy / self.n;
            self.span_samples += self.seg_last_x;
        }
        *self = Self {
            total_sxx: self.total_sxx,
            total_sxy: self.total_sxy,
            span_samples: self.span_samples,
            ..Self::default()
        };
    }

    /// Measured rate, Hz, and the samples it spans; `None` without two points in a segment.
    pub fn rate(mut self) -> Option<(f64, f64)> {
        self.close();
        (self.total_sxx > 0.0 && self.total_sxy > 0.0)
            .then(|| (1e9 * self.total_sxx / self.total_sxy, self.span_samples))
    }
}

/// A clock rate as a deviation from the nominal rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockRate {
    /// Measured rate, Hz.
    pub hz: f64,
    /// Deviation from the nominal rate, ppm (positive: the device runs fast against the
    /// host clock).
    pub ppm: f64,
    /// Seconds of audio the measurement spans.
    pub span_s: f64,
}

impl ClockRate {
    /// The rate `fit` measured, against `nominal` Hz.
    pub fn from_fit(fit: RateFit, nominal: f64) -> Option<Self> {
        fit.rate().map(|(hz, span)| Self {
            hz,
            ppm: (hz / nominal - 1.0) * 1e6,
            span_s: span / nominal,
        })
    }

    /// This rate, measured against a stamp clock, re-expressed against a `reference` clock
    /// whose own rate was measured against the same stamp clock (`reference.ppm`: positive
    /// when the reference runs fast against it). Rates on one time base divide: the device
    /// advances `(1 + d)` samples per nominal stamp-clock second and the reference
    /// `(1 + r)` seconds, so per reference second it advances `(1 + d) / (1 + r)`.
    pub fn against(self, reference: ClockRate) -> Self {
        let r = 1.0 + reference.ppm * 1e-6;
        Self {
            hz: self.hz / r,
            ppm: ((1.0 + self.ppm * 1e-6) / r - 1.0) * 1e6,
            span_s: self.span_s,
        }
    }
}

/// The last `window` seconds of stamps, fitted on demand.
///
/// One point is kept per `step` seconds of audio (and the first after every break): the
/// slope's error falls with the span, not with the point count, so a minute at a point per
/// 50 ms reads a clock to a fraction of a ppm under callback jitter of tens of µs, with a
/// bounded store whatever the device period.
#[derive(Clone, Debug)]
pub struct RateWindow {
    nominal: f64,
    window: u64,
    step: u64,
    points: VecDeque<(u64, u64, bool)>,
}

/// The share of the window the pooled segments must span before a rate is given: gaps
/// (xruns) take a little of it, but a reading never rests on much less than the window.
const READY_SHARE: f64 = 0.9;

impl RateWindow {
    /// A window of `window_s` seconds at `nominal` Hz, a point kept per `step_s` seconds.
    pub fn new(nominal: f64, window_s: f64, step_s: f64) -> Self {
        let n = |s: f64| (s * nominal).max(1.0) as u64;
        Self {
            nominal,
            window: n(window_s),
            step: n(step_s),
            points: VecDeque::new(),
        }
    }

    /// The frame at index `sample` was stamped `ns`; `breaks` as [`RateFit::add`].
    pub fn push(&mut self, sample: u64, ns: u64, breaks: bool) {
        let due = self
            .points
            .back()
            .is_none_or(|&(last, _, _)| breaks || sample < last || sample - last >= self.step);
        if !due {
            return;
        }
        // An index that went back is another stream's: nothing before it belongs to it.
        if self
            .points
            .back()
            .is_some_and(|&(last, _, _)| sample < last)
        {
            self.points.clear();
        }
        self.points.push_back((sample, ns, breaks));
        while self
            .points
            .get(1)
            .is_some_and(|&(s, _, _)| sample - s >= self.window)
        {
            self.points.pop_front();
        }
    }

    /// The rate over the window; `None` until the points span (most of) a window.
    pub fn rate(&self) -> Option<ClockRate> {
        let mut fit = RateFit::default();
        for (i, &(s, ns, breaks)) in self.points.iter().enumerate() {
            fit.add(s, ns, breaks && i > 0);
        }
        ClockRate::from_fit(fit, self.nominal)
            .filter(|r| r.span_s * self.nominal >= READY_SHARE * self.window as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stamps of a clock running `ppm` fast against the host at 48 kHz, block 256, with
    /// callback jitter spread uniformly over ±`jitter_ns` (a fixed pseudo-random sequence).
    fn feed(w: &mut RateWindow, seconds: f64, ppm: f64, jitter_ns: f64, from: u64) -> u64 {
        let fs = 48_000.0 * (1.0 + ppm * 1e-6);
        let blocks = (seconds * 48_000.0 / 256.0) as u64;
        let mut s = from;
        for _ in 0..blocks {
            // A 64-bit mix of the sample index, mapped to [-1, 1).
            let h = (s ^ 0x9e37_79b9_7f4a_7c15).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            let u = (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
            let j = u * jitter_ns;
            let ns = (s as f64 / fs * 1e9 + 1e9 + j) as u64;
            w.push(s, ns, false);
            s += 256;
        }
        s
    }

    #[test]
    fn a_skewed_clock_reads_its_ppm_once_a_window_is_full() {
        let mut w = RateWindow::new(48_000.0, 60.0, 0.05);
        let s = feed(&mut w, 30.0, 6.0, 40_000.0, 0);
        assert!(w.rate().is_none(), "half a window is no reading");
        feed(&mut w, 40.0, 6.0, 40_000.0, s);
        let r = w.rate().expect("a full window");
        assert!((r.ppm - 6.0).abs() < 0.3, "{r:?}");
        assert!(r.span_s > 54.0 && r.span_s <= 60.0, "{r:?}");
    }

    #[test]
    fn the_window_follows_a_clock_that_changes() {
        let mut w = RateWindow::new(48_000.0, 60.0, 0.05);
        let s = feed(&mut w, 120.0, -20.0, 0.0, 0);
        // The stamps of the second clock continue the first's time line.
        let t0 = (s as f64 / (48_000.0 * (1.0 - 20e-6)) * 1e9 + 1e9) as u64;
        let mut x = s;
        for _ in 0..(70.0 * 48_000.0 / 256.0) as u64 {
            let ns = t0 + ((x - s) as f64 / (48_000.0 * (1.0 + 8e-6)) * 1e9) as u64;
            w.push(x, ns, false);
            x += 256;
        }
        let r = w.rate().expect("a full window");
        assert!((r.ppm - 8.0).abs() < 0.1, "{r:?}");
    }

    #[test]
    fn a_gap_of_unknown_length_moves_nothing() {
        let mut w = RateWindow::new(48_000.0, 60.0, 0.05);
        let s = feed(&mut w, 30.0, 3.0, 0.0, 0);
        // Index jumps by 10 000 samples, the host time by a whole second more.
        let fs = 48_000.0 * (1.0 + 3e-6);
        let mut x = s + 10_000;
        let mut first = true;
        for _ in 0..(35.0 * 48_000.0 / 256.0) as u64 {
            let ns = (x as f64 / fs * 1e9 + 2e9) as u64;
            w.push(x, ns, first);
            first = false;
            x += 256;
        }
        let r = w.rate().expect("pooled segments span the window");
        assert!((r.ppm - 3.0).abs() < 0.1, "{r:?}");
    }

    /// Pairs of reads of a reference clock (index, ns at 1 GHz nominal) and a stamp clock
    /// the reference runs `ppm` fast against, one per `every_s`, with read jitter
    /// ±`jitter_ns`.
    fn pairs(w: &mut RateWindow, seconds: f64, ppm: f64, every_s: f64, jitter_ns: f64) {
        let n = (seconds / every_s) as u64;
        for i in 0..n {
            let reference = (i as f64 * every_s * 1e9) as u64 + 5_000_000_000;
            let h = (i ^ 0x9e37_79b9_7f4a_7c15).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            let u = (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
            let stamp = (reference as f64 / (1.0 + ppm * 1e-6) + u * jitter_ns) as u64;
            w.push(reference, stamp, false);
        }
    }

    #[test]
    fn pairs_of_reads_measure_the_reference_against_the_stamp_clock() {
        let mut w = RateWindow::new(1e9, 60.0, 0.25);
        pairs(&mut w, 70.0, 8.16, 0.05, 2_000.0);
        let r = w.rate().expect("a full window");
        assert!((r.ppm - 8.16).abs() < 0.02, "{r:?}");
    }

    #[test]
    fn a_rate_against_the_stamp_clock_reads_against_the_reference() {
        let at = |ppm| ClockRate {
            hz: 48_000.0 * (1.0 + ppm * 1e-6),
            ppm,
            span_s: 59.9,
        };
        let reference = |ppm| ClockRate {
            hz: 1e9 * (1.0 + ppm * 1e-6),
            ppm,
            span_s: 60.0,
        };
        // A device locked to true time on a host whose stamp clock (the crystal) runs
        // 8.2 ppm slow: +8.2 against the crystal, the disciplined clock +8.2 against it too.
        let device = (1.0 + 0.0) / (1.0 - 8.2e-6) - 1.0;
        let disc = 1.0 / (1.0 - 8.2e-6) - 1.0;
        let r = at(device * 1e6).against(reference(disc * 1e6));
        assert!(r.ppm.abs() < 1e-6, "{r:?}");
        assert!((r.hz - 48_000.0).abs() < 1e-6, "{r:?}");
        assert_eq!(r.span_s, 59.9);
        // A device 5.6 ppm fast on a host whose crystal runs 0.64 ppm fast.
        let device = (1.0 + 5.6e-6) / (1.0 + 0.64e-6) - 1.0;
        let disc = 1.0 / (1.0 + 0.64e-6) - 1.0;
        let r = at(device * 1e6).against(reference(disc * 1e6));
        assert!((r.ppm - 5.6).abs() < 1e-6, "{r:?}");
        // A stamp clock that is the reference changes nothing.
        assert!((at(3.0).against(reference(0.0)).ppm - 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_restarted_index_starts_over() {
        let mut w = RateWindow::new(48_000.0, 60.0, 0.05);
        feed(&mut w, 70.0, 5.0, 0.0, 1_000_000);
        assert!(w.rate().is_some());
        feed(&mut w, 1.0, 5.0, 0.0, 0);
        assert!(w.rate().is_none());
    }
}
