//! Sends the rig's DMX universe as sACN (ANSI E1.31-2018) data packets over UDP: to one
//! receiver, or to the universe's multicast group out of the interface the target names.
//!
//! Multicast goes out with TTL 1 (the lighting network is the subnet the interface is on;
//! a router does not carry it further, and a routed receiver takes unicast) and loops back
//! to the host (a visualiser on the rig's own machine hears it). Sending to a group needs
//! no membership, so the daemon joins none and sends no IGMP.
//!
//! The source's CID is derived from the rig's id, so a receiver sees the same source
//! across restarts and does not hold a stale one beside it. The sequence number counts up
//! per packet of the universe. When a target is turned off, changed or the daemon stops in
//! order, the stream ends with three packets marked Stream_Terminated, so receivers let
//! go of the source at once instead of after the network data loss timeout (2.5 s).

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};

use ac2_proto::model::{DMX_CHANNELS, OutputStatus, SacnDest, SacnTarget};
use ac2_proto::units::RigId;
use socket2::{Domain, Protocol, Socket, Type};

use super::DmxPorts;

/// Bytes of a data packet carrying a whole universe (E1.31 Table 4-1): 126 bytes of root,
/// framing and DMP layers, then the 512 slots.
pub(crate) const PACKET_LEN: usize = 126 + DMX_CHANNELS;

/// The root layer's ACN Packet Identifier (E1.31 §5.3).
const ACN_PID: [u8; 12] = *b"ASC-E1.17\0\0\0";
/// VECTOR_ROOT_E131_DATA (E1.31 Appendix A).
const VECTOR_ROOT_DATA: u32 = 0x0000_0004;
/// VECTOR_E131_DATA_PACKET (E1.31 Appendix A).
const VECTOR_FRAMING_DATA: u32 = 0x0000_0002;
/// VECTOR_DMP_SET_PROPERTY (E1.31 Appendix A).
const VECTOR_DMP_SET: u8 = 0x02;
/// Options bit 6: Stream_Terminated (E1.31 §6.2.6).
const STREAM_TERMINATED: u8 = 0x40;
/// How many terminating packets end a stream (E1.31 §6.2.6: three).
const TERMINATE_COUNT: usize = 3;
/// Multicast hops: the receivers are on the interface's own subnet.
const MULTICAST_TTL: u32 = 1;

/// A PDU's Flags and Length field: the flags 0x7 in the top nibble and the PDU's length,
/// counted from this field to the end of the packet (E1.31 §5.4, §6.2.1, §7.2).
fn flags_length(from: usize) -> [u8; 2] {
    // At most 638 bytes: fits the 12 bits by construction.
    (0x7000 | (PACKET_LEN - from) as u16).to_be_bytes()
}

/// The source's CID: an RFC 9562 version 8 UUID holding the rig id, the same for the rig
/// at every start. The id fills bytes 0–5 and 10–11, clear of the version and variant
/// bits; the rest is a fixed tag that keeps these CIDs apart from other UUIDs.
pub(crate) fn cid(rig: RigId) -> [u8; 16] {
    let id = rig.0.to_be_bytes();
    let mut c = [0u8; 16];
    c[..6].copy_from_slice(&id[..6]);
    c[6] = 0x80; // version 8 in the high nibble
    c[7] = b'a';
    c[8] = 0x80; // variant 0b10 in the top bits
    c[9] = b'c';
    c[10..12].copy_from_slice(&id[6..]);
    c[12..16].copy_from_slice(b"2dmx");
    c
}

/// The data packet for `frame` (E1.31 Table 4-1).
pub(crate) fn encode(
    cid: &[u8; 16],
    source_name: &[u8; 64],
    target: &SacnTarget,
    sequence: u8,
    terminated: bool,
    frame: &[u8; DMX_CHANNELS],
) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    // Root layer.
    p[0..2].copy_from_slice(&0x0010u16.to_be_bytes()); // Preamble Size
    // 2–3 Post-amble Size: 0.
    p[4..16].copy_from_slice(&ACN_PID);
    p[16..18].copy_from_slice(&flags_length(16));
    p[18..22].copy_from_slice(&VECTOR_ROOT_DATA.to_be_bytes());
    p[22..38].copy_from_slice(cid);
    // Framing layer.
    p[38..40].copy_from_slice(&flags_length(38));
    p[40..44].copy_from_slice(&VECTOR_FRAMING_DATA.to_be_bytes());
    p[44..108].copy_from_slice(source_name);
    p[108] = target.priority.get();
    // 109–110 Synchronization Address: 0, no synchronisation.
    p[111] = sequence;
    p[112] = if terminated { STREAM_TERMINATED } else { 0 };
    p[113..115].copy_from_slice(&target.universe.get().to_be_bytes());
    // DMP layer.
    p[115..117].copy_from_slice(&flags_length(115));
    p[117] = VECTOR_DMP_SET;
    p[118] = 0xa1; // Address Type & Data Type
    // 119–120 First Property Address: 0.
    p[121..123].copy_from_slice(&1u16.to_be_bytes()); // Address Increment
    p[123..125].copy_from_slice(&(1 + DMX_CHANNELS as u16).to_be_bytes()); // Property value count
    // 125 START Code: 0, dimmer data.
    p[126..].copy_from_slice(frame);
    p
}

/// Why `t` cannot be an sACN target, before the operating system is asked.
pub(crate) fn check_target(t: &SacnTarget) -> Result<(), String> {
    let bad = |ip: Ipv4Addr| ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast();
    match t.dest {
        SacnDest::Unicast { addr } if bad(addr) => Err(format!(
            "{addr} is not a receiver's address: give its IPv4 unicast address, or send \
             multicast"
        )),
        SacnDest::Multicast { interface } if bad(interface) => Err(format!(
            "{interface} is not an interface's address: give this host's IPv4 address on \
             the lighting network"
        )),
        _ => Ok(()),
    }
}

/// The socket, the stream's sequence and how sending fares.
#[derive(Debug)]
pub(crate) struct SacnSender {
    ports: DmxPorts,
    cid: [u8; 16],
    source_name: [u8; 64],
    /// The socket and the destination it was set up for.
    socket: Option<(Socket, SacnDest)>,
    /// The next packet's sequence number.
    sequence: u8,
    status: OutputStatus,
}

impl SacnSender {
    pub(crate) fn new(ports: DmxPorts, rig: RigId, rig_name: &str) -> Self {
        Self {
            ports,
            cid: cid(rig),
            // E1.31 §6.2.2: UTF-8, NUL-terminated, 64 bytes.
            source_name: super::fixed_text(rig_name, false),
            socket: None,
            sequence: 0,
            status: OutputStatus::default(),
        }
    }

    /// Sets the socket up for `dest` unless it is; the operating system's refusal (a
    /// multicast interface the host does not have) as an error.
    pub(crate) fn open(&mut self, dest: SacnDest) -> Result<(), String> {
        if self.socket.as_ref().is_some_and(|(_, d)| *d == dest) {
            return Ok(());
        }
        self.socket = None;
        let s = socket(self.ports.sacn_bind, dest).map_err(|e| open_error(dest, &e))?;
        self.socket = Some((s, dest));
        Ok(())
    }

    /// Whether a socket can be set up for `dest`, without touching the one in use.
    pub(crate) fn check_dest(&self, dest: SacnDest) -> Result<(), String> {
        socket(self.ports.sacn_bind, dest)
            .map(drop)
            .map_err(|e| open_error(dest, &e))
    }

    /// Sends `frame` to `target`; the error is also kept in the status.
    pub(crate) fn send(
        &mut self,
        target: &SacnTarget,
        frame: &[u8; DMX_CHANNELS],
    ) -> Result<(), String> {
        self.packet(target, frame, false)
    }

    /// Ends the stream to `target`: [`TERMINATE_COUNT`] packets marked Stream_Terminated
    /// (carrying `frame`, which a receiver ignores), then the socket closes.
    pub(crate) fn terminate(&mut self, target: &SacnTarget, frame: &[u8; DMX_CHANNELS]) {
        for _ in 0..TERMINATE_COUNT {
            // Counted and kept in the status; a stream that cannot be ended times out at
            // the receiver.
            let _ = self.packet(target, frame, true);
        }
        self.socket = None;
    }

    fn packet(
        &mut self,
        target: &SacnTarget,
        frame: &[u8; DMX_CHANNELS],
        terminated: bool,
    ) -> Result<(), String> {
        let r = self.open(target.dest).and_then(|()| {
            let bytes = encode(
                &self.cid,
                &self.source_name,
                target,
                self.sequence,
                terminated,
                frame,
            );
            let ip = match target.dest {
                SacnDest::Unicast { addr } => addr,
                SacnDest::Multicast { .. } => target.universe.multicast_group(),
            };
            let to = SocketAddrV4::new(ip, self.ports.sacn_port);
            let s = self.socket.as_ref().map(|(s, _)| s);
            s.ok_or_else(|| "no sACN socket".to_owned()).and_then(|s| {
                super::send_nowait(s, &bytes, to).map(|_| ()).map_err(|e| {
                    format!("cannot send sACN universe {} to {to}: {e}", target.universe)
                })
            })
        });
        // A receiver orders packets by the sequence and drops one that goes backwards; a
        // lost datagram still uses its number.
        self.sequence = self.sequence.wrapping_add(1);
        if r.is_err() {
            // A socket set up for an interface that went away or changed its address fails
            // every send: the next packet sets up a new one.
            self.socket = None;
        }
        match &r {
            Ok(()) => {
                self.status.sent += 1;
                self.status.last_error = None;
            }
            Err(e) => {
                tracing::debug!("sacn: {e}");
                self.status.last_error = Some(e.clone());
            }
        }
        r
    }

    /// Sent count and the latest error.
    pub(crate) fn status(&self) -> &OutputStatus {
        &self.status
    }
}

fn open_error(dest: SacnDest, e: &io::Error) -> String {
    match dest {
        SacnDest::Multicast { interface } => {
            format!("cannot send sACN multicast out of {interface}: {e}")
        }
        SacnDest::Unicast { .. } => format!("cannot open an sACN socket: {e}"),
    }
}

/// A UDP socket on `bind` (a port the system picks) set up for `dest`.
fn socket(bind: Ipv4Addr, dest: SacnDest) -> io::Result<Socket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    super::nonblocking_on_windows(&s)?;
    if let SacnDest::Multicast { interface } = dest {
        s.set_multicast_if_v4(&interface)?;
        s.set_multicast_ttl_v4(MULTICAST_TTL)?;
        s.set_multicast_loop_v4(true)?;
    }
    s.bind(&SocketAddrV4::new(bind, 0).into())?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use std::net::UdpSocket;

    use ac2_proto::model::{SacnPriority, SacnUniverse};

    use super::*;

    fn target(addr: Ipv4Addr, universe: u16, priority: u8) -> SacnTarget {
        SacnTarget {
            dest: SacnDest::Unicast { addr },
            universe: SacnUniverse::new(universe).expect("test value"),
            priority: SacnPriority::new(priority).expect("test value"),
        }
    }

    #[test]
    fn encodes_a_data_packet_byte_for_byte() {
        let cid: [u8; 16] = core::array::from_fn(|i| i as u8 + 1);
        let name: [u8; 64] = super::super::fixed_text("ac2 rig", false);
        let mut frame = [0u8; DMX_CHANNELS];
        frame[0] = 0xff;
        frame[511] = 0x07;
        let t = target(Ipv4Addr::LOCALHOST, 0x1234, 150);
        let p = encode(&cid, &name, &t, 0xfe, false, &frame);
        assert_eq!(p.len(), 638);
        // E1.31-2018 Table 4-1, E1.31 Data Packet, by octet.
        assert_eq!(
            p[..38],
            [
                0x00, 0x10, // 0 Preamble Size
                0x00, 0x00, // 2 Post-amble Size
                0x41, 0x53, 0x43, 0x2d, 0x45, 0x31, 0x2e, 0x31, 0x37, 0x00, 0x00,
                0x00, // 4 ACN Packet Identifier "ASC-E1.17"
                0x72, 0x6e, // 16 Flags and Length: 0x7000 | 622
                0x00, 0x00, 0x00, 0x04, // 18 Vector: VECTOR_ROOT_E131_DATA
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, // 22 CID
            ]
        );
        assert_eq!(
            p[38..44],
            [
                0x72, 0x58, // 38 Flags and Length: 0x7000 | 600
                0x00, 0x00, 0x00, 0x02, // 40 Vector: VECTOR_E131_DATA_PACKET
            ]
        );
        assert_eq!(&p[44..52], b"ac2 rig\0"); // 44 Source Name, NUL-padded to 64
        assert!(p[52..108].iter().all(|b| *b == 0));
        assert_eq!(
            p[108..126],
            [
                150, // 108 Priority
                0x00, 0x00, // 109 Synchronization Address
                0xfe, // 111 Sequence Number
                0x00, // 112 Options
                0x12, 0x34, // 113 Universe
                0x72, 0x0b, // 115 Flags and Length: 0x7000 | 523
                0x02, // 117 Vector: VECTOR_DMP_SET_PROPERTY
                0xa1, // 118 Address Type & Data Type
                0x00, 0x00, // 119 First Property Address
                0x00, 0x01, // 121 Address Increment
                0x02, 0x01, // 123 Property value count: START Code + 512
                0x00, // 125 START Code
            ]
        );
        assert_eq!((p[126], p[637]), (0xff, 0x07));
        // §6.2.6: Stream_Terminated is Options bit 6.
        let p = encode(&cid, &name, &t, 0, true, &frame);
        assert_eq!(p[112], 0x40);
    }

    #[test]
    fn the_cid_is_the_rigs_own_and_a_version_8_uuid() {
        let a = cid(RigId(0x0102_0304_0506_0708));
        assert_eq!(a, cid(RigId(0x0102_0304_0506_0708)));
        assert_ne!(a, cid(RigId(0x0102_0304_0506_0709)));
        assert_ne!(a, cid(RigId(0x1102_0304_0506_0708)));
        assert_eq!(a[6] >> 4, 8, "version");
        assert_eq!(a[8] >> 6, 0b10, "variant");
        assert_eq!(&a[..6], &[1, 2, 3, 4, 5, 6]);
        assert_eq!(&a[10..12], &[7, 8]);
    }

    #[test]
    fn unicast_counts_up_and_ends_with_three_terminations() {
        let rx = UdpSocket::bind("127.0.0.1:0").expect("test value");
        let ports = DmxPorts {
            sacn_port: rx.local_addr().expect("test value").port(),
            ..DmxPorts::LOOPBACK
        };
        let mut s = SacnSender::new(ports, RigId(42), "rig");
        let t = target(Ipv4Addr::LOCALHOST, 7, 100);
        let mut frame = [0u8; DMX_CHANNELS];
        frame[2] = 33;
        s.send(&t, &frame).expect("test value");
        s.send(&t, &frame).expect("test value");
        s.terminate(&t, &frame);
        let mut buf = [0u8; 700];
        let mut got = Vec::new();
        for _ in 0..5 {
            let n = rx.recv(&mut buf).expect("test value");
            assert_eq!(n, PACKET_LEN);
            assert_eq!(&buf[22..38], &cid(RigId(42)));
            assert_eq!(buf[113..115], [0, 7]);
            assert_eq!(buf[128], 33);
            got.push((buf[111], buf[112]));
        }
        assert_eq!(got, [(0, 0), (1, 0), (2, 0x40), (3, 0x40), (4, 0x40)]);
        assert_eq!(s.status().sent, 5);
        assert!(s.socket.is_none(), "closed after the stream ended");
        // The sequence wraps at 255 to 0.
        s.sequence = 255;
        s.send(&t, &frame).expect("test value");
        rx.recv(&mut buf).expect("test value");
        assert_eq!((buf[111], s.sequence), (255, 0));
    }

    #[test]
    fn a_failed_send_sets_up_a_new_socket_for_the_next() {
        let rx = UdpSocket::bind("127.0.0.1:0").expect("test value");
        rx.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .expect("test value");
        let ports = DmxPorts {
            sacn_port: rx.local_addr().expect("test value").port(),
            ..DmxPorts::LOOPBACK
        };
        let mut s = SacnSender::new(ports, RigId(3), "rig");
        let t = target(Ipv4Addr::LOCALHOST, 1, 100);
        let frame = [0u8; DMX_CHANNELS];
        // A socket that cannot send there, as one whose interface went away: an IPv6-only
        // socket refuses every IPv4 destination.
        let broken = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP)).expect("v6");
        broken.set_only_v6(true).expect("v6 only");
        s.socket = Some((broken, t.dest));
        assert!(s.send(&t, &frame).is_err());
        assert!(s.status().last_error.is_some());
        assert!(s.socket.is_none(), "the failed socket is dropped");
        s.send(&t, &frame).expect("a new socket");
        let mut buf = [0u8; 700];
        assert_eq!(rx.recv(&mut buf).expect("test value"), PACKET_LEN);
        assert_eq!(s.status().last_error, None);
    }

    #[test]
    fn multicast_takes_the_interface_the_target_names() {
        // Only the socket is set up: nothing is sent to a group.
        let mut s = SacnSender::new(DmxPorts::LOOPBACK, RigId(1), "rig");
        let lo = SacnDest::Multicast {
            interface: Ipv4Addr::LOCALHOST,
        };
        s.open(lo).expect("loopback is an interface");
        let sock = &s.socket.as_ref().expect("open").0;
        assert_eq!(
            sock.multicast_if_v4().expect("test value"),
            Ipv4Addr::LOCALHOST
        );
        assert_eq!(sock.multicast_ttl_v4().expect("test value"), 1);
        assert!(sock.multicast_loop_v4().expect("test value"));
        // An address no interface of this host has (TEST-NET-1) is refused, and the
        // socket in use stays.
        let away = SacnDest::Multicast {
            interface: Ipv4Addr::new(192, 0, 2, 77),
        };
        let e = s.check_dest(away).expect_err("not this host's");
        assert!(e.contains("192.0.2.77"), "{e}");
        assert!(s.socket.is_some());
        assert!(s.open(away).is_err());
    }

    #[test]
    fn a_target_names_a_receiver_or_an_interface() {
        assert!(check_target(&target(Ipv4Addr::new(10, 0, 0, 9), 1, 100)).is_ok());
        for bad in [
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::BROADCAST,
            Ipv4Addr::new(239, 255, 0, 1),
        ] {
            assert!(check_target(&target(bad, 1, 100)).is_err(), "{bad}");
            let t = SacnTarget {
                dest: SacnDest::Multicast { interface: bad },
                ..target(bad, 1, 100)
            };
            assert!(check_target(&t).is_err(), "{bad}");
        }
    }
}
