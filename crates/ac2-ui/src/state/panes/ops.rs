//! The reducer's pane operations: which measurement and modes each pane shows, and
//! splitting, closing, resizing and focusing panes.

use crate::state::panes::MIN_RATIO;
use crate::state::*;
use ac2_scene::freq_column::{FreqPlot, natural_right};

impl AppState {
    /// Measurement `own` when pane kind `kind` can show it. An unpicked pane shows nothing:
    /// it never stands in the selection or the first of a kind, so what a pane shows is only
    /// ever what was put there.
    fn resolve_meas(&self, kind: PaneKind, own: Option<MeasId>) -> Option<&Measurement> {
        let own = self.meas(own?)?;
        kind.shows(&own.config.kind).then_some(own)
    }

    /// The measurement pane `id` shows: its pick, if it can draw it.
    /// None for a pane of another rig: its ids are that rig's.
    pub fn pane_meas(&self, id: PaneId) -> Option<&Measurement> {
        let v = self.layout.view(id).filter(|v| self.ours(v))?;
        self.resolve_meas(v.kind, v.meas)
    }

    /// The measurement "the `kind` pane" shows, for what is about a kind rather than one pane
    /// (an SPL command, the delay banner): the focused pane's when it is of that kind, else
    /// the one of that kind focused last; with none showing one, the selected measurement
    /// when it is of that kind, else none (the caller says what to select).
    pub fn kind_meas(&self, kind: PaneKind) -> Option<&Measurement> {
        self.lead(kind)
            .and_then(|id| self.pane_meas(id))
            .or_else(|| {
                self.selected_meas()
                    .filter(|m| PaneKind::for_kind(&m.config.kind) == kind)
            })
    }

    /// The measurement "the `kind` pane" shows: the focused pane's when it is of that kind,
    /// else that of the one of that kind focused last; never one picked for it. What acts on
    /// what a pane shows (the SPL commands, the delay banner) follows it.
    pub fn shown_of_kind(&self, kind: PaneKind) -> Option<&Measurement> {
        self.lead(kind).and_then(|id| self.pane_meas(id))
    }

    /// The view state pane `id` is drawn with: the shared one (the frequency axis, read
    /// across panes) with the pane's own modes and level axis, its frequency plot as wide
    /// as its column's ([`Self::column_right`]).
    pub fn view_for(&self, id: PaneId) -> ViewState {
        ViewState {
            freq_right: self.column_right(id),
            ..self.own_view(id)
        }
    }

    /// What pane `id` draws on a log-frequency x axis, if anything.
    fn freq_plot(&self, id: PaneId) -> Option<FreqPlot> {
        let v = self.layout.view(id)?;
        match v.kind {
            PaneKind::Spectrum => Some(FreqPlot::Spectrum),
            PaneKind::Transfer if !v.shows_ir() => Some(FreqPlot::Transfer),
            PaneKind::Distortion if v.modes.sweep == ac2_scene::view::SweepMode::Response => {
                Some(FreqPlot::Distortion)
            }
            PaneKind::Transfer
            | PaneKind::Distortion
            | PaneKind::Spl
            | PaneKind::Programme
            | PaneKind::Walk => None,
        }
    }

    /// The right margin pane `id`'s frequency plot keeps: the widest any frequency pane
    /// stacked in its column (same left edge and width, within a pixel) needs on its own,
    /// so one frequency is at one x down the column. A pane alone, or one without a
    /// frequency axis, keeps its own (0).
    pub fn column_right(&self, id: PaneId) -> f32 {
        if self.freq_plot(id).is_none() {
            return 0.0;
        }
        let (w, h) = self.pane_area;
        let rects = self.pane_rects(PaneRect::new(0.0, 0.0, w, h), 0.0);
        let Some(&(_, mine)) = rects.iter().find(|(p, _)| *p == id) else {
            return 0.0;
        };
        rects
            .iter()
            .filter(|(_, r)| (r.x - mine.x).abs() < 1.0 && (r.w - mine.w).abs() < 1.0)
            .filter_map(|&(p, _)| Some(natural_right(self.freq_plot(p)?, &self.own_view(p))))
            .fold(0.0, f32::max)
    }

    /// [`Self::view_for`] before the column's margin.
    fn own_view(&self, id: PaneId) -> ViewState {
        let mut v = self.view;
        self.pane_levels(id).apply(&mut v);
        let modes = self.pane_modes(id);
        v.spectrum.mode = modes.spectrum;
        v.spectrum.follow_shade = self.follow_shade(id);
        v.ir.mode = match self.layout.kind(id) {
            PaneKind::Distortion => modes.ir(IrPane::Sweep),
            _ => modes.ir(IrPane::Live),
        };
        v.chrome = modes.chrome;
        v.distortion.reference = modes.sweep_reference;
        v.plates = self.layout.view(id).map(|v| v.plates).unwrap_or_default();
        v.plate_hover = self
            .pane_local
            .plate_hover
            .filter(|(p, _)| *p == id)
            .map(|(_, h)| h);
        // How the picked curve's peaks show is the spectrum pane's to say (its scene input).
        v.peak_points = self
            .peak_points_of(id)
            .map(|key| ac2_scene::view::PeakPoints {
                key,
                list: false,
                named: false,
            });
        // The transfer scene lays out the plots it is asked for: one alone fills the pane.
        // The coherence rides on the magnitude view only when placed over it.
        let over =
            v.tf.coherence_placement == ac2_scene::view::CoherencePlacement::OverlayOnMagnitude;
        let shown = match modes.transfer {
            TransferView::Ir(_) | TransferView::Alignment => None,
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
        self.lead(kind)
            .and_then(|id| self.layout.view(id))
            .map_or(self.modes(), |v| v.modes)
    }

    pub(in crate::state) fn modes_mut(&mut self) -> &mut PaneModes {
        &mut self.layout.focused_mut().modes
    }

    /// Some spectrum pane shows the spectrograph: its history is kept.
    pub fn spectrograph_shown(&self) -> bool {
        self.layout.views.values().any(|v| {
            self.ours(v) && v.kind == PaneKind::Spectrum && v.modes.spectrum.spectrograph()
        })
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
    pub(in crate::state) fn laid_out_rect(&self, p: PaneId) -> PaneRect {
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
    pub(in crate::state) fn apply_template(
        &mut self,
        t: PaneTemplate,
        p: PaneId,
        meas: MeasId,
    ) -> bool {
        let Some(base) = self.layout.view(p).copied() else {
            return false;
        };
        let rect = self.laid_out_rect(p);
        let least = t.panes().iter().map(|x| x.2).fold(f32::INFINITY, f32::min);
        let total: f32 = t.panes().iter().map(|x| x.2).sum();
        if rect.h * least / total < MIN_RATIO * self.pane_area.1 {
            let close = self.keys.reach(CommandId::ClosePane);
            self.warn(format!(
                "the pane is too small for the {}: {close} closes one to make room",
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
        self.rig.pending.pane_meas.remove(&p);
        self.focus_pane(p);
        true
    }

    /// The transfer stack laid out for the focused transfer pane's measurement; on an empty
    /// pane, for the selected transfer measurement. A pane showing something else keeps it:
    /// what a pane shows changes by a row dropped on it.
    pub(in crate::state) fn transfer_stack(&mut self) {
        let f = self.layout.focus;
        let meas = if self.layout.kind(f) == PaneKind::Transfer {
            self.pane_meas(f)
        } else if self.layout.view(f).is_some_and(|v| !v.has_content()) {
            self.rig.selected.and_then(|id| self.meas(id))
        } else {
            self.warn(
                "the stack lays out a transfer pane: focus one, or drag a transfer row onto a pane",
            );
            return;
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

    /// A transfer measurement this app created, once listed: the pane its screen opened with
    /// (else a pane of its own, [`Self::own_pane_for_new`]) holds the transfer stack (or,
    /// too small for one, the transfer view alone).
    pub(in crate::state) fn stack_created(&mut self, meas: MeasId) {
        let shown = self
            .layout
            .lead_of(PaneKind::Transfer, |v| v.meas == Some(meas));
        if let Some(p) = shown.or_else(|| self.own_pane_for_new(meas, PaneKind::Transfer)) {
            self.apply_template(PaneTemplate::TransferStack, p, meas);
        }
    }

    /// N: the focused pane split in two along its longer side; the new half shows the
    /// same and has the focus. Maximised, the split shows at once: the layout goes back to
    /// all panes.
    pub(in crate::state) fn split_pane(&mut self) {
        if self.split_focused_pane().is_none() {
            let close = self.keys.reach(CommandId::ClosePane);
            self.warn(format!(
                "the pane is too small to split: {close} closes one to make room"
            ));
        }
    }

    /// N's split: the new pane, or `None` when a half would be smaller than resizing keeps
    /// a pane.
    pub(in crate::state) fn split_focused_pane(&mut self) -> Option<PaneId> {
        let rect = self.focused_rect();
        let axis = if rect.w >= rect.h {
            Axis::Row
        } else {
            Axis::Column
        };
        self.split_pane_at(self.layout.focus, axis, Side::B)
    }

    /// A pane of its own for something new of the rig in `rig` (a measurement made, meters
    /// opened): the focused pane when it shows nothing (an empty pane is room, no split
    /// needed), else N's split of the focused pane, the new half set on that rig with
    /// nothing chosen yet, whichever rig the focused pane shows. Never a pane that shows
    /// something; `None`: no room to split.
    pub(in crate::state) fn split_for_new(&mut self) -> Option<PaneId> {
        let f = self.layout.focus;
        let waits = self.rig.pending.pane_meas.contains_key(&f);
        let empty = self
            .layout
            .view(f)
            .is_some_and(|v| !v.has_content() && self.ours(v));
        if empty && !waits && self.others.visiting.is_none() {
            return Some(f);
        }
        let new = self.split_focused_pane()?;
        let here = self.here();
        if let Some(v) = self.layout.view_mut(new)
            && v.rig != here
        {
            v.rig = here;
            v.meas = None;
            v.with.clear();
            v.meters = None;
        }
        Some(new)
    }

    /// A measurement this app created, on a pane of its own ([`Self::split_for_new`]) of
    /// `kind`, focused: the pane. No room to split: no pane changes and a warning says how
    /// to show it.
    pub(in crate::state) fn own_pane_for_new(
        &mut self,
        meas: MeasId,
        kind: PaneKind,
    ) -> Option<PaneId> {
        let Some(p) = self.split_for_new() else {
            let name = self
                .meas(meas)
                .map_or_else(String::new, |m| m.config.name.clone());
            self.warn(ac2_scene::pane_edit::no_room_for_pane(&name));
            return None;
        };
        self.set_pane_kind(p, kind);
        if let Some(v) = self.layout.view_mut(p) {
            v.meas = Some(meas);
        }
        self.focus_pane(p);
        Some(p)
    }

    /// Pane `p` split in halves along `axis`, the new half on `side` (`A`: left or top),
    /// showing what `p` does and taking the focus: the new pane, or `None` when a half would
    /// be smaller than resizing keeps a pane.
    pub(in crate::state) fn split_pane_at(
        &mut self,
        p: PaneId,
        axis: Axis,
        side: Side,
    ) -> Option<PaneId> {
        self.layout.view(p)?;
        if !self.can_split(p, axis) {
            return None;
        }
        // The new half starts with the pane's lead alone (what is drawn behind it is the
        // operator's to tick again), or nothing. A lead that waits for the mirror has both
        // halves wait for it.
        let waiting = self.rig.pending.pane_meas.get(&p).cloned();
        self.layout.maximized = false;
        let new = self.layout.split_pane(p, axis, side);
        self.copy_levels(p, new);
        if let Some(v) = self.layout.view_mut(new) {
            let lead = if v.meas.is_none() {
                v.with.as_slice().first().copied()
            } else {
                None
            };
            v.with = With::of(lead.as_slice());
        }
        if let Some(n) = waiting {
            self.rig.pending.pane_meas.insert(new, n);
        }
        Some(new)
    }

    /// A measurement this app created, opened: on a screen of its own
    /// ([`Self::fresh_screen_for_new`]) when any pane shows something or the panes are a
    /// measurement's own; on panes that show nothing and are nobody's (an app just started,
    /// the operator's empty grid) there is nothing of another's to carry, so it takes a pane
    /// of them ([`Self::own_pane_for_new`]; a transfer's, once listed, by its stack).
    pub(in crate::state) fn open_new(
        &mut self,
        meas: MeasId,
        kind: PaneKind,
        view: TransferView,
        stack: bool,
    ) {
        let blank = self.rig.choice.own_panes.is_none()
            && self.layout.views.values().all(|v| !v.has_content());
        if !blank {
            self.fresh_screen_for_new(meas, kind, view);
            return;
        }
        if stack {
            return;
        }
        if let Some(p) = self.own_pane_for_new(meas, kind)
            && let Some(v) = self.layout.view_mut(p)
        {
            v.modes.transfer = view;
        }
    }

    /// The screen a measurement this app created opens on: one pane of `kind` over the
    /// whole area, showing `meas` in transfer view `view`, with the focus. The panes it
    /// replaces stay the layout of the measurement they belonged to (its row brings them
    /// back), so the new one starts with only its own pane, nothing of another's.
    pub(in crate::state) fn fresh_screen_for_new(
        &mut self,
        meas: MeasId,
        kind: PaneKind,
        view: TransferView,
    ) {
        let id = self.layout.fresh_id();
        let mut v = View::of(kind);
        v.meas = Some(meas);
        v.rig = self.here();
        v.modes.transfer = view;
        self.layout
            .install(PaneNode::Leaf(id), BTreeMap::from([(id, v)]), id);
        self.layout.maximized = false;
        self.rig.pending.pane_meas.clear();
        self.focus_pane(id);
    }

    /// Q: the focused pane closed, its neighbour taking its place and the focus; the
    /// last pane stays.
    pub(in crate::state) fn close_pane(&mut self) {
        if !self.layout.close_focused() {
            let split = self.keys.label(CommandId::SplitPane, Scope::Global);
            let text =
                ac2_scene::new_meas::last_pane_stays(split.as_deref(), &self.new_meas_reach());
            self.warn(text);
            return;
        }
        self.rig
            .pending
            .pane_meas
            .retain(|id, _| self.layout.views.contains_key(id));
    }

    /// The panes' area last drawn, without gaps: what resizing keeps every pane readable in.
    fn area(&self) -> PaneRect {
        let (w, h) = self.pane_area;
        PaneRect::new(0.0, 0.0, w, h)
    }

    /// Shift+N: the split that holds the focused pane turns, side by side ↔ stacked, its
    /// ratio and order kept. Maximised, the turn shows at once: the layout goes back to
    /// all panes.
    pub(in crate::state) fn turn_split(&mut self) {
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
                let close = self.keys.reach(CommandId::ClosePane);
                self.warn(format!(
                    "the panes would be too small turned: {close} closes one to make room"
                ));
            }
        }
    }

    /// Alt+Right / Alt+Left: the focused pane grows / shrinks by a tenth of the split
    /// that holds it, never past the size N refuses to split below.
    pub(in crate::state) fn resize_pane(&mut self, grow: bool) {
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
    pub(in crate::state) fn drag_split(&mut self, path: &[Side], ratio: f32) {
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
    pub(in crate::state) fn focus_nth(&mut self, n: usize) {
        let panes = self.layout.panes();
        match panes.get(n.saturating_sub(1)) {
            Some(id) => {
                let id = *id;
                self.focus_pane(id);
            }
            None => {
                let split = self.keys.reach(CommandId::SplitPane);
                self.warn(format!(
                    "no pane {n}: {} on screen ({split} splits)",
                    panes.len()
                ));
            }
        }
    }

    /// Focuses pane `id` from the keyboard: it takes the keys; the selection stays on the
    /// row the operator chose.
    pub(in crate::state) fn focus_pane(&mut self, id: PaneId) {
        self.rig.input_pick = None;
        self.layout.set_focus(id);
    }

    /// Brings up the pane of kind `p` a command is about (the sweep's views, the SPL modes):
    /// the one of that kind on screen worked in last. With none, no pane turns into one:
    /// the hint says which row to drag onto a pane, and `false`.
    pub(in crate::state) fn focus_kind(&mut self, p: PaneKind) -> bool {
        match self.focus_lead(p) {
            true => true,
            false => {
                let name = self
                    .selected_meas()
                    .filter(|m| PaneKind::for_kind(&m.config.kind) == p)
                    .map(|m| m.config.name.clone());
                self.warn(ac2_scene::pane_edit::no_pane_of(p.title(), name.as_deref()));
                false
            }
        }
    }

    /// [`Self::focus_kind`] without the hint: for what acts whether a pane shows it or not.
    pub(in crate::state) fn focus_lead(&mut self, p: PaneKind) -> bool {
        let lead = self.lead(p);
        if let Some(id) = lead {
            self.focus_pane(id);
        }
        lead.is_some()
    }

    /// Pane `id` turns into a pane of kind `kind`, with nothing picked when the kind changes.
    pub(in crate::state) fn set_pane_kind(&mut self, id: PaneId, kind: PaneKind) {
        if let Some(v) = self.layout.view_mut(id) {
            if v.kind != kind {
                v.meas = None;
                v.with.clear();
            }
            if kind != PaneKind::Programme {
                v.meters = None;
            }
            if kind != PaneKind::Walk {
                v.walk = None;
            }
            v.kind = kind;
        }
        self.rig.pending.pane_meas.remove(&id);
        self.rig.pending.deleted.remove(&id);
        // The history goes with the last spectrograph shown.
        if !self.spectrograph_shown() {
            self.rig.folded.spectrographs.clear();
            self.view.spectrum.spectrograph.cursor_s = None;
        }
    }
}
