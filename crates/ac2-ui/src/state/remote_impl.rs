//! Remote inputs in the reducer (`docs/design/remote-inputs.md` §2.3, §5): the add dialog
//! over the rigs this app holds, pairing two rigs (the source admits this rig, this rig
//! pairs with the source) and reopening this rig's session with the input, removal, the
//! refusal of a setup change on one, and the words its rows and the Connection page show.

use super::*;
use crate::remote_inputs::{PickLine, RemotePick, RemoteRow, admit_name, peer_endpoints};
use ac2_proto::model::{
    InputSetup, OpenSession, RemoteInput, RemoteState, RigPeer, ServerInfo, ServerMode, remote_slot,
};
use ac2_scene::remote_inputs::{self as words, SourceRefusal};
use eframe::egui::Key;

/// What pairing needs of the source rig: where this rig reaches it, its key and name, and
/// the name this rig is admitted under there.
struct Source {
    id: RigId,
    name: String,
    ctrl: String,
    data: String,
    key: String,
}

impl AppState {
    /// The open session's remote input on session channel `ch`.
    pub fn remote_input(&self, ch: u16) -> Option<&RemoteInput> {
        let slot = remote_slot(ch)?;
        self.open_session()?
            .config
            .remote_inputs
            .iter()
            .find(|r| r.slot == slot)
    }

    /// What its rig says is on remote channel `ch`: its kind, mic and curve.
    pub(super) fn remote_setup(&self, ch: u16) -> Option<InputSetup> {
        let slot = remote_slot(ch)?;
        let s = self
            .daemon()?
            .remote
            .inputs
            .iter()
            .find(|s| s.slot == slot)?;
        s.setup.clone().map(|mut x| {
            x.channel = ch;
            x
        })
    }

    /// Remote channel `ch`'s name: its rig's mic name, then the rig and its input
    /// (`MM1 34804 · ketunkolo in 1`); `None` for this rig's own inputs.
    pub fn remote_input_name(&self, ch: u16) -> Option<String> {
        let r = self.remote_input(ch)?;
        let setup = self.remote_setup(ch);
        let rig = self.daemon().map_or_else(
            || r.rig_name.clone(),
            |s| ac2_scene::between_rigs::rig_name(&s.remote, r),
        );
        Some(words::remote_name(
            &rig,
            r.channel,
            setup.as_ref().and_then(InputSetup::mic),
        ))
    }

    /// The local time of day of daemon wall time `t`, as the rows say it.
    fn at_clock(&self, t: ac2_proto::units::WallNs) -> String {
        let now = crate::scenes::daemon_wall(self, super::sessions_impl::wall_now());
        ac2_scene::time::local_clock(t, now, |x| self.local_zone.offset_s(x))
    }

    /// The Audio I/O page's remote input rows, in the session's order.
    pub fn remote_rows(&self) -> Vec<RemoteRow> {
        let (Some(st), Some(o)) = (self.daemon(), self.open_session()) else {
            return Vec::new();
        };
        let meters = self.input_meters();
        let now = super::sessions_impl::wall_now();
        let offset = self.clock_offset();
        o.config
            .remote_inputs
            .iter()
            .map(|r| {
                let ch = r.session_channel();
                let status = st.remote.inputs.iter().find(|s| s.slot == r.slot);
                let setup = status.and_then(|s| s.setup.as_ref());
                let role = setup
                    .map(|s| crate::session_dialog::kind_role(crate::prefs::KindPref::of(&s.kind)));
                let mut text = words::source_row(r.channel, role, setup.and_then(InputSetup::mic));
                if let Some(s) = status {
                    if let Some(c) = &s.curve {
                        text.push_str(&format!(" · {}", c.label));
                    }
                    text.push_str(&format!(
                        " · {}",
                        ac2_scene::spl::cal_text(s.cal_status, s.curve.is_some(), now, offset)
                    ));
                }
                let state = ac2_scene::between_rigs::input_state(&st.remote, r, &|t| {
                    Some(self.at_clock(t))
                });
                RemoteRow {
                    slot: r.slot,
                    rig: ac2_scene::between_rigs::rig_name(&st.remote, r),
                    text,
                    state: state.text,
                    problem: state.problem,
                    reading: meters
                        .get(&ch)
                        .cloned()
                        .unwrap_or_else(ac2_scene::meter::MeterReading::none),
                }
            })
            .collect()
    }

    /// The Connection page's *Between rigs* blocks, one per rig the session takes inputs
    /// from.
    pub fn between_rigs(&self) -> Vec<Vec<String>> {
        let (Some(st), Some(o)) = (self.daemon(), self.open_session()) else {
            return Vec::new();
        };
        st.remote
            .rigs
            .iter()
            .map(|rig| {
                ac2_scene::between_rigs::rig_block(&st.remote, rig, &o.config.remote_inputs, &|t| {
                    Some(self.at_clock(t))
                })
            })
            .collect()
    }

    /// Measurement `meas` of rig `rig` as a transfer function across two rigs: its state,
    /// that rig's remote state and that rig's own name (the words say both rigs).
    pub fn cross_rig_of(
        &self,
        rig: RigId,
        meas: ac2_proto::units::MeasId,
    ) -> Option<(&ac2_proto::model::CrossRigTransfer, &RemoteState, String)> {
        let st = self.find_rig(rig)?.daemon()?;
        let x = ac2_scene::cross_rig::entry(&st.remote, meas)?;
        Some((x, &st.remote, self.rig_name(rig)))
    }

    /// [`Self::cross_rig_of`] on the rig in `rig`: the one whose measurement `meas` is, also
    /// while a pane of a rig in the background is drawn (a visit), when it is not the shown
    /// rig. A measurement id means nothing on another rig.
    pub fn cross_rig(
        &self,
        meas: ac2_proto::units::MeasId,
    ) -> Option<(&ac2_proto::model::CrossRigTransfer, &RemoteState, String)> {
        self.cross_rig_of(self.rig.id, meas)
    }

    /// How the words of a transfer function across rigs name the delay finder: its key in
    /// a transfer pane, else where the palette has it.
    pub fn find_delay_key(&self) -> String {
        self.keys
            .label(CommandId::InsertDelay, Scope::Transfer)
            .unwrap_or_else(|| self.keys.palette_entry(CommandId::InsertDelay.title()))
    }

    /// The top bar's chip per rig the session takes inputs from, beside the rig drop-down.
    pub fn rig_chips(&self) -> Vec<ac2_scene::cross_rig::RigChip> {
        let (Some(st), Some(o)) = (self.daemon(), self.open_session()) else {
            return Vec::new();
        };
        st.remote
            .rigs
            .iter()
            .map(|rig| ac2_scene::cross_rig::rig_chip(&st.remote, rig, &o.config.remote_inputs))
            .collect()
    }

    /// The top bar's banner while a transfer function across two rigs is stopped by the
    /// clocks: why and what to do, one per stopped measurement.
    pub fn cross_rig_banners(&self) -> Vec<String> {
        let Some(st) = self.daemon() else {
            return Vec::new();
        };
        let ours = self.rig_label(&self.rig);
        st.remote
            .transfers
            .iter()
            .filter_map(|x| {
                let m = st.measurements.iter().find(|m| m.id == x.meas)?;
                ac2_scene::cross_rig::banner(&m.config.name, x, &st.remote, &ours)
            })
            .collect()
    }

    /// This rig's key for another rig, or why it has none to give.
    fn own_key(&self) -> Result<String, String> {
        let name = self.rig_label(&self.rig);
        match &self.rig.server {
            Some(Ok(ServerInfo {
                mode: ServerMode::Network { server_key, .. },
                ..
            })) => Ok(server_key.clone()),
            Some(Ok(_)) => Err(words::this_rig_local(&name)),
            Some(Err(e)) => Err(format!("{name}: {e}")),
            None => Err(words::asking_this_rig(&name)),
        }
    }

    /// Rig `b` as a source this rig can pair with, or why not.
    fn source(&self, b: &RigState) -> Result<Source, SourceRefusal> {
        let info = match &b.server {
            None => return Err(SourceRefusal::Asking),
            Some(Err(e)) => return Err(SourceRefusal::Unanswered(e.clone())),
            Some(Ok(i)) => i,
        };
        let ServerMode::Network {
            ctrl,
            data,
            server_key,
            authorized,
            ..
        } = &info.mode
        else {
            return Err(SourceRefusal::LocalOnly);
        };
        let st = b.daemon();
        let open = st.and_then(|s| s.session.open.as_ref());
        match open {
            None => return Err(SourceRefusal::NoSession),
            Some(o) if o.replay.is_some() => return Err(SourceRefusal::Replaying),
            Some(_) => {}
        }
        let (ctrl, data) = peer_endpoints(ctrl, data, b.daemon_addr.as_deref(), &self.local_host)
            .ok_or(SourceRefusal::LocalOnly)?;
        let as_name = admit_name(&self.rig_label(&self.rig));
        if authorized
            .iter()
            .any(|a| a.name == as_name && a.rig != Some(self.rig.id))
        {
            return Err(SourceRefusal::NameTaken(as_name));
        }
        Ok(Source {
            id: b.id,
            name: self.rig_label(b),
            ctrl,
            data,
            key: server_key.clone(),
        })
    }

    /// The add dialog's lines: per rig held besides this one, its heading and inputs, or
    /// why they cannot be taken.
    pub fn remote_pick_lines(&self) -> Vec<PickLine> {
        let text = |text: String, problem| PickLine::Text { text, problem };
        if self.background.is_empty() {
            return vec![text(words::NO_OTHER_RIG.into(), false)];
        }
        let Some(open) = self.open_session() else {
            return vec![text(words::NO_SESSION_HERE.into(), true)];
        };
        let mut v = Vec::new();
        if let Err(e) = self.own_key() {
            v.push(text(e, true));
        }
        for b in self.rigs_in_order() {
            if b.id == self.rig.id {
                continue;
            }
            let name = self.rig_label(b);
            if let Err(why) = self.source(b) {
                v.push(text(
                    words::source_refusal(&name, &why),
                    why != SourceRefusal::Asking,
                ));
                continue;
            }
            v.extend(source_inputs(b).into_iter().map(|(channel, row)| {
                let added = open
                    .config
                    .remote_inputs
                    .iter()
                    .any(|r| r.rig == b.id && r.channel == channel);
                PickLine::Input {
                    rig: b.id,
                    channel,
                    text: words::pick_row(&name, &row, added),
                    added,
                }
            }));
        }
        v
    }

    /// Opens the add dialog and asks every rig held for its server info (mode, key), which
    /// pairing gives the other rig.
    pub(super) fn open_remote_pick(&mut self, out: &mut Vec<Request>) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        s.remote.pick = Some(RemotePick::default());
        s.remote.confirm = None;
        out.push(Request::ServerInfo);
        let others: Vec<RigId> = self.background.iter().map(|r| r.id).collect();
        self.elsewhere
            .extend(others.into_iter().map(|r| (r, Request::ServerInfo)));
    }

    /// Keys of the Audio I/O page that are about remote inputs: the add dialog's, and on a
    /// remote input's row the removal and the refusal of what is set on its rig. `true`
    /// when the key was taken.
    pub(super) fn remote_key(&mut self, chord: Chord, out: &mut Vec<Request>) -> bool {
        let plain = !(chord.command || chord.alt);
        let Overlay::Settings(s) = &self.overlay else {
            return false;
        };
        if s.on_ceiling {
            return false;
        }
        if let Some(p) = &s.remote.pick {
            let picks: Vec<(RigId, u16)> = self
                .remote_pick_lines()
                .iter()
                .filter_map(PickLine::pick)
                .collect();
            let focus = p.focus.min(picks.len().saturating_sub(1));
            match chord.key {
                Key::ArrowUp | Key::ArrowDown | Key::Tab if !picks.is_empty() => {
                    let back = chord.key == Key::ArrowUp || (chord.key == Key::Tab && chord.shift);
                    let n = picks.len() as i32;
                    let to = (focus as i32 + if back { -1 } else { 1 }).rem_euclid(n);
                    if let Some(p) = self.remote_pick_mut() {
                        p.focus = to as usize;
                    }
                }
                Key::Enter => match picks.get(focus) {
                    Some(&(rig, ch)) => self.pick_remote(rig, ch, out),
                    None => {
                        if let Some(p) = self.remote_pick_mut() {
                            p.error = Some("no input to pick".into());
                        }
                    }
                },
                _ => {}
            }
            return true;
        }
        let focus = s.session.focus;
        match focus {
            Row::AddRemote if chord.key == Key::Enter && plain => {
                self.open_remote_pick(out);
                true
            }
            Row::Remote(i) => {
                let Some(r) = s.session.remote.get(i).cloned() else {
                    return false;
                };
                match chord.key {
                    Key::Delete | Key::Backspace if plain => {
                        self.remove_remote(r.slot, out);
                        true
                    }
                    Key::Space
                    | Key::R
                    | Key::M
                    | Key::L
                    | Key::A
                    | Key::S
                    | Key::N
                    | Key::F2
                    | Key::ArrowLeft
                    | Key::ArrowRight
                        if plain =>
                    {
                        let rig = self.daemon().map_or_else(
                            || r.rig_name.clone(),
                            |st| ac2_scene::between_rigs::rig_name(&st.remote, &r),
                        );
                        if let Some(d) = self.overlay.session_mut() {
                            d.error = Some(words::set_on(&rig));
                        }
                        true
                    }
                    Key::P if plain && chord.shift => {
                        if let Some(d) = self.overlay.session_mut() {
                            d.error = Some(words::set_on(&r.rig_name));
                        }
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn remote_pick_mut(&mut self) -> Option<&mut RemotePick> {
        match &mut self.overlay {
            Overlay::Settings(s) => s.remote.pick.as_mut(),
            _ => None,
        }
    }

    fn pick_error(&mut self, e: String) {
        if let Some(p) = self.remote_pick_mut() {
            p.error = Some(e);
        }
    }

    /// Picks input `channel` of rig `rig` (another rig held): that rig admits this one, this
    /// rig pairs with it, and this rig's session reopens with the input added, everything
    /// else of it kept.
    pub(super) fn pick_remote(&mut self, rig: RigId, channel: u16, out: &mut Vec<Request>) {
        let key = match self.own_key() {
            Ok(k) => k,
            Err(e) => return self.pick_error(e),
        };
        let Some(b) = self.background.iter().find(|r| r.id == rig) else {
            return self.pick_error("that rig is no longer held by the app".into());
        };
        let src = match self.source(b) {
            Ok(s) => s,
            Err(why) => {
                let name = self.rig_label(b);
                return self.pick_error(words::source_refusal(&name, &why));
            }
        };
        let Some(open) = self.open_session().cloned() else {
            return self.pick_error(words::NO_SESSION_HERE.into());
        };
        let used: Vec<u16> = self
            .measurements()
            .iter()
            .flat_map(|m| crate::remote_inputs::meas_channels(&m.config.kind))
            .collect();
        let Some(slot) = crate::remote_inputs::next_slot(&open.config.remote_inputs, used) else {
            return self.pick_error("this session has no remote slot left".into());
        };
        let input = RemoteInput {
            slot,
            rig: src.id,
            rig_name: src.name.clone(),
            channel,
        };
        let mut config = open.config.clone();
        config.input_channels.push(input.session_channel());
        config.remote_inputs.push(input);
        let mut peers: Vec<RigPeer> = self
            .daemon()
            .map(|s| s.remote.peers.clone())
            .unwrap_or_default();
        peers.retain(|p| p.rig != src.id && p.name != src.name);
        peers.push(RigPeer {
            rig: src.id,
            name: src.name.clone(),
            ctrl: src.ctrl,
            data: src.data,
            key: src.key,
        });
        let me = self.rig_label(&self.rig);
        self.elsewhere.push((
            src.id,
            Request::Call {
                cmd: Command::AuthAdmitRig {
                    key,
                    rig_id: self.rig.id,
                    name: admit_name(&me),
                },
                what: words::admitted_toast(&src.name, &me),
            },
        ));
        let name = words::remote_name(&src.name, channel, None);
        out.push(Request::RemoteSession {
            peers: Some(peers),
            config,
            what: words::added_toast(&name),
        });
        if let Overlay::Settings(s) = &mut self.overlay {
            s.remote.pick = None;
        }
    }

    /// Delete on a remote input's row: the first press asks, the second reopens the
    /// session without it.
    pub(super) fn remove_remote(&mut self, slot: u16, out: &mut Vec<Request>) {
        let Some(open) = self.open_session().cloned() else {
            return;
        };
        let Some(r) = open.config.remote_inputs.iter().find(|r| r.slot == slot) else {
            return;
        };
        let name = self
            .remote_input_name(r.session_channel())
            .unwrap_or_else(|| words::remote_name(&r.rig_name, r.channel, None));
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        if s.remote.confirm != Some(slot) {
            s.remote.confirm = Some(slot);
            s.session.notice = Some(words::remove_confirm(&name));
            return;
        }
        s.remote.confirm = None;
        s.session.notice = None;
        out.push(Request::RemoteSession {
            peers: None,
            config: without(&open, slot),
            what: words::removed_toast(&name),
        });
    }
}

/// `open`'s configuration without remote slot `slot`.
fn without(open: &OpenSession, slot: u16) -> ac2_proto::model::SessionConfig {
    let mut c = open.config.clone();
    c.remote_inputs.retain(|r| r.slot != slot);
    let ch = ac2_proto::model::REMOTE_BASE + slot;
    c.input_channels.retain(|&x| x != ch);
    c
}

/// Rig `b`'s captured inputs with the row text it gives them (`in 1 · mic MM1 34804`): a
/// rig streams only what its own session captures.
fn source_inputs(b: &RigState) -> Vec<(u16, String)> {
    let Some(st) = b.daemon() else {
        return Vec::new();
    };
    let Some(o) = st.session.open.as_ref() else {
        return Vec::new();
    };
    let loopback = o.config.loopback.map(|l| l.input);
    o.config
        .input_channels
        .iter()
        .filter(|&&c| remote_slot(c).is_none())
        .map(|&c| {
            let setup = ac2_proto::cal::input_setup(&st.inputs, c);
            let role = if loopback == Some(c) {
                ac2_scene::meter::InputRole::Reference
            } else {
                crate::session_dialog::kind_role(crate::prefs::KindPref::of(&setup.kind))
            };
            (c, words::source_row(c, Some(role), setup.mic()))
        })
        .collect()
}
