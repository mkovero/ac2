//! Panes and the measurement list across rigs: the list is a tree rooted at the rig shown,
//! then at each rig that rig's daemon is paired with, a rig's rows under its heading (its
//! branch folds); a row of a paired rig drags onto a pane, and the drop sets the pane on
//! that rig; pins stay right as rigs come and go (a pin to the rig shown is its own pane
//! again, and measurement names wait by rig id for a rig not held); the layout prefs name
//! each pinned pane's rig.
//!
//! A rig this app holds that the rig shown is not paired with is reached through the
//! drop-down alone: the list is what the rig shown is connected to, so neither its rows
//! nor its curves are offered, and a pane left on it says so until it is shown or paired.
//!
//! A pane is pinned by the rig's id, never by where it is reached: a rig reached on another
//! address (a new IP, network mode turned on) is the same rig, and its panes come back.

use super::*;
use crate::state::panes::drag::RowDrop;
use ac2_proto::model::{RemoteLink, StandingRig};
use ac2_scene::clock_domain::{DomainTag, Levels, RigClockFacts, clock_domains};
use ac2_scene::meas_list::{Mark, TreeKey, TreeRow};
use ac2_scene::rig_tree::{
    Away, ClockTag, ClockVerdict, Heard, Pairing, RigAudio, RigRoot, rig_away_heading, rig_branch,
};

/// A rig not shown, as its part of the measurement list was last read: its rows, and what
/// each draggable row needs of a pane. Read with the rig swapped in, so between messages.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RigList {
    pub(crate) rig: RigId,
    pub(crate) rows: Vec<TreeRow>,
    pub(crate) drops: Vec<(TreeKey, RowDrop)>,
}

impl AppState {
    /// The rigs the rig shown's daemon is paired with, each by its id with the name it
    /// goes by: the rigs that daemon may take inputs from (its `remote.peers`, in its order),
    /// then the rigs this app holds whose daemon lists the rig shown among its own. A
    /// pairing is the two daemons', whichever side takes: either side shown lists the other.
    /// Both sides are read from the daemons' mirrored state, matched by rig id: a name is
    /// what a rig is called, never which rig it is.
    pub fn paired_rigs(&self) -> Vec<(RigId, String)> {
        let shown = self.active();
        let mut v: Vec<(RigId, String)> = Vec::new();
        let takes = self
            .find_rig(shown)
            .and_then(|r| r.daemon())
            .into_iter()
            .flat_map(|s| &s.remote.peers)
            .map(|p| match self.find_rig(p.rig) {
                Some(r) => (p.rig, self.rig_label(r)),
                None => (p.rig, p.name.clone()),
            });
        let gives = self
            .rigs_in_order()
            .into_iter()
            .filter(|r| {
                r.daemon()
                    .is_some_and(|s| s.remote.peers.iter().any(|p| p.rig == shown))
            })
            .map(|r| (r.id, self.rig_label(r)));
        for (id, name) in takes.chain(gives) {
            if id != shown && !v.iter().any(|(r, _)| *r == id) {
                v.push((id, name));
            }
        }
        v
    }

    /// Rig `r`'s rows and curves are within reach of the screen: it is the rig shown, or
    /// that rig's daemon is paired with it. The one rule for what the list offers, what a
    /// pane takes and what a pane draws of another rig.
    pub fn rig_in_reach(&self, r: RigId) -> bool {
        r == self.active() || self.paired_rigs().iter().any(|(id, _)| *id == r)
    }

    /// Reads the part of the list of each connected rig paired with the rig shown; none
    /// with one rig.
    pub(super) fn refresh_rig_lists(&mut self) {
        if self.others.visiting.is_some() {
            return;
        }
        if !self.many_rigs() {
            self.others.rig_lists.clear();
            return;
        }
        let others: Vec<RigId> = self
            .paired_rigs()
            .into_iter()
            .filter_map(|(id, _)| self.find_rig(id))
            .filter(|r| r.daemon().is_some())
            .filter(|r| matches!(r.conn, ConnState::Connected { .. }))
            .map(|r| r.id)
            .collect();
        let mut v = Vec::new();
        for id in others {
            if let Some(l) = self.visit(id, |st| st.own_rig_list()) {
                v.push(l);
            }
        }
        self.others.rig_lists = v;
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

    /// What rig `r`'s audio stream runs at, for its heading: the rate, else what the top
    /// bar says of a rig without one.
    fn rig_audio(r: &RigState) -> RigAudio {
        match r.daemon() {
            Some(s) => match &s.stream.open {
                Some(o) => RigAudio::Rate(o.sample_rate_hz),
                None if s.measurements.is_empty() => RigAudio::NoStream,
                None => RigAudio::NoAudio,
            },
            None => RigAudio::NoStream,
        }
    }

    /// The rig shown's daemon's standing relation with paired rig `id`: the clocks and the
    /// stream it hears in that rig's levels. None while the app is not connected to the
    /// rig shown: its mirror then holds what was, not what is.
    fn standing_with(shown: &RigState, id: RigId) -> Option<&StandingRig> {
        if !matches!(shown.conn, ConnState::Connected { .. }) {
            return None;
        }
        shown.daemon()?.remote.standing.iter().find(|s| s.rig == id)
    }

    /// Rig `r`'s daemon's wall time at the app's `client_now`, by the offset the link to it
    /// estimated; `None` before one is known.
    pub(super) fn rig_now(
        r: &RigState,
        client_now: ac2_proto::units::WallNs,
    ) -> Option<ac2_proto::units::WallNs> {
        let offset = r.mirror.as_ref()?.clock_offset_ns?;
        let t = i128::from(client_now.0) + offset;
        Some(ac2_proto::units::WallNs(
            t.clamp(0, i128::from(u64::MAX)) as u64
        ))
    }

    /// A paired rig's stream as its levels have it, for a rig this app does not read; `None`
    /// while the link between the daemons is down, when nothing is heard of it.
    fn heard_audio(s: &StandingRig) -> Option<RigAudio> {
        (s.link == RemoteLink::Up).then(|| {
            s.stream
                .as_ref()
                .map_or(RigAudio::NoStream, |x| RigAudio::Rate(x.sample_rate_hz))
        })
    }

    /// The clock domain of each rig listed, the rig shown first and then `paired` in order,
    /// judged together: a tag says which of the others a rig agrees with. A rig this app
    /// does not read (not held, or its link down) has the stream the rig shown's daemon
    /// hears in its levels, else none: the app then knows neither its rate nor its clocks,
    /// so it gets no tag and no other rig is compared with it.
    fn clock_tags(&self, shown: &RigState, paired: &[(RigId, String)]) -> Vec<DomainTag> {
        let unread = ac2_proto::model::Clocks::default();
        // Each daemon's marks age on its own clock.
        let client_now = super::sessions_impl::wall_now();
        let read = |id: RigId| {
            let r = self
                .find_rig(id)
                .filter(|r| matches!(r.conn, ConnState::Connected { .. }))?;
            Some((r.daemon()?, Self::rig_now(r, client_now)))
        };
        let names: Vec<String> = std::iter::once(self.rig_label(shown))
            .chain(paired.iter().map(|(_, n)| n.clone()))
            .collect();
        let facts: Vec<RigClockFacts<'_>> = std::iter::once(shown.id)
            .chain(paired.iter().map(|(id, _)| *id))
            .zip(&names)
            .map(|(id, name)| match read(id) {
                Some((s, now)) => RigClockFacts {
                    id: Some(id),
                    name,
                    rate_hz: s.stream.open.as_ref().map(|o| o.sample_rate_hz),
                    clocks: &s.clocks,
                    between: &s.remote.rigs,
                    levels: Some(Levels {
                        remote: &s.remote,
                        now,
                    }),
                },
                None => RigClockFacts {
                    id: Some(id),
                    name,
                    rate_hz: Self::standing_with(shown, id)
                        .and_then(Self::heard_audio)
                        .and_then(|a| match a {
                            RigAudio::Rate(hz) => Some(hz),
                            RigAudio::NoStream | RigAudio::NoAudio => None,
                        }),
                    clocks: &unread,
                    between: &[],
                    levels: None,
                },
            })
            .collect();
        clock_domains(&facts).rigs
    }

    /// Rig `r`'s branch of the list over its rows `rows`; `pairing` for a rig not shown,
    /// `clock` its tag among the rigs listed.
    fn rig_rows(
        &self,
        r: &RigState,
        rows: Vec<TreeRow>,
        pairing: Option<&Pairing>,
        clock: &DomainTag,
    ) -> Vec<TreeRow> {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let (measurements, traces) = r.daemon().map_or((0, 0), |s| {
            (count(s.measurements.len()), count(s.traces.len()))
        });
        let name = self.rig_label(r);
        let root = RigRoot {
            rig: r.id,
            name: &name,
            shown: pairing.is_none(),
            folded: self.rig_folded(r.id),
            measurements,
            traces,
            audio: Self::rig_audio(r),
            clock: clock
                .tag
                .as_deref()
                .zip(clock.tip.as_deref())
                .map(|(tag, tip)| ClockTag { tag, tip }),
            pairing,
        };
        rig_branch(&root, rows)
    }

    /// The measurement list as drawn: the rig shown's root first, then a root per rig its
    /// daemon is paired with, in that daemon's order, and under each (unfolded) that rig's
    /// rows, each row with its rig. One rig is a root too, so the list always says whose
    /// rows it lists; a lone rig with no daemon yet has nothing to list. A paired rig this
    /// app does not read (not connected to it, or its link down) is its heading alone: the
    /// pairing is the rig shown's to report, the measurements are not.
    pub fn list_rows(&self) -> Vec<(RigId, TreeRow)> {
        let mut v = Vec::new();
        let Some(shown) = self.find_rig(self.active()) else {
            return v;
        };
        let paired = self.paired_rigs();
        if paired.is_empty() && shown.daemon().is_none() {
            return v;
        }
        let clocks = self.clock_tags(shown, &paired);
        let own = self.rig_rows(shown, self.tree_rows(), None, &clocks[0]);
        v.extend(own.into_iter().map(|row| (shown.id, row)));
        let remote = shown.daemon().map(|s| &s.remote);
        let inputs = shown
            .daemon()
            .and_then(|s| s.stream.open.as_ref())
            .map_or(&[][..], |o| &o.config.remote_inputs[..]);
        for ((id, name), clock) in paired.into_iter().zip(&clocks[1..]) {
            // One verdict on a heading: a rig with a clock tag says the clocks there.
            let verdict = match clock.tag {
                Some(_) => ClockVerdict::OnTag,
                None => ClockVerdict::InWords,
            };
            let standing = Self::standing_with(shown, id);
            let pairing = match remote.map_or(Pairing::Plain, |r| {
                ac2_scene::rig_tree::pairing_text(r, inputs, id, verdict)
            }) {
                // No input from it: the link the levels come over is what the pairing says.
                Pairing::Plain => standing
                    .and_then(|s| Pairing::link(&self.rig_label(shown), &name, &s.link))
                    .unwrap_or(Pairing::Plain),
                p => p,
            };
            let heard = standing.and_then(Self::heard_audio).map(|audio| Heard {
                audio,
                clock: clock
                    .tag
                    .as_deref()
                    .zip(clock.tip.as_deref())
                    .map(|(tag, tip)| ClockTag { tag, tip }),
            });
            let held = self.find_rig(id);
            let list = self.others.rig_lists.iter().find(|l| l.rig == id);
            match (held, list) {
                (Some(r), Some(l)) => v.extend(
                    self.rig_rows(r, l.rows.clone(), Some(&pairing), clock)
                        .into_iter()
                        .map(|row| (id, row)),
                ),
                (Some(r), None) => {
                    let name = self.rig_label(r);
                    v.push((
                        id,
                        rig_away_heading(id, &name, &pairing, Away::Unreachable, heard),
                    ));
                }
                (None, _) => v.push((
                    id,
                    rig_away_heading(id, &name, &pairing, Away::NotConnected, heard),
                )),
            }
        }
        v
    }

    /// Rig `r`'s branch of the measurement list is folded.
    pub fn rig_folded(&self, r: RigId) -> bool {
        self.prefs.folded_rigs.contains(&r)
    }

    /// Folds or unfolds rig `r`'s branch of the list; remembered in the prefs by rig id.
    pub(super) fn toggle_rig_fold(&mut self, r: RigId) {
        if !self.prefs.folded_rigs.remove(&r) {
            self.prefs.folded_rigs.insert(r);
        }
        self.prefs_dirty = true;
    }

    /// Another rig's row as a drop, from its list as last read: none for a rig the rig
    /// shown is not paired with, whose list is not read.
    pub(super) fn other_row_drop(&self, rig: RigId, key: TreeKey) -> Option<RowDrop> {
        self.others
            .rig_lists
            .iter()
            .find(|l| l.rig == rig)?
            .drops
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, d)| d.clone())
    }

    /// Pane `p` shows row `key` of rig `r`, a rig not shown: as a drop of the shown rig's,
    /// with that rig swapped in, and the pane is set on it. Refused for a rig out of reach
    /// ([`AppState::rig_in_reach`]).
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
        if self.others.visiting.is_some() || !self.rig_in_reach(r) {
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
        if self.others.visiting.is_some() || self.find_rig(r).is_none() || !self.rig_in_reach(r) {
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
        for r in std::iter::once(&mut self.rig).chain(&mut self.others.background) {
            r.pending.pane_meas.remove(&p);
            r.pending.deleted.remove(&p);
            r.pending.with_names.remove(&p);
        }
        for w in self.others.rig_pending.values_mut() {
            w.remove(&p);
        }
        self.others.rig_pending.retain(|_, w| !w.is_empty());
    }

    /// Between messages: a pane pinned to the rig shown is its own pane (a rig remembered
    /// with its panes, shown before it said hello), and measurement names waiting for a rig
    /// go to it once it is held under that id.
    pub(super) fn settle_pins(&mut self) {
        if self.others.visiting.is_some() {
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
        if self.others.rig_pending.is_empty() {
            return;
        }
        for r in std::iter::once(&mut self.rig).chain(&mut self.others.background) {
            if let Some(w) = self.others.rig_pending.remove(&r.id) {
                r.pending.pane_meas.extend(w);
            }
        }
    }

    /// Rig `r` was closed: each of its panes keeps its measurement by name, to show it again
    /// when the rig is back (reached anywhere, its daemon maybe restarted with other ids).
    pub(super) fn park_closed_rig(&mut self, r: &RigState) {
        let mut waiting: BTreeMap<PaneId, String> = BTreeMap::new();
        let kept = self
            .others
            .screens
            .values_mut()
            .flat_map(|s| s.layout.views.iter_mut());
        for (pane, v) in self.layout.views.iter_mut().chain(kept) {
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
            self.others
                .rig_pending
                .entry(r.id)
                .or_default()
                .extend(waiting);
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
                .others
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
