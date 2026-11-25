//! Pairing the rig shown with another rig the app holds, both ways in one act, and
//! unpairing it (Settings › Connection, *Paired rigs*). A direction is two halves: the
//! taking rig lists the other (`rig.pair`) and the other has admitted it
//! (`auth.admit_rig`). Each half is read from the daemons' mirrored state and `server.info`
//! answers, matched by rig id; a half of a rig the app does not hold is not known.

use super::*;
use crate::remote_inputs::admit_name;
use crate::settings::{PairOffer, PairedRow};
use ac2_proto::model::ServerInfo;
use ac2_scene::pairing::{self as words, Half, Halves, PairRefusal, Side, Unseen};

use super::remote_impl::Source;

/// The rig ids rig `info` has admitted (its `server.info`), or why they are not known.
fn admitted(
    info: &Option<Result<ServerInfo, String>>,
) -> Result<Vec<(RigId, String, String)>, Unseen> {
    match info {
        None => Err(Unseen::Asking),
        Some(Err(_)) => Err(Unseen::Unanswered),
        // Off the network a daemon lists no keys: what it admitted is not known.
        Some(Ok(i)) if i.network.status.endpoints().is_none() => Err(Unseen::NetworkOff),
        Some(Ok(i)) => Ok(i
            .network
            .authorized
            .iter()
            .filter_map(|a| Some((a.role.rig_id()?, a.name.clone(), a.key.clone())))
            .collect()),
    }
}

fn connected(r: &RigState) -> bool {
    matches!(r.conn, ConnState::Connected { .. }) && r.daemon().is_some()
}

/// A rig's state when the app holds it and is connected to it now; else why it is not
/// seen. A rig the app lost keeps its last mirror and `server.info`, which are stale.
fn seen(r: Option<&RigState>) -> Result<&RigState, Unseen> {
    match r {
        None => Err(Unseen::NotHeld),
        Some(r) if !connected(r) => Err(Unseen::NotConnected),
        Some(r) => Ok(r),
    }
}

/// The server key a rig serves under now (`None`: it has none yet), or why it is not seen.
fn key_now(r: Result<&RigState, Unseen>) -> Result<Option<&str>, Unseen> {
    match &r?.server {
        None => Err(Unseen::Asking),
        Some(Err(_)) => Err(Unseen::Unanswered),
        Some(Ok(i)) => Ok(i.network.server_key.as_deref()),
    }
}

/// Whether an entry pinned to `pinned` is in place against the key its rig serves under
/// now: an entry under another key is stale (the link would be refused), and one whose
/// rig's key is not seen is there but unchecked.
fn key_half(pinned: &str, now: Result<Option<&str>, Unseen>, side: Side) -> Half {
    match now {
        Ok(Some(k)) if k == pinned => Half::Yes,
        Ok(_) => Half::No,
        Err(why) => Half::Unchecked { side, why },
    }
}

/// Whether `r` (on `side`) lists rig `of`, whose key now is `of_key` (on the other side).
fn lists(
    r: Result<&RigState, Unseen>,
    side: Side,
    of: RigId,
    of_key: Result<Option<&str>, Unseen>,
    of_side: Side,
) -> Half {
    let r = match r {
        Ok(r) => r,
        Err(why) => return Half::Unknown { side, why },
    };
    match r
        .daemon()
        .and_then(|s| s.remote.peers.iter().find(|p| p.rig == of))
    {
        None => Half::No,
        Some(p) => key_half(&p.key, of_key, of_side),
    }
}

/// Whether `r` (on `side`) has admitted rig `of`, whose key now is `of_key`.
fn admits(
    r: Result<&RigState, Unseen>,
    side: Side,
    of: RigId,
    of_key: Result<Option<&str>, Unseen>,
    of_side: Side,
) -> Half {
    let a = match r.and_then(|r| admitted(&r.server)) {
        Ok(a) => a,
        Err(why) => return Half::Unknown { side, why },
    };
    let halves: Vec<Half> = a
        .iter()
        .filter(|(id, ..)| *id == of)
        .map(|(_, _, k)| key_half(k, of_key, of_side))
        .collect();
    if halves.contains(&Half::Yes) {
        Half::Yes
    } else {
        halves
            .into_iter()
            .find(|h| matches!(h, Half::Unchecked { .. }))
            .unwrap_or(Half::No)
    }
}

impl AppState {
    /// The four halves between the rig shown and rig `other`.
    pub fn pair_halves(&self, other: RigId) -> Halves {
        let here = seen(Some(&self.rig));
        let there = seen(self.others.background.iter().find(|r| r.id == other));
        let (here_key, there_key) = (key_now(here), key_now(there));
        let (h, t) = (Side::Here, Side::There);
        Halves {
            here_lists: lists(here, h, other, there_key, t),
            here_admits: admits(here, h, other, there_key, t),
            there_lists: lists(there, t, self.rig.id, here_key, h),
            there_admits: admits(there, t, self.rig.id, here_key, h),
        }
    }

    /// Why the app cannot reach held rig `rig`'s daemon now, if it cannot.
    fn unreached(&self, rig: RigId) -> Option<Unseen> {
        seen(self.others.background.iter().find(|r| r.id == rig)).err()
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
        if connected(here) {
            for (id, name, _) in admitted(&here.server).unwrap_or_default() {
                add(id, named(id, &name));
            }
        }
        for b in self.rigs_in_order() {
            if b.id == here.id || !connected(b) {
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
                let unreached = self.unreached(rig);
                let reachable = unreached.is_none() && connected(&self.rig);
                // What the rig shown's daemon reports, only while it reports it.
                let clocks = self
                    .daemon()
                    .filter(|_| connected(&self.rig))
                    .and_then(|st| {
                        let s = st.remote.standing.iter().find(|s| s.rig == rig)?;
                        Some(ac2_scene::standing::evidence_line(&st.remote, s))
                    });
                PairedRow {
                    rig,
                    text: words::row(&name, &h),
                    state: words::row_state(&h),
                    tip: words::tip(&name, &h, clocks.as_deref(), reachable),
                    halves: h,
                    unreached,
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
        let admit = |admitter: &Source, rig: &Source| Request::ServerCall {
            cmd: Command::AuthAdmitRig {
                key: rig.key.clone(),
                rig_id: rig.id,
                name: admit_name(&rig.name),
            },
            what: ac2_scene::remote_inputs::admitted_toast(&admitter.name, &rig.name),
            failed: words::not_admitted(&admitter.name, &rig.name),
        };
        let list = |lister: &Source, peer: &Source| Request::Act {
            cmd: Command::RigPair { peer: peer.peer() },
            done: Some(words::listed_toast(&lister.name, &peer.name)),
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
            out.push(list(&here, &there));
        }
        if h.there_lists != Half::Yes {
            self.others.elsewhere.push((rig, list(&there, &here)));
        }
        if let Overlay::Settings(s) = &mut self.overlay {
            s.connection.pairing.offers = None;
        }
    }

    /// The open stream of `r`'s remote inputs taken from rig `from`, by name.
    fn inputs_from(&self, r: &RigState, from: RigId) -> Vec<String> {
        r.daemon()
            .and_then(|s| s.stream.open.as_ref())
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
    /// revokes its admission, and so does `rig` when the app reaches it. Refused, with
    /// nothing sent, while the app is not connected to the rig shown (its halves are not
    /// known), or while a stream the app sees takes inputs from the other rig: those inputs
    /// would lose their link under a running stream.
    pub(super) fn unpair(&mut self, rig: RigId, out: &mut Vec<Request>) {
        let me = self.rig_label(&self.rig);
        if !connected(&self.rig) {
            return self.pair_error(words::refusal(&me, &PairRefusal::NotConnected));
        }
        let held = self.others.background.iter().find(|r| r.id == rig);
        let there = held.filter(|r| connected(r));
        let unreached = self.unreached(rig);
        let name = held.map_or_else(
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
        let unlist = |lister: &str, drop: RigId, peer: &str| Request::Act {
            cmd: Command::RigUnpair { rig_id: drop },
            done: Some(words::unlisted_toast(lister, peer)),
            failed: words::not_unlisted(lister, peer),
        };
        // By the rig's identity, so an app authorized under the same name keeps its key.
        let revoke = |admitter: &str, rig_id: RigId, rig: &str| Request::ServerCall {
            cmd: Command::AuthRevokeRig { rig_id },
            what: words::revoked_toast(admitter, rig),
            failed: words::not_revoked(admitter, rig),
        };
        let lists = |r: &RigState, of: RigId| {
            r.daemon()
                .is_some_and(|s| s.remote.peers.iter().any(|p| p.rig == of))
        };
        let mut notes: Vec<String> = Vec::new();
        if lists(&self.rig, rig) {
            out.push(unlist(&me, rig, &name));
        }
        match admitted(&self.rig.server) {
            Ok(a) if a.iter().any(|(id, ..)| *id == rig) => out.push(revoke(&me, rig, &name)),
            Ok(_) => {}
            Err(why) => notes.push(words::unpair_admission_unseen(&name, Side::Here, why)),
        }
        if let Some(t) = there {
            let mut reqs = Vec::new();
            if lists(t, self.rig.id) {
                reqs.push(unlist(&name, self.rig.id, &me));
            }
            match admitted(&t.server) {
                Ok(a) if a.iter().any(|(id, ..)| *id == self.rig.id) => {
                    reqs.push(revoke(&name, self.rig.id, &me));
                }
                Ok(_) => {}
                Err(why) => notes.push(words::unpair_admission_unseen(&name, Side::There, why)),
            }
            self.others
                .elsewhere
                .extend(reqs.into_iter().map(|q| (rig, q)));
        }
        if let Some(why) = unreached {
            let rest = words::unpair_rest(&name, why);
            self.warn(rest.clone());
            notes.insert(0, rest);
        }
        if !notes.is_empty()
            && let Overlay::Settings(s) = &mut self.overlay
        {
            s.connection.notice = Some(notes.join(" · "));
        }
    }
}
