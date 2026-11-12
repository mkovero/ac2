//! What a pane holds in this app alone: pointer and camera state that is never saved, kept apart from the layout, which is stored and rebuilt.

use std::collections::HashMap;

use crate::state::PaneId;
use crate::state::cam3d::CamNav;

/// The panes' app-local state, by pane.
#[derive(Debug, Default)]
pub struct PaneLocal {
    /// The 3D spectrograph's camera while it eases or is dragged.
    pub(in crate::state) cam3d: Option<CamNav>,
    /// When each 3D spectrograph's axes show: hover and the last interaction.
    pub(in crate::state) axes3d: HashMap<PaneId, ac2_scene::spectrograph_3d::AxesReveal>,
    /// The plate under the pointer, and whose pane it is on.
    pub plate_hover: Option<(PaneId, ac2_scene::plate::PlateHover)>,
    /// The pointer over a 3D spectrograph's picture, and whose pane it is on.
    pub pointer3d: Option<(PaneId, ac2_scene::spectrograph_3d::Pointer3d)>,
}
