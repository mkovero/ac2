//! The Programme pane: its bars and chip from `ac2_scene::programme`, the embers of its
//! pair's ring placed by `ac2_plot::embers` in the goniometer's square.

use ac2_scene::primitives::{Scene, Viewport};
use ac2_scene::programme::{missing_text, programme_scene};
use ac2_scene::theme::Theme;
use ac2_scene::view::Style;

use crate::state::{AppState, PaneId};

/// The scene of Programme pane `pane` at `size`; `None` when the pane is not one.
pub fn programme(st: &AppState, pane: PaneId, theme: &Theme, size: Viewport) -> Option<Scene> {
    let (mut p, missing) = st.programme_pane(pane)?;
    let metered = st.layout.view(pane)?.meters?;
    let view = st.view_for(pane);
    p.plate = view.plates.loudness;
    p.plate_hover = view.plate_hover;
    let style = Style {
        chrome: view.chrome,
        theme,
    };
    let missing = (!missing.is_empty()).then(|| missing_text(&missing));
    let mut s = programme_scene(&p, style, size, missing.as_deref());
    if let (Some(square), Some(pair)) = (s.gonio, metered.pair())
        && let Some(ring) = st.programme.rings.get(&pair)
    {
        let points = ac2_plot::embers::ember_points(ring, st.now_s, st.ember_decay_s(), square);
        if let Some(base) = s.scene.layers.first_mut() {
            base.points.push(points);
        }
    }
    Some(s.scene)
}
