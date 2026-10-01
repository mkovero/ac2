//! ZAP (ZeroMQ RFC 27) authentication and the [`SecureContext`] that enforces it.
//!
//! libzmq asks the handler bound at `inproc://zeromq.zap.01` *in the same context* to approve
//! every CURVE handshake. The handler sees the client's long-term public key and answers 200
//! (with a user id that libzmq attaches to every message of that connection, see
//! [`crate::Message::user_id`]) or 400 (libzmq sends an ERROR command and drops the
//! connection).
//!
//! **libzmq fails open:** when no handler is bound, it skips ZAP and a CURVE server accepts
//! every client that knows the server public key (`ZMQ_ZAP_ENFORCE_DOMAIN`, which would refuse
//! instead, is draft API and not compiled in). This module makes that state unreachable:
//!
//! - CURVE server sockets can only be created by [`SecureContext::curve_server_socket`]; a
//!   plain [`crate::Context`] or [`crate::Socket`] has no way to enable server-side CURVE.
//! - [`SecureContext::new`] binds the handler before it returns, so it exists before any
//!   CURVE server socket can.
//! - Every CURVE server socket holds a reference to the handler: it keeps running until the
//!   last such socket and the `SecureContext` are dropped.
//! - If the handler stops answering for any other reason (socket error, panic), its socket
//!   stays bound but unanswered, so new handshakes stall and time out instead of failing
//!   open; [`SecureContext::zap_status`] reports the exit, and no further CURVE server socket
//!   can be created. The owner treats that as fatal and closes its network sockets.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::authorized::AuthorizedKeys;
use crate::curve::{KeyPair, PublicKey};
use crate::error::{Error, Result};
use crate::poll::{PollItem, poll};
use crate::raw;
use crate::socket::{Context, Message, Socket, SocketType, unique_inproc};

/// Where libzmq looks for the ZAP handler (RFC 27).
const ZAP_ENDPOINT: &str = "inproc://zeromq.zap.01";

/// Longest ZAP domain libzmq accepts.
const MAX_DOMAIN_LEN: usize = 255;

/// Why the ZAP handler stopped answering.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ZapExit {
    /// The context was terminated under it.
    ContextTerminated,
    /// Receiving a request or sending a reply failed.
    SocketError(String),
    /// The handler loop panicked.
    Panicked,
    /// Injected by a test.
    #[cfg(test)]
    Injected,
}

impl fmt::Display for ZapExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContextTerminated => f.write_str("context terminated"),
            Self::SocketError(e) => write!(f, "socket error: {e}"),
            Self::Panicked => f.write_str("handler panicked"),
            #[cfg(test)]
            Self::Injected => f.write_str("injected by test"),
        }
    }
}

/// The security mechanism a peer asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mechanism {
    /// No security.
    Null,
    /// Username/password in clear text.
    Plain,
    /// CurveZMQ.
    Curve,
    /// Anything else (e.g. GSSAPI).
    Other(String),
}

/// Why a peer was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenyReason {
    /// The ZAP request did not follow RFC 27.
    MalformedRequest,
    /// The socket's ZAP domain is not the one this handler serves.
    WrongDomain,
    /// The peer did not use CURVE.
    NotCurve,
    /// The client key is not in the authorized keys.
    UnknownKey,
    /// Deciding panicked; refused to fail closed.
    Internal,
}

/// The handler's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Accepted; `user_id` (the authorized key's name) is attached to every message.
    Allowed {
        /// Name of the authorized key.
        user_id: String,
    },
    /// Refused with ZAP status 400.
    Denied(DenyReason),
}

/// One handshake decision, for logs and audit ("refused key K from address A").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZapDecision {
    /// ZAP domain of the server socket.
    pub domain: String,
    /// Peer address (IP for tcp; empty for ipc).
    pub address: String,
    /// Mechanism the peer used.
    pub mechanism: Mechanism,
    /// The client's long-term public key (CURVE only).
    pub client_key: Option<PublicKey>,
    /// Accepted or refused, and why.
    pub verdict: Verdict,
}

impl ZapDecision {
    /// Whether the peer was accepted.
    pub fn allowed(&self) -> bool {
        matches!(self.verdict, Verdict::Allowed { .. })
    }
}

/// Called on the ZAP thread for every decision. Keep it quick: handshakes wait for it.
type Audit = Box<dyn Fn(&ZapDecision) + Send>;

/// Handler state shared with its thread.
#[derive(Debug, Default)]
struct ZapState {
    exit: Mutex<Option<ZapExit>>,
    /// After an abnormal exit the REP socket is kept here, bound and unanswered, so libzmq
    /// keeps routing handshakes to it (where they stall) instead of skipping ZAP.
    parked: Mutex<Option<Socket>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Owns the running handler; dropped when the [`SecureContext`] and every CURVE server
/// socket created from it are gone.
pub(crate) struct ZapGuard {
    ctx: Context,
    domain: String,
    stop_endpoint: String,
    state: Arc<ZapState>,
    keys: Arc<RwLock<AuthorizedKeys>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl fmt::Debug for ZapGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZapGuard")
            .field("exit", &*lock(&self.state.exit))
            .finish_non_exhaustive()
    }
}

/// Message on the stop pipe that asks the handler to exit normally.
const STOP: &[u8] = b"stop";
#[cfg(test)]
const FAIL: &[u8] = b"fail";

impl ZapGuard {
    fn signal(&self, what: &[u8]) -> Result<()> {
        let push = self.ctx.socket(SocketType::Push)?;
        // The message is in the inproc pipe once send returns; linger only covers the case
        // where the peer's pipe is not attached yet.
        push.set_linger(Some(Duration::from_secs(1)))?;
        push.connect(&self.stop_endpoint)?;
        push.send(&[what])
    }
}

impl Drop for ZapGuard {
    fn drop(&mut self) {
        let thread = lock(&self.thread).take();
        if let Some(t) = thread {
            // If the stop signal cannot be delivered the thread is left running (detached)
            // rather than joining forever; it keeps the context alive, which is a leak, not a
            // hole.
            if self.signal(STOP).is_ok() {
                let _ = t.join();
            }
        }
        drop(lock(&self.state.parked).take());
    }
}

/// A context whose CURVE server sockets are guaranteed to authenticate through ZAP.
///
/// Created together with its ZAP handler; see the [module docs](crate::zap) for the rules it
/// enforces. Server-side CURVE is not reachable any other way:
///
/// ```compile_fail
/// // A plain context cannot make a CURVE server socket ...
/// let ctx = ac2_zmq::Context::new().unwrap();
/// let keys = ac2_zmq::KeyPair::generate().unwrap();
/// let s = ctx.curve_server_socket(ac2_zmq::SocketType::Router, &keys);
/// ```
///
/// ```compile_fail
/// // ... and a socket has no setter for it.
/// let ctx = ac2_zmq::Context::new().unwrap();
/// let keys = ac2_zmq::KeyPair::generate().unwrap();
/// let s = ctx.socket(ac2_zmq::SocketType::Router).unwrap();
/// s.set_curve_server(&keys);
/// ```
#[derive(Debug)]
pub struct SecureContext {
    ctx: Context,
    zap: Arc<ZapGuard>,
}

impl SecureContext {
    /// A new context with a ZAP handler that accepts CURVE clients whose key is in `keys` on
    /// sockets of `domain`, and refuses everything else. `audit` sees every decision (on the
    /// handler thread; keep it quick).
    pub fn new(
        domain: &str,
        keys: AuthorizedKeys,
        audit: impl Fn(&ZapDecision) + Send + 'static,
    ) -> Result<Self> {
        if domain.is_empty() || domain.len() > MAX_DOMAIN_LEN || domain.contains('\0') {
            return Err(Error::InvalidArgument(
                "ZAP domain must be 1-255 bytes without NUL",
            ));
        }
        let ctx = Context::new()?;
        let rep = ctx.socket(SocketType::Rep)?;
        // Bound here, on the caller's thread, before any CURVE server socket can exist.
        rep.bind(ZAP_ENDPOINT)?;
        let stop_endpoint = unique_inproc("zap-stop");
        let stop = ctx.socket(SocketType::Pull)?;
        stop.bind(&stop_endpoint)?;

        let state = Arc::new(ZapState::default());
        let keys = Arc::new(RwLock::new(keys));
        let handler = Handler {
            domain: domain.to_owned(),
            keys: Arc::clone(&keys),
            audit: Box::new(audit),
        };
        let thread_state = Arc::clone(&state);
        let thread = std::thread::Builder::new()
            .name("zap".into())
            .spawn(move || handler.run(rep, &stop, &thread_state))
            .map_err(|e| Error::Spawn(e.to_string()))?;
        Ok(Self {
            zap: Arc::new(ZapGuard {
                ctx: ctx.clone(),
                domain: domain.to_owned(),
                stop_endpoint,
                state,
                keys,
                thread: Mutex::new(Some(thread)),
            }),
            ctx,
        })
    }

    /// The plain context: for sockets that need no server-side CURVE (inproc pipes, client
    /// sockets). They share this context's inproc namespace.
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// `Ok` while the ZAP handler is answering; otherwise why it stopped. Check it in the
    /// owner's I/O loop: an exited handler is fatal for the network sockets.
    pub fn zap_status(&self) -> Result<()> {
        match &*lock(&self.zap.state.exit) {
            None => Ok(()),
            Some(exit) => Err(Error::ZapHandlerExited(exit.clone())),
        }
    }

    /// A socket of `kind` that runs the CURVE server handshake with `keys` and authenticates
    /// every client through this context's ZAP handler. Refused once the handler has exited.
    /// Bind it after this returns; the socket keeps the handler running for its lifetime.
    pub fn curve_server_socket(&self, kind: SocketType, keys: &KeyPair) -> Result<Socket> {
        self.zap_status()?;
        let mut s = self.ctx.socket(kind)?;
        apply_curve_server(&s, keys, &self.zap.domain)?;
        s.attach_zap(Arc::clone(&self.zap));
        Ok(s)
    }

    /// Replaces the authorized keys. Applies to handshakes from now on; established
    /// connections are not re-checked.
    pub fn set_authorized(&self, keys: AuthorizedKeys) {
        *self
            .zap
            .keys
            .write()
            .unwrap_or_else(PoisonError::into_inner) = keys;
    }

    /// A copy of the current authorized keys.
    pub fn authorized(&self) -> AuthorizedKeys {
        self.zap
            .keys
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Makes the handler exit abnormally and waits until that is visible.
    #[cfg(test)]
    pub(crate) fn inject_handler_failure(&self) -> Result<()> {
        self.zap.signal(FAIL)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while self.zap_status().is_ok() {
            if std::time::Instant::now() > deadline {
                return Err(Error::InvalidArgument("handler did not exit"));
            }
            std::thread::yield_now();
        }
        Ok(())
    }
}

/// Server-side CURVE options. Only reachable through [`SecureContext::curve_server_socket`]
/// (and this module's tests): applied without a running handler they fail open.
fn apply_curve_server(s: &Socket, keys: &KeyPair, domain: &str) -> Result<()> {
    s.set_int(raw::ZMQ_CURVE_SERVER, 1)?;
    s.set_bytes(raw::ZMQ_CURVE_SECRETKEY, keys.secret.as_bytes())?;
    s.set_bytes(raw::ZMQ_ZAP_DOMAIN, domain.as_bytes())
}

struct Handler {
    domain: String,
    keys: Arc<RwLock<AuthorizedKeys>>,
    audit: Audit,
}

impl Handler {
    fn run(self, rep: Socket, stop: &Socket, state: &ZapState) {
        let exit = catch_unwind(AssertUnwindSafe(|| self.serve(&rep, stop)))
            .unwrap_or(Some(ZapExit::Panicked));
        let Some(exit) = exit else {
            // Normal stop: no CURVE server socket is left, so the handler socket may close.
            return;
        };
        let terminated = exit == ZapExit::ContextTerminated;
        *lock(&state.exit) = Some(exit);
        if !terminated {
            *lock(&state.parked) = Some(rep);
        }
    }

    /// Answers requests until asked to stop (`None`) or failing.
    fn serve(&self, rep: &Socket, stop: &Socket) -> Option<ZapExit> {
        let fail = |e: Error| match e {
            Error::ContextTerminated => ZapExit::ContextTerminated,
            e => ZapExit::SocketError(e.to_string()),
        };
        loop {
            let mut items = [PollItem::readable(rep), PollItem::readable(stop)];
            if let Err(e) = poll(&mut items, None) {
                return Some(fail(e));
            }
            if items[1].is_readable() {
                match stop.try_recv() {
                    #[cfg(test)]
                    Ok(Some(m)) if m.frames() == [FAIL] => return Some(ZapExit::Injected),
                    Ok(_) => return None,
                    Err(e) => return Some(fail(e)),
                }
            }
            if items[0].is_readable() {
                let request = match rep.try_recv() {
                    Ok(Some(r)) => r,
                    Ok(None) => continue,
                    Err(e) => return Some(fail(e)),
                };
                let decision = catch_unwind(AssertUnwindSafe(|| self.decide(&request)))
                    .unwrap_or_else(|_| internal_denial(&request));
                // A panicking audit hook must not take authentication down with it.
                let _ = catch_unwind(AssertUnwindSafe(|| (self.audit)(&decision)));
                // REP must answer every request before it can receive the next one.
                if let Err(e) = rep.send(&reply(&request, &decision)) {
                    return Some(fail(e));
                }
            }
        }
    }

    /// Request frames (RFC 27): version, request id, domain, address, routing id, mechanism,
    /// then the mechanism's credentials (CURVE: exactly one, the 32-byte client key).
    fn decide(&self, request: &Message) -> ZapDecision {
        let f = request.frames();
        let mechanism = match f.get(5).map(Vec::as_slice) {
            Some(b"NULL") => Mechanism::Null,
            Some(b"PLAIN") => Mechanism::Plain,
            Some(b"CURVE") => Mechanism::Curve,
            _ => Mechanism::Other(frame_text(request, 5)),
        };
        let client_key = match (&mechanism, f.get(6..)) {
            (Mechanism::Curve, Some([k])) => <[u8; 32]>::try_from(k.as_slice())
                .ok()
                .map(PublicKey::from_bytes),
            _ => None,
        };
        let domain = frame_text(request, 2);
        let verdict = if f.len() < 6 || f[0] != b"1.0" {
            Verdict::Denied(DenyReason::MalformedRequest)
        } else if domain != self.domain {
            Verdict::Denied(DenyReason::WrongDomain)
        } else if mechanism != Mechanism::Curve {
            Verdict::Denied(DenyReason::NotCurve)
        } else if let Some(key) = &client_key {
            let keys = self.keys.read().unwrap_or_else(PoisonError::into_inner);
            match keys.name_of(key) {
                Some(name) => Verdict::Allowed {
                    user_id: name.to_owned(),
                },
                None => Verdict::Denied(DenyReason::UnknownKey),
            }
        } else {
            Verdict::Denied(DenyReason::MalformedRequest)
        };
        ZapDecision {
            domain,
            address: frame_text(request, 3),
            mechanism,
            client_key,
            verdict,
        }
    }
}

fn frame_text(m: &Message, i: usize) -> String {
    String::from_utf8_lossy(m.frames().get(i).map_or(&[][..], Vec::as_slice)).into_owned()
}

fn internal_denial(request: &Message) -> ZapDecision {
    ZapDecision {
        domain: frame_text(request, 2),
        address: frame_text(request, 3),
        mechanism: Mechanism::Other(frame_text(request, 5)),
        client_key: None,
        verdict: Verdict::Denied(DenyReason::Internal),
    }
}

/// Reply frames (RFC 27): version, request id, status code, status text, user id, metadata.
fn reply(request: &Message, decision: &ZapDecision) -> [Vec<u8>; 6] {
    let request_id = request.frames().get(1).cloned().unwrap_or_default();
    let (code, text, user_id): (&[u8], &[u8], &[u8]) = match &decision.verdict {
        Verdict::Allowed { user_id } => (b"200", b"OK", user_id.as_bytes()),
        Verdict::Denied(_) => (b"400", b"not authorized", b""),
    };
    [
        b"1.0".to_vec(),
        request_id,
        code.to_vec(),
        text.to_vec(),
        user_id.to_vec(),
        Vec::new(),
    ]
}

#[cfg(test)]
mod tests;
