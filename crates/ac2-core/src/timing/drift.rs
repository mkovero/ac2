//! The offset model of one offset epoch: a straight line plus steps.
//!
//! Output and input clocks with rate ratio `1 + ε` move the generator → loopback offset by
//! `ε` samples per capture sample; dropped or repeated output frames add integer steps. So
//! within an epoch `offset(x) = c + ε·x + Σ steps`. [`DriftLine`] holds the measured
//! offsets of the last few tens of seconds, fits the line by least squares, predicts the
//! offset of the next window (with its uncertainty, so a step can be told from noise) and
//! takes confirmed steps out by shifting the held points: frames lost on the way do not
//! change the clocks, so the line continues with the same slope.
//!
//! See `docs/design/multi-device.md` §5.

use std::collections::VecDeque;

/// Smallest scatter assumed for one window's offset, samples. A few clean windows can fit a
/// line almost exactly; the next window still carries the estimator's own noise and the
/// parabolic interpolator's bias, both a few hundredths of a sample.
pub const SIGMA_FLOOR: f64 = 0.02;

/// Drift between the output and the input clock, from the slope of the offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftEstimate {
    /// Slope, ppm (offset samples per million capture samples; positive: offset grows).
    pub ppm: f64,
    /// Capture time covered by the regression, s.
    pub span_s: f64,
    /// Above the threshold on a span long enough to judge.
    pub warning: bool,
    /// Capture index of the newest window in the regression (its centre).
    pub end_sample: u64,
}

/// The offset the line expects at a capture index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prediction {
    /// Expected offset, samples.
    pub offset: f64,
    /// Standard deviation of a new window's offset around it: the scatter of the held
    /// points widened by how far the line is extrapolated, samples.
    pub sigma: f64,
}

#[derive(Debug, Clone, Copy)]
struct Fit {
    mx: f64,
    my: f64,
    slope: f64,
    sxx: f64,
    n: f64,
    resid_rms: f64,
}

/// Offset line of one epoch plus the last judged drift, which outlives epochs.
#[derive(Debug, Clone)]
pub struct DriftLine {
    sample_rate: f64,
    /// Regression length, samples.
    horizon: f64,
    /// Shortest span judged, samples.
    min_span: f64,
    threshold_ppm: f64,
    points: VecDeque<(f64, f64)>,
    judged: Option<DriftEstimate>,
}

impl DriftLine {
    /// A line over `window_s` seconds, judged on spans of at least `min_span_s`, warning
    /// above `threshold_ppm`; `points_capacity` points are reserved.
    pub fn new(
        sample_rate: f64,
        window_s: f64,
        min_span_s: f64,
        threshold_ppm: f64,
        points_capacity: usize,
    ) -> Self {
        Self {
            sample_rate,
            horizon: window_s * sample_rate,
            min_span: min_span_s * sample_rate,
            threshold_ppm,
            points: VecDeque::with_capacity(points_capacity),
            judged: None,
        }
    }

    /// Points held.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// No point held (a new epoch, or nothing measured yet).
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Adds the offset `y` measured at capture index `x` (non-decreasing); forgets points
    /// older than the regression length and judges the drift when the span allows.
    pub fn push(&mut self, x: f64, y: f64) {
        while self
            .points
            .front()
            .is_some_and(|&(x0, _)| x - x0 > self.horizon)
        {
            self.points.pop_front();
        }
        self.points.push_back((x, y));
        if let Some(e) = self.current()
            && e.span_s * self.sample_rate >= self.min_span
        {
            self.judged = Some(e);
        }
    }

    /// Takes a confirmed step of `dy` samples out of the held points: they are moved onto
    /// the offset after the step, so the slope is unchanged by it.
    pub fn shift(&mut self, dy: f64) {
        for p in &mut self.points {
            p.1 += dy;
        }
    }

    /// A new offset epoch: the offset may change arbitrarily, so the points go; the judged
    /// drift stays, because the clocks are the same.
    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// Slope of the held points, samples per sample; with fewer than two (spread) points the
    /// judged drift's, if any.
    pub fn slope(&self) -> Option<f64> {
        self.fit()
            .map(|f| f.slope)
            .or_else(|| self.judged.map(|j| j.ppm * 1e-6))
    }

    /// The offset expected at capture index `x`; `None` without points.
    pub fn predict(&self, x: f64) -> Option<Prediction> {
        if let Some(f) = self.fit() {
            let scatter = f.resid_rms.max(SIGMA_FLOOR);
            let lever = 1.0 + 1.0 / f.n + (x - f.mx).powi(2) / f.sxx;
            return Some(Prediction {
                offset: f.my + f.slope * (x - f.mx),
                sigma: scatter * lever.sqrt(),
            });
        }
        let &(x0, y0) = self.points.back()?;
        let slope = self.judged.map_or(0.0, |j| j.ppm * 1e-6);
        Some(Prediction {
            offset: y0 + slope * (x - x0),
            sigma: SIGMA_FLOOR,
        })
    }

    /// Regression over the held points (at least three).
    pub fn current(&self) -> Option<DriftEstimate> {
        if self.points.len() < 3 {
            return None;
        }
        let f = self.fit()?;
        let (&(x0, _), &(x1, _)) = (self.points.front()?, self.points.back()?);
        let span = x1 - x0;
        let ppm = f.slope * 1e6;
        Some(DriftEstimate {
            ppm,
            span_s: span / self.sample_rate,
            warning: span >= self.min_span && ppm.abs() > self.threshold_ppm,
            end_sample: x1.max(0.0).round() as u64,
        })
    }

    /// What to show: the current regression once it spans enough to judge, else the last
    /// judged one (from before a gap or an earlier epoch), else the short current one.
    pub fn estimate(&self) -> Option<DriftEstimate> {
        match (self.current(), self.judged) {
            (Some(c), _) if c.span_s * self.sample_rate >= self.min_span => Some(c),
            (_, Some(j)) => Some(j),
            (c, None) => c,
        }
    }

    fn fit(&self) -> Option<Fit> {
        let n = self.points.len();
        if n < 2 {
            return None;
        }
        let nf = n as f64;
        let mx = self.points.iter().map(|p| p.0).sum::<f64>() / nf;
        let my = self.points.iter().map(|p| p.1).sum::<f64>() / nf;
        let (mut sxy, mut sxx) = (0.0, 0.0);
        for &(x, y) in &self.points {
            sxy += (x - mx) * (y - my);
            sxx += (x - mx) * (x - mx);
        }
        if sxx <= 0.0 {
            return None;
        }
        let slope = sxy / sxx;
        let resid_rms = if n > 2 {
            let ss: f64 = self
                .points
                .iter()
                .map(|&(x, y)| (y - my - slope * (x - mx)).powi(2))
                .sum();
            (ss / (nf - 2.0)).sqrt()
        } else {
            0.0
        };
        Some(Fit {
            mx,
            my,
            slope,
            sxx,
            n: nf,
            resid_rms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::Rng;

    const FS: f64 = 48_000.0;
    const HOP: f64 = 12_000.0;

    fn line() -> DriftLine {
        DriftLine::new(FS, 30.0, 10.0, 2.0, 128)
    }

    /// Feeds `seconds` of windows of `offset(x)` plus Gaussian-ish noise of RMS `noise`.
    fn feed(l: &mut DriftLine, from: f64, seconds: f64, noise: f64, offset: impl Fn(f64) -> f64) {
        let mut rng = Rng::new(3);
        let n = (seconds * FS / HOP) as usize;
        for k in 0..n {
            let x = from + k as f64 * HOP;
            l.push(x, offset(x) + rng.uniform_unit_rms() * noise);
        }
    }

    #[test]
    fn known_drift_is_measured_exactly_without_noise() {
        for ppm in [-120.0, -2.5, 0.0, 1.0, 50.0, 500.0] {
            let mut l = line();
            feed(&mut l, 0.0, 30.0, 0.0, |x| 700.0 + ppm * 1e-6 * x);
            let e = l.estimate().expect("estimate");
            assert!((e.ppm - ppm).abs() < 1e-6, "{ppm}: {e:?}");
            assert!((e.span_s - 29.75).abs() < 1e-9, "{e:?}");
            assert_eq!(e.warning, ppm.abs() > 2.0, "{ppm}");
        }
    }

    #[test]
    fn jitter_of_a_twentieth_of_a_sample_leaves_the_slope_within_a_tenth_ppm() {
        // σ_slope ≈ σ·√(12/n)/S: 0.05·√(12/40)/(10 s·48 kHz) ≈ 0.06 ppm at 10 s.
        for ppm in [0.0, 2.5, 50.0] {
            let mut l = line();
            feed(&mut l, 1e6, 10.0, 0.05, |x| 300.0 + ppm * 1e-6 * x);
            let e = l.estimate().expect("estimate");
            assert!((e.ppm - ppm).abs() < 0.2, "{ppm}: {e:?}");
            let p = l.predict(1e6 + 10.25 * FS).expect("prediction");
            assert!(p.sigma > 0.04 && p.sigma < 0.08, "{p:?}");
        }
    }

    #[test]
    fn short_spans_are_shown_but_never_judged() {
        let mut l = line();
        feed(&mut l, 0.0, 5.0, 0.0, |x| 50e-6 * x);
        let e = l.estimate().expect("estimate");
        assert!((e.ppm - 50.0).abs() < 1e-6 && !e.warning, "{e:?}");
    }

    #[test]
    fn a_shifted_step_does_not_bend_the_slope() {
        // One dropped frame in the middle of the span; the tracker confirms it and shifts.
        let mut l = line();
        feed(&mut l, 0.0, 6.0, 0.0, |x| 100.0 + 3e-6 * x);
        let x = 6.0 * FS;
        let before = l.predict(x).expect("prediction").offset;
        l.shift(-1.0);
        feed(&mut l, x, 6.0, 0.0, |x| 99.0 + 3e-6 * x);
        let e = l.estimate().expect("estimate");
        assert!((e.ppm - 3.0).abs() < 1e-6, "{e:?}");
        assert!((before - (100.0 + 3e-6 * x)).abs() < 1e-9);
    }

    #[test]
    fn an_unshifted_step_reads_as_drift() {
        // Why steps must be taken out: one sample in the middle of 12 s biases the slope by
        // ≈ 1.5/S, here 2.6 ppm, over the 2 ppm threshold.
        let mut l = line();
        feed(&mut l, 0.0, 12.0, 0.0, |x| {
            if x < 6.0 * FS { 100.0 } else { 101.0 }
        });
        let e = l.estimate().expect("estimate");
        assert!(e.warning && e.ppm > 2.0, "{e:?}");
    }

    #[test]
    fn the_judged_drift_outlives_a_new_epoch_and_predicts_across_a_gap() {
        let mut l = line();
        feed(&mut l, 0.0, 20.0, 0.0, |x| 10.0 + 80e-6 * x);
        // A minute without stimulus: the line extrapolates the drift (230 samples).
        let gap_end = 80.0 * FS;
        let p = l.predict(gap_end).expect("prediction");
        assert!((p.offset - (10.0 + 80e-6 * gap_end)).abs() < 1e-6, "{p:?}");
        assert!(p.sigma < 0.1, "{p:?}");
        l.clear();
        assert!(l.is_empty() && l.predict(gap_end).is_none());
        let e = l.estimate().expect("judged survives");
        assert!((e.ppm - 80.0).abs() < 1e-6 && e.warning, "{e:?}");
        assert!((l.slope().expect("slope") - 80e-6).abs() < 1e-12);
        // A new regression replaces it only once it spans enough to judge.
        feed(&mut l, gap_end, 5.0, 0.0, |x| -40.0 + 80.5e-6 * x);
        assert!((l.estimate().expect("e").ppm - 80.0).abs() < 1e-6);
        feed(&mut l, gap_end + 5.0 * FS, 6.0, 0.0, |x| {
            -40.0 + 80.5e-6 * x
        });
        assert!((l.estimate().expect("e").ppm - 80.5).abs() < 1e-6);
    }

    #[test]
    fn old_points_leave_the_regression() {
        let mut l = line();
        feed(&mut l, 0.0, 40.0, 0.0, |x| {
            if x < 20.0 * FS {
                0.0
            } else {
                10e-6 * (x - 20.0 * FS)
            }
        });
        let e = l.estimate().expect("estimate");
        assert!(e.span_s <= 30.0);
        assert!(e.ppm > 2.0 && e.ppm < 10.0, "{e:?}");
        feed(&mut l, 40.0 * FS, 30.0, 0.0, |x| 10e-6 * (x - 20.0 * FS));
        assert!((l.estimate().expect("e").ppm - 10.0).abs() < 1e-6);
    }
}
