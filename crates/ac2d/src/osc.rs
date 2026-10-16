//! Sends OSC messages over UDP: one non-blocking socket per address family, bound to an
//! ephemeral port on first use. A send never waits (a full send buffer is an error like any
//! other) and never panics; every failure is counted in the status as the operating system
//! said it, so the control loop stays on time whatever the network does.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

use ac2_proto::model::{OscArg, OscMessage, OscStatus};
use rosc::{OscPacket, OscType};

/// The OSC packet for `msg`, with `level` (dB SPL) appended as a float when the message
/// asks for it and a level is known.
pub(crate) fn encode(msg: &OscMessage, level: Option<f32>) -> Result<Vec<u8>, String> {
    let mut args: Vec<OscType> = msg
        .args
        .iter()
        .map(|a| match a {
            OscArg::Int(i) => OscType::Int(*i),
            OscArg::Float(f) => OscType::Float(*f),
            OscArg::Str(s) => OscType::String(s.clone()),
        })
        .collect();
    if msg.append_level
        && let Some(l) = level
    {
        args.push(OscType::Float(l));
    }
    let packet = OscPacket::Message(rosc::OscMessage {
        addr: msg.path.as_str().to_owned(),
        args,
    });
    rosc::encoder::encode(&packet).map_err(|e| format!("cannot encode {}: {e}", msg.path))
}

/// The UDP sockets and how sending fares.
#[derive(Debug, Default)]
pub(crate) struct OscSender {
    v4: Option<UdpSocket>,
    v6: Option<UdpSocket>,
    status: OscStatus,
}

impl OscSender {
    /// Sends `msg` to `target`. The error is also kept in the status.
    pub(crate) fn send(
        &mut self,
        target: SocketAddr,
        msg: &OscMessage,
        level: Option<f32>,
    ) -> Result<(), String> {
        let r = encode(msg, level).and_then(|bytes| {
            self.socket(target)
                .and_then(|s| s.send_to(&bytes, target))
                .map(|_| ())
                .map_err(|e| format!("cannot send {} to {target}: {e}", msg.path))
        });
        match &r {
            Ok(()) => {
                self.status.sent += 1;
                self.status.last_error = None;
            }
            Err(e) => {
                tracing::debug!("osc: {e}");
                self.status.last_error = Some(e.clone());
            }
        }
        r
    }

    /// Sent count and the latest error.
    pub(crate) fn status(&self) -> &OscStatus {
        &self.status
    }

    fn socket(&mut self, target: SocketAddr) -> io::Result<&UdpSocket> {
        let (slot, any): (_, SocketAddr) = match target {
            SocketAddr::V4(_) => (&mut self.v4, (Ipv4Addr::UNSPECIFIED, 0).into()),
            SocketAddr::V6(_) => (&mut self.v6, (Ipv6Addr::UNSPECIFIED, 0).into()),
        };
        if slot.is_none() {
            let s = UdpSocket::bind(any)?;
            s.set_nonblocking(true)?;
            *slot = Some(s);
        }
        slot.as_ref().ok_or_else(|| io::Error::other("no socket"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::OscPath;

    fn msg(append_level: bool) -> OscMessage {
        OscMessage {
            path: OscPath::new("/light/red").expect("test value"),
            args: vec![
                OscArg::Int(1),
                OscArg::Float(0.5),
                OscArg::Str("over".into()),
            ],
            append_level,
        }
    }

    fn decoded(bytes: &[u8]) -> rosc::OscMessage {
        match rosc::decoder::decode_udp(bytes).expect("test value").1 {
            OscPacket::Message(m) => m,
            OscPacket::Bundle(_) => panic!("a bundle"),
        }
    }

    #[test]
    fn encodes_args_in_order() {
        let m = decoded(&encode(&msg(false), Some(101.5)).expect("test value"));
        assert_eq!(m.addr, "/light/red");
        assert_eq!(
            m.args,
            vec![
                OscType::Int(1),
                OscType::Float(0.5),
                OscType::String("over".into())
            ]
        );
    }

    #[test]
    fn appends_the_level_when_asked_and_known() {
        let m = decoded(&encode(&msg(true), Some(101.5)).expect("test value"));
        assert_eq!(m.args.len(), 4);
        assert_eq!(m.args[3], OscType::Float(101.5));
        let m = decoded(&encode(&msg(true), None).expect("test value"));
        assert_eq!(m.args.len(), 3);
    }

    #[test]
    fn sends_and_counts() {
        let rx = UdpSocket::bind("127.0.0.1:0").expect("test value");
        let mut s = OscSender::default();
        s.send(rx.local_addr().expect("test value"), &msg(false), None)
            .expect("test value");
        let mut buf = [0u8; 256];
        let n = rx.recv(&mut buf).expect("test value");
        assert_eq!(decoded(&buf[..n]).addr, "/light/red");
        assert_eq!(s.status().sent, 1);
        assert_eq!(s.status().last_error, None);
    }
}
