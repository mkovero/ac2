//! The reducer's pane operations: which measurement and modes each pane shows, and
//! splitting, closing, resizing and focusing panes.

use super::panes::MIN_RATIO;
use super::*;

impl AppState {
    /// Measurement `own` among those pane kind `kind` can show; else the selected one when
    /// its home is that kind's; else the first whose home it is, else the first it can show.
    /// Unchosen, a pane leads with its own kind: the transfer pane shows a sweep's runs only
    /// once picked for it, a live curve before that.
    fn resolve_meas(&self, kind: PaneKind, own: Option<MeasId>) -> Option<&Measurement> {
        let c = self.pane_candidates(kind);
        let pick = |id: Option<MeasId>| id.and_then(|id| c.iter().find(|m| m.id == id).copied());
        let home = |m: &Measurement| PaneKind::for_kind(&m.config.kind) == kind;
        pick(own)
            .or_else(|| pick(self.selected).filter(|m| home(m)))
            .or_else(|| c.iter().find(|m| home(m)).copied())
            .or_else(|| c.first().copied())
    }

    /// The measurement pane `id` shows: its own choice, else the selected measurement if it
    /// fits, else the first that fits.
    pub fn pane_meas(&self, id: PaneId) -> Option<&Measurement> {
        let v = self.layout.view(id)?;
        self.resolve_meas(v.kind, v.meas)
    }

    /// The measurement "the `kind` pane" shows, for what is about a kind rather than one pane
    /// (an SPL command, the delay banner): the focused pane's when it is of that kind, else
    /// the one of that kind focused last; with none on screen, what one would show.
    pub fn kind_meas(&self, kind: PaneKind) -> Option<&Measurement> {
        match self.layout.lead(kind) {
            Some(id) => self.pane_meas(id),
            None => self.resolve_meas(kind, None),
        }
    }

    /// The view state pane `id` is drawn with: the shared one (the frequency axis, read
    /// across panes) with the pane's own modes and level axis.
    pub fn view_for(&self, id: PaneId) -> ViewState {
        let mut v = self.view;
        self.pane_levels(id).apply(&mut v);
        let modes = self.pane_modes(id);
        v.spectrum.mode = modes.spectrum;
        v.ir.mode = match self.layout.kind(id) {
            PaneKind::Distortion => modes.ir(IrPane::Sweep),
            _ => modes.ir(IrPane::Live),
        };
        v.chrome = modes.chrome;
        v.distortion.reference = modes.sweep_reference;
        // The transfer scene lays out the plots it is asked for: one alone fills the pane.
        // The coherence rides on the magnitude view only when placed over it.
        let over =
            v.tf.coherence_placement == ac2_scene::view::CoherencePlacement::OverlayOnMagnitude;
        let shown = match modes.transfer {
            TransferView::Ir(_) => None,
            TransferView::Magnitude => Some((true, false, over)),
            TransferView::Phase => Some((false, true, false)),
            TransferView::Coherence => Some((false, false, true)),
        };
        if let Some((m, p, c)) = shown {
            (v.tf.show_magnitude, v.tf.show_phase, v.tf.show_coherence) = (m, p, c);
        }
        // The stack's top pane carries the legend for the column.
        if self.stacked_under(id) {
            v.tf.legend.hidden = true;
        }
        v
    }

    /// Whether pane `id` is a lower pane of a transfer stack: a phase or coherence view
    /// right under a pane of the same measurement that is the magnitude view, or itself
    /// such a lower pane. Whoever laid the panes out (the template, N and G by hand), they
    /// read as one column, and the top pane's legend and delay banner speak for it.
    pub fn stacked_under(&self, id: PaneId) -> bool {
        let lower = |p: PaneId| {
            self.layout.view(p).is_some_and(|v| {
                v.kind == PaneKind::Transfer
                    && matches!(
                        v.modes.transfer,
                        TransferView::Phase | TransferView::Coherence
                    )
            })
        };
        let mut p = id;
        // Each step goes one pane up the tree: at most as many as there are panes.
        for _ in 0..self.layout.views.len() {
            if !lower(p) {
                return false;
            }
            let Some(up) = self.layout.root.above(p) else {
                return false;
            };
            let same = self.layout.kind(up) == PaneKind::Transfer
                && self.pane_meas(up).map(|m| m.id) == self.pane_meas(p).map(|m| m.id)
                && self.pane_meas(p).is_some();
            if !same {
                return false;
            }
            if self.pane_modes(up).transfer == TransferView::Magnitude {
                return true;
            }
            p = up;
        }
        false
    }

    /// Pane `id`'s modes.
    pub fn pane_modes(&self, id: PaneId) -> PaneModes {
        self.layout.view(id).map(|v| v.modes).unwrap_or_default()
    }

    /// The focused pane's modes.
    pub fn modes(&self) -> PaneModes {
        self.layout.focused().modes
    }

    /// The modes of the pane of `kind` the keys mean (focused or last focused), else the
    /// focused pane's.
    pub fn kind_modes(&self, kind: PaneKind) -> PaneModes {
        self.layout
            .lead(kind)
            .and_then(|id| self.layout.view(id))
            .map_or(self.modes(), |v| v.modes)
    }

    pub(super) fn modes_mut(&mut self) -> &mut PaneModes {
        &mut self.layout.focused_mut().modes
    }

    /// Some spectrum pane shows the spectrograph: its history is kept.
    pub fn spectrograph_shown(&self) -> bool {
        self.layout
            .views
            .values()
            .any(|v| v.kind == PaneKind::Spectrum && v.modes.spectrum.spectrograph())
    }

    /// The tree drawn now: the whole layout, or the focused pane alone when maximised.
    pub fn visible_tree(&self) -> PaneNode {
        if self.layout.maximized {
            PaneNode::Leaf(self.layout.focus)
        } else {
            self.layout.root.clone()
        }
    }

    /// Panes drawn now, in reading order.
    pub fn visible_panes(&self) -> Vec<PaneId> {
        self.visible_tree().reading_order()
    }

    /// The kinds of the panes drawn now.
    pub fn visible_kinds(&self) -> Vec<PaneKind> {
        self.visible_panes()
            .into_iter()
            .map(|id| self.layout.kind(id))
            .collect()
    }

    /// Where the panes drawn now go inside `area`.
    pub fn pane_rects(&self, area: PaneRect, gap: f32) -> Vec<(PaneId, PaneRect)> {
        self.visible_tree().rects(area, gap)
    }

    /// The focused pane's rectangle in the area last drawn, as the tree is laid out there
    /// (a split un-maximises, so the laid-out tree is what shows after it).
    fn focused_rect(&self) -> PaneRect {
        self.laid_out_rect(self.layout.focus)
    }

    /// Pane `p`'s rectangle in the area last drawn, the tree laid out (not maximised).
    pub(super) fn laid_out_rect(&self, p: PaneId) -> PaneRect {
        let (w, h) = self.pane_area;
        self.layout
            .root
            .rects(PaneRect::new(0.0, 0.0, w, h), 0.0)
            .into_iter()
            .find(|(id, _)| *id == p)
            .map_or(PaneRect::new(0.0, 0.0, w, h), |(_, r)| r)
    }

    /// Pane `p` replaced by template `t`'s panes, each showing measurement `meas`; the
    /// top one keeps `p` and gets the focus. Refused (with why) when a pane of it would be
    /// lower than N splits to.
    pub(super) fn apply_template(&mut self, t: PaneTemplate, p: PaneId, meas: MeasId) -> bool {
        let Some(base) = self.layout.view(p).copied() else {
            return false;
        };
        let rect = self.laid_out_rect(p);
        let least = t.panes().iter().map(|x| x.2).fold(f32::INFINITY, f32::min);
        let total: f32 = t.panes().iter().map(|x| x.2).sum();
        if rect.h * least / total < MIN_RATIO * self.pane_area.1 {
            self.warn(format!(
                "the pane is too small for the {}: Q closes one to make room",
                t.label()
            ));
            return false;
        }
        let views: Vec<(View, f32)> = t
            .panes()
            .iter()
            .map(|&(kind, transfer, share)| {
                let mut v = base;
                v.kind = kind;
                v.meas = Some(meas);
                v.modes.transfer = transfer;
                (v, share)
            })
            .collect();
        if !self.layout.stack(p, &views) {
            return false;
        }
        self.pending_pane_meas.remove(&p);
        self.focus_pane(p);
        true
    }

    /// The transfer stack laid out for the focused transfer pane's measurement; on another
    /// pane, the focused pane turns into it for the selected transfer measurement.
    pub(super) fn transfer_stack(&mut self) {
        let f = self.layout.focus;
        let meas = if self.layout.kind(f) == PaneKind::Transfer {
            self.pane_meas(f)
        } else {
            self.selected.and_then(|id| self.meas(id))
        }
        .filter(|m| matches!(m.config.kind, MeasKind::Transfer { .. }))
        .map(|m| m.id);
        match meas {
            Some(m) => {
                let t = PaneTemplate::TransferStack;
                if self.apply_template(t, f, m) {
                    self.toast(t.label());
                }
            }
            None => self.warn("no transfer measurement to stack: select one in the list"),
        }
    }

    /// A transfer measurement this app created: the pane that shows it now (the focused
    /// transfer pane, else the transfer pane worked in last) lays it out in the transfer
    /// stack. With no transfer pane, a pane of its own split off the focused one as N
    /// splits it holds the stack (or, too small for one, the transfer view alone), so the
    /// panes already there keep their kinds; only a pane too small to split turns into it.
    pub(super) fn stack_created(&mut self, meas: MeasId) {
        let p = match self.layout.focus_kind() {
            PaneKind::Transfer => self.layout.focus,
            _ => match self.layout.lead(PaneKind::Transfer) {
                Some(p) => p,
                None => match self.split_focused_pane() {
                    Some(new) => {
                        self.set_pane_kind(new, PaneKind::Transfer);
                        if let Some(v) = self.layout.view_mut(new) {
                            v.meas = Some(meas);
                        }
                        self.focus_pane(new);
                        new
                    }
                    None => self.layout.focus,
                },
            },
        };
        self.apply_template(PaneTemplate::TransferStack, p, meas);
    }

    /// N: the focused pane split in two along its longer side; the new half shows the
    /// same and has the focus. Maximised, the split shows at once: the layout goes back to
    /// all panes.
    pub(super) fn split_pane(&mut self) {
        if self.split_focused_pane().is_none() {
            self.warn("the pane is too small to split: Q closes one to make room");
        }
    }

    /// N's split: the new pane, or `None` when a half would be smaller than resizing keeps
    /// a pane.
    fn split_focused_pane(&mut self) -> Option<PaneId> {
        let rect = self.focused_rect();
        let axis = if rect.w >= rect.h {
            Axis::Row
        } else {
            Axis::Column
        };
        self.split_pane_at(self.layout.focus, axis, Side::B)
    }

    /// Pane `p` split in halves along `axis`, the new half on `side` (`A`: left or top),
    /// showing what `p` does and taking the focus: the new pane, or `None` when a half would
    /// be smaller than resizing keeps a pane.
    pub(super) fn split_pane_at(&mut self, p: PaneId, axis: Axis, side: Side) -> Option<PaneId> {
        self.layout.view(p)?;
        if !self.can_split(p, axis) {
            return None;
        }
        // Both halves keep showing what the pane did, whatever is selected later: an
        // unchosen pane would follow the selection.
        let shown = self.pane_meas(p).map(|m| m.id);
        if let Some(v) = self.layout.view_mut(p) {
            v.meas = v.meas.or(shown);
        }
        let waiting = self.pending_pane_meas.get(&p).cloned();
        self.layout.maximized = false;
        let new = self.layout.split_pane(p, axis, side);
        self.copy_levels(p, new);
        if let Some(n) = waiting {
            self.pending_pane_meas.insert(new, n);
        }
        Some(new)
    }

    /// A measurement this app created, shown on the pane of its kind worked in last; with
    /// none, on a pane of its own split off the focused one as N splits it, so the panes
    /// already there keep their kinds. A sweep's pane takes the focus, as its runs are armed
    /// there: with no room to split, the focused pane turns into it.
    pub(super) fn open_created(&mut self, meas: MeasId, kind: PaneKind) {
        if self.layout.lead(kind).is_none()
            && let Some(new) = self.split_focused_pane()
        {
            self.set_pane_kind(new, kind);
            if let Some(v) = self.layout.view_mut(new) {
                v.meas = Some(meas);
            }
        }
        if kind == PaneKind::Distortion {
            self.focus_kind(kind);
        }
    }

    /// Q: the focused pane closed, its neighbour taking its place and the focus; the
    /// last pane stays.
    pub(super) fn close_pane(&mut self) {
        let gone = self.layout.focus;
        if !self.layout.close_focused() {
            self.warn("the last pane stays: Tab puts another measurement in it");
            return;
        }
        if matches!(self.overlay, Overlay::PaneMenu(m) if m.pane == gone) {
            self.overlay = Overlay::None;
        }
        let f = self.layout.focus;
        self.pending_pane_meas
            .retain(|id, _| self.layout.views.contains_key(id));
        self.select_shown(f);
    }

    /// The panes' area last drawn, without gaps: what resizing keeps every pane readable in.
    fn area(&self) -> PaneRect {
        let (w, h) = self.pane_area;
        PaneRect::new(0.0, 0.0, w, h)
    }

    /// Shift+N: the split that holds the focused pane turns, side by side ↔ stacked, its
    /// ratio and order kept. Maximised, the turn shows at once: the layout goes back to
    /// all panes.
    pub(super) fn turn_split(&mut self) {
        let Some((path, _)) = self.layout.root.parent_of(self.layout.focus) else {
            self.warn("one pane: nothing to turn");
            return;
        };
        let before = self.layout.root.clone();
        if let Some(PaneNode::Split { axis, .. }) = self.layout.root.at_mut(&path) {
            *axis = match axis {
                Axis::Row => Axis::Column,
                Axis::Column => Axis::Row,
            };
        }
        // The other axis may be too short for what the split holds: a pane past the
        // minimum is too small to read, so the turn is refused rather than squeezed.
        let area = self.area();
        match self.layout.root.ratio_range(&path, area) {
            Some((lo, hi)) if lo <= hi => {
                if let Some(PaneNode::Split { ratio, .. }) = self.layout.root.at_mut(&path) {
                    *ratio = ratio.clamp(lo, hi);
                }
                self.layout.maximized = false;
            }
            _ => {
                self.layout.root = before;
                self.warn("the panes would be too small turned: Q closes one to make room");
            }
        }
    }

    /// Alt+Right / Alt+Left: the focused pane grows / shrinks by a tenth of the split
    /// that holds it, never past the size N refuses to split below.
    pub(super) fn resize_pane(&mut self, grow: bool) {
        const STEP: f32 = 0.1;
        let Some((path, side)) = self.layout.root.parent_of(self.layout.focus) else {
            self.warn("one pane: nothing to resize");
            return;
        };
        let Some((lo, hi)) = self.layout.root.ratio_range(&path, self.area()) else {
            return;
        };
        let Some(PaneNode::Split { ratio, .. }) = self.layout.root.at_mut(&path) else {
            return;
        };
        // The ratio is `a`'s share: a pane on the `b` side grows as it falls.
        let up = (side == Side::A) == grow;
        let now = *ratio;
        let next = if up {
            (now + STEP).min(hi).max(now)
        } else {
            (now - STEP).max(lo).min(now)
        };
        if (next - now).abs() < 1e-6 {
            self.warn(if grow {
                "the pane is as large as it goes: Q closes a neighbour to make room"
            } else {
                "the pane is as small as it goes"
            });
            return;
        }
        *ratio = next;
        self.layout.maximized = false;
    }

    /// A gap dragged: the split at `path` takes `ratio`, kept to the range Alt+Left/Right
    /// stop at.
    pub(super) fn drag_split(&mut self, path: &[Side], ratio: f32) {
        if !ratio.is_finite() {
            return;
        }
        let Some((lo, hi)) = self.layout.root.ratio_range(path, self.area()) else {
            return;
        };
        if lo > hi {
            return;
        }
        if let Some(PaneNode::Split { ratio: r, .. }) = self.layout.root.at_mut(path) {
            *r = ratio.clamp(lo, hi);
        }
    }

    /// Alt+1 … 9: focuses the `n`-th pane drawn now, in reading order (1-based).
    pub(super) fn focus_nth(&mut self, n: usize) {
        let panes = self.layout.panes();
        match panes.get(n.saturating_sub(1)) {
            Some(id) => {
                let id = *id;
                self.focus_pane(id);
            }
            None => self.warn(format!("no pane {n}: {} on screen (N splits)", panes.len())),
        }
    }

    /// Focuses pane `id` from the keyboard: it selects the measurement the pane shows,
    /// unless the selected stored trace is drawn there (the sweep chosen on the sweep pane
    /// stays selected on the way to the transfer pane).
    pub(super) fn focus_pane(&mut self, id: PaneId) {
        self.layout.set_focus(id);
        let p = self.layout.kind(id);
        let keep = self
            .selected_trace_meta()
            .filter(|t| drawn_in(t, p))
            .map(|t| t.id);
        self.select_shown(id);
        if keep.is_some() {
            self.selected_trace = keep;
        }
    }

    /// Whether the selection (a stored trace, else a measurement) is of what pane `p`
    /// shows: of its measurement's group, or a stored curve the pane draws. A click there
    /// keeps it, so the trace the keys act on does not jump to the live curve under it.
    pub(super) fn selection_on_pane(&self, p: PaneId) -> bool {
        use ac2_proto::model::TraceOwner;
        let Some(shown) = self.pane_meas(p) else {
            return false;
        };
        let ms = self.measurements();
        let group = TraceOwner::Meas {
            meas: self.group_of_shown(Some(shown)).unwrap_or(shown.id),
        };
        let kind = self.layout.kind(p);
        if let Some(t) = self.selected_trace_meta() {
            if !drawn_in(t, kind) {
                return false;
            }
            let drawn = self.on_pane(kind, t, Some(shown))
                || self.trace_compared_on(kind, t, Some(shown))
                || self
                    .compared_sweeps(p)
                    .iter()
                    .any(|(d, _)| d.meta.id == t.id);
            return drawn || ac2_scene::meas_list::group_of(t, &ms) == group;
        }
        self.selected
            .and_then(|id| self.meas(id))
            .is_some_and(|m| m.id == shown.id || ac2_scene::meas_list::meas_group(m, &ms) == group)
    }

    /// Brings up a pane of kind `p` (a command about that kind: the sweep's views, the SPL
    /// modes): the one of that kind on screen worked in last, else the focused pane turns
    /// into one.
    pub(super) fn focus_kind(&mut self, p: PaneKind) {
        let id = match self.layout.lead(p) {
            Some(id) => id,
            None => {
                let f = self.layout.focus;
                self.set_pane_kind(f, p);
                f
            }
        };
        self.focus_pane(id);
    }

    /// Pane `id` turns into a pane of kind `kind`, following the selection.
    pub(super) fn set_pane_kind(&mut self, id: PaneId, kind: PaneKind) {
        if let Some(v) = self.layout.view_mut(id) {
            if v.kind != kind {
                v.meas = None;
            }
            v.kind = kind;
        }
        self.pending_pane_meas.remove(&id);
        // The history goes with the last spectrograph shown.
        if !self.spectrograph_shown() {
            self.spectrographs.clear();
            self.view.spectrum.spectrograph.cursor_s = None;
        }
    }

    /// The rows of pane `id`'s list: every measurement in the tree's order (the pane turns
    /// into the kind a picked one needs), then the kinds of pane no measurement brings, so
    /// a pane can still be readied for a measurement not yet made; last, the New
    /// measurement chooser, where a list of what exists is looked in for what does not.
    pub fn pane_menu_rows(&self, id: PaneId) -> Vec<(PaneMenuRow, String)> {
        let kind = self.layout.kind(id);
        let meas: Vec<&Measurement> = self
            .tree_meas_order()
            .into_iter()
            .filter_map(|m| self.meas(m))
            .collect();
        let mut v: Vec<(PaneMenuRow, String)> = meas
            .iter()
            .map(|m| {
                let hidden = if self.meas_hidden(m) {
                    " · hidden"
                } else {
                    ""
                };
                (
                    PaneMenuRow::Meas(m.id),
                    format!(
                        "{}  {}{hidden}",
                        ac2_scene::meas_list::kind_tag(&m.config.kind),
                        m.config.name
                    ),
                )
            })
            .collect();
        v.extend(
            PaneKind::ALL
                .into_iter()
                .filter(|k| *k != kind && !meas.iter().any(|m| k.shows(&m.config.kind)))
                .map(|k| (PaneMenuRow::Kind(k), format!("pane  {}", k.title()))),
        );
        let key = self.keys.label(CommandId::NewMeasurement, Scope::Global);
        v.push((
            PaneMenuRow::NewMeasurement,
            ac2_scene::new_meas::menu_row(key.as_deref()),
        ));
        v
    }

    /// A row of pane `id`'s list taken: it shows that measurement (selected), or turns into
    /// that kind of pane.
    pub(super) fn pane_pick(&mut self, id: PaneId, row: PaneMenuRow) {
        if matches!(self.overlay, Overlay::PaneMenu(_)) {
            self.overlay = Overlay::None;
        }
        self.pane_picked = true;
        match row {
            PaneMenuRow::Meas(m) => self.pane_show(id, m),
            PaneMenuRow::Kind(k) => {
                self.set_pane_kind(id, k);
                self.focus_pane(id);
            }
            PaneMenuRow::NewMeasurement => {
                // The pane whose list it was is the one the chooser's guess is about, and
                // the one the new measurement's pane splits off.
                self.focus_pane(id);
                let keymap = self.keys.clone();
                self.open_new_measurement(id, &keymap);
            }
        }
    }
}
