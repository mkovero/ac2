//! Display edits the operator makes while comparing curves: display offsets in steps,
//! showing / hiding and deleting (after a confirmation)
//! the selected measurement or stored trace, and which pane a selection brings up. No
//! measured value changes: an offset is drawn, never applied to the stored columns.

use ac2_proto::Command;
use ac2_proto::model::{
    MeasKind, Measurement, Operand, OwnedTraces, TraceKind, TraceMeta, TraceOwner,
};
use ac2_proto::units::{Db, MeasId, TraceId};
use ac2_scene::format;

use super::{AppState, Overlay, PaneKind, drawn_in, trace_label};
use crate::conn::Request;

/// What a delete confirmation would delete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteTarget {
    Trace(TraceId),
    Meas(MeasId),
    /// Nothing: the window says why the selection cannot go (a math channel computes from
    /// it) and only closes.
    Refused,
}

/// The confirmation before the selected measurement or stored trace is deleted.
#[derive(Clone, Debug, PartialEq)]
pub struct DeletePrompt {
    pub target: DeleteTarget,
    /// How toasts name it: `slot 1 (Main L S1)`, `TF 2`.
    pub label: String,
    pub confirm: ac2_scene::trace_list::DeleteConfirm,
}

/// What a question's answer acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChoicePurpose {
    /// Deleting measurement `meas` that owns traces or math channels: keep, delete, cancel
    /// ([`ac2_scene::meas_list::KEEP`] …).
    DeleteOwned { meas: MeasId },
    /// Filing a stored trace (or a math channel) under the owner of the answer picked.
    Move(MoveWhat),
    /// Unsaved changes before `then` replaces them (New, Open, Quit): Save / Discard /
    /// Cancel ([`ac2_scene::sessions::unsaved_question`]).
    Unsaved {
        then: Vec<crate::sessions::SessionStep>,
    },
    /// Save as onto another existing session: Replace runs `rest` (the save, overwriting).
    Replace {
        rest: Vec<crate::sessions::SessionStep>,
    },
    /// A session of another format version: import its traces.
    Salvage { from: crate::sessions::Source },
    /// New measurement: the answer is a kind, in [`ac2_scene::new_meas::NewKind::ALL`]'s
    /// order; its form opens.
    NewMeasurement,
}

/// What "Move to measurement…" moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveWhat {
    Trace(TraceId),
    Math(MeasId),
}

/// A question with a few answers, keyboard-driven.
#[derive(Clone, Debug, PartialEq)]
pub struct ChoicePrompt {
    pub purpose: ChoicePurpose,
    /// How toasts name what it acts on.
    pub label: String,
    pub title: String,
    pub lines: Vec<String>,
    pub choices: Vec<ac2_scene::meas_list::Choice>,
    /// Where each answer files it (Move only).
    pub owners: Vec<TraceOwner>,
    /// The answer Enter takes.
    pub index: usize,
    pub hint: String,
}

/// What the offset keys change.
enum OffsetTarget {
    Trace(TraceMeta),
    Live(Measurement),
}

/// `+3.0 dB`, `no offset`.
fn offset_words(v: f64) -> String {
    if v == 0.0 {
        "no offset".to_owned()
    } else {
        format!("offset {}", format::db_readout(v))
    }
}

/// A step in whole tenths of a dB: repeated steps never accumulate float error.
fn tenths(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// The pane a stored trace of this kind is drawn in first.
pub(super) fn home_pane(kind: TraceKind) -> PaneKind {
    match kind {
        TraceKind::Spectrum { .. } | TraceKind::Rta { .. } => PaneKind::Spectrum,
        TraceKind::Sweep => PaneKind::Distortion,
        _ => PaneKind::Transfer,
    }
}

impl AppState {
    // ----- display offsets ---------------------------------------------------------------

    /// The selected stored trace (any kind: every curve takes an offset), else the live
    /// measurement of the focused pane (the spectrum pane's when it has focus, else the
    /// transfer pane's). A locked trace refuses, saying so.
    fn offset_target(&mut self) -> Option<OffsetTarget> {
        if let Some(t) = self.selected_trace_meta().cloned() {
            if t.edit.locked {
                self.warn(format!("{} is locked: its offset stays", trace_label(&t)));
                return None;
            }
            return Some(OffsetTarget::Trace(t));
        }
        let pane = match self.layout.focus_kind() {
            PaneKind::Spectrum => PaneKind::Spectrum,
            _ => PaneKind::Transfer,
        };
        match self.kind_meas(pane).cloned() {
            Some(m) => Some(OffsetTarget::Live(m)),
            None => {
                let how = self
                    .keys
                    .label(
                        crate::keys::CommandId::NextTrace,
                        crate::keys::Scope::Global,
                    )
                    .map_or_else(
                        || "select a stored trace with a click in the list".to_owned(),
                        |v| format!("select a stored trace with {v} or a click in the list"),
                    );
                self.warn(format!("no {} measurement to offset ({how})", pane.what()));
                None
            }
        }
    }

    /// Alt+↑/↓ (±1 dB), Alt+Shift+↑/↓ (±3 dB), Alt+Home (`None`: back to 0 dB).
    pub(super) fn step_offset(&mut self, step: Option<f64>, out: &mut Vec<Request>) {
        let Some(target) = self.offset_target() else {
            return;
        };
        let new = |v: f64| step.map_or(0.0, |d| tenths(v + d));
        match target {
            OffsetTarget::Trace(t) => {
                let mut edit = t.edit.clone();
                edit.offset = Db(new(edit.offset.0));
                let what = format!("{}: {}", trace_label(&t), offset_words(edit.offset.0));
                self.call(out, Command::TraceUpdate { trace: t.id, edit }, what);
            }
            OffsetTarget::Live(m) => {
                let e = self.edits.entry(m.id).or_default();
                e.offset_db = new(e.offset_db);
                let v = e.offset_db;
                self.toast(format!("{}: {} (display)", m.config.name, offset_words(v)));
            }
        }
    }

    /// J: the typed offset of the same target as the steps.
    pub(super) fn offset_prompt(&mut self) {
        match self.offset_target() {
            Some(OffsetTarget::Trace(t)) => {
                let text = super::offset_text(t.edit.offset.0);
                self.prompt(super::PromptKind::TraceOffset(t.id), text);
            }
            Some(OffsetTarget::Live(m)) => {
                let text = super::offset_text(self.edit(m.id).offset_db);
                self.prompt(super::PromptKind::Offset(m.id), text);
            }
            None => {}
        }
    }

    /// The typed offset of a live measurement.
    pub(super) fn set_live_offset(&mut self, id: MeasId, v: f64) {
        self.edits.entry(id).or_default().offset_db = v;
        if let Some(name) = self.meas(id).map(|m| m.config.name.clone()) {
            self.toast(format!("{name}: {} (display)", offset_words(v)));
        }
    }

    // ----- the selected curve: show / hide, delete ----------------------------------------

    /// A: shows or hides the selected stored trace (the daemon keeps that), else the selected
    /// measurement's live curves in every pane (this app's display only: it keeps measuring).
    pub(super) fn toggle_selected(&mut self, keymap: &crate::keys::Keymap, out: &mut Vec<Request>) {
        if let Some(id) = self.selected_trace.filter(|_| self.keys_on_trace()) {
            self.toggle_shown(id, out);
            return;
        }
        let Some(id) = self.selected_meas().map(|m| m.id) else {
            self.warn(self.select_first("first"));
            return;
        };
        self.toggle_meas_hidden(id, Some(keymap));
    }

    /// Shows or hides measurement `id`'s live curves (this app's display only).
    pub(super) fn toggle_meas_hidden(&mut self, id: MeasId, keymap: Option<&crate::keys::Keymap>) {
        let Some(name) = self.meas(id).map(|m| m.config.name.clone()) else {
            return;
        };
        if self.hidden_meas.remove(&id) {
            self.toast(format!("{name} shown"));
        } else {
            let key = keymap
                .and_then(|k| {
                    k.first_chord(
                        crate::keys::CommandId::ToggleSelected,
                        crate::keys::Scope::Global,
                    )
                })
                .map_or_else(|| "its dot".to_owned(), |c| c.label());
            self.toast(format!(
                "{name} hidden: it keeps measuring · {key} shows it"
            ));
            self.hidden_meas.insert(id);
        }
    }

    /// Shift+A: hides the selected measurement's whole group — its live curve, its stored
    /// traces and the math channels made on it — or, when all of it is hidden, shows it
    /// again. With a trace selected, its group.
    pub(super) fn toggle_group_shown(&mut self, out: &mut Vec<Request>) {
        let Some(group) = self.selected_group() else {
            self.warn(self.select_first("first"));
            return;
        };
        let ms = self.measurements();
        let traces: Vec<TraceMeta> = self
            .stored_traces()
            .into_iter()
            .filter(|t| ac2_scene::meas_list::group_of(t, &ms) == group)
            .cloned()
            .collect();
        let live: Vec<MeasId> = ms
            .iter()
            .filter(|m| match &m.config.kind {
                MeasKind::Math { config } => config.owner == group,
                _ => group == TraceOwner::Meas { meas: m.id },
            })
            .map(|m| m.id)
            .collect();
        let name = match group {
            TraceOwner::Meas { meas } => self
                .meas(meas)
                .map_or_else(|| "the group".to_owned(), |m| m.config.name.clone()),
            TraceOwner::Imported => "Imported".to_owned(),
        };
        let any_shown = traces.iter().any(|t| t.edit.visible)
            || live.iter().any(|id| !self.hidden_meas.contains(id));
        for id in &live {
            if any_shown {
                self.hidden_meas.insert(*id);
            } else {
                self.hidden_meas.remove(id);
            }
        }
        for t in traces.into_iter().filter(|t| t.edit.visible == any_shown) {
            let mut edit = t.edit.clone();
            edit.visible = !any_shown;
            out.push(Request::Call {
                what: format!(
                    "{} {}",
                    t.edit.name,
                    if any_shown { "hidden" } else { "shown" }
                ),
                cmd: Command::TraceUpdate { trace: t.id, edit },
            });
        }
        self.toast(if any_shown {
            format!("{name} and everything under it hidden (it keeps measuring)")
        } else {
            format!("{name} and everything under it shown")
        });
    }

    /// "Move to measurement…": asks where the selected stored trace (or math channel) is
    /// filed: under another measurement, or in the imported group.
    pub(super) fn ask_move(&mut self) {
        let (what, label, current) =
            if let Some(t) = self.selected_trace_meta().filter(|_| self.keys_on_trace()) {
                (MoveWhat::Trace(t.id), t.edit.name.clone(), t.edit.owner)
            } else if let Some(m) = self.selected_meas() {
                match &m.config.kind {
                    MeasKind::Math { config } => {
                        (MoveWhat::Math(m.id), m.config.name.clone(), config.owner)
                    }
                    _ => {
                        self.warn(
                            "select a stored trace or a math channel to move (a measurement is a \
                         group of its own)",
                        );
                        return;
                    }
                }
            } else {
                self.warn(self.select_first("first"));
                return;
            };
        let ms = self.measurements();
        let q = ac2_scene::meas_list::move_choices(&label, current, &ms);
        let index = q.default;
        self.overlay = Overlay::Choose(Box::new(ChoicePrompt {
            purpose: ChoicePurpose::Move(what),
            label,
            title: q.title,
            lines: q.lines,
            choices: q.choices,
            owners: q.owners,
            index,
            hint: q.hint,
        }));
    }

    /// The answer to the open question (`None`: cancel).
    pub(super) fn choose(&mut self, pick: Option<usize>, out: &mut Vec<Request>) {
        let Overlay::Choose(c) = &self.overlay else {
            return;
        };
        let c = c.as_ref().clone();
        let Some(i) = pick else {
            self.overlay = Overlay::None;
            return;
        };
        if let Some(why) = c.choices.get(i).and_then(|x| x.blocked.clone()) {
            self.warn(format!("not possible: {why}"));
            return;
        }
        self.overlay = Overlay::None;
        match c.purpose {
            ChoicePurpose::NewMeasurement => self.new_measurement_pick(i, out),
            p @ (ChoicePurpose::Unsaved { .. }
            | ChoicePurpose::Replace { .. }
            | ChoicePurpose::Salvage { .. }) => self.session_answer(p, i, out),
            ChoicePurpose::DeleteOwned { meas } => {
                let traces = match i {
                    ac2_scene::meas_list::KEEP => OwnedTraces::Keep,
                    ac2_scene::meas_list::DELETE => OwnedTraces::Delete,
                    _ => return,
                };
                self.hidden_meas.remove(&meas);
                self.compared_meas.remove(&meas);
                let mut what = match traces {
                    OwnedTraces::Keep => {
                        format!("{} deleted; its traces are under Imported", c.label)
                    }
                    OwnedTraces::Delete => format!("{} deleted with its traces", c.label),
                };
                self.stimulus_with_deleted(meas, &mut what, out);
                self.call(out, Command::MeasDelete { meas, traces }, what);
            }
            ChoicePurpose::Move(what) => {
                let Some(owner) = c.owners.get(i).copied() else {
                    return;
                };
                let place = match owner {
                    TraceOwner::Meas { meas } => self
                        .meas(meas)
                        .map_or_else(|| format!("measurement {meas}"), |m| m.config.name.clone()),
                    TraceOwner::Imported => "Imported".to_owned(),
                };
                self.collapsed.remove(&owner);
                match what {
                    MoveWhat::Trace(id) => {
                        let Ok(t) = self.trace_meta(id) else {
                            return;
                        };
                        let mut edit = t.edit.clone();
                        edit.owner = owner;
                        self.call(
                            out,
                            Command::TraceUpdate { trace: id, edit },
                            format!("{} moved to {place}", c.label),
                        );
                    }
                    MoveWhat::Math(id) => {
                        let Some(mut config) = self.meas(id).map(|m| m.config.clone()) else {
                            return;
                        };
                        if let MeasKind::Math { config: mc } = &mut config.kind {
                            mc.owner = owner;
                        }
                        let cmd = self.meas_update(id, config);
                        self.call(out, cmd, format!("{} moved to {place}", c.label));
                    }
                }
            }
        }
    }

    /// Delete / Backspace: asks before the selected stored trace or measurement goes (a
    /// locked trace refuses; a measurement a math channel computes from says so in the
    /// confirmation's place).
    pub(super) fn ask_delete(&mut self) {
        if self.keys_on_trace() {
            self.ask_delete_trace();
            return;
        }
        let Some(m) = self.selected_meas().cloned() else {
            self.warn(self.select_first("first"));
            return;
        };
        let operand = Operand::Meas { meas: m.id };
        let owner = TraceOwner::Meas { meas: m.id };
        let owned: Vec<TraceId> = self
            .stored_traces()
            .iter()
            .filter(|t| t.edit.owner == owner)
            .map(|t| t.id)
            .collect();
        let maths: Vec<&Measurement> = self
            .measurements()
            .into_iter()
            .filter(
                |x| matches!(&x.config.kind, MeasKind::Math { config } if config.owner == owner),
            )
            .collect();
        // Math channels it owns go (or stay) with it; the others refuse as before.
        let outside: Vec<String> = self
            .measurements()
            .iter()
            .filter(|x| matches!(&x.config.kind, MeasKind::Math { config } if config.expr.names(operand) && config.owner != owner))
            .map(|x| x.config.name.clone())
            .collect();
        if outside.is_empty() && (!owned.is_empty() || !maths.is_empty()) {
            // Keep leaves its math channels computing from it: refused if they name it.
            let keep_blocked: Vec<String> = maths
                .iter()
                .filter(|x| matches!(&x.config.kind, MeasKind::Math { config } if config.expr.names(operand)))
                .map(|x| x.config.name.clone())
                .collect();
            // Delete takes its traces: refused if a math channel elsewhere names one.
            let delete_blocked: Vec<String> = self
                .measurements()
                .iter()
                .filter(|x| {
                    matches!(&x.config.kind, MeasKind::Math { config }
                    if config.owner != owner
                        && owned.iter().any(|t| config.expr.names(Operand::Trace { trace: *t })))
                })
                .map(|x| x.config.name.clone())
                .collect();
            let q = ac2_scene::meas_list::delete_choices(
                &m,
                owned.len(),
                maths.len(),
                &keep_blocked,
                &delete_blocked,
            );
            self.overlay = Overlay::Choose(Box::new(ChoicePrompt {
                purpose: ChoicePurpose::DeleteOwned { meas: m.id },
                label: m.config.name.clone(),
                title: q.title,
                lines: q.lines,
                choices: q.choices,
                owners: Vec::new(),
                index: q.default,
                hint: q.hint,
            }));
            return;
        }
        let users = outside;
        let (target, confirm) = if users.is_empty() {
            (
                DeleteTarget::Meas(m.id),
                ac2_scene::meas_list::delete_confirm(&m),
            )
        } else {
            (
                DeleteTarget::Refused,
                ac2_scene::meas_list::delete_refused(&m, &users),
            )
        };
        self.overlay = Overlay::Delete(Box::new(DeletePrompt {
            target,
            label: m.config.name.clone(),
            confirm,
        }));
    }

    fn ask_delete_trace(&mut self) {
        let Some(t) = self.selected_trace_meta().cloned() else {
            return;
        };
        let label = trace_label(&t);
        if t.edit.locked {
            self.warn(format!("{label} is locked: it is not deleted"));
            return;
        }
        let Some(row) = self.trace_rows().into_iter().find(|r| r.id == t.id) else {
            return;
        };
        self.overlay = Overlay::Delete(Box::new(DeletePrompt {
            target: DeleteTarget::Trace(t.id),
            label,
            confirm: ac2_scene::trace_list::delete_confirm(&row),
        }));
    }

    /// The delete confirmation's answer: delete, or keep it.
    pub(super) fn delete(&mut self, go: bool, out: &mut Vec<Request>) {
        let Overlay::Delete(p) = &self.overlay else {
            return;
        };
        let (target, label) = (p.target, p.label.clone());
        self.overlay = Overlay::None;
        if !go {
            return;
        }
        match target {
            DeleteTarget::Trace(id) => self.delete_trace(id, label, out),
            DeleteTarget::Meas(meas) => {
                self.hidden_meas.remove(&meas);
                self.compared_meas.remove(&meas);
                // It owns nothing (else the three-answer question was asked).
                let mut what = format!("{label} deleted");
                self.stimulus_with_deleted(meas, &mut what, out);
                self.call(
                    out,
                    Command::MeasDelete {
                        meas,
                        traces: OwnedTraces::Keep,
                    },
                    what,
                );
            }
            DeleteTarget::Refused => {}
        }
    }

    /// A deleted measurement ends its measuring as a stop does: the last running transfer
    /// measurement takes the stimulus this app holds with it, and the toast `what` says so.
    fn stimulus_with_deleted(&mut self, meas: MeasId, what: &mut String, out: &mut Vec<Request>) {
        if let Some(m) = self.meas(meas).cloned()
            && self.stop_stimulus_with(&m, out)
        {
            what.push_str(super::STIMULUS_STOPPED_TOO);
        }
    }

    /// Deletes stored trace `id`; the selection moves to the next shown trace in the list,
    /// else the one before it, else the live measurement.
    /// C: compares what is selected in the tree (a stored trace selected last, else the
    /// selected measurement or math channel) on the transfer pane, or stops comparing it.
    /// The pane draws compared curves besides its own group, whoever owns them; display
    /// only (never an average's or a math channel's operand).
    pub(super) fn toggle_compare(&mut self) {
        if let Some(t) = self.selected_trace_meta().cloned() {
            let label = trace_label(&t);
            if !drawn_in(&t, PaneKind::Transfer) {
                self.warn(format!("{label}: compare draws transfer curves only"));
                return;
            }
            let on = self.compared_traces.insert(t.id);
            if !on {
                self.compared_traces.remove(&t.id);
            }
            self.toast(ac2_scene::meas_list::compare_toast(
                &label,
                on,
                t.edit.visible,
            ));
            return;
        }
        let Some(m) = self.selected_meas().cloned() else {
            self.warn(self.select_first("to compare"));
            return;
        };
        let name = m.config.name.clone();
        if !m.config.kind.publishes_tf() {
            self.warn(format!("{name}: no live transfer curve to compare"));
            return;
        }
        let on = self.compared_meas.insert(m.id);
        if !on {
            self.compared_meas.remove(&m.id);
        }
        self.toast(ac2_scene::meas_list::compare_toast(
            &name,
            on,
            !self.meas_hidden(&m),
        ));
    }

    /// Clear compare: the transfer pane draws its own group alone again.
    pub(super) fn clear_compare(&mut self) {
        let n = self.compared_meas.len() + self.compared_traces.len();
        self.compared_meas.clear();
        self.pending_compared.clear();
        self.compared_traces.clear();
        self.toast(match n {
            0 => "nothing compared".to_owned(),
            1 => "compare cleared: 1 curve".to_owned(),
            n => format!("compare cleared: {n} curves"),
        });
    }

    /// A stored trace gone from the daemon (deleted here or elsewhere) drops out of compare.
    pub(super) fn prune_compared(&mut self) {
        let Some(st) = self.mirror.as_ref().and_then(|m| m.state.as_deref()) else {
            return;
        };
        let ids: std::collections::BTreeSet<TraceId> = st.traces.iter().map(|t| t.id).collect();
        self.compared_traces.retain(|id| ids.contains(id));
    }

    fn delete_trace(&mut self, id: TraceId, label: String, out: &mut Vec<Request>) {
        self.compared_traces.remove(&id);
        let list: Vec<(TraceId, bool)> = self
            .trace_list()
            .iter()
            .map(|t| (t.id, t.edit.visible))
            .collect();
        let at = list
            .iter()
            .position(|(x, _)| *x == id)
            .unwrap_or(list.len());
        let next = list[at.min(list.len())..]
            .iter()
            .chain(list[..at.min(list.len())].iter().rev())
            .find(|(x, shown)| *x != id && *shown)
            .map(|(x, _)| *x);
        self.select_trace(next);
        let then = match self.selected_trace_meta() {
            Some(t) => format!(" · {} selected", trace_label(t)),
            None => " · keys act on the live measurement".to_owned(),
        };
        self.call(
            out,
            Command::TraceDelete { trace: id },
            format!("{label} deleted{then}"),
        );
    }

    // ----- which pane a selection brings up ----------------------------------------------

    /// After a stored trace was selected with the layout maximised: the focus goes to a pane that draws it (the transfer pane draws sweeps
    /// too; else a sweep's home is the sweep pane), the focused pane turning into one when
    /// none is on screen. The split layout keeps the focus: every pane is on screen.
    pub(super) fn reveal_trace(&mut self) {
        if !self.layout.maximized {
            return;
        }
        let Some(t) = self.selected_trace_meta().cloned() else {
            return;
        };
        if drawn_in(&t, self.layout.focus_kind()) {
            return;
        }
        let p = home_pane(t.kind);
        match self.layout.lead(p) {
            Some(id) => self.layout.set_focus(id),
            None => {
                let f = self.layout.focus;
                self.set_pane_kind(f, p);
            }
        }
    }
}
