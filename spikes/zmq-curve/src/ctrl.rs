//! Daemon ctrl socket: ROUTER with request ids; each request runs on its own worker so a slow
//! handler never blocks other clients.
//!
//! Only the I/O thread touches the ROUTER (libzmq sockets are single-threaded). Workers hand
//! replies back over an inproc PUSH→PULL pair that the I/O thread polls next to the ROUTER;
//! this is the libzmq-native way to wake a poll loop from another thread.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::proto::{
    Cmd, ErrorCode, ErrorReply, PROTO_VERSION, Reply, Request, decode_request, encode_reply,
};
use crate::zmq::{
    Context, CurveKeyPair, Monitor, Result, Socket, SocketType, make_curve_client,
    make_curve_server, poll_in,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Unique suffix for inproc endpoint names within the process.
pub fn unique(name: &str) -> String {
    format!("{name}-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

#[derive(Clone, Debug)]
pub enum ServerSecurity {
    Null,
    Curve {
        keys: CurveKeyPair,
        zap_domain: String,
    },
}

#[derive(Clone, Debug)]
pub enum ClientSecurity {
    Null,
    Curve {
        keys: CurveKeyPair,
        server_public: String,
    },
}

pub fn apply_server(s: &Socket, sec: &ServerSecurity) -> Result<()> {
    match sec {
        ServerSecurity::Null => Ok(()),
        ServerSecurity::Curve { keys, zap_domain } => make_curve_server(s, keys, zap_domain),
    }
}

pub fn apply_client(s: &Socket, sec: &ClientSecurity) -> Result<()> {
    match sec {
        ClientSecurity::Null => Ok(()),
        ClientSecurity::Curve {
            keys,
            server_public,
        } => make_curve_client(s, keys, server_public),
    }
}

/// Test-controlled gates that `Cmd::Slow` handlers wait on (deterministic "slow").
#[derive(Debug, Default)]
pub struct Gates {
    open: Mutex<HashSet<u32>>,
    cv: Condvar,
}

impl Gates {
    pub fn open(&self, gate: u32) {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(gate);
        self.cv.notify_all();
    }

    fn wait(&self, gate: u32) {
        let mut g = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        while !g.contains(&gate) {
            g = self.cv.wait(g).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// What the daemon saw: (ZAP user id, request). Lets tests assert a refused client never
/// reached the application.
pub type SeenLog = Arc<Mutex<Vec<(Option<String>, Request)>>>;

#[derive(Debug)]
pub struct CtrlServer {
    endpoint: String,
    replies_ep: String,
    ctx: Context,
    seen: SeenLog,
    monitor: Monitor,
    thread: Option<JoinHandle<()>>,
}

impl CtrlServer {
    pub fn start(
        ctx: &Context,
        bind: &str,
        sec: &ServerSecurity,
        gates: Arc<Gates>,
    ) -> Result<Self> {
        let router = ctx.socket(SocketType::Router)?;
        apply_server(&router, sec)?;
        // Replies to a vanished client must fail loudly in tests rather than vanish silently.
        router.set_int(crate::ffi::ZMQ_ROUTER_MANDATORY, 1)?;
        // Attached before bind so no connection event is missed; read by the owner of
        // `CtrlServer` (tests use it to observe refused handshakes server-side).
        let monitor = Monitor::attach(ctx, &router, &unique("ctrl"))?;
        router.bind(bind)?;
        let endpoint = router.last_endpoint()?;

        let replies_ep = format!("inproc://{}", unique("ctrl-replies"));
        let replies = ctx.socket(SocketType::Pull)?;
        replies.bind(&replies_ep)?;

        let seen: SeenLog = Arc::default();
        let (ctx2, ep2, seen2) = (ctx.clone(), replies_ep.clone(), seen.clone());
        let thread = std::thread::Builder::new()
            .name("ctrl-io".into())
            .spawn(move || io_loop(&ctx2, &router, &replies, &ep2, &gates, &seen2))
            .map_err(|_| crate::zmq::Error(libc::EAGAIN))?;
        Ok(Self {
            endpoint,
            replies_ep,
            ctx: ctx.clone(),
            seen,
            monitor,
            thread: Some(thread),
        })
    }

    /// Resolved endpoint clients connect to.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Connection events of the ROUTER socket.
    pub fn monitor(&self) -> &Monitor {
        &self.monitor
    }

    pub fn seen(&self) -> Vec<(Option<String>, Request)> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for CtrlServer {
    fn drop(&mut self) {
        // An empty message on the replies pipe is the stop signal.
        if let Ok(push) = self.ctx.socket(SocketType::Push)
            && push.set_int(crate::ffi::ZMQ_LINGER, 1000).is_ok()
            && push.connect(&self.replies_ep).is_ok()
        {
            let _ = push.send_multipart(&[[0u8; 0]], 0);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn io_loop(
    ctx: &Context,
    router: &Socket,
    replies: &Socket,
    replies_ep: &str,
    gates: &Arc<Gates>,
    seen: &SeenLog,
) {
    loop {
        let Ok(ready) = poll_in(&[router, replies], Duration::from_secs(3600)) else {
            return;
        };
        if ready[0] {
            while let Ok(Some(m)) = router.try_recv() {
                let [rid, payload] = m.frames.as_slice() else {
                    continue;
                };
                dispatch(
                    ctx,
                    replies_ep,
                    gates,
                    seen,
                    rid.clone(),
                    payload,
                    m.user_id,
                );
            }
        }
        if ready[1] {
            while let Ok(Some(m)) = replies.try_recv() {
                if m.frames.iter().all(Vec::is_empty) {
                    return;
                }
                // ROUTER_MANDATORY: EHOSTUNREACH when the client is gone; nothing to do.
                let _ = router.send_multipart(&m.frames, 0);
            }
        }
    }
}

fn dispatch(
    ctx: &Context,
    replies_ep: &str,
    gates: &Arc<Gates>,
    seen: &SeenLog,
    rid: Vec<u8>,
    payload: &[u8],
    user_id: Option<String>,
) {
    let req = match decode_request(payload) {
        Ok(r) => r,
        Err(e) => {
            let reply = Reply {
                v: PROTO_VERSION,
                id: 0,
                result: Err(ErrorReply {
                    code: ErrorCode::BadRequest,
                    msg: e,
                }),
            };
            send_reply(ctx, replies_ep, rid, &reply);
            return;
        }
    };
    seen.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((user_id.clone(), req.clone()));
    let (ctx, ep, gates) = (ctx.clone(), replies_ep.to_owned(), gates.clone());
    // A thread per request is spike-sized; ac2d runs handlers as tokio tasks.
    let _ = std::thread::Builder::new()
        .name("ctrl-worker".into())
        .spawn(move || {
            let result = if req.v == PROTO_VERSION {
                Ok(match req.cmd {
                    Cmd::Echo { text } => text,
                    Cmd::Slow { gate } => {
                        gates.wait(gate);
                        format!("slow {gate} done")
                    }
                    Cmd::Whoami => user_id.unwrap_or_default(),
                })
            } else {
                Err(ErrorReply {
                    code: ErrorCode::VersionMismatch,
                    msg: format!("daemon speaks v{PROTO_VERSION}, request is v{}", req.v),
                })
            };
            send_reply(
                &ctx,
                &ep,
                rid,
                &Reply {
                    v: PROTO_VERSION,
                    id: req.id,
                    result,
                },
            );
        });
}

fn send_reply(ctx: &Context, replies_ep: &str, rid: Vec<u8>, reply: &Reply) {
    let Ok(push) = ctx.socket(SocketType::Push) else {
        return;
    };
    // Linger so closing right after the send cannot discard the queued reply.
    if push.set_int(crate::ffi::ZMQ_LINGER, 1000).is_ok() && push.connect(replies_ep).is_ok() {
        let _ = push.send_multipart(&[rid, encode_reply(reply)], 0);
    }
}
