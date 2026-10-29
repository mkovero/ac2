//! The walk pane: its lines and plan from `ac2_scene::walk`, fed the walk, the session's copy
//! of its plan, the measurement's readiness (its bar gliding as the meters' bars do), the
//! average and what differs between the captures.

use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;
use ac2_scene::view::Style;
use ac2_scene::walk::{WalkPane, WalkScene, conditions_text, walk_scene};

use super::{Now, daemon_wall};
use crate::state::{AppState, PaneId};

/// The scene of walk pane `pane` at `size`; `None` when the pane shows no walk of the
/// session.
pub fn walk(
    st: &AppState,
    pane: PaneId,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<WalkScene> {
    let w = st.layout.view(pane)?.walk.and_then(|id| st.walk(id))?;
    let average = w
        .average
        .and_then(|m| st.meas(m))
        .map(|m| m.config.name.as_str());
    let view = st.view_for(pane);
    let p = WalkPane {
        walk: w,
        plan: st.walk_plan(w),
        ready: st.walk_ready(w),
        average,
        unsettled: st.unsettled_traces(&w.captured_traces()),
        bar_drawn: w
            .meas
            .first()
            .and_then(|&m| st.walk_bar_drawn(m, now.instant)),
        // The captures are stamped on the daemon's clock: today is read on it too.
        conditions: conditions_text(w, daemon_wall(st, now.wall), |t| st.local_zone.offset_s(t)),
    };
    let style = Style {
        chrome: view.chrome,
        theme,
    };
    Some(walk_scene(&p, style, size))
}
