//! Panes follow the selection (a Display setting, off by default): with it on, the layout
//! keeps only the panes that draw the selected measurement — its live curve, its math
//! channels or its shown stored traces — or, for a selected stored trace, the panes that
//! draw it and its measurement. Nothing selected, or no pane on screen drawing it, keeps
//! every pane: the screen is never empty. W and Alt+digit act on the panes it keeps.

use ac2_proto::model::TraceOwner;

use super::text::drawn_in;
use super::*;

/// The panes that draw any of the live measurement kinds `kinds` or the stored traces
/// `traces`, in pane order. A pane draws a live measurement when its source selector can
/// pick it ([`PaneKind::shows`]), a stored trace when its curve is drawn there.
pub fn panes_drawing(kinds: &[&MeasKind], traces: &[&TraceMeta]) -> Vec<PaneKind> {
    PaneKind::ALL
        .into_iter()
        .filter(|p| {
            // A sweep's home is the sweep pane: the transfer pane follows it only for the runs
            // it draws.
            kinds.iter().any(|k| {
                p.shows(k) && !(*p == PaneKind::Transfer && matches!(k, MeasKind::Sweep { .. }))
            }) || traces.iter().any(|t| drawn_in(t, *p))
        })
        .collect()
}

impl AppState {
    /// The panes the selection asks for while panes follow it; `None` with the setting off
    /// or nothing selected.
    pub fn follow_set(&self) -> Option<Vec<PaneKind>> {
        if !self.prefs.panes_follow {
            return None;
        }
        if let Some(t) = self.selected_trace_meta() {
            let mut panes = panes_drawing(&[], &[t]);
            if let Some(m) = t.edit.owner.meas().and_then(|id| self.meas(id)) {
                panes.extend(self.panes_of_meas(m));
            }
            panes.sort();
            panes.dedup();
            return Some(panes);
        }
        self.selected_meas().map(|m| self.panes_of_meas(m))
    }

    /// The panes drawing measurement `m`: its live curve, the math channels listed under
    /// it and its shown stored traces (a sweep's runs).
    fn panes_of_meas(&self, m: &Measurement) -> Vec<PaneKind> {
        let group = TraceOwner::Meas { meas: m.id };
        let mut kinds = vec![&m.config.kind];
        kinds.extend(
            self.measurements()
                .into_iter()
                .filter(|c| matches!(&c.config.kind, MeasKind::Math { config } if config.owner == group))
                .map(|c| &c.config.kind),
        );
        let traces: Vec<&TraceMeta> = self
            .daemon()
            .map(|s| {
                s.traces
                    .iter()
                    .filter(|t| t.edit.owner == group && t.edit.visible)
                    .collect()
            })
            .unwrap_or_default();
        panes_drawing(&kinds, &traces)
    }

    /// The tree laid out (before W keeps only the focused pane): every pane, and with
    /// panes following the selection only those that draw it, each pane left out giving its
    /// place to its neighbour.
    pub fn laid_out_tree(&self) -> PaneNode {
        let Some(f) = self.follow_set() else {
            return self.layout.root.clone();
        };
        let keep = |id: PaneId| f.contains(&self.layout.kind(id));
        self.layout
            .root
            .pruned(&keep)
            .unwrap_or_else(|| self.layout.root.clone())
    }

    /// The panes laid out, in reading order.
    pub fn laid_out_panes(&self) -> Vec<PaneId> {
        self.laid_out_tree().reading_order()
    }

    /// The tree drawn now: the laid-out one, or the focused pane alone when maximised.
    pub fn visible_tree(&self) -> Option<PaneNode> {
        let laid = self.laid_out_tree();
        if !self.layout.maximized {
            return Some(laid);
        }
        if laid.contains(self.layout.focus) {
            return Some(PaneNode::Leaf(self.layout.focus));
        }
        laid.reading_order().first().map(|id| PaneNode::Leaf(*id))
    }

    /// Panes drawn now, in reading order.
    pub fn visible_panes(&self) -> Vec<PaneId> {
        self.visible_tree()
            .map(|t| t.reading_order())
            .unwrap_or_default()
    }

    /// Keeps the keyboard on a pane that is drawn: a selection that hid the focused pane
    /// moves the focus to the first pane kept.
    pub(super) fn fit_focus(&mut self) {
        if self.follow_set().is_none() {
            return;
        }
        let laid = self.laid_out_panes();
        if !laid.contains(&self.layout.focus)
            && let Some(p) = laid.first()
        {
            self.layout.set_focus(*p);
        }
    }

    /// After a selection from the list: with panes following it and none of its panes on
    /// screen (each put away), every pane stays, and the toast says why.
    pub(super) fn follow_selection_toast(&mut self) {
        let Some(f) = self.follow_set() else {
            return;
        };
        if self.layout.views.values().any(|v| f.contains(&v.kind)) {
            return;
        }
        let name = match self.selected_trace_meta() {
            Some(t) => trace_label(t),
            None => match self.selected_meas() {
                Some(m) => m.config.name.clone(),
                None => return,
            },
        };
        self.toast(format!(
            "{name}: no pane on screen draws it · every pane stays (panes follow selection)"
        ));
    }

    /// The Display setting and its palette command.
    pub(super) fn toggle_panes_follow(&mut self) {
        self.prefs.panes_follow = !self.prefs.panes_follow;
        self.prefs_dirty = true;
        self.fit_focus();
        self.toast(if self.prefs.panes_follow {
            "panes follow selection: only the panes that draw the selected measurement"
        } else {
            "panes follow selection off: every pane"
        });
    }
}
