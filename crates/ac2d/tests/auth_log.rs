//! Network mode: a refused client is visible in the daemon's log by fingerprint and address,
//! once per key and address while it keeps retrying. Its own test binary because it installs
//! a global log subscriber that captures what the ZAP thread writes.
#![allow(clippy::unwrap_used)]

#[path = "it/common/mod.rs"]
mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac2_proto::{Command, ReplyBody};
use ac2_zmq::{AuthorizedKeys, Context, CurveClient, KeyPair};
use ac2d::{Daemon, NetworkSecurity};
use common::*;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn lines_with(&self, needle: &str) -> Vec<String> {
        String::from_utf8_lossy(&self.0.lock().unwrap())
            .lines()
            .filter(|l| l.contains(needle))
            .map(str::to_owned)
            .collect()
    }
}

#[test]
fn refused_key_is_logged_once_per_key_and_address() {
    let log = Captured::default();
    let writer = log.clone();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::new("ac2d::auth=info"))
        .with_writer(move || writer.clone())
        .init();

    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let stranger = KeyPair::generate().unwrap();
    let other = KeyPair::generate().unwrap();
    let mut keys = AuthorizedKeys::new();
    keys.insert("laptop", laptop.public, ac2_zmq::Role::Operator)
        .unwrap();
    let authorized = dir.path().join("authorized_clients");
    keys.save(&authorized).unwrap();
    let security = NetworkSecurity {
        server_key_file: dir.path().join("server.key"),
        authorized_clients_file: authorized.clone(),
    };
    let h = Daemon::start(net_config(manual_rig(), security)).unwrap();
    let server_key = h.server_public_key().unwrap();
    let ctx = Context::new().unwrap();
    let curve = |keys: &KeyPair| CurveClient {
        keys: keys.clone(),
        server_key,
    };

    // The stranger knocks on both sockets, several times (as a retrying client does).
    let mut socks = Vec::new();
    let mut subs = Vec::new();
    for _ in 0..3 {
        socks.push(Client::connect_with(
            &ctx,
            net_endpoints(&h).0.as_str(),
            Some(&curve(&stranger)),
        ));
        subs.push(Sub::knock(
            &ctx,
            net_endpoints(&h).1.as_str(),
            &[b""],
            Some(&curve(&stranger)),
        ));
    }
    let fp = stranger.public.fingerprint();
    let needle = format!("refused client key fingerprint {fp} from 127.0.0.1");
    let deadline = Instant::now() + Duration::from_secs(10);
    while log.lines_with(&needle).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // The remaining handshakes reached the handler: the daemon counts all six refusals
    // (`server.info`, asked on the rig's own host: a network client is listed none).
    let mut on_host = Client::connect(&ctx, h.ctrl_endpoint());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ReplyBody::Server(info) = on_host.ok(Command::ServerInfo) else {
            panic!("server.info: not a server reply");
        };
        let n = info
            .network
            .refused
            .iter()
            .flatten()
            .find(|r| r.fingerprint.as_deref() == Some(fp.as_str()))
            .map_or(0, |r| r.count);
        if n >= 6 {
            break;
        }
        assert!(Instant::now() < deadline, "{n} refusals counted");
        std::thread::sleep(Duration::from_millis(20));
    }
    let lines = log.lines_with(&needle);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains("WARN"), "{}", lines[0]);
    assert!(
        lines[0].contains(&authorized.display().to_string())
            && lines[0].contains(&stranger.public.to_z85()),
        "{}",
        lines[0]
    );

    // Another key from the same address is its own entry.
    socks.push(Client::connect_with(
        &ctx,
        net_endpoints(&h).0.as_str(),
        Some(&curve(&other)),
    ));
    let other_needle = format!(
        "refused client key fingerprint {} from 127.0.0.1",
        other.public.fingerprint()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while log.lines_with(&other_needle).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(log.lines_with(&other_needle).len(), 1);

    // An accepted client is named with its fingerprint.
    let _ok = Client::connect_with(&ctx, net_endpoints(&h).0.as_str(), Some(&curve(&laptop)));
    let accepted = format!(
        "accepted client \"laptop\" (fingerprint {}) from 127.0.0.1",
        laptop.public.fingerprint()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while log.lines_with(&accepted).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!log.lines_with(&accepted).is_empty());
    drop((socks, subs));
    h.shutdown();
}
