//! Panes across rigs: a pane's list offers every connected rig's measurements and meters,
//! and a pick of another rig's sets the pane on that rig; pins stay right as rigs come and
//! go (a pin to the rig shown is its own pane again, and measurement names wait by rig id
//! for a rig not held); the layout prefs name each pinned pane's rig.
//!
//! A pane is pinned by the rig's id, never by where it is reached: a rig reached on another
//! address (a new IP, network mode turned on) is the same rig, and its panes come back.

use super::*;

/// One rig's rows of a pane's list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MenuRig {
    pub(crate) rig: RigId,
    /// Its measurements, in its tree's order.
    pub(crate) meas: Vec<(PaneMenuRow, String)>,
    /// What it can meter: its session's pairs and inputs.
    pub(crate) meters: Vec<(PaneMenuRow, String)>,
    /// The kinds of pane its measurements bring.
    pub(crate) brought: Vec<PaneKind>,
}

impl AppState {
    /// Reads each connected rig's rows for the pane list open, the rig shown first; none
    /// with one rig or no list open. A rig is read with it swapped in, so only between
    /// messages.
    pub(super) fn refresh_menu_rigs(&mut self) {
        if self.visiting.is_some() {
            return;
        }
        if !self.many_rigs() || !matches!(self.overlay, Overlay::PaneMenu(_)) {
            self.menu_rigs.clear();
            return;
        }
        let shown = self.rig.id;
        let others: Vec<RigId> = self
            .rigs_in_order()
            .into_iter()
            .filter(|r| r.id != shown && r.daemon().is_some())
            .filter(|r| matches!(r.conn, ConnState::Connected { .. }))
            .map(|r| r.id)
            .collect();
        let mut v = vec![self.own_menu_rows()];
        for id in others {
            if let Some(m) = self.visit(id, |st| st.own_menu_rows()) {
                v.push(m);
            }
        }
        self.menu_rigs = v;
    }

    /// The rows of every connected rig, each under its heading, and the kinds of pane the
    /// shown rig's measurements bring. Another rig's rows name it, so a pick sets the pane on
    /// that rig whichever rig handles the pick.
    pub(super) fn every_rig_menu_rows(&self) -> (Vec<(PaneMenuRow, String)>, Vec<PaneKind>) {
        let shown = self.active();
        let mut v = Vec::new();
        let mut brought = Vec::new();
        for m in &self.menu_rigs {
            v.push((PaneMenuRow::Heading(m.rig), self.rig_name(m.rig)));
            let tag = |r: PaneMenuRow| match r {
                _ if m.rig == shown => r,
                PaneMenuRow::Meas(x) => PaneMenuRow::RigMeas(m.rig, x),
                PaneMenuRow::Meter(x) => PaneMenuRow::RigMeter(m.rig, x),
                other => other,
            };
            v.extend(
                m.meas
                    .iter()
                    .chain(&m.meters)
                    .map(|(r, t)| (tag(*r), t.clone())),
            );
            if m.rig == shown {
                brought.clone_from(&m.brought);
            }
        }
        (v, brought)
    }

    /// The row of pane `p`'s list that is what it shows: the shown rig's measurement, or
    /// another rig's measurement or meters it is set on.
    pub(super) fn pane_menu_shown(&self, p: PaneId) -> Option<PaneMenuRow> {
        let v = self.layout.view(p)?;
        match v.rig {
            PaneRig::Active => self.pane_meas(p).map(|m| PaneMenuRow::Meas(m.id)),
            PaneRig::Rig(r) => v
                .meas
                .map(|m| PaneMenuRow::RigMeas(r, m))
                .or_else(|| v.meters.map(|m| PaneMenuRow::RigMeter(r, m))),
        }
    }

    /// Pane `p` shows measurement `m` of rig `r`, a rig not shown: as a pick of the shown
    /// rig's, with that rig swapped in, and the pane is set on it.
    pub(super) fn pane_show_from(&mut self, p: PaneId, r: RigId, m: MeasId) {
        if r == self.rig.id {
            self.pane_show(p, m);
            return;
        }
        if self.visiting.is_some() {
            return;
        }
        self.forget_pane_waits(p);
        let shown = self
            .visit(r, |st| {
                st.pane_show(p, m);
                st.layout.view(p).is_some_and(|v| v.meas == Some(m))
            })
            .unwrap_or(false);
        if shown && let Some(v) = self.layout.view_mut(p) {
            v.rig = PaneRig::Rig(r);
            v.meters = None;
        }
    }

    /// Pane `p` meters inputs or a pair `m` of rig `r`, a rig not shown: a Programme pane set
    /// on that rig, on its meters.
    pub(super) fn pane_meter_from(&mut self, p: PaneId, r: RigId, m: Metered) {
        if self.visiting.is_some() || self.find_rig(r).is_none() {
            return;
        }
        self.forget_pane_waits(p);
        self.set_pane_kind(p, PaneKind::Programme);
        if let Some(v) = self.layout.view_mut(p) {
            v.meas = None;
            v.meters = Some(m);
            v.rig = if r == self.rig.id {
                PaneRig::Active
            } else {
                PaneRig::Rig(r)
            };
        }
        self.focus_pane(p);
    }

    /// Pane `p` was given something else: no rig's remembered measurement waits for it.
    fn forget_pane_waits(&mut self, p: PaneId) {
        for r in std::iter::once(&mut self.rig).chain(&mut self.background) {
            r.pending.pane_meas.remove(&p);
        }
        for w in self.rig_pending.values_mut() {
            w.remove(&p);
        }
        self.rig_pending.retain(|_, w| !w.is_empty());
    }

    /// Between messages: a pane pinned to the rig shown is its own pane (a rig remembered
    /// with its panes, shown before it said hello), and measurement names waiting for a rig
    /// go to it once it is held under that id.
    pub(super) fn settle_pins(&mut self) {
        if self.visiting.is_some() {
            return;
        }
        let shown = PaneRig::Rig(self.rig.id);
        if self.layout.views.values().any(|v| v.rig == shown) {
            let unpin = |l: &mut Layout| {
                for v in l.views.values_mut() {
                    if v.rig == shown {
                        v.rig = PaneRig::Active;
                    }
                }
            };
            unpin(&mut self.layout);
            self.history.layouts_mut(unpin);
        }
        if self.rig_pending.is_empty() {
            return;
        }
        for r in std::iter::once(&mut self.rig).chain(&mut self.background) {
            if let Some(w) = self.rig_pending.remove(&r.id) {
                r.pending.pane_meas.extend(w);
            }
        }
    }

    /// Rig `r` was closed: each of its panes keeps its measurement by name, to show it again
    /// when the rig is back (reached anywhere, its daemon maybe restarted with other ids).
    pub(super) fn park_closed_rig(&mut self, r: &RigState) {
        let mut waiting: BTreeMap<PaneId, String> = BTreeMap::new();
        for (pane, v) in &mut self.layout.views {
            if v.rig != PaneRig::Rig(r.id) {
                continue;
            }
            if let Some(n) = r.pending.pane_meas.get(pane) {
                waiting.insert(*pane, n.clone());
            }
            if let Some(m) = v.meas.take()
                && let Some(m) = r.meas(m)
            {
                waiting.insert(*pane, m.config.name.clone());
            }
        }
        if !waiting.is_empty() {
            self.rig_pending.entry(r.id).or_default().extend(waiting);
        }
    }

    /// What the layout prefs keep as pane `id`'s measurement: its name on the pane's own
    /// rig, the one waiting for it, or (its rig not known yet) what the prefs said; `None`
    /// for a pane following the selection.
    pub(super) fn pane_pref_meas(&self, id: PaneId, v: &View) -> Option<String> {
        let remembered = || {
            self.prefs
                .layout
                .panes
                .as_ref()
                .and_then(|p| p.views.iter().find(|v| v.id == id))
                .and_then(|v| v.measurement.clone())
        };
        let rig = self.pane_rig(v);
        let Some(r) = self.find_rig(rig) else {
            return self
                .rig_pending
                .get(&rig)
                .and_then(|w| w.get(&id))
                .cloned()
                .or_else(remembered);
        };
        if let Some(n) = r.pending.pane_meas.get(&id) {
            return Some(n.clone());
        }
        if r.daemon().is_none() {
            return remembered();
        }
        let m = v.meas?;
        if r.id == self.rig.id {
            return self.pane_meas(id).map(|m| m.config.name.clone());
        }
        r.meas(m).map(|m| m.config.name.clone())
    }

    /// The rig the layout prefs keep for a pane: none for one following the rig shown, else
    /// the pinned rig's id and name.
    pub(super) fn pane_pref_rig(&self, v: &View) -> Option<(RigId, String)> {
        match v.rig {
            PaneRig::Active => None,
            PaneRig::Rig(r) => Some((r, self.rig_name(r))),
        }
    }
}
