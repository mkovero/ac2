//! The cursor's mark on the spectrum, and the curve the cursor jumps along: the selected
//! curve when the pane draws it, else the first of the pane's own (not compared), else
//! the first — one curve, so the jumps and the differences read the same one.

use super::*;
use crate::comb::{self, End};
use crate::cursor_jump::{self, Landmark, Step};
use crate::readout::{MarkPoint, MarkReadout, mark_readout};
use crate::view::PhaseView;

fn target(traces: &[SpectrumTrace<'_>]) -> Option<usize> {
    traces
        .iter()
        .position(|t| t.selected)
        .or_else(|| traces.iter().position(|t| !t.compared))
        .or_else(|| (!traces.is_empty()).then_some(0))
}

/// `t`'s levels as drawn, one per column.
fn values(t: &SpectrumTrace<'_>) -> Vec<f64> {
    (0..t.freqs.len()).map(|j| t.value(j)).collect()
}

/// `t` where `hz` is: the frequency as put (the spectrum's cursor stays where it was put,
/// [`SpectrumCursor`]) and the level of the column whose band holds it (returned beside
/// it), as drawn.
fn point(t: &SpectrumTrace<'_>, hz: f64) -> Option<(MarkPoint, End)> {
    let j = column_at(t.freqs, t.edges, hz)?;
    let p = MarkPoint {
        hz,
        level_db: t.value(j),
        phase: None,
    };
    Some((p, End { col: j, hz }))
}

/// The smoothing a narrowband trace is drawn under; RTA bands are not a smoothing.
fn smoothing(q: Quantity) -> Option<SmoothingFraction> {
    match q {
        Quantity::SmoothedTone(f) | Quantity::SmoothedDensity(f) => Some(f),
        Quantity::Tone | Quantity::Density | Quantity::Band => None,
    }
}

/// The cursor read against its mark on the target curve, with the comb evidence of its
/// levels as the jumps read the view; `None` without both, or while the plot's chrome
/// hides the cursor (T).
pub(super) fn readout(traces: &[SpectrumTrace<'_>], view: &ViewState) -> Option<MarkReadout> {
    let (m, c) = view.cursor_mark_hz.zip(view.cursor_hz)?;
    if !view.chrome.cursor() {
        return None;
    }
    let t = traces.get(target(traces)?)?;
    let name = (traces.len() > 1).then_some(t.name.as_str());
    let v = values(t);
    let (mark, m_end) = point(t, m)?;
    let (cursor, c_end) = point(t, c)?;
    let comb = comb::evidence(
        t.freqs,
        &v,
        view.freq.range(),
        view.jump_prominence_db,
        smoothing(t.quantity),
        m_end,
        c_end,
    );
    Some(mark_readout(
        mark,
        cursor,
        comb,
        PhaseView::Wrapped,
        name,
        view.temperature_c,
    ))
}

/// Where the cursor jumps on the spectrum from where it is (`view.cursor_hz`): the next
/// `kind` of the target curve's levels as drawn (smoothed as served, offset, invalid
/// columns left out) inside `shown`, standing out by the view's prominence.
pub fn jump(
    traces: &[SpectrumTrace<'_>],
    view: &ViewState,
    shown: axis::Range,
    kind: Landmark,
    step: Step,
) -> Option<f64> {
    let t = traces.get(target(traces)?)?;
    let v = values(t);
    cursor_jump::jump(
        t.freqs,
        &v,
        shown,
        view.cursor_hz,
        kind,
        step,
        view.jump_prominence_db,
    )
}
