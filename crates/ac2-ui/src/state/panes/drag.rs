//! A row of the measurement list dragged onto the panes. Dropped on a pane, the pane shows
//! it as a pick from its chip (or a child row) shows it on the focused pane; dropped near
//! one of its sides, the pane splits there first, as N would, and the new half shows it.
//! With Shift held, the item is drawn compared over the pane, which keeps what it shows.
//!
//! Dropped on another row of the list instead, it opens the math dialog combining the two
//! (`crate::state::drop_math`).
//!
//! A drop is a guest pick: it never brings the dropped measurement's own panes, and an edit
//! of the panes afterwards still goes to the measurement whose row was chosen
//! ([`crate::state::panes::meas_layout`]).

use ac2_proto::model::Compared;
use ac2_scene::meas_list::TreeKey;
use ac2_scene::pane_edit::{self, CompareRefusal, DropSpot, Edge};

use crate::state::*;

/// How far in from each side of a pane (a share of its width or height) a drop splits it
/// on that side: wide enough to hit at a glance, small enough that the middle of the pane
/// (a plain drop) stays the larger target.
pub const EDGE_SHARE: f32 = 0.22;

/// Where the pointer is over the panes: a pane, and the side it is near.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropTarget {
    pub pane: PaneId,
    /// Within [`EDGE_SHARE`] of that side.
    pub edge: Option<Edge>,
}

/// A list row being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowDrag {
    pub item: TreeKey,
    /// The pane under the pointer, if any.
    pub over: Option<DropTarget>,
    /// The list row under the pointer, if any: a drop there combines the two.
    pub onto: Option<TreeKey>,
    /// Shift is held: the drop compares.
    pub compare: bool,
    /// Esc cancelled it: nothing happens on release.
    pub cancelled: bool,
}

/// The mouse dragging a list row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragMsg {
    /// The pointer moved far enough with the button held on the row of `item`.
    Start(TreeKey),
    /// The pointer over the panes (`None`: elsewhere), Shift held or not.
    Over {
        target: Option<DropTarget>,
        compare: bool,
    },
    /// The pointer over a row of the list (`None`: off the list's rows).
    OverRow(Option<TreeKey>),
    /// The button released: dropped where the pointer last was over, comparing with Shift.
    Drop { compare: bool },
}

/// The pane and side under `(x, y)` among `rects` (the panes drawn now).
pub fn drop_target(rects: &[(PaneId, PaneRect)], x: f32, y: f32) -> Option<DropTarget> {
    let (pane, r) = rects
        .iter()
        .find(|(_, r)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)?;
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let fx = (x - r.x) / r.w;
    let fy = (y - r.y) / r.h;
    let edge = [
        (fy, Edge::Top),
        (1.0 - fy, Edge::Bottom),
        (fx, Edge::Left),
        (1.0 - fx, Edge::Right),
    ]
    .into_iter()
    .filter(|(d, _)| *d < EDGE_SHARE)
    .min_by(|a, b| a.0.total_cmp(&b.0))
    .map(|(_, e)| e);
    Some(DropTarget { pane: *pane, edge })
}

/// The split an edge drop makes: its axis, and the side the new half takes.
fn edge_split(e: Edge) -> (Axis, Side) {
    match e {
        Edge::Top => (Axis::Column, Side::A),
        Edge::Bottom => (Axis::Column, Side::B),
        Edge::Left => (Axis::Row, Side::A),
        Edge::Right => (Axis::Row, Side::B),
    }
}

impl AppState {
    /// Pane `p`'s number in reading order (from 1), as its title shows it.
    pub(in crate::state) fn pane_number(&self, p: PaneId) -> usize {
        self.layout
            .panes()
            .iter()
            .position(|x| *x == p)
            .map_or(0, |i| i + 1)
    }

    /// What a list row is called in the drag label and toasts: a measurement's or math
    /// channel's name, a stored trace's label.
    pub(in crate::state) fn row_name(&self, item: TreeKey) -> Option<String> {
        match item {
            TreeKey::Meas(id) | TreeKey::Live(id) | TreeKey::Math(id) => {
                self.meas(id).map(|m| m.config.name.clone())
            }
            TreeKey::Trace(id) => self.trace_meta(id).ok().map(|t| trace_label(&t)),
            TreeKey::Imported | TreeKey::Rig | TreeKey::Session => None,
        }
    }

    /// Where a drop on `target` lands: an edge drop on a pane too small to split (as N
    /// refuses) is a plain drop on it.
    pub fn drop_spot(&self, target: DropTarget, compare: bool) -> Option<DropSpot> {
        self.layout.view(target.pane)?;
        let n = self.pane_number(target.pane);
        if compare {
            return Some(DropSpot::Compare(n));
        }
        Some(match target.edge {
            Some(e) if self.can_split(target.pane, edge_split(e).0) => DropSpot::NewPane(e),
            _ => DropSpot::Pane(n),
        })
    }

    /// The row being dragged, unless Esc cancelled it.
    pub fn row_drag(&self) -> Option<&RowDrag> {
        self.row_drag.as_ref().filter(|d| !d.cancelled)
    }

    /// A row is held dragged (cancelled or not): no other drag starts before release.
    pub fn row_drag_held(&self) -> bool {
        self.row_drag.is_some()
    }

    /// The label beside the pointer: `FOH SPL → pane 2`, over a row the math channel the
    /// drop opens (or why the two do not combine); `None` elsewhere.
    pub fn drag_label(&self) -> Option<String> {
        let d = self.row_drag()?;
        if let Some(onto) = d.onto {
            return Some(match self.drop_math_plan(d.item, onto) {
                Ok(p) => p.label,
                Err(why) => why,
            });
        }
        let spot = self.drop_spot(d.over?, d.compare)?;
        Some(pane_edit::drop_label(&self.row_name(d.item)?, spot))
    }

    /// What the drop would take, for its highlight: the pane, and the side that becomes
    /// the new pane on an edge drop.
    pub fn drop_highlight(&self) -> Option<(PaneId, Option<Edge>)> {
        let d = self.row_drag()?;
        let t = d.over?;
        match self.drop_spot(t, d.compare)? {
            DropSpot::NewPane(e) => Some((t.pane, Some(e))),
            DropSpot::Pane(_) | DropSpot::Compare(_) => Some((t.pane, None)),
        }
    }

    pub(in crate::state) fn row_drag_msg(&mut self, m: DragMsg, out: &mut Vec<Request>) {
        match m {
            DragMsg::Start(item) => {
                if self.row_drag.is_none() && !item.is_heading() {
                    self.row_drag = Some(RowDrag {
                        item,
                        over: None,
                        onto: None,
                        compare: false,
                        cancelled: false,
                    });
                }
            }
            DragMsg::Over { target, compare } => {
                if let Some(d) = self.row_drag.as_mut().filter(|d| !d.cancelled) {
                    d.over = target;
                    d.compare = compare;
                    if target.is_some() {
                        d.onto = None;
                    }
                }
            }
            DragMsg::OverRow(row) => {
                if let Some(d) = self.row_drag.as_mut().filter(|d| !d.cancelled) {
                    // Its own row is where the drag started: nothing to combine with.
                    d.onto = row.filter(|r| *r != d.item && !r.is_heading());
                    if d.onto.is_some() {
                        d.over = None;
                    }
                }
            }
            DragMsg::Drop { compare } => {
                if let Some(d) = self.row_drag.take()
                    && !d.cancelled
                {
                    if let Some(onto) = d.onto {
                        self.drop_math(d.item, onto);
                    } else if let Some(t) = d.over {
                        self.drop_row(d.item, t, compare, out);
                    }
                }
            }
        }
    }

    /// Esc during a drag: the drag ends on release with nothing done. `false`: no drag.
    pub(in crate::state) fn cancel_row_drag(&mut self) -> bool {
        match self.row_drag.as_mut() {
            Some(d) if !d.cancelled => {
                d.cancelled = true;
                d.over = None;
                d.onto = None;
                true
            }
            _ => false,
        }
    }

    /// Pane `p` can be split in halves along `axis`, each at least the size N keeps.
    pub(in crate::state) fn can_split(&self, p: PaneId, axis: Axis) -> bool {
        let rect = self.laid_out_rect(p);
        let (w, h) = self.pane_area;
        let (along, extent) = match axis {
            Axis::Row => (rect.w, w),
            Axis::Column => (rect.h, h),
        };
        along / 2.0 >= crate::state::panes::MIN_RATIO * extent
    }

    /// `item` dropped on `target`: shown by that pane (or a half split off it), or with
    /// `compare` drawn compared over it.
    pub(in crate::state) fn drop_row(
        &mut self,
        item: TreeKey,
        target: DropTarget,
        compare: bool,
        out: &mut Vec<Request>,
    ) {
        if self.layout.view(target.pane).is_none() {
            return;
        }
        if compare {
            self.drop_compare(item, target.pane, out);
            return;
        }
        let mut p = target.pane;
        if let Some(e) = target.edge {
            let (axis, side) = edge_split(e);
            if let Some(new) = self.split_pane_at(p, axis, side) {
                p = new;
            }
        }
        // The same as a pick on that pane's chip: the pane shows the row's measurement (a
        // stored trace's, in the kind of pane that draws the trace); the selection stays.
        self.layout.set_focus(p);
        match item {
            TreeKey::Meas(id) | TreeKey::Live(id) | TreeKey::Math(id) => self.pane_show(p, id),
            TreeKey::Trace(id) => self.pane_show_trace(p, id),
            TreeKey::Imported | TreeKey::Rig | TreeKey::Session => {}
        }
    }

    /// Stored trace `id` dropped on pane `p`: the pane shows its measurement, turning into
    /// the kind that draws the trace when it cannot; a sweep run is the one its sweep pane
    /// draws.
    fn pane_show_trace(&mut self, p: PaneId, id: TraceId) {
        let Some(t) = self
            .daemon()
            .and_then(|s| s.traces.iter().find(|t| t.id == id))
            .cloned()
        else {
            return;
        };
        if !drawn_in(&t, self.layout.kind(p)) {
            self.set_pane_kind(p, display::home_pane(t.kind));
        }
        if let TraceOwner::Meas { meas } = ac2_scene::meas_list::group_of(&t, &self.measurements())
            && self
                .meas(meas)
                .is_some_and(|m| self.layout.kind(p).shows(&m.config.kind))
            && let Some(v) = self.layout.view_mut(p)
        {
            v.meas = Some(meas);
        }
        if t.kind == TraceKind::Sweep {
            self.rig.sweep.shown = Some(id);
        }
    }

    /// `item` compared into the measurement pane `p` shows, as the C chooser compares it: a
    /// graph pane draws compared curves of its kind; an SPL pane has none (meters are
    /// compared side by side), and an imported trace is no measurement's until moved under
    /// one.
    fn drop_compare(&mut self, item: TreeKey, p: PaneId, out: &mut Vec<Request>) {
        let Some(name) = self.row_name(item) else {
            return;
        };
        let kind = self.layout.kind(p);
        if kind == PaneKind::Spl {
            self.warn(pane_edit::compare_refused(
                &name,
                CompareRefusal::MeterPane {
                    pane: self.pane_number(p),
                },
            ));
            return;
        }
        let no_curve = CompareRefusal::NoCurve { what: kind.what() };
        let ms = self.measurements();
        let (compared, group, shown) = match item {
            TreeKey::Trace(id) => {
                let Ok(t) = self.trace_meta(id) else {
                    return;
                };
                if !crate::state::display::trace_compare_kinds(&t).contains(&kind) {
                    self.warn(pane_edit::compare_refused(&name, no_curve));
                    return;
                }
                let group = ac2_scene::meas_list::group_of(&t, &ms);
                if group == ac2_proto::model::TraceOwner::Imported {
                    self.warn(pane_edit::compare_refused(&name, CompareRefusal::Imported));
                    return;
                }
                (Compared::Trace { trace: id }, group, t.edit.visible)
            }
            TreeKey::Meas(id) | TreeKey::Live(id) | TreeKey::Math(id) => {
                let Some(m) = self.meas(id).cloned() else {
                    return;
                };
                if !crate::state::display::meas_compare_kinds(&m).contains(&kind) {
                    self.warn(pane_edit::compare_refused(&name, no_curve));
                    return;
                }
                let group = ac2_scene::meas_list::meas_group(&m, &ms);
                (Compared::Meas { meas: id }, group, !self.meas_hidden(&m))
            }
            TreeKey::Imported | TreeKey::Rig | TreeKey::Session => return,
        };
        let Some(target) = self
            .pane_meas(p)
            .map(|m| self.compare_owner(m))
            .and_then(|id| self.meas(id))
            .cloned()
        else {
            self.warn(format!(
                "{name} not compared: the pane shows no measurement yet"
            ));
            return;
        };
        if ac2_scene::meas_list::meas_group(&target, &ms) == group {
            self.warn(pane_edit::compare_refused(&name, CompareRefusal::OwnGroup));
            return;
        }
        self.layout.set_focus(p);
        if target.compared.contains(&compared) {
            self.toast(format!(
                "{name}: already compared into {}",
                target.config.name
            ));
            return;
        }
        self.set_compared(target.id, compared, true, &name, shown, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outer_share_of_a_pane_is_its_edge() {
        let rects = [
            (PaneId(1), PaneRect::new(0.0, 0.0, 100.0, 100.0)),
            (PaneId(2), PaneRect::new(0.0, 100.0, 100.0, 100.0)),
        ];
        let at = |x, y| drop_target(&rects, x, y);
        assert_eq!(
            at(50.0, 50.0),
            Some(DropTarget {
                pane: PaneId(1),
                edge: None
            })
        );
        assert_eq!(at(50.0, 190.0).and_then(|t| t.edge), Some(Edge::Bottom));
        assert_eq!(at(50.0, 105.0).and_then(|t| t.edge), Some(Edge::Top));
        assert_eq!(at(5.0, 150.0).and_then(|t| t.edge), Some(Edge::Left));
        assert_eq!(at(95.0, 150.0).and_then(|t| t.edge), Some(Edge::Right));
        assert_eq!(at(50.0, 150.0).map(|t| t.pane), Some(PaneId(2)));
        // A corner goes to the nearer side.
        assert_eq!(at(3.0, 10.0).and_then(|t| t.edge), Some(Edge::Left));
        assert_eq!(at(150.0, 50.0), None, "off the panes");
    }
}
