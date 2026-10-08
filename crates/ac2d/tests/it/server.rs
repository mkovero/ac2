//! Server features over the protocol (`server.*`): a network-mode daemon lists its
//! authorized clients and the keys it refused, authorizes a refused key so it connects, and
//! revokes one so its requests are refused at once; a local daemon has no client keys.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_proto::model::{ServerInfo, ServerMode};
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2_zmq::{AuthorizedKeys, Context, CurveClient, KeyPair};
use ac2d::{Daemon, Listen, NetworkSecurity};
use common::*;

fn info(c: &mut Client) -> ServerInfo {
    match c.ok(Command::ServerInfo) {
        ReplyBody::Server(s) => s,
        other => panic!("{other:?}"),
    }
}

fn names(s: &ServerInfo) -> Vec<String> {
    match &s.mode {
        ServerMode::Network { authorized, .. } => {
            authorized.iter().map(|a| a.name.clone()).collect()
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn authorize_a_refused_key_and_revoke_it_over_the_protocol() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let tablet = KeyPair::generate().unwrap();
    let mut keys = AuthorizedKeys::new();
    keys.insert("laptop", laptop.public).unwrap();
    let file = dir.path().join("authorized_clients");
    keys.save(&file).unwrap();
    let listen = Listen::Network {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
        security: NetworkSecurity {
            server_key_file: dir.path().join("server.key"),
            authorized_clients_file: file.clone(),
        },
    };
    let h = Daemon::start(config(manual_rig(), listen)).unwrap();
    let server_key = h.server_public_key().unwrap();
    let ctx = Context::new().unwrap();
    let as_laptop = CurveClient {
        keys: laptop,
        server_key,
    };
    let as_tablet = CurveClient {
        keys: tablet.clone(),
        server_key,
    };
    let mut c = Client::connect_with(&ctx, h.ctrl_endpoint(), Some(&as_laptop));

    let s = info(&mut c);
    let ServerMode::Network {
        server_key: key,
        fingerprint,
        refused,
        ..
    } = &s.mode
    else {
        panic!("{s:?}")
    };
    assert_eq!(key, &server_key.to_z85());
    assert_eq!(fingerprint, &server_key.fingerprint());
    assert!(refused.is_empty());
    assert_eq!(names(&s), vec!["laptop"]);

    // The tablet knocks and is refused; its key shows in the refused list.
    let knock = Client::connect_with(&ctx, h.ctrl_endpoint(), Some(&as_tablet));
    knock.send(&ac2_proto::Request::new(
        ac2_proto::units::RequestId(1),
        Command::Hello {
            client: "tablet".into(),
        },
    ));
    assert!(knock.recv(Duration::from_millis(500)).is_none());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let s = info(&mut c);
        let ServerMode::Network { refused, .. } = &s.mode else {
            unreachable!()
        };
        if let Some(r) = refused
            .iter()
            .find(|r| r.key.as_deref() == Some(tablet.public.to_z85().as_str()))
        {
            assert_eq!(r.fingerprint, Some(tablet.public.fingerprint()));
            assert!(r.count >= 1);
            break;
        }
        assert!(Instant::now() < deadline, "refusal not listed: {s:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(knock);

    // Bad input is refused without touching the file.
    let e = c
        .call(Command::ServerAuthorize {
            name: "tablet".into(),
            key: "not a key".into(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);

    // Authorized from the laptop: the tablet connects, and its refusals are forgotten.
    match c.ok(Command::ServerAuthorize {
        name: "tablet".into(),
        key: tablet.public.to_z85(),
    }) {
        ReplyBody::Server(s) => {
            assert_eq!(names(&s), vec!["laptop", "tablet"]);
            let ServerMode::Network { refused, .. } = &s.mode else {
                unreachable!()
            };
            assert!(
                refused
                    .iter()
                    .all(|r| r.key.as_deref() != Some(&*tablet.public.to_z85()))
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(std::fs::read_to_string(&file).unwrap().contains("tablet"));
    let mut t = Client::connect_with(&ctx, h.ctrl_endpoint(), Some(&as_tablet));
    match t.ok(Command::Hello {
        client: "tablet".into(),
    }) {
        ReplyBody::Welcome(w) => assert_eq!(w.client_id.0, "tablet"),
        other => panic!("{other:?}"),
    }

    // Nobody revokes their own key; an unknown name is not found.
    let e = c
        .call(Command::ServerRevoke {
            name: "laptop".into(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    let e = c
        .call(Command::ServerRevoke {
            name: "phone".into(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);

    // Revoked from the laptop: the tablet's connection is refused from the next request on.
    match c.ok(Command::ServerRevoke {
        name: "tablet".into(),
    }) {
        ReplyBody::Server(s) => assert_eq!(names(&s), vec!["laptop"]),
        other => panic!("{other:?}"),
    }
    let e = t.call(Command::SessionStatus).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    assert!(!std::fs::read_to_string(&file).unwrap().contains("tablet"));
    // A restart reads the file as left.
    h.shutdown();
}

#[test]
fn a_local_daemon_has_no_client_keys() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let h = Daemon::start(config(manual_rig(), local(dir.path()))).unwrap();
    let ctx = Context::new().unwrap();
    let mut c = Client::connect(&ctx, h.ctrl_endpoint());
    let s = info(&mut c);
    assert!(matches!(s.mode, ServerMode::Local { .. }), "{s:?}");
    let e = c
        .call(Command::ServerAuthorize {
            name: "x".into(),
            key: KeyPair::generate().unwrap().public.to_z85(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    h.shutdown();

    let h = Daemon::start(config(manual_rig(), inproc("server-info"))).unwrap();
    let (mut c, _) = connect(&h, &[b"evt"]);
    assert!(matches!(info(&mut c).mode, ServerMode::Embedded));
    h.shutdown();
}
