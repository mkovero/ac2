//! Judgements of a window against its limit, the peak hold and the release latch.

use super::{Second, WindowValue};
use crate::spectrum::power_dbfs;

/// A window's state against its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Judgement {
    /// More than the margin below the limit.
    Ok,
    /// Within the margin below the limit, or at it.
    Near,
    /// Above the limit.
    Over,
}

/// `v` at the 0.1 dB resolution levels are shown at.
pub fn round_tenth(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Judges a Leq against `limit_db` with a warn `margin_db`, at the displayed 0.1 dB
/// resolution so the state never disagrees with the number shown. `None` without a value.
pub fn judge(leq_db: f64, limit_db: f64, margin_db: f64) -> Option<Judgement> {
    if !leq_db.is_finite() {
        return (leq_db == f64::NEG_INFINITY).then_some(Judgement::Ok);
    }
    let l = round_tenth(leq_db);
    Some(if l > limit_db {
        Judgement::Over
    } else if l > limit_db - margin_db.max(0.0) {
        Judgement::Near
    } else {
        Judgement::Ok
    })
}

/// A window's judgement, with how a filling window stands on its budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Verdict {
    pub judgement: Judgement,
    /// Filling, not over yet, and the Leq so far above the limit: at the pace so far the
    /// full window ends over it (`judgement` is then [`Judgement::Near`]).
    pub on_course: bool,
}

/// Judges window value `v` against `limit_db` with a warn `margin_db`, the window's levels
/// raised by `offset_db` (the sensitivity, dBFS → dB SPL), at the displayed 0.1 dB
/// resolution.
///
/// A full window is judged on its Leq ([`judge`]). Limits are defined on full windows, so a
/// filling one is over only when the full window must end over the limit even if the rest
/// of it is silent ([`WindowValue::least_dbfs`] over the limit: its energy has spent the
/// budget `limit · (measured + remaining)`). Its Leq so far is what it ends at if the rest
/// goes on at the same mean power: above the limit it is on course to go over (near),
/// within the margin near, else ok. Once full, the least level is the Leq and both rules
/// agree. `None` without a value.
pub fn judge_window(
    v: &WindowValue,
    offset_db: f64,
    limit_db: f64,
    margin_db: f64,
) -> Option<Verdict> {
    let j = judge(v.leq_dbfs + offset_db, limit_db, margin_db)?;
    let certain = !v.filling()
        || judge(v.least_dbfs() + offset_db, limit_db, margin_db) == Some(Judgement::Over);
    Some(match j {
        Judgement::Over if !certain => Verdict {
            judgement: Judgement::Near,
            on_course: true,
        },
        j => Verdict {
            judgement: j,
            on_course: false,
        },
    })
}

/// Seconds a peak limit is judged over: its level is the highest LCpeak (LAFmax) of the
/// newest this many seconds. A peak is an instant: judged on its own second it would be
/// over for one second and back the next, too short to be seen from a desk and a flicker
/// with every kick drum near the limit. Held this long, the same dwell as
/// [`RELEASE_HOLD_S`], one peak over keeps the state over long enough to be noticed, and
/// peaks recurring within the hold keep it over without toggling.
pub const PEAK_HOLD_S: usize = 10;

/// The newest [`PEAK_HOLD_S`] seconds' LCpeak and LAFmax. No allocation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeakHold {
    ring: [[f64; 2]; PEAK_HOLD_S],
    head: usize,
    len: usize,
}

impl PeakHold {
    /// Adds the newest second (a gap adds nothing: its maxima are zero).
    pub fn push(&mut self, s: &Second) {
        self.ring[self.head] = [s.c_peak_sq, s.af_max_ms];
        self.head = (self.head + 1) % PEAK_HOLD_S;
        self.len = (self.len + 1).min(PEAK_HOLD_S);
    }

    /// Forgets every second.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The highest LCpeak and LAFmax over the seconds held, dBFS; NaN before any second,
    /// −∞ when none of them had signal.
    pub fn max_dbfs(&self) -> [f64; 2] {
        if self.len == 0 {
            return [f64::NAN; 2];
        }
        let mut m = [0.0f64; 2];
        for k in 0..self.len {
            let i = (self.head + PEAK_HOLD_S - 1 - k) % PEAK_HOLD_S;
            m[0] = m[0].max(self.ring[i][0]);
            m[1] = m[1].max(self.ring[i][1]);
        }
        m.map(power_dbfs)
    }
}

/// A judged state is lowered at once only when its level is this far under the state's
/// boundary (the limit for over, the limit less the margin for near). A full window's Leq
/// moves by about `4.34 · (10^(Δ/10) − 1) / N` dB in a second whose level is Δ dB off the
/// window's: a 6 dB louder second moves a 1 min window 0.22 dB, a 15 min one 0.015 dB. Three
/// display steps (0.3 dB) are more than one loud second can undo for any window of a minute
/// or more, so a window let go this far below does not come straight back.
pub const RELEASE_DB: f64 = 0.3;

/// …or when it has stayed under the boundary this many seconds in a row. A long window
/// drifting across its limit dithers between the two 0.1 dB steps either side for a few
/// seconds (its per-second movement is far below the display step, a few hundredths of a
/// dB for 15 min); 10 s outlasts that, and is short against every window a rule defines
/// (15 min and up: about 1 %), so "recovered" is never late by more than a glance.
pub const RELEASE_HOLD_S: u32 = 10;

fn rank(j: Judgement) -> u8 {
    match j {
        Judgement::Ok => 0,
        Judgement::Near => 1,
        Judgement::Over => 2,
    }
}

/// A window's judgement with hysteresis, one call per second: a state rises at once (an
/// alarm is never late) and is lowered only when the level is [`RELEASE_DB`] under the
/// state's boundary, or has been under it for [`RELEASE_HOLD_S`] seconds in a row. A window
/// hovering at its limit thus reports at most one over / recovered pair per hold, instead of
/// one every time its rounded Leq crosses the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Latch {
    held: Option<Judgement>,
    /// Seconds in a row the judgement has been below the held one.
    under: u32,
}

impl Latch {
    /// Holding `j` (a job that carries on from a state another run reported).
    pub fn holding(j: Option<Judgement>) -> Self {
        Self { held: j, under: 0 }
    }

    /// The state held.
    pub fn held(&self) -> Option<Judgement> {
        self.held
    }

    /// This second's verdict `raw` ([`judge_window`]) on a window whose Leq is `leq_db`
    /// (the unit of `limit_db`), against `limit_db` with warn `margin_db`: the verdict with
    /// the held state. A lowered state not yet released is reported as held (not on course).
    pub fn judge(
        &mut self,
        raw: Option<Verdict>,
        leq_db: f64,
        limit_db: f64,
        margin_db: f64,
    ) -> Option<Verdict> {
        let Some(v) = raw else {
            *self = Self::default();
            return None;
        };
        let held = match self.held {
            Some(h) if rank(v.judgement) < rank(h) => h,
            _ => {
                *self = Self::holding(Some(v.judgement));
                return Some(v);
            }
        };
        let boundary = match held {
            Judgement::Over => limit_db,
            _ => limit_db - margin_db.max(0.0),
        };
        self.under += 1;
        let far = leq_db.is_finite() && round_tenth(leq_db) <= boundary - RELEASE_DB + 1e-9;
        let far = far || leq_db == f64::NEG_INFINITY;
        if far || self.under >= RELEASE_HOLD_S {
            *self = Self::holding(Some(v.judgement));
            return Some(v);
        }
        Some(Verdict {
            judgement: held,
            on_course: false,
        })
    }
}
