//! The rig's DMX universe on the network: Art-Net ([`artnet`], with the ArtPoll listener in
//! [`poll`]) and sACN, ANSI E1.31 ([`sacn`]). Both carry the same frame, made once by the
//! control loop's cue engine (`control::artnet`); each output only encodes and sends it.
//!
//! Sends never wait: the control loop must not stall on a network that is slow or gone.
//!
//! Not done: no sACN universe discovery packets (a source browser does not list the rig;
//! a receiver patched to the universe hears it all the same), no sACN synchronisation
//! packets, no periodic ArtPoll of our own (the rig sends to the one target it is given,
//! not to subscribers it discovers), and IPv4 only.

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
/// lost datagram (the next keepalive repeats it), never a stalled control loop. A Winsock
/// UDP send can wait for buffer space like any other, so there the socket itself is
/// non-blocking ([`nonblocking_on_windows`]).
fn send_nowait(s: &Socket, buf: &[u8], to: SocketAddrV4) -> io::Result<usize> {
    let to = SockAddr::from(to);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let r = s.send_to_with_flags(buf, &to, libc::MSG_DONTWAIT);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let r = s.send_to(buf, &to);
    r.map_err(dropped)
}

/// A send refused for want of buffer room, said as what it is: this frame was dropped.
fn dropped(e: io::Error) -> io::Error {
    if e.kind() == io::ErrorKind::WouldBlock {
        io::Error::new(e.kind(), "the send buffer is full: frame dropped")
    } else {
        e
    }
}

/// Makes a sending socket non-blocking on Windows, where [`send_nowait`] has no per-call
/// flag; elsewhere the socket stays blocking (a handle of the Art-Net socket waits for
/// ArtPoll on it).
fn nonblocking_on_windows(s: &Socket) -> io::Result<()> {
    #[cfg(windows)]
    s.set_nonblocking(true)?;
    let _ = s;
    Ok(())
}

/// Winsock's own calls that socket2 does not make.
#[cfg(windows)]
mod winsock {
    use std::io;
    use std::os::windows::io::AsRawSocket;
    use std::time::Duration;

    use windows_sys::Win32::Networking::WinSock::{
        POLLRDNORM, SO_EXCLUSIVEADDRUSE, SOCKET, SOL_SOCKET, WSAGetLastError, WSAPOLLFD, WSAPoll,
        setsockopt,
    };

    /// SO_EXCLUSIVEADDRUSE: without it, a later socket that sets SO_REUSEADDR can bind the
    /// same port and take its unicast.
    pub(super) fn exclusive(s: &impl AsRawSocket) -> io::Result<()> {
        let on: i32 = 1;
        // SAFETY: `on` outlives the call, and the length passed is its size.
        #[allow(unsafe_code)]
        let r = unsafe {
            setsockopt(
                s.as_raw_socket() as SOCKET,
                SOL_SOCKET,
                SO_EXCLUSIVEADDRUSE,
                (&raw const on).cast(),
                size_of::<i32>() as i32,
            )
        };
        if r == 0 {
            Ok(())
        } else {
            // SAFETY: reads the calling thread's last Winsock error; no arguments.
            #[allow(unsafe_code)]
            Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }))
        }
    }

    /// Waits up to `timeout` for a datagram on `s`. The Art-Net socket is non-blocking for
    /// its sends, and a duplicated Winsock handle may share that mode, so the listener's
    /// read timeout alone would not make it wait.
    pub(super) fn wait_readable(s: &impl AsRawSocket, timeout: Duration) {
        let mut fd = WSAPOLLFD {
            fd: s.as_raw_socket() as SOCKET,
            events: POLLRDNORM,
            revents: 0,
        };
        let ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: one valid WSAPOLLFD, and the count passed is 1. A failed wait only makes
        // the caller read at once.
        #[allow(unsafe_code)]
        unsafe {
            WSAPoll(&mut fd, 1, ms);
        }
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
    fn a_full_buffer_reads_as_a_dropped_frame() {
        let e = dropped(io::Error::from(io::ErrorKind::WouldBlock));
        assert_eq!(e.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(e.to_string(), "the send buffer is full: frame dropped");
        let e = dropped(io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    }

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
