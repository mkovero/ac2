//! Headroom advice on a transfer pane: one Info banner (LOWER STIMULUS, RAISE STIMULUS or
//! STIMULUS AT MAX LEVEL; ranked under every fault, warning and other notice) that says whether the stimulus should go down or up. It is
//! advice only: nothing here, nor in the app, ever changes a level.
//!
//! Rules, in order (the first that holds is shown):
//!
//! 1. **Lower**: the reference input's sample peak comes within [`AdviceSettings::ref_headroom_db`]
//!    of full scale (default 3 dB). A loopback reference that close to 0 dBFS clips on the
//!    next crest of a noise stimulus, and a clipped reference corrupts every transfer value.
//! 2. **Raise**: the pane's coherence (median γ² of the columns the stimulus reaches,
//!    [`pane_coherence`]) is under [`AdviceSettings::low_coherence`] (default 0.70) *and* the
//!    measurement input's RMS is under [`AdviceSettings::low_level_dbfs`] (default
//!    −50 dBFS). Low coherence alone may be reflections, wind or a noisy venue, which more
//!    stimulus does not cure; together with a quiet measurement input the likely cause is
//!    too little signal over the noise floor. The step aims the measurement input
//!    [`HYSTERESIS_DB`] past the low-level threshold (so the advice clears once followed),
//!    rounded up to whole dB, and is capped so the stimulus never goes above the system max
//!    level in force; the text says so when the max level is the limit, and names the
//!    input gain as what is left. Without this rig's generator playing (an external
//!    stimulus) no cap is known and none is applied.
//! 3. Otherwise nothing.
//!
//! No advice while a fault explains the signal (CLIP, NO REFERENCE, CHECK ROUTING, NO
//! SIGNAL): the fault says what to fix first, and advice about level would point the wrong
//! way (a clipping input is no reason to raise anything). While the daemon holds the
//! transfer average on a weak reference (`WEAK_REFERENCE`: the stimulus just dropped far
//! under its running level) the advice stands still: the coherence shown is from before
//! the drop and says nothing about the level now.
//!
//! Hysteresis and hold ([`Hold`]): live levels move with every frame (a noise stimulus's
//! peak swings by several dB, γ² by a few hundredths), and a banner that came and went with
//! them would flicker over the plots and could not be read. So:
//! - a shown advice stays while its condition holds within [`HYSTERESIS_DB`] /
//!   [`COHERENCE_HYSTERESIS`] of the threshold, and then for [`HOLD_S`] more;
//! - Raise appears only after its condition has held for [`RAISE_AFTER_S`]: coherence of a
//!   stimulus just started or just changed is still averaging and reads low for a moment;
//!   Lower appears at once, since the next crest may clip;
//! - the raise step and the readings in its text change only when the step moves by
//!   [`HYSTERESIS_DB`] or more.

use ac2_proto::frame::{ProtectionFlags, ValidityMask};

use crate::format;

/// Level hysteresis, dB: a shown advice stays until its level is this far back inside its
/// threshold. Larger than the frame-to-frame swing of a pink-noise peak or RMS reading.
pub const HYSTERESIS_DB: f64 = 3.0;
/// Coherence hysteresis, γ²: Raise stays until the pane's coherence is this far above the
/// threshold. Larger than the frame-to-frame jitter of a median over many columns.
pub const COHERENCE_HYSTERESIS: f64 = 0.05;
/// How long a shown advice stays after its condition stopped holding, s.
pub const HOLD_S: f64 = 5.0;
/// How long the Raise condition must hold before it shows, s.
pub const RAISE_AFTER_S: f64 = 2.0;
/// Smallest raise worth a banner, dB: under it the max level leaves no real room.
pub const MIN_STEP_DB: f64 = 1.0;

/// Choices of [`AdviceSettings::ref_headroom_db`], dB.
pub const REF_HEADROOM_DB: [f64; 6] = [1.0, 2.0, 3.0, 6.0, 9.0, 12.0];
/// Choices of [`AdviceSettings::low_coherence`], γ².
pub const LOW_COHERENCE: [f64; 7] = [0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 0.95];
/// Choices of [`AdviceSettings::low_level_dbfs`], dBFS RMS.
pub const LOW_LEVEL_DBFS: [f64; 9] = [
    -80.0, -70.0, -65.0, -60.0, -55.0, -50.0, -45.0, -40.0, -30.0,
];

/// The thresholds, the operator's (Settings → Display).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdviceSettings {
    /// Off: no advice at all.
    pub on: bool,
    /// Lower when the reference's peak is within this many dB of full scale.
    pub ref_headroom_db: f64,
    /// Raise only while the pane's coherence is under this γ².
    pub low_coherence: f64,
    /// Raise only while the measurement input's RMS is under this, dBFS.
    pub low_level_dbfs: f64,
}

impl Default for AdviceSettings {
    fn default() -> Self {
        Self {
            on: true,
            ref_headroom_db: 3.0,
            low_coherence: 0.7,
            low_level_dbfs: -50.0,
        }
    }
}

/// The next of `choices` from `v`, `d` steps on (clamped at the ends).
pub fn step_choice(choices: &[f64], v: f64, d: i32) -> f64 {
    let i = choices
        .iter()
        .position(|c| (c - v).abs() < 1e-9)
        .unwrap_or_else(|| {
            choices
                .iter()
                .position(|c| *c > v)
                .unwrap_or(choices.len())
                .saturating_sub(1)
        }) as i32;
    choices[(i + d).clamp(0, choices.len() as i32 - 1) as usize]
}

/// Whether `v` is one of `choices` (what a settings file may hold).
pub fn is_choice(choices: &[f64], v: f64) -> bool {
    choices.iter().any(|c| (c - v).abs() < 1e-9)
}

/// The Settings value of each threshold: `within 3 dB of full scale`, `γ² under 0.70`,
/// `under −50.0 dBFS RMS`.
pub fn ref_headroom_text(db: f64) -> String {
    format!(
        "reference peak within {} dB of full scale",
        format::fixed(db, 0)
    )
}
pub fn low_coherence_text(g2: f64) -> String {
    format!("coherence under γ² {}", format::coherence_readout(g2))
}
pub fn low_level_text(dbfs: f64) -> String {
    format!("measurement input under {} RMS", format::dbfs(dbfs))
}
pub fn on_text(on: bool) -> &'static str {
    if on {
        "on: a notice suggests lowering or raising the stimulus (never changes it)"
    } else {
        "off"
    }
}

/// This rig's stimulus while its generator plays: its level and the system max level in
/// force, both dBFS RMS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stimulus {
    pub level_dbfs: f64,
    pub max_level_dbfs: f64,
}

/// What one step reads.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    /// Protection flags of the measurement's newest frame.
    pub protection: ProtectionFlags,
    /// The reference input's sample peak, dBFS.
    pub ref_peak_dbfs: Option<f64>,
    /// The measurement input's RMS, dBFS.
    pub meas_rms_dbfs: Option<f64>,
    /// [`pane_coherence`] of the newest transfer frame.
    pub coherence: Option<f64>,
    pub stimulus: Option<Stimulus>,
}

/// Columns that say nothing about the stimulus's level: no bin there, outside the band,
/// still averaging, no stimulus energy there (a band-limited stimulus), withheld, or a
/// math column. Below-floor and no-measurement columns stay in: a too-quiet measurement
/// input is exactly what makes them.
const NOT_JUDGED: ValidityMask = ValidityMask(
    ValidityMask::THINNED.0
        | ValidityMask::OUT_OF_BAND.0
        | ValidityMask::SETTLING.0
        | ValidityMask::NO_REFERENCE.0
        | ValidityMask::PROTECTED.0
        | ValidityMask::INSUFFICIENT_RESOLUTION.0
        | ValidityMask::ABOVE_NYQUIST.0
        | ValidityMask::FEW_OPERANDS.0,
);

/// The pane's coherence: the median γ² over the judged columns (a column without a value
/// counts as 0), so a few reflection notches do not decide it, a broadly noisy response
/// does. `None` without a judged column.
pub fn pane_coherence(coh: &[f32], validity: &[ValidityMask]) -> Option<f64> {
    let mut v: Vec<f64> = coh
        .iter()
        .enumerate()
        .filter(|(i, _)| validity.get(*i).is_none_or(|m| m.0 & NOT_JUDGED.0 == 0))
        .map(|(_, c)| if c.is_finite() { f64::from(*c) } else { 0.0 })
        .collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// An advice being shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Advice {
    /// Lower the stimulus: the highest reference peak since shown, and the threshold.
    Lower {
        ref_peak_dbfs: f64,
        headroom_db: f64,
    },
    /// Raise the stimulus by `want_db` (whole dB), from the readings that gave it; capped by
    /// `stimulus`'s max level when known.
    Raise {
        want_db: f64,
        coherence: f64,
        meas_rms_dbfs: f64,
        stimulus: Option<Stimulus>,
    },
}

/// The advice of one transfer measurement over time ([`Hold::step`] with each frame).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hold {
    shown: Option<Advice>,
    /// Since when the Raise condition holds while Raise is not shown.
    raise_since: Option<f64>,
    /// Since when the shown advice's condition no longer holds.
    clear_since: Option<f64>,
}

const FAULTS: [ProtectionFlags; 4] = [
    ProtectionFlags::CLIP,
    ProtectionFlags::NO_REFERENCE,
    ProtectionFlags::CHECK_ROUTING,
    ProtectionFlags::NO_SIGNAL,
];

impl Hold {
    pub fn shown(&self) -> Option<Advice> {
        self.shown
    }

    /// Folds one reading at `now_s` (any monotonic clock, s).
    pub fn step(&mut self, now_s: f64, r: &Reading, s: &AdviceSettings) {
        if !s.on || FAULTS.iter().any(|f| r.protection.contains(*f)) {
            *self = Self::default();
            return;
        }
        if r.protection.contains(ProtectionFlags::WEAK_REFERENCE) {
            return;
        }
        let shown_lower = matches!(self.shown, Some(Advice::Lower { .. }));
        if let Some(peak) = r.ref_peak_dbfs {
            let margin = if shown_lower { HYSTERESIS_DB } else { 0.0 };
            if peak > -s.ref_headroom_db - margin {
                let highest = match self.shown {
                    Some(Advice::Lower { ref_peak_dbfs, .. }) => ref_peak_dbfs.max(peak),
                    _ => peak,
                };
                self.shown = Some(Advice::Lower {
                    ref_peak_dbfs: highest,
                    headroom_db: s.ref_headroom_db,
                });
                self.raise_since = None;
                self.clear_since = None;
                return;
            }
        }
        let shown_raise = match self.shown {
            Some(Advice::Raise { want_db, .. }) => Some(want_db),
            _ => None,
        };
        let (coh_margin, level_margin) = if shown_raise.is_some() {
            (COHERENCE_HYSTERESIS, HYSTERESIS_DB)
        } else {
            (0.0, 0.0)
        };
        let raise = match (r.coherence, r.meas_rms_dbfs) {
            (Some(c), Some(rms))
                if c < s.low_coherence + coh_margin && rms < s.low_level_dbfs + level_margin =>
            {
                Some((c, rms))
            }
            _ => None,
        };
        if let Some((c, rms)) = raise {
            let want = (s.low_level_dbfs + HYSTERESIS_DB - rms)
                .ceil()
                .max(MIN_STEP_DB);
            let fresh = Advice::Raise {
                want_db: want,
                coherence: c,
                meas_rms_dbfs: rms,
                stimulus: r.stimulus,
            };
            self.clear_since = None;
            match (shown_raise, self.shown) {
                (
                    Some(w),
                    Some(Advice::Raise {
                        coherence,
                        meas_rms_dbfs,
                        ..
                    }),
                ) if (want - w).abs() < HYSTERESIS_DB => {
                    // The step holds; only the max level, an operator's change, is new.
                    self.shown = Some(Advice::Raise {
                        want_db: w,
                        coherence,
                        meas_rms_dbfs,
                        stimulus: r.stimulus,
                    });
                }
                (Some(_), _) => self.shown = Some(fresh),
                (None, _) => {
                    let since = *self.raise_since.get_or_insert(now_s);
                    if now_s - since >= RAISE_AFTER_S {
                        self.shown = Some(fresh);
                        self.raise_since = None;
                    } else {
                        self.hold_or_clear(now_s);
                    }
                }
            }
            return;
        }
        self.raise_since = None;
        self.hold_or_clear(now_s);
    }

    /// The shown advice's condition does not hold: keep it until [`HOLD_S`] has passed.
    fn hold_or_clear(&mut self, now_s: f64) {
        if self.shown.is_none() {
            return;
        }
        let since = *self.clear_since.get_or_insert(now_s);
        if now_s - since >= HOLD_S {
            self.shown = None;
            self.clear_since = None;
        }
    }
}

/// How far a raise may go: the step, capped by the max level when a stimulus is known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RaiseBy {
    /// The whole step fits.
    Full(f64),
    /// Only this much fits under the max level.
    ToMax { by_db: f64, max_level_dbfs: f64 },
    /// The stimulus is at (or within [`MIN_STEP_DB`] of) the max level.
    AtMax { max_level_dbfs: f64 },
}

/// `want_db` capped so the stimulus never goes above the max level in force.
pub fn raise_by(want_db: f64, stimulus: Option<Stimulus>) -> RaiseBy {
    let Some(s) = stimulus else {
        return RaiseBy::Full(want_db);
    };
    let room = (s.max_level_dbfs - s.level_dbfs).floor();
    if room >= want_db {
        RaiseBy::Full(want_db)
    } else if room >= MIN_STEP_DB {
        RaiseBy::ToMax {
            by_db: room,
            max_level_dbfs: s.max_level_dbfs,
        }
    } else {
        RaiseBy::AtMax {
            max_level_dbfs: s.max_level_dbfs,
        }
    }
}

fn step_db(db: f64) -> String {
    format!("{} dB", format::signed(db, 0))
}

/// The banner's headline and detail. Short enough that both fit a full-width banner row
/// (the detail is left out where they do not, the action is always in the headline); `ref`
/// and `meas` as the input meters tag the two inputs.
pub fn text(a: &Advice) -> (String, String) {
    match *a {
        Advice::Lower {
            ref_peak_dbfs,
            headroom_db,
        } => (
            format!("LOWER STIMULUS · ref peak {}", format::dbfs(ref_peak_dbfs)),
            format!("within {} dB of 0 dBFS", format::fixed(headroom_db, 0)),
        ),
        Advice::Raise {
            want_db,
            coherence,
            meas_rms_dbfs,
            stimulus,
        } => {
            let why = format!(
                "γ² {} · meas {} RMS",
                format::coherence_readout(coherence),
                format::dbfs(meas_rms_dbfs)
            );
            match raise_by(want_db, stimulus) {
                RaiseBy::Full(by) => (format!("RAISE STIMULUS · about {}", step_db(by)), why),
                RaiseBy::ToMax { by_db, .. } => (
                    format!("RAISE STIMULUS · {} to max level", step_db(by_db)),
                    "then raise the meas gain".into(),
                ),
                RaiseBy::AtMax { max_level_dbfs } => (
                    format!("STIMULUS AT MAX LEVEL · {}", format::dbfs(max_level_dbfs)),
                    "raise the meas gain".into(),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests;
