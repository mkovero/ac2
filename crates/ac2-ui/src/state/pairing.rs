//! Pairing the rig shown with another rig the app holds, both ways in one act, and
//! unpairing it (Settings › Connection, *Paired rigs*). A direction is two halves: the
//! taking rig lists the other (`rig.peers`) and the other has admitted it
//! (`auth.admit_rig`). Each half is read from the daemons' mirrored state and `server.info`
//! answers, matched by rig id; a half of a rig the app does not hold is not known.

use super::*;
use crate::remote_inputs::admit_name;
use crate::settings::{PairOffer, PairedRow};
use ac2_proto::model::{RigPeer, ServerInfo};
use ac2_scene::pairing::{self as words, Half, Halves, PairRefusal, Unseen};

use super::remote_impl::Source;

/// The rig ids rig `info` has admitted (its `server.info`), or why they are not known.
fn admitted(
    info: &Option<Result<ServerInfo, String>>,
) -> Result<Vec<(RigId, String, String)>, Unseen> {
    match info {
        None | Some(Err(_)) => Err(Unseen::Asking),
        // Off the network a daemon lists no keys: what it admitted is not known.
        Some(Ok(i)) if i.network.status.endpoints().is_none() => Err(Unseen::NetworkOff),
        Some(Ok(i)) => Ok(i
            .network
            .authorized
            .iter()
            .filter_map(|a| Some((a.rig?, a.name.clone(), a.key.clone())))
            .collect()),
    }
}

/// The server key a rig's `server.info` gives, when known.
fn server_key(r: &RigState) -> Option<&str> {
    match &r.server {
        Some(Ok(i)) => i.network.server_key.as_deref(),
        _ => None,
    }
}

fn connected(r: &RigState) -> bool {
    matches!(r.conn, ConnState::Connected { .. }) && r.daemon().is_some()
}

/// `peers` with `s` in, in place of any entry for the same rig or under the same name.
pub(super) fn peers_with(peers: &[RigPeer], s: &Source) -> Vec<RigPeer> {
    let mut v: Vec<RigPeer> = peers
        .iter()
        .filter(|p| p.rig != s.id && p.name != s.name)
        .cloned()
        .collect();
    v.push(s.peer());
    v
}

impl AppState {
    /// The four halves between the rig shown and rig `other`.
    pub fn pair_halves(&self, other: RigId) -> Halves {
        let here = &self.rig;
        let there = self.others.background.iter().find(|r| r.id == other);
        // A listing or an admission under another key than the rig serves now is stale: the
        // link would be refused, so it is not counted as a half in place.
        let key_ok =
            |r: Option<&RigState>, key: &str| r.and_then(server_key).is_none_or(|k| k == key);
        let lists = |r: &RigState, of: RigId, of_rig: Option<&RigState>| match r.daemon() {
            Some(s) => {
                if s.remote
                    .peers
                    .iter()
                    .any(|p| p.rig == of && key_ok(of_rig, &p.key))
                {
                    Half::Yes
                } else {
                    Half::No
                }
            }
            None => Half::Unknown(Unseen::Asking),
        };
        let admits = |r: &RigState, of: RigId, of_rig: Option<&RigState>| match admitted(&r.server)
        {
            Ok(a) if a.iter().any(|(id, _, k)| *id == of && key_ok(of_rig, k)) => Half::Yes,
            Ok(_) => Half::No,
            Err(u) => Half::Unknown(u),
        };
        let not_held = Half::Unknown(Unseen::NotHeld);
        Halves {
            here_lists: lists(here, other, there),
            here_admits: admits(here, other, there),
            there_lists: there.map_or(not_held.clone(), |t| lists(t, here.id, Some(here))),
            there_admits: there.map_or(not_held, |t| admits(t, here.id, Some(here))),
        }
    }

    /// The rigs linked with the rig shown by any half its own state or a held rig's shows,
    /// with their names: the rig's label when held, else what the rig shown calls it.
    pub fn paired_with_shown(&self) -> Vec<(RigId, String)> {
        let here = &self.rig;
        let mut v: Vec<(RigId, String)> = Vec::new();
        let mut add = |id: RigId, name: String| {
            if id != here.id && !v.iter().any(|(r, _)| *r == id) {
                v.push((id, name));
            }
        };
        let named = |id: RigId, fallback: &str| match self.find_rig(id) {
            Some(r) => self.rig_label(r),
            None => fallback.to_owned(),
        };
        for p in here
            .daemon()
            .map(|s| s.remote.peers.as_slice())
            .unwrap_or_default()
        {
            add(p.rig, named(p.rig, &p.name));
        }
        for (id, name, _) in admitted(&here.server).unwrap_or_default() {
            add(id, named(id, &name));
        }
        for b in self.rigs_in_order() {
            if b.id == here.id {
                continue;
            }
            let lists = b
                .daemon()
                .is_some_and(|s| s.remote.peers.iter().any(|p| p.rig == here.id));
            let admits = admitted(&b.server).is_ok_and(|a| a.iter().any(|(id, ..)| *id == here.id));
            if lists || admits {
                add(b.id, self.rig_label(b));
            }
        }
        v
    }

    /// Both ends of pairing the rig shown with held rig `b`, or why not, in words.
    fn pair_ends(&self, b: &RigState) -> Result<(Source, Source), String> {
        let me = self.rig_label(&self.rig);
        let name = self.rig_label(b);
        if !connected(&self.rig) {
            return Err(words::refusal(&me, &PairRefusal::NotConnected));
        }
        if !connected(b) {
            return Err(words::refusal(&name, &PairRefusal::NotConnected));
        }
        let here = self
            .pair_end(&self.rig, b)
            .map_err(|w| words::refusal(&me, &w))?;
        let there = self
            .pair_end(b, &self.rig)
            .map_err(|w| words::refusal(&name, &w))?;
        Ok((here, there))
    }

    /// The rigs *Pair with rig…* offers: each held rig not yet paired both ways.
    fn pair_offers(&self) -> Vec<PairOffer> {
        self.rigs_in_order()
            .into_iter()
            .filter(|b| b.id != self.rig.id)
            .filter_map(|b| {
                let h = self.pair_halves(b.id);
                if h.both_ways() {
                    return None;
                }
                let refusal = self.pair_ends(b).err();
                Some(PairOffer {
                    rig: b.id,
                    text: refusal
                        .clone()
                        .unwrap_or_else(|| words::offer_row(&self.rig_label(b), &h)),
                    refusal,
                })
            })
            .collect()
    }

    /// The Connection page's pairing rows, worked out again from the state now.
    pub(super) fn refresh_pairing(&mut self) {
        let on_page = matches!(&self.overlay, Overlay::Settings(s)
            if s.page == crate::settings::Page::Connection);
        if !on_page || self.others.visiting.is_some() {
            return;
        }
        let rows: Vec<PairedRow> = self
            .paired_with_shown()
            .into_iter()
            .map(|(rig, name)| {
                let h = self.pair_halves(rig);
                PairedRow {
                    rig,
                    text: words::row(&name, &h),
                    tip: words::tip(&name, &h),
                    mend: h.missing(),
                    held: self.find_rig(rig).is_some(),
                    name,
                }
            })
            .collect();
        let open =
            matches!(&self.overlay, Overlay::Settings(s) if s.connection.pairing.offers.is_some());
        let offers = open.then(|| self.pair_offers());
        if let Overlay::Settings(s) = &mut self.overlay {
            let p = &mut s.connection.pairing;
            p.rows = rows;
            if p.confirm.is_some_and(|c| p.row(c).is_none()) {
                p.confirm = None;
            }
            if let Some(o) = offers {
                p.offers = Some(o);
            }
        }
    }

    /// Asks every other rig held for its `server.info` (its key and admitted rigs), which
    /// the pairing rows read.
    pub(super) fn ask_others_server_info(&mut self) {
        let others: Vec<RigId> = self
            .others
            .background
            .iter()
            .filter(|r| matches!(r.conn, ConnState::Connected { .. }))
            .map(|r| r.id)
            .collect();
        self.others
            .elsewhere
            .extend(others.into_iter().map(|r| (r, Request::ServerInfo)));
    }

    /// *Pair with rig…*: the pick of the rigs held, or why there is none.
    pub(super) fn open_pair_pick(&mut self, out: &mut Vec<Request>) {
        let held = !self.others.background.is_empty();
        let offers = self.pair_offers();
        out.push(Request::ServerInfo);
        self.ask_others_server_info();
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let p = &mut s.connection;
        if !held {
            p.error = Some(words::NO_OTHER_RIG.into());
            return;
        }
        if offers.is_empty() {
            p.notice = Some(words::ALL_PAIRED.into());
            return;
        }
        let first = offers[0].rig;
        p.pairing.offers = Some(offers);
        if let Some(i) = p
            .lines()
            .iter()
            .position(|l| *l == crate::settings::ConnLine::PairOffer(first))
        {
            p.focus = i;
        }
    }

    fn pair_error(&mut self, e: String) {
        if let Overlay::Settings(s) = &mut self.overlay {
            s.connection.error = Some(e);
        }
    }

    /// Pairs the rig shown with held rig `rig` both ways: each rig lists the other and
    /// admits the other. Only the halves not in place are sent, so the toasts say what was
    /// done; nothing is sent when either end refuses.
    pub(super) fn pair_with(&mut self, rig: RigId, out: &mut Vec<Request>) {
        let Some(b) = self.others.background.iter().find(|r| r.id == rig) else {
            let name = self.rig_name(rig);
            return self.pair_error(words::not_held(&name));
        };
        let (here, there) = match self.pair_ends(b) {
            Ok(e) => e,
            Err(e) => return self.pair_error(e),
        };
        let h = self.pair_halves(rig);
        let peers_here = self
            .daemon()
            .map(|s| s.remote.peers.clone())
            .unwrap_or_default();
        let peers_there = b
            .daemon()
            .map(|s| s.remote.peers.clone())
            .unwrap_or_default();
        let admit = |admitter: &Source, rig: &Source| Request::ServerCall {
            cmd: Command::AuthAdmitRig {
                key: rig.key.clone(),
                rig_id: rig.id,
                name: admit_name(&rig.name),
            },
            what: ac2_scene::remote_inputs::admitted_toast(&admitter.name, &rig.name),
            failed: words::not_admitted(&admitter.name, &rig.name),
        };
        let list = |lister: &Source, peers: &[RigPeer], peer: &Source| Request::Act {
            cmd: Command::RigPeers {
                peers: peers_with(peers, peer),
            },
            done: words::listed_toast(&lister.name, &peer.name),
            failed: words::not_listed(&lister.name, &peer.name),
        };
        // Admissions first: a listing whose admission is not there yet only retries.
        if h.there_admits != Half::Yes {
            self.others.elsewhere.push((rig, admit(&there, &here)));
        }
        if h.here_admits != Half::Yes {
            out.push(admit(&here, &there));
        }
        if h.here_lists != Half::Yes {
            out.push(list(&here, &peers_here, &there));
        }
        if h.there_lists != Half::Yes {
            self.others
                .elsewhere
                .push((rig, list(&there, &peers_there, &here)));
        }
        if let Overlay::Settings(s) = &mut self.overlay {
            s.connection.pairing.offers = None;
        }
    }

    /// The open session of `r`'s remote inputs taken from rig `from`, by name.
    fn inputs_from(&self, r: &RigState, from: RigId) -> Vec<String> {
        r.daemon()
            .and_then(|s| s.session.open.as_ref())
            .map(|o| {
                o.config
                    .remote_inputs
                    .iter()
                    .filter(|i| i.rig == from)
                    .map(|i| ac2_scene::remote_inputs::remote_name(&i.rig_name, i.channel, None))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Unpairs the rig shown and rig `rig`: this rig drops it from its paired rigs and
    /// revokes its admission, and so does `rig` when the app holds it. Refused, with
    /// nothing sent, while either session takes inputs from the other: those inputs would
    /// lose their link under a running session.
    pub(super) fn unpair(&mut self, rig: RigId, out: &mut Vec<Request>) {
        let me = self.rig_label(&self.rig);
        let there = self.others.background.iter().find(|r| r.id == rig);
        let held = there.is_some();
        let name = there.map_or_else(
            || {
                self.paired_with_shown()
                    .into_iter()
                    .find(|(id, _)| *id == rig)
                    .map_or_else(|| self.rig_name(rig), |(_, n)| n)
            },
            |r| self.rig_label(r),
        );
        let here_inputs = self.inputs_from(&self.rig, rig);
        if !here_inputs.is_empty() {
            return self.pair_error(words::unpair_inputs(&me, &here_inputs));
        }
        if let Some(t) = there {
            let inputs = self.inputs_from(t, self.rig.id);
            if !inputs.is_empty() {
                return self.pair_error(words::unpair_inputs(&name, &inputs));
            }
        }
        let unlist = |lister: &str, peers: &[RigPeer], drop: RigId, peer: &str| Request::Act {
            cmd: Command::RigPeers {
                peers: peers.iter().filter(|p| p.rig != drop).cloned().collect(),
            },
            done: words::unlisted_toast(lister, peer),
            failed: words::not_unlisted(lister, peer),
        };
        let revoke = |admitter: &str, admitted_name: String, rig: &str| Request::ServerCall {
            cmd: Command::ServerRevoke {
                name: admitted_name,
            },
            what: words::revoked_toast(admitter, rig),
            failed: words::not_revoked(admitter, rig),
        };
        let mut here_reqs = Vec::new();
        if let Some(s) = self.daemon()
            && s.remote.peers.iter().any(|p| p.rig == rig)
        {
            here_reqs.push(unlist(&me, &s.remote.peers, rig, &name));
        }
        for (id, admitted_name, _) in admitted(&self.rig.server).unwrap_or_default() {
            if id == rig {
                here_reqs.push(revoke(&me, admitted_name, &name));
            }
        }
        let mut there_reqs = Vec::new();
        if let Some(t) = there {
            if let Some(s) = t.daemon()
                && s.remote.peers.iter().any(|p| p.rig == self.rig.id)
            {
                there_reqs.push(unlist(&name, &s.remote.peers, self.rig.id, &me));
            }
            for (id, admitted_name, _) in admitted(&t.server).unwrap_or_default() {
                if id == self.rig.id {
                    there_reqs.push(revoke(&name, admitted_name, &me));
                }
            }
        }
        out.extend(here_reqs);
        self.others
            .elsewhere
            .extend(there_reqs.into_iter().map(|q| (rig, q)));
        if !held {
            let rest = words::unpair_rest(&name);
            if let Overlay::Settings(s) = &mut self.overlay {
                s.connection.notice = Some(rest.clone());
            }
            self.warn(rest);
        }
    }
}
