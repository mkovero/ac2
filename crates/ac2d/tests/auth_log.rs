//! Network mode: a refused client is visible in the daemon's log by fingerprint and address,
//! once per key and address while it keeps retrying. Its own test binary because it installs
//! a global log subscriber that captures what the ZAP thread writes.
#![allow(clippy::unwrap_used)]

#[path = "it/common/mod.rs"]
mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac2_zmq::{AuthorizedKeys, Context, CurveClient, KeyPair};
use ac2d::{Daemon, Listen, NetworkSecurity};
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
    keys.insert("laptop", laptop.public).unwrap();
    let authorized = dir.path().join("authorized_clients");
    keys.save(&authorized).unwrap();
    let listen = Listen::Network {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
        security: NetworkSecurity {
            server_key_file: dir.path().join("server.key"),
            authorized_clients_file: authorized.clone(),
        },
    };
    let h = Daemon::start(config(manual_rig(), listen)).unwrap();
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
            h.ctrl_endpoint(),
            Some(&curve(&stranger)),
        ));
        subs.push(Sub::connect_with(
            &ctx,
            h.data_endpoint(),
            &[b""],
            Some(&curve(&stranger)),
            None,
        ));
    }
    let fp = stranger.public.fingerprint();
    let needle = format!("refused client key fingerprint {fp} from 127.0.0.1");
    let deadline = Instant::now() + Duration::from_secs(10);
    while log.lines_with(&needle).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // Let the remaining handshakes (and libzmq's reconnects) reach the handler.
    std::thread::sleep(Duration::from_millis(1500));
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
        h.ctrl_endpoint(),
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
    let _ok = Client::connect_with(&ctx, h.ctrl_endpoint(), Some(&curve(&laptop)));
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
