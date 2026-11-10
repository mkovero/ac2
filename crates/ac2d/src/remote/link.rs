//! The link to one paired rig (`docs/design/remote-inputs.md` §2.2, §2.3): a thread of its
//! own with two sync DEALERs on the other rig's ctrl and data sockets, under CURVE with this
//! rig's own server key pair as the client key, pinned to the key the app gave.
//!
//! It reads the other rig's filtered state (`source.state`) once a second and subscribes the
//! audio of the channels the open session takes from it, handing each frame to the fan-out
//! through a bounded channel. libzmq reconnects on its own (backing off up to
//! [`RECONNECT_MAX`]); a completed handshake brings `hello` again and the subscriptions
//! again. The link never runs on the fan-out's thread and needs no async runtime.

use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::flow::{DATA_WINDOW_MAX, FLOW_TOPIC, frame_charge, outside_window};
use ac2_proto::frame::{AudioFrame, decode_audio};
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
) -> std::io::Result<LinkHandle> {
    let server_key = PublicKey::from_z85(&peer.key)
        .map_err(|_| std::io::Error::other(format!("{}: not a server key", peer.name)))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let p = peer.clone();
    let thread = std::thread::Builder::new()
        .name(format!("ac2d-link-{:x}", peer.rig.0))
        .spawn(move || {
            let curve = CurveClient { keys, server_key };
            match Link::open(&ctx, &p, &curve) {
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
) -> ac2_zmq::Result<(Socket, Monitor)> {
    let s = ctx.socket(SocketType::Dealer)?;
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

struct Link<'p> {
    peer: &'p RigPeer,
    ctrl: Socket,
    ctrl_mon: Monitor,
    data: Socket,
    data_mon: Monitor,
    ctrl_hs: Handshakes,
    data_hs: Handshakes,
    /// The other rig answered `hello` as the rig paired.
    welcomed: bool,
    /// It answered as another rig.
    other_rig: Option<ac2_proto::units::RigId>,
    next_id: u64,
    hello: Option<RequestId>,
    poll: Option<(RequestId, Instant)>,
    next_poll: Instant,
    /// When the last reply came, or the link began.
    heard: Instant,
    began: Instant,
    unreachable_since: Option<WallNs>,
    channels: Vec<u16>,
    to: Option<SyncSender<LinkFrame>>,
    /// Owed credit: frame bytes and outside-window messages.
    owed: (u32, u32),
    bytes: u64,
    next_report: Instant,
    last: Option<RemoteLink>,
}

impl<'p> Link<'p> {
    fn open(ctx: &Context, peer: &'p RigPeer, curve: &CurveClient) -> ac2_zmq::Result<Self> {
        let (ctrl, ctrl_mon) = dealer(ctx, curve, &peer.ctrl)?;
        let (data, data_mon) = dealer(ctx, curve, &peer.data)?;
        let now = Instant::now();
        Ok(Self {
            peer,
            ctrl,
            ctrl_mon,
            data,
            data_mon,
            ctrl_hs: Handshakes::default(),
            data_hs: Handshakes::default(),
            welcomed: false,
            other_rig: None,
            next_id: 1,
            hello: None,
            poll: None,
            next_poll: now,
            heard: now,
            began: now,
            unreachable_since: None,
            channels: Vec::new(),
            to: None,
            owed: (0, 0),
            bytes: 0,
            next_report: now,
            last: None,
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
                    if self.ctrl_hs.event(e.event) {
                        // A new connection: the other rig may be another daemon now.
                        self.welcomed = false;
                        self.other_rig = None;
                        self.poll = None;
                        self.hello = self.ask(Command::Hello {
                            client: format!("ac2d {}", env!("CARGO_PKG_VERSION")),
                        });
                    }
                }
            }
            if ready[3] {
                while let Ok(Some(e)) = self.data_mon.try_recv() {
                    if self.data_hs.event(e.event) {
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
            if self.welcomed && self.poll.is_none() && now >= self.next_poll {
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
                let link = self.status(now);
                if self.last.as_ref() != Some(&link) {
                    tracing::info!("link to {}: {link:?}", self.peer.name);
                    self.last = Some(link.clone());
                }
                report(to_control, self.peer, link, rate, DATA_WINDOW_MAX);
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
            self.heard = now;
            match reply.result {
                Ok(ReplyBody::Welcome(w)) if Some(reply.id) == self.hello => {
                    if w.rig_id == self.peer.rig {
                        self.welcomed = true;
                        self.other_rig = None;
                    } else {
                        tracing::warn!(
                            "link to {}: {} answers as rig {:x}, not {:x}",
                            self.peer.name,
                            self.peer.ctrl,
                            w.rig_id.0,
                            self.peer.rig.0
                        );
                        self.other_rig = Some(w.rig_id);
                    }
                }
                Ok(ReplyBody::SourceState(s)) => {
                    self.poll = None;
                    if s.rig != self.peer.rig {
                        continue;
                    }
                    if std::mem::take(&mut self.ctrl_hs.refused) {
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
                        self.ctrl_hs.refused = true;
                    }
                    tracing::warn!("link to {}: {}", self.peer.name, e.msg);
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
                    && decode_notice(b) == Ok(FlowNotice::Rejoin)
                {
                    self.join();
                }
                continue;
            }
            let len: usize = f.iter().map(Vec::len).sum();
            self.owed.0 += frame_charge(len);
            self.bytes += len as u64;
            // Audio of a rig that is not the one paired never reaches a session.
            if !self.welcomed {
                continue;
            }
            let parts: Vec<&[u8]> = f.iter().map(Vec::as_slice).collect();
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
}
