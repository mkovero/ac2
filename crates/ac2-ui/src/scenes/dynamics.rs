//! The Dynamics pane: its curves lent to `ac2_scene::dynamics`, which makes every number.

use ac2_scene::dynamics::{DynamicsFrame, DynamicsScene, dynamics_scene, missing_text};
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;

use crate::keys::{CommandId, Keymap};
use crate::state::{AppState, PaneId};

pub fn dynamics(
    st: &AppState,
    pane: PaneId,
    keymap: &Keymap,
    theme: &Theme,
    size: Viewport,
    now: super::Now,
) -> DynamicsScene {
    let status = super::status(st, &[], None, now);
    let modes = st.pane_modes(pane).dynamics;
    let held = st.dynamics_curves(pane);
    let curves = st.dynamics_frame_curves(&held, &modes, theme);
    let missing = missing_text(
        st.pane_meas(pane).map(|m| m.config.name.as_str()),
        &keymap.reach(CommandId::StimulusArm),
        &keymap.reach(CommandId::NewMeasurement),
    );
    dynamics_scene(
        &DynamicsFrame {
            curves: &curves,
            modes,
            pointer: None,
            missing: &missing,
        },
        &status,
        &st.view_for(pane),
        theme,
        size,
    )
}
