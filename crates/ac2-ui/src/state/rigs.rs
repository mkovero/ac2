//! Many rigs: which rig a message is for, handling one for a rig in the background, the
//! rig shown and switching it, closing a rig, and whose panes are whose.
//!
//! Each rig's daemon numbers its measurements, traces and inputs on its own, so one rig's
//! ids name nothing (or something else) on another. A pane says whose content it shows
//! ([`PaneRig`]), and everything that resolves a pane's ids does it against that rig alone:
//! a message for a rig in the background swaps that rig in as `rig` for its handling
//! ([`AppState::visit`]), and the pane queries take only the panes of the rig in `rig`
//! ([`AppState::ours`]).

use super::*;

/// A rig in the background swapped in as `rig` while one message is handled for it.
pub(crate) struct Visit {
    /// Where the shown rig waits meanwhile, in `background`.
    index: usize,
    /// The shown rig's window: a window acts on the shown rig only.
    overlay: Overlay,
    window_rig: Option<RigId>,
}

impl AppState {
    /// The rig shown (the drop-down's): the one keys and windows act on.
    pub fn active(&self) -> RigId {
        self.visiting.unwrap_or(self.rig.id)
    }

    /// Every rig this app holds, the shown one first.
    pub fn rigs(&self) -> impl Iterator<Item = &RigState> {
        std::iter::once(&self.rig).chain(&self.background)
    }

    /// The rigs in the order they were added (the drop-down's).
    pub fn rigs_in_order(&self) -> Vec<&RigState> {
        let mut v: Vec<&RigState> = self.rigs().collect();
        v.sort_by_key(|r| r.slot);
        v
    }

    /// The rig with id `id`, shown or in the background.
    pub fn find_rig(&self, id: RigId) -> Option<&RigState> {
        self.rigs().find(|r| r.id == id)
    }

    /// The rig in slot `slot`.
    pub fn rig_in_slot(&self, slot: u32) -> Option<&RigState> {
        self.rigs().find(|r| r.slot == slot)
    }

    /// The rig in slot `slot`, to change.
    pub fn rig_in_slot_mut(&mut self, slot: u32) -> Option<&mut RigState> {
        std::iter::once(&mut self.rig)
            .chain(&mut self.background)
            .find(|r| r.slot == slot)
    }

    /// The shown rig's state (`rig` is another while a message for one in the background is
    /// handled).
    pub fn shown_rig(&self) -> &RigState {
        match self.visiting {
            Some(id) => self.find_rig(id).unwrap_or(&self.rig),
            None => &self.rig,
        }
    }

    /// The name rig `id` goes by: its daemon's, else where it is reached, else the name it
    /// had when closed.
    pub fn rig_name(&self, id: RigId) -> String {
        match self.find_rig(id) {
            Some(r) => rig_label(r),
            None => self.rig_names.get(&id).cloned().unwrap_or_default(),
        }
    }

    /// The rigs not shown whose stimulus is armed or playing (by their mirror), by name.
    pub fn live_elsewhere(&self) -> Vec<String> {
        let shown = self.active();
        self.rigs_in_order()
            .into_iter()
            .filter(|r| r.id != shown)
            .filter(|r| {
                r.stimulus.phase != StimPhase::Idle
                    || r.daemon()
                        .is_some_and(|s| s.generator.armed || s.generator.firing)
            })
            .map(rig_label)
            .collect()
    }

    /// More than one rig: the stimulus and the top bar name the rig.
    pub fn many_rigs(&self) -> bool {
        !self.background.is_empty()
    }

    /// Adds a rig reached through `target` (its link the app's to make): its slot. A rig
    /// remembered from the last run comes with its id and name, so its panes and the
    /// drop-down name it before it answers.
    pub fn add_rig(
        &mut self,
        target: String,
        known: Option<(RigId, String)>,
        addr: Option<crate::prefs::RigAddr>,
    ) -> u32 {
        let slot = self.next_slot;
        self.next_slot += 1;
        let mut r = RigState::in_slot(slot, target);
        if let Some((id, name)) = known.filter(|(id, _)| self.find_rig(*id).is_none()) {
            r.id = id;
            r.name = name;
        }
        r.addr = addr;
        self.background.push(r);
        slot
    }

    /// Drops the rig in slot `slot` that never got a link (its connect failed).
    pub fn forget_rig(&mut self, slot: u32) {
        self.background
            .retain(|r| r.slot != slot || r.link.is_some());
    }

    /// The rig a pane shows: the shown rig's, or the one it was set on.
    pub fn pane_rig(&self, v: &View) -> RigId {
        match v.rig {
            PaneRig::Active => self.active(),
            PaneRig::Rig(r) => r,
        }
    }

    /// Pane view `v` shows the rig in `rig`: its ids are that rig's.
    pub fn ours(&self, v: &View) -> bool {
        self.pane_rig(v) == self.rig.id
    }

    /// Pane `p` shows the rig in `rig`.
    pub fn pane_ours(&self, p: PaneId) -> bool {
        self.layout.view(p).is_some_and(|v| self.ours(v))
    }

    /// What a pane given content now is set on: the shown rig, or the one in `rig` while a
    /// message for it is handled.
    fn here(&self) -> PaneRig {
        match self.visiting {
            Some(_) => PaneRig::Rig(self.rig.id),
            None => PaneRig::Active,
        }
    }

    /// [`Layout::lead`] among the panes of the rig in `rig`.
    pub fn lead(&self, kind: PaneKind) -> Option<PaneId> {
        self.layout.lead_of(kind, |v| self.ours(v))
    }

    /// Panes drawn now that show the rig in `rig`, in reading order.
    pub fn own_visible_panes(&self) -> Vec<PaneId> {
        self.visible_panes()
            .into_iter()
            .filter(|p| self.pane_ours(*p))
            .collect()
    }

    /// Runs `f` with rig `id` in `rig`: the shown one as is, one in the background swapped
    /// in for it (`None`: no such rig).
    pub fn visit<R>(&mut self, id: RigId, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        if id == self.rig.id {
            return Some(f(self));
        }
        let v = self.enter(id)?;
        let r = f(self);
        self.leave(v);
        Some(r)
    }

    pub(crate) fn enter(&mut self, id: RigId) -> Option<Visit> {
        if self.visiting.is_some() {
            return None;
        }
        let index = self.background.iter().position(|r| r.id == id)?;
        let tab = self.rig.id;
        std::mem::swap(&mut self.rig, &mut self.background[index]);
        std::mem::swap(
            &mut self.view.tf.phase_reference,
            &mut self.rig.phase_reference,
        );
        self.visiting = Some(tab);
        Some(Visit {
            index,
            overlay: std::mem::take(&mut self.overlay),
            window_rig: self.window_rig.take(),
        })
    }

    /// Back from a visit. A window the visit opened is closed: windows act on the shown rig.
    /// Returns whether it did.
    pub(crate) fn leave(&mut self, v: Visit) -> bool {
        let opened = !matches!(self.overlay, Overlay::None);
        self.overlay = v.overlay;
        self.window_rig = v.window_rig;
        std::mem::swap(
            &mut self.view.tf.phase_reference,
            &mut self.rig.phase_reference,
        );
        std::mem::swap(&mut self.rig, &mut self.background[v.index]);
        self.visiting = None;
        opened
    }

    /// The prefix of a notification about the rig in `rig` while another is shown.
    pub(super) fn notice_prefix(&self) -> Option<String> {
        self.visiting.map(|_| format!("{}: ", rig_label(&self.rig)))
    }

    /// The rig message `msg` is for: a link's own, a pane's for what is about that pane, the
    /// focused pane's for a key of its kind; anything else the shown rig's.
    fn msg_rig(&self, msg: &Msg, keymap: &Keymap) -> RigId {
        let pane = |p: PaneId| {
            self.layout
                .view(p)
                .map_or(self.rig.id, |v| self.pane_rig(v))
        };
        let focused = pane(self.layout.focus);
        let free = matches!(self.overlay, Overlay::None) && self.rig.input_pick.is_none();
        let pane_command = |c: CommandId| {
            let s = c.scopes();
            !s.contains(&Scope::Global) && !s.contains(&Scope::Inputs)
        };
        match msg {
            Msg::Conn(r, _) => *r,
            Msg::LegendPick(p, _) | Msg::LeqStrip(p, _) => pane(*p),
            Msg::LevelZoom { pane: p, .. } | Msg::LevelPan { pane: p, .. } => pane(*p),
            Msg::IrNav(..) | Msg::SpectrographCursor { .. } if free => focused,
            Msg::Command(c) if free && pane_command(*c) => focused,
            Msg::Key(chord) if free => {
                let scope = self.layout.focused().scope();
                match keymap.lookup(scope, *chord) {
                    Some(c) if pane_command(c) => focused,
                    _ => self.rig.id,
                }
            }
            _ => self.rig.id,
        }
    }

    /// One message in; the requests it makes out, each for the rig whose link sends it.
    pub fn update(&mut self, msg: Msg, keymap: &Keymap) -> Vec<(RigId, Request)> {
        let stop_all = match &msg {
            Msg::Key(c) => *c == STOP_ANYWHERE,
            Msg::Command(c) => *c == CommandId::StopAnywhere,
            _ => false,
        };
        let conn = matches!(msg, Msg::Conn(..));
        let ids: Vec<(u32, RigId)> = self.rigs().map(|r| (r.slot, r.id)).collect();
        let to = self.msg_rig(&msg, keymap);
        let Some(slot) = self.find_rig(to).map(|r| r.slot) else {
            // A closed rig's link, drained after it closed.
            return Vec::new();
        };
        let mut out: Vec<(u32, Request)> = Vec::new();
        if to == self.rig.id {
            self.switched_at = None;
            let reqs = self.update_active(msg, keymap);
            // A switch midway: what follows it (the meters, the device list, the layout
            // of the rig now shown) is the new rig's, not the one the message was for.
            let (at, shown) = match self.switched_at.take() {
                Some(at) => (at, self.rig.slot),
                None => (reqs.len(), slot),
            };
            out.extend(
                reqs.into_iter()
                    .enumerate()
                    .map(|(i, q)| (if i < at { slot } else { shown }, q)),
            );
        } else if let Some(v) = self.enter(to) {
            out.extend(
                self.update_active(msg, keymap)
                    .into_iter()
                    .map(|q| (slot, q)),
            );
            let name = rig_label(&self.rig);
            if self.leave(v) && !conn {
                self.warn(ac2_scene::chrome::rig_window_elsewhere(&name));
            }
        }
        if stop_all {
            let others: Vec<(u32, RigId)> = self
                .rigs()
                .filter(|r| r.slot != slot)
                .map(|r| (r.slot, r.id))
                .collect();
            for (s, id) in others {
                if let Some(qs) = self.visit(id, |st| {
                    let mut q = Vec::new();
                    st.stop_stimulus_here(&mut q);
                    q
                }) {
                    out.extend(qs.into_iter().map(|q| (s, q)));
                }
            }
        }
        self.follow_rig_ids(&ids);
        self.close_duplicates();
        out.into_iter()
            .filter_map(|(s, q)| self.rig_in_slot(s).map(|r| (r.id, q)))
            .collect()
    }

    /// Stops the stimulus of the rig in `rig` where it is armed or firing, and drops a
    /// re-arm waiting on a stop.
    pub(super) fn stop_stimulus_here(&mut self, out: &mut Vec<Request>) {
        self.rig.stimulus.arm_after_stop = false;
        self.rig.sweep.arm_after_stop = false;
        if self.stimulus_live() {
            self.rig.stimulus.phase = StimPhase::Stopping;
            out.push(Request::StimStop);
        }
    }

    /// A rig whose hello named another id than it had (the first hello, or another daemon
    /// at its address): its panes and window follow it.
    fn follow_rig_ids(&mut self, before: &[(u32, RigId)]) {
        for &(slot, old) in before {
            let Some(new) = self.rig_in_slot(slot).map(|r| r.id) else {
                continue;
            };
            if new == old {
                continue;
            }
            let repin = |l: &mut Layout| {
                for v in l.views.values_mut() {
                    if v.rig == PaneRig::Rig(old) {
                        v.rig = PaneRig::Rig(new);
                    }
                }
            };
            repin(&mut self.layout);
            self.history.layouts_mut(repin);
            if self.window_rig == Some(old) {
                self.window_rig = Some(new);
            }
        }
    }

    /// Closes each rig that said hello as one already held: the same daemon reached twice.
    fn close_duplicates(&mut self) {
        let dups: Vec<u32> = self
            .rigs()
            .filter(|r| r.duplicate)
            .map(|r| r.slot)
            .collect();
        for slot in dups {
            let Some(i) = self.background.iter().position(|r| r.slot == slot) else {
                continue;
            };
            let r = self.background.remove(i);
            self.warn(ac2_scene::chrome::rig_already_connected(&rig_label(&r)));
            self.closing.push(r);
        }
    }

    /// Shows rig `to`: its panes are the ones that follow the shown rig, and its windows
    /// and keys are the operator's. Panes given content on the rig shown before keep it,
    /// named by it. Refused while a window is open: the window acts on the rig it opened
    /// on.
    pub(super) fn switch_rig(&mut self, to: RigId, out: &mut Vec<Request>) {
        if self.visiting.is_some() || to == self.rig.id {
            return;
        }
        if !matches!(self.overlay, Overlay::None | Overlay::RigMenu(_)) {
            self.warn(ac2_scene::chrome::RIG_SWITCH_WINDOW_OPEN);
            return;
        }
        let Some(i) = self.background.iter().position(|r| r.id == to) else {
            return;
        };
        self.overlay = Overlay::None;
        self.flush_layout_drag(out);
        let from = self.rig.id;
        let pin = |l: &mut Layout| {
            for v in l.views.values_mut() {
                if v.rig == PaneRig::Active && v.has_content() {
                    v.rig = PaneRig::Rig(from);
                }
            }
        };
        pin(&mut self.layout);
        self.history.layouts_mut(pin);
        // Whose own panes these were stays on the rig they were stored on: the next rig's
        // edits never land in its measurement's layout.
        self.rig.own_panes = None;
        self.rig.own_panes_due = false;
        self.rig.input_pick = None;
        std::mem::swap(
            &mut self.view.tf.phase_reference,
            &mut self.rig.phase_reference,
        );
        std::mem::swap(&mut self.rig, &mut self.background[i]);
        self.view.tf.phase_reference = self.rig.phase_reference.take();
        self.switched_at = Some(out.len());
        let unpin = |l: &mut Layout| {
            for v in l.views.values_mut() {
                if v.rig == PaneRig::Rig(to) {
                    v.rig = PaneRig::Active;
                }
            }
        };
        unpin(&mut self.layout);
        self.history.layouts_mut(unpin);
        self.plate_hover = None;
    }

    /// Closes rig `id`: its link stops the stimulus it holds and gives up its lease before
    /// it goes ([`crate::conn::Conn::close`]); its panes stay, saying it is not connected.
    /// The last rig is never closed: the app always shows one.
    pub(super) fn close_rig(&mut self, id: RigId, out: &mut Vec<Request>) {
        if !self.many_rigs() || self.visiting.is_some() {
            return;
        }
        if id == self.rig.id {
            let Some(next) = self.rigs_in_order().into_iter().find(|r| r.id != id) else {
                return;
            };
            let next = next.id;
            self.switch_rig(next, out);
            if self.rig.id == id {
                return;
            }
        }
        let Some(i) = self.background.iter().position(|r| r.id == id) else {
            return;
        };
        let r = self.background.remove(i);
        let name = rig_label(&r);
        self.rig_names.insert(id, name.clone());
        let before = self.prefs.rigs.len();
        self.prefs.rigs.retain(|p| p.id != id);
        self.prefs_dirty |= self.prefs.rigs.len() != before;
        self.toast(ac2_scene::chrome::rig_closed(&name));
        self.closing.push(r);
    }

    /// The rows of the rigs drop-down: every rig in the order added, Connect…, and closing
    /// the rig shown when there is another to show.
    pub fn rig_menu_rows(&self) -> Vec<RigRow> {
        let mut rows: Vec<RigRow> = self
            .rigs_in_order()
            .into_iter()
            .map(|r| RigRow::Rig(r.id))
            .collect();
        rows.push(RigRow::Connect);
        if self.many_rigs() {
            rows.push(RigRow::Close(self.active()));
        }
        rows
    }

    /// The rigs drop-down opens, on the rig shown. Not while a window is open: switching
    /// would leave the window on another rig.
    pub(super) fn open_rig_menu(&mut self) {
        if matches!(self.overlay, Overlay::RigMenu(_)) {
            self.overlay = Overlay::None;
            return;
        }
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        let rows = self.rig_menu_rows();
        let index = rows
            .iter()
            .position(|r| *r == RigRow::Rig(self.active()))
            .unwrap_or(0);
        self.overlay = Overlay::RigMenu(RigMenu { index });
    }

    /// A row of the rigs drop-down taken.
    pub(super) fn rig_pick(&mut self, row: RigRow, out: &mut Vec<Request>) {
        if matches!(self.overlay, Overlay::RigMenu(_)) {
            self.overlay = Overlay::None;
        } else if !matches!(self.overlay, Overlay::None) {
            return;
        }
        match row {
            RigRow::Rig(id) => self.switch_rig(id, out),
            RigRow::Connect => self.want_connect_dialog = true,
            RigRow::Close(id) => self.close_rig(id, out),
        }
    }

    /// Up/Down/Home/End/Enter in the rigs drop-down.
    pub(super) fn rig_menu_key(&mut self, mut menu: RigMenu, chord: Chord, out: &mut Vec<Request>) {
        use eframe::egui::Key;
        let rows = self.rig_menu_rows();
        let n = rows.len();
        match chord.key {
            Key::ArrowDown => menu.index = (menu.index + 1) % n,
            Key::ArrowUp => menu.index = (menu.index + n - 1) % n,
            Key::Home | Key::PageUp => menu.index = 0,
            Key::End | Key::PageDown => menu.index = n - 1,
            Key::Enter => {
                if let Some(r) = rows.get(menu.index) {
                    self.rig_pick(*r, out);
                }
                return;
            }
            _ => return,
        }
        self.overlay = Overlay::RigMenu(menu);
    }

    /// The wheel over the rigs drop-down.
    pub(super) fn rig_menu_wheel(&mut self, mut menu: RigMenu, rows: i32) {
        let n = self.rig_menu_rows().len() as i64;
        menu.index = (menu.index as i64 + i64::from(rows)).clamp(0, n - 1) as usize;
        self.overlay = Overlay::RigMenu(menu);
    }

    /// The rig in `rig` said hello with `id`: a rig already held is not taken twice.
    pub(super) fn is_duplicate(&self, id: RigId) -> bool {
        self.rigs()
            .chain(self.closing.iter())
            .any(|r| r.slot != self.rig.slot && r.id == id && !r.duplicate)
    }

    /// The rig in `rig` connected: remembered (where it was reached, its id and name) for
    /// the next start.
    pub(super) fn remember_rig(&mut self) {
        let Some(addr) = self.rig.addr.clone() else {
            return;
        };
        let entry = crate::prefs::RememberedRig {
            id: self.rig.id,
            name: self.rig.name.clone(),
            addr,
        };
        let at = self
            .prefs
            .rigs
            .iter()
            .position(|p| p.id == entry.id || p.addr == entry.addr);
        match at {
            Some(i) if self.prefs.rigs[i] == entry => {}
            Some(i) => {
                self.prefs.rigs[i] = entry;
                self.prefs.rigs.dedup_by(|a, b| a.id == b.id);
                self.prefs_dirty = true;
            }
            None => {
                self.prefs.rigs.push(entry);
                self.prefs_dirty = true;
            }
        }
    }

    /// Before a message: each pane's content, so the panes given content by it are set on
    /// the rig that gave it.
    pub(super) fn pane_contents(&self) -> BTreeMap<PaneId, View> {
        self.layout.views.clone()
    }

    /// After a message: panes given content (or new) are set on the rig it was for.
    pub(super) fn set_pane_rigs(&mut self, before: &BTreeMap<PaneId, View>) {
        let here = self.here();
        for (id, v) in &mut self.layout.views {
            match before.get(id) {
                Some(b) if b.meas == v.meas && b.meters == v.meters => {}
                Some(_) => v.rig = here,
                None if v.rig == PaneRig::Active => v.rig = here,
                None => {}
            }
        }
    }
}

/// What a rig is called: its daemon's name, else where it is reached.
pub fn rig_label(r: &RigState) -> String {
    if !r.name.is_empty() {
        return r.name.clone();
    }
    match &r.conn {
        ConnState::Connecting { target } | ConnState::Failed { target, .. } => target.clone(),
        ConnState::Connected { name, target, .. } => {
            if name.is_empty() {
                target.clone()
            } else {
                name.clone()
            }
        }
    }
}
