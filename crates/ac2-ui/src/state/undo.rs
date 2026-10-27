//! Undo and redo of what the operator does to the panes: drops, splits, closes, picks into
//! a pane, view and chrome steps, resizes, W and F, compare on and off. Each edit keeps the
//! panes as they were before and after it; undo puts the before back, redo the after. What
//! the daemon does (deleting, starting, delays, settings) is not undone here.
//!
//! The history is this app's alone and lives while it runs: [`HISTORY_LEN`] steps, a new
//! edit after an undo dropping the steps that could be redone.
//!
//! A step that was stored as the own layout of the measurement whose row was chosen
//! ([`super::panes::meas_layout`]) is stored again when undone or redone, so the daemon's copy
//! matches the screen; no other measurement's layout is written.

use std::collections::VecDeque;

use ac2_scene::meas_list::TreeKey;
use ac2_scene::pane_edit::{self, DropSpot, PaneEdit, ScreenStep};

use super::panes::drag::DragMsg;
use super::*;

/// Steps kept: enough to walk back through a working session's rearranging, small enough
/// that the copies of the layout cost nothing.
pub const HISTORY_LEN: usize = 50;

/// What undo restores: the panes and how they are shown. What is compared into a
/// measurement's panes is the measurement's, on the daemon, as its smoothing is.
#[derive(Clone, Debug)]
pub(crate) struct PaneSnap {
    layout: Layout,
    fullscreen: bool,
    panes_only: bool,
    /// Whose own panes these were: a measurement row brings its panes, and undoing that
    /// makes the panes before it the earlier owner's again.
    own_panes: Option<MeasId>,
}

/// Equal as the operator sees them and as the next edit stores them.
impl PartialEq for PaneSnap {
    fn eq(&self, o: &Self) -> bool {
        self.same_screen(o) && self.own_panes == o.own_panes
    }
}

impl PaneSnap {
    /// The same on screen (whoever's own panes they are).
    fn same_screen(&self, o: &PaneSnap) -> bool {
        self.layout.same_panes(&o.layout)
            && self.fullscreen == o.fullscreen
            && self.panes_only == o.panes_only
    }

    /// The focused pane's number in reading order, as the panes were.
    fn focus_number(&self) -> usize {
        self.layout
            .panes()
            .iter()
            .position(|p| *p == self.layout.focus)
            .map_or(0, |i| i + 1)
    }
}

#[derive(Clone, Debug)]
struct Step {
    before: PaneSnap,
    after: PaneSnap,
    edit: PaneEdit,
    /// The measurement the edit was stored on as its own layout.
    owner: Option<MeasId>,
}

/// The undo history.
#[derive(Clone, Debug, Default)]
pub(crate) struct History {
    done: VecDeque<Step>,
    undone: Vec<Step>,
    /// A split or a plate being dragged: the panes before its first step, and what the
    /// drag edits. The drag is one step.
    drag: Option<(PaneSnap, PaneEdit)>,
    /// Measurements whose layout the edit under way stored (or will, once a drag rests).
    pub(super) stored: BTreeSet<MeasId>,
}

impl History {
    fn push(&mut self, s: Step) {
        self.undone.clear();
        self.done.push_back(s);
        while self.done.len() > HISTORY_LEN {
            self.done.pop_front();
        }
    }

    /// Steps that undo can take back.
    #[cfg(test)]
    pub(crate) fn undo_len(&self) -> usize {
        self.done.len()
    }

    /// Steps that redo can do again.
    #[cfg(test)]
    pub(crate) fn redo_len(&self) -> usize {
        self.undone.len()
    }
}

/// A message that may edit the panes, as it was before it was handled.
pub(super) struct UndoWatch {
    before: PaneSnap,
    what: Watch,
}

enum Watch {
    /// A key or palette command: which one ran says what it was.
    Command { menu: Option<PaneId> },
    /// A list row chosen (click, drop): `item` went to the focused pane, or to `spot`.
    Row {
        item: String,
        spot: Option<DropSpot>,
    },
    /// A row of pane `p`'s list taken.
    Pick(PaneId),
    /// The programme meter form's Enter: new panes, or (`Some`) that pane's pick.
    MeterForm(Option<PaneId>),
}

impl AppState {
    pub(super) fn pane_snap(&self) -> PaneSnap {
        PaneSnap {
            layout: self.layout.clone(),
            fullscreen: self.fullscreen,
            panes_only: self.panes_only,
            own_panes: self.own_panes,
        }
    }

    /// Before message `msg`: what to compare after it, for one that may be an edit.
    pub(super) fn undo_watch(&mut self, msg: &Msg) -> Option<UndoWatch> {
        let row = |st: &Self, k: TreeKey| st.row_name(k).unwrap_or_default();
        // The form's submit runs no command, so it is told by the form it closes.
        let meter_form = match &self.overlay {
            Overlay::Form(f)
                if matches!(f.kind, FormKind::Programme | FormKind::ProgrammeEdit)
                    && match msg {
                        Msg::Key(c) => c.key == eframe::egui::Key::Enter,
                        Msg::Form(FormMsg::Submit) => true,
                        _ => false,
                    } =>
            {
                Some(f.meter.as_ref().and_then(|m| m.edit))
            }
            _ => None,
        };
        let what = match msg {
            _ if meter_form.is_some() => Watch::MeterForm(meter_form.flatten()),
            Msg::Key(_) | Msg::Command(_) => Watch::Command {
                menu: match self.overlay {
                    Overlay::PaneMenu(m) => Some(m.pane),
                    _ => None,
                },
            },
            Msg::SelectMeas(id) => Watch::Row {
                item: row(self, TreeKey::Meas(*id)),
                spot: None,
            },
            Msg::SelectLive(id) => Watch::Row {
                item: row(self, TreeKey::Live(*id)),
                spot: None,
            },
            Msg::SelectMath(id) => Watch::Row {
                item: row(self, TreeKey::Math(*id)),
                spot: None,
            },
            Msg::SelectTrace(id) => Watch::Row {
                item: row(self, TreeKey::Trace(*id)),
                spot: None,
            },
            Msg::PanePick(p, _) => Watch::Pick(*p),
            Msg::RowDrag(DragMsg::Drop { compare }) => {
                let d = self.row_drag()?;
                let spot = self.drop_spot(d.over?, *compare)?;
                Watch::Row {
                    item: row(self, d.item),
                    spot: Some(spot),
                }
            }
            _ => return None,
        };
        self.history.stored.clear();
        Some(UndoWatch {
            before: self.pane_snap(),
            what,
        })
    }

    /// After the message: a step of the history when it changed the panes.
    pub(super) fn record_edit(&mut self, w: UndoWatch, ran: Option<CommandId>) {
        let after = self.pane_snap();
        if after.same_screen(&w.before) {
            return;
        }
        let Some(edit) = self.edit_of(&w, ran) else {
            return;
        };
        let owner = self.own_panes.filter(|o| self.history.stored.contains(o));
        self.history.push(Step {
            before: w.before,
            after,
            edit,
            owner,
        });
    }

    /// What edit the watched message made, named for its toasts.
    fn edit_of(&self, w: &UndoWatch, ran: Option<CommandId>) -> Option<PaneEdit> {
        use CommandId as C;
        let focus = self.pane_number(self.layout.focus);
        let was = w.before.focus_number();
        let put = |item: String, n: usize| PaneEdit::Put {
            item,
            spot: DropSpot::Pane(n),
        };
        Some(match &w.what {
            Watch::Row { item, spot } => PaneEdit::Put {
                item: item.clone(),
                spot: spot.unwrap_or(DropSpot::Pane(focus)),
            },
            Watch::Pick(p) => self.pick_edit(*p),
            Watch::MeterForm(Some(p)) => self.pick_edit(*p),
            Watch::MeterForm(None) => PaneEdit::Split(was),
            Watch::Command { menu } => match ran {
                None => self.pick_edit((*menu)?),
                Some(c) => match c {
                    C::SplitPane | C::ProgrammePane => PaneEdit::Split(was),
                    C::ClosePane => PaneEdit::Close(was),
                    C::TurnSplit => PaneEdit::Turn(was),
                    C::GrowPane | C::ShrinkPane => PaneEdit::Resize,
                    C::TransferStack => PaneEdit::Stack(was),
                    C::NextView
                    | C::PrevView
                    | C::SweepIr
                    | C::SplShowMeter
                    | C::SplShowLeq
                    | C::SplShowMeterLeq
                    | C::SplShowBands => PaneEdit::View(focus),
                    C::PlotChrome => PaneEdit::Chrome(was),
                    C::MaximisePane => PaneEdit::Screen(match self.layout.maximized {
                        true if self.fullscreen => ScreenStep::FullScreen(focus),
                        true => ScreenStep::Alone(focus),
                        false => ScreenStep::AllPanes,
                    }),
                    C::Fullscreen => PaneEdit::Screen(match (self.fullscreen, self.panes_only) {
                        (false, _) => ScreenStep::Windowed,
                        (true, true) => ScreenStep::PanesOnly,
                        (true, false) => ScreenStep::Window,
                    }),
                    C::NextInTree | C::PrevInTree => {
                        let item = match self.selected_trace {
                            Some(t) => self.row_name(TreeKey::Trace(t)),
                            None => self.selected.and_then(|m| self.row_name(TreeKey::Meas(m))),
                        };
                        put(item.unwrap_or_default(), focus)
                    }
                    _ => return None,
                },
            },
        })
    }

    /// A pick from pane `p`'s list: the measurement it shows now, or its new kind.
    fn pick_edit(&self, p: PaneId) -> PaneEdit {
        let item = match self.layout.view(p).and_then(|v| v.meas) {
            Some(_) => self.pane_meas(p).map(|m| m.config.name.clone()),
            None => None,
        }
        .unwrap_or_else(|| self.layout.kind(p).title().to_owned());
        PaneEdit::Put {
            item,
            spot: DropSpot::Pane(self.pane_number(p)),
        }
    }

    /// The first step of a split or plate dragged (`msg`): the panes before it.
    pub(super) fn start_drag(&mut self, msg: &Msg) {
        if self.history.drag.is_none() {
            let edit = match msg {
                Msg::Plate(
                    PlateMsg::Move { pane, .. }
                    | PlateMsg::Scale { pane, .. }
                    | PlateMsg::Reset { pane, .. },
                ) => PaneEdit::Plates(self.pane_number(*pane)),
                _ => PaneEdit::Resize,
            };
            self.history.stored.clear();
            self.history.drag = Some((self.pane_snap(), edit));
        }
    }

    /// The drag over (released, or something else happened): one step.
    pub(super) fn finish_drag(&mut self) {
        let Some((before, edit)) = self.history.drag.take() else {
            return;
        };
        let after = self.pane_snap();
        if after.same_screen(&before) {
            return;
        }
        let owner = self.own_panes.filter(|o| self.history.stored.contains(o));
        self.history.push(Step {
            before,
            after,
            edit,
            owner,
        });
    }

    /// Ctrl+Z: the last edit taken back.
    pub(super) fn undo(&mut self, out: &mut Vec<Request>) {
        let Some(step) = self.history.done.pop_back() else {
            self.toast(pane_edit::NOTHING_TO_UNDO);
            return;
        };
        self.restore_snap(&step.before, step.owner, out);
        self.toast(pane_edit::undid(&step.edit));
        self.history.undone.push(step);
    }

    /// Ctrl+Shift+Z: the last edit undone made again.
    pub(super) fn redo(&mut self, out: &mut Vec<Request>) {
        let Some(step) = self.history.undone.pop() else {
            self.toast(pane_edit::NOTHING_TO_REDO);
            return;
        };
        self.restore_snap(&step.after, step.owner, out);
        self.toast(pane_edit::redid(&step.edit));
        self.history.done.push_back(step);
    }

    /// Panes `s` back on screen. A measurement deleted since leaves its pane on its kind's
    /// usual choice, as a measurement that goes does. With `owner` the measurement the edit
    /// was stored on, and still the one whose row was chosen, the panes are stored on it
    /// again: the daemon's copy follows the screen.
    fn restore_snap(&mut self, s: &PaneSnap, owner: Option<MeasId>, out: &mut Vec<Request>) {
        self.undoing = true;
        let ids: BTreeSet<MeasId> = self.measurements().iter().map(|m| m.id).collect();
        self.layout.restore(&s.layout);
        for v in self.layout.views.values_mut() {
            if v.meas.is_some_and(|m| !ids.contains(&m)) {
                v.meas = None;
            }
        }
        self.pending
            .pane_meas
            .retain(|p, _| self.layout.views.contains_key(p));
        if matches!(self.overlay, Overlay::PaneMenu(m) if !self.layout.views.contains_key(&m.pane))
        {
            self.overlay = Overlay::None;
        }
        self.fullscreen = s.fullscreen;
        self.panes_only = s.panes_only;
        self.own_panes = s.own_panes.filter(|m| ids.contains(m));
        if !self.spectrograph_shown() {
            self.spectrographs.clear();
            self.view.spectrum.spectrograph.cursor_s = None;
        }
        if let Some(o) = owner.filter(|o| self.own_panes == Some(*o)) {
            self.store_layout(o, out);
        }
    }
}
