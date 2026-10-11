//! The timing state machine and the [`LoopbackTiming`] bundle of estimator and tracker.

use super::{
    DriftEstimate, DriftLine, GccPhat, LagRange, NoEstimate, Outcome, Peak, TimingConfig,
    TimingError, WindowMeasurement,
};

// ---------------------------------------------------------------------------------------
// State machine

/// Monitor state (Q3 diagram).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimingState {
    /// No generator output; [`TimingTracker::last_lock`] still reports the last lock.
    NoStimulus,
    /// New epoch or stimulus restart; collecting agreeing windows.
    Acquiring,
    /// Offset validated.
    Locked {
        /// Current offset, samples.
        offset: i64,
    },
    /// A jump was confirmed by the last window; the next window reports Locked.
    Jumped {
        /// Offset before the jump.
        from: i64,
        /// Offset after the jump.
        to: i64,
    },
    /// Stimulus present but no confident offset for several windows; searching wide.
    Lost,
}

/// Events the daemon publishes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimingEvent {
    /// First lock in an epoch, or re-lock at the epoch's previous offset.
    Locked {
        /// Offset epoch.
        epoch: u64,
        /// Offset, samples.
        offset: i64,
        /// Capture index of the window that completed the lock.
        at_capture_sample: u64,
    },
    /// The offset changed within an epoch (`timing.jump`).
    Jump {
        /// Offset epoch.
        epoch: u64,
        /// Offset before, samples.
        from: i64,
        /// Offset after, samples.
        to: i64,
        /// Start of the first capture window showing the new offset; the jump lies at or
        /// before the end of that window.
        at_capture_sample: u64,
    },
    /// Confidence lost while the stimulus is present.
    Lost {
        /// Offset epoch.
        epoch: u64,
        /// Capture index of the window that declared it.
        at_capture_sample: u64,
    },
    /// The generator stopped.
    StimulusOff {
        /// Offset epoch.
        epoch: u64,
        /// Capture index of the first window without stimulus.
        at_capture_sample: u64,
    },
    /// Drift exceeded the threshold: input and output are on different clocks.
    DriftWarning {
        /// Offset epoch.
        epoch: u64,
        /// Estimated drift, ppm (positive: offset grows).
        ppm: f64,
    },
}

/// Up to two events from one window (for example a lock completing while drift is judged).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TimingEvents {
    slots: [Option<TimingEvent>; 2],
}

impl TimingEvents {
    fn push(&mut self, e: TimingEvent) {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.is_none()) {
            *slot = Some(e);
        }
    }

    /// The events, in order.
    pub fn iter(&self) -> impl Iterator<Item = &TimingEvent> {
        self.slots.iter().flatten()
    }

    /// True if no event happened.
    pub fn is_empty(&self) -> bool {
        self.slots[0].is_none()
    }
}

/// The most recent lock, kept across stimulus gaps and epochs for display with its age.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LastLock {
    /// Epoch it belongs to.
    pub epoch: u64,
    /// Offset, samples.
    pub offset: i64,
    /// Capture index of the last window that confirmed it.
    pub at_capture_sample: u64,
}

/// Windows that agree on one offset line: an acquisition, or the new offset after a jump.
#[derive(Debug, Clone)]
struct Candidate {
    /// Integer offset of the newest window.
    offset: i64,
    /// Capture start of the first window.
    first_start: u64,
    /// (window centre, offset) of each window, oldest first.
    points: Vec<(f64, f64)>,
}

impl Candidate {
    fn new(x: f64, y: f64, offset: i64, start: u64) -> Self {
        let mut points = Vec::with_capacity(8);
        points.push((x, y));
        Self {
            offset,
            first_start: start,
            points,
        }
    }

    fn count(&self) -> usize {
        self.points.len()
    }

    /// Whether a window at (`x`, `y`) lies on this candidate's line: its own slope once it
    /// has two windows, else `slope` (the drift known so far); with neither, any drift up
    /// to the largest followed is allowed. The candidate's own windows take precedence, so
    /// a drift line fitted on too short a span cannot keep a correct candidate out.
    fn agrees(&self, x: f64, y: f64, slope: Option<f64>, cfg: &TimingConfig) -> bool {
        let (&(x0, y0), &(x1, y1)) = match (self.points.first(), self.points.last()) {
            (Some(a), Some(b)) => (a, b),
            _ => return false,
        };
        let own = (x1 > x0).then(|| (y1 - y0) / (x1 - x0));
        let max_slope = cfg.max_drift_ppm * 1e-6;
        match own.or(slope) {
            Some(s) if s.abs() <= max_slope + cfg.agree_samples / (x1 - x0).max(1.0) => {
                (y - (y1 + s * (x - x1))).abs() <= cfg.agree_samples
            }
            Some(_) => false,
            None => (y - y1).abs() <= cfg.agree_samples + max_slope * (x - x1).abs(),
        }
    }

    fn add(&mut self, x: f64, y: f64, offset: i64) {
        self.points.push((x, y));
        self.offset = offset;
    }
}

/// Q3 state machine and drift model; pure logic over [`WindowMeasurement`]s.
///
/// Every window is judged against the offset the epoch's [`DriftLine`] predicts for it, so
/// a drifting offset is followed however fast it moves (up to
/// [`TimingConfig::max_drift_ppm`]) and a step is told from drift by its size against the
/// prediction's uncertainty (`docs/design/multi-device.md` §5).
#[derive(Debug)]
pub struct TimingTracker {
    cfg: TimingConfig,
    epoch: u64,
    state: TimingState,
    last_lock: Option<LastLock>,
    acquire: Option<Candidate>,
    jump: Option<Candidate>,
    misses: usize,
    /// Consecutive locked windows off the line that did not confirm a jump.
    off_line: usize,
    wide_next: bool,
    /// Offsets of the epoch's validated windows; kept through Lost and NoStimulus so a
    /// re-lock elsewhere than the line predicts is reported as a jump instead of passing
    /// silently.
    line: DriftLine,
    drift_warning: bool,
}

impl TimingTracker {
    /// Tracker in epoch 0, state NoStimulus.
    pub fn new(cfg: TimingConfig) -> Self {
        let capacity =
            (cfg.drift_window_s * cfg.sample_rate / cfg.hop.max(1) as f64).ceil() as usize + 4;
        Self {
            cfg,
            epoch: 0,
            state: TimingState::NoStimulus,
            last_lock: None,
            acquire: None,
            jump: None,
            misses: 0,
            off_line: 0,
            wide_next: false,
            line: DriftLine::new(
                cfg.sample_rate,
                cfg.drift_window_s,
                cfg.drift_min_span_s,
                cfg.drift_threshold_ppm,
                cfg.slope_sigma_ppm,
                capacity,
            ),
            drift_warning: false,
        }
    }

    /// Current state.
    pub fn state(&self) -> TimingState {
        self.state
    }

    /// Current offset epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Last validated lock, possibly from an earlier epoch.
    pub fn last_lock(&self) -> Option<LastLock> {
        self.last_lock
    }

    /// The generator may stand in for the reference only when locked on one clock.
    pub fn internal_reference_allowed(&self) -> bool {
        matches!(self.state, TimingState::Locked { .. }) && !self.drift_warning
    }

    /// Starts a new offset epoch (stream open, device/rate/buffer change, continuity break,
    /// xrun). The offset may legitimately change, so the next lock is not a jump. The
    /// judged drift stays: the clocks are those of the same stream.
    pub fn new_epoch(&mut self) {
        self.epoch += 1;
        self.line.clear();
        self.acquire = None;
        self.jump = None;
        self.misses = 0;
        self.off_line = 0;
        self.wide_next = false;
        if self.state != TimingState::NoStimulus {
            self.state = TimingState::Acquiring;
        }
    }

    /// Range the next window should search.
    pub fn search_range(&self) -> LagRange {
        let locked = match self.state {
            TimingState::Locked { offset } | TimingState::Jumped { to: offset, .. } => offset,
            _ => return self.cfg.acquisition,
        };
        let r = self.cfg.track_radius;
        let candidate_far = self
            .jump
            .as_ref()
            .is_some_and(|c| (c.offset - locked).abs() as f64 > r as f64 - self.cfg.agree_samples);
        if self.wide_next || candidate_far {
            self.cfg.acquisition
        } else {
            LagRange {
                min: locked - r,
                max: locked + r,
            }
        }
    }

    /// Drift between the output and the input clock: the current regression once it spans
    /// enough to judge, else the last judged one of this stream, else the short current one.
    pub fn drift(&self) -> Option<DriftEstimate> {
        self.line.estimate()
    }

    /// Feeds one window's measurement.
    pub fn observe(&mut self, m: &WindowMeasurement) -> TimingEvents {
        let mut ev = TimingEvents::default();
        if let TimingState::Jumped { to, .. } = self.state {
            self.state = TimingState::Locked { offset: to };
        }
        match m.outcome {
            Outcome::NoStimulus => {
                if self.state != TimingState::NoStimulus {
                    self.state = TimingState::NoStimulus;
                    ev.push(TimingEvent::StimulusOff {
                        epoch: self.epoch,
                        at_capture_sample: m.capture_start,
                    });
                }
                self.reset_search();
            }
            // The stimulus cannot be timed to a sample, which says nothing about the loopback:
            // the state and the search stay as they are.
            Outcome::NoEstimate(NoEstimate::Narrowband) => {}
            Outcome::NoEstimate(_) => {
                if self.state == TimingState::NoStimulus {
                    self.state = TimingState::Acquiring;
                }
                self.misses += 1;
                self.wide_next = true;
                if self.misses >= self.cfg.lost_after_windows && self.state != TimingState::Lost {
                    self.state = TimingState::Lost;
                    self.reset_search();
                    ev.push(TimingEvent::Lost {
                        epoch: self.epoch,
                        at_capture_sample: m.capture_start,
                    });
                }
            }
            Outcome::Offset(p) => {
                if self.state == TimingState::NoStimulus {
                    self.state = TimingState::Acquiring;
                }
                self.misses = 0;
                match self.state {
                    TimingState::Locked { offset } => self.track(offset, p, m, &mut ev),
                    _ => self.acquire(p, m, &mut ev),
                }
            }
        }
        let drift = self.drift().filter(|d| d.warning);
        if let Some(d) = drift
            && !self.drift_warning
        {
            ev.push(TimingEvent::DriftWarning {
                epoch: self.epoch,
                ppm: d.ppm,
            });
        }
        self.drift_warning = drift.is_some();
        ev
    }

    fn reset_search(&mut self) {
        self.acquire = None;
        self.jump = None;
        self.off_line = 0;
    }

    /// Capture index a window's offset belongs to: its centre.
    fn centre(&self, m: &WindowMeasurement) -> f64 {
        m.capture_start as f64 + self.cfg.window as f64 / 2.0
    }

    /// Distance from the line's prediction beyond which a window is a jump candidate:
    /// three standard deviations of a window's offset around the line, never less than
    /// [`TimingConfig::jump_samples`]. Under drift a window's stimulus slides by `W·ε`
    /// while it is captured, and its peak can sit anywhere along that smear (σ of a uniform
    /// spread, `W·ε/√12`). That scatter is a property of the drift, so it holds even right
    /// after a lock, when a few windows cannot yet show it in their residuals.
    fn jump_threshold(&self, sigma: f64) -> f64 {
        let smear = self.line.slope().map_or(0.0, |s| s.abs()) * self.cfg.window as f64;
        let sigma = sigma.max(smear / 12f64.sqrt());
        self.cfg.jump_samples.max(3.0 * sigma)
    }

    fn acquire(&mut self, p: Peak, m: &WindowMeasurement, ev: &mut TimingEvents) {
        let (x, y) = (self.centre(m), p.offset as f64 + p.fraction);
        let slope = self.line.slope();
        let c = match self.acquire.take() {
            Some(mut c) if c.agrees(x, y, slope, &self.cfg) => {
                c.add(x, y, p.offset);
                c
            }
            _ => Candidate::new(x, y, p.offset, m.capture_start),
        };
        let disjoint = m.capture_start >= c.first_start + self.cfg.window as u64;
        if c.count() < self.cfg.acquire_windows || !disjoint {
            self.acquire = Some(c);
            return;
        }
        self.wide_next = false;
        // A re-lock within the epoch (after a gap or a loss) is judged against the drift line
        // extrapolated across the gap, at the newest window: the first may straddle the
        // stimulus onset.
        let (xn, yn) = c.points[c.count() - 1];
        match self.line.predict(xn) {
            Some(q) if (yn - q.offset).abs() > self.jump_threshold(q.sigma) => {
                let from = q.offset.round() as i64;
                self.line.shift(yn - q.offset);
                self.state = TimingState::Jumped { from, to: p.offset };
                ev.push(TimingEvent::Jump {
                    epoch: self.epoch,
                    from,
                    to: p.offset,
                    at_capture_sample: c.first_start,
                });
            }
            _ => {
                self.state = TimingState::Locked { offset: p.offset };
                ev.push(TimingEvent::Locked {
                    epoch: self.epoch,
                    offset: p.offset,
                    at_capture_sample: m.capture_start,
                });
            }
        }
        for &(px, py) in &c.points {
            self.line.push(px, py);
        }
        self.confirm(p, m);
    }

    fn track(&mut self, locked: i64, p: Peak, m: &WindowMeasurement, ev: &mut TimingEvents) {
        let (x, y) = (self.centre(m), p.offset as f64 + p.fraction);
        let on_line = match self.line.predict(x) {
            Some(q) => (y - q.offset).abs() <= self.jump_threshold(q.sigma),
            None => (y - locked as f64).abs() <= self.cfg.agree_samples,
        };
        if on_line {
            // Clock drift moves the offset steadily; the line follows it.
            self.jump = None;
            self.off_line = 0;
            self.wide_next = false;
            self.state = TimingState::Locked { offset: p.offset };
            self.line.push(x, y);
            self.confirm(p, m);
            return;
        }
        let slope = self.line.slope();
        let c = match self.jump.take() {
            Some(mut c) if c.agrees(x, y, slope, &self.cfg) => {
                c.add(x, y, p.offset);
                c
            }
            _ => Candidate::new(x, y, p.offset, m.capture_start),
        };
        if m.capture_start < c.first_start + self.cfg.window as u64 {
            self.jump = Some(c);
            self.off_line += 1;
            // Off the line, yet no new line forms: the windows scatter more than the model
            // allows (drift beyond the largest followed, or a stimulus the estimator cannot
            // time). Locked would claim an offset nothing confirms any more.
            let confirm_windows = self.cfg.window.div_ceil(self.cfg.hop.max(1));
            if self.off_line > confirm_windows + self.cfg.lost_after_windows {
                self.state = TimingState::Lost;
                self.reset_search();
                self.wide_next = true;
                ev.push(TimingEvent::Lost {
                    epoch: self.epoch,
                    at_capture_sample: m.capture_start,
                });
            }
            return;
        }
        self.off_line = 0;
        // The step is measured against the drifted offset at the newest window: the first
        // one may straddle the step and read part of it.
        let (xn, yn) = c.points[c.count() - 1];
        if let Some(q) = self.line.predict(xn) {
            self.line.shift(yn - q.offset);
        } else {
            self.line.clear();
        }
        for &(px, py) in &c.points {
            self.line.push(px, py);
        }
        self.wide_next = false;
        self.state = TimingState::Jumped {
            from: locked,
            to: p.offset,
        };
        ev.push(TimingEvent::Jump {
            epoch: self.epoch,
            from: locked,
            to: p.offset,
            at_capture_sample: c.first_start,
        });
        self.confirm(p, m);
    }

    /// Records a validated offset as the last lock.
    fn confirm(&mut self, p: Peak, m: &WindowMeasurement) {
        self.last_lock = Some(LastLock {
            epoch: self.epoch,
            offset: p.offset,
            at_capture_sample: m.capture_start,
        });
    }
}

/// Estimator plus tracker: the whole Q3 monitor minus threads and I/O.
///
/// Per hop: ask [`LoopbackTiming::search_range`], slice W capture samples from
/// `capture_start` and `range.reference_len(W)` history samples from output index
/// `range.reference_start(capture_start)`, then call [`LoopbackTiming::process_window`].
#[derive(Debug)]
pub struct LoopbackTiming {
    cfg: TimingConfig,
    estimator: GccPhat,
    tracker: TimingTracker,
}

impl LoopbackTiming {
    /// Builds the monitor; allocates FFT plans for tracking and acquisition spans.
    pub fn new(cfg: TimingConfig) -> Self {
        let max_span = cfg
            .acquisition
            .span()
            .max(2 * cfg.track_radius.max(0) as usize);
        Self {
            estimator: GccPhat::new(cfg.window, max_span),
            tracker: TimingTracker::new(cfg),
            cfg,
        }
    }

    /// Configuration.
    pub fn config(&self) -> &TimingConfig {
        &self.cfg
    }

    /// The state machine.
    pub fn tracker(&self) -> &TimingTracker {
        &self.tracker
    }

    /// Range the next window should search.
    pub fn search_range(&self) -> LagRange {
        self.tracker.search_range()
    }

    /// Starts a new offset epoch.
    pub fn new_epoch(&mut self) {
        self.tracker.new_epoch();
    }

    /// Measures one window and feeds the tracker.
    pub fn process_window(
        &mut self,
        capture_start: u64,
        capture: &[f32],
        reference: &[f32],
        range: LagRange,
    ) -> Result<(WindowMeasurement, TimingEvents), TimingError> {
        let m = self
            .estimator
            .measure(&self.cfg, capture_start, capture, reference, range)?;
        Ok((m, self.tracker.observe(&m)))
    }

    /// Feeds the tracker a window whose stimulus is below the floor, judged from only the
    /// newest W history samples (see [`GccPhat::measure_no_stimulus`]); the outcome, levels
    /// and events are exactly those [`Self::process_window`] gives for the same window.
    /// `None`, with nothing fed: the stimulus is present, so the window needs
    /// [`Self::process_window`] with its whole history slice.
    pub fn process_if_no_stimulus(
        &mut self,
        capture_start: u64,
        capture: &[f32],
        newest: &[f32],
    ) -> Result<Option<(WindowMeasurement, TimingEvents)>, TimingError> {
        let m = self
            .estimator
            .measure_no_stimulus(&self.cfg, capture_start, capture, newest)?;
        Ok(m.map(|m| (m, self.tracker.observe(&m))))
    }
}
