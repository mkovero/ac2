//! A measurement's own panes (`meas.layout`): choosing a measurement's row in the list
//! puts its panes on screen; one without keeps the panes there are. Anything else that
//! moves the selection — a pane clicked or picked from its chip, a child row (live curve,
//! math channel, stored trace) — keeps the panes: it is about one pane. Editing the panes
//! (N, Q, a pane's kind or view, T, a split dragged, the transfer stack) while a
//! measurement is selected gives it that layout, and every client that selects it shows the
//! same. Measurements whose own panes are the ones on screen share them: an edit goes to
//! each, so two measurements laid out side by side keep the arrangement whichever of them
//! is selected.
//!
//! Only the arrangement and what each pane draws travel: the measurement each pane was
//! picked for, the focus, W and F stay this app's. A layout received is put on screen, never
//! sent back: only what the operator does here is sent.
//!
//! One call per measurement is in flight at a time, the newest edit waiting behind it: each
//! call is a task of its own on the link, so two in flight could reach the daemon in either
//! order and leave it holding an older layout than the screen. A call is done when it is
//! answered and the daemon's copy has shown it: a reply can come before the state events
//! that carry the layouts sent before it, and those older layouts are this app's own.

use ac2_proto::model::PaneLayout;
use ac2_proto::model::layout as wire;

use super::*;

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

/// What the panes were before a message: whether it changed the selection, the selected
/// measurement's layout or the panes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PanesWatch {
    selected: Option<MeasId>,
    theirs: Option<PaneLayout>,
    panes: PaneLayout,
}

/// A split dragged is sent once it rests this long (s): a drag is many steps.
const DRAG_REST_S: f64 = 0.4;

impl AppState {
    /// The panes on screen as a layout.
    pub fn pane_layout(&self) -> PaneLayout {
        self.layout_of(&self.layout.root)
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
                    pane: wire_pane(&v),
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
        self.layout_queued
            .get(&id)
            .cloned()
            .or_else(|| {
                self.layout_calls
                    .iter()
                    .rev()
                    .find(|c| c.meas == id && !c.applied)
                    .map(|c| c.layout.clone())
            })
            .or_else(|| self.meas_layout(Some(id)))
    }

    pub(super) fn panes_watch(&self) -> PanesWatch {
        PanesWatch {
            selected: self.selected,
            theirs: self.meas_layout(self.selected),
            panes: self.pane_layout(),
        }
    }

    /// After a message: the selected measurement's layout on screen when its row was chosen
    /// or another client changed it; the panes sent as its layout when the operator edited
    /// them (`conn`: the message came from the daemon, which edits nothing; `drag`: a split
    /// being dragged, sent when it rests).
    pub(super) fn sync_meas_layout(
        &mut self,
        before: PanesWatch,
        conn: bool,
        drag: bool,
        out: &mut Vec<Request>,
    ) {
        self.settle_layout_calls();
        if std::mem::take(&mut self.own_panes_due) {
            if let Some(l) = self
                .selected
                .and_then(|id| self.own_layout(id))
                .filter(|l| *l != self.pane_layout())
            {
                self.show_layout(&l);
            }
            return;
        }
        // A selection moved by a pane (clicked, picked) or a child row: about that pane, and
        // a pane it turned into another kind is no edit.
        if self.selected != before.selected {
            return;
        }
        let theirs = self.meas_layout(self.selected);
        let Some(sel) = self.selected else {
            return;
        };
        if !conn && self.pane_layout() != before.panes {
            // Those whose own panes were the ones on screen share them: the edit is theirs too.
            let mut ids = vec![sel];
            ids.extend(
                self.measurements().iter().map(|m| m.id).filter(|id| {
                    *id != sel && self.own_layout(*id).as_ref() == Some(&before.panes)
                }),
            );
            if drag {
                // A drag is many steps: those sharing the panes before its first one.
                let ids = self.layout_due.take().map_or(ids, |(ids, _)| ids);
                self.layout_due = Some((ids, self.now_s + DRAG_REST_S));
            } else {
                for id in ids {
                    self.store_layout(id, out);
                }
            }
            return;
        }
        if theirs == before.theirs {
            return;
        }
        // This app's own, come back: on screen already, or older than the screen.
        if let Some(i) = self
            .layout_calls
            .iter()
            .position(|c| c.meas == sel && Some(&c.layout) == theirs.as_ref())
        {
            for c in self.layout_calls.iter_mut().take(i + 1) {
                if c.meas == sel {
                    c.applied = true;
                }
            }
            self.layout_calls.retain(|c| !(c.answered && c.applied));
            return;
        }
        // While a call of this app's is unanswered, the daemon's copy is older than the
        // screen: the last call made wins.
        if self.layout_queued.contains_key(&sel)
            || self
                .layout_calls
                .iter()
                .any(|c| c.meas == sel && !c.answered)
        {
            return;
        }
        // Another client's edit.
        self.layout_calls.retain(|c| c.meas != sel);
        if let Some(l) = theirs.filter(|l| *l != self.pane_layout()) {
            self.show_layout(&l);
        }
    }

    /// Calls of measurements not selected whose layout the daemon's copy shows (or a later
    /// one of this app's) are done once answered; the selected one's are settled as its
    /// copy changes, above.
    fn settle_layout_calls(&mut self) {
        let sel = self.selected;
        let shown: Vec<(MeasId, usize)> = self
            .layout_calls
            .iter()
            .enumerate()
            .filter(|(_, c)| Some(c.meas) != sel && !c.applied)
            .filter(|(_, c)| self.meas_layout(Some(c.meas)).as_ref() == Some(&c.layout))
            .map(|(i, c)| (c.meas, i))
            .collect();
        if shown.is_empty() {
            return;
        }
        for (meas, i) in shown {
            for c in self.layout_calls.iter_mut().take(i + 1) {
                if c.meas == meas {
                    c.applied = true;
                }
            }
        }
        self.layout_calls.retain(|c| !(c.answered && c.applied));
    }

    /// A dragged split resting long enough: sent.
    pub(super) fn layout_drag_due(&mut self, out: &mut Vec<Request>) {
        if self
            .layout_due
            .as_ref()
            .is_some_and(|(_, at)| self.now_s >= *at)
        {
            self.flush_layout_drag(out);
        }
    }

    /// A dragged split sent now (before anything else changes the panes).
    pub(super) fn flush_layout_drag(&mut self, out: &mut Vec<Request>) {
        if let Some((ids, _)) = self.layout_due.take() {
            for id in ids {
                self.store_layout(id, out);
            }
        }
    }

    /// The panes on screen sent as measurement `id`'s layout, unless it has them already;
    /// held until the call in flight for it is answered.
    fn store_layout(&mut self, id: MeasId, out: &mut Vec<Request>) {
        let l = self.pane_layout();
        if self
            .layout_calls
            .iter()
            .any(|c| c.meas == id && !c.answered)
        {
            self.layout_queued.insert(id, l);
            return;
        }
        self.send_layout(id, l, out);
    }

    fn send_layout(&mut self, id: MeasId, l: PaneLayout, out: &mut Vec<Request>) {
        let Some(m) = self.meas(id) else {
            return;
        };
        let last_sent = self.layout_calls.iter().rev().find(|c| c.meas == id);
        if last_sent.map_or(m.layout.as_ref(), |c| Some(&c.layout)) == Some(&l) {
            return;
        }
        let first = m.layout.is_none() && last_sent.is_none();
        let what = format!(
            "{}: {}",
            m.config.name,
            ac2_scene::format::own_panes(l.panes().len())
        );
        self.layout_calls.push_back(LayoutCall {
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
    pub(super) fn layout_reply(
        &mut self,
        what: &str,
        result: &Result<(), String>,
        out: &mut Vec<Request>,
    ) -> bool {
        let Some(i) = self
            .layout_calls
            .iter()
            .position(|c| c.what == what && !c.answered)
        else {
            return false;
        };
        let meas = self.layout_calls[i].meas;
        match result {
            Ok(()) => {
                let c = &mut self.layout_calls[i];
                c.answered = true;
                let first = c.first;
                if c.applied {
                    self.layout_calls.remove(i);
                }
                if first {
                    self.toast(what);
                }
            }
            Err(e) => {
                self.layout_calls.remove(i);
                self.fault(format!("{what}: {e}"));
            }
        }
        if let Some(l) = self.layout_queued.remove(&meas) {
            self.send_layout(meas, l, out);
        }
        true
    }

    /// The link went down or came up: no answer to an earlier call comes.
    pub(super) fn forget_layout_calls(&mut self) {
        self.layout_calls.clear();
        self.layout_queued.clear();
        self.layout_due = None;
    }

    /// Layout `l` on screen. Laid out as the panes on screen (the same splits and kinds,
    /// another view or size), each pane keeps the measurement it was picked for; otherwise
    /// every pane follows the selection. The focus goes to a pane picked for the selected
    /// measurement, else to the first pane that shows it, which it then shows.
    fn show_layout(&mut self, l: &PaneLayout) {
        if l.check().is_err() {
            return;
        }
        let kept: Vec<Option<MeasId>> = if same_shape(l, &self.pane_layout()) {
            self.layout
                .root
                .reading_order()
                .iter()
                .map(|id| self.layout.view(*id).and_then(|v| v.meas))
                .collect()
        } else {
            Vec::new()
        };
        let mut views = BTreeMap::new();
        let root = self.install_node(l, &mut views);
        let sel = self.selected.and_then(|id| self.meas(id)).cloned();
        let order = root.reading_order();
        for (id, meas) in order.iter().zip(kept) {
            if let Some(v) = views.get_mut(id) {
                v.meas = meas;
            }
        }
        let focus = sel
            .as_ref()
            .and_then(|m| {
                let home = PaneKind::for_kind(&m.config.kind);
                order
                    .iter()
                    .find(|id| views.get(*id).is_some_and(|v: &View| v.meas == Some(m.id)))
                    .or_else(|| {
                        order
                            .iter()
                            .find(|id| views.get(*id).is_some_and(|v: &View| v.kind == home))
                    })
                    .or_else(|| {
                        order.iter().find(|id| {
                            views
                                .get(*id)
                                .is_some_and(|v: &View| v.kind.shows(&m.config.kind))
                        })
                    })
            })
            .copied()
            .unwrap_or(order[0]);
        if let Some(m) = &sel
            && let Some(v) = views.get_mut(&focus)
            && v.kind.shows(&m.config.kind)
        {
            v.meas = Some(m.id);
        }
        self.layout.install(root, views, focus);
        self.pane_auto = false;
        self.pending_pane_meas.clear();
        if matches!(self.overlay, Overlay::PaneMenu(m) if !self.layout.views.contains_key(&m.pane))
        {
            self.overlay = Overlay::None;
        }
        // The history goes with the last spectrograph shown.
        if !self.spectrograph_shown() {
            self.spectrographs.clear();
            self.view.spectrum.spectrograph.cursor_s = None;
        }
    }

    fn install_node(&mut self, l: &PaneLayout, views: &mut BTreeMap<PaneId, View>) -> PaneNode {
        match l {
            PaneLayout::Pane { pane } => {
                let id = self.layout.fresh_id();
                views.insert(id, ui_view(pane));
                PaneNode::Leaf(id)
            }
            PaneLayout::Split { axis, ratio, a, b } => PaneNode::Split {
                axis: match axis {
                    wire::SplitAxis::Row => Axis::Row,
                    wire::SplitAxis::Column => Axis::Column,
                },
                ratio: *ratio as f32,
                a: Box::new(self.install_node(a, views)),
                b: Box::new(self.install_node(b, views)),
            },
        }
    }
}

/// Whether `a` and `b` split the same way into panes of the same kinds (views and sizes
/// aside).
fn same_shape(a: &PaneLayout, b: &PaneLayout) -> bool {
    match (a, b) {
        (PaneLayout::Pane { pane: x }, PaneLayout::Pane { pane: y }) => x.kind == y.kind,
        (
            PaneLayout::Split {
                axis: xa,
                a: x1,
                b: x2,
                ..
            },
            PaneLayout::Split {
                axis: ya,
                a: y1,
                b: y2,
                ..
            },
        ) => xa == ya && same_shape(x1, y1) && same_shape(x2, y2),
        _ => false,
    }
}

fn wire_pane(v: &View) -> wire::LayoutPane {
    let m = &v.modes;
    wire::LayoutPane {
        kind: match v.kind {
            PaneKind::Transfer => wire::PaneKind::Transfer,
            PaneKind::Spectrum => wire::PaneKind::Spectrum,
            PaneKind::Spl => wire::PaneKind::Spl,
            PaneKind::Distortion => wire::PaneKind::Distortion,
        },
        transfer_view: match m.transfer {
            TransferView::Magnitude => wire::TransferView::Magnitude,
            TransferView::Phase => wire::TransferView::Phase,
            TransferView::Coherence => wire::TransferView::Coherence,
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
        chrome: match m.chrome {
            PlotChrome::Full => wire::PlotChrome::Full,
            PlotChrome::NoGrid => wire::PlotChrome::NoGrid,
            PlotChrome::Bare => wire::PlotChrome::TracesOnly,
        },
    }
}

fn ui_view(p: &wire::LayoutPane) -> View {
    View {
        kind: match p.kind {
            wire::PaneKind::Transfer => PaneKind::Transfer,
            wire::PaneKind::Spectrum => PaneKind::Spectrum,
            wire::PaneKind::Spl => PaneKind::Spl,
            wire::PaneKind::Distortion => PaneKind::Distortion,
        },
        meas: None,
        modes: PaneModes {
            transfer: match p.transfer_view {
                wire::TransferView::Magnitude => TransferView::Magnitude,
                wire::TransferView::Phase => TransferView::Phase,
                wire::TransferView::Coherence => TransferView::Coherence,
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
            chrome: match p.chrome {
                wire::PlotChrome::Full => PlotChrome::Full,
                wire::PlotChrome::NoGrid => PlotChrome::NoGrid,
                wire::PlotChrome::TracesOnly => PlotChrome::Bare,
            },
        },
    }
}
