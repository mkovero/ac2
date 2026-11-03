//! What the daemon keeps with a measurement about how it is drawn: a spectrum's style, peak
//! hold and spectrograph (B, P, Shift+B, colours), which every client draws the same and
//! a session restores; and what a graph pane draws behind its lead (C's tick list), kept
//! in the pane.

use crate::state::*;
use ac2_proto::model::layout::EntryRef;
use ac2_proto::model::{Colormap, SpectrumDisplay};

impl AppState {
    /// How measurement `m`'s curves draw: its own display, a math channel its owner's (as it
    /// takes its owner's colours), the defaults for anything else.
    pub fn display_of(&self, m: &Measurement) -> SpectrumDisplay {
        self.rig.display_of(m)
    }

    /// How stored trace `t` draws: as its owner measurement does.
    pub fn trace_display(&self, t: &TraceMeta) -> SpectrumDisplay {
        t.edit
            .owner
            .meas()
            .and_then(|id| self.meas(id))
            .map_or_else(SpectrumDisplay::default, |m| self.display_of(m))
    }

    /// The measurement B, P, Shift+B and the colour maps act on: the spectrum pane's (the
    /// focused one, else the one focused last), or the owner of the math channel it shows.
    fn display_target(&self) -> Result<Measurement, String> {
        let Some(m) = self.kind_meas(PaneKind::Spectrum) else {
            return Err(format!(
                "no spectrum or RTA: {} makes one",
                self.new_meas_reach()
            ));
        };
        if m.config.kind.display().is_some() {
            return Ok(m.clone());
        }
        if let MeasKind::Math { config } = &m.config.kind
            && let Some(o) = config.owner.meas().and_then(|id| self.meas(id))
            && o.config.kind.display().is_some()
        {
            return Ok(o.clone());
        }
        Err(format!(
            "{} draws as the spectrum it is filed under, and it is filed under none",
            m.config.name
        ))
    }

    /// Changes the display of the measurement the display keys act on; `f` says what it
    /// changed for the toast (`Spectrum 1: fill`).
    pub(in crate::state) fn edit_display(
        &mut self,
        f: impl FnOnce(&mut SpectrumDisplay) -> String,
        out: &mut Vec<Request>,
    ) {
        let mut m = match self.display_target() {
            Ok(m) => m,
            Err(e) => {
                self.warn(e);
                return;
            }
        };
        let Some(d) = m.config.kind.display_mut() else {
            return;
        };
        let said = f(d);
        let what = format!("{}: {said}", m.config.name);
        let cmd = self.meas_update(m.id, m.config);
        self.call(out, cmd, what);
    }

    /// B: the next curve style.
    pub(in crate::state) fn step_spectrum_style(&mut self, out: &mut Vec<Request>) {
        self.edit_display(
            |d| {
                d.style = d.style.next();
                d.style.name().to_owned()
            },
            out,
        );
    }

    /// P: peak hold, then peak tracking, then off (the hold and the tracked peaks are this
    /// app's, kept while the mode asks for them, so each hold starts afresh).
    pub(in crate::state) fn step_peak_mode(&mut self, out: &mut Vec<Request>) {
        self.edit_display(
            |d| {
                d.peak = d.peak.next();
                d.peak.name().to_owned()
            },
            out,
        );
    }

    /// Shift+B: the next spectrograph span.
    pub(in crate::state) fn step_spectrograph_span(&mut self, out: &mut Vec<Request>) {
        self.edit_display(
            |d| {
                d.spectrograph_span_s = SpectrumDisplay::next_span(d.spectrograph_span_s);
                format!("spectrograph: last {} s", d.spectrograph_span_s)
            },
            out,
        );
    }

    /// The spectrograph colour map, stepped `d` places or set.
    pub(in crate::state) fn spectrograph_colours(
        &mut self,
        to: Result<Colormap, i32>,
        out: &mut Vec<Request>,
    ) {
        self.edit_display(
            |d| {
                d.spectrograph_colours = match to {
                    Ok(c) => c,
                    Err(step) => d.spectrograph_colours.step(step),
                };
                format!("spectrograph colours: {}", d.spectrograph_colours.name())
            },
            out,
        );
    }

    /// What graph pane `pane` draws behind its lead, of its own rig, nearest layer first
    /// (another rig's entries are kept, drawn by no pane yet).
    pub fn compared_into(&self, pane: PaneId) -> Vec<EntryRef> {
        let Some(v) = self
            .layout
            .view(pane)
            .filter(|v| self.ours(v) && v.kind.draws_curves())
        else {
            return Vec::new();
        };
        let mine = self.pane_rig(v);
        v.with
            .as_slice()
            .iter()
            .filter(|p| p.rig.is_none_or(|r| r == mine))
            .map(|p| p.what)
            .collect()
    }

    /// The group measurement `id` draws: its own, or a math channel's owner's.
    fn group_meas(&self, id: MeasId) -> Option<MeasId> {
        let m = self.meas(id)?;
        ac2_scene::meas_list::meas_group(m, &self.measurements()).meas()
    }

    /// Whether entry `e` brings curve `c` (a live curve or a stored trace): the curve
    /// itself, or a group it is filed under.
    pub(in crate::state) fn entry_brings(&self, e: EntryRef, c: EntryRef) -> bool {
        if e == c {
            return true;
        }
        let EntryRef::Group { meas: g } = e else {
            return false;
        };
        let group = self.group_meas(g);
        match c {
            EntryRef::Live { meas } => group.is_some() && self.group_meas(meas) == group,
            EntryRef::Trace { trace } => self
                .daemon()
                .and_then(|s| s.traces.iter().find(|t| t.id == trace))
                .is_some_and(|t| {
                    ac2_scene::meas_list::group_of(t, &self.measurements()).meas() == group
                        && group.is_some()
                }),
            EntryRef::Group { .. } => false,
        }
    }

    /// Stacking layer of curve `c` on pane `pane`: 1 for what the first entry behind the
    /// lead brings, 2 for the next…; 0 for what the lead's group draws.
    pub fn compare_layer(&self, pane: PaneId, c: EntryRef) -> u8 {
        self.compared_into(pane)
            .iter()
            .position(|e| self.entry_brings(*e, c))
            .map_or(0, |i| u8::try_from(i + 1).unwrap_or(u8::MAX))
    }

    /// The measurements C lists on graph pane `pane`: its lead first, then every other
    /// measurement of the pane's rig whose group the pane's kind draws, in list order. A
    /// math channel comes with its owner's group.
    fn tick_rows(&self, pane: PaneId) -> Vec<MeasId> {
        let Some(v) = self.layout.view(pane) else {
            return Vec::new();
        };
        let lead = v.meas.filter(|id| self.meas(*id).is_some());
        lead.into_iter()
            .chain(
                self.measurements()
                    .iter()
                    .filter(|m| {
                        Some(m.id) != lead
                            && !matches!(m.config.kind, MeasKind::Math { .. })
                            && v.kind.shows(&m.config.kind)
                    })
                    .map(|m| m.id),
            )
            .collect()
    }

    /// Whether pane `pane` draws measurement `id`'s group: its lead, or ticked behind it.
    fn pane_ticks(&self, pane: PaneId, id: MeasId) -> bool {
        self.layout.view(pane).is_some_and(|v| {
            v.meas == Some(id) || v.with.contains(&Pick::here(EntryRef::Group { meas: id }))
        })
    }

    /// What measurement `m`'s group brings to a pane of kind `kind`: its live curve, if the
    /// pane draws it, and its shown traces of a kind the pane draws.
    fn brings(&self, m: &Measurement, kind: PaneKind) -> String {
        let ms = self.measurements();
        let mine = TraceOwner::Meas { meas: m.id };
        let traces = self
            .stored_traces()
            .into_iter()
            .filter(|t| {
                ac2_scene::meas_list::group_of(t, &ms) == mine
                    && if kind == PaneKind::Distortion {
                        drawn_in(t, kind)
                    } else {
                        kind_shown(t, kind)
                    }
            })
            .count();
        ac2_scene::meas_list::brings_words(Self::live_drawn_in(m, kind), traces)
    }

    /// C: the tick list over the focused graph pane, what can be drawn with its lead. On an
    /// SPL, Programme or Walk pane, which show one source, the hint bar says so.
    pub(in crate::state) fn open_compare(&mut self, _keymap: &crate::keys::Keymap) {
        let pane = self.layout.focus;
        let Some(v) = self.layout.view(pane).copied() else {
            return;
        };
        if !v.kind.draws_curves() {
            self.warn(ac2_scene::meas_list::ONE_SOURCE);
            return;
        }
        if !self.ours(&v) {
            let rig = self.rig_name(self.pane_rig(&v));
            self.warn(format!(
                "this pane shows {rig}: switch to it to tick what the pane draws"
            ));
            return;
        }
        let rows = self.tick_rows(pane);
        if rows.is_empty() {
            self.warn(ac2_scene::meas_list::tick_no_rows(v.kind.what()));
            return;
        }
        self.overlay = Overlay::Choose(Box::new(self.tick_prompt(pane, rows, 0)));
    }

    /// C's list for pane `pane` over `rows`, `index` highlighted.
    fn tick_prompt(&self, pane: PaneId, rows: Vec<MeasId>, index: usize) -> ChoicePrompt {
        let kind = self.layout.kind(pane);
        let lead = self.layout.view(pane).and_then(|v| v.meas);
        let ticks: Vec<ac2_scene::meas_list::TickRow> = rows
            .iter()
            .map(|id| ac2_scene::meas_list::TickRow {
                name: self
                    .meas(*id)
                    .map_or_else(|| format!("measurement {id}"), |m| m.config.name.clone()),
                brings: self
                    .meas(*id)
                    .map_or_else(String::new, |m| self.brings(m, kind)),
                on: self.pane_ticks(pane, *id),
                lead: lead == Some(*id),
            })
            .collect();
        let label = self.pane_title(pane);
        let q = ac2_scene::meas_list::tick_list(&label, &ticks);
        ChoicePrompt {
            purpose: ChoicePurpose::Compare { pane, rows },
            label,
            title: q.title,
            lines: q.lines,
            choices: q.choices,
            owners: Vec::new(),
            index,
            hint: q.hint,
        }
    }

    /// Row `i` of C's list taken: its measurement is ticked into the pane or unticked; the
    /// list stays open with the row's tick changed.
    pub(in crate::state) fn compare_pick(
        &mut self,
        c: ChoicePrompt,
        i: usize,
        _out: &mut Vec<Request>,
    ) {
        let ChoicePurpose::Compare { pane, rows } = c.purpose else {
            return;
        };
        let Some(&id) = rows.get(i) else {
            return;
        };
        let on = !self.pane_ticks(pane, id);
        if self.set_ticked(pane, id, on) {
            let (name, shown) = self
                .meas(id)
                .map_or((format!("measurement {id}"), true), |m| {
                    (m.config.name.clone(), !self.meas_hidden(m))
                });
            self.toast(ac2_scene::meas_list::tick_toast(&name, on, shown));
        }
        self.overlay = Overlay::Choose(Box::new(self.tick_prompt(pane, rows, i)));
    }

    /// Measurement `id`'s group ticked into pane `pane` (`on`), behind its lead (or as the
    /// lead of a pane with nothing picked), or unticked: unticking the lead makes the next
    /// entry lead, unticking the last leaves nothing picked. What the group draws anyway
    /// leaves the pane as a single entry. `false`: nothing changed (no room).
    pub(in crate::state) fn set_ticked(&mut self, pane: PaneId, id: MeasId, on: bool) -> bool {
        let group = EntryRef::Group { meas: id };
        let brought: Vec<Pick> = self
            .layout
            .view(pane)
            .map(|v| {
                v.with
                    .as_slice()
                    .iter()
                    .filter(|p| {
                        p.rig.is_none() && self.entry_brings(group, p.what) && p.what != group
                    })
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        let Some(v) = self.layout.view_mut(pane) else {
            return false;
        };
        if on {
            if v.meas == Some(id) || v.with.contains(&Pick::here(group)) {
                return false;
            }
            v.with.retain(|p| !brought.contains(p));
            if v.meas.is_none() && v.with.is_empty() {
                v.meas = Some(id);
            } else if !v.with.push(Pick::here(group)) {
                self.warn(format!(
                    "a pane draws at most {} curves: untick one first",
                    ac2_proto::model::layout::PaneContent::MAX_ENTRIES
                ));
                return false;
            }
        } else if v.meas == Some(id) {
            v.meas = None;
            if let Some(first) = v.with.as_slice().first().copied()
                && first.rig.is_none()
                && let EntryRef::Group { meas } = first.what
            {
                v.with.take_first();
                v.meas = Some(meas);
            }
        } else {
            v.with.retain(|p| *p != Pick::here(group));
        }
        self.rig.pending.pane_meas.remove(&pane);
        self.rig.pending.deleted.remove(&pane);
        true
    }

    /// Clear compare: the focused pane draws its lead alone again.
    pub(in crate::state) fn clear_compare(&mut self, _out: &mut Vec<Request>) {
        let pane = self.layout.focus;
        let lead = self
            .pane_meas(pane)
            .map_or_else(|| "this pane".to_owned(), |m| m.config.name.clone());
        let Some(v) = self.layout.view_mut(pane).filter(|v| v.kind.draws_curves()) else {
            self.toast("nothing compared: the focused pane shows one source");
            return;
        };
        let n = v.with.len();
        let text = match n {
            0 => format!("{lead}: nothing drawn behind it"),
            1 => format!("{lead}: compare cleared, 1 curve"),
            n => format!("{lead}: compare cleared, {n} curves"),
        };
        if v.meas.is_some() {
            v.with.clear();
        } else {
            // A pane led by one curve keeps it.
            let first = v.with.as_slice().first().copied();
            v.with = With::of(first.as_slice());
        }
        self.toast(text);
    }
}
