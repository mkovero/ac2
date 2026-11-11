//! What a pane holds in this app alone: pointer and camera state and picks that are never
//! saved, kept apart from the layout, which is stored and rebuilt.

use std::collections::HashMap;

use crate::state::cam3d::CamNav;
use crate::state::{PaneId, PeakPick};

/// The panes' app-local state, by pane.
#[derive(Debug, Default)]
pub struct PaneLocal {
    /// The 3D spectrograph's camera while it eases or is dragged.
    pub(in crate::state) cam3d: Option<CamNav>,
    /// When each 3D spectrograph's axes show: hover and the last interaction.
    pub(in crate::state) axes3d: HashMap<PaneId, ac2_scene::spectrograph_3d::AxesReveal>,
    /// Each pane's peak points pick ([`crate::state::AppState::peak_points_of`]): app-local
    /// and unsaved, kept apart from the layout, which is stored and rebuilt.
    pub(in crate::state) peak_points: HashMap<PaneId, PeakPick>,
    /// The plate under the pointer, and whose pane it is on.
    pub plate_hover: Option<(PaneId, ac2_scene::plate::PlateHover)>,
}
