//! EQ what-if sets (`docs/design/eq-overlay.md`): the filters a user plans to set on a
//! console or processor, kept on a measurement. The daemon evaluates them; clients only
//! show and edit them.

use serde::{Deserialize, Serialize};

use crate::units::{Db, EqSetId, Hz, MeasId, Seconds, TraceId};

use super::Operand;

/// How a set's sections are realised. A processor runs every section the same way.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EqRealisation {
    /// Bilinear transform prewarped at each section's f, at the processor's sample rate
    /// (shows its cramping near Nyquist; columns at or above fs/2 are gaps).
    Digital {
        /// The processor's sample rate.
        rate: Hz,
    },
    /// The s-domain prototype (decramped or FIR-based processors).
    Analog,
}

/// What a peaking section's Q measures. Named by what it measures, never by a brand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeakQ {
    /// The symmetric (cookbook) peaking EQ; Q between the half-gain points.
    HalfGain,
    /// The same filter, Q read between the points 3 dB below the peak (above the dip).
    Minus3dbFromPeak,
    /// The band-pass-sum family; Q of the added band-pass.
    BandPass,
}

/// A free bandwidth: as Q or in octaves (`1/Q = 2·sinh(ln2/2 · N)`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Bandwidth {
    /// Quality factor.
    Q {
        /// Q.
        value: f64,
    },
    /// Octaves.
    Octaves {
        /// Octaves.
        n: f64,
    },
}

/// A second-order shelf's slope.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShelfSlope {
    /// Quality factor.
    Q {
        /// Q.
        value: f64,
    },
    /// The cookbook slope S (1: the steepest without overshoot).
    S {
        /// S.
        value: f64,
    },
    /// Octaves, read as a Q.
    Octaves {
        /// Octaves.
        n: f64,
    },
}

/// A shelf's order.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShelfOrder {
    /// First order: pole and zero a factor of the gain apart.
    First,
    /// Second order (cookbook) with a slope.
    Second {
        /// Slope.
        slope: ShelfSlope,
    },
}

/// Which frequency of a shelf `f` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShelfFreq {
    /// Where the gain is half the dB gain (cookbook).
    Midpoint,
    /// 3 dB from flat on the flat side.
    Corner,
}

/// Normalisation of a Bessel high- or low-pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BesselNorm {
    /// Asymptotic phase matches the Butterworth's (the common crossover convention).
    Phase,
    /// Unit group delay: f is 1/(2πτ).
    Delay,
    /// −3 dB at f.
    Minus3db,
}

/// A high- or low-pass alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Alignment {
    /// −3 dB at f.
    Butterworth,
    /// Even orders, −6 dB at f.
    LinkwitzRiley,
    /// Bessel of a normalisation.
    Bessel {
        /// Normalisation.
        norm: BesselNorm,
    },
}

/// One section's shape and parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EqShape {
    /// Peaking (bell).
    Peaking {
        /// Centre.
        f: Hz,
        /// Boost or cut.
        gain: Db,
        /// Bandwidth as entered.
        bw: Bandwidth,
        /// What the entered Q measures; `None`: the set's [`EqSet::peak_q`].
        q_measured: Option<PeakQ>,
    },
    /// Low shelf.
    LowShelf {
        /// Frequency named by `f_at`.
        f: Hz,
        /// Gain.
        gain: Db,
        /// Order and slope.
        order: ShelfOrder,
        /// Which frequency `f` is.
        f_at: ShelfFreq,
    },
    /// High shelf.
    HighShelf {
        /// Frequency named by `f_at`.
        f: Hz,
        /// Gain.
        gain: Db,
        /// Order and slope.
        order: ShelfOrder,
        /// Which frequency `f` is.
        f_at: ShelfFreq,
    },
    /// High-pass of an alignment, order 1 … 8.
    HighPass {
        /// Corner.
        f: Hz,
        /// Alignment.
        alignment: Alignment,
        /// Order (6 dB/oct each).
        order: u8,
    },
    /// Low-pass of an alignment, order 1 … 8.
    LowPass {
        /// Corner.
        f: Hz,
        /// Alignment.
        alignment: Alignment,
        /// Order (6 dB/oct each).
        order: u8,
    },
    /// One free second-order high-pass section.
    HighPassQ {
        /// Natural frequency.
        f: Hz,
        /// Q.
        q: f64,
    },
    /// One free second-order low-pass section.
    LowPassQ {
        /// Natural frequency.
        f: Hz,
        /// Q.
        q: f64,
    },
    /// Notch; bandwidth between its −3 dB points.
    Notch {
        /// Centre.
        f: Hz,
        /// Bandwidth.
        bw: Bandwidth,
    },
    /// Band-pass, 0 dB peak.
    BandPass {
        /// Centre.
        f: Hz,
        /// Bandwidth.
        bw: Bandwidth,
    },
    /// First-order all-pass (90° point).
    #[serde(rename = "all_pass_1")]
    AllPass1 {
        /// 90° point.
        f: Hz,
    },
    /// Second-order all-pass (180° point).
    #[serde(rename = "all_pass_2")]
    AllPass2 {
        /// 180° point.
        f: Hz,
        /// Q.
        q: f64,
    },
    /// Broadband gain.
    Gain {
        /// Gain.
        gain: Db,
    },
    /// Polarity inversion.
    Polarity,
    /// Pure delay, exact.
    Delay {
        /// Delay.
        seconds: Seconds,
    },
    /// A raw biquad at its own rate (`a0 = 1`), always digital.
    Biquad {
        /// b0.
        b0: f64,
        /// b1.
        b1: f64,
        /// b2.
        b2: f64,
        /// a1.
        a1: f64,
        /// a2.
        a2: f64,
        /// Its sample rate.
        rate: Hz,
    },
    /// A stored transfer trace multiplied in (a FIR's exported response, a preset measured).
    Response {
        /// The trace.
        trace: TraceId,
    },
}

/// One section of a set.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqSection {
    /// Off: skipped.
    pub enabled: bool,
    /// Shape.
    pub shape: EqShape,
}

/// What a set holds, as an edit sends it (`meas.eq` `put`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqBody {
    /// Name (`EQ 1`), also the name of its filter curve.
    pub name: String,
    /// Realisation.
    pub realisation: EqRealisation,
    /// The default reading of a peaking section's Q.
    pub peak_q: PeakQ,
    /// Off: the set evaluates as no filter (its bypass).
    pub enabled: bool,
    /// Sections, in cascade order.
    pub sections: Vec<EqSection>,
}

/// A set as it was when marked "in the console": what the curves measured since contain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqSnapshot {
    /// The set.
    pub set: EqSetId,
    /// Its rev when marked.
    pub rev: u32,
    /// Its realisation, default Q reading, bypass and sections when marked.
    pub body: EqBody,
}

/// An EQ set of a measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqSet {
    /// Id, unique within the session.
    pub id: EqSetId,
    /// Bumped on every edit.
    pub rev: u32,
    /// What it holds.
    pub body: EqBody,
    /// The snapshot marked "in the console", if any.
    pub in_console: Option<EqSnapshot>,
    /// Its filter curve: a locked transfer trace the daemon keeps.
    pub trace: TraceId,
}

impl EqSet {
    /// Most sets a measurement holds.
    pub const MAX_SETS: usize = 8;
    /// Most sections a set holds.
    pub const MAX_SECTIONS: usize = 64;

    /// What a prediction of a curve that recorded `eq_in` (`None`: nothing of this set
    /// recorded) shows.
    pub fn basis(&self, eq_in: Option<&EqSnapshot>) -> EqBasis {
        match eq_in {
            None => EqBasis::Whole,
            Some(s) if active(&s.body) == active(&self.body) => EqBasis::NoChange { rev: s.rev },
            Some(s) => EqBasis::Change { since: s.rev },
        }
    }

    /// The snapshot of this set now.
    pub fn snapshot(&self) -> EqSnapshot {
        EqSnapshot {
            set: self.id,
            rev: self.rev,
            body: self.body.clone(),
        }
    }
}

/// What a body filters: its realisation and enabled sections, or nothing when bypassed.
/// Two bodies with the same active filter are the same filter.
fn active(b: &EqBody) -> Option<(EqRealisation, PeakQ, Vec<EqShape>)> {
    b.enabled.then(|| {
        (
            b.realisation,
            b.peak_q,
            b.sections
                .iter()
                .filter(|s| s.enabled)
                .map(|s| s.shape)
                .collect(),
        )
    })
}

/// What a prediction shows, by what its curve is known to contain of the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EqBasis {
    /// The curve contains none of it: the whole set.
    Whole,
    /// The curve contains the set as of `since`: only the change `H_now / H_since`.
    Change {
        /// The rev the curve contains.
        since: u32,
    },
    /// The curve contains exactly what the set is now: nothing to predict.
    NoChange {
        /// The rev the curve contains.
        rev: u32,
    },
}

/// One `meas.eq` edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EqEdit {
    /// A new set (`EQ n` unless named), predicting the live curve.
    New {
        /// Name; `None`: the next free `EQ n`.
        name: Option<String>,
        /// Realisation; `None`: digital at the session rate (48 kHz without a session).
        realisation: Option<EqRealisation>,
        /// Default Q reading; `None`: half gain.
        peak_q: Option<PeakQ>,
        /// Sections to start with.
        sections: Vec<EqSection>,
    },
    /// Replace what a set holds; its rev goes up, its curve and predictions follow.
    Put {
        /// The set.
        set: EqSetId,
        /// What it holds now.
        body: EqBody,
    },
    /// Delete a set, its filter curve and its predictions.
    Delete {
        /// The set.
        set: EqSetId,
    },
    /// Mark the set as it is now "in the console" (`clear`: the console was reset).
    Mark {
        /// The set.
        set: EqSetId,
        /// Clear the mark instead.
        clear: bool,
    },
    /// Show (or stop showing) a curve of the measurement after the set: a math channel
    /// `curve × set`.
    Predict {
        /// The set.
        set: EqSetId,
        /// The live measurement or one of its stored transfer traces.
        curve: Operand,
        /// On or off.
        on: bool,
    },
    /// Copy the set to another measurement (a copy, not shared).
    Copy {
        /// The set.
        set: EqSetId,
        /// Measurement it is copied to.
        to: MeasId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::samples::{eq_edits, eq_measurement};

    #[test]
    fn every_edit_and_set_round_trips() {
        for e in eq_edits() {
            let b = rmp_serde::to_vec_named(&e).expect("encodes");
            assert_eq!(rmp_serde::from_slice::<EqEdit>(&b).expect("decodes"), e);
        }
        let m = eq_measurement();
        let b = rmp_serde::to_vec_named(&m.eq).expect("encodes");
        assert_eq!(
            rmp_serde::from_slice::<Vec<EqSet>>(&b).expect("decodes"),
            m.eq
        );
    }

    #[test]
    fn basis_follows_what_the_curve_contains() {
        let mut set = eq_measurement().eq.remove(0);
        assert_eq!(set.basis(None), EqBasis::Whole);
        let marked = set.in_console.clone().expect("marked");
        assert_eq!(set.basis(Some(&marked)), EqBasis::Change { since: 4 });
        let now = set.snapshot();
        assert_eq!(set.basis(Some(&now)), EqBasis::NoChange { rev: 5 });
        // A disabled section changes nothing filtered, so it is still no change; a
        // bypassed set is a change (it takes the EQ out).
        set.body.sections.push(EqSection {
            enabled: false,
            shape: EqShape::Polarity,
        });
        assert_eq!(set.basis(Some(&now)), EqBasis::NoChange { rev: 5 });
        set.body.enabled = false;
        assert_eq!(set.basis(Some(&now)), EqBasis::Change { since: 5 });
    }
}
