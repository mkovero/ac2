//! The pane tree: the operator splits the screen into panes (N), closes them (Q) and picks
//! what each one shows (Tab and the list put a measurement in the focused pane, the title
//! chip, G for the pane's views). A pane's content is a [`View`]: a kind, the measurement it
//! shows and the kind's display modes. The layout on screen is kept in `ui.toml`; its
//! arrangement and views are also a measurement's own when edited while it is selected
//! (`state/panes/meas_layout.rs`).

pub(super) mod drag;
pub(super) mod levels;
pub(super) mod meas_display;
pub(super) mod meas_layout;
pub(super) mod ops;
pub(super) mod pane_export;
pub(super) mod plates;
pub(super) mod views;

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
pub(super) const MIN_RATIO: f32 = 0.05;

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

    pub(super) fn at_mut(&mut self, path: &[Side]) -> Option<&mut PaneNode> {
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
        self.split_side(id, axis, new, Side::B)
    }

    /// Splits leaf `id` along `axis` in halves, `new` on `side` (`A`: left or top).
    /// `false`: no such leaf.
    pub fn split_side(&mut self, id: PaneId, axis: Axis, new: PaneId, side: Side) -> bool {
        match self {
            PaneNode::Leaf(x) if *x == id => {
                let (a, b) = match side {
                    Side::A => (new, id),
                    Side::B => (id, new),
                };
                *self = PaneNode::Split {
                    axis,
                    ratio: 0.5,
                    a: Box::new(PaneNode::Leaf(a)),
                    b: Box::new(PaneNode::Leaf(b)),
                };
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { a, b, .. } => {
                a.split_side(id, axis, new, side) || b.split_side(id, axis, new, side)
            }
        }
    }

    /// The pane right above leaf `id` in its column: the nearest split stacking `id`'s side
    /// under another, when that other side is one pane. `None` past a side-by-side split
    /// (what is above is not the whole width of `id`'s column) or at the top.
    pub fn above(&self, id: PaneId) -> Option<PaneId> {
        let (mut path, mut side) = self.parent_of(id)?;
        loop {
            let PaneNode::Split { axis, a, .. } = self.at(&path)? else {
                return None;
            };
            match (axis, side) {
                (Axis::Row, _) => return None,
                (Axis::Column, Side::B) => {
                    return match **a {
                        PaneNode::Leaf(p) => Some(p),
                        PaneNode::Split { .. } => None,
                    };
                }
                (Axis::Column, Side::A) => {
                    side = path.pop()?;
                }
            }
        }
    }

    /// Leaf `id` replaced by `new`. `false`: no such leaf.
    pub(super) fn replace(&mut self, id: PaneId, new: PaneNode) -> bool {
        match self {
            PaneNode::Leaf(x) if *x == id => {
                *self = new;
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { a, b, .. } => {
                if a.contains(id) {
                    a.replace(id, new)
                } else {
                    b.replace(id, new)
                }
            }
        }
    }

    /// The tree with only the leaves `keep` takes, each split with one side gone replaced
    /// by the other side (`None`: no leaf kept).
    pub fn pruned(&self, keep: &dyn Fn(PaneId) -> bool) -> Option<PaneNode> {
        match self {
            PaneNode::Leaf(id) => keep(*id).then_some(PaneNode::Leaf(*id)),
            PaneNode::Split { axis, ratio, a, b } => match (a.pruned(keep), b.pruned(keep)) {
                (Some(a), Some(b)) => Some(PaneNode::Split {
                    axis: *axis,
                    ratio: *ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (one, None) | (None, one) => one,
            },
        }
    }

    /// Leaf `id` replaced by `with`; false when there is no such leaf.
    pub fn replace_leaf(&mut self, id: PaneId, with: PaneNode) -> bool {
        match self {
            PaneNode::Leaf(l) if *l == id => {
                *self = with;
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { a, b, .. } => {
                if a.contains(id) {
                    a.replace_leaf(id, with)
                } else {
                    b.replace_leaf(id, with)
                }
            }
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
