//! Network mode: CURVE on both sockets; an authorized client works, an unauthorized one is
//! refused on ctrl and on data.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use ac2_proto::{Command, ReplyBody};
use ac2_zmq::{AuthorizedKeys, Context, CurveClient, KeyPair};
use ac2d::{Daemon, Listen, NetworkSecurity};
use common::*;

#[test]
fn curve_authorized_ok_unauthorized_refused_on_both_sockets() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let stranger = KeyPair::generate().unwrap();
    let mut keys = AuthorizedKeys::new();
    keys.insert("laptop", laptop.public).unwrap();
    let authorized = dir.path().join("authorized_clients");
    keys.save(&authorized).unwrap();
    let security = NetworkSecurity {
        server_key_file: dir.path().join("server.key"),
        authorized_clients_file: authorized,
    };
    let listen = Listen::Network {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
        security: security.clone(),
    };
    let h = Daemon::start(config(manual_rig(), listen)).unwrap();
    let server_key = h
        .server_public_key()
        .expect("network mode has a server key");
    assert!(
        security.server_key_file.exists(),
        "key pair generated on first run"
    );

    let ctx = Context::new().unwrap();
    let ok = CurveClient {
        keys: laptop,
        server_key,
    };
    let bad = CurveClient {
        keys: stranger,
        server_key,
    };

    // Authorized: ctrl answers with the ZAP identity, data delivers keepalives.
    let mut c = Client::connect_with(&ctx, h.ctrl_endpoint(), Some(&ok));
    match c.ok(Command::Hello {
        client: "laptop".into(),
    }) {
        ReplyBody::Welcome(w) => assert_eq!(w.client_id.0, "laptop"),
        other => panic!("{other:?}"),
    }
    let sub = Sub::connect_with(&ctx, h.data_endpoint(), &[b"ka"], Some(&ok), None);
    assert!(sub.ka(Duration::from_secs(5)).is_some());

    // Unauthorized: nothing comes back on either socket.
    let intruder = Client::connect_with(&ctx, h.ctrl_endpoint(), Some(&bad));
    let req = ac2_proto::Request::new(
        ac2_proto::units::RequestId(1),
        Command::Hello {
            client: "stranger".into(),
        },
    );
    intruder.send(&req);
    let sub_bad = Sub::connect_with(&ctx, h.data_endpoint(), &[b""], Some(&bad), None);
    // A plain (non-CURVE) client is refused too.
    let plain = Sub::connect(&ctx, h.data_endpoint(), &[b""]);
    assert!(
        intruder.recv(Duration::from_millis(1500)).is_none(),
        "ctrl refused"
    );
    assert!(
        sub_bad.next(Duration::from_millis(500)).is_none(),
        "data refused"
    );
    assert!(
        plain.next(Duration::from_millis(300)).is_none(),
        "plain refused"
    );
    // The authorized client is unaffected.
    assert!(matches!(
        c.ok(Command::SessionStatus),
        ReplyBody::Session(_)
    ));
    h.shutdown();

    // A restart reuses the stored key pair.
    let listen = Listen::Network {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
        security,
    };
    let h2 = Daemon::start(config(manual_rig(), listen)).unwrap();
    assert_eq!(h2.server_public_key(), Some(server_key));
    h2.shutdown();
}
