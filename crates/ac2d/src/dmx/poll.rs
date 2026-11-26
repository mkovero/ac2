//! ArtPoll in, ArtPollReply out: how a console finds the rig on the Art-Net network.
//!
//! A thread of its own waits on the Art-Net socket, so that the control loop never reads
//! the network. Every datagram arriving on port 6454 lands there (a lighting network
//! broadcasts thousands of ArtDmx a second), so the thread reads into one fixed buffer,
//! looks at the header only and drops everything that is not an ArtPoll: received DMX is
//! never acted on. Replies are rate-limited per source and in total, so that a flood of
//! ArtPoll (or spoofed ones) costs the daemon little and is not amplified onto the network.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ac2_proto::model::PortAddress;

/// The Art-Net packet ID: "Art-Net" and a NUL (Art-Net 4, every packet's field 1).
pub(super) const ID: [u8; 8] = *b"Art-Net\0";

/// OpPoll, transmitted low byte first (Art-Net 4, Table 1).
const OP_POLL: u16 = 0x2000;
/// OpPollReply (Art-Net 4, Table 1).
const OP_POLL_REPLY: u16 = 0x2100;

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

/// Sources remembered for [`PER_SOURCE`]; past that the one answered longest ago is
/// forgotten. A fixed table: no allocation however many sources poll.
const SOURCES: usize = 16;

/// Most replies a second in total, whatever the sources: a 14-byte poll draws a 239-byte
/// reply, so this caps what spoofed polls could make the rig send at a few kB/s.
const REPLIES_PER_SECOND: usize = 8;

/// How long the thread waits for a datagram before it looks whether it should stop.
const WAKE: Duration = Duration::from_millis(250);

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

/// The reply budget: one reply a second per source, [`REPLIES_PER_SECOND`] in total.
#[derive(Debug)]
pub(super) struct Limiter {
    /// Each source's latest reply.
    sources: [Option<(Ipv4Addr, Instant)>; SOURCES],
    /// The latest replies to anyone, oldest at `next`.
    recent: [Option<Instant>; REPLIES_PER_SECOND],
    next: usize,
}

impl Default for Limiter {
    fn default() -> Self {
        Self {
            sources: [None; SOURCES],
            recent: [None; REPLIES_PER_SECOND],
            next: 0,
        }
    }
}

impl Limiter {
    /// Whether `from` may have a reply at `now`; a yes is counted.
    pub(super) fn allow(&mut self, from: Ipv4Addr, now: Instant) -> bool {
        let within = |t: Instant, d: Duration| now.saturating_duration_since(t) < d;
        let slot = self
            .sources
            .iter()
            .position(|s| s.is_some_and(|(ip, _)| ip == from));
        if slot.is_some_and(|i| self.sources[i].is_some_and(|(_, t)| within(t, PER_SOURCE))) {
            return false;
        }
        if self.recent[self.next].is_some_and(|t| within(t, Duration::from_secs(1))) {
            return false;
        }
        self.recent[self.next] = Some(now);
        self.next = (self.next + 1) % REPLIES_PER_SECOND;
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

/// The listener thread's handle: dropping it stops the thread within [`WAKE`].
#[derive(Debug)]
pub(super) struct Listener {
    stop: Arc<AtomicBool>,
    info: Arc<Mutex<ReplyInfo>>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Listener {
    /// Starts answering ArtPoll arriving on `socket` (a handle of the Art-Net socket, in
    /// blocking mode), replying to the poller's `reply_port`.
    pub(super) fn spawn(socket: UdpSocket, reply_port: u16, info: ReplyInfo) -> io::Result<Self> {
        socket.set_read_timeout(Some(WAKE))?;
        let stop = Arc::new(AtomicBool::new(false));
        let info = Arc::new(Mutex::new(info));
        let (s, i) = (stop.clone(), info.clone());
        std::thread::Builder::new()
            .name("ac2d-artpoll".into())
            .spawn(move || listen(&socket, reply_port, &s, &i))?;
        Ok(Self { stop, info })
    }

    /// The universe the replies name from now on.
    pub(super) fn set_universe(&self, u: Option<PortAddress>) {
        self.info
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .universe = u;
    }
}

fn listen(socket: &UdpSocket, reply_port: u16, stop: &AtomicBool, info: &Mutex<ReplyInfo>) {
    // An ArtPoll is at most 22 bytes; a longer datagram is cut to the buffer (or reported
    // as too long, on Windows) and is no ArtPoll anyway.
    let mut buf = [0u8; 64];
    let mut limiter = Limiter::default();
    let mut count: u16 = 0;
    while !stop.load(Ordering::Relaxed) {
        let from = match socket.recv_from(&mut buf) {
            Ok((n, SocketAddr::V4(from))) => match parse_poll(&buf[..n]) {
                Some(poll) => (poll, from),
                None => continue,
            },
            Ok(_) => continue,
            Err(e) => {
                if !matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) {
                    // A datagram too long, or an ICMP error of an earlier send reported
                    // here (Windows): nothing to do but read on; the pause keeps an error
                    // that repeats at once from spinning the thread.
                    std::thread::sleep(Duration::from_millis(10));
                }
                continue;
            }
        };
        let (poll, from) = from;
        let i = *info.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((bottom, top)) = poll.targeted {
            // Targeted mode: reply only for a Port-Address in the range.
            if !i
                .universe
                .is_some_and(|u| (bottom..=top).contains(&u.get()))
            {
                continue;
            }
        }
        if !limiter.allow(*from.ip(), Instant::now()) {
            continue;
        }
        let Some(ip) = local_ip_toward(*from.ip()) else {
            continue;
        };
        count = count.wrapping_add(1) % 10_000;
        let reply = encode_reply(ip, &i, count);
        // Art-Net 4: ArtPollReply is unicast to the poller, never broadcast.
        let to = SocketAddrV4::new(*from.ip(), reply_port);
        if let Err(e) = socket.send_to(&reply, to) {
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
        // Many sources: the total stops at REPLIES_PER_SECOND within any second.
        let mut l = Limiter::default();
        let sent = (0..100u8)
            .filter(|i| l.allow(Ipv4Addr::new(10, 0, 1, *i), ms(u64::from(*i))))
            .count();
        assert_eq!(sent, REPLIES_PER_SECOND);
        assert!(l.allow(Ipv4Addr::new(10, 0, 2, 1), ms(1000)));
        // More sources than the table holds: the oldest is forgotten, never a panic.
        let mut l = Limiter::default();
        for i in 0..40u8 {
            l.allow(Ipv4Addr::new(10, 0, 3, i), ms(u64::from(i) * 200));
        }
        assert!(l.sources.iter().all(Option::is_some));
    }
}
