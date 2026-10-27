//! The alignment view's inputs: which alignment a transfer pane shows, its frame, names
//! and colour. The daemon fitted it; `ac2_scene::align` words and draws it.

use ac2_proto::FrameData;
use ac2_proto::model::{MathExpr, MeasKind, Measurement, TraceOwner};
use ac2_proto::topic::Stream;
use ac2_scene::align::view::{AlignScene, Content, Shown, alignment_scene};
use ac2_scene::grid::columns;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;

use super::{Now, frame, meas_color, shown_math, status};
use crate::state::{AppState, PaneId};

/// Whether `m` is an alignment.
pub fn is_alignment(m: &Measurement) -> bool {
    matches!(&m.config.kind, MeasKind::Math { config } if matches!(config.expr, MathExpr::Align { .. }))
}

/// The alignment pane `pane` shows: its measurement when that is one, else the first one
/// listed in its measurement's group (the tree's order).
pub fn pane_alignment(st: &AppState, pane: PaneId) -> Option<&Measurement> {
    let m = st.pane_meas(pane)?;
    if is_alignment(m) {
        return Some(m);
    }
    let group = match &m.config.kind {
        MeasKind::Math { config } => config.owner,
        _ => TraceOwner::Meas { meas: m.id },
    };
    st.measurements().into_iter().find(|a| {
        is_alignment(a)
            && matches!(&a.config.kind, MeasKind::Math { config } if config.owner == group)
    })
}

/// The alignment view of transfer pane `pane`.
pub fn alignment(
    st: &AppState,
    pane: PaneId,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> AlignScene {
    let view = st.view_for(pane);
    let Some(m) = pane_alignment(st, pane) else {
        let why = match st.pane_meas(pane) {
            Some(m) => format!(
                "No alignment in {}'s group — Alt+M aligns two transfer curves",
                m.config.name
            ),
            None => "No alignment — Alt+M aligns two transfer curves".into(),
        };
        return alignment_scene(
            &Content::Empty(&why),
            &status(st, &[], None, now),
            &view,
            theme,
            size,
        );
    };
    let MeasKind::Math { config } = &m.config.kind else {
        unreachable!("an alignment is a math channel")
    };
    let MathExpr::Align { a, b, band, .. } = config.expr else {
        unreachable!("an alignment's expression")
    };
    let tf = frame(st, m.id, Stream::Tf);
    let mut status = status(st, tf.as_slice(), Some(m), now);
    if let Some(f) = tf {
        status.math = shown_math(st, Some(m), &[(m.id, f)]);
    }
    let grid = tf.and_then(|f| {
        let g = f.frame.stamp.grid_id?;
        st.rig.data.as_ref()?.grids.get(&g).map(|g| columns(g))
    });
    let shown = tf.zip(grid).and_then(|(f, cols)| {
        let FrameData::Tf(t) = &f.frame.data else {
            return None;
        };
        let state = t.meta.math.as_ref()?.align.as_deref()?;
        Some((t.align.as_ref()?, state, cols))
    });
    let (na, nb) = (st.operand_name(a), st.operand_name(b));
    match shown {
        Some((curves, state, cols)) => alignment_scene(
            &Content::Shown(Shown {
                a: &na,
                b: &nb,
                band,
                state,
                curves,
                freqs: &cols.freqs,
                color: meas_color(st, theme, m.id),
            }),
            &status,
            &view,
            theme,
            size,
        ),
        None => {
            let why = format!("{} — waiting for its first frame", m.config.name);
            alignment_scene(&Content::Empty(&why), &status, &view, theme, size)
        }
    }
}
