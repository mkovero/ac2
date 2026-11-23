//! The cursor's mark on the transfer view, and the curve the cursor jumps along: both the
//! curve peak tracking follows ([`extremes::tracked`]), so the jumps, the marks and
//! the differences read one curve.

use super::*;
use crate::comb::{self, End};
use crate::cursor_jump::{self, Landmark, Step};
use crate::readout::{MarkPoint, MarkReadout, mark_readout};

/// `d` at its column nearest `hz` (returned beside it) and its phase in the pane's mode.
fn point(d: &DisplayTrace, hz: f64, phase: PhaseView) -> Option<(MarkPoint, End)> {
    let i = d.nearest_estimate(hz)?;
    let ph = match phase {
        PhaseView::Wrapped => &d.phase_wrapped_deg,
        PhaseView::Unwrapped { .. } => &d.phase_unwrapped_deg,
        PhaseView::GroupDelay { .. } => &d.group_delay_s,
    };
    let p = MarkPoint {
        hz: d.freqs[i],
        level_db: d.magnitude_db.get(i).copied().unwrap_or(f64::NAN),
        phase: Some(ph.get(i).copied().unwrap_or(f64::NAN)),
    };
    Some((p, End { col: i, hz: p.hz }))
}

/// The cursor read against its mark on the tracked curve, with the comb evidence of its
/// magnitude as the jumps read the view (smoothed as served: both smoothing modes smooth
/// the magnitude); `None` without both, or while the plot's chrome hides the cursor (T).
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
    let (mark, m_end) = point(d, m, view.tf.phase)?;
    let (cursor, c_end) = point(d, c, view.tf.phase)?;
    // Over the estimates, as the jumps read them: a peak the jump lands on in a joined
    // range has the bins beside it as neighbours here too.
    let (freqs, values) = d.own_estimates(&d.magnitude_db);
    let own = |e: End| End {
        col: d.own_index(e.col),
        hz: e.hz,
    };
    let comb = comb::evidence(
        &freqs,
        &values,
        view.freq.range(),
        view.jump_prominence_db,
        d.smoothing.map(|s| s.fraction),
        own(m_end),
        own(c_end),
    );
    Some(mark_readout(
        mark,
        cursor,
        comb,
        view.tf.phase,
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
    // Over the estimates alone: where the line joins bins across empty columns, a room
    // mode's neighbours are the bins beside it, as drawn.
    let (freqs, values) = d.own_estimates(&d.magnitude_db);
    cursor_jump::jump(
        &freqs,
        &values,
        shown,
        view.cursor_hz,
        kind,
        step,
        view.jump_prominence_db,
    )
}
