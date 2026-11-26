//! Sends the rig's DMX universe as Art-Net ArtDmx packets over UDP, from Art-Net's port
//! 6454 as the specification requires of every packet ("The UDP port used as sources and
//! destinations is 0x1936"): a node that checks the source port drops ArtDmx from any
//! other, and a console finds the rig only by its answer to ArtPoll on that port
//! ([`super::poll`]).
//!
//! The port is bound with address reuse, so a lighting program on the same host (QLC+,
//! a visualiser) that shares it keeps working beside the daemon: broadcasts reach both.
//! When 6454 cannot be bound (a program holds it without sharing), frames still go out,
//! from a port the system picks, and the socket's state says so in `rig.artnet_socket`
//! and in the `artnet.target` reply: the light keeps working on the nodes that accept it,
//! and the operator reads why a console does not list the rig, instead of a dark rig
//! with nothing to show for it. The daemon tries 6454 again every [`RETRY`].
//!
//! Like OSC, a send never waits and never panics; every failure is kept in the status as
//! the operating system said it.

use std::io;
use std::net::{SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use ac2_proto::model::{ArtnetBindFailure, ArtnetSocket, DMX_CHANNELS, OutputStatus, PortAddress};
use socket2::{Domain, Protocol, Socket, Type};

use super::DmxPorts;
use super::poll::{ID, Listener, ReplyInfo};

/// How often a socket not on Art-Net's port tries to bind it again: the program holding
/// it may have quit, and a rig left running all season should come back to conforming on
/// its own.
pub(crate) const RETRY: Duration = Duration::from_secs(30);

/// Bytes of an ArtDmx packet carrying a whole universe: an 18-byte header, then the data.
pub(crate) const PACKET_LEN: usize = 18 + DMX_CHANNELS;

/// The ArtDmx packet for `frame` on `universe`.
///
/// `Sequence` is 0, which tells receivers not to reorder: each packet is the whole universe
/// and the keepalive resends it every second, so a datagram delivered out of order is
/// corrected within a second; and a counter that restarts with the daemon could make a node
/// that remembers a higher sequence drop the restarted daemon's frames.
pub(crate) fn encode(universe: PortAddress, frame: &[u8; DMX_CHANNELS]) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[..8].copy_from_slice(&ID);
    // OpCode ArtDmx, low byte first (the one little-endian field of the header).
    p[8..10].copy_from_slice(&0x5000u16.to_le_bytes());
    // Protocol version 14, high byte first.
    p[10..12].copy_from_slice(&14u16.to_be_bytes());
    p[12] = 0; // Sequence: disabled.
    p[13] = 0; // Physical: informational only.
    p[14] = universe.sub_uni();
    p[15] = universe.net();
    p[16..18].copy_from_slice(&(DMX_CHANNELS as u16).to_be_bytes());
    p[18..].copy_from_slice(frame);
    p
}

/// The socket, its listener and how sending fares.
#[derive(Debug)]
pub(crate) struct ArtnetSender {
    ports: DmxPorts,
    socket: Option<Socket>,
    /// Answers ArtPoll while the socket is on Art-Net's port.
    listener: Option<Listener>,
    /// The universe the ArtPollReply names.
    universe: Option<PortAddress>,
    rig_name: String,
    state: ArtnetSocket,
    /// When a socket not on Art-Net's port tries it again.
    retry_at: Option<Instant>,
    status: OutputStatus,
}

impl ArtnetSender {
    pub(crate) fn new(ports: DmxPorts, rig_name: &str) -> Self {
        Self {
            ports,
            socket: None,
            listener: None,
            universe: None,
            rig_name: rig_name.to_owned(),
            state: ArtnetSocket::Closed,
            retry_at: None,
            status: OutputStatus::default(),
        }
    }

    /// Opens the socket unless it is open: on Art-Net's port, else on one the system
    /// picks. The state after it.
    pub(crate) fn open(&mut self, now: Instant) -> &ArtnetSocket {
        if self.socket.is_none() {
            match self.bind_conforming() {
                Ok(()) => {}
                Err(e) => self.bind_fallback(&e, now),
            }
        }
        &self.state
    }

    /// Closes the socket and stops answering ArtPoll.
    pub(crate) fn close(&mut self) {
        self.listener = None;
        self.socket = None;
        self.retry_at = None;
        self.state = ArtnetSocket::Closed;
    }

    /// How the socket is bound.
    pub(crate) fn state(&self) -> &ArtnetSocket {
        &self.state
    }

    /// When the next try for Art-Net's port is due.
    pub(crate) fn retry_due(&self) -> Option<Instant> {
        self.retry_at
    }

    /// Tries Art-Net's port again when due; whether the state changed.
    pub(crate) fn retry(&mut self, now: Instant) -> bool {
        if self.retry_at.is_none_or(|t| t > now) {
            return false;
        }
        let before = self.socket.take();
        match self.bind_conforming() {
            Ok(()) => {
                tracing::info!("artnet: port {} bound at last", self.ports.artnet_bind);
                self.retry_at = None;
                true
            }
            Err(_) => {
                self.socket = before;
                self.retry_at = Some(now + RETRY);
                false
            }
        }
    }

    fn bind_conforming(&mut self) -> io::Result<()> {
        let s = bind(self.ports.artnet_bind, true)?;
        let clone = UdpSocket::from(s.try_clone()?);
        let mut info = ReplyInfo::new(&self.rig_name);
        info.universe = self.universe;
        let listener = Listener::spawn(clone, self.ports.artnet_reply_port, info)?;
        self.socket = Some(s);
        self.listener = Some(listener);
        self.state = ArtnetSocket::Conforming;
        Ok(())
    }

    fn bind_fallback(&mut self, e: &io::Error, now: Instant) {
        let reason = if e.kind() == io::ErrorKind::AddrInUse {
            ArtnetBindFailure::PortTaken
        } else {
            ArtnetBindFailure::Refused
        };
        let any = SocketAddrV4::new(*self.ports.artnet_bind.ip(), 0);
        match bind(any, false).and_then(|s| Ok((s.local_addr()?, s))) {
            Ok((local, s)) => {
                let from_port = local.as_socket().map_or(0, |a| a.port());
                tracing::warn!(
                    "artnet: cannot bind {} ({e}): sending from port {from_port}; strict \
                     nodes drop these frames and no console lists this rig",
                    self.ports.artnet_bind
                );
                self.socket = Some(s);
                self.retry_at = Some(now + RETRY);
                self.state = ArtnetSocket::NotConforming {
                    from_port,
                    reason,
                    os_error: e.to_string(),
                };
            }
            Err(e2) => {
                let msg = format!("cannot open an Art-Net socket: {e2}");
                tracing::warn!("artnet: {msg}");
                self.status.last_error = Some(msg);
                self.state = ArtnetSocket::Closed;
            }
        }
    }

    /// Sends `frame` on `universe` to `target`, opening the socket first when it is
    /// closed. The error is also kept in the status.
    pub(crate) fn send(
        &mut self,
        target: SocketAddr,
        universe: PortAddress,
        frame: &[u8; DMX_CHANNELS],
        now: Instant,
    ) -> Result<(), String> {
        if self.universe != Some(universe) {
            self.universe = Some(universe);
            if let Some(l) = &self.listener {
                l.set_universe(Some(universe));
            }
        }
        self.open(now);
        let bytes = encode(universe, frame);
        let r = match (&self.socket, target) {
            (Some(s), SocketAddr::V4(to)) => super::send_nowait(s, &bytes, to)
                .map(|_| ())
                .map_err(|e| format!("cannot send Art-Net universe {universe} to {target}: {e}")),
            (Some(_), SocketAddr::V6(_)) => Err(format!("{target}: Art-Net is IPv4 only")),
            (None, _) => Err(self
                .status
                .last_error
                .clone()
                .unwrap_or_else(|| "no Art-Net socket".into())),
        };
        match &r {
            Ok(()) => {
                self.status.sent += 1;
                self.status.last_error = None;
            }
            Err(e) => {
                tracing::debug!("artnet: {e}");
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

/// A UDP socket on `addr` that may broadcast; `share`: with address reuse, so that another
/// Art-Net program on the host can hold the same port.
fn bind(addr: SocketAddrV4, share: bool) -> io::Result<Socket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    if share {
        s.set_reuse_address(true)?;
        // BSD sockets share an exact address and port only with SO_REUSEPORT as well.
        #[cfg(target_os = "macos")]
        s.set_reuse_port(true)?;
    }
    // Art-Net is often broadcast to a subnet (2.255.255.255, or the LAN's); without
    // SO_BROADCAST the kernel refuses that send (EACCES).
    s.set_broadcast(true)?;
    s.bind(&addr.into())?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_known_frame_byte_for_byte() {
        let mut frame = [0u8; DMX_CHANNELS];
        frame[0] = 0xff;
        frame[11] = 0x80;
        frame[511] = 0x01;
        let uni = PortAddress::from_parts(0x12, 0x3, 0x4).expect("test value");
        let p = encode(uni, &frame);
        assert_eq!(p.len(), 530);
        assert_eq!(
            p[..18],
            [
                b'A', b'r', b't', b'-', b'N', b'e', b't', 0, // ID
                0x00, 0x50, // OpCode 0x5000, little-endian
                0x00, 0x0e, // ProtVer 14, big-endian
                0x00, // Sequence
                0x00, // Physical
                0x34, // SubUni: sub-net 3, universe 4
                0x12, // Net
                0x02, 0x00, // Length 512, big-endian
            ]
        );
        assert_eq!(p[18], 0xff);
        assert_eq!(p[18 + 11], 0x80);
        assert_eq!(p[18 + 511], 0x01);
        assert_eq!(p[19..29], [0u8; 10]);
    }

    #[test]
    fn the_port_address_splits_into_net_and_subuni() {
        let p = encode(PortAddress::new(0x7fff).expect("test value"), &[0; 512]);
        assert_eq!((p[14], p[15]), (0xff, 0x7f));
        let p = encode(PortAddress::new(1).expect("test value"), &[0; 512]);
        assert_eq!((p[14], p[15]), (0x01, 0x00));
        let p = encode(PortAddress::new(256).expect("test value"), &[0; 512]);
        assert_eq!((p[14], p[15]), (0x00, 0x01));
    }

    fn ephemeral() -> DmxPorts {
        DmxPorts::LOOPBACK
    }

    #[test]
    fn sends_from_its_bound_port_and_counts() {
        let rx = UdpSocket::bind("127.0.0.1:0").expect("test value");
        let mut s = ArtnetSender::new(ephemeral(), "rig");
        let mut frame = [0u8; DMX_CHANNELS];
        frame[4] = 9;
        let uni = PortAddress::new(3).expect("test value");
        let now = Instant::now();
        s.send(rx.local_addr().expect("test value"), uni, &frame, now)
            .expect("test value");
        let mut buf = [0u8; 600];
        let (n, from) = rx.recv_from(&mut buf).expect("test value");
        assert_eq!(buf[..n], encode(uni, &frame));
        assert_eq!(s.status().sent, 1);
        assert_eq!(*s.state(), ArtnetSocket::Conforming);
        let bound = s
            .socket
            .as_ref()
            .and_then(|s| s.local_addr().ok())
            .and_then(|a| a.as_socket())
            .expect("bound");
        assert_eq!(from, bound, "sent from the bound socket");
        assert!(
            s.socket
                .as_ref()
                .is_some_and(|s| s.broadcast().unwrap_or(false))
        );
        s.close();
        assert_eq!(*s.state(), ArtnetSocket::Closed);
    }

    #[test]
    fn a_port_held_without_sharing_falls_back_and_says_so() {
        // A program that holds the port without address reuse.
        let held = UdpSocket::bind("127.0.0.1:0").expect("test value");
        let SocketAddr::V4(port) = held.local_addr().expect("test value") else {
            panic!("v4");
        };
        let ports = DmxPorts {
            artnet_bind: port,
            ..DmxPorts::LOOPBACK
        };
        let rx = UdpSocket::bind("127.0.0.1:0").expect("test value");
        let mut s = ArtnetSender::new(ports, "rig");
        let t0 = Instant::now();
        let from_port = match s.open(t0).clone() {
            ArtnetSocket::NotConforming {
                from_port,
                reason: ArtnetBindFailure::PortTaken,
                os_error,
            } => {
                assert!(!os_error.is_empty());
                from_port
            }
            other => panic!("{other:?}"),
        };
        assert_ne!(from_port, port.port());
        assert_eq!(s.retry_due(), Some(t0 + RETRY));
        // Frames still go out, from the port it says.
        let uni = PortAddress::new(0).expect("test value");
        s.send(rx.local_addr().expect("test value"), uni, &[0; 512], t0)
            .expect("test value");
        let mut buf = [0u8; 600];
        let (_, from) = rx.recv_from(&mut buf).expect("test value");
        assert_eq!(from.port(), from_port);
        // Not due: nothing tried. Due while still held: a later try. Freed: conforming.
        assert!(!s.retry(t0));
        assert!(!s.retry(t0 + RETRY));
        assert_eq!(s.retry_due(), Some(t0 + RETRY + RETRY));
        drop(held);
        assert!(s.retry(t0 + RETRY + RETRY));
        assert_eq!(*s.state(), ArtnetSocket::Conforming);
        assert_eq!(s.retry_due(), None);
    }
}
