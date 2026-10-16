//! Art-Net cues end to end on the fake rig: a UDP receiver on the loopback interface is the
//! rig's Art-Net node. The target is rev-guarded, a test frame arrives, the cue frame (HTP
//! over the cues' current levels) is sent at once on a change and again every second, and a
//! clean shutdown sends every cue's `ok` levels.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig, ClientError, Endpoints};
use ac2_proto::model::{
    ArtnetTarget, Cue, CueAction, LeqConfig, LeqWindow, MeasConfig, MeasKind, PeakWeighting,
    PortAddress, SplConfig, TimeWeighting, Weighting, dmx_list_text, parse_dmx_list,
};
use ac2_proto::units::{Db, DbSpl, MeasId, Rev, Seconds};
use ac2_proto::{Command, ErrorCode, ErrorDetail, ReplyBody};
use ac2d::Daemon;
use common::*;

const WAIT: Duration = Duration::from_secs(20);

async fn connect(h: &ac2d::Handle) -> Client {
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep, "ac2d-artnet-test"))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
    c
}

/// One received ArtDmx packet.
struct Frame {
    sub_uni: u8,
    net: u8,
    data: Vec<u8>,
    at: Instant,
}

impl Frame {
    /// Channel `n` (1–512).
    fn ch(&self, n: usize) -> u8 {
        self.data[n - 1]
    }
}

/// The node: a socket on the loopback interface.
struct Node(UdpSocket);

impl Node {
    fn new() -> Self {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        Self(s)
    }

    fn addr(&self) -> SocketAddr {
        self.0.local_addr().unwrap()
    }

    /// The next frame `want` accepts (others are skipped).
    fn until(&self, what: &str, want: impl Fn(&Frame) -> bool) -> Frame {
        let deadline = Instant::now() + WAIT;
        let mut buf = [0u8; 1024];
        loop {
            assert!(
                Instant::now() < deadline,
                "no frame with {what} within {WAIT:?}"
            );
            let Ok(n) = self.0.recv(&mut buf) else {
                continue;
            };
            let p = &buf[..n];
            assert_eq!(n, 530, "a whole universe");
            assert_eq!(&p[..8], b"Art-Net\0");
            assert_eq!(p[8..10], [0x00, 0x50], "OpCode ArtDmx");
            assert_eq!(p[10..12], [0, 14], "ProtVer 14");
            assert_eq!(p[12], 0, "Sequence disabled");
            assert_eq!(p[16..18], [0x02, 0x00], "Length 512");
            let f = Frame {
                sub_uni: p[14],
                net: p[15],
                data: p[18..].to_vec(),
                at: Instant::now(),
            };
            if want(&f) {
                return f;
            }
        }
    }
}

fn levels(text: &str) -> Vec<ac2_proto::model::DmxSet> {
    parse_dmx_list(text).unwrap()
}

/// A cue holding `ok`/`near`/`over`/`fault` levels, no OSC.
fn cue(ok: &str, near: &str, over: &str, fault: &str) -> Option<Box<Cue>> {
    let a = |t: &str| {
        (!t.is_empty()).then(|| CueAction {
            osc: None,
            dmx: levels(t),
        })
    };
    Some(Box::new(Cue {
        ok: a(ok),
        near: a(near),
        over: a(over),
        fault: a(fault),
    }))
}

fn meter(name: &str, limit: Option<f64>, c: Option<Box<Cue>>) -> MeasConfig {
    MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name: name.into(),
        kind: MeasKind::Spl {
            config: SplConfig {
                cue: None,
                bands: None,
                input: 1,
                weighting: Weighting::A,
                time_weighting: TimeWeighting::Fast,
                peak_weighting: PeakWeighting::C,
                leq: LeqConfig {
                    windows: vec![LeqWindow {
                        cue: c,
                        duration: Seconds(1.0),
                        weighting: Weighting::A,
                        limit: limit.map(DbSpl),
                        warn_margin: Db(3.0),
                    }],
                    horizon: Seconds(2.0),
                    peaks: Default::default(),
                },
                position: None,
            },
        },
    }
}

async fn create(c: &Client, config: MeasConfig) -> MeasId {
    match c.call(Command::MeasCreate { config }).await.unwrap() {
        ReplyBody::Measurement(m) => m.id,
        other => panic!("{other:?}"),
    }
}

fn rig_rev(c: &Client) -> Rev {
    c.view().state.clone().unwrap().rig.rev
}

/// Waits until the rig entity's lit channels read `want`.
async fn rig_dmx_until(c: &Client, want: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let got = dmx_list_text(&c.view().state.clone().unwrap().rig.dmx);
        if got == want {
            return;
        }
        assert!(Instant::now() < deadline, "rig.dmx {got:?}, never {want:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_universe_follows_the_cues_and_keeps_alive() {
    init_log();
    let h = Daemon::start(steady_config(realtime_rig(), local_tcp())).unwrap();
    let c = connect(&h).await;
    let node = Node::new();
    let target = ArtnetTarget {
        addr: node.addr(),
        universe: PortAddress::from_parts(1, 2, 3).unwrap(),
    };

    // Set on the rig's rev: the current frame (no cues: all zero) arrives at once, on the
    // universe asked for. A stale rev is a conflict naming the current one.
    let stale = rig_rev(&c);
    c.call(Command::ArtnetTarget {
        base: stale,
        target: Some(target),
    })
    .await
    .unwrap();
    let f = node.until("the first frame", |_| true);
    assert_eq!((f.net, f.sub_uni), (1, 0x23));
    assert!(f.data.iter().all(|v| *v == 0));
    let e = c
        .call(Command::ArtnetTarget {
            base: stale,
            target: None,
        })
        .await
        .unwrap_err();
    match e {
        ClientError::Daemon(p) => {
            assert_eq!(p.code, ErrorCode::Conflict);
            assert_eq!(p.detail, Some(ErrorDetail::Conflict { rev: rig_rev(&c) }));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        c.view().state.clone().unwrap().rig.artnet_target,
        Some(target)
    );

    // A test frame arrives as given, every other channel 0.
    c.call(Command::ArtnetTest {
        channels: levels("12=255 13=40"),
    })
    .await
    .unwrap();
    let f = node.until("the test levels", |f| f.ch(12) == 255);
    assert_eq!(f.ch(13), 40);
    assert_eq!(f.data.iter().filter(|v| **v > 0).count(), 2);

    // A meter with no session cannot judge: its fault levels go out at once, replacing the
    // test frame. A second meter's fault levels merge HTP.
    create(
        &c,
        meter("A", Some(90.0), cue("21=200", "", "", "20=255 30=10")),
    )
    .await;
    let f = node.until("A's fault", |f| f.ch(20) == 255);
    assert_eq!((f.ch(12), f.ch(13), f.ch(21), f.ch(30)), (0, 0, 0, 10));
    let b = create(&c, meter("B", None, cue("22=255", "", "", "30=99"))).await;
    node.until("A and B's fault, HTP", |f| {
        f.ch(20) == 255 && f.ch(30) == 99
    });
    rig_dmx_until(&c, "20=255 30=99").await;

    // Sent again every second while nothing changes.
    let f0 = node.until("a keepalive", |f| f.ch(30) == 99);
    let f1 = node.until("the next keepalive", |f| f.ch(30) == 99);
    let gap = f1.at - f0.at;
    assert!(
        gap > Duration::from_millis(500) && gap < Duration::from_millis(1600),
        "{gap:?}"
    );
    assert_eq!(f0.data, f1.data);

    // B runs without a limit: ok once its first second is judged, at once on the wire.
    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    c.call(Command::MeasStart { meas: b }).await.unwrap();
    let f = node.until("B's ok", |f| f.ch(22) == 255);
    assert_eq!(
        (f.ch(20), f.ch(30)),
        (255, 10),
        "A still fault, alone on 30"
    );
    rig_dmx_until(&c, "20=255 22=255 30=10").await;
    assert!(c.view().state.clone().unwrap().rig.artnet.sent > 0);

    // A clean shutdown leaves every cue's ok levels.
    drop(h);
    let f = node.until("every ok", |f| f.ch(21) == 200);
    assert_eq!((f.ch(20), f.ch(22), f.ch(30)), (0, 255, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn targets_and_tests_are_checked() {
    init_log();
    let h = Daemon::start(steady_config(realtime_rig(), local_tcp())).unwrap();
    let c = connect(&h).await;
    let refused = |e: ClientError| match e {
        ClientError::Daemon(p) => p.code,
        other => panic!("{other:?}"),
    };
    // No target: a test has nowhere to go.
    let e = c
        .call(Command::ArtnetTest {
            channels: levels("1=1"),
        })
        .await
        .unwrap_err();
    assert_eq!(refused(e), ErrorCode::Invalid);
    for addr in ["0.0.0.0:6454", "127.0.0.1:0", "[::1]:6454"] {
        let e = c
            .call(Command::ArtnetTarget {
                base: rig_rev(&c),
                target: Some(ArtnetTarget {
                    addr: addr.parse().unwrap(),
                    universe: PortAddress::new(0).unwrap(),
                }),
            })
            .await
            .unwrap_err();
        assert_eq!(refused(e), ErrorCode::Invalid, "{addr}");
    }
    let node = Node::new();
    c.call(Command::ArtnetTarget {
        base: rig_rev(&c),
        target: Some(ArtnetTarget {
            addr: node.addr(),
            universe: PortAddress::new(0).unwrap(),
        }),
    })
    .await
    .unwrap();
    let dup = vec![levels("5=1")[0], levels("5=2")[0]];
    let e = c
        .call(Command::ArtnetTest { channels: dup })
        .await
        .unwrap_err();
    assert_eq!(refused(e), ErrorCode::Invalid);
}
