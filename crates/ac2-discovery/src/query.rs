//! The browser's own `_ac2._tcp` questions, one per interface address.
//!
//! The question leaves from the mDNS port itself, so it is a regular multicast query (RFC
//! 6762 §5.2, not a §6.7 legacy one) and responders answer to the multicast group, where
//! the responder's socket hears them on every interface that joined it. The socket shares
//! the port the way every mDNS stack on the host does (`SO_REUSEADDR`, plus `SO_REUSEPORT`
//! on Unix) and lives only for the send.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use crate::{Options, SERVICE_TYPE};

const GROUP_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const GROUP_V6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb);

/// One interface address the browser asked on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceQuery {
    /// Interface name (`br0`).
    pub name: String,
    /// The address the question left from.
    pub addr: IpAddr,
    /// Why the question could not be sent, if it could not.
    pub error: Option<String>,
}

/// A DNS message with one PTR question for [`SERVICE_TYPE`], class IN, multicast response
/// wanted (the QU bit clear).
pub(crate) fn ptr_query() -> Vec<u8> {
    // Header: id 0 (RFC 6762 §18.1), flags 0, one question, no records.
    let mut m = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in SERVICE_TYPE.trim_end_matches('.').split('.') {
        m.push(label.len() as u8);
        m.extend_from_slice(label.as_bytes());
    }
    m.push(0);
    m.extend_from_slice(&[0, 12, 0, 1]);
    m
}

fn socket(domain: Domain, port: u16) -> std::io::Result<Socket> {
    let s = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(unix)]
    s.set_reuse_port(true)?;
    let any: SocketAddr = if domain == Domain::IPV6 {
        s.set_only_v6(true)?;
        SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0).into()
    } else {
        SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into()
    };
    s.bind(&any.into())?;
    Ok(s)
}

fn send(i: &if_addrs::Interface, port: u16, msg: &[u8]) -> std::io::Result<()> {
    match i.ip() {
        IpAddr::V4(a) => {
            let s = socket(Domain::IPV4, port)?;
            s.set_multicast_if_v4(&a)?;
            s.set_multicast_ttl_v4(255)?;
            s.send_to(msg, &SockAddr::from(SocketAddrV4::new(GROUP_V4, port)))?;
        }
        IpAddr::V6(_) => {
            let index = i.index.unwrap_or(0);
            let s = socket(Domain::IPV6, port)?;
            s.set_multicast_if_v6(index)?;
            s.set_multicast_hops_v6(255)?;
            s.send_to(
                msg,
                &SockAddr::from(SocketAddrV6::new(GROUP_V6, port, 0, index)),
            )?;
        }
    }
    Ok(())
}

/// The interfaces to ask on: every address of every interface, loopback only when asked
/// for (and then only that), point-to-point links never (they carry no multicast). An
/// interface not flagged running is still asked: some bridges never report the flag.
fn interfaces(opts: &Options) -> Vec<if_addrs::Interface> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| {
            if opts.loopback_only {
                i.is_loopback() && i.ip().is_ipv4()
            } else {
                !i.is_loopback() && !i.is_p2p()
            }
        })
        .collect()
}

/// Sends one `_ac2._tcp` question on every interface address; says where it went.
pub fn query_interfaces(opts: &Options) -> Vec<InterfaceQuery> {
    let msg = ptr_query();
    interfaces(opts)
        .into_iter()
        .map(|i| InterfaceQuery {
            error: send(&i, opts.mdns_port, &msg).err().map(|e| e.to_string()),
            addr: i.ip(),
            name: i.name,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_question_is_a_ptr_for_the_service_type() {
        let m = ptr_query();
        assert_eq!(&m[..12], &[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&m[12..], b"\x04_ac2\x04_tcp\x05local\x00\x00\x0c\x00\x01");
    }

    #[test]
    fn loopback_only_asks_on_loopback_alone() {
        let opts = Options {
            mdns_port: 0,
            loopback_only: true,
        };
        assert!(interfaces(&opts).iter().all(|i| i.is_loopback()));
        let all = interfaces(&Options::default());
        assert!(all.iter().all(|i| !i.is_loopback()));
    }
}
