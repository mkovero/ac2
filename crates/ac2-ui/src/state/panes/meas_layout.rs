//! A measurement's own panes (`meas.layout`): choosing a measurement's row in the list
//! puts its panes on screen; one without keeps the panes there are. Nothing else moves the
//! panes as a whole: a child row (live curve, math channel, stored trace) only selects, a
//! click in a pane only focuses it, and a chip pick or a row dropped on a pane changes that
//! pane alone. Editing the panes (N, Q, a pane's kind or view, T, a split dragged, the
//! transfer stack, a pick, a level axis zoomed or panned) gives the measurement whose row
//! was chosen that layout, and every client that selects it shows the same. A measurement
//! picked into one of its panes is a guest there: the edit is still the chosen one's, never
//! the guest's own layout, and never any other measurement's: two measurements whose
//! layouts happen to be alike are still two layouts. Panes none of which shows the
//! measurement are no layout of its: nothing is sent, and they stop being its own.
//!
//! A selection this app made by itself — the first one, the measurement a remembered pane
//! shows, or a stand-in for one the daemon lost (a restart, a session reopened or salvaged)
//! — never brings a measurement's panes, and an edit of the panes gives it none. Another
//! client's layout for the measurement goes on screen only while its own panes are there
//! because the operator chose its row here (or edited them with it chosen): the kiosk
//! showing a meter keeps its pane whoever lays out that meter elsewhere.
//!
//! The arrangement, what each pane draws, the measurement each pane was picked for and the
//! level ranges the operator set travel; the focus, W and F stay this app's. A layout
//! received is put on screen, never sent back: only what the operator does here is sent.
//!
//! One call per measurement is in flight at a time, the newest edit waiting behind it: each
//! call is a task of its own on the link, so two in flight could reach the daemon in either
//! order and leave it holding an older layout than the screen. A call is done when it is
//! answered and the daemon's copy has shown it: a reply can come before the state events
//! that carry the layouts sent before it, and those older layouts are this app's own.

use ac2_proto::model::PaneLayout;
use ac2_proto::model::layout as wire;

use crate::state::*;

/// The `meas.layout` calls in flight and the edits waiting behind them.
#[derive(Clone, Debug, Default)]
pub(crate) struct LayoutQueue {
    /// `meas.layout` calls not answered yet, oldest first.
    pub(in crate::state) calls: std::collections::VecDeque<LayoutCall>,
    /// Edits waiting for the `meas.layout` call in flight for their measurement: the newest.
    pub(in crate::state) queued: BTreeMap<MeasId, PaneLayout>,
    /// A split dragged: the measurement whose layout takes it, and when (`now_s`) the drag
    /// counts as done.
    pub(in crate::state) due: Option<(MeasId, f64)>,
}

/// A `meas.layout` call not answered yet.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayoutCall {
    meas: MeasId,
    /// The call's `what`: its reply is recognised by it.
    what: String,
    /// The measurement had no layout of its own: the reply says it has one now.
    first: bool,
    /// The layout sent.
    layout: PaneLayout,
    /// The reply came.
    answered: bool,
    /// The daemon's copy has shown this layout or a later one of this app's.
    applied: bool,
}

/// What the panes were before a message: whose own panes they were, that measurement's
/// layout as the daemon had it, and the panes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PanesWatch {
    owner: Option<MeasId>,
    theirs: Option<PaneLayout>,
    panes: PaneLayout,
}

/// A split dragged is sent once it rests this long (s): a drag is many steps.
const DRAG_REST_S: f64 = 0.4;

impl AppState {
    /// The panes on screen as a layout: those of the rig in `rig`, as if the others' were
    /// not there (another rig's panes are never stored in this rig's measurement).
    pub fn pane_layout(&self) -> PaneLayout {
        let own = self.layout.root.pruned(&|p| self.pane_ours(p));
        self.layout_of(own.as_ref().unwrap_or(&self.layout.root))
    }

    fn layout_of(&self, n: &PaneNode) -> PaneLayout {
        match n {
            PaneNode::Leaf(id) => {
                let v = self
                    .layout
                    .view(*id)
                    .copied()
                    .unwrap_or(View::of(PaneKind::Transfer));
                PaneLayout::Pane {
                    pane: Box::new(wire_pane(&v, self.operator_levels(*id))),
                }
            }
            PaneNode::Split { axis, ratio, a, b } => PaneLayout::Split {
                axis: match axis {
                    Axis::Row => wire::SplitAxis::Row,
                    Axis::Column => wire::SplitAxis::Column,
                },
                ratio: f64::from(*ratio),
                a: Box::new(self.layout_of(a)),
                b: Box::new(self.layout_of(b)),
            },
        }
    }

    /// The layout of measurement `id`, as the daemon has it.
    fn meas_layout(&self, id: Option<MeasId>) -> Option<PaneLayout> {
        id.and_then(|id| self.meas(id))
            .and_then(|m| m.layout.clone())
    }

    /// The layout of measurement `id` as this app last has it: the edit waiting to be sent,
    /// else the last one sent that the daemon's copy has not shown yet, else that copy.
    fn own_layout(&self, id: MeasId) -> Option<PaneLayout> {
        self.rig
            .layout_queue
            .queued
            .get(&id)
            .cloned()
            .or_else(|| {
                self.rig
                    .layout_queue
                    .calls
                    .iter()
                    .rev()
                    .find(|c| c.meas == id && !c.applied)
                    .map(|c| c.layout.clone())
            })
            .or_else(|| self.meas_layout(Some(id)))
    }

    pub(in crate::state) fn panes_watch(&self) -> PanesWatch {
        PanesWatch {
            owner: self.rig.own_panes,
            theirs: self.meas_layout(self.rig.own_panes),
            panes: self.pane_layout(),
        }
    }

    /// After a message: the chosen measurement's layout on screen when its row was chosen
    /// or another client changed it; the panes sent as its layout when the operator edited
    /// them (`conn`: the message came from the daemon, which edits nothing; `drag`: a split
    /// being dragged, sent when it rests).
    pub(in crate::state) fn sync_meas_layout(
        &mut self,
        before: PanesWatch,
        conn: bool,
        drag: bool,
        out: &mut Vec<Request>,
    ) {
        self.settle_layout_calls();
        if std::mem::take(&mut self.rig.own_panes_due) {
            self.rig.own_panes = self.rig.selected;
            let own = self.rig.selected.and_then(|id| self.own_layout(id));
            if let Some(l) = own.filter(|l| *l != self.pane_layout()) {
                self.show_layout(&l);
            }
            return;
        }
        // The panes are the own panes of the measurement whose row was chosen here; with
        // none chosen (a stand-in, the measurement a remembered pane shows) they are
        // nobody's. An edit is stored on that one measurement alone.
        let Some(owner) = self.rig.own_panes else {
            return;
        };
        if before.owner != Some(owner) {
            return;
        }
        if !conn && self.pane_layout() != before.panes {
            if drag {
                self.history.stored.insert(owner);
                self.rig.layout_queue.due = Some((owner, self.now_s + DRAG_REST_S));
            } else {
                self.store_layout(owner, out);
            }
            return;
        }
        let theirs = self.meas_layout(Some(owner));
        if theirs == before.theirs {
            return;
        }
        // This app's own, come back: on screen already, or older than the screen.
        if let Some(i) = self
            .rig
            .layout_queue
            .calls
            .iter()
            .position(|c| c.meas == owner && Some(&c.layout) == theirs.as_ref())
        {
            for c in self.rig.layout_queue.calls.iter_mut().take(i + 1) {
                if c.meas == owner {
                    c.applied = true;
                }
            }
            self.rig
                .layout_queue
                .calls
                .retain(|c| !(c.answered && c.applied));
            return;
        }
        // While a call of this app's is unanswered, the daemon's copy is older than the
        // screen: the last call made wins.
        if self.rig.layout_queue.queued.contains_key(&owner)
            || self
                .rig
                .layout_queue
                .calls
                .iter()
                .any(|c| c.meas == owner && !c.answered)
        {
            return;
        }
        // Another client's edit.
        self.rig.layout_queue.calls.retain(|c| c.meas != owner);
        if let Some(l) = theirs.filter(|l| *l != self.pane_layout()) {
            self.show_layout(&l);
        }
    }

    /// Calls of measurements whose panes are not on screen whose layout the daemon's copy
    /// shows (or a later one of this app's) are done once answered; the own panes' are
    /// settled as its copy changes, above.
    fn settle_layout_calls(&mut self) {
        let own = self.rig.own_panes;
        let shown: Vec<(MeasId, usize)> = self
            .rig
            .layout_queue
            .calls
            .iter()
            .enumerate()
            .filter(|(_, c)| Some(c.meas) != own && !c.applied)
            .filter(|(_, c)| self.meas_layout(Some(c.meas)).as_ref() == Some(&c.layout))
            .map(|(i, c)| (c.meas, i))
            .collect();
        if shown.is_empty() {
            return;
        }
        for (meas, i) in shown {
            for c in self.rig.layout_queue.calls.iter_mut().take(i + 1) {
                if c.meas == meas {
                    c.applied = true;
                }
            }
        }
        self.rig
            .layout_queue
            .calls
            .retain(|c| !(c.answered && c.applied));
    }

    /// A dragged split resting long enough: sent.
    pub(in crate::state) fn layout_drag_due(&mut self, out: &mut Vec<Request>) {
        if self
            .rig
            .layout_queue
            .due
            .as_ref()
            .is_some_and(|(_, at)| self.now_s >= *at)
        {
            self.flush_layout_drag(out);
        }
    }

    /// A dragged split sent now (before anything else changes the panes).
    pub(in crate::state) fn flush_layout_drag(&mut self, out: &mut Vec<Request>) {
        if let Some((id, _)) = self.rig.layout_queue.due.take() {
            self.store_layout(id, out);
        }
    }

    /// The panes on screen sent as measurement `id`'s layout, unless it has them already;
    /// held until the call in flight for it is answered.
    /// Panes none of which shows `id` are no layout of its: nothing is sent, and they are
    /// not its own panes any more.
    pub(in crate::state) fn store_layout(&mut self, id: MeasId, out: &mut Vec<Request>) {
        if !self.panes_show(id) {
            if self.rig.own_panes == Some(id) {
                self.rig.own_panes = None;
            }
            return;
        }
        self.history.stored.insert(id);
        let l = self.pane_layout();
        if self
            .rig
            .layout_queue
            .calls
            .iter()
            .any(|c| c.meas == id && !c.answered)
        {
            self.rig.layout_queue.queued.insert(id, l);
            return;
        }
        self.send_layout(id, l, out);
    }

    /// Some pane on screen shows measurement `id`: one of a kind that draws it, showing it.
    fn panes_show(&self, id: MeasId) -> bool {
        self.layout
            .panes()
            .into_iter()
            .any(|p| self.pane_meas(p).is_some_and(|m| m.id == id))
    }

    fn send_layout(&mut self, id: MeasId, l: PaneLayout, out: &mut Vec<Request>) {
        let Some(m) = self.meas(id) else {
            return;
        };
        let last_sent = self
            .rig
            .layout_queue
            .calls
            .iter()
            .rev()
            .find(|c| c.meas == id);
        if last_sent.map_or(m.layout.as_ref(), |c| Some(&c.layout)) == Some(&l) {
            return;
        }
        let first = m.layout.is_none() && last_sent.is_none();
        let what = format!(
            "{}: {}",
            m.config.name,
            ac2_scene::format::own_panes(l.panes().len())
        );
        self.rig.layout_queue.calls.push_back(LayoutCall {
            meas: id,
            what: what.clone(),
            first,
            layout: l.clone(),
            answered: false,
            applied: false,
        });
        self.call(
            out,
            Command::MeasLayout {
                meas: id,
                layout: Some(l),
            },
            what,
        );
    }

    /// A reply to a `meas.layout` call of this app's: `true` when it was one. Said only
    /// when the measurement got its first layout or the call failed: every pane edit
    /// answered aloud would bury what the edit itself said.
    /// The edit held behind the call goes now.
    pub(in crate::state) fn layout_reply(
        &mut self,
        what: &str,
        result: &Result<(), String>,
        out: &mut Vec<Request>,
    ) -> bool {
        let Some(i) = self
            .rig
            .layout_queue
            .calls
            .iter()
            .position(|c| c.what == what && !c.answered)
        else {
            return false;
        };
        let meas = self.rig.layout_queue.calls[i].meas;
        match result {
            Ok(()) => {
                let c = &mut self.rig.layout_queue.calls[i];
                c.answered = true;
                let first = c.first;
                if c.applied {
                    self.rig.layout_queue.calls.remove(i);
                }
                if first {
                    self.toast(what);
                }
            }
            Err(e) => {
                self.rig.layout_queue.calls.remove(i);
                self.fault(format!("{what}: {e}"));
            }
        }
        if let Some(l) = self.rig.layout_queue.queued.remove(&meas) {
            self.send_layout(meas, l, out);
        }
        true
    }

    /// Layout `l` on screen, each pane on the measurement it was picked for (one gone since,
    /// or of a kind the pane no longer draws, leaves it on the chosen measurement) and on the
    /// level ranges the operator set. The focus stays at its place when the pane there is
    /// of the same kind; else it goes to a pane picked for the chosen measurement, else to
    /// a pane that shows it, else to the first of its kind. With no pane showing it, the
    /// focused pane does, as its row puts it there.
    fn show_layout(&mut self, l: &PaneLayout) {
        if l.check().is_err() {
            return;
        }
        let mut views = BTreeMap::new();
        let mut levels = Vec::new();
        let root = self.install_node(l, &mut views, &mut levels);
        for v in views.values_mut() {
            v.meas = v
                .meas
                .filter(|id| self.meas(*id).is_some_and(|m| v.kind.shows(&m.config.kind)));
        }
        let sel = self.rig.chosen.and_then(|id| self.meas(id)).cloned();
        let order = root.reading_order();
        // The operator's eyes stay where they are: the pane at the focused one's place, when
        // it is of the same kind.
        let at = self
            .layout
            .root
            .reading_order()
            .iter()
            .position(|p| *p == self.layout.focus);
        let kept = at
            .and_then(|i| order.get(i))
            .filter(|p| views.get(*p).map(|v: &View| v.kind) == Some(self.layout.focus_kind()))
            .copied();
        let focus = kept
            .or_else(|| {
                sel.as_ref().and_then(|m| {
                    let home = PaneKind::for_kind(&m.config.kind);
                    let find = |f: &dyn Fn(&View) -> bool| {
                        order
                            .iter()
                            .find(|id| views.get(*id).is_some_and(|v: &View| f(v)))
                            .copied()
                    };
                    find(&|v| v.meas == Some(m.id))
                        .or_else(|| find(&|v| v.meas.is_none() && v.kind == home))
                        .or_else(|| find(&|v| v.kind == home))
                        .or_else(|| find(&|v| v.kind.shows(&m.config.kind)))
                })
            })
            .unwrap_or(order[0]);
        if let Some(m) = &sel {
            let home = PaneKind::for_kind(&m.config.kind);
            let shown = views
                .values()
                .any(|v| v.meas == Some(m.id) || (v.meas.is_none() && v.kind == home));
            if !shown
                && let Some(v) = views.get_mut(&focus)
                && v.kind.shows(&m.config.kind)
            {
                v.meas = Some(m.id);
            }
        }
        let (root, views) = self.beside_other_rigs(root, views);
        self.layout.install(root, views, focus);
        self.prune_levels();
        for (id, w) in levels {
            self.set_operator_levels(id, &w);
        }
        self.pane_auto = false;
        self.rig.pending.pane_meas.clear();
        if matches!(self.overlay, Overlay::PaneMenu(m) if !self.layout.views.contains_key(&m.pane))
        {
            self.overlay = Overlay::None;
        }
        // The history goes with the last spectrograph shown.
        if !self.spectrograph_shown() {
            self.rig.spectrographs.clear();
            self.view.spectrum.spectrograph.cursor_s = None;
        }
    }

    /// Panes `root` brought on screen with the other rigs' panes kept where they are: they
    /// take the place of this rig's panes (the focused one's, else the first's), or with
    /// none of this rig's on screen go beside them.
    fn beside_other_rigs(
        &self,
        root: PaneNode,
        mut views: BTreeMap<PaneId, View>,
    ) -> (PaneNode, BTreeMap<PaneId, View>) {
        let (ours, theirs): (Vec<PaneId>, Vec<PaneId>) = self
            .layout
            .root
            .reading_order()
            .into_iter()
            .partition(|p| self.pane_ours(*p));
        if theirs.is_empty() {
            return (root, views);
        }
        let mut tree = self.layout.root.clone();
        let anchor = Some(self.layout.focus)
            .filter(|f| ours.contains(f))
            .or_else(|| ours.first().copied());
        match anchor {
            Some(a) => {
                for p in ours.iter().filter(|p| **p != a) {
                    tree.remove(*p);
                }
                tree.replace_leaf(a, root);
            }
            None => {
                tree = PaneNode::Split {
                    axis: Axis::Row,
                    ratio: 0.5,
                    a: Box::new(tree),
                    b: Box::new(root),
                };
            }
        }
        for p in theirs {
            if let Some(v) = self.layout.view(p) {
                views.insert(p, *v);
            }
        }
        (tree, views)
    }

    fn install_node(
        &mut self,
        l: &PaneLayout,
        views: &mut BTreeMap<PaneId, View>,
        levels: &mut Vec<(PaneId, wire::LayoutLevels)>,
    ) -> PaneNode {
        match l {
            PaneLayout::Pane { pane } => {
                let id = self.layout.fresh_id();
                views.insert(id, ui_view(pane));
                levels.push((id, pane.levels));
                PaneNode::Leaf(id)
            }
            PaneLayout::Split { axis, ratio, a, b } => PaneNode::Split {
                axis: match axis {
                    wire::SplitAxis::Row => Axis::Row,
                    wire::SplitAxis::Column => Axis::Column,
                },
                ratio: *ratio as f32,
                a: Box::new(self.install_node(a, views, levels)),
                b: Box::new(self.install_node(b, views, levels)),
            },
        }
    }
}

fn wire_pane(v: &View, levels: wire::LayoutLevels) -> wire::LayoutPane {
    let m = &v.modes;
    wire::LayoutPane {
        kind: match v.kind {
            PaneKind::Transfer => wire::PaneKind::Transfer,
            PaneKind::Spectrum => wire::PaneKind::Spectrum,
            PaneKind::Spl => wire::PaneKind::Spl,
            PaneKind::Distortion => wire::PaneKind::Distortion,
            PaneKind::Programme => wire::PaneKind::Programme,
        },
        transfer_view: match m.transfer {
            TransferView::Magnitude => wire::TransferView::Magnitude,
            TransferView::Phase => wire::TransferView::Phase,
            TransferView::Coherence => wire::TransferView::Coherence,
            TransferView::Alignment => wire::TransferView::Alignment,
            TransferView::Ir(IrMode::Linear) => wire::TransferView::IrLinear,
            TransferView::Ir(IrMode::Log) => wire::TransferView::IrLog,
            TransferView::Ir(IrMode::Etc) => wire::TransferView::Etc,
        },
        spectrum_view: match m.spectrum {
            SpectrumMode::Spectrum => wire::SpectrumView::Spectrum,
            SpectrumMode::Split => wire::SpectrumView::SpectrumSpectrograph,
            SpectrumMode::Spectrograph => wire::SpectrumView::Spectrograph,
        },
        spl_view: match m.spl {
            SplMode::Meter => wire::SplView::Meter,
            SplMode::Leq => wire::SplView::Leq,
            SplMode::MeterLeq => wire::SplView::MeterLeq,
            SplMode::Bands => wire::SplView::Bands,
        },
        sweep_view: match m.sweep {
            SweepMode::Response => wire::SweepView::Response,
            SweepMode::Ir(IrMode::Linear) => wire::SweepView::IrLinear,
            SweepMode::Ir(IrMode::Log) => wire::SweepView::IrLog,
            SweepMode::Ir(IrMode::Etc) => wire::SweepView::Etc,
            SweepMode::Room => wire::SweepView::Room,
        },
        programme_view: match m.programme {
            ac2_scene::view::ProgrammeMode::Meters => wire::ProgrammeView::Meters,
            ac2_scene::view::ProgrammeMode::Goniometer => wire::ProgrammeView::Goniometer,
        },
        chrome: match m.chrome {
            PlotChrome::Full => wire::PlotChrome::Full,
            PlotChrome::NoGrid => wire::PlotChrome::NoGrid,
            PlotChrome::Bare => wire::PlotChrome::TracesOnly,
        },
        content: match v.meters {
            Some(m) => Some(m.wire()),
            None => v.meas.map(|meas| wire::PaneContent::Meas { meas }),
        },
        levels,
        plates: super::plates::wire_plates(&v.plates),
        // The one rig the app holds is the one whose measurement this layout is.
        rig: wire::RigRef::ThisRig,
    }
}

fn ui_view(p: &wire::LayoutPane) -> View {
    View {
        kind: match p.kind {
            wire::PaneKind::Transfer => PaneKind::Transfer,
            wire::PaneKind::Spectrum => PaneKind::Spectrum,
            wire::PaneKind::Spl => PaneKind::Spl,
            wire::PaneKind::Distortion => PaneKind::Distortion,
            wire::PaneKind::Programme => PaneKind::Programme,
        },
        meas: p.content.as_ref().and_then(wire::PaneContent::meas),
        meters: p
            .content
            .as_ref()
            .and_then(crate::state::Metered::from_wire),
        plates: super::plates::ui_plates(&p.plates),
        // The rig whose measurement's layout this is: the one the reducer runs for.
        rig: PaneRig::Active,
        modes: PaneModes {
            transfer: match p.transfer_view {
                wire::TransferView::Magnitude => TransferView::Magnitude,
                wire::TransferView::Phase => TransferView::Phase,
                wire::TransferView::Coherence => TransferView::Coherence,
                wire::TransferView::Alignment => TransferView::Alignment,
                wire::TransferView::IrLinear => TransferView::Ir(IrMode::Linear),
                wire::TransferView::IrLog => TransferView::Ir(IrMode::Log),
                wire::TransferView::Etc => TransferView::Ir(IrMode::Etc),
            },
            spectrum: match p.spectrum_view {
                wire::SpectrumView::Spectrum => SpectrumMode::Spectrum,
                wire::SpectrumView::SpectrumSpectrograph => SpectrumMode::Split,
                wire::SpectrumView::Spectrograph => SpectrumMode::Spectrograph,
            },
            spl: match p.spl_view {
                wire::SplView::Meter => SplMode::Meter,
                wire::SplView::Leq => SplMode::Leq,
                wire::SplView::MeterLeq => SplMode::MeterLeq,
                wire::SplView::Bands => SplMode::Bands,
            },
            sweep: match p.sweep_view {
                wire::SweepView::Response => SweepMode::Response,
                wire::SweepView::IrLinear => SweepMode::Ir(IrMode::Linear),
                wire::SweepView::IrLog => SweepMode::Ir(IrMode::Log),
                wire::SweepView::Etc => SweepMode::Ir(IrMode::Etc),
                wire::SweepView::Room => SweepMode::Room,
            },
            programme: match p.programme_view {
                wire::ProgrammeView::Meters => ac2_scene::view::ProgrammeMode::Meters,
                wire::ProgrammeView::Goniometer => ac2_scene::view::ProgrammeMode::Goniometer,
            },
            // Context, not part of what the measurement's layout shows: as a new pane.
            sweep_reference: true,
            chrome: match p.chrome {
                wire::PlotChrome::Full => PlotChrome::Full,
                wire::PlotChrome::NoGrid => PlotChrome::NoGrid,
                wire::PlotChrome::TracesOnly => PlotChrome::Bare,
            },
        },
    }
}

impl RigState {
    /// The link went down or came up: no answer to an earlier call comes.
    pub(in crate::state) fn forget_layout_calls(&mut self) {
        self.layout_queue.calls.clear();
        self.layout_queue.queued.clear();
        self.layout_queue.due = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Metered;
    use ac2_scene::view::ProgrammeMode;

    /// A pair's goniometer pane goes out as such and comes back as one.
    #[test]
    fn a_goniometer_pane_round_trips() {
        let mut v = View::of(PaneKind::Programme);
        v.meters = Some(Metered::Pair { left: 2, right: 3 });
        v.modes.programme = ProgrammeMode::Goniometer;
        let p = wire_pane(&v, wire::LayoutLevels::default());
        assert_eq!(p.programme_view, wire::ProgrammeView::Goniometer);
        assert_eq!(ui_view(&p), v);
    }
}
