//! The remembered pane layout, the preferences, the session watch and the key hints.

use crate::state::*;

impl AppState {
    // ----- reducer -----------------------------------------------------------------------

    /// Once the daemon's state is known, each pane shows the measurement it showed when the
    /// app last ran (or before the daemon lost it), once one of that name (and kind) exists:
    /// meanwhile the pane's usual choice, quietly; a name the daemon never listed is dropped. With no layout remembered, the one pane shows the kind of the first
    /// measurement (a transfer pane while there is none).
    pub(in crate::state) fn restore_pane_meas(&mut self) {
        // A layout the operator changed is theirs from then on.
        let fresh = Layout::default();
        if self.layout.root != fresh.root || self.layout.views != fresh.views {
            self.pane_auto = false;
        }
        if self.daemon().is_none() {
            return;
        }
        // The first pane shows what the daemon measures: the kind of its first measurement.
        if self.pane_auto
            && let Some(kind) = self
                .measurements()
                .first()
                .map(|m| PaneKind::for_kind(&m.config.kind))
        {
            self.pane_auto = false;
            let f = self.layout.focus;
            self.set_pane_kind(f, kind);
        }
        if self.rig.pending.pane_meas.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.rig.pending.pane_meas);
        for (pane, name) in pending {
            // Given another measurement meanwhile: the pane's choice now.
            if self.layout.view(pane).is_none_or(|v| v.meas.is_some()) {
                continue;
            }
            let kind = self.layout.kind(pane);
            let id = self
                .measurements()
                .iter()
                .find(|m| m.config.name == name && kind.shows(&m.config.kind))
                .map(|m| m.id);
            let Some(id) = id else {
                // Gone for now (a session reopening, salvaged, a daemon restarting makes it
                // again), or none made yet: it comes back. One the daemon never listed is
                // not coming: the pane's usual choice is it, and nothing is saved for it.
                if self.rig.meas_seen.contains(&name) || self.measurements().is_empty() {
                    self.rig.pending.pane_meas.insert(pane, name);
                }
                continue;
            };
            if let Some(v) = self.layout.view_mut(pane) {
                v.meas = Some(id);
            }
            // The focused pane's measurement is the selection, unless the operator chose
            // another since.
            if self.layout.focus == pane
                && (self.rig.selected.is_none() || self.selection_is_standin())
            {
                self.select_on(pane, id);
                // Still this app's pick: a viewer nobody touches neither takes this
                // measurement's own panes nor gives it its own.
                self.rig.standin = Some(id);
            }
        }
    }

    /// The layout as it is now, as the preferences keep it.
    pub fn layout_prefs(&self) -> crate::prefs::LayoutPrefs {
        let remembered = |id: PaneId| {
            self.prefs
                .layout
                .panes
                .as_ref()
                .and_then(|p| p.views.iter().find(|v| v.id == id))
                .and_then(|v| v.measurement.clone())
        };
        // Unknown until the daemon's state is (and while a remembered one waits for it):
        // keep what the preferences say. No layout of the operator's yet: none is kept, and
        // neither are its panes' level axes (an automatic fit or a zoom there is no layout
        // choice: kept, the panes would stop taking the first measurement's kind).
        let panes = (!self.pane_auto).then(|| crate::prefs::PanesPrefs {
            root: self.layout.root.clone(),
            focus: self.layout.focus,
            views: self
                .layout
                .root
                .leaves()
                .into_iter()
                .filter_map(|id| {
                    let v = self.layout.view(id)?;
                    let measurement = match (self.rig.pending.pane_meas.get(&id), self.daemon()) {
                        (Some(n), _) => Some(n.clone()),
                        (None, None) => remembered(id),
                        // A pane following the selection keeps following it: what it
                        // resolves to now is not its choice.
                        (None, Some(_)) if v.meas.is_none() => None,
                        (None, Some(_)) => self.pane_meas(id).map(|m| m.config.name.clone()),
                    };
                    Some(crate::prefs::PanePrefs {
                        id,
                        kind: v.kind,
                        measurement,
                        meters: v.meters,
                        modes: v.modes,
                        levels: self.operator_levels(id),
                        plates: v.plates,
                    })
                })
                .collect(),
        });
        let hidden = self.remembered_names(&self.rig.hidden_meas, &self.rig.pending.hidden);
        crate::prefs::LayoutPrefs {
            panes,
            maximized: self.layout.maximized,
            fullscreen: self.fullscreen,
            panes_only: self.panes_only,
            distortion_unit: self.view.distortion.unit,
            hidden,
        }
    }

    /// The names `ui.toml` keeps for the measurements `ids` (hidden) and the
    /// names still waiting in `pending`. A name only when it is that measurement's alone: a
    /// name several measurements share would apply to none of them in the next run
    /// ([`Self::restore_meas_choices`]), and kept, it would come to mean whichever of them
    /// was left once the others went. A deleted one's name must not hide a new one of that
    /// name in a later run, so a waiting name is kept only while the daemon lists no
    /// measurements at all (a session reopening): until the daemon's state is known, all.
    fn remembered_names(
        &self,
        ids: &BTreeSet<MeasId>,
        pending: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        if self.daemon().is_none() {
            return pending.clone();
        }
        let ms = self.measurements();
        let mut names: BTreeSet<String> = ms
            .iter()
            .filter(|m| ids.contains(&m.id))
            .map(|m| &m.config.name)
            .filter(|n| ms.iter().filter(|o| o.config.name == **n).count() == 1)
            .cloned()
            .collect();
        if ms.is_empty() {
            names.extend(pending.iter().cloned());
        }
        names
    }

    /// Hidden measurements remembered by name (`ui.toml`, or ones the daemon took away)
    /// become ids once the daemon lists a measurement of that name; after that, a rename
    /// keeps the choice on the measurement and a new one of an old name starts shown.
    ///
    /// A name several measurements share applies to none of them, and is forgotten: the
    /// name cannot say which was meant, and a hide is the riskier guess (a curve missing
    /// from the plot reads as a measurement not working; one shown too many is seen and
    /// hidden again with one key). Forgotten rather than kept waiting, so it never lands
    /// later on whichever duplicate is left after the others are deleted or renamed. A
    /// name no measurement has waits as a pane's remembered measurement does: while the
    /// daemon lists none, or once listed (it comes back with a session reopening).
    pub(in crate::state) fn restore_meas_choices(&mut self) {
        if self.daemon().is_none() || self.rig.pending.hidden.is_empty() {
            return;
        }
        let ms: Vec<(MeasId, String)> = self
            .measurements()
            .iter()
            .map(|m| (m.id, m.config.name.clone()))
            .collect();
        let seen = &self.rig.meas_seen;
        let resolve = |pending: &mut BTreeSet<String>, ids: &mut BTreeSet<MeasId>| {
            pending.retain(|name| {
                let mut of_name = ms.iter().filter(|(_, n)| n == name).map(|(id, _)| *id);
                match (of_name.next(), of_name.next()) {
                    (Some(id), None) => {
                        ids.insert(id);
                        false
                    }
                    (Some(_), Some(_)) => false,
                    (None, _) => ms.is_empty() || seen.contains(name),
                }
            });
        };
        resolve(&mut self.rig.pending.hidden, &mut self.rig.hidden_meas);
    }

    /// Saves the layout (with each pane's level axes), the IR axes and the legend with the
    /// preferences when they changed.
    pub(in crate::state) fn remember_layout(&mut self) {
        self.prune_levels();
        let now = self.layout_prefs();
        if now != self.prefs.layout {
            self.prefs.layout = now;
            self.prefs_dirty = true;
        }
        let levels = crate::prefs::IrLevelPrefs::of(&self.view);
        if levels != self.prefs.ir_levels {
            self.prefs.ir_levels = levels;
            self.prefs_dirty = true;
        }
        let legend = crate::prefs::LegendPrefs::of(&self.view.tf.legend);
        if legend != self.prefs.legend {
            self.prefs.legend = legend;
            self.prefs_dirty = true;
        }
    }

    /// What needs metering: the session's input meters while a session is open (the
    /// sidebar shows them all the time) or a dialog meters inputs, and a device preview (the
    /// session dialog on a device the session does not capture).
    pub(in crate::state) fn meter_wants(
        &self,
    ) -> (
        bool,
        Option<(ac2_proto::model::BackendKind, ac2_proto::model::DeviceId)>,
    ) {
        let session = self.open_session().is_some();
        match &self.overlay {
            _ if self.overlay.session().is_some() => (
                true,
                self.overlay.session().and_then(|d| d.preview_target()),
            ),
            Overlay::Form(_) => (true, None),
            _ => (session, None),
        }
    }

    /// Follows what the always-on parts of the window need from the mirror: the device
    /// list once per session (the inputs' channel names), and when the running sweep's
    /// step began.
    pub(in crate::state) fn sync_session_watch(&mut self, out: &mut Vec<Request>) {
        let epoch = self
            .daemon()
            .filter(|s| s.session.open.is_some())
            .map(|s| s.session.epoch);
        if let Some(e) = epoch
            && self.rig.devices.is_none()
            && self.rig.devices_for != Some(e)
            && self.connected()
        {
            self.rig.devices_for = Some(e);
            if !matches!(self.overlay, Overlay::Settings(_) | Overlay::Form(_)) {
                out.push(Request::Devices);
            }
        }
        let run = self
            .daemon()
            .and_then(|s| s.sweep.as_ref())
            .filter(|r| r.active())
            .map(|r| (r.id, r.status.clone()));
        match run {
            None => self.rig.sweep.step_seen = None,
            Some((id, status)) => {
                let same = self
                    .rig
                    .sweep
                    .step_seen
                    .as_ref()
                    .is_some_and(|(i, s, _)| *i == id && *s == status);
                if !same {
                    self.rig.sweep.step_seen = Some((id, status, self.now_s));
                }
            }
        }
    }

    /// Takes the preferences read at startup, and the view choices they hold: the layout as
    /// last left (the measurements each pane showed come back once the daemon's state is
    /// known). Nothing here arms or plays anything.
    pub fn set_prefs(&mut self, prefs: UiPrefs) {
        self.view.spl.layout = prefs.leq;
        let l = &prefs.layout;
        self.rig.pending.pane_meas.clear();
        self.pane_levels.clear();
        self.level_set.clear();
        self.spectrum_hold.clear();
        match &l.panes {
            Some(p) => {
                let views = p
                    .views
                    .iter()
                    .map(|v| {
                        (
                            v.id,
                            View {
                                kind: v.kind,
                                meas: None,
                                meters: v.meters,
                                modes: v.modes,
                                plates: v.plates,
                            },
                        )
                    })
                    .collect();
                self.layout = Layout::of(p.root.clone(), views, p.focus);
                for v in &p.views {
                    self.set_operator_levels(v.id, &v.levels);
                    if let Some(n) = &v.measurement
                        && self.layout.views.contains_key(&v.id)
                    {
                        self.rig.pending.pane_meas.insert(v.id, n.clone());
                    }
                }
                self.pane_auto = false;
            }
            None => {
                self.layout = Layout::default();
                self.pane_auto = true;
            }
        }
        self.layout.maximized = l.maximized;
        self.fullscreen = l.fullscreen;
        self.panes_only = l.fullscreen && l.panes_only;
        self.view.distortion.unit = l.distortion_unit;
        // Before any frame: a spectrum that starts still fits its axis on its first one.
        prefs.ir_levels.apply(&mut self.view);
        prefs.legend.apply(&mut self.view.tf.legend);
        self.view.unresolved = prefs.resolution_marker;
        self.view.jump_prominence_db = prefs.jump_prominence_db;
        self.rig.hidden_meas.clear();
        self.rig.pending.hidden = l.hidden.clone();
        self.prefs = prefs;
        self.restore_meas_choices();
    }

    /// W: the split layout → the focused pane alone → the focused pane full screen (the
    /// window too) → the split layout again. In F's panes-only full screen, W steps between
    /// all the panes and the focused one alone, staying full screen: leaving full screen is
    /// F's.
    pub(in crate::state) fn cycle_layout(&mut self) {
        if self.panes_only {
            self.layout.maximized = !self.layout.maximized;
        } else if !self.layout.maximized {
            self.layout.maximized = true;
        } else if !self.fullscreen {
            self.fullscreen = true;
        } else {
            self.layout.maximized = false;
            self.fullscreen = false;
        }
    }

    /// F: windowed → the window full screen → the panes alone full screen (every pane of
    /// the layout, no top bar or lists) → windowed. With one pane alone (W) the window full
    /// screen already shows the pane alone, so F leaves full screen from there.
    pub(in crate::state) fn cycle_fullscreen(&mut self) {
        if !self.fullscreen {
            self.fullscreen = true;
            self.panes_only = false;
        } else if !self.panes_only && !self.layout.maximized {
            self.panes_only = true;
        } else {
            self.fullscreen = false;
            self.panes_only = false;
        }
    }

    /// The stage view: the focused pane alone with the window full screen, so the screen
    /// holds only the pane's picture (no top bar, list or pane title; the SPL meter and its
    /// Leq windows read across a room). Full screen is the operator's explicit choice to see
    /// only the pane: the stimulus arming, playing or stopping and a sweep running change
    /// nothing on it (no top bar, strip or badge comes back, no pane resizes), and Esc and
    /// Shift+Esc stop as everywhere. Outside full screen the top bar shows the stimulus.
    pub fn stage_view(&self) -> bool {
        self.fullscreen && (self.layout.maximized || self.panes_only)
    }

    /// Whether the panes show key hints now: on in the preferences, and never in the stage
    /// view (the audience sees the windows alone).
    pub fn key_hints_shown(&self) -> bool {
        self.prefs.key_hints && !self.stage_view()
    }

    /// The hint line of `pane`: its most used keys as bound in `keymap`, written in `style`,
    /// then the help key. Only the focused pane has one, and only while hints are shown.
    pub fn key_hint_line(
        &self,
        keymap: &Keymap,
        pane: PaneId,
        style: crate::keys::LabelStyle,
    ) -> Option<Vec<crate::hints::KeyHint>> {
        if !self.key_hints_shown() || self.layout.focus != pane {
            return None;
        }
        Some(self.pane_hints(keymap, pane, style))
    }

    /// Every hint of `pane` (the title's tooltip lists them whether or not the line shows).
    pub fn pane_hints(
        &self,
        keymap: &Keymap,
        id: PaneId,
        style: crate::keys::LabelStyle,
    ) -> Vec<crate::hints::KeyHint> {
        let pane = self.layout.kind(id);
        let scope = self.layout.view(id).map_or(pane.scope(), |v| v.scope());
        crate::hints::line(keymap, scope, style)
    }
}
