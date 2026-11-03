//! A row dragged onto the panes: a row of the measurement list (any connected rig's) or of
//! the Inputs list. This is the one way to change what a pane shows. Dropped in the middle
//! of a pane, the pane shows that row alone, turning into the kind of pane that draws it
//! (the drag label says so before release); dropped near one of its sides, the pane splits
//! there first, as N would, and the new half shows it. A drop pins the pane to the row's
//! rig. The selection in the list stays; the focus moves to the pane dropped on.
//!
//! Dropped on another row of the measurement list instead, it opens the math dialog
//! combining the two (`crate::state::drop_math`); rows of two rigs do not combine.
//!
//! A drop is a guest pick: it never brings the dropped measurement's own panes, and an edit
//! of the panes afterwards still goes to the measurement whose row was chosen
//! ([`crate::state::panes::meas_layout`]).

use ac2_proto::units::RigId;
use ac2_scene::meas_list::TreeKey;
use ac2_scene::pane_edit::{self, DropSpot, Edge};

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

/// What is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragItem {
    /// A row of rig `rig`'s measurement list.
    Row { rig: RigId, key: TreeKey },
    /// A row of the Inputs list, by its channel (a pair's by its left side): a Programme
    /// pane meters it.
    Input(u16),
}

impl DragItem {
    /// The measurement-list row it is, if it is one.
    pub fn row(self) -> Option<(RigId, TreeKey)> {
        match self {
            DragItem::Row { rig, key } => Some((rig, key)),
            DragItem::Input(_) => None,
        }
    }
}

/// A list row being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowDrag {
    pub item: DragItem,
    /// The pane under the pointer, if any.
    pub over: Option<DropTarget>,
    /// The measurement-list row under the pointer, if any: a drop there combines the two.
    pub onto: Option<(RigId, TreeKey)>,
    /// Esc cancelled it: nothing happens on release.
    pub cancelled: bool,
}

/// The mouse dragging a list row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragMsg {
    /// The pointer moved far enough with the button held on the row of `item`.
    Start(DragItem),
    /// The pointer over the panes (`None`: elsewhere).
    Over(Option<DropTarget>),
    /// The pointer over a row of the measurement list (`None`: off the list's rows).
    OverRow(Option<(RigId, TreeKey)>),
    /// The button released: dropped where the pointer last was over.
    Drop,
}

/// What a dragged row needs a pane to be: its name, the kinds of pane that draw it, and the
/// kind a pane of another kind turns into for it.
#[derive(Clone, Debug, PartialEq)]
pub struct RowDrop {
    pub name: String,
    pub draws: Vec<PaneKind>,
    pub home: PaneKind,
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

    /// What a row of the rig in `rig`'s list is called in the drag label and toasts: a
    /// measurement's or math channel's name, a stored trace's label.
    pub(in crate::state) fn row_name(&self, item: TreeKey) -> Option<String> {
        self.row_drop(item).map(|d| d.name)
    }

    /// A row of the rig in `rig`'s list as a drop: what it is called and what draws it.
    pub(in crate::state) fn row_drop(&self, item: TreeKey) -> Option<RowDrop> {
        match item {
            TreeKey::Meas(id) | TreeKey::Live(id) | TreeKey::Math(id) => {
                let m = self.meas(id)?;
                Some(RowDrop {
                    name: m.config.name.clone(),
                    draws: PaneKind::ALL
                        .into_iter()
                        .filter(|k| k.shows(&m.config.kind))
                        .collect(),
                    home: PaneKind::for_kind(&m.config.kind),
                })
            }
            TreeKey::Trace(id) => {
                let t = self.trace_meta(id).ok()?;
                Some(RowDrop {
                    name: trace_label(&t),
                    draws: PaneKind::ALL
                        .into_iter()
                        .filter(|k| drawn_in(&t, *k))
                        .collect(),
                    home: display::home_pane(t.kind),
                })
            }
            TreeKey::Imported | TreeKey::Rig | TreeKey::Session | TreeKey::RigName(_) => None,
        }
    }

    /// The dragged item as a drop: for another rig's row, from that rig's list as last read.
    fn item_drop(&self, item: DragItem) -> Option<RowDrop> {
        match item {
            DragItem::Row { rig, key } if rig == self.rig.id => self.row_drop(key),
            DragItem::Row { rig, key } => self.other_row_drop(rig, key),
            DragItem::Input(c) => {
                let m = self.row_meters(c)?;
                Some(RowDrop {
                    name: self.metered_name(m),
                    draws: vec![PaneKind::Programme],
                    home: PaneKind::Programme,
                })
            }
        }
    }

    /// Where a drop on `target` lands: an edge drop on a pane too small to split (as N
    /// refuses) is a plain drop on it.
    pub fn drop_spot(&self, target: DropTarget) -> Option<DropSpot> {
        self.layout.view(target.pane)?;
        let n = self.pane_number(target.pane);
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

    /// The label beside the pointer: `FOH SPL → pane 2`, `FOH SPL → pane 2, becomes SPL`
    /// where the drop turns the pane's kind; over a row the math channel the drop opens (or
    /// why the two do not combine); `None` elsewhere.
    pub fn drag_label(&self) -> Option<String> {
        let d = self.row_drag()?;
        if let Some(onto) = d.onto {
            return Some(match self.drop_math_plan_of(d.item, onto) {
                Ok(p) => p.label,
                Err(why) => why,
            });
        }
        let t = d.over?;
        let spot = self.drop_spot(t)?;
        let drop = self.item_drop(d.item)?;
        let kind = self.layout.kind(t.pane);
        let becomes = (matches!(spot, DropSpot::Pane(_)) && !drop.draws.contains(&kind))
            .then(|| drop.home.title());
        Some(pane_edit::drop_label(&drop.name, spot, becomes))
    }

    /// What the drop would take, for its highlight: the pane, and the side that becomes
    /// the new pane on an edge drop.
    pub fn drop_highlight(&self) -> Option<(PaneId, Option<Edge>)> {
        let d = self.row_drag()?;
        let t = d.over?;
        match self.drop_spot(t)? {
            DropSpot::NewPane(e) => Some((t.pane, Some(e))),
            DropSpot::Pane(_) => Some((t.pane, None)),
        }
    }

    /// The name the undo history gives a drop of `item`.
    pub(in crate::state) fn drag_item_name(&self, item: DragItem) -> String {
        self.item_drop(item).map(|d| d.name).unwrap_or_default()
    }

    pub(in crate::state) fn row_drag_msg(&mut self, m: DragMsg, out: &mut Vec<Request>) {
        match m {
            DragMsg::Start(item) => {
                let heading = item.row().is_some_and(|(_, k)| k.is_heading());
                if self.row_drag.is_none() && !heading {
                    self.row_drag = Some(RowDrag {
                        item,
                        over: None,
                        onto: None,
                        cancelled: false,
                    });
                }
            }
            DragMsg::Over(target) => {
                if let Some(d) = self.row_drag.as_mut().filter(|d| !d.cancelled) {
                    d.over = target;
                    if target.is_some() {
                        d.onto = None;
                    }
                }
            }
            DragMsg::OverRow(row) => {
                if let Some(d) = self.row_drag.as_mut().filter(|d| !d.cancelled) {
                    // Its own row is where the drag started: nothing to combine with; an
                    // Inputs row combines with nothing.
                    d.onto = row
                        .filter(|r| d.item.row().is_some_and(|own| own != *r) && !r.1.is_heading());
                    if d.onto.is_some() {
                        d.over = None;
                    }
                }
            }
            DragMsg::Drop => {
                if let Some(d) = self.row_drag.take()
                    && !d.cancelled
                {
                    if let (Some(onto), Some(item)) = (d.onto, d.item.row()) {
                        self.drop_on_row(item, onto, out);
                    } else if let Some(t) = d.over {
                        self.drop_row(d.item, t);
                    }
                }
            }
        }
    }

    /// A row dropped on another row: the math dialog combining them, on their rig (shown
    /// first when it is not); rows of two rigs are refused.
    fn drop_on_row(
        &mut self,
        (rig, item): (RigId, TreeKey),
        (onto_rig, onto): (RigId, TreeKey),
        out: &mut Vec<Request>,
    ) {
        if rig != onto_rig {
            self.warn(pane_edit::cross_rig_math_refused(&self.rig_name(onto_rig)));
            return;
        }
        if rig != self.rig.id {
            self.switch_rig(rig, out);
            if self.rig.id != rig {
                return;
            }
        }
        self.drop_math(item, onto);
    }

    /// [`Self::drop_math_plan`] for a drag: rows of two rigs do not combine, and another
    /// rig's rows are planned once that rig is shown (the label says what the drop does).
    fn drop_math_plan_of(
        &self,
        item: DragItem,
        (onto_rig, onto): (RigId, TreeKey),
    ) -> Result<DropPlan, String> {
        let Some((rig, key)) = item.row() else {
            return Err(String::new());
        };
        if rig != onto_rig {
            return Err(pane_edit::cross_rig_math_refused(&self.rig_name(onto_rig)));
        }
        if rig != self.rig.id {
            return Err(format!(
                "drop to show {} and combine the two there",
                self.rig_name(rig)
            ));
        }
        self.drop_math_plan(key, onto)
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

    /// `item` dropped on `target`: shown alone by that pane (or a half split off it).
    pub(in crate::state) fn drop_row(&mut self, item: DragItem, target: DropTarget) {
        if self.layout.view(target.pane).is_none() {
            return;
        }
        let mut p = target.pane;
        if let Some(e) = target.edge {
            let (axis, side) = edge_split(e);
            if let Some(new) = self.split_pane_at(p, axis, side) {
                p = new;
            }
        }
        self.layout.set_focus(p);
        match item {
            DragItem::Row { rig, key } if rig == self.rig.id => match key {
                TreeKey::Meas(id) | TreeKey::Live(id) | TreeKey::Math(id) => {
                    self.pane_show(p, id);
                }
                TreeKey::Trace(id) => self.pane_show_trace(p, id),
                TreeKey::Imported | TreeKey::Rig | TreeKey::Session | TreeKey::RigName(_) => {}
            },
            DragItem::Row { rig, key } => self.pane_row_from(p, rig, key),
            DragItem::Input(c) => {
                if let Some(m) = self.row_meters(c) {
                    let rig = self.rig.id;
                    self.pane_meter_from(p, rig, m);
                    // A pair opens on its goniometer, inputs on their bars (G steps them).
                    if let Some(v) = self.layout.view_mut(p) {
                        v.modes.programme = match m.pair() {
                            Some(_) => ac2_scene::view::ProgrammeMode::Goniometer,
                            None => ac2_scene::view::ProgrammeMode::Meters,
                        };
                    }
                }
            }
        }
    }

    /// Stored trace `id` dropped on pane `p`: the pane shows its measurement, turning into
    /// the kind that draws the trace when it cannot; a sweep run is the one its sweep pane
    /// draws.
    pub(in crate::state) fn pane_show_trace(&mut self, p: PaneId, id: TraceId) {
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
        let here = self.here();
        if let TraceOwner::Meas { meas } = ac2_scene::meas_list::group_of(&t, &self.measurements())
            && self
                .meas(meas)
                .is_some_and(|m| self.layout.kind(p).shows(&m.config.kind))
            && let Some(v) = self.layout.view_mut(p)
        {
            v.meas = Some(meas);
            v.with.clear();
            v.rig = here;
            self.rig.pending.deleted.remove(&p);
        }
        if t.kind == TraceKind::Sweep {
            self.rig.sweep.shown = Some(id);
        }
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
