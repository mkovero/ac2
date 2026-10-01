//! Minimal safe wrapper over the libzmq C API.
//!
//! libzmq sockets are not thread-safe: a `Socket` is `Send` (it may be handed to another
//! thread, libzmq issues a full barrier on migration) but not `Sync`. The context is
//! thread-safe and outlives every socket created from it.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ffi;

/// A libzmq error (errno value).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Error(pub c_int);

impl Error {
    fn last() -> Self {
        // SAFETY: zmq_errno reads thread-local errno.
        Self(unsafe { ffi::zmq_errno() })
    }

    pub fn is_again(self) -> bool {
        self.0 == libc::EAGAIN
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: zmq_strerror returns a static NUL-terminated string for any errno.
        let s = unsafe { CStr::from_ptr(ffi::zmq_strerror(self.0)) };
        write!(f, "{} (errno {})", s.to_string_lossy(), self.0)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn check(rc: c_int) -> Result<c_int> {
    if rc < 0 { Err(Error::last()) } else { Ok(rc) }
}

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error(libc::EINVAL))
}

/// libzmq version triple of the linked library.
pub fn version() -> (i32, i32, i32) {
    let (mut a, mut b, mut c) = (0, 0, 0);
    // SAFETY: out-pointers are valid.
    unsafe { ffi::zmq_version(&mut a, &mut b, &mut c) };
    (a, b, c)
}

/// Whether the linked libzmq was built with a capability (`"curve"`, `"ipc"`, ...).
pub fn has(capability: &str) -> bool {
    let Ok(c) = CString::new(capability) else {
        return false;
    };
    // SAFETY: valid C string.
    unsafe { ffi::zmq_has(c.as_ptr()) == 1 }
}

#[derive(Debug)]
struct CtxInner(*mut c_void);

// SAFETY: a libzmq context is thread-safe.
unsafe impl Send for CtxInner {}
// SAFETY: a libzmq context is thread-safe.
unsafe impl Sync for CtxInner {}

impl Drop for CtxInner {
    fn drop(&mut self) {
        // Every Socket holds an Arc to this, so all sockets are closed by now and term
        // returns once their linger expires.
        // SAFETY: pointer came from zmq_ctx_new and is terminated exactly once.
        while unsafe { ffi::zmq_ctx_term(self.0) } != 0 {
            if Error::last().0 != libc::EINTR {
                break;
            }
        }
    }
}

/// A libzmq context. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Context(Arc<CtxInner>);

impl Context {
    pub fn new() -> Result<Self> {
        // SAFETY: plain constructor.
        let raw = unsafe { ffi::zmq_ctx_new() };
        if raw.is_null() {
            return Err(Error::last());
        }
        Ok(Self(Arc::new(CtxInner(raw))))
    }

    pub fn socket(&self, kind: SocketType) -> Result<Socket> {
        // SAFETY: context pointer is live (held by Arc).
        let raw = unsafe { ffi::zmq_socket(self.0.0, kind as c_int) };
        if raw.is_null() {
            return Err(Error::last());
        }
        let s = Socket {
            raw,
            _ctx: self.0.clone(),
        };
        // Never block context teardown on undeliverable messages.
        s.set_int(ffi::ZMQ_LINGER, 0)?;
        Ok(s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum SocketType {
    Pair = ffi::ZMQ_PAIR,
    Pub = ffi::ZMQ_PUB,
    Sub = ffi::ZMQ_SUB,
    Rep = ffi::ZMQ_REP,
    Dealer = ffi::ZMQ_DEALER,
    Router = ffi::ZMQ_ROUTER,
    Pull = ffi::ZMQ_PULL,
    Push = ffi::ZMQ_PUSH,
    XPub = ffi::ZMQ_XPUB,
}

/// One received multipart message plus the ZAP user id the server attached to the connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Received {
    pub frames: Vec<Vec<u8>>,
    /// `User-Id` metadata (set by the ZAP handler on the server side; `None` without ZAP).
    pub user_id: Option<String>,
}

#[derive(Debug)]
pub struct Socket {
    raw: *mut c_void,
    _ctx: Arc<CtxInner>,
}

// SAFETY: libzmq allows migrating a socket between threads as long as it is used by one
// thread at a time; `Socket` is not `Sync`, so `&Socket` cannot be shared across threads.
unsafe impl Send for Socket {}

impl Drop for Socket {
    fn drop(&mut self) {
        // SAFETY: closed exactly once.
        unsafe { ffi::zmq_close(self.raw) };
    }
}

impl Socket {
    pub fn set_int(&self, opt: c_int, v: c_int) -> Result<()> {
        // SAFETY: value pointer/len describe a live c_int.
        check(unsafe {
            ffi::zmq_setsockopt(self.raw, opt, (&raw const v).cast(), size_of::<c_int>())
        })
        .map(drop)
    }

    pub fn set_bytes(&self, opt: c_int, v: &[u8]) -> Result<()> {
        // SAFETY: value pointer/len describe a live slice.
        check(unsafe { ffi::zmq_setsockopt(self.raw, opt, v.as_ptr().cast(), v.len()) }).map(drop)
    }

    pub fn get_int(&self, opt: c_int) -> Result<c_int> {
        let mut v: c_int = 0;
        let mut len = size_of::<c_int>();
        // SAFETY: out-pointer/len describe a live c_int.
        check(unsafe { ffi::zmq_getsockopt(self.raw, opt, (&raw mut v).cast(), &mut len) })?;
        Ok(v)
    }

    fn get_string(&self, opt: c_int) -> Result<String> {
        let mut buf = [0u8; 512];
        let mut len = buf.len();
        // SAFETY: out-pointer/len describe a live buffer.
        check(unsafe { ffi::zmq_getsockopt(self.raw, opt, buf.as_mut_ptr().cast(), &mut len) })?;
        let s = CStr::from_bytes_until_nul(&buf[..len]).map_err(|_| Error(libc::EINVAL))?;
        Ok(s.to_string_lossy().into_owned())
    }

    pub fn bind(&self, endpoint: &str) -> Result<()> {
        let c = cstring(endpoint)?;
        // SAFETY: valid C string.
        check(unsafe { ffi::zmq_bind(self.raw, c.as_ptr()) }).map(drop)
    }

    pub fn connect(&self, endpoint: &str) -> Result<()> {
        let c = cstring(endpoint)?;
        // SAFETY: valid C string.
        check(unsafe { ffi::zmq_connect(self.raw, c.as_ptr()) }).map(drop)
    }

    /// Resolved endpoint after `bind` (e.g. the port picked for `tcp://127.0.0.1:*`).
    pub fn last_endpoint(&self) -> Result<String> {
        self.get_string(ffi::ZMQ_LAST_ENDPOINT)
    }

    pub fn subscribe(&self, prefix: &[u8]) -> Result<()> {
        self.set_bytes(ffi::ZMQ_SUBSCRIBE, prefix)
    }

    pub fn unsubscribe(&self, prefix: &[u8]) -> Result<()> {
        self.set_bytes(ffi::ZMQ_UNSUBSCRIBE, prefix)
    }

    /// Publish socket events on an inproc PAIR endpoint (see [`Monitor`]).
    pub fn monitor(&self, endpoint: &str) -> Result<()> {
        let c = cstring(endpoint)?;
        // SAFETY: valid C string.
        check(unsafe { ffi::zmq_socket_monitor(self.raw, c.as_ptr(), ffi::ZMQ_EVENT_ALL) })
            .map(drop)
    }

    /// Send all `frames` as one atomic multipart message.
    pub fn send_multipart<B: AsRef<[u8]>>(&self, frames: &[B], flags: c_int) -> Result<()> {
        let last = frames.len().saturating_sub(1);
        for (i, f) in frames.iter().enumerate() {
            let more = if i < last { ffi::ZMQ_SNDMORE } else { 0 };
            self.send_frame(f.as_ref(), flags | more)?;
        }
        Ok(())
    }

    fn send_frame(&self, data: &[u8], flags: c_int) -> Result<()> {
        let mut msg = ffi::zmq_msg_t::zeroed();
        // SAFETY: msg is a valid zmq_msg_t; init_size allocates `len` bytes we fill below.
        unsafe {
            check(ffi::zmq_msg_init_size(&mut msg, data.len()))?;
            if !data.is_empty() {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    ffi::zmq_msg_data(&mut msg).cast::<u8>(),
                    data.len(),
                );
            }
            if ffi::zmq_msg_send(&mut msg, self.raw, flags) < 0 {
                let e = Error::last();
                ffi::zmq_msg_close(&mut msg);
                return Err(e);
            }
        }
        Ok(())
    }

    /// Receive one whole multipart message. With `ZMQ_DONTWAIT`, returns EAGAIN when empty.
    pub fn recv_multipart(&self, flags: c_int) -> Result<Received> {
        let mut out = Received::default();
        loop {
            let mut msg = ffi::zmq_msg_t::zeroed();
            // SAFETY: msg initialised before use and closed on every path.
            unsafe {
                check(ffi::zmq_msg_init(&mut msg))?;
                // Only the first frame may report EAGAIN; later frames of a multipart message
                // are delivered atomically with it.
                let f = if out.frames.is_empty() { flags } else { 0 };
                if ffi::zmq_msg_recv(&mut msg, self.raw, f) < 0 {
                    let e = Error::last();
                    ffi::zmq_msg_close(&mut msg);
                    return Err(e);
                }
                let len = ffi::zmq_msg_size(&msg);
                let data = ffi::zmq_msg_data(&mut msg).cast::<u8>();
                let bytes = if len == 0 {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(data, len).to_vec()
                };
                if out.frames.is_empty() {
                    let p = ffi::zmq_msg_gets(&msg, c"User-Id".as_ptr());
                    if !p.is_null() {
                        out.user_id = Some(CStr::from_ptr(p).to_string_lossy().into_owned());
                    }
                }
                let more = ffi::zmq_msg_more(&msg) == 1;
                ffi::zmq_msg_close(&mut msg);
                out.frames.push(bytes);
                if !more {
                    return Ok(out);
                }
            }
        }
    }

    /// Receive one message, waiting at most `timeout`. `Ok(None)` on timeout.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<Received>> {
        if poll_in(&[self], timeout)?[0] {
            self.recv_multipart(ffi::ZMQ_DONTWAIT).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Non-blocking receive: `Ok(None)` when nothing is queued.
    pub fn try_recv(&self) -> Result<Option<Received>> {
        match self.recv_multipart(ffi::ZMQ_DONTWAIT) {
            Ok(m) => Ok(Some(m)),
            Err(e) if e.is_again() => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Wait until any socket is readable or `timeout` expires; returns readability per socket.
pub fn poll_in(sockets: &[&Socket], timeout: Duration) -> Result<Vec<bool>> {
    let mut items: Vec<ffi::zmq_pollitem_t> = sockets
        .iter()
        .map(|s| ffi::zmq_pollitem_t {
            socket: s.raw,
            fd: 0,
            events: ffi::ZMQ_POLLIN,
            revents: 0,
        })
        .collect();
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let ms = c_long::try_from(remaining.as_millis()).unwrap_or(c_long::MAX);
        let n = c_int::try_from(items.len()).map_err(|_| Error(libc::EINVAL))?;
        // SAFETY: items point at live sockets for the duration of the call.
        let rc = unsafe { ffi::zmq_poll(items.as_mut_ptr(), n, ms) };
        if rc < 0 {
            let e = Error::last();
            if e.0 == libc::EINTR && !remaining.is_zero() {
                continue;
            }
            return Err(e);
        }
        return Ok(items
            .iter()
            .map(|i| i.revents & ffi::ZMQ_POLLIN != 0)
            .collect());
    }
}

/// A CURVE keypair, Z85-encoded (40 characters each).
#[derive(Clone)]
pub struct CurveKeyPair {
    pub public: String,
    pub secret: String,
}

impl fmt::Debug for CurveKeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CurveKeyPair")
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

impl CurveKeyPair {
    pub fn generate() -> Result<Self> {
        let mut public = [0 as c_char; 41];
        let mut secret = [0 as c_char; 41];
        // SAFETY: both buffers are 41 bytes as the API requires.
        check(unsafe { ffi::zmq_curve_keypair(public.as_mut_ptr(), secret.as_mut_ptr()) })?;
        // SAFETY: zmq_curve_keypair NUL-terminates both buffers.
        let to_s = |b: &[c_char; 41]| unsafe { CStr::from_ptr(b.as_ptr()) }.to_string_lossy();
        Ok(Self {
            public: to_s(&public).into_owned(),
            secret: to_s(&secret).into_owned(),
        })
    }
}

/// Z85 → 32 raw key bytes (the form ZAP passes as CURVE credentials).
pub fn z85_decode_key(z85: &str) -> Result<[u8; 32]> {
    if z85.len() != 40 {
        return Err(Error(libc::EINVAL));
    }
    let c = cstring(z85)?;
    let mut out = [0u8; 32];
    // SAFETY: 40 Z85 chars decode to exactly 32 bytes.
    let p = unsafe { ffi::zmq_z85_decode(out.as_mut_ptr(), c.as_ptr()) };
    if p.is_null() {
        Err(Error(libc::EINVAL))
    } else {
        Ok(out)
    }
}

/// 32 raw key bytes → Z85.
pub fn z85_encode_key(key: &[u8; 32]) -> String {
    let mut buf = [0 as c_char; 41];
    // SAFETY: 32 bytes encode to 40 chars + NUL.
    unsafe { ffi::zmq_z85_encode(buf.as_mut_ptr(), key.as_ptr(), key.len()) };
    // SAFETY: NUL-terminated by zmq_z85_encode.
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// Server-side CURVE: this socket authenticates clients through ZAP under `zap_domain`.
pub fn make_curve_server(s: &Socket, server: &CurveKeyPair, zap_domain: &str) -> Result<()> {
    s.set_int(ffi::ZMQ_CURVE_SERVER, 1)?;
    s.set_bytes(ffi::ZMQ_CURVE_SECRETKEY, server.secret.as_bytes())?;
    s.set_bytes(ffi::ZMQ_ZAP_DOMAIN, zap_domain.as_bytes())
}

/// Client-side CURVE: pins the server's public key.
pub fn make_curve_client(s: &Socket, client: &CurveKeyPair, server_public: &str) -> Result<()> {
    s.set_bytes(ffi::ZMQ_CURVE_SERVERKEY, server_public.as_bytes())?;
    s.set_bytes(ffi::ZMQ_CURVE_PUBLICKEY, client.public.as_bytes())?;
    s.set_bytes(ffi::ZMQ_CURVE_SECRETKEY, client.secret.as_bytes())
}

/// A connection-level event from `zmq_socket_monitor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorEvent {
    pub event: u16,
    pub value: u32,
    pub endpoint: String,
}

/// Reader side of a socket monitor.
#[derive(Debug)]
pub struct Monitor {
    pair: Socket,
}

impl Monitor {
    /// Attach a monitor to `socket` on a fresh inproc endpoint named `name`.
    pub fn attach(ctx: &Context, socket: &Socket, name: &str) -> Result<Self> {
        let ep = format!("inproc://monitor-{name}");
        socket.monitor(&ep)?;
        let pair = ctx.socket(SocketType::Pair)?;
        pair.connect(&ep)?;
        Ok(Self { pair })
    }

    /// Wait (up to `timeout`) for an event whose code is in `mask`.
    pub fn wait_for(&self, mask: u16, timeout: Duration) -> Result<Option<MonitorEvent>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Some(m) = self.pair.recv_timeout(remaining)? else {
                return Ok(None);
            };
            let Some(ev) = parse_monitor(&m) else {
                continue;
            };
            if ev.event & mask != 0 {
                return Ok(Some(ev));
            }
        }
    }

    /// Every event currently queued, without waiting.
    pub fn drain(&self) -> Result<Vec<MonitorEvent>> {
        let mut out = Vec::new();
        while let Some(m) = self.pair.try_recv()? {
            out.extend(parse_monitor(&m));
        }
        Ok(out)
    }
}

fn parse_monitor(m: &Received) -> Option<MonitorEvent> {
    let head = m.frames.first()?;
    if head.len() != 6 {
        return None;
    }
    Some(MonitorEvent {
        event: u16::from_ne_bytes([head[0], head[1]]),
        value: u32::from_ne_bytes([head[2], head[3], head[4], head[5]]),
        endpoint: String::from_utf8_lossy(m.frames.get(1).map_or(&[][..], |v| v)).into_owned(),
    })
}
