//! The plates over a pane's plot (the legend, the cursor values, the mark's readout):
//! dragged, scaled by their grip and put back by a double-click. Where they sit is the
//! pane's, stored with its other display choices ([`View::plates`]), so it travels with
//! a measurement's own panes and comes back with `ui.toml`.

use ac2_proto::model::layout as wire;
use ac2_scene::legend::LegendCorner;
use ac2_scene::plate::{PlateKind, PlatePlace, Plates};

use crate::state::*;

impl AppState {
    pub(in crate::state) fn plate_msg(&mut self, m: PlateMsg) {
        match m {
            PlateMsg::Hover(h) => self.plate_hover = h,
            PlateMsg::Move { pane, kind, x, y } => {
                if let Some(p) = self.plate_mut(pane, kind) {
                    p.move_to(x, y);
                }
            }
            PlateMsg::Scale { pane, kind, scale } => {
                if let Some(p) = self.plate_mut(pane, kind) {
                    p.set_scale(scale);
                }
            }
            PlateMsg::Reset { pane, kind } => {
                if let Some(p) = self.plate_mut(pane, kind) {
                    *p = PlatePlace::default();
                }
            }
            // The drag was closed before the message, as any other message closes it.
            PlateMsg::End => {}
        }
    }

    fn plate_mut(&mut self, pane: PaneId, kind: PlateKind) -> Option<&mut PlatePlace> {
        self.layout
            .views
            .get_mut(&pane)
            .map(|v| v.plates.get_mut(kind))
    }

    /// The legend snapped to corner `c` (the palette) and shown: on the focused pane's
    /// column, whose top pane carries the legend for a transfer stack.
    pub(in crate::state) fn snap_legend(&mut self, c: LegendCorner) {
        let mut id = self.layout.focus;
        // Each step goes one pane up the tree: at most as many as there are panes.
        for _ in 0..self.layout.views.len() {
            match self
                .layout
                .root
                .above(id)
                .filter(|_| self.stacked_under(id))
            {
                Some(up) => id = up,
                None => break,
            }
        }
        if let Some(p) = self.plate_mut(id, PlateKind::Legend) {
            p.at = Some(c.position());
        }
        self.view.tf.legend.hidden = false;
    }
}

/// The plates as a layout carries them.
pub(in crate::state) fn wire_plates(p: &Plates) -> wire::LayoutPlates {
    let one = |p: &PlatePlace| wire::LayoutPlate {
        at: p.at.map(|[x, y]| [f64::from(x), f64::from(y)]),
        scale: f64::from(p.scale()),
    };
    wire::LayoutPlates {
        legend: one(&p.legend),
        readout: one(&p.readout),
        mark: one(&p.mark),
    }
}

/// The plates a layout carries, kept inside the plot and within the scale's bounds.
pub(in crate::state) fn ui_plates(w: &wire::LayoutPlates) -> Plates {
    let one = |w: &wire::LayoutPlate| {
        let mut p = PlatePlace::default();
        if let Some([x, y]) = w.at {
            p.move_to(x as f32, y as f32);
        }
        p.set_scale(w.scale as f32);
        p
    };
    Plates {
        legend: one(&w.legend),
        readout: one(&w.readout),
        mark: one(&w.mark),
    }
}
