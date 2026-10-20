//! The cursor's mark on the spectrum, and the curve the cursor jumps along: the selected
//! curve when the pane draws it, else the first of the pane's own (not compared), else
//! the first — one curve, so the jumps and the differences read the same one.

use super::*;
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

/// `t` where `hz` is: the frequency as put (the spectrum's cursor stays where it was put,
/// [`SpectrumCursor`]) and the level of the column whose band holds it, as drawn.
fn point(t: &SpectrumTrace<'_>, hz: f64) -> Option<MarkPoint> {
    let j = column_at(t.freqs, t.edges, hz)?;
    Some(MarkPoint {
        hz,
        level_db: t.value(j),
        phase: None,
    })
}

/// The cursor read against its mark on the target curve; `None` without both, or while
/// the plot's chrome hides the cursor (T).
pub(super) fn readout(traces: &[SpectrumTrace<'_>], view: &ViewState) -> Option<MarkReadout> {
    let (m, c) = view.cursor_mark_hz.zip(view.cursor_hz)?;
    if !view.chrome.cursor() {
        return None;
    }
    let t = traces.get(target(traces)?)?;
    let name = (traces.len() > 1).then_some(t.name.as_str());
    Some(mark_readout(
        point(t, m)?,
        point(t, c)?,
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
    let v: Vec<f64> = (0..t.freqs.len()).map(|j| t.value(j)).collect();
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
