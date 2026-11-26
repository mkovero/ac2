//! ArtPoll in, ArtPollReply out: how a console finds the rig on the Art-Net network.
//!
//! A thread of its own waits on the Art-Net socket, so that the control loop never reads
//! the network. Every datagram arriving on port 6454 lands there (a lighting network
//! broadcasts thousands of ArtDmx a second), so the thread reads into one fixed buffer,
//! looks at the header only and drops everything that is not an ArtPoll: received DMX is
//! never acted on. Replies are rate-limited per source and in total, so that a flood of
//! ArtPoll (or spoofed ones) costs the daemon little and is not amplified onto the network.
//!
//! What that limit defends, and what not: a poll's source address is not proven, so only a
//! host that can put forged datagrams onto the lighting network (or a router that forwards
//! them there) can spoof one. Such a host can make the rig send at most
//! [`REPLIES_PER_SECOND`] replies a second, about 2 kB/s, to an address of its choosing,
//! never a broadcast or group: the limited broadcast, a multicast group and the directed
//! broadcast of each of the host's own interfaces are refused as sources (a directed broadcast
//! of a subnet the host is not on cannot be told from a host address and is not). It can
//! also spend the reply budget with polls from many addresses, or poll as a console's own
//! address, and so delay or stop a console's discovery of the rig. It cannot change what
//! the rig sends or where its DMX goes: nothing received is acted on, and the DMX goes to
//! the one target the operator set, not to pollers.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::model::PortAddress;
use socket2::SockRef;

/// The Art-Net packet ID: "Art-Net" and a NUL (Art-Net 4, every packet's field 1).
pub(super) const ID: [u8; 8] = *b"Art-Net\0";

/// OpPoll, transmitted low byte first (Art-Net 4, Table 1).
const OP_POLL: u16 = 0x2000;
/// OpPollReply (Art-Net 4, Table 1).
const OP_POLL_REPLY: u16 = 0x2100;

/// The oldest protocol revision answered: Art-Net 4 has controllers ignore devices on a
/// lower one (ArtPoll field 4, ProtVerLo).
const PROT_VER_MIN: u16 = 14;

/// The shortest ArtPoll a receiver accepts (Art-Net 4, ArtPoll, "Minimum Packet Length"):
/// fields after it are newer and read as zero when absent.
const POLL_MIN_LEN: usize = 14;

/// Bytes of an ArtPollReply (Art-Net 4, ArtPollReply packet definition, fields 1–54).
pub(super) const REPLY_LEN: usize = 239;

/// Style code StController: a lighting console, the kind of device that originates DMX
/// (Art-Net 4, Table 4).
const ST_CONTROLLER: u8 = 0x01;

/// Oem code of a product without a registered one ("OemUnknown" in the Art-Net SDK's
/// code list): ac2 has none.
const OEM_UNKNOWN: u16 = 0x00ff;

/// NodeReport code RcPowerOk: running normally (Art-Net 4, Table 3).
const RC_POWER_OK: u16 = 0x0001;

/// How long a source waits for its next reply: Art-Net controllers poll every 2.5–3 s, so
/// one reply a second answers every honest poller.
const PER_SOURCE: Duration = Duration::from_secs(1);

/// Most replies a second in total, whatever the sources: a 14-byte poll draws a 239-byte
/// reply, so this caps what spoofed polls could make the rig send at a few kB/s.
const REPLIES_PER_SECOND: usize = 8;

/// Of [`REPLIES_PER_SECOND`], how many may go to sources not in the table: the rest stay
/// for the consoles already answered, so polls from sources never seen before, however
/// many, cannot starve a console in the table that polls every 2.5–3 s. Polls spoofed from
/// addresses already in the table are not held back by this and can spend the whole total
/// (see the module's notes).
const NEW_PER_SECOND: usize = 4;

/// Sources remembered; past that the one answered longest ago is forgotten. A console
/// polling every 3 s stays in it while new sources come in at [`NEW_PER_SECOND`] (12 in
/// those 3 s). A fixed table: no allocation however many sources poll.
const SOURCES: usize = 32;

/// Bytes read per datagram: more than the largest Art-Net 4 packet (ArtFirmwareMaster,
/// 1066 bytes), so no Art-Net packet arriving here is cut, nor reported as too long.
const RECV_BUF: usize = 2048;
// ArtFirmwareMaster: 42 bytes from ID to Spare, then 512 16-bit words (Art-Net 4,
// ArtFirmwareMaster packet definition).
const _: () = assert!(RECV_BUF > 42 + 2 * 512);

/// Read errors in a row after which the thread pauses for [`WAKE`] before reading again:
/// an error that repeats at once must not spin a core, while a single one costs no wait.
const ERRORS_BEFORE_PAUSE: u32 = 16;

/// How long the thread waits for a datagram before it looks whether it should stop.
const WAKE: Duration = Duration::from_millis(250);

/// How long closing waits for the thread to let go of the socket: the thread is woken at
/// once, so this is reached only when the wake datagram is lost, and then the thread's own
/// [`WAKE`] ends its wait.
const STOP_WAIT: Duration = Duration::from_millis(500);

/// How old the list of the host's directed broadcast addresses may get before a poll
/// arriving has it read again: interfaces come and go rarely, and Art-Net's own retry
/// period is the rig's pace for noticing what changed on the host.
const BROADCASTS_FOR: Duration = super::artnet::RETRY;

/// An ArtPoll's request, as far as a reply depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Poll {
    /// Targeted mode: only devices with a Port-Address in `bottom..=top` reply.
    pub(super) targeted: Option<(u16, u16)>,
}

/// The ArtPoll in `p`; `None` for any other packet.
pub(super) fn parse_poll(p: &[u8]) -> Option<Poll> {
    if p.len() < POLL_MIN_LEN || p[..8] != ID || p[8..10] != OP_POLL.to_le_bytes() {
        return None;
    }
    if u16::from_be_bytes([p[10], p[11]]) < PROT_VER_MIN {
        return None;
    }
    let byte = |i: usize| p.get(i).copied().unwrap_or(0);
    // Flags bit 5: targeted mode; the range is fields 7–10, top first, each high byte
    // first.
    let targeted = (p[12] & 0x20 != 0).then(|| {
        let top = u16::from_be_bytes([byte(14), byte(15)]);
        let bottom = u16::from_be_bytes([byte(16), byte(17)]);
        (bottom, top)
    });
    Some(Poll { targeted })
}

/// What the reply says about the rig.
#[derive(Debug, Clone, Copy)]
pub(super) struct ReplyInfo {
    /// PortName (the "short name"), 17 ASCII characters and a NUL.
    pub(super) short_name: [u8; 18],
    /// LongName, 63 ASCII characters and a NUL.
    pub(super) long_name: [u8; 64],
    /// The universe the rig sends; `None` while it sends none.
    pub(super) universe: Option<PortAddress>,
}

impl ReplyInfo {
    pub(super) fn new(rig_name: &str) -> Self {
        Self {
            short_name: super::fixed_text(rig_name, true),
            long_name: super::fixed_text(rig_name, true),
            universe: None,
        }
    }
}

/// The ArtPollReply describing the rig at `ip`; `count` is the NodeReport's counter.
///
/// The rig is a controller with one input port (Art-Net 4 calls a port that puts DMX onto
/// the network an input): the universe it sends. It claims no output port: it takes no DMX
/// from the network.
pub(super) fn encode_reply(ip: Ipv4Addr, info: &ReplyInfo, count: u16) -> [u8; REPLY_LEN] {
    let mut p = [0u8; REPLY_LEN];
    p[..8].copy_from_slice(&ID);
    p[8..10].copy_from_slice(&OP_POLL_REPLY.to_le_bytes());
    p[10..14].copy_from_slice(&ip.octets());
    // Port: always 0x1936, and (unlike the other 16-bit fields) low byte first.
    p[14..16].copy_from_slice(&0x1936u16.to_le_bytes());
    // 16–17 VersInfo: no firmware to version.
    if let Some(u) = info.universe {
        p[18] = u.net(); // NetSwitch
        p[19] = u.sub(); // SubSwitch
    }
    p[20..22].copy_from_slice(&OEM_UNKNOWN.to_be_bytes());
    // 22 UbeaVersion, 23 Status1 (indicators and programming authority unknown), 24–25
    // EstaMan: all zero, no UBEA, no ESTA manufacturer code.
    p[26..44].copy_from_slice(&info.short_name);
    p[44..108].copy_from_slice(&info.long_name);
    // NodeReport, written in place (no allocation per reply); a report longer than 63
    // bytes is cut, and the field's last byte stays the NUL.
    {
        use std::io::Write as _;
        let mut w = &mut p[108..171];
        let _ = write!(w, "#{RC_POWER_OK:04x} [{:04}] ", count % 10_000);
        let _ = match info.universe {
            Some(u) => write!(w, "sending universe {u}"),
            None => write!(w, "sending no universe"),
        };
    }
    if let Some(u) = info.universe {
        p[173] = 1; // NumPortsLo
        p[174] = 0x40; // PortTypes[0]: input onto Art-Net, DMX512
        p[178] = 0x80; // GoodInput[0]: data being received (here: sent)
        p[186] = u.uni(); // SwIn[0]
    }
    // 194 AcnPriority … 199 Spare: zero.
    p[200] = ST_CONTROLLER;
    // 201–206 MAC, 207–210 BindIp, 211 BindIndex: zero (unknown; the root device).
    // Status2 bit 3: 15-bit Port-Addresses (Art-Net 3 or 4).
    p[212] = 0x08;
    // 213… GoodOutputB, Status3, DefaultRespUID, User, RefreshRate, BackgroundQueuePolicy,
    // Filler: zero.
    p
}

/// Replies sent within the last second, oldest at `next`.
#[derive(Debug)]
struct Window<const N: usize> {
    at: [Option<Instant>; N],
    next: usize,
}

impl<const N: usize> Window<N> {
    fn new() -> Self {
        Self {
            at: [None; N],
            next: 0,
        }
    }

    /// `N` replies within the second before `now`.
    fn full(&self, now: Instant) -> bool {
        self.at[self.next]
            .is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(1))
    }

    fn push(&mut self, now: Instant) {
        self.at[self.next] = Some(now);
        self.next = (self.next + 1) % N;
    }
}

/// The reply budget: one reply a second per source, [`REPLIES_PER_SECOND`] in total, of
/// which [`NEW_PER_SECOND`] to sources not in the table.
#[derive(Debug)]
pub(super) struct Limiter {
    /// Each source's latest reply.
    sources: [Option<(Ipv4Addr, Instant)>; SOURCES],
    /// Replies to anyone.
    all: Window<REPLIES_PER_SECOND>,
    /// Replies to sources not in the table.
    new: Window<NEW_PER_SECOND>,
}

impl Default for Limiter {
    fn default() -> Self {
        Self {
            sources: [None; SOURCES],
            all: Window::new(),
            new: Window::new(),
        }
    }
}

impl Limiter {
    /// Whether `from` may have a reply at `now`; a yes is counted.
    pub(super) fn allow(&mut self, from: Ipv4Addr, now: Instant) -> bool {
        let slot = self
            .sources
            .iter()
            .position(|s| s.is_some_and(|(ip, _)| ip == from));
        let last = slot.and_then(|i| self.sources[i]).map(|(_, t)| t);
        if last.is_some_and(|t| now.saturating_duration_since(t) < PER_SOURCE) {
            return false;
        }
        if self.all.full(now) || (slot.is_none() && self.new.full(now)) {
            return false;
        }
        self.all.push(now);
        if slot.is_none() {
            self.new.push(now);
        }
        let slot = slot.unwrap_or_else(|| {
            // A free slot, else the source answered longest ago.
            (0..SOURCES)
                .min_by_key(|&i| self.sources[i].map(|(_, t)| t))
                .unwrap_or(0)
        });
        self.sources[slot] = Some((from, now));
        true
    }
}

/// Whether a poll from `from` gets a reply at `now`, the rig sending `universe`: never to
/// an address no reply can go to (a spoofed source such as 0.0.0.0, a broadcast, one of the
/// directed `broadcasts` of the host's interfaces, or a group, which would also spend the
/// budget), only for the rig's universe in targeted mode, and within the [`Limiter`]'s
/// budget.
pub(super) fn answers(
    poll: Poll,
    from: Ipv4Addr,
    universe: Option<PortAddress>,
    broadcasts: &[Ipv4Addr],
    limiter: &mut Limiter,
    now: Instant,
) -> bool {
    if from.is_unspecified()
        || from.is_broadcast()
        || from.is_multicast()
        || broadcasts.contains(&from)
    {
        return false;
    }
    if let Some((bottom, top)) = poll.targeted
        && !universe.is_some_and(|u| (bottom..=top).contains(&u.get()))
    {
        return false;
    }
    limiter.allow(from, now)
}

/// The listener thread's handle. [`Listener::stop`] (and dropping it) stops the thread and
/// waits until the thread has dropped its handle of the socket (at most [`STOP_WAIT`]): a
/// handle left open holds the port, and the target set again at once would find it taken
/// by the rig itself.
#[derive(Debug)]
pub(super) struct Listener {
    stop: Arc<AtomicBool>,
    info: Arc<Mutex<ReplyInfo>>,
    /// A handle of the socket that wakes the thread with an empty datagram to itself.
    wake: UdpSocket,
    /// Where that datagram goes: the socket's own address, on loopback when it is bound to
    /// every interface.
    wake_to: SocketAddrV4,
    /// The thread, until it is stopped.
    thread: Option<Closing>,
}

/// A listener thread told to stop: whether it has let go of the socket yet.
#[derive(Debug)]
pub(super) struct Closing {
    /// Signalled when the thread has dropped its socket (disconnected if it panicked).
    done: Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl Closing {
    /// Whether the thread has let go of the socket; it is joined once it has.
    pub(super) fn ended(&mut self) -> bool {
        match self.done.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => {
                if let Some(t) = self.thread.take() {
                    let _ = t.join();
                }
                true
            }
            Err(TryRecvError::Empty) => false,
        }
    }

    /// One that ends when the returned sender sends or is dropped.
    #[cfg(test)]
    pub(super) fn pending() -> (mpsc::Sender<()>, Self) {
        let (tx, done) = mpsc::channel();
        (tx, Self { done, thread: None })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // Nothing is left to ask later whether it has ended: the warning in `stop` says it.
        let _ = self.stop();
    }
}

impl Listener {
    /// Starts answering ArtPoll arriving on `socket` (a handle of the Art-Net socket),
    /// replying to the poller's `reply_port`. The read timeout makes a blocking handle
    /// wait at most [`WAKE`]; on Windows the handle shares the socket's non-blocking mode,
    /// and the thread waits in `WSAPoll` instead.
    pub(super) fn spawn(socket: UdpSocket, reply_port: u16, info: ReplyInfo) -> io::Result<Self> {
        socket.set_read_timeout(Some(WAKE))?;
        let wake = socket.try_clone()?;
        let wake_to = match socket.local_addr()? {
            SocketAddr::V4(a) if a.ip().is_unspecified() => {
                SocketAddrV4::new(Ipv4Addr::LOCALHOST, a.port())
            }
            SocketAddr::V4(a) => a,
            SocketAddr::V6(a) => {
                return Err(io::Error::other(format!("{a}: Art-Net is IPv4 only")));
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let info = Arc::new(Mutex::new(info));
        let (s, i) = (stop.clone(), info.clone());
        let (done_tx, done) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("ac2d-artpoll".into())
            .spawn(move || {
                listen(&socket, reply_port, &s, &i);
                drop(socket);
                let _ = done_tx.send(());
            })?;
        Ok(Self {
            stop,
            info,
            wake,
            wake_to,
            thread: Some(Closing {
                done,
                thread: Some(thread),
            }),
        })
    }

    /// The universe the replies name from now on.
    pub(super) fn set_universe(&self, u: Option<PortAddress>) {
        self.info
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .universe = u;
    }

    /// Stops the thread and waits up to [`STOP_WAIT`] for it to let go of the socket; the
    /// thread when it has not by then, still holding the port.
    pub(super) fn stop(&mut self) -> Option<Closing> {
        let mut c = self.thread.take()?;
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = &c.thread {
            t.thread().unpark();
        }
        // Read as no ArtPoll; it only ends the thread's wait for a datagram. Not waited
        // for: a full send buffer leaves the thread's own WAKE to end the wait.
        let _ = super::send_nowait(&SockRef::from(&self.wake), &[], self.wake_to);
        match c.done.recv_timeout(STOP_WAIT) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                if let Some(t) = c.thread.take() {
                    let _ = t.join();
                }
                None
            }
            Err(RecvTimeoutError::Timeout) => {
                tracing::warn!(
                    "artnet: the ArtPoll listener did not stop within {STOP_WAIT:?}: {} stays \
                     held until it does",
                    self.wake_to
                );
                Some(c)
            }
        }
    }
}

/// What a failed read means to the listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadFailure {
    /// The wait for a datagram ended with none.
    Idle,
    /// The system reporting an ICMP error for an earlier send (Winsock returns a port
    /// unreachable as WSAECONNRESET on the next read): about a datagram already gone, once
    /// per report, so it says nothing about this socket and never pauses the reads. A
    /// frame target with nothing listening draws one per frame.
    SendReport,
    /// Anything else; one that repeats at once must not spin a core.
    Fault,
}

fn read_failure(e: &io::Error) -> ReadFailure {
    match e.kind() {
        // The read timeout. Not on Windows: there the wait said a datagram was ready, and
        // a socket in an error state that a read does not clear says so at once every time.
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut if !cfg!(windows) => ReadFailure::Idle,
        io::ErrorKind::ConnectionReset => ReadFailure::SendReport,
        _ => ReadFailure::Fault,
    }
}

/// The host's directed broadcast addresses, read again when a poll arrives and the list is
/// [`BROADCASTS_FOR`] old.
#[derive(Debug)]
struct Broadcasts {
    list: Vec<Ipv4Addr>,
    at: Instant,
    /// The latest error the system gave when asked, said once until another or a success.
    error: Option<String>,
}

impl Broadcasts {
    fn new(r: io::Result<Vec<Ipv4Addr>>, now: Instant) -> Self {
        let mut b = Self {
            list: Vec::new(),
            at: now,
            error: None,
        };
        b.take(r, now);
        b
    }

    /// Reads the list again through `read` when it is due.
    fn refresh(&mut self, now: Instant, read: impl FnOnce() -> io::Result<Vec<Ipv4Addr>>) {
        if now.saturating_duration_since(self.at) >= BROADCASTS_FOR {
            self.take(read(), now);
        }
    }

    /// Takes a new reading; a failed one keeps the list last read (an interface that was
    /// there is likely still there). Whether an error was logged.
    fn take(&mut self, r: io::Result<Vec<Ipv4Addr>>, now: Instant) -> bool {
        self.at = now;
        match r {
            Ok(list) => {
                self.list = list;
                self.error = None;
                false
            }
            Err(e) => {
                let e = e.to_string();
                let new = self.error.as_ref() != Some(&e);
                if new {
                    tracing::debug!(
                        "artnet: cannot list the host's interfaces ({e}): polls from their \
                         broadcast addresses are told apart by the list last read"
                    );
                }
                self.error = Some(e);
                new
            }
        }
    }
}

fn listen(socket: &UdpSocket, reply_port: u16, stop: &AtomicBool, info: &Mutex<ReplyInfo>) {
    let mut buf = [0u8; RECV_BUF];
    let mut limiter = Limiter::default();
    let mut count: u16 = 0;
    let mut errors: u32 = 0;
    let mut broadcasts = Broadcasts::new(super::local_broadcasts(), Instant::now());
    while !stop.load(Ordering::Relaxed) {
        #[cfg(windows)]
        match super::winsock::wait_readable(socket, WAKE) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => {
                // A wait that fails at once fails again: pausing keeps it off a core.
                std::thread::park_timeout(WAKE);
                continue;
            }
        }
        let (poll, from) = match socket.recv_from(&mut buf) {
            Ok((n, SocketAddr::V4(from))) => {
                errors = 0;
                match parse_poll(&buf[..n]) {
                    Some(poll) => (poll, from),
                    None => continue,
                }
            }
            Ok(_) => continue,
            Err(e) => {
                if read_failure(&e) == ReadFailure::Fault {
                    errors += 1;
                    if errors >= ERRORS_BEFORE_PAUSE {
                        errors = 0;
                        std::thread::park_timeout(WAKE);
                    }
                }
                continue;
            }
        };
        let now = Instant::now();
        broadcasts.refresh(now, super::local_broadcasts);
        let i = *info.lock().unwrap_or_else(PoisonError::into_inner);
        if !answers(
            poll,
            *from.ip(),
            i.universe,
            &broadcasts.list,
            &mut limiter,
            now,
        ) {
            continue;
        }
        let Some(ip) = local_ip_toward(*from.ip()) else {
            continue;
        };
        count = count.wrapping_add(1) % 10_000;
        let reply = encode_reply(ip, &i, count);
        // Art-Net 4: ArtPollReply is unicast to the poller, never broadcast. Not waited
        // for, like a frame: a full send buffer loses this reply, and the console polls
        // again within seconds.
        let to = SocketAddrV4::new(*from.ip(), reply_port);
        if let Err(e) = super::send_nowait(&SockRef::from(socket), &reply, to) {
            tracing::debug!("artnet: cannot answer ArtPoll from {from}: {e}");
        }
    }
}

/// The rig's own address on the way to `peer`: the reply's IP field, which a console shows
/// and sends to. The socket listens on every interface, so the routing table says which.
fn local_ip_toward(peer: Ipv4Addr) -> Option<Ipv4Addr> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // A UDP connect sends nothing: it only picks the route and the source address.
    s.connect((peer, 9)).ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(ip) => Some(ip),
        IpAddr::V6(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_artpoll_is_read() {
        // Art-Net 4, ArtPoll packet definition, fields 1–6: the 14-byte minimum.
        let poll = [
            b'A', b'r', b't', b'-', b'N', b'e', b't', 0, // ID
            0x00, 0x20, // OpCode OpPoll, low byte first
            0x00, 0x0e, // ProtVer 14
            0x00, // Flags
            0x00, // DiagPriority
        ];
        assert_eq!(parse_poll(&poll), Some(Poll { targeted: None }));
        assert_eq!(parse_poll(&poll[..13]), None);
        // An older protocol revision than 14 is not answered.
        let mut old = poll;
        old[11] = 13;
        assert_eq!(parse_poll(&old), None);
        let mut newer = poll;
        newer[11] = 15;
        assert_eq!(parse_poll(&newer), Some(Poll { targeted: None }));
        let mut dmx = poll;
        dmx[8..10].copy_from_slice(&[0x00, 0x50]); // OpDmx
        assert_eq!(parse_poll(&dmx), None);
        let mut reply = poll;
        reply[9] = 0x21; // OpPollReply
        assert_eq!(parse_poll(&reply), None);
        let mut other = poll;
        other[..8].copy_from_slice(b"Art-Ne?\0");
        assert_eq!(parse_poll(&other), None);
    }

    #[test]
    fn a_targeted_poll_carries_its_range() {
        // Fields 5 and 7–10: Flags bit 5, then the top and the bottom Port-Address.
        let mut p = [0u8; 22];
        p[..8].copy_from_slice(&ID);
        p[8..10].copy_from_slice(&[0x00, 0x20]);
        p[10..12].copy_from_slice(&[0, 14]);
        p[12] = 0x20;
        p[14..18].copy_from_slice(&[0x01, 0x23, 0x00, 0x10]);
        assert_eq!(
            parse_poll(&p),
            Some(Poll {
                targeted: Some((0x0010, 0x0123))
            })
        );
        // Cut short after the flags: the missing range reads as zero.
        assert_eq!(
            parse_poll(&p[..14]),
            Some(Poll {
                targeted: Some((0, 0))
            })
        );
    }

    #[test]
    fn the_reply_describes_a_controller_byte_for_byte() {
        let mut info = ReplyInfo::new("ac2 on kärppä, the long rig name");
        info.universe = Some(PortAddress::from_parts(0x12, 0x3, 0x4).expect("test value"));
        let p = encode_reply(Ipv4Addr::new(10, 0, 0, 5), &info, 7);
        // Art-Net 4, ArtPollReply packet definition: fields 1–14 by offset.
        assert_eq!(
            p[..26],
            [
                b'A', b'r', b't', b'-', b'N', b'e', b't', 0, // 0 ID
                0x00, 0x21, // 8 OpCode OpPollReply, low byte first
                10, 0, 0, 5, // 10 IP Address
                0x36, 0x19, // 14 Port 0x1936, low byte first
                0x00, 0x00, // 16 VersInfo
                0x12, // 18 NetSwitch: Port-Address bits 14–8
                0x03, // 19 SubSwitch: bits 7–4
                0x00, 0xff, // 20 Oem: unknown
                0x00, // 22 UbeaVersion
                0x00, // 23 Status1
                0x00, 0x00, // 24 EstaMan
            ]
        );
        // 26 PortName[18], 44 LongName[64]: ASCII, NUL-terminated.
        assert_eq!(&p[26..44], b"ac2 on k?rpp?, th\0");
        assert_eq!(&p[44..77], b"ac2 on k?rpp?, the long rig name\0");
        assert!(p[77..108].iter().all(|b| *b == 0));
        // 108 NodeReport[64]: "#xxxx [yyyy] text".
        let report = b"#0001 [0007] sending universe 18.3.4\0";
        assert_eq!(&p[108..108 + report.len()], report);
        assert_eq!(
            p[172..201],
            [
                0x00, 0x01, // 172 NumPorts: one
                0x40, 0, 0, 0, // 174 PortTypes: input onto Art-Net, DMX512
                0x80, 0, 0, 0, // 178 GoodInput: data
                0, 0, 0, 0, // 182 GoodOutputA
                0x04, 0, 0, 0, // 186 SwIn: Port-Address bits 3–0
                0, 0, 0, 0, // 190 SwOut: no output ports
                0, 0, 0, // 194 AcnPriority, SwMacro, SwRemote
                0, 0, 0,    // 197 Spare
                0x01, // 200 Style: StController
            ]
        );
        assert!(
            p[201..212].iter().all(|b| *b == 0),
            "MAC, BindIp, BindIndex"
        );
        assert_eq!(p[212], 0x08, "Status2: 15-bit Port-Address");
        assert!(p[213..].iter().all(|b| *b == 0));
        assert_eq!(p.len(), 239);
    }

    #[test]
    fn a_rig_sending_nothing_claims_no_port() {
        let p = encode_reply(Ipv4Addr::LOCALHOST, &ReplyInfo::new("rig"), 0);
        assert_eq!(
            (p[18], p[19], p[173], p[174], p[178], p[186]),
            (0, 0, 0, 0, 0, 0)
        );
        assert_eq!(p[200], ST_CONTROLLER);
    }

    #[test]
    fn replies_are_one_a_second_per_source_and_few_in_total() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let a = Ipv4Addr::new(10, 0, 0, 1);
        let mut l = Limiter::default();
        assert!(l.allow(a, t0));
        assert!(!l.allow(a, ms(10)));
        assert!(!l.allow(a, ms(999)));
        assert!(l.allow(a, ms(1000)));
        // Many new sources: they stop at NEW_PER_SECOND within any second.
        let mut l = Limiter::default();
        let sent = (0..100u8)
            .filter(|i| l.allow(Ipv4Addr::new(10, 0, 1, *i), ms(u64::from(*i))))
            .count();
        assert_eq!(sent, NEW_PER_SECOND);
        assert!(l.allow(Ipv4Addr::new(10, 0, 2, 1), ms(1000)));
        // More sources than the table holds: the oldest is forgotten, never a panic.
        let mut l = Limiter::default();
        for i in 0..40u8 {
            l.allow(Ipv4Addr::new(10, 0, 3, i), ms(u64::from(i) * 200));
        }
        assert!(l.sources.iter().all(Option::is_some));
    }

    #[test]
    fn a_flood_of_new_sources_leaves_room_for_a_known_console() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let console = Ipv4Addr::new(10, 0, 0, 1);
        let mut l = Limiter::default();
        assert!(l.allow(console, t0));
        // From a second on, a poll every millisecond from a new source each time, for
        // three seconds; the console polls again 2.5 s in and is answered.
        let mut new = 0;
        for i in 1000..4000u64 {
            if i == 2500 {
                assert!(l.allow(console, ms(i)), "the console starved");
            }
            let spoofed = Ipv4Addr::from(0x0a01_0000 + u32::try_from(i).expect("small"));
            new += usize::from(l.allow(spoofed, ms(i)));
        }
        assert_eq!(new, 3 * NEW_PER_SECOND);
        // Known sources (in the table) take the rest of the total.
        let mut l = Limiter::default();
        let known: Vec<Ipv4Addr> = (1..=REPLIES_PER_SECOND as u8)
            .map(|i| Ipv4Addr::new(10, 0, 4, i))
            .collect();
        for (i, ip) in known.iter().enumerate() {
            // Learned a second apart, within the new budget.
            assert!(l.allow(*ip, t0 + Duration::from_secs(i as u64)));
        }
        let t = t0 + Duration::from_secs(20);
        assert!(known.iter().all(|ip| l.allow(*ip, t)));
        assert!(
            !l.allow(Ipv4Addr::new(10, 0, 5, 1), t),
            "the total is spent"
        );
    }

    #[test]
    fn a_poll_from_nowhere_is_not_answered_nor_counted() {
        let poll = Poll { targeted: None };
        let t0 = Instant::now();
        let mut l = Limiter::default();
        for from in [
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::BROADCAST,
            Ipv4Addr::new(239, 255, 0, 1),
            Ipv4Addr::new(224, 0, 0, 1),
        ] {
            for _ in 0..20 {
                assert!(!answers(poll, from, None, &[], &mut l, t0), "{from}");
            }
        }
        // The directed broadcast of one of the host's interfaces, as the source.
        let directed = [Ipv4Addr::new(192, 168, 1, 255)];
        for _ in 0..20 {
            assert!(!answers(poll, directed[0], None, &directed, &mut l, t0));
        }
        assert!(l.sources.iter().all(Option::is_none));
        assert!(answers(
            poll,
            Ipv4Addr::new(10, 0, 0, 1),
            None,
            &directed,
            &mut l,
            t0
        ));
        // Targeted: only for the rig's universe.
        let u = PortAddress::new(0x20).expect("test value");
        let narrow = Poll {
            targeted: Some((0x10, 0x1f)),
        };
        let wide = Poll {
            targeted: Some((0x10, 0x30)),
        };
        assert!(!answers(
            narrow,
            Ipv4Addr::new(10, 0, 0, 2),
            Some(u),
            &[],
            &mut l,
            t0
        ));
        assert!(!answers(
            wide,
            Ipv4Addr::new(10, 0, 0, 2),
            None,
            &[],
            &mut l,
            t0
        ));
        assert!(answers(
            wide,
            Ipv4Addr::new(10, 0, 0, 2),
            Some(u),
            &[],
            &mut l,
            t0
        ));
    }

    #[test]
    fn the_hosts_directed_broadcasts_are_listed() {
        // Loopback is a /8 on every system that lists it: its directed broadcast is
        // 127.255.255.255, never one of the host's own addresses.
        let b = super::super::local_broadcasts().expect("interfaces");
        assert!(!b.contains(&Ipv4Addr::LOCALHOST), "{b:?}");
        let all = if_addrs::get_if_addrs().expect("interfaces");
        for i in all {
            if let if_addrs::IfAddr::V4(a) = i.addr
                && a.prefixlen <= 30
            {
                let want = Ipv4Addr::from(u32::from(a.ip) | (u32::MAX >> a.prefixlen));
                assert!(b.contains(&want), "{}: {want} not in {b:?}", i.name);
            }
        }
    }

    #[test]
    fn a_poll_after_a_flood_of_long_datagrams_is_answered_at_once() {
        let rig = UdpSocket::bind("127.0.0.1:0").expect("test value");
        // Room for the whole flood, so the poll after it is not dropped on a small default
        // buffer.
        socket2::SockRef::from(&rig)
            .set_recv_buffer_size(1 << 21)
            .expect("test value");
        let poller = UdpSocket::bind("127.0.0.1:0").expect("test value");
        poller
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("test value");
        let to = rig.local_addr().expect("test value");
        let reply_port = poller.local_addr().expect("test value").port();
        let _l = Listener::spawn(rig, reply_port, ReplyInfo::new("rig")).expect("test value");
        let mut dmx = [0x55u8; 530];
        dmx[..8].copy_from_slice(&ID);
        dmx[8..10].copy_from_slice(&[0x00, 0x50]);
        let firmware = [0x33u8; 1066];
        for _ in 0..200 {
            poller.send_to(&dmx, to).expect("test value");
            poller.send_to(&firmware, to).expect("test value");
        }
        let t0 = Instant::now();
        let mut poll = [0u8; 14];
        poll[..8].copy_from_slice(&ID);
        poll[8..10].copy_from_slice(&[0x00, 0x20]);
        poll[10..12].copy_from_slice(&[0, 14]);
        poller.send_to(&poll, to).expect("test value");
        let mut buf = [0u8; 300];
        let n = poller.recv(&mut buf).expect("a reply");
        assert_eq!(n, REPLY_LEN);
        // A read that paused even 10 ms per foreign datagram would take 4 s for these 400;
        // the bound leaves a loaded runner its stalls.
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    }

    #[test]
    fn a_report_of_an_earlier_send_never_pauses_the_reads() {
        // Winsock's word for an ICMP port unreachable that an earlier send drew: one per
        // report, however many come.
        let reset = io::Error::from(io::ErrorKind::ConnectionReset);
        assert_eq!(read_failure(&reset), ReadFailure::SendReport);
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::InvalidInput,
            io::ErrorKind::Other,
        ] {
            assert_eq!(read_failure(&io::Error::from(kind)), ReadFailure::Fault);
        }
        let timeout = read_failure(&io::Error::from(io::ErrorKind::WouldBlock));
        if cfg!(windows) {
            assert_eq!(timeout, ReadFailure::Fault);
        } else {
            assert_eq!(timeout, ReadFailure::Idle);
        }
    }

    #[test]
    fn a_failed_interface_list_is_said_once_per_error_and_keeps_the_last() {
        let t0 = Instant::now();
        let lan = vec![Ipv4Addr::new(192, 168, 1, 255)];
        let mut b = Broadcasts::new(Ok(lan.clone()), t0);
        let fail = |what: &str| Err(io::Error::other(what.to_owned()));
        assert!(b.take(fail("no netlink"), t0));
        assert!(!b.take(fail("no netlink"), t0), "the same error again");
        assert_eq!(b.list, lan, "the list last read stays");
        assert!(b.take(fail("out of memory"), t0), "another error");
        assert!(!b.take(Ok(lan.clone()), t0));
        assert!(
            b.take(fail("no netlink"), t0),
            "failing again after a success"
        );
        // Read again only when the list is due.
        let mut read = false;
        b.refresh(t0 + BROADCASTS_FOR / 2, || {
            read = true;
            Ok(Vec::new())
        });
        assert!(!read);
        b.refresh(t0 + BROADCASTS_FOR, || Ok(Vec::new()));
        assert!(b.list.is_empty() && b.error.is_none());
    }
}
