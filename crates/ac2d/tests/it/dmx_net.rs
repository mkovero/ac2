//! The DMX outputs on the network, end to end on the fake rig: Art-Net from its own port
//! answering ArtPoll, and sACN beside it. Every socket is on the loopback interface at a
//! port the test picks ([`ac2d::DmxPorts`]): nothing is sent past the host, to a broadcast
//! or a multicast address, and the host's real ports 6454 and 5568 stay free.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use ac2_client::{Client, ClientError};
use ac2_proto::model::{
    ArtnetBindFailure, ArtnetSocket, ArtnetTarget, DmxLevel, PortAddress, Rig, SacnDest,
    SacnPriority, SacnTarget, SacnUniverse, parse_dmx_list,
};
use ac2_proto::{Command, ErrorCode, Warning};
use ac2d::{Daemon, DaemonConfig, DmxPorts};
use common::*;

const WAIT: Duration = Duration::from_secs(20);
const RIG_NAME: &str = "Front of house rig";

fn socket() -> UdpSocket {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    s
}

fn port_of(s: &UdpSocket) -> u16 {
    s.local_addr().unwrap().port()
}

/// A loopback port free a moment ago, for the daemon to bind.
fn free_port() -> u16 {
    port_of(&UdpSocket::bind("127.0.0.1:0").unwrap())
}

fn config(dmx: DmxPorts) -> DaemonConfig {
    let mut cfg = steady_config(realtime_rig(), local_ipc());
    cfg.name = RIG_NAME.into();
    cfg.dmx = dmx;
    cfg
}

fn rig(c: &Client) -> Rig {
    c.view().state.clone().unwrap().rig.clone()
}

async fn rig_until(c: &Client, what: &str, want: impl Fn(&Rig) -> bool) -> Rig {
    let deadline = Instant::now() + WAIT;
    loop {
        let r = rig(c);
        if want(&r) {
            return r;
        }
        assert!(Instant::now() < deadline, "rig never {what}: {r:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The next datagram `want` accepts, with its sender.
fn until(s: &UdpSocket, what: &str, want: impl Fn(&[u8]) -> bool) -> (Vec<u8>, SocketAddr) {
    let deadline = Instant::now() + WAIT;
    let mut buf = [0u8; 1024];
    loop {
        assert!(Instant::now() < deadline, "no {what} within {WAIT:?}");
        if let Ok((n, from)) = s.recv_from(&mut buf)
            && want(&buf[..n])
        {
            return (buf[..n].to_vec(), from);
        }
    }
}

/// Every datagram arriving within `d`.
fn gather(s: &UdpSocket, d: Duration) -> Vec<Vec<u8>> {
    let end = Instant::now() + d;
    let mut buf = [0u8; 1024];
    let mut out = Vec::new();
    while Instant::now() < end {
        if let Ok(n) = s.recv(&mut buf) {
            out.push(buf[..n].to_vec());
        }
    }
    out
}

fn levels(text: &str) -> Vec<DmxLevel> {
    parse_dmx_list(text).unwrap()
}

/// An ArtPoll as a controller sends it (Art-Net 4, ArtPoll fields 1–6).
const ART_POLL: [u8; 14] = [
    b'A', b'r', b't', b'-', b'N', b'e', b't', 0, 0x00, 0x20, 0, 14, 0, 0,
];

fn is_art_dmx(p: &[u8]) -> bool {
    p.len() == 530 && p[8..10] == [0x00, 0x50]
}

fn is_poll_reply(p: &[u8]) -> bool {
    p.len() == 239 && p[..8] == *b"Art-Net\0" && p[8..10] == [0x00, 0x21]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn artnet_sends_from_its_port_and_answers_artpoll_sparingly() {
    init_log();
    let port = free_port();
    let poller = socket();
    let h = Daemon::start(config(DmxPorts {
        artnet_bind: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
        artnet_reply_port: port_of(&poller),
        ..DmxPorts::LOOPBACK
    }))
    .unwrap();
    let c = mirror(&h, "ac2d-dmx-net-test").await;
    let node = socket();
    let (_, warnings) = c
        .call_warned(Command::ArtnetTarget {
            base: rig(&c).rev,
            target: Some(ArtnetTarget {
                addr: node.local_addr().unwrap(),
                universe: PortAddress::from_parts(0, 1, 2).unwrap(),
            }),
        })
        .await
        .unwrap();
    assert_eq!(warnings, []);
    rig_until(&c, "conforming", |r| {
        r.artnet_socket == ArtnetSocket::Conforming
    })
    .await;
    // Frames go out from the Art-Net port.
    let (_, from) = until(&node, "a frame", is_art_dmx);
    assert_eq!(from.port(), port);

    // A flood of ArtPoll, and DMX sent to the rig, which it never takes.
    let rig_addr = (Ipv4Addr::LOCALHOST, port);
    let mut dmx = [0xffu8; 530];
    dmx[..14].copy_from_slice(&ART_POLL);
    dmx[8..10].copy_from_slice(&[0x00, 0x50]);
    poller.send_to(&dmx, rig_addr).unwrap();
    for _ in 0..20 {
        poller.send_to(&ART_POLL, rig_addr).unwrap();
    }
    let replies = gather(&poller, Duration::from_millis(700));
    assert_eq!(replies.len(), 1, "one reply a second to a source");
    let r = &replies[0];
    assert!(is_poll_reply(r));
    // Art-Net 4 ArtPollReply: IP, Port 0x1936, Net/SubSwitch, names, one input port, Style.
    assert_eq!(r[10..14], [127, 0, 0, 1]);
    assert_eq!(r[14..16], [0x36, 0x19]);
    assert_eq!((r[18], r[19]), (0, 1));
    assert_eq!(&r[26..44], b"Front of house ri\0");
    assert_eq!(&r[44..44 + RIG_NAME.len() + 1], b"Front of house rig\0");
    assert_eq!((r[173], r[174], r[186], r[190]), (1, 0x40, 2, 0));
    assert_eq!(r[200], 0x01, "StController");
    let (f, _) = until(&node, "a keepalive", is_art_dmx);
    assert!(f[18..].iter().all(|v| *v == 0), "received DMX not acted on");
    assert!(rig(&c).dmx.is_empty());

    // A second later the source is answered again.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    poller.send_to(&ART_POLL, rig_addr).unwrap();
    until(&poller, "the next reply", is_poll_reply);

    // Universe 0 is kept, with a warning: Art-Net 4 deprecates it.
    let (_, warnings) = c
        .call_warned(Command::ArtnetTarget {
            base: rig(&c).rev,
            target: Some(ArtnetTarget {
                addr: node.local_addr().unwrap(),
                universe: PortAddress::new(0).unwrap(),
            }),
        })
        .await
        .unwrap();
    assert_eq!(warnings, [Warning::ArtnetUniverseZero]);
    rig_until(&c, "on universe 0", |r| {
        r.artnet_target.is_some_and(|t| t.universe.get() == 0)
    })
    .await;

    // Off: the socket closes and polls go unanswered.
    c.call(Command::ArtnetTarget {
        base: rig(&c).rev,
        target: None,
    })
    .await
    .unwrap();
    rig_until(&c, "closed", |r| r.artnet_socket == ArtnetSocket::Closed).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    poller.send_to(&ART_POLL, rig_addr).unwrap();
    assert!(gather(&poller, Duration::from_millis(500)).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_taken_artnet_port_is_said_and_frames_still_go() {
    init_log();
    // A program holding the port without sharing it.
    let held = socket();
    let h = Daemon::start(config(DmxPorts {
        artnet_bind: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port_of(&held)),
        ..DmxPorts::LOOPBACK
    }))
    .unwrap();
    let c = mirror(&h, "ac2d-dmx-net-test").await;
    let node = socket();
    let (_, warnings) = c
        .call_warned(Command::ArtnetTarget {
            base: rig(&c).rev,
            target: Some(ArtnetTarget {
                addr: node.local_addr().unwrap(),
                universe: PortAddress::new(1).unwrap(),
            }),
        })
        .await
        .unwrap();
    let from_port = match warnings.as_slice() {
        [
            Warning::ArtnetNotConforming {
                from_port,
                reason: ArtnetBindFailure::PortTaken,
            },
        ] => *from_port,
        other => panic!("{other:?}"),
    };
    let r = rig_until(&c, "not conforming", |r| {
        matches!(r.artnet_socket, ArtnetSocket::NotConforming { .. })
    })
    .await;
    match r.artnet_socket {
        ArtnetSocket::NotConforming {
            from_port: p,
            reason: ArtnetBindFailure::PortTaken,
            os_error,
        } => assert!(p == from_port && !os_error.is_empty()),
        other => panic!("{other:?}"),
    }
    let (_, from) = until(&node, "a frame", is_art_dmx);
    assert_eq!(from.port(), from_port);
}

fn sacn_unicast(universe: u16) -> SacnTarget {
    SacnTarget {
        dest: SacnDest::Unicast {
            addr: Ipv4Addr::LOCALHOST,
        },
        universe: SacnUniverse::new(universe).unwrap(),
        priority: SacnPriority::new(150).unwrap(),
    }
}

/// An E1.31 data packet's sequence, options and slot `n` (1–512).
fn seq(p: &[u8]) -> u8 {
    p[111]
}
fn terminated(p: &[u8]) -> bool {
    p[112] & 0x40 != 0
}
fn slot(p: &[u8], n: usize) -> u8 {
    p[125 + n]
}

/// Sets the sACN target and waits for the mirror to show it: the next edit is made on the
/// rev it brought.
async fn set_sacn(c: &Client, target: Option<SacnTarget>) -> Result<(), ClientError> {
    c.call(Command::SacnTarget {
        base: rig(c).rev,
        target,
    })
    .await?;
    rig_until(c, "the sACN target", |r| r.sacn_target == target).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sacn_carries_the_frame_beside_artnet_and_ends_its_stream() {
    init_log();
    let rx = socket();
    let h = Daemon::start(config(DmxPorts {
        sacn_port: port_of(&rx),
        ..DmxPorts::LOOPBACK
    }))
    .unwrap();
    let c = mirror(&h, "ac2d-dmx-net-test").await;
    let refused = |e: ClientError| match e {
        ClientError::Daemon(p) => p.code,
        other => panic!("{other:?}"),
    };
    // A receiver that is a group or nobody, and an interface this host does not have.
    for dest in [
        SacnDest::Unicast {
            addr: Ipv4Addr::new(239, 255, 0, 1),
        },
        SacnDest::Unicast {
            addr: Ipv4Addr::UNSPECIFIED,
        },
        SacnDest::Multicast {
            interface: Ipv4Addr::new(192, 0, 2, 77),
        },
    ] {
        let t = SacnTarget {
            dest,
            ..sacn_unicast(1)
        };
        let e = set_sacn(&c, Some(t)).await.unwrap_err();
        assert_eq!(refused(e), ErrorCode::Invalid, "{dest:?}");
    }
    assert_eq!(rig(&c).sacn_target, None);

    set_sacn(&c, Some(sacn_unicast(9))).await.unwrap();
    let (p, _) = until(&rx, "the first packet", |p| p.len() == 638);
    // E1.31 Table 4-1: universe, priority, source name, a live stream.
    assert_eq!(p[113..115], [0, 9]);
    assert_eq!(p[108], 150);
    assert_eq!(&p[44..44 + RIG_NAME.len() + 1], b"Front of house rig\0");
    assert!(!terminated(&p));
    let cid = p[22..38].to_vec();

    // Art-Net beside it: a test frame reaches both.
    let node = socket();
    c.call(Command::ArtnetTarget {
        base: rig(&c).rev,
        target: Some(ArtnetTarget {
            addr: node.local_addr().unwrap(),
            universe: PortAddress::new(0).unwrap(),
        }),
    })
    .await
    .unwrap();
    c.call(Command::ArtnetTest {
        channels: levels("12=255 13=40"),
    })
    .await
    .unwrap();
    let (a, _) = until(&node, "the test on Art-Net", |p| {
        is_art_dmx(p) && p[18 + 11] == 255
    });
    assert_eq!(a[18 + 12], 40);
    let (s, _) = until(&rx, "the test on sACN", |p| slot(p, 12) == 255);
    assert_eq!(slot(&s, 13), 40);
    assert_eq!(&s[22..38], cid.as_slice(), "one source");

    // Sent twice more at once, then again at the keepalive, each packet one further in the
    // sequence: a gap of about 0.9 s comes within a few packets. Only bounds a loaded host
    // cannot break are checked (a keepalive is never early; it may be late).
    let mut prev = s;
    let mut at = Instant::now();
    let mut keepalive = None;
    for _ in 0..6 {
        let (p, _) = until(&rx, "the test frame again", |p| slot(p, 12) == 255);
        let gap = at.elapsed();
        assert_eq!(seq(&p), seq(&prev).wrapping_add(1));
        (prev, at) = (p, Instant::now());
        if gap >= Duration::from_millis(450) {
            keepalive = Some(gap);
            break;
        }
    }
    let gap = keepalive.expect("a keepalive gap");
    assert!(gap < Duration::from_secs(5), "{gap:?}");

    // Off: three packets end the stream, and nothing follows; Art-Net goes on.
    set_sacn(&c, None).await.unwrap();
    let (e, _) = until(&rx, "the end of the stream", terminated);
    let rest = gather(&rx, Duration::from_millis(1500));
    assert_eq!(rest.len(), 2, "three terminations in all");
    assert!(rest.iter().all(|p| terminated(p)));
    assert_eq!(seq(&rest[0]), seq(&e).wrapping_add(1));
    until(&node, "Art-Net still", is_art_dmx);

    // A clean shutdown sends the cues' ok frame, then ends the stream.
    set_sacn(&c, Some(sacn_unicast(9))).await.unwrap();
    until(&rx, "the stream back", |p| !terminated(p));
    drop(c);
    drop(h);
    let tail = gather(&rx, Duration::from_millis(500));
    let n = tail.len();
    assert!(n >= 3, "{n} packets");
    assert!(tail[n - 3..].iter().all(|p| terminated(p)));
    assert!(tail[..n - 3].iter().all(|p| !terminated(p)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sacn_source_is_the_same_after_a_restart() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let rx = socket();
    let mut cids = Vec::new();
    for _ in 0..2 {
        let mut cfg = config(DmxPorts {
            sacn_port: port_of(&rx),
            ..DmxPorts::LOOPBACK
        });
        cfg.rig_id = ac2d::RigIdSource::File(dir.path().join("rig-id"));
        let h = Daemon::start(cfg).unwrap();
        let c = mirror(&h, "ac2d-dmx-net-test").await;
        set_sacn(&c, Some(sacn_unicast(1))).await.unwrap();
        let (p, _) = until(&rx, "a packet", |p| !terminated(p));
        cids.push(p[22..38].to_vec());
        drop(c);
        drop(h);
        gather(&rx, Duration::from_millis(300));
    }
    assert_eq!(cids[0], cids[1]);
}
