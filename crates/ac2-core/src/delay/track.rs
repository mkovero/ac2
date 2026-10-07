//! Delay tracking (design Q1 §10.2).

use super::{Band, FinderResult, Outcome};

/// Two agreeing first arrivals closer than this, samples, are repeatable to a fraction of a
/// sample, and the tracker inserts their fractional mean. A looser pair moves the delay in
/// whole samples only: a fraction that wanders by more than this between windows would only
/// turn the phase by noise.
pub const FINE_AGREEMENT_SAMPLES: f64 = 0.1;

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
/// The delay it inserts is whole samples, or the pair's fractional mean when the two agree
/// within [`FINE_AGREEMENT_SAMPLES`]; a fractional insert moves again only when the next
/// such mean is more than that away from it.
///
/// Agreement between windows is repeatability, not correctness: a deterministic wrong
/// arrival would be tracked as confidently as a right one.
#[derive(Debug, Clone, PartialEq)]
pub struct Tracker {
    agreement: Agreement,
    held: Option<i64>,
    /// The delay last inserted (fractional), samples.
    inserted: Option<f64>,
    /// First-arrival delay (fractional) and the end of its meas window.
    pending: Option<(f64, u64)>,
}

impl Tracker {
    pub fn new(agreement: Agreement) -> Self {
        Self {
            agreement,
            held: None,
            inserted: None,
            pending: None,
        }
    }

    /// Feed one finder result; returns the delay to insert, samples, when it moves.
    pub fn observe(&mut self, r: &FinderResult) -> Option<f64> {
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
            let whole_moved = self.held != Some(first.delay);
            self.held = Some(first.delay);
            let insert = if (d - pd).abs() < FINE_AGREEMENT_SAMPLES {
                let fine = 0.5 * (d + pd);
                let moved = self
                    .inserted
                    .is_none_or(|i| (fine - i).abs() > FINE_AGREEMENT_SAMPLES);
                moved.then_some(fine)
            } else {
                whole_moved.then_some(first.delay as f64)
            };
            if insert.is_some() {
                self.inserted = insert;
            }
            return insert;
        }
        self.pending = Some((d, end));
        None
    }

    /// A change of band, threshold, search range or routing: the pending result no longer
    /// compares with the next one.
    pub fn reset(&mut self) {
        self.pending = None;
    }

    /// The held delay, whole samples (the first arrival the local search follows).
    pub fn held(&self) -> Option<i64> {
        self.held
    }

    /// The operator set the delay (inserted a proposal or typed one).
    pub fn set_held(&mut self, delay: Option<i64>) {
        self.held = delay;
        self.inserted = delay.map(|d| d as f64);
        self.pending = None;
    }

    pub fn agreement(&self) -> Agreement {
        self.agreement
    }
}
