//! The pane tree: the operator splits the screen into panes (N), closes them (Q) and picks
//! what each one shows (Tab and the list put a measurement in the focused pane, the title
//! chip, G for the pane's views). A pane's content is a [`View`]: a kind, the measurement it
//! shows and the kind's display modes. All of it is this app's: it lives in `ui.toml`, never
//! on the wire.

use super::*;

/// A pane of the tree. Stable for the pane's life: a split keeps the split pane's id and
/// gives the new one a fresh id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u32);

/// How a split divides its rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Side by side: `a` left, `b` right.
    Row,
    /// Stacked: `a` on top, `b` under it.
    Column,
}

/// A rectangle of the pane area, in its units (screen points when drawn).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl PaneRect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
}

/// The tree: leaves are panes, a split shares its rectangle between two subtrees.
#[derive(Clone, Debug, PartialEq)]
pub enum PaneNode {
    Leaf(PaneId),
    Split {
        axis: Axis,
        /// The share of the rectangle `a` takes, 0 … 1.
        ratio: f32,
        a: Box<PaneNode>,
        b: Box<PaneNode>,
    },
}

/// N refuses a split whose halves would be narrower or lower than this share of the
/// panes' area, and a saved ratio keeps each side of its split at least this share of it:
/// plots past it are too small to read.
const MIN_RATIO: f32 = 0.05;

impl PaneNode {
    /// The leaves, `a` before `b`.
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut v = Vec::new();
        self.collect(&mut v);
        v
    }

    fn collect(&self, v: &mut Vec<PaneId>) {
        match self {
            PaneNode::Leaf(id) => v.push(*id),
            PaneNode::Split { a, b, .. } => {
                a.collect(v);
                b.collect(v);
            }
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            PaneNode::Leaf(x) => *x == id,
            PaneNode::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Each leaf's rectangle inside `area`, `gap` between the two sides of every split.
    pub fn rects(&self, area: PaneRect, gap: f32) -> Vec<(PaneId, PaneRect)> {
        let mut v = Vec::new();
        self.place(area, gap, &mut v);
        v
    }

    /// Gaps across this subtree along `axis` at its widest: what of its extent is not plot.
    fn gaps(&self, axis: Axis) -> u32 {
        match self {
            PaneNode::Leaf(_) => 0,
            PaneNode::Split { axis: s, a, b, .. } if *s == axis => 1 + a.gaps(axis) + b.gaps(axis),
            PaneNode::Split { a, b, .. } => a.gaps(axis).max(b.gaps(axis)),
        }
    }

    /// The ratio shares the extent left after every gap along the axis, so equal ratios
    /// give equal plots however the splits nest (three panes from ratios 1/3 and 1/2).
    fn place(&self, r: PaneRect, gap: f32, v: &mut Vec<(PaneId, PaneRect)>) {
        match self {
            PaneNode::Leaf(id) => v.push((*id, r)),
            PaneNode::Split { a, b, .. } => {
                let (ra, rb) = self.halves(r, gap).expect("a split");
                a.place(ra, gap, v);
                b.place(rb, gap, v);
            }
        }
    }

    /// The rectangles of a split's `a` and `b` inside `r`; `None` for a leaf.
    fn halves(&self, r: PaneRect, gap: f32) -> Option<(PaneRect, PaneRect)> {
        let PaneNode::Split { axis, ratio, a, b } = self else {
            return None;
        };
        let ratio = ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
        let (ga, gb) = (a.gaps(*axis) as f32 * gap, b.gaps(*axis) as f32 * gap);
        let share = |len: f32| {
            let content = (len - gap - ga - gb).max(0.0);
            let la = (content * ratio + ga).min((len - gap).max(0.0));
            (la, (len - gap - la).max(0.0))
        };
        Some(match axis {
            Axis::Row => {
                let (wa, wb) = share(r.w);
                (
                    PaneRect::new(r.x, r.y, wa, r.h),
                    PaneRect::new(r.x + wa + gap, r.y, wb, r.h),
                )
            }
            Axis::Column => {
                let (ha, hb) = share(r.h);
                (
                    PaneRect::new(r.x, r.y, r.w, ha),
                    PaneRect::new(r.x, r.y + ha + gap, r.w, hb),
                )
            }
        })
    }

    /// Every split's gap inside `area`, as [`PaneNode::rects`] lays them out: where the
    /// mouse grabs a split to resize it.
    pub fn split_gaps(&self, area: PaneRect, gap: f32) -> Vec<SplitGap> {
        let mut v = Vec::new();
        self.collect_gaps(area, gap, &mut Vec::new(), &mut v);
        v
    }

    fn collect_gaps(&self, r: PaneRect, gap: f32, path: &mut Vec<Side>, v: &mut Vec<SplitGap>) {
        let PaneNode::Split { axis, a, b, .. } = self else {
            return;
        };
        let (ra, rb) = self.halves(r, gap).expect("a split");
        let ga = a.gaps(*axis) as f32 * gap;
        let gb = b.gaps(*axis) as f32 * gap;
        let (gap_rect, start, len) = match axis {
            Axis::Row => (PaneRect::new(ra.x + ra.w, r.y, gap, r.h), r.x, r.w),
            Axis::Column => (PaneRect::new(r.x, ra.y + ra.h, r.w, gap), r.y, r.h),
        };
        v.push(SplitGap {
            path: path.clone(),
            axis: *axis,
            gap: gap_rect,
            start: start + ga + gap / 2.0,
            content: (len - gap - ga - gb).max(0.0),
        });
        path.push(Side::A);
        a.collect_gaps(ra, gap, path, v);
        path.pop();
        path.push(Side::B);
        b.collect_gaps(rb, gap, path, v);
        path.pop();
    }

    /// The node `path` leads to from here.
    fn at(&self, path: &[Side]) -> Option<&PaneNode> {
        match (path.split_first(), self) {
            (None, n) => Some(n),
            (Some((s, rest)), PaneNode::Split { a, b, .. }) => match s {
                Side::A => a.at(rest),
                Side::B => b.at(rest),
            },
            (Some(_), PaneNode::Leaf(_)) => None,
        }
    }

    fn at_mut(&mut self, path: &[Side]) -> Option<&mut PaneNode> {
        match (path.split_first(), self) {
            (None, n) => Some(n),
            (Some((s, rest)), PaneNode::Split { a, b, .. }) => match s {
                Side::A => a.at_mut(rest),
                Side::B => b.at_mut(rest),
            },
            (Some(_), PaneNode::Leaf(_)) => None,
        }
    }

    /// The path to the split that directly holds leaf `id`, and the side `id` is on;
    /// `None` for a lone leaf or no such leaf.
    pub fn parent_of(&self, id: PaneId) -> Option<(Vec<Side>, Side)> {
        let PaneNode::Split { a, b, .. } = self else {
            return None;
        };
        if **a == PaneNode::Leaf(id) {
            return Some((Vec::new(), Side::A));
        }
        if **b == PaneNode::Leaf(id) {
            return Some((Vec::new(), Side::B));
        }
        for (side, child) in [(Side::A, a), (Side::B, b)] {
            if let Some((mut path, s)) = child.parent_of(id) {
                path.insert(0, side);
                return Some((path, s));
            }
        }
        None
    }

    /// The rectangle of the node at `path` inside `area`, no gaps.
    fn rect_at(&self, path: &[Side], area: PaneRect) -> Option<PaneRect> {
        let Some((s, rest)) = path.split_first() else {
            return Some(area);
        };
        let (ra, rb) = self.halves(area, 0.0)?;
        let PaneNode::Split { a, b, .. } = self else {
            return None;
        };
        match s {
            Side::A => a.rect_at(rest, ra),
            Side::B => b.rect_at(rest, rb),
        }
    }

    /// The ratios the split at `path` may take in `area`: each side at least
    /// [`MIN_RATIO`] of its split, and every pane in it at least that share of the area
    /// along the split's axis, the rule N splits by. Empty (`lo > hi`) when no ratio keeps
    /// both sides that large.
    pub fn ratio_range(&self, path: &[Side], area: PaneRect) -> Option<(f32, f32)> {
        let PaneNode::Split { axis, a, b, .. } = self.at(path)? else {
            return None;
        };
        let r = self.rect_at(path, area)?;
        let along = |p: &PaneRect| match axis {
            Axis::Row => p.w,
            Axis::Column => p.h,
        };
        let (len, extent) = (along(&r), along(&area));
        // Without gaps a subtree scales with its rectangle: its narrowest pane is a fixed
        // fraction of it, laid out once in the unit square.
        let narrowest = |n: &PaneNode| {
            n.rects(PaneRect::new(0.0, 0.0, 1.0, 1.0), 0.0)
                .iter()
                .map(|(_, p)| along(p))
                .fold(1.0_f32, f32::min)
        };
        let need = |n: &PaneNode| {
            let f = narrowest(n) * len;
            if f > 0.0 {
                MIN_RATIO * extent / f
            } else {
                f32::INFINITY
            }
        };
        let lo = need(a).max(MIN_RATIO);
        let hi = (1.0 - need(b)).min(1.0 - MIN_RATIO);
        Some((lo, hi))
    }

    /// The leaves in reading order: top to bottom, then left to right, by their corners in
    /// the unit square (what Alt+1 … 9 and the number in each title count).
    pub fn reading_order(&self) -> Vec<PaneId> {
        let mut r = self.rects(PaneRect::new(0.0, 0.0, 1.0, 1.0), 0.0);
        // Corners of panes on one row are equal up to rounding of the ratios.
        let key = |p: &PaneRect| ((p.y * 1e4).round() as i64, (p.x * 1e4).round() as i64);
        r.sort_by_key(|(_, p)| key(p));
        r.into_iter().map(|(id, _)| id).collect()
    }

    /// Splits leaf `id` along `axis` in halves, `new` the second half. `false`: no such leaf.
    pub fn split(&mut self, id: PaneId, axis: Axis, new: PaneId) -> bool {
        match self {
            PaneNode::Leaf(x) if *x == id => {
                *self = PaneNode::Split {
                    axis,
                    ratio: 0.5,
                    a: Box::new(PaneNode::Leaf(id)),
                    b: Box::new(PaneNode::Leaf(new)),
                };
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { a, b, .. } => a.split(id, axis, new) || b.split(id, axis, new),
        }
    }

    /// Removes leaf `id`: its sibling takes the parent's place. The leaf of that sibling
    /// that bordered `id` along the split (its first when `id` was `a`, its last when `id`
    /// was `b`), which gets the focus; `None` for the last leaf (never removed) or no such
    /// leaf.
    pub fn remove(&mut self, id: PaneId) -> Option<PaneId> {
        let PaneNode::Split { a, b, .. } = self else {
            return None;
        };
        let keep = if **a == PaneNode::Leaf(id) {
            let k = (**b).clone();
            let near = k.leaves().first().copied();
            Some((k, near))
        } else if **b == PaneNode::Leaf(id) {
            let k = (**a).clone();
            let near = k.leaves().last().copied();
            Some((k, near))
        } else {
            None
        };
        match keep {
            Some((k, near)) => {
                *self = k;
                near
            }
            None => a.remove(id).or_else(|| b.remove(id)),
        }
    }
}

/// Which child of a split a step down the tree takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    A,
    B,
}

/// A split's gap as laid out: the strip the mouse drags, and how a pointer position along
/// the split's axis maps back to its ratio.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitGap {
    /// The split, from the root.
    pub path: Vec<Side>,
    pub axis: Axis,
    /// The strip between the split's two sides.
    pub gap: PaneRect,
    /// Where the gap's centre sits at ratio 0, and the extent the ratio shares.
    start: f32,
    content: f32,
}

impl SplitGap {
    /// The ratio that puts the gap's centre at `pos` (x for side by side, y stacked).
    pub fn ratio_at(&self, pos: f32) -> f32 {
        if self.content <= 0.0 {
            return 0.5;
        }
        ((pos - self.start) / self.content).clamp(0.0, 1.0)
    }
}

/// What a transfer pane draws of its measurement (G steps through them in this order).
/// Phase and coherence alone fill the pane: one quantity read at the full height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransferView {
    /// Magnitude, phase and coherence (its placement is Shift+C's).
    #[default]
    Response,
    Phase,
    Coherence,
    /// The impulse response of the measurement the pane shows.
    Ir,
}

impl TransferView {
    pub const ALL: [TransferView; 4] = [
        TransferView::Response,
        TransferView::Phase,
        TransferView::Coherence,
        TransferView::Ir,
    ];

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|v| *v == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// For the toast and the hint: what the pane now draws.
    pub fn label(self) -> &'static str {
        match self {
            TransferView::Response => "response",
            TransferView::Phase => "phase",
            TransferView::Coherence => "coherence",
            TransferView::Ir => "impulse response",
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
    /// The transfer pane's and the sweep pane's IR view.
    pub ir: IrMode,
    pub sweep: SweepMode,
    /// Grid, labels and cursor (T), whatever the pane shows.
    pub chrome: PlotChrome,
}

impl Default for PaneModes {
    fn default() -> Self {
        Self {
            transfer: TransferView::Response,
            spectrum: SpectrumMode::Spectrum,
            spl: SplMode::MeterLeq,
            ir: IrMode::Linear,
            sweep: SweepMode::Response,
            chrome: PlotChrome::Full,
        }
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
        self.kind == PaneKind::Transfer && self.modes.transfer == TransferView::Ir
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

    /// Splits the focused pane (`rect` where it is drawn) along its longer side: a wide
    /// pane side by side, a tall one stacked. The new pane shows what the focused one does
    /// and gets the focus.
    pub fn split_focused(&mut self, rect: PaneRect) -> PaneId {
        let axis = if rect.w >= rect.h {
            Axis::Row
        } else {
            Axis::Column
        };
        let new = PaneId(self.next_id);
        let view = self.focused();
        if self.root.split(self.focus, axis, new) {
            self.next_id += 1;
            self.views.insert(new, view);
            self.set_focus(new);
        }
        new
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

    /// The view state pane `id` is drawn with: the shared one with the pane's own modes.
    pub fn view_for(&self, id: PaneId) -> ViewState {
        let mut v = self.view;
        let modes = self.pane_modes(id);
        v.spectrum.mode = modes.spectrum;
        v.ir.mode = modes.ir;
        v.chrome = modes.chrome;
        // The transfer scene lays out the plots it is asked for: one alone fills the pane.
        let shown = match modes.transfer {
            TransferView::Response | TransferView::Ir => None,
            TransferView::Phase => Some((false, true, false)),
            TransferView::Coherence => Some((false, false, true)),
        };
        if let Some((m, p, c)) = shown {
            (v.tf.show_magnitude, v.tf.show_phase, v.tf.show_coherence) = (m, p, c);
        }
        v
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
        let (w, h) = self.pane_area;
        self.layout
            .root
            .rects(PaneRect::new(0.0, 0.0, w, h), 0.0)
            .into_iter()
            .find(|(id, _)| *id == self.layout.focus)
            .map_or(PaneRect::new(0.0, 0.0, w, h), |(_, r)| r)
    }

    /// N: the focused pane split in two along its longer side; the new half shows the
    /// same and has the focus. Maximised, the split shows at once: the layout goes back to
    /// all panes.
    pub(super) fn split_pane(&mut self) {
        let rect = self.focused_rect();
        let (w, h) = self.pane_area;
        let (side, extent) = if rect.w >= rect.h {
            (rect.w, w)
        } else {
            (rect.h, h)
        };
        if side / 2.0 < MIN_RATIO * extent {
            self.warn("the pane is too small to split: Q closes one to make room");
            return;
        }
        // Both halves keep showing what the pane did, whatever is selected later: an
        // unchosen pane would follow the selection.
        let f = self.layout.focus;
        let shown = self.pane_meas(f).map(|m| m.id);
        if let Some(v) = self.layout.view_mut(f) {
            v.meas = v.meas.or(shown);
        }
        let waiting = self.pending_pane_meas.get(&f).cloned();
        self.layout.maximized = false;
        let new = self.layout.split_focused(rect);
        if let Some(n) = waiting {
            self.pending_pane_meas.insert(new, n);
        }
    }

    /// Q: the focused pane closed, its neighbour taking its place and the focus; the
    /// last pane stays.
    pub(super) fn close_pane(&mut self) {
        let gone = self.layout.focus;
        if !self.layout.close_focused() {
            self.warn("the last pane stays: Tab puts the next measurement in it");
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
            let drawn = match kind {
                PaneKind::Transfer => {
                    self.on_transfer_pane(t, Some(shown))
                        || self.trace_compared_on_transfer(t, Some(shown))
                }
                // The spectrum pane draws every shown stored spectrum and band set.
                PaneKind::Spectrum => t.edit.visible,
                PaneKind::Distortion | PaneKind::Spl => false,
            };
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
    /// a pane can still be readied for a measurement not yet made.
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
        v
    }

    /// A row of pane `id`'s list taken: it shows that measurement (selected), or turns into
    /// that kind of pane.
    pub(super) fn pane_pick(&mut self, id: PaneId, row: PaneMenuRow) {
        if matches!(self.overlay, Overlay::PaneMenu(_)) {
            self.overlay = Overlay::None;
        }
        match row {
            PaneMenuRow::Meas(m) => self.pane_show(id, m),
            PaneMenuRow::Kind(k) => {
                self.set_pane_kind(id, k);
                self.focus_pane(id);
            }
        }
    }
}
