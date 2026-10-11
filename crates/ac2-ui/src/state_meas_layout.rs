//! A measurement's own panes (`meas.layout`): selecting a measurement that has a layout
//! puts its panes on screen; one without keeps the panes there are. Editing the panes (N,
//! Q, a pane's kind or view, T, a split dragged, the transfer stack) while a measurement is
//! selected gives it that layout, and every client that selects it shows the same.
//!
//! Only the arrangement and what each pane draws travel: the measurement each pane was
//! picked for, the focus, W and F stay this app's. A layout received is put on screen, never
//! sent back: only what the operator does here is sent.

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

    pub(super) fn panes_watch(&self) -> PanesWatch {
        PanesWatch {
            selected: self.selected,
            theirs: self.meas_layout(self.selected),
            panes: self.pane_layout(),
        }
    }

    /// After a message: the selected measurement's layout on screen when the selection
    /// moved to it or another client changed it; the panes sent as its layout when the
    /// operator edited them (`conn`: the message came from the daemon, which edits nothing;
    /// `drag`: a split being dragged, sent when it rests).
    pub(super) fn sync_meas_layout(
        &mut self,
        before: PanesWatch,
        conn: bool,
        drag: bool,
        out: &mut Vec<Request>,
    ) {
        let theirs = self.meas_layout(self.selected);
        if self.selected != before.selected {
            if let Some(l) = theirs.filter(|l| *l != self.pane_layout()) {
                self.show_layout(&l);
            }
            return;
        }
        let Some(sel) = self.selected else {
            return;
        };
        if !conn && self.pane_layout() != before.panes {
            if drag {
                self.layout_due = Some((sel, self.now_s + DRAG_REST_S));
            } else {
                self.store_layout(sel, out);
            }
            return;
        }
        // Another client's edit (this app's own, come back, is on screen already). While a call of this app's for it is unanswered, the
        // daemon's copy is older than the screen: the last call made wins.
        if theirs != before.theirs
            && !self.layout_calls.iter().any(|c| c.meas == sel)
            && let Some(l) = theirs.filter(|l| *l != self.pane_layout())
        {
            self.show_layout(&l);
        }
    }

    /// A dragged split resting long enough: sent.
    pub(super) fn layout_drag_due(&mut self, out: &mut Vec<Request>) {
        if self.layout_due.is_some_and(|(_, at)| self.now_s >= at) {
            self.flush_layout_drag(out);
        }
    }

    /// A dragged split sent now (before anything else changes the panes).
    pub(super) fn flush_layout_drag(&mut self, out: &mut Vec<Request>) {
        if let Some((meas, _)) = self.layout_due.take() {
            self.store_layout(meas, out);
        }
    }

    /// The panes on screen sent as measurement `id`'s layout, unless it has them already.
    fn store_layout(&mut self, id: MeasId, out: &mut Vec<Request>) {
        let l = self.pane_layout();
        let Some(m) = self.meas(id) else {
            return;
        };
        if m.layout.as_ref() == Some(&l) {
            return;
        }
        let first = m.layout.is_none();
        let what = format!(
            "{}: {}",
            m.config.name,
            ac2_scene::format::own_panes(l.panes().len())
        );
        self.layout_calls.push_back(LayoutCall {
            meas: id,
            what: what.clone(),
            first,
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
    pub(super) fn layout_reply(&mut self, what: &str, result: &Result<(), String>) -> bool {
        let Some(i) = self.layout_calls.iter().position(|c| c.what == what) else {
            return false;
        };
        let call = self.layout_calls.remove(i).expect("found");
        match result {
            Ok(()) if call.first => self.toast(what),
            Ok(()) => {}
            Err(e) => self.fault(format!("{what}: {e}")),
        }
        true
    }

    /// The link went down or came up: no answer to an earlier call comes.
    pub(super) fn forget_layout_calls(&mut self) {
        self.layout_calls.clear();
        self.layout_due = None;
    }

    /// Layout `l` on screen. Every pane follows the selection; the focus goes to the first
    /// pane that shows the selected measurement, which it then shows.
    fn show_layout(&mut self, l: &PaneLayout) {
        if l.check().is_err() {
            return;
        }
        let mut views = BTreeMap::new();
        let root = self.install_node(l, &mut views);
        let sel = self.selected.and_then(|id| self.meas(id)).cloned();
        let order = root.reading_order();
        let focus = sel
            .as_ref()
            .and_then(|m| {
                let home = PaneKind::for_kind(&m.config.kind);
                order
                    .iter()
                    .find(|id| views.get(*id).is_some_and(|v: &View| v.kind == home))
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
