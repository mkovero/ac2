//! The cursor's mark on the sweep's response, and the curve the cursor jumps along: the
//! fundamental of the pane's own run, the one curve of the view that has peaks and notches
//! of a response (an order's level is read where it is valid, in runs between its floors).

use super::*;
use crate::comb::{self, End};
use crate::cursor_jump::{self, Landmark, Step};
use crate::readout::{MarkPoint, MarkReadout, mark_readout};
use crate::view::PhaseView;

/// The fundamental's magnitude, one value per column.
fn values(t: &SweepView<'_>) -> Vec<f64> {
    (0..t.freqs.len())
        .map(|i| t.data.mag_db.get(i).map_or(f64::NAN, |v| f64::from(*v)))
        .collect()
}

/// The fundamental at its column nearest `hz` (returned beside it), where the cursor
/// reads it.
fn point(t: &SweepView<'_>, values: &[f64], hz: f64) -> Option<(MarkPoint, End)> {
    let i = nearest_column(t.freqs, hz)?;
    let p = MarkPoint {
        hz: t.freqs[i],
        level_db: values[i],
        phase: None,
    };
    Some((p, End { col: i, hz: p.hz }))
}

/// The cursor read against its mark on the fundamental, named when other runs are drawn
/// beside it (`named`), with the comb evidence of its magnitude as the jumps read the view;
/// `None` without both, or while the plot's chrome hides the cursor (T).
pub(super) fn readout(t: &SweepView<'_>, named: bool, view: &ViewState) -> Option<MarkReadout> {
    let (m, c) = view.cursor_mark_hz.zip(view.cursor_hz)?;
    if !view.chrome.cursor() {
        return None;
    }
    let v = values(t);
    let (mark, m_end) = point(t, &v, m)?;
    let (cursor, c_end) = point(t, &v, c)?;
    // The response is on the sweep's own grid, not smoothed after it.
    let comb = comb::evidence(
        t.freqs,
        &v,
        view.freq.range(),
        view.jump_prominence_db,
        None,
        m_end,
        c_end,
    );
    Some(mark_readout(
        mark,
        cursor,
        comb,
        PhaseView::Wrapped,
        named.then_some(t.data.meta.edit.name.as_str()),
        view.temperature_c,
    ))
}

/// Where the cursor jumps on the sweep's response from where it is (`view.cursor_hz`): the
/// next `kind` of the fundamental's magnitude inside `shown`, standing out by the view's
/// prominence.
pub fn jump(
    t: &SweepView<'_>,
    view: &ViewState,
    shown: Range,
    kind: Landmark,
    step: Step,
) -> Option<f64> {
    cursor_jump::jump(
        t.freqs,
        &values(t),
        shown,
        view.cursor_hz,
        kind,
        step,
        view.jump_prominence_db,
    )
}
