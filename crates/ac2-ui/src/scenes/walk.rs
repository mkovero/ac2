//! The walk pane: its lines and plan from `ac2_scene::walk`, fed the walk, the session's copy
//! of its plan, the measurement's readiness (its bar gliding as the meters' bars do), the
//! average and what differs between the captures; a sweep walk's run instead of readiness.

use ac2_proto::model::SimStatus;
use ac2_scene::primitives::Viewport;
use ac2_scene::room::sim::status_line;
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
    let mics = st.walk_mic_names(w);
    let (ready, slowest) = st.walk_ready(w);
    let p = WalkPane {
        walk: w,
        plan: st.walk_plan(w),
        ready,
        mics: &mics,
        slowest,
        average,
        unsettled: st.unsettled_traces(&w.captured_traces()),
        // The bar is the slowest mic's, gliding as its own measurement fills.
        bar_drawn: slowest
            .and_then(|k| w.mics.get(k))
            .and_then(|m| st.walk_bar_drawn(m.meas, now.instant)),
        // The captures are stamped on the daemon's clock: today is read on it too.
        conditions: conditions_text(w, daemon_wall(st, now.wall), |t| st.local_zone.offset_s(t)),
        sweep: st.walk_sweep_at(w),
        sim: st.daemon().and_then(|s| {
            status_line(
                &[s.sim.as_ref(), s.sim_map.as_ref()],
                s.sim_fit.as_ref(),
                &w.plan,
            )
        }),
        // Simulated, so drawn only while the simulation hints are (T, Settings).
        coverage: st.daemon().and_then(|s| s.sim_map.as_ref()).filter(|m| {
            view.sim_shown() && m.plan == w.plan && m.status == SimStatus::Done && m.map.is_some()
        }),
    };
    let style = Style {
        chrome: view.chrome,
        theme,
    };
    Some(walk_scene(&p, style, size))
}
