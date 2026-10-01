//! ZAP (RFC 27) handler with an authorized-clients list.
//!
//! libzmq asks the handler bound at `inproc://zeromq.zap.01` *in the same context* to approve
//! every CURVE handshake on a socket that has a ZAP domain. The handler sees the client's
//! long-term public key and answers 200 (with a user id that libzmq attaches to every message
//! from that connection) or 400 (libzmq sends an ERROR command and drops the connection).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::zmq::{Context, Result, SocketType, z85_encode_key};

pub const ZAP_ENDPOINT: &str = "inproc://zeromq.zap.01";

/// Client public key (raw 32 bytes) → user id reported to the application.
pub type AuthorizedClients = HashMap<[u8; 32], String>;

/// One decision the handler took, kept for tests and the audit trail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZapDecision {
    pub domain: String,
    pub mechanism: String,
    /// Z85 client key, empty for non-CURVE mechanisms.
    pub client_key: String,
    pub allowed: bool,
}

/// Runs the ZAP handler on its own thread until dropped.
#[derive(Debug)]
pub struct ZapHandler {
    stop: Arc<AtomicBool>,
    decisions: Arc<Mutex<Vec<ZapDecision>>>,
    thread: Option<JoinHandle<()>>,
}

impl ZapHandler {
    /// Bind the handler before any CURVE server socket accepts connections: libzmq skips ZAP
    /// when no handler is bound, and a CURVE server then accepts any client that knows the
    /// server public key (see the `without_zap_handler_*` test).
    pub fn start(ctx: &Context, domain: &str, authorized: AuthorizedClients) -> Result<Self> {
        let rep = ctx.socket(SocketType::Rep)?;
        rep.bind(ZAP_ENDPOINT)?;
        let stop = Arc::new(AtomicBool::new(false));
        let decisions = Arc::new(Mutex::new(Vec::new()));
        let (stop2, decisions2, domain) = (stop.clone(), decisions.clone(), domain.to_owned());
        let thread = std::thread::Builder::new()
            .name("zap".into())
            .spawn(move || {
                while !stop2.load(Ordering::Acquire) {
                    // The timeout only bounds shutdown latency.
                    let Ok(Some(req)) = rep.recv_timeout(Duration::from_millis(20)) else {
                        continue;
                    };
                    let (reply, decision) = answer(&req.frames, &domain, &authorized);
                    decisions2
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(decision);
                    // A REP socket must answer before it can receive again.
                    let _ = rep.send_multipart(&reply, 0);
                }
            })
            .map_err(|_| crate::zmq::Error(libc::EAGAIN))?;
        Ok(Self {
            stop,
            decisions,
            thread: Some(thread),
        })
    }

    pub fn decisions(&self) -> Vec<ZapDecision> {
        self.decisions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for ZapHandler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Request: version, request id, domain, address, routing id, mechanism, credentials...
/// Reply: version, request id, status code, status text, user id, metadata.
fn answer(
    frames: &[Vec<u8>],
    domain: &str,
    authorized: &AuthorizedClients,
) -> (Vec<Vec<u8>>, ZapDecision) {
    let text = |i: usize| {
        frames
            .get(i)
            .map(|f| String::from_utf8_lossy(f).into_owned())
    };
    let request_id = frames.get(1).cloned().unwrap_or_default();
    let req_domain = text(2).unwrap_or_default();
    let mechanism = text(5).unwrap_or_default();
    let key: Option<[u8; 32]> = frames.get(6).and_then(|k| k.as_slice().try_into().ok());

    let well_formed = frames.len() >= 6 && frames[0] == b"1.0";
    let user = match (well_formed, mechanism.as_str(), key) {
        (true, "CURVE", Some(k)) if req_domain == domain => authorized.get(&k).cloned(),
        _ => None,
    };
    let decision = ZapDecision {
        domain: req_domain,
        mechanism,
        client_key: key.map(|k| z85_encode_key(&k)).unwrap_or_default(),
        allowed: user.is_some(),
    };
    let (code, status, user_id): (&[u8], &[u8], Vec<u8>) = match user {
        Some(u) => (b"200", b"OK", u.into_bytes()),
        None => (b"400", b"client not authorized", Vec::new()),
    };
    let reply = vec![
        b"1.0".to_vec(),
        request_id,
        code.to_vec(),
        status.to_vec(),
        user_id,
        Vec::new(),
    ];
    (reply, decision)
}
