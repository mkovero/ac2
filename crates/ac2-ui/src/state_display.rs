//! Display edits the operator makes while comparing curves: display offsets in steps, the
//! level (vertical) axis of each pane, showing / hiding and deleting (after a confirmation)
//! the selected measurement or stored trace, and which pane a selection brings up. No
//! measured value changes: an offset is drawn, never applied to the stored columns.

use ac2_proto::Command;
use ac2_proto::model::{
    LevelScale, MeasKind, Measurement, Operand, OwnedTraces, TraceKind, TraceMeta, TraceOwner,
};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{Db, MeasId, TraceId};
use ac2_scene::axis::Range;
use ac2_scene::format;
use ac2_scene::grid::column_frequencies;
use ac2_scene::view::{ViewState, level};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoicePurpose {
    /// Deleting measurement `meas` that owns traces or math channels: keep, delete, cancel
    /// ([`ac2_scene::meas_list::KEEP`] …).
    DeleteOwned { meas: MeasId },
    /// Filing a stored trace (or a math channel) under the owner of the answer picked.
    Move(MoveWhat),
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

/// Zoom factor of one level-zoom key press.
pub const LEVEL_ZOOM_FACTOR: f64 = 1.5;

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

/// The pane whose level axis the keys act on, if it has one.
fn level_pane(p: PaneKind) -> Option<PaneKind> {
    match p {
        PaneKind::Transfer | PaneKind::Spectrum | PaneKind::Distortion => Some(p),
        PaneKind::Ir | PaneKind::Spl => None,
    }
}

/// The level range of `pane` in `view`; the spectrum pane's is the one for `scale`, the
/// scale its curves are shown in ([`crate::scenes::spectrum_scale`]).
pub fn level_range(view: &ViewState, pane: PaneKind, scale: LevelScale) -> Option<Range> {
    match pane {
        PaneKind::Transfer => Some(view.tf.magnitude_db),
        PaneKind::Spectrum => Some(view.spectrum.range(scale)),
        PaneKind::Distortion => Some(view.distortion.range_db),
        PaneKind::Ir | PaneKind::Spl => None,
    }
}

fn level_range_mut(view: &mut ViewState, pane: PaneKind, scale: LevelScale) -> Option<&mut Range> {
    match pane {
        PaneKind::Transfer => Some(&mut view.tf.magnitude_db),
        PaneKind::Spectrum => Some(view.spectrum.range_mut(scale)),
        PaneKind::Distortion => Some(&mut view.distortion.range_db),
        PaneKind::Ir | PaneKind::Spl => None,
    }
}

/// The pane a stored trace of this kind is drawn in first.
fn home_pane(kind: TraceKind) -> PaneKind {
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
                self.warn(format!(
                    "no {} measurement to offset ({SELECT_TRACE_FIRST_SHORT})",
                    pane.what()
                ));
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

    // ----- the level axis ----------------------------------------------------------------

    /// The pane the level keys act on (the focused one), or why not.
    fn level_target(&mut self) -> Option<PaneKind> {
        let p = self.layout.focus_kind();
        let p = level_pane(p).filter(|p| {
            *p != PaneKind::Distortion || self.modes().sweep == ac2_scene::view::SweepMode::Response
        });
        if p.is_none() {
            self.warn(format!(
                "{} has no level axis to zoom",
                self.layout.focus_kind().title()
            ));
        }
        p
    }

    /// Zoom pane `pane`'s level axis by `factor` (> 1 in) about `about` (dB; `None`: the
    /// middle).
    pub(super) fn level_zoom(&mut self, pane: PaneKind, about: Option<f64>, factor: f64) {
        let scale = crate::scenes::spectrum_scale(self);
        if let Some(r) = level_range_mut(&mut self.view, pane, scale) {
            let a = about.unwrap_or((r.lo + r.hi) / 2.0);
            *r = level::zoom(*r, a, factor);
        }
    }

    /// Pan pane `pane`'s level axis by `db`.
    pub(super) fn level_pan(&mut self, pane: PaneKind, db: f64) {
        let scale = crate::scenes::spectrum_scale(self);
        if let Some(r) = level_range_mut(&mut self.view, pane, scale) {
            *r = level::pan(*r, db);
        }
    }

    pub(super) fn level_key(&mut self, c: crate::keys::CommandId) {
        use crate::keys::CommandId as C;
        let Some(p) = self.level_target() else {
            return;
        };
        let scale = crate::scenes::spectrum_scale(self);
        let Some(r) = level_range(&self.view, p, scale) else {
            return;
        };
        match c {
            C::LevelZoomIn => self.level_zoom(p, None, LEVEL_ZOOM_FACTOR),
            C::LevelZoomOut => self.level_zoom(p, None, 1.0 / LEVEL_ZOOM_FACTOR),
            C::LevelPanUp => self.level_pan(p, level::pan_step(r)),
            C::LevelPanDown => self.level_pan(p, -level::pan_step(r)),
            C::LevelReset => {
                let d = level_range(&ViewState::default(), p, scale);
                if let (Some(slot), Some(d)) = (level_range_mut(&mut self.view, p, scale), d) {
                    *slot = d;
                }
            }
            C::LevelFit => match level::fit(self.level_values(p)) {
                Some(fit) => {
                    if let Some(slot) = level_range_mut(&mut self.view, p, scale) {
                        *slot = fit;
                    }
                    self.toast(format!(
                        "{}: level {} … {} dB",
                        p.title(),
                        format::fixed(fit.lo, 0),
                        format::fixed(fit.hi, 0)
                    ));
                }
                None => self.warn(format!("{}: no curve shown to fit", p.title())),
            },
            _ => {}
        }
    }

    /// A started spectrum / RTA measurement's first frame is in: the spectrum pane's level
    /// axis frames what it shows, as Shift+Home does (frequency left as it is). A level
    /// range that suits one signal rarely suits the next, so each start begins framed.
    pub(super) fn fit_started_spectra(&mut self) {
        let arrived: Vec<MeasId> = self
            .spectrum_fit
            .iter()
            .filter(|(id, shown)| {
                self.spectrum_frame(**id).is_some_and(|f| {
                    shown
                        .as_ref()
                        .is_none_or(|old| !std::sync::Arc::ptr_eq(old, &f.frame))
                })
            })
            .map(|(id, _)| *id)
            .collect();
        if arrived.is_empty() {
            return;
        }
        for id in &arrived {
            self.spectrum_fit.remove(id);
        }
        let scale = crate::scenes::spectrum_scale(self);
        if let Some(fit) = level::fit(self.level_values(PaneKind::Spectrum))
            && let Some(slot) = level_range_mut(&mut self.view, PaneKind::Spectrum, scale)
        {
            *slot = fit;
        }
    }

    /// A finished sweep's data is in: the sweep pane's level axis frames its harmonics and
    /// THD, as Shift+Home does (frequency left as it is). Distortion levels differ by tens
    /// of dB from one device or drive level to the next, so a range kept from the last
    /// result often shows the new one off the plot. Only once per result: a zoom after it
    /// stays until the next sweep.
    pub(super) fn fit_new_sweep(&mut self) {
        let Some(id) = self.sweep.fit else {
            return;
        };
        if !self.traces.contains_key(&id) {
            return;
        }
        self.sweep.fit = None;
        if self.shown_sweep().map(|(t, _)| t.meta.id) != Some(id) {
            return;
        }
        if let Some(fit) = level::fit(self.level_values(PaneKind::Distortion)) {
            self.view.distortion.range_db = fit;
        }
    }

    /// Every level pane `p` draws in the shown frequency range, as drawn (offsets
    /// included).
    fn level_values(&self, p: PaneKind) -> Vec<f64> {
        let (lo, hi) = (self.view.freq.lo, self.view.freq.hi);
        let mut v = Vec::new();
        let mut add = |freqs: &[f64], vals: &[f32], offset: f64| {
            v.extend(
                freqs
                    .iter()
                    .zip(vals)
                    .filter(|(f, _)| **f >= lo && **f <= hi)
                    .map(|(_, x)| f64::from(*x) + offset),
            );
        };
        let live = |stream: Stream, m: &Measurement| {
            let d = self.data.as_ref()?;
            let f = d.latest.get(&Topic::Data { meas: m.id, stream })?;
            let g = d.grids.get(&f.frame.stamp.grid_id?)?;
            Some((f.frame.clone(), column_frequencies(g)))
        };
        let shown = self.kind_meas(PaneKind::Transfer);
        match p {
            PaneKind::Transfer => {
                for m in self.measurements().into_iter().filter(|m| {
                    self.live_on_transfer_pane(m, shown) || self.live_compared_on_transfer(m, shown)
                }) {
                    if let Some((f, freqs)) = live(Stream::Tf, m)
                        && let ac2_proto::FrameData::Tf(tf) = &f.data
                    {
                        add(&freqs, &tf.mag, self.edit(m.id).offset_db);
                    }
                }
                for (t, g) in self.traces.values() {
                    if self.on_transfer_pane(&t.meta, shown)
                        || self.trace_compared_on_transfer(&t.meta, shown)
                    {
                        add(&column_frequencies(g), &t.mag_db, t.meta.edit.offset.0);
                    }
                }
            }
            PaneKind::Spectrum => {
                for m in self
                    .measurements()
                    .into_iter()
                    .filter(|m| !self.meas_hidden(m))
                {
                    if !m.config.kind.publishes_levels() {
                        continue;
                    }
                    let Some(stream) = m.config.kind.stream() else {
                        continue;
                    };
                    if let Some((f, freqs)) = live(stream, m) {
                        let level = match &f.data {
                            ac2_proto::FrameData::Spec(s) => &s.level,
                            ac2_proto::FrameData::Rta(r) => &r.level,
                            _ => continue,
                        };
                        add(&freqs, level, self.edit(m.id).offset_db);
                    }
                }
                for (t, g) in self.traces.values() {
                    if t.meta.edit.visible
                        && matches!(
                            t.meta.kind,
                            TraceKind::Spectrum { .. } | TraceKind::Rta { .. }
                        )
                    {
                        add(&column_frequencies(g), &t.mag_db, t.meta.edit.offset.0);
                    }
                }
            }
            PaneKind::Distortion => {
                if let Some((t, g)) = self.shown_sweep()
                    && let Some(s) = &t.sweep
                {
                    let freqs = column_frequencies(g);
                    for c in s.harmonics.iter().map(|h| &h.curve).chain([&s.thd]) {
                        add(&freqs, &c.level_db, 0.0);
                        add(&freqs, &c.floor_db, 0.0);
                    }
                }
            }
            PaneKind::Ir | PaneKind::Spl => {}
        }
        v
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
            self.warn(SELECT_FIRST);
            return;
        };
        self.toggle_meas_hidden(id, Some(keymap));
    }

    /// Shows or hides measurement `id`'s live curves (this app's display only).
    pub(super) fn toggle_meas_hidden(&mut self, id: MeasId, keymap: Option<&crate::keys::Keymap>) {
        let Some(name) = self.meas(id).map(|m| m.config.name.clone()) else {
            return;
        };
        if self.hidden_meas.remove(&name) {
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
            self.hidden_meas.insert(name);
        }
    }

    /// Shift+A: hides the selected measurement's whole group — its live curve, its stored
    /// traces and the math channels made on it — or, when all of it is hidden, shows it
    /// again. With a trace selected, its group.
    pub(super) fn toggle_group_shown(&mut self, out: &mut Vec<Request>) {
        let Some(group) = self.selected_group() else {
            self.warn(SELECT_FIRST);
            return;
        };
        let ms = self.measurements();
        let traces: Vec<TraceMeta> = self
            .stored_traces()
            .into_iter()
            .filter(|t| ac2_scene::meas_list::group_of(t, &ms) == group)
            .cloned()
            .collect();
        let live: Vec<String> = ms
            .iter()
            .filter(|m| match &m.config.kind {
                MeasKind::Math { config } => config.owner == group,
                _ => group == TraceOwner::Meas { meas: m.id },
            })
            .map(|m| m.config.name.clone())
            .collect();
        let name = match group {
            TraceOwner::Meas { meas } => self
                .meas(meas)
                .map_or_else(|| "the group".to_owned(), |m| m.config.name.clone()),
            TraceOwner::Imported => "Imported".to_owned(),
        };
        let any_shown = traces.iter().any(|t| t.edit.visible)
            || live.iter().any(|n| !self.hidden_meas.contains(n));
        for n in &live {
            if any_shown {
                self.hidden_meas.insert(n.clone());
            } else {
                self.hidden_meas.remove(n);
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
                self.warn(SELECT_FIRST);
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
            ChoicePurpose::DeleteOwned { meas } => {
                let traces = match i {
                    ac2_scene::meas_list::KEEP => OwnedTraces::Keep,
                    ac2_scene::meas_list::DELETE => OwnedTraces::Delete,
                    _ => return,
                };
                self.hidden_meas.remove(&c.label);
                self.compared_meas.remove(&c.label);
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
                        self.call(
                            out,
                            Command::MeasUpdate { meas: id, config },
                            format!("{} moved to {place}", c.label),
                        );
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
            self.warn(SELECT_FIRST);
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
                // A measurement of that name made later starts shown.
                self.hidden_meas.remove(&label);
                self.compared_meas.remove(&label);
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
            self.warn(
                "select a measurement or a stored trace to compare (click it in the list, N, V)",
            );
            return;
        };
        let name = m.config.name.clone();
        if !m.config.kind.publishes_tf() {
            self.warn(format!("{name}: no live transfer curve to compare"));
            return;
        }
        let on = self.compared_meas.insert(name.clone());
        if !on {
            self.compared_meas.remove(&name);
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

    /// After a measurement was picked (the list, a pane's chip): unless the focused pane
    /// draws it, the pane of its kind worked in last gets the focus; with none on screen the
    /// focused pane turns into one, so a single or maximised pane switches to it.
    pub(super) fn reveal_meas(&mut self, id: MeasId) {
        let Some(kind) = self.meas(id).map(|m| m.config.kind.clone()) else {
            return;
        };
        // A sweep picked in the list is the sweep pane's: the transfer pane draws its runs
        // only when chosen there.
        let fk = self.layout.focus_kind();
        if fk.shows(&kind)
            && !(matches!(kind, MeasKind::Sweep { .. }) && fk != PaneKind::Distortion)
        {
            return;
        }
        let home = PaneKind::for_kind(&kind);
        let laid = self.laid_out_panes();
        match self.layout.lead(home).filter(|p| laid.contains(p)) {
            Some(p) => self.layout.set_focus(p),
            None => {
                let f = self.layout.focus;
                self.set_pane_kind(f, home);
            }
        }
        let f = self.layout.focus;
        self.select_on(f, id);
    }

    /// After a stored trace was selected with the layout maximised, or with panes following
    /// the selection: the focus goes to a pane that draws it (the transfer pane draws sweeps
    /// too; else a sweep's home is the sweep pane), the focused pane turning into one when
    /// none is on screen. The split layout keeps the focus: every pane is on screen.
    pub(super) fn reveal_trace(&mut self) {
        if !self.layout.maximized && !self.prefs.panes_follow {
            return;
        }
        let Some(t) = self.selected_trace_meta().cloned() else {
            return;
        };
        if drawn_in(&t, self.layout.focus_kind()) {
            return;
        }
        let p = home_pane(t.kind);
        let laid = self.laid_out_panes();
        match self.layout.lead(p).filter(|id| laid.contains(id)) {
            Some(id) => self.layout.set_focus(id),
            None => {
                let f = self.layout.focus;
                self.set_pane_kind(f, p);
            }
        }
    }
}

/// How to select a trace, in a sentence that goes on.
const SELECT_TRACE_FIRST_SHORT: &str = "select a stored trace with V or a click in the list";

/// What Delete and A say with nothing selected.
const SELECT_FIRST: &str =
    "select a measurement or a stored trace first (click it in the list, N, V)";
