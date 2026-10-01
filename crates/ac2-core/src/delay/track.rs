//! Delay tracking (design Q1 §10.2).

use super::{Band, FinderResult, Outcome};

/// How close two first-arrival delays must be to count as agreeing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Agreement {
    /// Maximum |Δ| between the fractional first-arrival delays, samples.
    pub samples: f64,
}

impl Agreement {
    /// The band's agreement (§9): ±1 sample full range and mid, ±0.1 ms sub.
    pub fn for_band(band: Band, fs: f64) -> Self {
        Self {
            samples: band.class().agreement_samples(fs),
        }
    }
}

/// Moves the held delay only when two `Accepted` results from windows that share no samples
/// agree. `Ambiguous` and `NoEstimate` results clear the pending one and never move it.
///
/// Agreement between windows is repeatability, not correctness: a deterministic wrong
/// arrival would be tracked as confidently as a right one.
#[derive(Debug, Clone, PartialEq)]
pub struct Tracker {
    agreement: Agreement,
    held: Option<i64>,
    /// First-arrival delay (fractional) and the end of its meas window.
    pending: Option<(f64, u64)>,
}

impl Tracker {
    pub fn new(agreement: Agreement) -> Self {
        Self {
            agreement,
            held: None,
            pending: None,
        }
    }

    /// Feed one finder result; returns the new held delay when it moves.
    pub fn observe(&mut self, r: &FinderResult) -> Option<i64> {
        let Outcome::Accepted { first, .. } = &r.outcome else {
            self.pending = None;
            return None;
        };
        let (d, start, end) = (first.delay_frac, r.meas_window.start, r.meas_window.end);
        let Some((pd, pend)) = self.pending else {
            self.pending = Some((d, end));
            return None;
        };
        if start < pend {
            return None; // overlapping windows are the same audio read twice
        }
        if (d - pd).abs() <= self.agreement.samples {
            self.pending = None;
            if self.held != Some(first.delay) {
                self.held = Some(first.delay);
                return Some(first.delay);
            }
            return None;
        }
        self.pending = Some((d, end));
        None
    }

    /// A change of band, threshold, search range or routing: the pending result no longer
    /// compares with the next one.
    pub fn reset(&mut self) {
        self.pending = None;
    }

    /// The held delay.
    pub fn held(&self) -> Option<i64> {
        self.held
    }

    /// The operator set the delay (inserted a proposal or typed one).
    pub fn set_held(&mut self, delay: Option<i64>) {
        self.held = delay;
        self.pending = None;
    }

    pub fn agreement(&self) -> Agreement {
        self.agreement
    }
}
