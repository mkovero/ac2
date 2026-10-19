//! What the daemon keeps with a measurement about how it is drawn: a spectrum's style, peak
//! hold and spectrograph (B, P, Shift+B, colours), and the curves compared into its panes
//! (C). Every client draws them the same, and a session restores them.

use super::*;
use ac2_proto::model::{Colormap, Compared, SpectrumDisplay};
use ac2_scene::meas_list::{CompareTarget, compare_choices, compare_no_target, compare_toast};

impl AppState {
    /// How measurement `m`'s curves draw: its own display, a math channel its owner's (as it
    /// takes its owner's colours), the defaults for anything else.
    pub fn display_of(&self, m: &Measurement) -> SpectrumDisplay {
        if let Some(d) = m.config.kind.display() {
            return *d;
        }
        if let MeasKind::Math { config } = &m.config.kind
            && let Some(d) = config
                .owner
                .meas()
                .and_then(|id| self.meas(id))
                .and_then(|o| o.config.kind.display())
        {
            return *d;
        }
        SpectrumDisplay::default()
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
    pub(super) fn edit_display(
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
    pub(super) fn step_spectrum_style(&mut self, out: &mut Vec<Request>) {
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
    pub(super) fn step_peak_mode(&mut self, out: &mut Vec<Request>) {
        self.edit_display(
            |d| {
                d.peak = d.peak.next();
                d.peak.name().to_owned()
            },
            out,
        );
    }

    /// Shift+B: the next spectrograph span.
    pub(super) fn step_spectrograph_span(&mut self, out: &mut Vec<Request>) {
        self.edit_display(
            |d| {
                d.spectrograph_span_s = SpectrumDisplay::next_span(d.spectrograph_span_s);
                format!("spectrograph: last {} s", d.spectrograph_span_s)
            },
            out,
        );
    }

    /// The spectrograph colour map, stepped `d` places or set.
    pub(super) fn spectrograph_colours(
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

    /// The curves compared into the panes of `shown`, first compared first. A pane showing
    /// a math channel draws its owner's group, and so what is compared into its owner.
    pub fn compared_into<'a>(&'a self, shown: Option<&'a Measurement>) -> &'a [Compared] {
        let Some(m) = shown else {
            return &[];
        };
        match &m.config.kind {
            MeasKind::Math { config } => config
                .owner
                .meas()
                .and_then(|id| self.meas(id))
                .map_or(&[], |o| o.compared.as_slice()),
            _ => m.compared.as_slice(),
        }
    }

    /// Stacking layer of `c` on a pane showing `shown`: 1 for the first compared, 2 for the
    /// next…; 0 for what is not compared there (the pane's own group).
    pub fn compare_layer(&self, shown: Option<&Measurement>, c: Compared) -> u8 {
        self.compared_into(shown)
            .iter()
            .position(|x| *x == c)
            .map_or(0, |i| u8::try_from(i + 1).unwrap_or(u8::MAX))
    }

    /// The measurements a pane of one of `kinds` shows that are not of `group` (whose
    /// panes draw the curve as their own): those on screen first, each in list order. A
    /// math channel is no target: its panes draw its owner's group, compared into the owner.
    fn compare_targets(&self, group: TraceOwner, kinds: &[PaneKind]) -> Vec<(MeasId, bool)> {
        let on_screen: BTreeSet<MeasId> = self
            .layout
            .root
            .leaves()
            .into_iter()
            .filter_map(|p| self.pane_meas(p).map(|m| self.compare_owner(m)))
            .collect();
        let ms = self.measurements();
        let mut v: Vec<(MeasId, bool)> = ms
            .iter()
            .filter(|m| {
                !matches!(m.config.kind, MeasKind::Math { .. })
                    && match PaneKind::for_kind(&m.config.kind) {
                        // A sweep's runs are transfer functions: the transfer pane shows
                        // its group as the sweep pane does.
                        PaneKind::Distortion => kinds
                            .iter()
                            .any(|k| matches!(k, PaneKind::Distortion | PaneKind::Transfer)),
                        k => kinds.contains(&k),
                    }
                    && ac2_scene::meas_list::meas_group(m, &ms) != group
            })
            .map(|m| (m.id, on_screen.contains(&m.id)))
            .collect();
        v.sort_by_key(|(_, shown)| !*shown);
        v
    }

    /// The measurement whose compare list a pane showing `m` draws: `m`, or a math
    /// channel's owner.
    pub(super) fn compare_owner(&self, m: &Measurement) -> MeasId {
        match &m.config.kind {
            MeasKind::Math { config } => config.owner.meas().unwrap_or(m.id),
            _ => m.id,
        }
    }

    /// C: asks which measurement's panes draw the selected curve (a stored trace selected
    /// last, else the selected measurement or math channel). An imported trace is no
    /// measurement's, so it is not compared until moved under one; an SPL meter is a
    /// number, compared by putting two side by side.
    pub(super) fn open_compare(&mut self, keymap: &crate::keys::Keymap) {
        let ms = self.measurements();
        let (item, label, group, kinds, visible) =
            if let Some(t) = self.selected_trace_meta().cloned() {
                let label = trace_label(&t);
                let group = ac2_scene::meas_list::group_of(&t, &ms);
                if group == TraceOwner::Imported {
                    let key = keymap
                        .chords(
                            crate::keys::CommandId::MoveTrace,
                            crate::keys::Scope::Global,
                        )
                        .first()
                        .map(|c| c.label());
                    self.warn(ac2_scene::meas_list::imported_compare_hint(
                        &label,
                        key.as_deref(),
                    ));
                    return;
                }
                let kinds = display::trace_compare_kinds(&t);
                (
                    Compared::Trace { trace: t.id },
                    label,
                    group,
                    kinds,
                    t.edit.visible,
                )
            } else {
                let Some(m) = self.selected_meas().cloned() else {
                    self.warn(self.select_first("to compare"));
                    return;
                };
                let name = m.config.name.clone();
                if matches!(m.config.kind, MeasKind::Spl { .. }) {
                    self.warn(ac2_scene::meas_list::meter_compare_hint(&name));
                    return;
                }
                let kinds = display::meas_compare_kinds(&m);
                if kinds.is_empty() {
                    self.warn(format!(
                        "{name}: no live curve to compare — select one of its runs or traces"
                    ));
                    return;
                }
                let group = ac2_scene::meas_list::meas_group(&m, &ms);
                let visible = !self.meas_hidden(&m);
                (Compared::Meas { meas: m.id }, name, group, kinds, visible)
            };
        let targets = self.compare_targets(group, &kinds);
        if targets.is_empty() {
            let what = kinds.first().map_or("graph", |k| k.what());
            self.warn(compare_no_target(&label, what));
            return;
        }
        let on: Vec<bool> = targets
            .iter()
            .map(|(id, _)| self.meas(*id).is_some_and(|m| m.compared.contains(&item)))
            .collect();
        let purpose = ChoicePurpose::Compare {
            item,
            visible,
            targets: targets.iter().map(|(id, _)| *id).collect(),
            shown: targets.iter().map(|(_, s)| *s).collect(),
            on,
        };
        self.overlay = Overlay::Choose(Box::new(self.compare_prompt(label, purpose, 0)));
    }

    /// The chooser for `purpose` (a [`ChoicePurpose::Compare`]), `index` highlighted.
    fn compare_prompt(&self, label: String, purpose: ChoicePurpose, index: usize) -> ChoicePrompt {
        let rows: Vec<CompareTarget> = match &purpose {
            ChoicePurpose::Compare {
                targets, shown, on, ..
            } => targets
                .iter()
                .zip(shown)
                .zip(on)
                .map(|((id, shown), on)| CompareTarget {
                    name: self
                        .meas(*id)
                        .map_or_else(|| format!("measurement {id}"), |m| m.config.name.clone()),
                    shown: *shown,
                    on: *on,
                })
                .collect(),
            _ => Vec::new(),
        };
        let q = compare_choices(&label, &rows);
        ChoicePrompt {
            purpose,
            label,
            title: q.title,
            lines: q.lines,
            choices: q.choices,
            owners: Vec::new(),
            index,
            hint: q.hint,
        }
    }

    /// Row `i` of the compare chooser taken: the curve is compared into that measurement,
    /// or no longer; the chooser stays open with the row's tick changed.
    pub(super) fn compare_pick(&mut self, c: ChoicePrompt, i: usize, out: &mut Vec<Request>) {
        let ChoicePurpose::Compare {
            item,
            visible,
            targets,
            shown,
            mut on,
        } = c.purpose
        else {
            return;
        };
        let (Some(&target), Some(was)) = (targets.get(i), on.get(i).copied()) else {
            return;
        };
        on[i] = !was;
        self.set_compared(target, item, !was, &c.label, visible, out);
        let purpose = ChoicePurpose::Compare {
            item,
            visible,
            targets,
            shown,
            on,
        };
        self.overlay = Overlay::Choose(Box::new(self.compare_prompt(c.label, purpose, i)));
    }

    /// Compares `item` (named `label`) into measurement `target`'s panes, last, or takes
    /// it out; `visible`: its curve is shown (a hidden one is compared but not drawn).
    pub(super) fn set_compared(
        &mut self,
        target: MeasId,
        item: Compared,
        on: bool,
        label: &str,
        visible: bool,
        out: &mut Vec<Request>,
    ) {
        let Some(m) = self.meas(target) else {
            return;
        };
        let mut compared = m.compared.clone();
        compared.retain(|c| *c != item);
        if on {
            if compared.len() >= Compared::MAX {
                self.warn(format!(
                    "{}: at most {} curves are compared into one measurement",
                    m.config.name,
                    Compared::MAX
                ));
                return;
            }
            compared.push(item);
        }
        let what = compare_toast(label, &m.config.name, on, visible);
        self.call(
            out,
            Command::MeasCompare {
                meas: target,
                compared,
            },
            what,
        );
    }

    /// Clear compare: the focused pane's measurement draws its own group alone again.
    pub(super) fn clear_compare(&mut self, out: &mut Vec<Request>) {
        let Some(m) = self
            .pane_meas(self.layout.focus)
            .map(|m| self.compare_owner(m))
            .and_then(|id| self.meas(id))
            .cloned()
        else {
            self.toast("nothing compared");
            return;
        };
        let what = match m.compared.len() {
            0 => {
                self.toast(format!("{}: nothing compared", m.config.name));
                return;
            }
            1 => format!("{}: compare cleared, 1 curve", m.config.name),
            n => format!("{}: compare cleared, {n} curves", m.config.name),
        };
        self.call(
            out,
            Command::MeasCompare {
                meas: m.id,
                compared: Vec::new(),
            },
            what,
        );
    }
}
