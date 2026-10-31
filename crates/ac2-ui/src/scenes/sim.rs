//! Room simulation results over the sweep pane: the reflections a simulation predicts at the
//! point a trace was captured, over its impulse response, and the simulated room table under
//! the measured one. The simulation is picked here; what it says comes from
//! `ac2_scene::room::sim` and `sim_marks`.

use ac2_proto::model::{SimState, SimStatus, TraceMeta, WalkState};
use ac2_scene::ir::IrScene;
use ac2_scene::room::RoomTable;
use ac2_scene::room::sim::{receiver_table, room_table};
use ac2_scene::room::sim_marks::{draw_marks, reflection_marks};
use ac2_scene::theme::Theme;

use crate::state::AppState;

/// The finished simulation run (not a map) of `plan`, if the daemon has one.
fn finished<'a>(st: &'a AppState, plan: &str) -> Option<&'a SimState> {
    st.daemon()?
        .sim
        .as_ref()
        .filter(|s| s.status == SimStatus::Done && s.plan == plan && s.map.is_none())
}

/// Draws the simulated reflections at the point trace `t` was captured over its IR scene `sc`.
pub fn reflections(st: &AppState, t: &TraceMeta, sc: &mut IrScene, theme: &Theme) {
    let Some(pos) = t.position.as_ref() else {
        return;
    };
    let Some(s) = finished(st, &pos.plan) else {
        return;
    };
    let plan = st
        .daemon()
        .and_then(|d| d.walk_plans.iter().find(|p| p.name == pos.plan));
    if let Some(marks) = reflection_marks(s, plan, pos) {
        draw_marks(sc, marks, theme);
    }
}

/// The simulated table for the Room view: a simulation at walk `w`'s points (its first
/// source), else one heard at the point trace `t` was captured.
pub fn room(st: &AppState, w: Option<&WalkState>, t: Option<&TraceMeta>) -> Option<RoomTable> {
    if let Some(w) = w {
        let s = finished(st, &w.plan).filter(|s| s.walk == Some(w.id))?;
        return room_table(s, 0);
    }
    let pos = t?.position.as_ref()?;
    let s = finished(st, &pos.plan).filter(|s| s.walk == Some(pos.walk))?;
    receiver_table(s, &pos.label)
}
