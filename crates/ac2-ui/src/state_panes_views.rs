//! What a pane shows: its [`View`] (kind, measurement, display modes), the transfer
//! views and templates, and the [`Layout`] of views over the pane tree.

use super::panes::MIN_RATIO;
use super::*;

/// What a transfer pane draws of its measurement (G steps through them in this order,
/// Shift+G back). Each fills the pane: one quantity read at the full height. Magnitude,
/// phase and coherence read together side by side in panes of their own (the transfer
/// stack, [`PaneTemplate`]). The impulse response's three displays are views of their own,
/// so one key walks from the response to its time domain and back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransferView {
    /// Magnitude, with the coherence over it when that is its placement (Settings).
    #[default]
    Magnitude,
    Phase,
    Coherence,
    /// The impulse response of the measurement the pane shows, drawn as this.
    Ir(IrMode),
}

impl TransferView {
    pub const ALL: [TransferView; 6] = [
        TransferView::Magnitude,
        TransferView::Phase,
        TransferView::Coherence,
        TransferView::Ir(IrMode::Linear),
        TransferView::Ir(IrMode::Log),
        TransferView::Ir(IrMode::Etc),
    ];

    pub fn next(self) -> Self {
        ac2_scene::view::step(&Self::ALL, self, 1)
    }

    pub fn prev(self) -> Self {
        ac2_scene::view::step(&Self::ALL, self, -1)
    }

    /// The impulse-response display, in an IR view.
    pub fn ir(self) -> Option<IrMode> {
        match self {
            TransferView::Ir(m) => Some(m),
            _ => None,
        }
    }

    /// For the toast and the hint: what the pane now draws.
    pub fn label(self) -> &'static str {
        match self {
            TransferView::Magnitude => "magnitude",
            TransferView::Phase => "phase",
            TransferView::Coherence => "coherence",
            TransferView::Ir(IrMode::Linear) => "impulse response",
            TransferView::Ir(IrMode::Log) => "impulse response (log)",
            TransferView::Ir(IrMode::Etc) => "ETC",
        }
    }
}

/// A ready-made arrangement of panes put in place of one, every pane showing one
/// measurement: the panes are ordinary ones after, split, resized and closed as any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneTemplate {
    /// Magnitude over phase over coherence, stacked so the three share the frequency axis
    /// as one reads down a column.
    TransferStack,
}

impl PaneTemplate {
    /// The panes top to bottom: kind, view and share of the height. The magnitude reads
    /// the finest detail, so it takes half.
    pub(super) fn panes(self) -> &'static [(PaneKind, TransferView, f32)] {
        match self {
            PaneTemplate::TransferStack => &[
                (PaneKind::Transfer, TransferView::Magnitude, 0.5),
                (PaneKind::Transfer, TransferView::Phase, 0.25),
                (PaneKind::Transfer, TransferView::Coherence, 0.25),
            ],
        }
    }

    /// For the toast: what was laid out.
    pub fn label(self) -> &'static str {
        match self {
            PaneTemplate::TransferStack => "transfer stack: magnitude / phase / coherence",
        }
    }
}

/// The display modes of a pane, each used while the pane shows its kind: two panes of one
/// kind may show it differently (a spectrum beside its spectrograph, a response beside its
/// impulse response).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneModes {
    pub transfer: TransferView,
    pub spectrum: SpectrumMode,
    pub spl: SplMode,
    pub sweep: SweepMode,
    /// The sweep's reference input's own harmonics behind the measurement's (E).
    pub sweep_reference: bool,
    /// Grid, labels and cursor (T), whatever the pane shows.
    pub chrome: PlotChrome,
}

impl Default for PaneModes {
    fn default() -> Self {
        Self {
            transfer: TransferView::Magnitude,
            spectrum: SpectrumMode::Spectrum,
            spl: SplMode::MeterLeq,
            sweep: SweepMode::Response,
            sweep_reference: true,
            chrome: PlotChrome::Full,
        }
    }
}

impl PaneModes {
    /// How IR picture `p` is drawn: its view's display, linear while the pane shows no IR
    /// (the axes keys act on no picture then).
    pub fn ir(&self, p: IrPane) -> IrMode {
        match p {
            IrPane::Live => self.transfer.ir(),
            IrPane::Sweep => self.sweep.ir(),
        }
        .unwrap_or_default()
    }
}

/// What a pane shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// The kind of plot: the kind of the measurement put in it last (Tab, the list).
    pub kind: PaneKind,
    /// The measurement chosen for it (Tab, the list, the chip's list); `None` follows the
    /// selection.
    pub meas: Option<MeasId>,
    pub modes: PaneModes,
}

impl View {
    pub fn of(kind: PaneKind) -> Self {
        Self {
            kind,
            meas: None,
            modes: PaneModes::default(),
        }
    }

    /// A transfer pane in its IR view.
    pub fn shows_ir(&self) -> bool {
        self.kind == PaneKind::Transfer && self.modes.transfer.ir().is_some()
    }

    /// The pane's name in its title: the IR view names what it draws, the other views the
    /// kind (their plots carry their own titles).
    pub fn title(&self) -> &'static str {
        if self.shows_ir() {
            "Impulse response"
        } else {
            self.kind.title()
        }
    }

    /// The keys that act while this view has the focus: an IR has its own (time axis, IR
    /// mode), the kind's otherwise.
    pub fn scope(&self) -> Scope {
        if self.shows_ir() {
            Scope::Ir
        } else {
            self.kind.scope()
        }
    }
}

/// The panes on screen, what each shows and which has the keyboard.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub root: PaneNode,
    pub views: BTreeMap<PaneId, View>,
    pub focus: PaneId,
    /// Only the focused pane.
    pub maximized: bool,
    /// Panes by when they last had the focus, the focused one first: a command about a kind
    /// acts on the pane of that kind focused last.
    mru: Vec<PaneId>,
    /// The id the next split's pane takes: past every id used this run, so a closed pane's
    /// id never comes back to a new pane (what it showed, kept by id, would follow it).
    next_id: u32,
}

impl Default for Layout {
    fn default() -> Self {
        Self::single(PaneKind::Transfer)
    }
}

impl Layout {
    /// One pane over the whole area.
    pub fn single(kind: PaneKind) -> Self {
        let id = PaneId(1);
        Self {
            root: PaneNode::Leaf(id),
            views: BTreeMap::from([(id, View::of(kind))]),
            focus: id,
            maximized: false,
            mru: vec![id],
            next_id: id.0 + 1,
        }
    }

    /// A layout of `root` with `views` (one per leaf; a leaf without one shows a transfer
    /// pane), the focus on `focus` if it is a leaf, else on the first.
    pub fn of(root: PaneNode, mut views: BTreeMap<PaneId, View>, focus: PaneId) -> Self {
        let leaves = root.leaves();
        views.retain(|id, _| leaves.contains(id));
        for id in &leaves {
            views
                .entry(*id)
                .or_insert_with(|| View::of(PaneKind::Transfer));
        }
        let focus = if leaves.contains(&focus) {
            focus
        } else {
            leaves[0]
        };
        let next_id = leaves.iter().map(|id| id.0).max().unwrap_or(0) + 1;
        Self {
            root,
            views,
            focus,
            maximized: false,
            mru: vec![focus],
            next_id,
        }
    }

    /// The leaves in reading order.
    pub fn panes(&self) -> Vec<PaneId> {
        self.root.reading_order()
    }

    pub fn view(&self, id: PaneId) -> Option<&View> {
        self.views.get(&id)
    }

    pub fn view_mut(&mut self, id: PaneId) -> Option<&mut View> {
        self.views.get_mut(&id)
    }

    /// The kind pane `id` shows (a transfer pane for an id not in the tree).
    pub fn kind(&self, id: PaneId) -> PaneKind {
        self.views.get(&id).map_or(PaneKind::Transfer, |v| v.kind)
    }

    /// The focused pane's kind: the scope of the keys.
    pub fn focus_kind(&self) -> PaneKind {
        self.kind(self.focus)
    }

    /// The focused pane's view.
    pub fn focused(&self) -> View {
        self.views
            .get(&self.focus)
            .copied()
            .unwrap_or(View::of(PaneKind::Transfer))
    }

    pub fn focused_mut(&mut self) -> &mut View {
        let f = self.focus;
        self.views
            .entry(f)
            .or_insert_with(|| View::of(PaneKind::Transfer))
    }

    /// Gives pane `id` the keyboard.
    pub fn set_focus(&mut self, id: PaneId) {
        if !self.views.contains_key(&id) {
            return;
        }
        self.focus = id;
        self.mru.retain(|x| *x != id);
        self.mru.insert(0, id);
    }

    /// The pane of kind `kind` the operator worked in last, else the first in reading order.
    pub fn lead(&self, kind: PaneKind) -> Option<PaneId> {
        self.mru
            .iter()
            .copied()
            .chain(self.panes())
            .find(|id| self.views.get(id).is_some_and(|v| v.kind == kind))
    }

    /// Splits pane `id` in halves along `axis`, the new pane on `side` of it (`A`: left or
    /// top). The new pane shows what `id` does and gets the focus.
    pub fn split_pane(&mut self, id: PaneId, axis: Axis, side: Side) -> PaneId {
        let new = PaneId(self.next_id);
        let Some(view) = self.views.get(&id).copied() else {
            return new;
        };
        if self.root.split_side(id, axis, new, side) {
            self.next_id += 1;
            self.views.insert(new, view);
            self.set_focus(new);
        }
        new
    }

    /// The same panes, focus and views as `o`, whatever ids each has handed out.
    pub(super) fn same_panes(&self, o: &Layout) -> bool {
        self.root == o.root
            && self.views == o.views
            && self.focus == o.focus
            && self.maximized == o.maximized
            && self.mru == o.mru
    }

    /// The panes of `snap` back on screen (an undo or redo). Pane ids stay past every id
    /// used this run, so a pane split later never takes the id of one closed since.
    pub(super) fn restore(&mut self, snap: &Layout) {
        let next_id = self.next_id.max(snap.next_id);
        *self = snap.clone();
        self.next_id = next_id;
    }

    /// Leaf `id` replaced by `views` stacked top to bottom, each taking its share of the
    /// height; the top one keeps `id` (and the focus if it had it). `false`: no such leaf
    /// or no views.
    pub fn stack(&mut self, id: PaneId, views: &[(View, f32)]) -> bool {
        let Some((first, rest)) = views.split_first() else {
            return false;
        };
        if !self.root.contains(id) {
            return false;
        }
        let ids: Vec<PaneId> = std::iter::once(id)
            .chain((0..rest.len() as u32).map(|i| PaneId(self.next_id + i)))
            .collect();
        // Each split gives its top pane that pane's share of what is left below the panes
        // above it.
        fn node(ids: &[PaneId], shares: &[f32]) -> PaneNode {
            match (ids, shares) {
                ([id], _) => PaneNode::Leaf(*id),
                ([id, more @ ..], [w, rest @ ..]) => {
                    let below: f32 = rest.iter().sum();
                    PaneNode::Split {
                        axis: Axis::Column,
                        ratio: (w / (w + below)).clamp(MIN_RATIO, 1.0 - MIN_RATIO),
                        a: Box::new(PaneNode::Leaf(*id)),
                        b: Box::new(node(more, rest)),
                    }
                }
                _ => unreachable!("one share per pane"),
            }
        }
        let shares: Vec<f32> = views.iter().map(|v| v.1).collect();
        if !self.root.replace(id, node(&ids, &shares)) {
            return false;
        }
        self.next_id += rest.len() as u32;
        self.views.insert(id, first.0);
        for (pid, (v, _)) in ids[1..].iter().zip(rest) {
            self.views.insert(*pid, *v);
        }
        self.maximized = false;
        true
    }

    /// An id no pane of this run has had.
    pub fn fresh_id(&mut self) -> PaneId {
        let id = PaneId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Every pane replaced by `root` and its `views` (ids from [`Self::fresh_id`], so
    /// nothing kept by a gone pane's id follows a new one), the focus on `focus`. A
    /// maximised layout stays maximised: that is this screen's, not the panes'.
    pub fn install(&mut self, root: PaneNode, views: BTreeMap<PaneId, View>, focus: PaneId) {
        let leaves = root.leaves();
        let focus = if leaves.contains(&focus) {
            focus
        } else {
            leaves[0]
        };
        self.next_id = self
            .next_id
            .max(leaves.iter().map(|id| id.0).max().unwrap_or(0) + 1);
        self.root = root;
        self.views = views;
        self.focus = focus;
        self.mru = vec![focus];
    }

    /// Closes the focused pane: its sibling takes its place and the focus. `false` on the
    /// last pane, which stays.
    pub fn close_focused(&mut self) -> bool {
        let gone = self.focus;
        let Some(next) = self.root.remove(gone) else {
            return false;
        };
        self.views.remove(&gone);
        self.mru.retain(|x| *x != gone);
        self.set_focus(next);
        true
    }
}

/// The panes' area before the first frame is drawn (the default window's): what a split
/// measures the focused pane's longer side in until the view says.
pub const DEFAULT_PANE_AREA: (f32, f32) = (1280.0, 760.0);

/// A row of a pane's list (its title chip, `PaneMeasurement`): a measurement to show, or
/// another kind of pane to turn into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneMenuRow {
    Meas(MeasId),
    Kind(PaneKind),
}
