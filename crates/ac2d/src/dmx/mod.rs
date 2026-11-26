//! The rig's DMX universe on the network: Art-Net ([`artnet`], with the ArtPoll listener in
//! [`poll`]) and sACN, ANSI E1.31 ([`sacn`]). Both carry the same frame, made once by the
//! control loop's cue engine (`control::artnet`); each output only encodes and sends it.
//!
//! Sends never wait: the control loop must not stall on a network that is slow or gone.

pub(crate) mod artnet;
mod poll;
pub(crate) mod sacn;

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};

use ac2_proto::model::{ARTNET_PORT, SACN_PORT};
use socket2::{SockAddr, Socket};

/// The UDP addresses of the DMX outputs. The daemon program uses [`DmxPorts::STANDARD`];
/// a daemon run inside a test uses its own on the loopback interface, so that a test never
/// takes the host's real Art-Net or sACN ports (another rig's daemon may hold them) and
/// never reaches another host. Not an operator setting: Art-Net and sACN each have one
/// port, and a rig on another one would be found by no console.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmxPorts {
    /// Where the Art-Net socket binds: it sends from there and hears ArtPoll there.
    pub artnet_bind: SocketAddrV4,
    /// The port an ArtPollReply goes to on the poller.
    pub artnet_reply_port: u16,
    /// The local address the sACN socket binds (its port is the system's choice: E1.31
    /// fixes only the receivers' port).
    pub sacn_bind: Ipv4Addr,
    /// The port sACN receivers listen on.
    pub sacn_port: u16,
}

impl DmxPorts {
    /// Art-Net on 6454 on every interface, sACN to 5568.
    pub const STANDARD: Self = Self {
        artnet_bind: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, ARTNET_PORT),
        artnet_reply_port: ARTNET_PORT,
        sacn_bind: Ipv4Addr::UNSPECIFIED,
        sacn_port: SACN_PORT,
    };

    /// Both outputs bound to the loopback interface, Art-Net on a port the system picks:
    /// nothing reaches another host, and the host's real ports stay free.
    pub const LOOPBACK: Self = Self {
        artnet_bind: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
        artnet_reply_port: ARTNET_PORT,
        sacn_bind: Ipv4Addr::LOCALHOST,
        sacn_port: SACN_PORT,
    };
}

/// Sends `buf` to `to` without waiting for room in the socket's buffer: a full buffer is a
/// lost datagram (the next keepalive repeats it), never a stalled control loop.
fn send_nowait(s: &Socket, buf: &[u8], to: SocketAddrV4) -> io::Result<usize> {
    let to = SockAddr::from(to);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        s.send_to_with_flags(buf, &to, libc::MSG_DONTWAIT)
    }
    // Winsock copies a datagram into its buffer or drops it; a UDP send does not wait for
    // the wire.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        s.send_to(buf, &to)
    }
}

/// `text` in a fixed field of `N` bytes, NUL-terminated: cut at a character boundary to at
/// most `N - 1` bytes, every character outside printable ASCII written as `?` when `ascii`.
fn fixed_text<const N: usize>(text: &str, ascii: bool) -> [u8; N] {
    let mut out = [0u8; N];
    let mut n = 0;
    for c in text.chars() {
        let c = if ascii && !(c.is_ascii_graphic() || c == ' ') {
            '?'
        } else {
            c
        };
        let len = c.len_utf8();
        if n + len > N - 1 {
            break;
        }
        c.encode_utf8(&mut out[n..n + len]);
        n += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_text_cuts_at_a_character_and_keeps_the_nul() {
        let t: [u8; 6] = fixed_text("ac2 on kärppä", false);
        assert_eq!(&t, b"ac2 o\0");
        let t: [u8; 10] = fixed_text("ac2 kä", false);
        assert_eq!(&t[..8], "ac2 kä\0".as_bytes());
        let t: [u8; 10] = fixed_text("aaaaaaaaä", false);
        // The ä would end at byte 10, where the NUL has to be.
        assert_eq!(&t, b"aaaaaaaa\0\0");
        let t: [u8; 10] = fixed_text("kärppä\t", true);
        assert_eq!(&t, b"k?rpp??\0\0\0");
    }
}
