//! Cursor jumps: the next or previous peak or notch of a curve as its pane draws it, inside
//! the shown frequency range. Display logic on the drawn columns, as the peak marks are
//! ([`crate::spectrum::peaks`]): nothing is found that the curve does not show.

use crate::axis::Range;
use crate::format;
use crate::spectrum::peaks::prominence;

/// How far a peak (or notch) must stand out of the curve around it to be jumped to, dB,
/// unless the operator chose otherwise: a 3 dB bump is the smallest a tuning decision is
/// made on (half or double the power), and the ripple of a smoothed transfer curve stays
/// under it.
pub const DEFAULT_PROMINENCE_DB: f64 = 3.0;

/// The prominences ←/→ step through in Settings › Display, dB: finer for a smoothed curve's
/// gentle features, coarser to skip a raw narrowband spectrum's ripple.
pub const PROMINENCE_STEPS_DB: [f64; 6] = [1.0, 2.0, 3.0, 6.0, 10.0, 20.0];

/// The prominence one ←/→ (`d` = −1 / +1) from `db`, held at the ends; a value between the
/// steps moves to the nearest step in that direction.
pub fn step_prominence(db: f64, d: i32) -> f64 {
    let steps = PROMINENCE_STEPS_DB;
    let next = match d.signum() {
        1 => steps.iter().copied().find(|s| *s > db + 1e-9),
        -1 => steps.iter().rev().copied().find(|s| *s < db - 1e-9),
        _ => None,
    };
    next.unwrap_or(if d > 0 {
        steps[steps.len() - 1]
    } else if d < 0 {
        steps[0]
    } else {
        db
    })
    .clamp(steps[0], steps[steps.len() - 1])
}

/// What the setting says: `stands out by 3 dB`.
pub fn prominence_text(db: f64) -> String {
    format!("stands out by {} dB", format::fixed(db, 0))
}

/// A peak or a notch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landmark {
    Peak,
    Notch,
}

impl Landmark {
    /// `v` as a height: how far it stands out this way.
    fn height(self, v: f64) -> f64 {
        match self {
            Self::Peak => v,
            Self::Notch => -v,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Peak => "peak",
            Self::Notch => "notch",
        }
    }
}

/// Which way along the frequency axis a jump goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// To a higher frequency.
    Up,
    /// To a lower frequency.
    Down,
}

/// The frequencies of every `kind` of `values` (dB, column-aligned with ascending `freqs`;
/// NaN where the curve is not drawn) inside `shown` that stands out by `prominence_db`,
/// ascending. Only the shown columns count, the prominence too: a jump goes to what the
/// pane shows standing out, not to a bump that is a peak only of a curve off screen. A
/// column at the edge of the range or of a gap has nothing beside it on that side, so it
/// is never a peak: whether the curve turns there is not shown.
pub fn landmarks(
    freqs: &[f64],
    values: &[f64],
    shown: Range,
    kind: Landmark,
    prominence_db: f64,
) -> Vec<f64> {
    let n = freqs.len().min(values.len());
    let v: Vec<f64> = (0..n)
        .map(|i| {
            let f = freqs[i];
            let x = values[i];
            if f >= shown.lo && f <= shown.hi && x.is_finite() {
                kind.height(x)
            } else {
                f64::NAN
            }
        })
        .collect();
    (0..n)
        .filter(|&i| {
            let b = v[i];
            let (Some(&l), Some(&r)) = (i.checked_sub(1).and_then(|j| v.get(j)), v.get(i + 1))
            else {
                return false;
            };
            // Strictly above the left, not below the right: a flat top counts once, at its
            // first column.
            b.is_finite()
                && l.is_finite()
                && r.is_finite()
                && b > l
                && b >= r
                && prominence(&v, i) >= prominence_db
        })
        .map(|i| freqs[i])
        .collect()
}

/// Where the cursor at `from_hz` jumps to: the nearest `kind` above it (`Step::Up`) or
/// below it ([`landmarks`]). Without a cursor, the lowest (up) or highest (down) one shown.
/// `None` when there is none that way.
pub fn jump(
    freqs: &[f64],
    values: &[f64],
    shown: Range,
    from_hz: Option<f64>,
    kind: Landmark,
    step: Step,
    prominence_db: f64,
) -> Option<f64> {
    let found = landmarks(freqs, values, shown, kind, prominence_db);
    // The cursor snaps to a column, so a landmark it sits on reads the same frequency: a
    // hair of tolerance keeps the next press from finding the one it is on.
    let eps = 1e-9;
    match (step, from_hz) {
        (Step::Up, Some(f)) => found.into_iter().find(|x| *x > f * (1.0 + eps)),
        (Step::Down, Some(f)) => found.into_iter().rev().find(|x| *x < f * (1.0 - eps)),
        (Step::Up, None) => found.first().copied(),
        (Step::Down, None) => found.last().copied(),
    }
}

/// Why a jump does nothing with no frequency pane to read.
pub const NO_PANE: &str = "the cursor jumps along a transfer magnitude or a spectrum pane";

/// The toast when a jump finds nothing: `no higher peak in view stands out by 3 dB`.
pub fn none_text(kind: Landmark, step: Step, prominence_db: f64) -> String {
    let way = match step {
        Step::Up => "higher",
        Step::Down => "lower",
    };
    format!(
        "no {way} {} in view {}",
        kind.name(),
        prominence_text(prominence_db)
    )
}

#[cfg(test)]
mod tests;
