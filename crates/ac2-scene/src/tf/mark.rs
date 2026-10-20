//! The cursor's mark on the transfer view, and the curve the cursor jumps along: both the
//! curve min / max tracking follows ([`extremes::tracked`]), so the jumps, the marks and
//! the differences read one curve.

use super::*;
use crate::cursor_jump::{self, Landmark, Step};
use crate::grid::nearest_column;
use crate::readout::{MarkPoint, MarkReadout, mark_readout};

/// `d` at its column nearest `hz`, its phase in the pane's mode.
fn point(d: &DisplayTrace, hz: f64, phase: PhaseView) -> Option<MarkPoint> {
    let i = nearest_column(&d.freqs, hz)?;
    let ph = match phase {
        PhaseView::Wrapped => &d.phase_wrapped_deg,
        PhaseView::Unwrapped { .. } => &d.phase_unwrapped_deg,
        PhaseView::GroupDelay { .. } => &d.group_delay_s,
    };
    Some(MarkPoint {
        hz: d.freqs[i],
        level_db: d.magnitude_db.get(i).copied().unwrap_or(f64::NAN),
        phase: Some(ph.get(i).copied().unwrap_or(f64::NAN)),
    })
}

/// The cursor read against its mark on the tracked curve; `None` without both, or while
/// the plot's chrome hides the cursor (T).
pub(super) fn readout(
    traces: &[TfTrace<'_>],
    shown: &[DisplayTrace],
    view: &ViewState,
) -> Option<MarkReadout> {
    let (m, c) = view.cursor_mark_hz.zip(view.cursor_hz)?;
    if !view.chrome.cursor() {
        return None;
    }
    let d = shown.get(extremes::tracked(traces)?)?;
    let name = (shown.len() > 1).then_some(d.name.as_str());
    let ph = view.tf.phase;
    Some(mark_readout(
        point(d, m, ph)?,
        point(d, c, ph)?,
        ph,
        name,
        view.temperature_c,
    ))
}

/// The mark's line, dashed so it never reads as the cursor, in every pane of the view.
pub(super) fn line(c: &mut Canvas, plots: &[Rect], x: f32, theme: &Theme) {
    let stroke = canvas::mark_stroke(theme);
    for p in plots {
        canvas::vline(&mut c.data, *p, x, stroke);
    }
}

/// Where the cursor jumps on the transfer view from where it is (`view.cursor_hz`): the
/// next `kind` of the tracked curve's magnitude as drawn (smoothed as served, offset,
/// coherence-blanked columns left out) inside `shown`, standing out by the view's
/// prominence.
pub fn jump(
    traces: &[TfTrace<'_>],
    cache: &DisplayCache,
    view: &ViewState,
    shown: Range,
    kind: Landmark,
    step: Step,
) -> Option<f64> {
    let (_, drawn) = display_traces(traces, cache, view.tf.phase_reference, &view.tf.coherence);
    let d = drawn.get(extremes::tracked(traces)?)?;
    cursor_jump::jump(
        &d.freqs,
        &d.magnitude_db,
        shown,
        view.cursor_hz,
        kind,
        step,
        view.jump_prominence_db,
    )
}
