//! The data ROUTER's peers: one per client connection (a DEALER), each with its own
//! subscriptions, byte window, credit, delta chains and event bound.
//!
//! - **Credit.** At most the peer's window of frame bytes is in flight (each frame charged at
//!   least [`FRAME_CHARGE_MIN`]); a frame larger than the window goes alone. A peer out of
//!   credit is skipped until its next credit; meanwhile its topics' slots keep only their
//!   newest frame, so credit buys the newest frame of each topic, never a backlog.
//! - **Rotation.** A peer with credit is served its dirty topics in turn from where its last
//!   round stopped, one message per topic per round: a large frame waiting for credit goes
//!   first when the credit comes, so small topics cannot starve it.
//! - **Events and keepalives** go at once, outside the window, at most [`EVENT_BOUND`]
//!   unacknowledged; past it a peer's events stop, and once it has room it is told
//!   `resync_required` first. A peer that does not subscribe to `ka` still receives one when
//!   nothing else went to it for [`KA_IDLE`]: a send is what tells the daemon a peer is
//!   gone, since a send to a vanished connection fails `EHOSTUNREACH` (`ROUTER_MANDATORY`),
//!   and a connection whose topics are quiet would otherwise never be sent anything.
//! - **Audio** (`audio/<channel>`, served only to another rig's daemon) is a stream, not a
//!   latest value: a frame waiting for credit queues behind the ones before it instead of
//!   replacing them, up to the peer's window, the oldest dropped and counted past that.
//!   Queued audio goes before the rotation, so a remote input's samples arrive in order and
//!   on time while credit lasts. An app is never served audio, and a rig is served nothing
//!   else.
//! - **Lifecycle.** A peer is born with its first message. libzmq gives every connection a
//!   routing id of its own, so a reconnect is a new peer, whose subscriptions bring
//!   keyframes. A peer is dropped when a send to it fails `EHOSTUNREACH`, or when it holds
//!   unacknowledged messages and has said nothing for [`STALLED_AFTER`]; one heard from
//!   again after that is told to `rejoin`.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use ac2_proto::flow::{
    DATA_WINDOW_DEFAULT, DATA_WINDOW_MAX, DATA_WINDOW_MIN, EVENT_BOUND, FRAME_CHARGE_MIN,
    frame_charge,
};
use ac2_proto::{DataUp, FlowNotice, Subscription, Topic, decode_data_request, encode_notice};
use ac2_zmq::{Admission, Error as ZmqError, Part, PublicKey, Socket};

use crate::chain::{Chain, Encoded, Encodes, Slot};
use crate::control::CurvePeer;
use crate::io::Interest;
use crate::refusals::Refusals;
use crate::util::hex;

/// Send queue of the data ROUTER per peer, in messages: twice what credit and the event
/// bound let be outstanding (the largest window's frames at the least charge, one frame
/// larger than the window, and the event bound). libzmq tells the writing side how much
/// the connection has taken from the queue only every half queue, so the writer may count
/// up to half a queue too many: twice the bound keeps a credited message from ever meeting
/// a full queue. `EAGAIN` on a data send is a bug, logged as one.
pub(crate) const DATA_SNDHWM: u32 = 2 * (DATA_WINDOW_MAX / FRAME_CHARGE_MIN + 1 + EVENT_BOUND);

/// A peer that holds unacknowledged messages and has sent nothing (no credit, no request)
/// for this long is dropped. A live reader returns credit for what it reads, the keepalive
/// a quiet connection is sent each second included; one that read nothing for a minute has
/// stopped, and if it reads again it is told to rejoin.
pub(crate) const STALLED_AFTER: Duration = Duration::from_secs(60);

/// Longest a peer goes without being sent anything while keepalives are produced.
pub(crate) const KA_IDLE: Duration = Duration::from_secs(1);

/// What went to a peer outside the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outside {
    Event,
    Ka,
}

/// The peer's connection is gone.
#[derive(Debug)]
struct Gone;

#[derive(Debug)]
struct Peer {
    rid: Part,
    window: u32,
    /// Frame charge sent and not yet credited.
    in_flight: u64,
    /// Events, keepalives and notices sent and not yet credited.
    events_out: u32,
    subs: Vec<Subscription>,
    chains: HashMap<Topic, Chain>,
    /// Where the next round starts, an index into the topic order.
    cursor: usize,
    /// Out of credit: skipped until the next credit.
    blocked: bool,
    /// The frame that waits for credit, formed: (topic, slot generation, message).
    staged: Option<(Topic, u64, Rc<Encoded>)>,
    resync_owed: bool,
    heard: Instant,
    /// When anything was last handed to libzmq for it.
    last_out: Option<Instant>,
    /// The CURVE key it connected with; `None` on a local connection.
    key: Option<PublicKey>,
    /// Another rig's daemon: served audio and keepalives, nothing else.
    rig: bool,
    /// Audio frames waiting for credit, oldest first, and their charge.
    audio: VecDeque<Rc<Vec<Part>>>,
    audio_queued: u64,
    /// Audio frames dropped because the queue was full.
    audio_dropped: u64,
}

/// Charge of an audio frame on the wire.
fn audio_charge(parts: &[Part]) -> u64 {
    u64::from(frame_charge(parts.iter().map(|p| p.len()).sum()))
}

/// Sends `[rid][topic][body]` without blocking.
fn send(sock: &Socket, rid: &Part, parts: &[Part]) -> Result<(), ZmqError> {
    let mut v: Vec<&Part> = Vec::with_capacity(1 + parts.len());
    v.push(rid);
    v.extend(parts);
    sock.try_send_shared(&v)
}

impl Peer {
    fn new(rid: &[u8], now: Instant, key: Option<PublicKey>, rig: bool) -> Result<Self, ZmqError> {
        Ok(Self {
            rid: Part::copy_from(rid)?,
            window: DATA_WINDOW_DEFAULT,
            in_flight: 0,
            events_out: 0,
            subs: Vec::new(),
            chains: HashMap::new(),
            cursor: 0,
            blocked: false,
            staged: None,
            resync_owed: false,
            heard: now,
            last_out: None,
            key,
            rig,
            audio: VecDeque::new(),
            audio_queued: 0,
            audio_dropped: 0,
        })
    }

    /// Queues an audio frame, dropping the oldest past the window, then sends what credit
    /// allows.
    fn audio(&mut self, sock: &Socket, frame: &Rc<Vec<Part>>) -> Result<(), Gone> {
        let charge = audio_charge(frame);
        while !self.audio.is_empty() && self.audio_queued + charge > u64::from(self.window) {
            if let Some(old) = self.audio.pop_front() {
                self.audio_queued -= audio_charge(&old);
                self.audio_dropped += 1;
                if self.audio_dropped.is_power_of_two() {
                    tracing::warn!(
                        "rig peer {} reads audio too slowly: {} frame(s) dropped",
                        self.name(),
                        self.audio_dropped
                    );
                }
            }
        }
        self.audio.push_back(Rc::clone(frame));
        self.audio_queued += charge;
        self.flush_audio(sock)
    }

    /// Sends queued audio while credit lasts; a frame larger than the window goes alone.
    fn flush_audio(&mut self, sock: &Socket) -> Result<(), Gone> {
        while let Some(front) = self.audio.front() {
            let charge = audio_charge(front);
            if self.in_flight > 0 && self.in_flight + charge > u64::from(self.window) {
                return Ok(());
            }
            let parts: Vec<&Part> = std::iter::once(&self.rid).chain(front.iter()).collect();
            match sock.try_send_shared(&parts) {
                Ok(()) => {
                    self.in_flight += charge;
                    self.last_out = Some(Instant::now());
                }
                Err(ZmqError::HostUnreachable) => return Err(Gone),
                Err(ZmqError::WouldBlock) => {
                    tracing::error!(
                        "bug: data socket refused credited audio to peer {}",
                        self.name()
                    );
                    return Ok(());
                }
                Err(e) => tracing::warn!("audio send to peer {}: {e}", self.name()),
            }
            self.audio_queued -= charge;
            self.audio.pop_front();
        }
        Ok(())
    }

    /// Whether this peer may hold `sub`: audio only for a rig, everything else only for an
    /// app. A rig is sent no keepalive either: it carries this rig's generator and health,
    /// and the other rig learns this one is up from its `source.state` reads.
    fn may_subscribe(&self, sub: &Subscription) -> bool {
        let audio = matches!(sub, Subscription::Topic(Topic::Audio { .. }));
        audio == self.rig
    }

    fn wants(&self, t: &Topic) -> bool {
        self.subs.iter().any(|s| s.matches(t))
    }

    fn name(&self) -> String {
        hex(&self.rid)
    }

    /// A notice, counted against the event bound.
    fn notice(&mut self, sock: &Socket, n: FlowNotice) -> Result<bool, Gone> {
        let parts: Result<Vec<Part>, String> =
            encode_notice(n).map_err(|e| e.to_string()).and_then(|v| {
                v.into_iter()
                    .map(|b| Part::from_vec(b).map_err(|e| e.to_string()))
                    .collect()
            });
        let parts = match parts {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("notice not formed: {e}");
                return Ok(false);
            }
        };
        self.outside(sock, &parts)
    }

    /// One message outside the window. `Ok(false)`: not queued.
    fn outside(&mut self, sock: &Socket, parts: &[Part]) -> Result<bool, Gone> {
        match send(sock, &self.rid, parts) {
            Ok(()) => {
                self.events_out += 1;
                self.last_out = Some(Instant::now());
                Ok(true)
            }
            Err(ZmqError::HostUnreachable) => Err(Gone),
            Err(ZmqError::WouldBlock) => {
                tracing::error!(
                    "bug: data socket queue of peer {} full below its bounds",
                    self.name()
                );
                Ok(false)
            }
            Err(e) => {
                tracing::warn!("data send to peer {}: {e}", self.name());
                Ok(false)
            }
        }
    }

    /// `resync_required`, owed since events were dropped, once there is room for it.
    fn flush_resync(&mut self, sock: &Socket) -> Result<(), Gone> {
        if self.resync_owed
            && self.events_out < EVENT_BOUND
            && self.notice(sock, FlowNotice::ResyncRequired)?
        {
            self.resync_owed = false;
        }
        Ok(())
    }

    fn event(
        &mut self,
        sock: &Socket,
        parts: &[Part],
        kind: Outside,
        now: Instant,
    ) -> Result<(), Gone> {
        let wanted = match kind {
            Outside::Event => self.wants(&Topic::Evt),
            Outside::Ka if self.rig => false,
            Outside::Ka => {
                self.wants(&Topic::Ka)
                    || self
                        .last_out
                        .is_none_or(|t| now.saturating_duration_since(t) >= KA_IDLE)
            }
        };
        if !wanted {
            return Ok(());
        }
        self.flush_resync(sock)?;
        if self.resync_owed || self.events_out >= EVENT_BOUND {
            // A keepalive is superseded by the next one; an event is covered by the snapshot
            // `resync_required` brings.
            if kind == Outside::Event {
                self.resync_owed = true;
            }
            return Ok(());
        }
        if !self.outside(sock, parts)? && kind == Outside::Event {
            self.resync_owed = true;
        }
        Ok(())
    }

    /// One round: the peer's dirty topics in turn, from its cursor, while credit lasts.
    #[allow(clippy::too_many_arguments)]
    fn serve(
        &mut self,
        sock: &Socket,
        slots: &HashMap<Topic, Slot>,
        order: &[Topic],
        now: Instant,
        min_gap: Duration,
        encodes: &mut Encodes,
        due: &mut Option<Instant>,
    ) -> Result<(), Gone> {
        self.flush_audio(sock)?;
        let n = order.len();
        if self.blocked || n == 0 {
            return Ok(());
        }
        let start = self.cursor % n;
        for i in 0..n {
            let at = (start + i) % n;
            let topic = order[at];
            let Some(slot) = slots.get(&topic) else {
                continue;
            };
            if !self.wants(&topic) {
                continue;
            }
            let chain = self.chains.entry(topic).or_default();
            if !chain.dirty(slot) {
                continue;
            }
            if let Some(t) = chain.ready_at(min_gap)
                && t > now
            {
                *due = Some(due.map_or(t, |d| d.min(t)));
                continue;
            }
            let staged = match self.staged.take() {
                Some((t, g, e)) if t == topic && g == slot.put => Some(e),
                other => {
                    self.staged = other;
                    None
                }
            };
            let Some(e) = staged.or_else(|| chain.form(slot, now, encodes)) else {
                continue;
            };
            let charge = u64::from(frame_charge(e.len));
            if self.in_flight > 0 && self.in_flight + charge > u64::from(self.window) {
                self.blocked = true;
                self.cursor = at;
                self.staged = Some((topic, slot.put, e));
                return Ok(());
            }
            match send(sock, &self.rid, &e.parts) {
                Ok(()) => {
                    chain.sent(slot, &e, now);
                    self.in_flight += charge;
                    self.cursor = at + 1;
                    self.last_out = Some(now);
                }
                Err(ZmqError::HostUnreachable) => return Err(Gone),
                Err(ZmqError::WouldBlock) => {
                    tracing::error!(
                        "bug: data socket refused a credited frame of {topic} to peer {}",
                        hex(&self.rid)
                    );
                    chain.refused();
                    self.blocked = true;
                    self.cursor = at;
                    return Ok(());
                }
                Err(err) => {
                    tracing::warn!("{topic}: data send to peer {}: {err}", hex(&self.rid));
                    chain.refused();
                }
            }
        }
        Ok(())
    }
}

/// Every peer of the data ROUTER.
#[derive(Debug, Default)]
pub(crate) struct Peers {
    map: HashMap<Vec<u8>, Peer>,
    encodes: Encodes,
    /// Undecodable requests already warned about.
    refusals: Refusals,
}

fn subscribe(interest: &Interest, s: &Subscription) {
    interest.add(&s.prefix());
}

impl Peers {
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    /// One message from a peer: `[routing id][request]`, from `client` when it passed CURVE;
    /// `admission` tells what that client's key is admitted as now.
    pub(crate) fn request(
        &mut self,
        sock: &Socket,
        client: Option<&CurvePeer>,
        frames: Vec<Vec<u8>>,
        interest: &Interest,
        now: Instant,
        admission: impl FnOnce(&CurvePeer) -> Admission,
    ) {
        let [rid, body] = frames.as_slice() else {
            tracing::warn!(
                "data message with {} frames dropped (expected routing id + 1)",
                frames.len()
            );
            return;
        };
        // Checked on every message: the handshake checked the key once, and a connection
        // outlives a revoke (or a kind change) unless each message is held to the list.
        let rig = match client.map(|c| (c, admission(c))) {
            None => false,
            Some((c, Admission::Revoked)) => {
                if self.refusals.first(Some(c.key), "revoked") {
                    tracing::warn!(
                        "data from revoked client {:?} dropped (repeats at debug)",
                        c.name
                    );
                }
                self.drop_peer(rid, interest, "its key was revoked");
                return;
            }
            Some((_, Admission::App)) => false,
            Some((_, Admission::Rig(_))) => true,
        };
        if self.map.get(rid.as_slice()).is_some_and(|p| p.rig != rig) {
            // What it may receive changed with its kind: it starts over as the new kind.
            self.drop_peer(rid, interest, "its key was admitted as another kind");
        }
        let req = match decode_data_request(body) {
            Ok(r) => r,
            Err(e) => {
                let why = e.to_string();
                let who = client.map_or("a local client", |c| c.name.as_str());
                if self.refusals.first(client.map(|c| c.key), &why) {
                    tracing::warn!("data request from {who} dropped: {why} (repeats at debug)");
                } else {
                    tracing::debug!("data request from {who} ({}) dropped: {why}", hex(rid));
                }
                return;
            }
        };
        let known = self.map.contains_key(rid.as_slice());
        if !known {
            match Peer::new(rid, now, client.map(|c| c.key), rig) {
                Ok(p) => {
                    tracing::debug!("data peer {} joined", hex(rid));
                    // Every app is sent keepalives, whatever it subscribes to.
                    if !rig {
                        interest.add(&Topic::Ka.to_bytes());
                    }
                    self.map.insert(rid.clone(), p);
                }
                Err(e) => {
                    tracing::warn!("data peer {}: {e}", hex(rid));
                    return;
                }
            }
        }
        let Some(p) = self.map.get_mut(rid.as_slice()) else {
            return;
        };
        p.heard = now;
        // A peer the daemon has no state for, other than one stating its window or a
        // subscription (a new connection does), is one that was dropped as stalled.
        let rejoin = !known && !matches!(req.msg, DataUp::Window { .. } | DataUp::Subscribe { .. });
        let mut gone = false;
        match req.msg {
            DataUp::Window { bytes } => {
                p.window = bytes.clamp(DATA_WINDOW_MIN, DATA_WINDOW_MAX);
                p.blocked = false;
            }
            DataUp::Subscribe { sub } => {
                if !p.may_subscribe(&sub) {
                    let who = client.map_or("a local client", |c| c.name.as_str());
                    if self
                        .refusals
                        .first(client.map(|c| c.key), "subscription not served")
                    {
                        tracing::warn!(
                            "{who} may not subscribe {:?}: audio goes only to another rig, \
                             and a rig is sent only audio (repeats at debug)",
                            sub
                        );
                    }
                } else if !p.subs.contains(&sub) {
                    tracing::debug!("peer {} subscribes {:?}", p.name(), sub);
                    subscribe(interest, &sub);
                    p.subs.push(sub);
                }
            }
            DataUp::Unsubscribe { sub } => {
                if let Some(i) = p.subs.iter().position(|s| *s == sub) {
                    p.subs.swap_remove(i);
                    interest.remove(&sub.prefix());
                    let subs = &p.subs;
                    p.chains.retain(|t, _| subs.iter().any(|s| s.matches(t)));
                }
            }
            DataUp::Keyframe { topic } => {
                if let Some(c) = p.chains.get_mut(&topic) {
                    c.ask_key(now);
                }
            }
            DataUp::Credit { bytes, events } => {
                p.in_flight = p.in_flight.saturating_sub(u64::from(bytes));
                p.events_out = p.events_out.saturating_sub(events);
                p.blocked = false;
                gone = p.flush_resync(sock).is_err() || p.flush_audio(sock).is_err();
            }
        }
        if rejoin && !gone {
            gone = p.notice(sock, FlowNotice::Rejoin).is_err();
        }
        if gone {
            self.drop_peer(rid, interest, "its connection is gone");
        }
    }

    /// An event or keepalive, `[topic][body]`, to every peer that receives it.
    pub(crate) fn outside(
        &mut self,
        sock: &Socket,
        parts: &[Part],
        kind: Outside,
        interest: &Interest,
    ) {
        let now = Instant::now();
        let gone: Vec<Vec<u8>> = self
            .map
            .iter_mut()
            .filter_map(|(rid, p)| p.event(sock, parts, kind, now).err().map(|_| rid.clone()))
            .collect();
        for rid in gone {
            self.drop_peer(&rid, interest, "its connection is gone");
        }
    }

    /// An audio frame, `[topic][header][samples]`, queued to every peer subscribed to it.
    pub(crate) fn audio(
        &mut self,
        sock: &Socket,
        topic: Topic,
        parts: Vec<Part>,
        interest: &Interest,
    ) {
        let frame = Rc::new(parts);
        let gone: Vec<Vec<u8>> = self
            .map
            .iter_mut()
            .filter(|(_, p)| p.rig && p.wants(&topic))
            .filter_map(|(rid, p)| p.audio(sock, &frame).err().map(|_| rid.clone()))
            .collect();
        for rid in gone {
            self.drop_peer(&rid, interest, "its connection is gone");
        }
    }

    /// Drops every peer connected with `key`, whose authorization was just withdrawn: a
    /// revoked rig must stop receiving audio now, not at its next handshake.
    pub(crate) fn drop_key(&mut self, key: &PublicKey, interest: &Interest) {
        let rids: Vec<Vec<u8>> = self
            .map
            .iter()
            .filter(|(_, p)| p.key.as_ref() == Some(key))
            .map(|(rid, _)| rid.clone())
            .collect();
        for rid in rids {
            self.drop_peer(&rid, interest, "its key was revoked");
        }
    }

    /// Audio frames dropped for slow readers, over every peer now connected.
    #[cfg(test)]
    pub(crate) fn audio_dropped(&self) -> u64 {
        self.map.values().map(|p| p.audio_dropped).sum()
    }

    /// One send round over every peer. Returns when a frame held back by the shortest
    /// interval between frames of a topic may go.
    pub(crate) fn serve(
        &mut self,
        sock: &Socket,
        slots: &HashMap<Topic, Slot>,
        order: &[Topic],
        now: Instant,
        min_gap: Duration,
        interest: &Interest,
    ) -> Option<Instant> {
        let mut due = None;
        self.encodes.clear();
        let mut gone = Vec::new();
        for (rid, p) in &mut self.map {
            if p.serve(
                sock,
                slots,
                order,
                now,
                min_gap,
                &mut self.encodes,
                &mut due,
            )
            .is_err()
            {
                gone.push(rid.clone());
            }
        }
        // Encodes hold message parts; no send round reuses another's.
        self.encodes.clear();
        for rid in gone {
            self.drop_peer(&rid, interest, "its connection is gone");
        }
        due
    }

    /// Drops peers that stopped reading ([`STALLED_AFTER`]).
    pub(crate) fn sweep(&mut self, now: Instant, interest: &Interest) {
        let stalled: Vec<Vec<u8>> = self
            .map
            .iter()
            .filter(|(_, p)| {
                (p.in_flight > 0 || p.events_out > 0)
                    && now.saturating_duration_since(p.heard) >= STALLED_AFTER
            })
            .map(|(rid, _)| rid.clone())
            .collect();
        for rid in stalled {
            self.drop_peer(&rid, interest, "it stopped reading");
        }
    }

    /// Topics under `prefix` are gone (`TAG_CLEAR`).
    pub(crate) fn clear(&mut self, prefix: &[u8]) {
        for p in self.map.values_mut() {
            p.chains.retain(|t, _| !t.to_bytes().starts_with(prefix));
            if p.staged
                .as_ref()
                .is_some_and(|(t, ..)| t.to_bytes().starts_with(prefix))
            {
                p.staged = None;
            }
        }
    }

    fn drop_peer(&mut self, rid: &[u8], interest: &Interest, why: &str) {
        let Some(p) = self.map.remove(rid) else {
            return;
        };
        for s in &p.subs {
            interest.remove(&s.prefix());
        }
        if !p.rig {
            interest.remove(&Topic::Ka.to_bytes());
        }
        tracing::info!(
            "data peer {} dropped ({why}) with {} subscription(s)",
            p.name(),
            p.subs.len()
        );
    }
}

#[cfg(test)]
#[path = "peers_tests.rs"]
mod tests;
