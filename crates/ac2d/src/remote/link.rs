//! The link to one paired rig (`docs/design/remote-inputs.md` §2.2, §2.3): a thread of its
//! own with two sync DEALERs on the other rig's ctrl and data sockets, under CURVE with this
//! rig's own server key pair as the client key, pinned to the key the app gave.
//!
//! It reads the other rig's filtered state (`source.state`) once a second, subscribes the
//! audio of the channels the open session takes from it, handing each frame to the fan-out
//! through a bounded channel, and always subscribes its levels (`source/levels`), whose
//! timing marks relate the two rigs' clocks with no audio ([`super::standing`]). libzmq
//! reconnects on its own (backing off up to [`RECONNECT_MAX`]); a completed handshake
//! brings `hello` again and the subscriptions again. The link never runs on the fan-out's
//! thread and needs no async runtime.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::flow::{DATA_WINDOW_MAX, FLOW_TOPIC, frame_charge, outside_window};
use ac2_proto::frame::{AudioFrame, CodedFrame, FrameData, WireFrame, decode_audio, decode_wire};
use ac2_proto::model::{RemoteLink, RigPeer, SourceState};
use ac2_proto::units::{RequestId, WallNs};
use ac2_proto::{
    Command, DataRequest, DataUp, ErrorCode, FlowNotice, ReplyBody, Request, Subscription, Topic,
    decode_notice, decode_reply, encode_data_request, encode_request,
};
use ac2_zmq::{
    Context, CurveClient, KeyPair, Monitor, MonitorEvent, PollItem, PublicKey, Socket, SocketType,
    poll,
};

use crate::control::ControlMsg;
use crate::util::wall_ns;

use super::RemoteMsg;
use super::standing::{LocalClock, StandingFeed};

/// How often the other rig's filtered state is read: its clocks' map is refitted every 5 s,
/// and a session it reopens should show here within a second or so.
const SOURCE_EVERY: Duration = Duration::from_secs(1);
/// A connection whose `source.state` goes unanswered this long is not reaching the rig
/// (a half-open TCP connection looks connected).
const SILENT_AFTER: Duration = Duration::from_secs(5);
/// Unreachable once no handshake completed for this long since the link began trying.
const UNREACHABLE_AFTER: Duration = Duration::from_secs(3);
/// Reconnect backoff: libzmq doubles from the first interval up to this.
const RECONNECT_FIRST: Duration = Duration::from_millis(250);
const RECONNECT_MAX: Duration = Duration::from_secs(5);
/// Handshakes that end with the connection closed before CURVE finished, neither admitted
/// nor refused: the other rig could not open what this one sent, i.e. it holds another key
/// than the one pinned. More than one, so a rig restarting mid-handshake is not taken for it.
const KEY_FAILURES: u32 = 3;
/// The rates are reported once a second.
const REPORT_EVERY: Duration = Duration::from_secs(1);
/// The poll's longest sleep, so commands are taken promptly.
const POLL: Duration = Duration::from_millis(50);

/// One frame from the other rig for the fan-out.
pub(crate) enum LinkFrame {
    Audio(AudioFrame),
    /// The other rig's state as last read.
    Source(Box<SourceState>),
}

/// What the link is told.
pub(crate) enum LinkCmd {
    /// Take the audio of `channels` (the other rig's device inputs) into `to`; `None`: no
    /// session takes anything from this rig.
    Feed {
        channels: Vec<u16>,
        to: Option<SyncSender<LinkFrame>>,
    },
    Stop,
}

/// A running link.
pub(crate) struct LinkHandle {
    pub(crate) peer: RigPeer,
    tx: Sender<LinkCmd>,
    thread: Option<JoinHandle<()>>,
}

impl LinkHandle {
    pub(crate) fn feed(&self, channels: Vec<u16>, to: Option<SyncSender<LinkFrame>>) {
        let _ = self.tx.send(LinkCmd::Feed { channels, to });
    }
}

impl Drop for LinkHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(LinkCmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Starts the link to `peer`, connecting as `keys`.
pub(crate) fn spawn(
    peer: RigPeer,
    keys: KeyPair,
    ctx: Context,
    to_control: Sender<ControlMsg>,
    local: Arc<LocalClock>,
) -> std::io::Result<LinkHandle> {
    let server_key = PublicKey::from_z85(&peer.key)
        .map_err(|_| std::io::Error::other(format!("{}: not a server key", peer.name)))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let p = peer.clone();
    let thread = std::thread::Builder::new()
        .name(format!("ac2d-link-{:x}", peer.rig.0))
        .spawn(move || {
            let curve = CurveClient { keys, server_key };
            match Link::open(&ctx, &p, &curve, local) {
                Ok(mut l) => l.run(&rx, &to_control),
                Err(e) => {
                    tracing::error!("link to {}: {e}", p.name);
                    report(
                        &to_control,
                        &p,
                        RemoteLink::Unreachable {
                            since: WallNs(wall_ns()),
                        },
                        0.0,
                        0,
                    );
                }
            }
        })?;
    Ok(LinkHandle {
        peer,
        tx,
        thread: Some(thread),
    })
}

fn report(
    to: &Sender<ControlMsg>,
    peer: &RigPeer,
    link: RemoteLink,
    bytes_per_s: f64,
    window: u32,
) {
    let _ = to.send(ControlMsg::Remote(RemoteMsg::Link {
        rig: peer.rig,
        link,
        bytes_per_s,
        window,
    }));
}

fn dealer(
    ctx: &Context,
    curve: &CurveClient,
    endpoint: &str,
    max_part: usize,
) -> ac2_zmq::Result<(Socket, Monitor)> {
    let s = ctx.socket(SocketType::Dealer)?;
    // The other rig is trusted only as far as its key: a part past what the protocol
    // allows closes the connection instead of being buffered whole.
    s.set_max_message_size(Some(max_part + ac2_proto::TRANSPORT_SLACK_BYTES))?;
    s.set_linger(Some(Duration::ZERO))?;
    s.set_reconnect_interval(RECONNECT_FIRST)?;
    s.set_reconnect_interval_max(Some(RECONNECT_MAX))?;
    s.set_curve_client(curve)?;
    let m = s.monitor()?;
    s.connect(endpoint)?;
    Ok((s, m))
}

/// The handshake history of one socket.
#[derive(Default)]
struct Handshakes {
    /// A connection is up and no handshake ended on it yet.
    pending: bool,
    /// Consecutive connections closed mid-handshake.
    failed: u32,
    refused: bool,
    up: bool,
}

impl Handshakes {
    /// Takes one event; whether a handshake just completed.
    fn event(&mut self, e: MonitorEvent) -> bool {
        match e {
            MonitorEvent::Connected => self.pending = true,
            MonitorEvent::HandshakeSucceeded => {
                (self.pending, self.failed, self.refused, self.up) = (false, 0, false, true);
                return true;
            }
            MonitorEvent::HandshakeFailedAuth { .. } => {
                (self.pending, self.refused, self.up) = (false, true, false);
            }
            MonitorEvent::HandshakeFailedProtocol { .. }
            | MonitorEvent::HandshakeFailedNoDetail { .. } => {
                (self.pending, self.up) = (false, false);
                self.failed += 1;
            }
            MonitorEvent::Disconnected | MonitorEvent::Closed => {
                if self.pending {
                    self.failed += 1;
                }
                (self.pending, self.up) = (false, false);
            }
            _ => {}
        }
        false
    }
}

/// What the link's state is read from: the handshakes, the replies and the notices, apart
/// from the sockets so that the reading is tested on its own.
struct Health {
    ctrl_hs: Handshakes,
    data_hs: Handshakes,
    /// The other rig answered `hello` as the rig paired.
    welcomed: bool,
    /// It answered as another rig.
    other_rig: Option<ac2_proto::units::RigId>,
    /// When the last reply came, or the link began.
    heard: Instant,
    began: Instant,
    unreachable_since: Option<WallNs>,
    /// The other rig said it turned its network off and has not been heard since: cleared
    /// by any welcome and by a handshake that fails (something else answers there), and the
    /// link to an address it moved to is a new link.
    network_off: bool,
}

impl Health {
    fn new(now: Instant) -> Self {
        Self {
            ctrl_hs: Handshakes::default(),
            data_hs: Handshakes::default(),
            welcomed: false,
            other_rig: None,
            heard: now,
            began: now,
            unreachable_since: None,
            network_off: false,
        }
    }

    /// The `network_off` notice: its connections are about to end.
    fn told_network_off(&mut self) {
        self.network_off = true;
        self.welcomed = false;
    }

    /// A `hello` answered as `found`; whether that is the rig paired, `paired`.
    fn welcome(&mut self, found: ac2_proto::units::RigId, paired: ac2_proto::units::RigId) -> bool {
        self.network_off = false;
        let ok = found == paired;
        self.welcomed = ok;
        self.other_rig = (!ok).then_some(found);
        ok
    }

    /// One event of the ctrl (`ctrl`) or data socket; whether a handshake just completed.
    fn event(&mut self, ctrl: bool, e: MonitorEvent) -> bool {
        if matches!(
            e,
            MonitorEvent::HandshakeFailedAuth { .. }
                | MonitorEvent::HandshakeFailedProtocol { .. }
                | MonitorEvent::HandshakeFailedNoDetail { .. }
        ) {
            self.network_off = false;
        }
        if ctrl {
            self.ctrl_hs.event(e)
        } else {
            self.data_hs.event(e)
        }
    }

    fn status(&mut self, now: Instant) -> RemoteLink {
        if let Some(found) = self.other_rig {
            return RemoteLink::OtherRig { found };
        }
        if self.ctrl_hs.refused || self.data_hs.refused {
            return RemoteLink::NotAdmitted;
        }
        if self.ctrl_hs.failed >= KEY_FAILURES || self.data_hs.failed >= KEY_FAILURES {
            return RemoteLink::KeyChanged;
        }
        // Said before the connection ended: the reason, not a loss, until it is back.
        if self.network_off {
            return RemoteLink::NetworkOff;
        }
        let silent = now.saturating_duration_since(self.heard) > SILENT_AFTER;
        if self.welcomed && !silent {
            self.unreachable_since = None;
            return RemoteLink::Up;
        }
        if now.saturating_duration_since(self.began) > UNREACHABLE_AFTER || silent {
            let since = *self.unreachable_since.get_or_insert(WallNs(wall_ns()));
            return RemoteLink::Unreachable { since };
        }
        RemoteLink::Connecting
    }
}

struct Link<'p> {
    peer: &'p RigPeer,
    ctrl: Socket,
    ctrl_mon: Monitor,
    data: Socket,
    data_mon: Monitor,
    h: Health,
    next_id: u64,
    hello: Option<RequestId>,
    poll: Option<(RequestId, Instant)>,
    next_poll: Instant,
    channels: Vec<u16>,
    to: Option<SyncSender<LinkFrame>>,
    /// Owed credit: frame bytes and outside-window messages.
    owed: (u32, u32),
    bytes: u64,
    next_report: Instant,
    last: Option<RemoteLink>,
    /// The source's last refusal as logged: the link polls about once a second, and a
    /// standing refusal is one fact, not one log line per poll.
    warned: Option<String>,
    /// This rig's own timing, as the fan-out writes it.
    local: Arc<LocalClock>,
    standing: StandingFeed,
    /// The newest `source/levels` frame as coded, the base of the next delta.
    held: Option<CodedFrame>,
    /// When a keyframe was last asked for, after a delta that did not apply.
    asked: Option<Instant>,
    /// The other rig's device period, frames, from its last `source.state`.
    their_period: Option<u32>,
}

/// A delta that does not apply asks for a keyframe, at most this often.
const KEYFRAME_ASK_GAP: Duration = Duration::from_millis(500);

impl<'p> Link<'p> {
    fn open(
        ctx: &Context,
        peer: &'p RigPeer,
        curve: &CurveClient,
        local: Arc<LocalClock>,
    ) -> ac2_zmq::Result<Self> {
        let (ctrl, ctrl_mon) = dealer(ctx, curve, &peer.ctrl, ac2_proto::MAX_CTRL_BYTES)?;
        let (data, data_mon) = dealer(ctx, curve, &peer.data, ac2_proto::MAX_DATA_PART_BYTES)?;
        let now = Instant::now();
        Ok(Self {
            peer,
            ctrl,
            ctrl_mon,
            data,
            data_mon,
            h: Health::new(now),
            next_id: 1,
            hello: None,
            poll: None,
            next_poll: now,
            channels: Vec::new(),
            to: None,
            owed: (0, 0),
            bytes: 0,
            next_report: now,
            last: None,
            warned: None,
            local,
            standing: StandingFeed::new(),
            held: None,
            asked: None,
            their_period: None,
        })
    }

    fn id(&mut self) -> RequestId {
        self.next_id += 1;
        RequestId(self.next_id)
    }

    fn ask(&mut self, cmd: Command) -> Option<RequestId> {
        let id = self.id();
        let b = encode_request(&Request::new(id, cmd)).ok()?;
        self.ctrl.try_send(&[b]).ok()?;
        Some(id)
    }

    fn up(&self, msg: DataUp) {
        if let Ok(b) = encode_data_request(&DataRequest::new(msg)) {
            let _ = self.data.try_send(&[b]);
        }
    }

    /// The window and every subscription, as a new connection states them.
    fn join(&self) {
        self.up(DataUp::Window {
            bytes: DATA_WINDOW_MAX,
        });
        self.up(DataUp::Subscribe {
            sub: Subscription::Topic(Topic::SourceLevels),
        });
        for &channel in &self.channels {
            self.up(DataUp::Subscribe {
                sub: Subscription::Topic(Topic::Audio { channel }),
            });
        }
    }

    fn set_feed(&mut self, channels: Vec<u16>, to: Option<SyncSender<LinkFrame>>) {
        for &c in &self.channels {
            if !channels.contains(&c) {
                self.up(DataUp::Unsubscribe {
                    sub: Subscription::Topic(Topic::Audio { channel: c }),
                });
            }
        }
        for &c in &channels {
            if !self.channels.contains(&c) {
                self.up(DataUp::Subscribe {
                    sub: Subscription::Topic(Topic::Audio { channel: c }),
                });
            }
        }
        self.channels = channels;
        self.to = to;
    }

    fn run(&mut self, rx: &Receiver<LinkCmd>, to_control: &Sender<ControlMsg>) {
        loop {
            loop {
                match rx.try_recv() {
                    Ok(LinkCmd::Feed { channels, to }) => self.set_feed(channels, to),
                    Ok(LinkCmd::Stop) | Err(TryRecvError::Disconnected) => return,
                    Err(TryRecvError::Empty) => break,
                }
            }
            let ready = {
                let mut items = [
                    PollItem::readable(&self.ctrl),
                    PollItem::readable(&self.data),
                    PollItem::readable(self.ctrl_mon.socket()),
                    PollItem::readable(self.data_mon.socket()),
                ];
                if poll(&mut items, Some(POLL)).is_err() {
                    std::thread::sleep(POLL);
                }
                items.map(|i| i.is_readable())
            };
            let now = Instant::now();
            if ready[2] {
                while let Ok(Some(e)) = self.ctrl_mon.try_recv() {
                    if self.h.event(true, e.event) {
                        // A new connection: the other rig may be another daemon now.
                        self.h.welcomed = false;
                        self.h.other_rig = None;
                        self.poll = None;
                        self.hello = self.ask(Command::Hello {
                            client: format!("ac2d {}", env!("CARGO_PKG_VERSION")),
                        });
                    }
                }
            }
            if ready[3] {
                while let Ok(Some(e)) = self.data_mon.try_recv() {
                    if self.h.event(false, e.event) {
                        self.join();
                    }
                }
            }
            if ready[0] {
                self.read_ctrl(to_control, now);
            }
            if ready[1] {
                self.read_data();
            }
            if self.h.welcomed && self.poll.is_none() && now >= self.next_poll {
                self.next_poll = now + SOURCE_EVERY;
                self.poll = self.ask(Command::SourceState).map(|id| (id, now));
            }
            if let Some((_, at)) = self.poll
                && now.saturating_duration_since(at) > SILENT_AFTER
            {
                // Unanswered: ask again; the status says unreachable meanwhile.
                self.poll = None;
            }
            if now >= self.next_report {
                let elapsed = now.saturating_duration_since(self.next_report - REPORT_EVERY);
                let rate = self.bytes as f64 / elapsed.as_secs_f64().max(1e-3);
                self.bytes = 0;
                self.next_report = now + REPORT_EVERY;
                let link = self.h.status(now);
                if self.last.as_ref() != Some(&link) {
                    tracing::info!("link to {}: {link:?}", self.peer.name);
                    self.last = Some(link.clone());
                }
                report(to_control, self.peer, link, rate, DATA_WINDOW_MAX);
                let report = self.standing.report(now);
                let _ = to_control.send(ControlMsg::Remote(RemoteMsg::Standing {
                    rig: self.peer.rig,
                    report: Box::new(report),
                }));
            }
        }
    }

    fn read_ctrl(&mut self, to_control: &Sender<ControlMsg>, now: Instant) {
        while let Ok(Some(m)) = self.ctrl.try_recv() {
            let Some(body) = m.frames().first() else {
                continue;
            };
            let reply = match decode_reply(body) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("link to {}: reply not read: {e}", self.peer.name);
                    continue;
                }
            };
            self.h.heard = now;
            match reply.result {
                Ok(ReplyBody::Welcome(w)) if Some(reply.id) == self.hello => {
                    if !self.h.welcome(w.rig_id, self.peer.rig) {
                        tracing::warn!(
                            "link to {}: {} answers as rig {:x}, not {:x}",
                            self.peer.name,
                            self.peer.ctrl,
                            w.rig_id.0,
                            self.peer.rig.0
                        );
                    }
                }
                Ok(ReplyBody::SourceState(s)) => {
                    self.poll = None;
                    if s.rig != self.peer.rig {
                        continue;
                    }
                    self.their_period = s.session.as_ref().map(|x| x.buffer_frames);
                    if self.warned.take().is_some() {
                        tracing::info!("link to {}: answered again", self.peer.name);
                    }
                    if std::mem::take(&mut self.h.ctrl_hs.refused) {
                        // Admitted (as a rig) on the connection that was refused: the
                        // subscriptions it refused meanwhile are stated again.
                        self.join();
                    }
                    let _ = to_control.send(ControlMsg::Remote(RemoteMsg::Source {
                        rig: self.peer.rig,
                        state: s.clone(),
                    }));
                    if let Some(to) = &self.to {
                        let _ = to.try_send(LinkFrame::Source(s));
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    self.poll = None;
                    // A connection that outlived its admission, or a key admitted as an
                    // app rather than a rig, is refused request by request.
                    if e.code == ErrorCode::Refused {
                        self.h.ctrl_hs.refused = true;
                    }
                    if self.warned.as_deref() != Some(e.msg.as_str()) {
                        tracing::warn!("link to {}: {}", self.peer.name, e.msg);
                        self.warned = Some(e.msg);
                    }
                }
            }
        }
    }

    fn read_data(&mut self) {
        while let Ok(Some(m)) = self.data.try_recv() {
            let f = m.into_frames();
            let topic = f.first().map(Vec::as_slice).unwrap_or_default();
            if outside_window(topic) {
                self.owed.1 += 1;
                if topic == FLOW_TOPIC
                    && let Some(b) = f.get(1)
                {
                    match decode_notice(b) {
                        Ok(FlowNotice::Rejoin) => self.join(),
                        Ok(FlowNotice::NetworkOff) => {
                            tracing::info!("link to {}: it turned its network off", self.peer.name);
                            self.h.told_network_off();
                        }
                        _ => {}
                    }
                }
                continue;
            }
            let len: usize = f.iter().map(Vec::len).sum();
            self.owed.0 += frame_charge(len);
            self.bytes += len as u64;
            // Audio of a rig that is not the one paired never reaches a session.
            if !self.h.welcomed {
                continue;
            }
            let parts: Vec<&[u8]> = f.iter().map(Vec::as_slice).collect();
            if topic == b"source/levels" {
                self.levels(&parts);
                continue;
            }
            match decode_audio(&parts) {
                Ok(a) => {
                    if let Some(to) = &self.to {
                        match to.try_send(LinkFrame::Audio(a)) {
                            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
                            // The fan-out is behind: the gap shows as late audio there.
                            Err(TrySendError::Full(_)) => {}
                        }
                    }
                }
                Err(e) => tracing::debug!("link to {}: frame not read: {e}", self.peer.name),
            }
        }
        let (bytes, events) = std::mem::take(&mut self.owed);
        if bytes > 0 || events > 0 {
            self.up(DataUp::Credit { bytes, events });
        }
    }

    /// A `source/levels` frame: a keyframe whole, a delta on the one held; its mark goes to
    /// the standing feed.
    fn levels(&mut self, parts: &[&[u8]]) {
        let now = Instant::now();
        let coded = match decode_wire(parts) {
            Ok(w @ WireFrame::Key(_)) => w.apply(None),
            Ok(w @ WireFrame::Delta(_)) => w.apply(self.held.as_ref()),
            Err(e) => Err(e),
        };
        let coded = match coded {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!("link to {}: levels not read: {e}", self.peer.name);
                if self
                    .asked
                    .is_none_or(|t| now.saturating_duration_since(t) >= KEYFRAME_ASK_GAP)
                {
                    self.asked = Some(now);
                    self.up(DataUp::Keyframe {
                        topic: Topic::SourceLevels,
                    });
                }
                return;
            }
        };
        let frame = coded.to_frame();
        self.held = Some(coded);
        match frame {
            Ok(f) => {
                if let FrameData::SourceLevels(l) = f.data {
                    let local = self.local.get(now);
                    self.standing
                        .frame(&f.stamp, l, local, self.their_period, now);
                }
            }
            Err(e) => tracing::debug!("link to {}: levels not read: {e}", self.peer.name),
        }
    }
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;
