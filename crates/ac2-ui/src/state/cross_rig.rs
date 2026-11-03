//! Panes and the measurement list across rigs: the list shows every connected rig's rows
//! under its heading, any rig's row drags onto a pane, and a drop of another rig's sets the
//! pane on that rig; pins stay right as rigs come and go (a pin to the rig shown is its own
//! pane again, and measurement names wait by rig id for a rig not held); the layout prefs
//! name each pinned pane's rig.
//!
//! A pane is pinned by the rig's id, never by where it is reached: a rig reached on another
//! address (a new IP, network mode turned on) is the same rig, and its panes come back.

use super::*;
use crate::state::panes::drag::RowDrop;
use ac2_scene::meas_list::{Mark, TreeKey, TreeRow};

/// A rig not shown, as its part of the measurement list was last read: its rows, and what
/// each draggable row needs of a pane. Read with the rig swapped in, so between messages.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RigList {
    pub(crate) rig: RigId,
    pub(crate) rows: Vec<TreeRow>,
    pub(crate) drops: Vec<(TreeKey, RowDrop)>,
}

impl AppState {
    /// Reads each connected rig not shown's part of the list; none with one rig.
    pub(super) fn refresh_rig_lists(&mut self) {
        if self.visiting.is_some() {
            return;
        }
        if !self.many_rigs() {
            self.rig_lists.clear();
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
        let mut v = Vec::new();
        for id in others {
            if let Some(l) = self.visit(id, |st| st.own_rig_list()) {
                v.push(l);
            }
        }
        self.rig_lists = v;
    }

    /// The rig in `rig`'s part of the list: its rows (no row marked: the keys act on the
    /// shown rig's selection) and what its rows need of a pane.
    fn own_rig_list(&self) -> RigList {
        let mut rows = self.tree_rows();
        for r in &mut rows {
            r.mark = Mark::None;
        }
        let drops = rows
            .iter()
            .filter_map(|r| Some((r.key, self.row_drop(r.key)?)))
            .collect();
        RigList {
            rig: self.rig.id,
            rows,
            drops,
        }
    }

    /// The measurement list as drawn: every connected rig's rows under its heading, in the
    /// order the rigs were added (the drop-down's), each row with its rig. With one rig,
    /// its rows alone.
    pub fn list_rows(&self) -> Vec<(RigId, TreeRow)> {
        let shown = self.active();
        if self.rig_lists.is_empty() {
            return self.tree_rows().into_iter().map(|r| (shown, r)).collect();
        }
        let mut v = Vec::new();
        for r in self.rigs_in_order() {
            let rows = if r.id == shown {
                self.tree_rows()
            } else if let Some(l) = self.rig_lists.iter().find(|l| l.rig == r.id) {
                l.rows.clone()
            } else {
                continue;
            };
            v.push((
                r.id,
                ac2_scene::meas_list::rig_heading(r.id, &self.rig_label(r), r.id == shown),
            ));
            v.extend(rows.into_iter().map(|row| (r.id, row)));
        }
        v
    }

    /// Another rig's row as a drop, from its list as last read.
    pub(super) fn other_row_drop(&self, rig: RigId, key: TreeKey) -> Option<RowDrop> {
        self.rig_lists
            .iter()
            .find(|l| l.rig == rig)?
            .drops
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, d)| d.clone())
    }

    /// Pane `p` shows row `key` of rig `r`, a rig not shown: as a drop of the shown rig's,
    /// with that rig swapped in, and the pane is set on it.
    pub(super) fn pane_row_from(&mut self, p: PaneId, r: RigId, key: TreeKey) {
        let id = match key {
            TreeKey::Meas(id) | TreeKey::Live(id) | TreeKey::Math(id) => Some(id),
            TreeKey::Trace(_) => None,
            TreeKey::Imported | TreeKey::Rig | TreeKey::Session | TreeKey::RigName(_) => return,
        };
        if r == self.rig.id {
            match (id, key) {
                (Some(m), _) => self.pane_show(p, m),
                (None, TreeKey::Trace(t)) => self.pane_show_trace(p, t),
                _ => {}
            }
            return;
        }
        if self.visiting.is_some() {
            return;
        }
        self.forget_pane_waits(p);
        let shown = self
            .visit(r, |st| {
                match (id, key) {
                    (Some(m), _) => st.pane_show(p, m),
                    (None, TreeKey::Trace(t)) => st.pane_show_trace(p, t),
                    _ => {}
                }
                st.layout.view(p).is_some_and(|v| v.meas.is_some())
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
            v.with.clear();
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
            r.pending.deleted.remove(&p);
            r.pending.with_names.remove(&p);
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

    /// What graph pane view `v` draws behind its lead, as the preferences keep it: each entry
    /// by id with the name it has in its rig's mirror now (a measurement's, a trace's), else
    /// the name it was remembered by.
    pub(super) fn pane_pref_curves(&self, id: PaneId, v: &View) -> Vec<crate::prefs::CurvePrefs> {
        let remembered = |what: EntryRef| {
            self.prefs
                .layout
                .panes
                .as_ref()
                .and_then(|p| p.views.iter().find(|v| v.id == id))
                .and_then(|v| v.curves.iter().find(|c| c.what == what))
                .and_then(|c| c.name.clone())
        };
        let pane = self.pane_rig(v);
        v.with
            .as_slice()
            .iter()
            .map(|p| {
                let rig = p.rig.unwrap_or(pane);
                let r = self.find_rig(rig).filter(|r| r.daemon().is_some());
                let now = r.and_then(|r| {
                    let s = r.daemon()?;
                    match p.what {
                        EntryRef::Group { meas } | EntryRef::Live { meas } => s
                            .measurements
                            .iter()
                            .find(|m| m.id == meas)
                            .map(|m| m.config.name.clone()),
                        EntryRef::Trace { trace } => s
                            .traces
                            .iter()
                            .find(|t| t.id == trace)
                            .map(|t| t.edit.name.clone()),
                    }
                });
                crate::prefs::CurvePrefs {
                    what: p.what,
                    name: now.or_else(|| remembered(p.what)),
                    rig: p.rig.map(|r| (r, self.rig_name(r))),
                }
            })
            .collect()
    }
}
