//! What a transfer function between two rigs keeps beside its averages
//! (`docs/design/remote-inputs.md` §3.2–§3.4, §4): the clock gate, the results it may hold
//! while the clocks are apart, the drift of its tracked delay and how often its remote
//! input arrives late.
//!
//! Every average taken while the clocks drift is wrong, and a relation trips only after its
//! window has seen the drift: the 40 s window catches a fallback to a crystal after 20–40 s,
//! the 60 s window a 2 ppm step after up to a minute. So the result is kept every 20 s while
//! the clocks agree, enough of them to reach back past the longest window, and a stop holds
//! the newest one taken before the window that failed began.

use std::collections::VecDeque;

use ac2_core::delay::FinderResult;
use ac2_proto::frame::TfFrame;
use ac2_proto::model::StopWhy;

use super::StampArgs;
use crate::remote::MAX_LINK_OFFSET_S;

/// Spacing of the kept results, s.
const SNAPSHOT_S: f64 = 20.0;
/// The longest relation window, s: the cross-rate's long window and a rig's own clock line.
const LONGEST_WINDOW_S: f64 = 60.0;
/// How far behind the job's audio a failed window can end, s, beyond the link offset: the
/// relation is judged from a report up to a second old and reaches the job up to a second
/// after that (`docs/design/remote-inputs.md` §3.3).
pub(crate) const GATE_LAG_S: f64 = 2.0;
/// Results kept: the oldest is at least `SNAPSHOTS - 1` spacings old, which must reach back
/// past the longest window and the gate's lag, or a stop on that window holds nothing.
const SNAPSHOTS: usize = 5;
const _: () = assert!(
    (SNAPSHOTS - 1) as f64 * SNAPSHOT_S >= LONGEST_WINDOW_S + GATE_LAG_S + MAX_LINK_OFFSET_S
);
/// The tracked delay's drift is fitted over this much audio, s.
const DRIFT_SPAN_S: f64 = 120.0;
/// A steady drift beyond this is a cue, s per minute (0.05 ms/min ≈ 0.8 ppm: below it an
/// 8 s average slides ≤ 7 µs, ≤ 25° at 10 kHz).
const DRIFT_ON: f64 = 0.05e-3;
/// The cue clears below this, so a drift about the bound does not flicker.
const DRIFT_OFF: f64 = 0.04e-3;
/// Steady: the line explains this much of the delay's variance. A path that steps once
/// (a moved speaker) fits a line poorly; clocks apart slide evenly.
const DRIFT_R2: f64 = 0.9;
/// The late cue clears after this long without a late frame, s (at least a settle span).
const LATE_CLEAR_S: f64 = 60.0;

/// Whether a cross-rig transfer function may average, as the control thread judges the
/// clocks between the rigs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ClockGate {
    /// The clocks agree.
    Run,
    /// They do not, or cannot be told to: `window_s` reaches from the job's newest audio
    /// back to the start of the evidence that failed (the result before it is held).
    Stop { why: StopWhy, window_s: f64 },
}

/// What the job tells the control thread about a cross-rig transfer function.
#[derive(Debug)]
pub(crate) enum CrossEvent {
    /// It stopped averaging; `held`: it shows a result from before the failure.
    Stopped { held: bool },
    /// Tracking agreed again after a stop: the arrival before and now, samples.
    Relocked { was: f64, now: f64 },
    /// Tracking found the delay ambiguous after a stop: the operator picks (decision 1c).
    RelockAmbiguous(Box<FinderResult>),
    /// Back after a stop with tracking off: the delay may have moved.
    FindAgain,
    /// The remote input slipped: the arrival moved by this many samples.
    Slipped { by: i64 },
    /// The tracked delay drifts steadily, s per minute; `None`: it does not (any more).
    Drift(Option<f64>),
    /// The remote input is late often enough that the measurement never settles.
    Late(bool),
}

/// A result kept while the clocks agreed.
pub(crate) struct Snapshot {
    /// The sample its audio ends at.
    end: u64,
    /// Its frame header fields as they were when it was kept: a stored trace is dated by
    /// its own audio.
    pub(crate) stamp: StampArgs,
    /// As published (display smoothing on).
    pub(crate) shown: TfFrame,
    /// As captured.
    pub(crate) raw: TfFrame,
}

/// The clock gate's side of a transfer job.
pub(crate) struct CrossRig {
    fs: f64,
    gate: ClockGate,
    /// It averaged at some point since the job started: a later return to agreement is a
    /// relock, not a first start.
    ran: bool,
    /// Kept while running; through a stop only those before every failed window, the
    /// newest of them held.
    snapshots: VecDeque<Snapshot>,
    next_snapshot: u64,
    /// Through a stop: nothing ending after this sample can be held.
    cutoff: Option<u64>,
    /// The arrival held through the current stop, samples.
    was: f64,
    pub(crate) drift: DriftFit,
    pub(crate) late: LateWatch,
}

impl CrossRig {
    /// A cross-rig job starts stopped: nothing averages before the clocks are judged.
    pub(crate) fn new(fs: f64, settle_s: f64) -> Self {
        Self {
            fs,
            gate: ClockGate::Stop {
                why: StopWhy::Measuring,
                window_s: 0.0,
            },
            ran: false,
            snapshots: VecDeque::new(),
            next_snapshot: 0,
            cutoff: None,
            was: 0.0,
            drift: DriftFit::default(),
            late: LateWatch::new(settle_s, fs),
        }
    }

    pub(crate) fn running(&self) -> bool {
        self.gate == ClockGate::Run
    }

    /// The result shown while stopped; `None` while running or with nothing held.
    pub(crate) fn held(&self) -> Option<&Snapshot> {
        self.snapshots.back().filter(|_| !self.running())
    }

    /// A result is due at `end` (every [`SNAPSHOT_S`] of running audio).
    pub(crate) fn snapshot_due(&self, end: u64) -> bool {
        self.running() && end >= self.next_snapshot
    }

    pub(crate) fn keep(&mut self, end: u64, stamp: StampArgs, shown: TfFrame, raw: TfFrame) {
        if self.snapshots.len() == SNAPSHOTS {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(Snapshot {
            end,
            stamp,
            shown,
            raw,
        });
        self.next_snapshot = end + (SNAPSHOT_S * self.fs) as u64;
    }

    /// A new gate at audio `end`, the arrival `arrival` (samples). What changed: `Some(true)`
    /// stopped (or a further stop picked an older result), `Some(false)` released, `None`
    /// nothing to do for the caller.
    pub(crate) fn set_gate(&mut self, gate: ClockGate, end: u64, arrival: f64) -> Option<bool> {
        let was_running = self.running();
        self.gate = gate;
        match gate {
            ClockGate::Stop { window_s, .. } => {
                // A later failure can reach further back than the first (measuring, then a
                // 60 s window tripping): the held result must precede every failed window.
                let cutoff = end.saturating_sub((window_s.max(0.0) * self.fs) as u64);
                let cutoff = self.cutoff.map_or(cutoff, |c| c.min(cutoff));
                self.cutoff = Some(cutoff);
                while self.snapshots.back().is_some_and(|s| s.end > cutoff) {
                    self.snapshots.pop_back();
                }
                if was_running {
                    self.was = arrival;
                }
                Some(true)
            }
            ClockGate::Run if !was_running => {
                self.snapshots.clear();
                self.cutoff = None;
                self.next_snapshot = end + (SNAPSHOT_S * self.fs) as u64;
                Some(false)
            }
            ClockGate::Run => None,
        }
    }

    /// Whether the release just made is a relock (it ran before), and marks it running.
    pub(crate) fn relock(&mut self) -> bool {
        std::mem::replace(&mut self.ran, true)
    }

    /// The arrival held through the stop, samples.
    pub(crate) fn was(&self) -> f64 {
        self.was
    }

    /// The remote input slipped by `by` samples: the held arrival moves with it.
    pub(crate) fn slipped(&mut self, by: i64) {
        self.was += by as f64;
        self.drift.shift(by as f64 / self.fs);
    }
}

/// A line through the tracked arrival over the last [`DRIFT_SPAN_S`], one point a second.
#[derive(Debug, Default)]
pub(crate) struct DriftFit {
    points: VecDeque<(f64, f64)>,
    on: Option<f64>,
}

impl DriftFit {
    /// Forgets the fit; whether a cue was on (it is off now and must be said so).
    pub(crate) fn clear(&mut self) -> bool {
        self.points.clear();
        self.on.take().is_some()
    }

    /// The remote input slipped: the arrival jumped by exactly `by_s`, which is no drift.
    pub(crate) fn shift(&mut self, by_s: f64) {
        for p in &mut self.points {
            p.1 += by_s;
        }
    }

    /// Whether a point is due at audio time `t_s`.
    pub(crate) fn due(&self, t_s: f64) -> bool {
        self.points.back().is_none_or(|p| t_s - p.0 >= 1.0)
    }

    /// Adds the arrival `arrival_s` at audio time `t_s`; the cue when it changed.
    pub(crate) fn add(&mut self, t_s: f64, arrival_s: f64) -> Option<Option<f64>> {
        self.points.push_back((t_s, arrival_s));
        while self
            .points
            .front()
            .is_some_and(|p| t_s - p.0 > DRIFT_SPAN_S)
        {
            self.points.pop_front();
        }
        let next = self.judge();
        let shown = |v: Option<f64>| v.map(|x| (x * 1e6).round() as i64);
        (shown(next) != shown(self.on)).then(|| {
            self.on = next;
            next
        })
    }

    /// The cue: the slope, s per minute, when steady and beyond the bound.
    fn judge(&self) -> Option<f64> {
        let (first, last) = (self.points.front()?, self.points.back()?);
        if last.0 - first.0 < DRIFT_SPAN_S - 1.0 {
            return None;
        }
        let (slope, r2) = fit(self.points.iter().copied())?;
        let per_min = slope * 60.0;
        let bound = if self.on.is_some() {
            DRIFT_OFF
        } else {
            DRIFT_ON
        };
        (per_min.abs() > bound && r2 >= DRIFT_R2).then_some(per_min)
    }
}

/// Least-squares slope and r² of `points`.
fn fit(points: impl Iterator<Item = (f64, f64)> + Clone) -> Option<(f64, f64)> {
    let n = points.clone().count() as f64;
    if n < 3.0 {
        return None;
    }
    let (sx, sy) = points
        .clone()
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let (mx, my) = (sx / n, sy / n);
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in points {
        sxx += (x - mx) * (x - mx);
        sxy += (x - mx) * (y - my);
        syy += (y - my) * (y - my);
    }
    if sxx <= 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    // A delay that never moved is perfectly steady: no drift, nothing to explain.
    let r2 = if syy > 0.0 {
        sxy * sxy / (sxx * syy)
    } else {
        1.0
    };
    Some((slope, r2))
}

/// Late remote audio under one transfer function: two late blocks inside one settle span
/// (its longest MTW stage to full) mean its averages never fill.
#[derive(Debug)]
pub(crate) struct LateWatch {
    settle: u64,
    clear: u64,
    last: Option<u64>,
    on: bool,
}

impl LateWatch {
    pub(crate) fn new(settle_s: f64, fs: f64) -> Self {
        Self {
            settle: (settle_s * fs) as u64,
            clear: (settle_s.max(LATE_CLEAR_S) * fs) as u64,
            last: None,
            on: false,
        }
    }

    /// A block starting at `at` missed remote samples; the cue when it changed.
    pub(crate) fn seen(&mut self, at: u64) -> Option<bool> {
        let again = self
            .last
            .is_some_and(|l| at.saturating_sub(l) <= self.settle);
        self.last = Some(at);
        (again && !self.on).then(|| {
            self.on = true;
            true
        })
    }

    /// Audio reached `end`; the cue when it cleared.
    pub(crate) fn tick(&mut self, end: u64) -> Option<bool> {
        let quiet = self.last.is_none_or(|l| end.saturating_sub(l) > self.clear);
        (self.on && quiet).then(|| {
            self.on = false;
            false
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_steady_drift_is_a_cue_and_a_step_is_not() {
        // 1.5 ppm: 0.09 ms a minute.
        let mut d = DriftFit::default();
        let mut cue = None;
        for t in 0..=125 {
            if let Some(c) = d.add(f64::from(t), 0.012 + 1.5e-6 * f64::from(t)) {
                cue = c;
            }
        }
        let c = cue.expect("a cue");
        assert!((c - 0.09e-3).abs() < 1e-6, "{c}");

        // A path that stepped once by 0.2 ms: no steady drift.
        let mut d = DriftFit::default();
        let mut cue = None;
        for t in 0..=125 {
            let y = 0.012 + if t > 60 { 0.0002 } else { 0.0 };
            if let Some(c) = d.add(f64::from(t), y) {
                cue = c;
            }
        }
        assert_eq!(cue, None);

        // Locked: nothing.
        let mut d = DriftFit::default();
        for t in 0..=125 {
            assert_eq!(d.add(f64::from(t), 0.012), None);
        }
    }

    #[test]
    fn a_slip_is_no_drift() {
        let mut d = DriftFit::default();
        for t in 0..=125 {
            if t == 60 {
                d.shift(-0.0049);
            }
            let y = 0.012 + if t >= 60 { -0.0049 } else { 0.0 };
            assert_eq!(d.add(f64::from(t), y), None);
        }
    }

    #[test]
    fn late_twice_in_a_settle_span_is_a_cue_until_quiet() {
        let fs = 1000.0;
        let mut w = LateWatch::new(10.0, fs);
        assert_eq!(w.seen(0), None);
        assert_eq!(w.seen(20_000), None, "further apart than a settle span");
        assert_eq!(w.seen(25_000), Some(true));
        assert_eq!(w.tick(80_000), None);
        assert_eq!(w.tick(86_000), Some(false));
    }
}
