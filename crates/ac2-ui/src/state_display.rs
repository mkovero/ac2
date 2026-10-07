//! Display edits the operator makes while comparing curves: display offsets in steps, the
//! level (vertical) axis of each pane, deleting a stored trace after a confirmation, and
//! which pane a selection brings up. No measured value changes: an offset is drawn, never
//! applied to the stored columns.

use ac2_proto::Command;
use ac2_proto::model::{LevelScale, Measurement, TraceKind, TraceMeta};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{Db, MeasId, TraceId};
use ac2_scene::axis::Range;
use ac2_scene::format;
use ac2_scene::grid::column_frequencies;
use ac2_scene::view::{ViewState, level};

use super::{AppState, Overlay, PaneKind, drawn_in, on_transfer_pane, trace_label};
use crate::conn::Request;

/// The confirmation before a stored trace is deleted.
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteTracePrompt {
    pub trace: TraceId,
    /// How toasts name it: `slot 1 (Main L S1)`.
    pub label: String,
    pub confirm: ac2_scene::trace_list::DeleteConfirm,
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
                self.error(format!("{} is locked: its offset stays", trace_label(&t)));
                return None;
            }
            return Some(OffsetTarget::Trace(t));
        }
        let pane = match self.layout.focus {
            PaneKind::Spectrum => PaneKind::Spectrum,
            _ => PaneKind::Transfer,
        };
        match self.pane_meas(pane).cloned() {
            Some(m) => Some(OffsetTarget::Live(m)),
            None => {
                self.error(format!(
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
        let p = self.layout.focus;
        let p = level_pane(p).filter(|p| {
            *p != PaneKind::Distortion
                || self.view.distortion.mode == ac2_scene::view::SweepMode::Response
        });
        if p.is_none() {
            self.error(format!(
                "{} has no level axis to zoom",
                self.layout.focus.title()
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
                None => self.error(format!("{}: no curve shown to fit", p.title())),
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
        match p {
            PaneKind::Transfer => {
                for m in self.measurements() {
                    if let Some((f, freqs)) = live(Stream::Tf, m)
                        && let ac2_proto::FrameData::Tf(tf) = &f.data
                    {
                        add(&freqs, &tf.mag, self.edit(m.id).offset_db);
                    }
                }
                for (t, g) in self.traces.values() {
                    if on_transfer_pane(&t.meta) {
                        add(&column_frequencies(g), &t.mag_db, t.meta.edit.offset.0);
                    }
                }
            }
            PaneKind::Spectrum => {
                for m in self.measurements() {
                    if !m.config.kind.publishes_levels() {
                        continue;
                    }
                    let stream = m.config.kind.stream();
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

    // ----- deleting a stored trace -------------------------------------------------------

    /// Delete: asks before the selected stored trace goes; a locked one refuses.
    pub(super) fn ask_delete_trace(&mut self) {
        let Some(t) = self.selected_trace_meta().cloned() else {
            self.error(format!(
                "Delete removes a stored trace: {SELECT_TRACE_FIRST_SHORT} (a live measurement \
                 is deleted from the palette)"
            ));
            return;
        };
        let label = trace_label(&t);
        if t.edit.locked {
            self.error(format!("{label} is locked: it is not deleted"));
            return;
        }
        let Some(row) = self.trace_rows().into_iter().find(|r| r.id == t.id) else {
            return;
        };
        self.overlay = Overlay::DeleteTrace(Box::new(DeleteTracePrompt {
            trace: t.id,
            label,
            confirm: ac2_scene::trace_list::delete_confirm(&row),
        }));
    }

    /// The delete confirmation: delete (the selection moves to the next shown trace in the
    /// list, else the one before it, else the live measurement), or keep it.
    pub(super) fn delete_trace(&mut self, go: bool, out: &mut Vec<Request>) {
        let Overlay::DeleteTrace(p) = &self.overlay else {
            return;
        };
        let (id, label) = (p.trace, p.label.clone());
        self.overlay = Overlay::None;
        if !go {
            return;
        }
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

    /// After a measurement was picked (the list, a pane's chip): the pane that draws its
    /// kind gets the focus unless the focused one draws it already, so a maximised layout
    /// switches to it.
    pub(super) fn reveal_meas(&mut self, id: MeasId) {
        let Some(kind) = self.meas(id).map(|m| m.config.kind.clone()) else {
            return;
        };
        if !self.layout.focus.shows(&kind) {
            let p = PaneKind::for_kind(&kind);
            self.layout.shown[p.index()] = true;
            self.layout.focus = p;
        }
    }

    /// After a stored trace was selected with the layout maximised: the pane shows a pane
    /// that draws it (the transfer pane draws sweeps too; else a sweep's home is the sweep
    /// pane). The split layout keeps the focus: every pane is on screen.
    pub(super) fn reveal_trace(&mut self) {
        if !self.layout.maximized {
            return;
        }
        let Some(t) = self.selected_trace_meta().cloned() else {
            return;
        };
        if !drawn_in(&t, self.layout.focus) {
            let p = home_pane(t.kind);
            self.layout.shown[p.index()] = true;
            self.layout.focus = p;
        }
    }
}

/// How to select a trace, in a sentence that goes on.
const SELECT_TRACE_FIRST_SHORT: &str = "select a stored trace with V or a click in the list";
