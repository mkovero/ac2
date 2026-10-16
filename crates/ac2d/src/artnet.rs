//! Sends the rig's DMX universe as Art-Net ArtDmx packets over UDP: one non-blocking IPv4
//! socket (Art-Net is IPv4 only) bound to an ephemeral port on first use. Like OSC, a send
//! never waits and never panics; every failure is kept in the status as the operating
//! system said it.
//!
//! The source port is ephemeral rather than Art-Net's 6454: receivers take ArtDmx from any
//! port, and binding 6454 would collide with a lighting program (QLC+) listening on the
//! same machine.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

use ac2_proto::model::{DMX_CHANNELS, OutputStatus, PortAddress};

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
    p[..8].copy_from_slice(b"Art-Net\0");
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

/// The socket and how sending fares.
#[derive(Debug, Default)]
pub(crate) struct ArtnetSender {
    socket: Option<UdpSocket>,
    status: OutputStatus,
}

impl ArtnetSender {
    /// Sends `frame` on `universe` to `target`. The error is also kept in the status.
    pub(crate) fn send(
        &mut self,
        target: SocketAddr,
        universe: PortAddress,
        frame: &[u8; DMX_CHANNELS],
    ) -> Result<(), String> {
        let bytes = encode(universe, frame);
        let r = self
            .socket()
            .and_then(|s| s.send_to(&bytes, target))
            .map(|_| ())
            .map_err(|e| format!("cannot send Art-Net universe {universe} to {target}: {e}"));
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

    fn socket(&mut self) -> io::Result<&UdpSocket> {
        if self.socket.is_none() {
            let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
            s.set_nonblocking(true)?;
            // Art-Net is often broadcast to a subnet (2.255.255.255, or the LAN's); without
            // SO_BROADCAST the kernel refuses that send (EACCES).
            s.set_broadcast(true)?;
            self.socket = Some(s);
        }
        self.socket
            .as_ref()
            .ok_or_else(|| io::Error::other("no socket"))
    }
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

    #[test]
    fn sends_and_counts() {
        let rx = UdpSocket::bind("127.0.0.1:0").expect("test value");
        let mut s = ArtnetSender::default();
        let mut frame = [0u8; DMX_CHANNELS];
        frame[4] = 9;
        let uni = PortAddress::new(3).expect("test value");
        s.send(rx.local_addr().expect("test value"), uni, &frame)
            .expect("test value");
        let mut buf = [0u8; 600];
        let n = rx.recv(&mut buf).expect("test value");
        assert_eq!(buf[..n], encode(uni, &frame));
        assert_eq!(s.status().sent, 1);
        assert!(
            s.socket()
                .expect("test value")
                .broadcast()
                .expect("test value")
        );
    }
}
