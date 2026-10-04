//! The only module with `unsafe`: the libzmq C API subset ac2 uses, and owning wrappers whose
//! signatures are safe. Every other module is `deny(unsafe_code)`.
//!
//! Invariants upheld here:
//! - A [`RawSocket`] keeps the [`RawContext`] it was created from alive (`Arc`), so a socket
//!   pointer is never used after `zmq_ctx_term`, and the context is terminated only after every
//!   socket of it is closed.
//! - `RawSocket` is `Send` but not `Sync`: libzmq sockets may migrate between threads (libzmq
//!   requires a full memory barrier, which moving a value across threads provides) but must
//!   never be used by two threads at once.
//! - Every `zmq_msg_t` initialised here is closed on every path.

#![allow(non_camel_case_types, reason = "FFI items mirror zmq.h names")]

use std::ffi::{CStr, c_char, c_int, c_long, c_short, c_void};
use std::ptr::NonNull;
use std::sync::Arc;

/// Result of a libzmq call: the errno value on failure.
pub(crate) type RawResult<T> = Result<T, c_int>;

// Values mirror zmq.h (4.3.5).
const ZMQ_HAUSNUMERO: c_int = 156_384_712;
pub(crate) const ETERM: c_int = ZMQ_HAUSNUMERO + 53;

pub(crate) const ZMQ_PAIR: c_int = 0;
pub(crate) const ZMQ_SUB: c_int = 2;
pub(crate) const ZMQ_REP: c_int = 4;
pub(crate) const ZMQ_DEALER: c_int = 5;
pub(crate) const ZMQ_ROUTER: c_int = 6;
pub(crate) const ZMQ_PULL: c_int = 7;
pub(crate) const ZMQ_PUSH: c_int = 8;
pub(crate) const ZMQ_XPUB: c_int = 9;

pub(crate) const ZMQ_ROUTING_ID: c_int = 5;
pub(crate) const ZMQ_SUBSCRIBE: c_int = 6;
pub(crate) const ZMQ_UNSUBSCRIBE: c_int = 7;
pub(crate) const ZMQ_SNDBUF: c_int = 11;
pub(crate) const ZMQ_RCVBUF: c_int = 12;
pub(crate) const ZMQ_FD: c_int = 14;
pub(crate) const ZMQ_EVENTS: c_int = 15;
pub(crate) const ZMQ_LINGER: c_int = 17;
pub(crate) const ZMQ_RECONNECT_IVL: c_int = 18;
pub(crate) const ZMQ_RECONNECT_IVL_MAX: c_int = 21;
pub(crate) const ZMQ_SNDHWM: c_int = 23;
pub(crate) const ZMQ_RCVHWM: c_int = 24;
pub(crate) const ZMQ_LAST_ENDPOINT: c_int = 32;
pub(crate) const ZMQ_ROUTER_MANDATORY: c_int = 33;
pub(crate) const ZMQ_TCP_KEEPALIVE: c_int = 34;
pub(crate) const ZMQ_TCP_KEEPALIVE_CNT: c_int = 35;
pub(crate) const ZMQ_TCP_KEEPALIVE_IDLE: c_int = 36;
pub(crate) const ZMQ_TCP_KEEPALIVE_INTVL: c_int = 37;
pub(crate) const ZMQ_XPUB_VERBOSE: c_int = 40;
pub(crate) const ZMQ_CURVE_SERVER: c_int = 47;
pub(crate) const ZMQ_CURVE_PUBLICKEY: c_int = 48;
pub(crate) const ZMQ_CURVE_SECRETKEY: c_int = 49;
pub(crate) const ZMQ_CURVE_SERVERKEY: c_int = 50;
pub(crate) const ZMQ_ZAP_DOMAIN: c_int = 55;
pub(crate) const ZMQ_HANDSHAKE_IVL: c_int = 66;
pub(crate) const ZMQ_TCP_MAXRT: c_int = 80;

pub(crate) const ZMQ_DONTWAIT: c_int = 1;
pub(crate) const ZMQ_SNDMORE: c_int = 2;

pub(crate) const ZMQ_POLLIN: c_short = 1;
pub(crate) const ZMQ_POLLOUT: c_short = 2;

pub(crate) const ZMQ_EVENT_ALL: c_int = 0xFFFF;

/// `zmq_msg_t` is an opaque 64-byte, pointer-aligned blob.
#[repr(C, align(8))]
struct zmq_msg_t {
    _opaque: [u8; 64],
}

#[cfg(windows)]
type zmq_fd_t = usize;
#[cfg(not(windows))]
type zmq_fd_t = c_int;

#[repr(C)]
struct zmq_pollitem_t {
    socket: *mut c_void,
    fd: zmq_fd_t,
    events: c_short,
    revents: c_short,
}

unsafe extern "C" {
    fn zmq_errno() -> c_int;
    fn zmq_strerror(errnum: c_int) -> *const c_char;
    fn zmq_version(major: *mut c_int, minor: *mut c_int, patch: *mut c_int);
    fn zmq_has(capability: *const c_char) -> c_int;

    fn zmq_ctx_new() -> *mut c_void;
    fn zmq_ctx_term(ctx: *mut c_void) -> c_int;

    fn zmq_socket(ctx: *mut c_void, kind: c_int) -> *mut c_void;
    fn zmq_close(s: *mut c_void) -> c_int;
    fn zmq_setsockopt(s: *mut c_void, opt: c_int, val: *const c_void, len: usize) -> c_int;
    fn zmq_getsockopt(s: *mut c_void, opt: c_int, val: *mut c_void, len: *mut usize) -> c_int;
    fn zmq_bind(s: *mut c_void, addr: *const c_char) -> c_int;
    fn zmq_connect(s: *mut c_void, addr: *const c_char) -> c_int;
    fn zmq_socket_monitor(s: *mut c_void, addr: *const c_char, events: c_int) -> c_int;

    fn zmq_msg_init(msg: *mut zmq_msg_t) -> c_int;
    fn zmq_msg_init_size(msg: *mut zmq_msg_t, size: usize) -> c_int;
    fn zmq_msg_init_data(
        msg: *mut zmq_msg_t,
        data: *mut c_void,
        size: usize,
        ffn: Option<unsafe extern "C" fn(data: *mut c_void, hint: *mut c_void)>,
        hint: *mut c_void,
    ) -> c_int;
    fn zmq_msg_copy(dest: *mut zmq_msg_t, src: *mut zmq_msg_t) -> c_int;
    fn zmq_msg_data(msg: *mut zmq_msg_t) -> *mut c_void;
    fn zmq_msg_size(msg: *const zmq_msg_t) -> usize;
    fn zmq_msg_more(msg: *const zmq_msg_t) -> c_int;
    fn zmq_msg_gets(msg: *const zmq_msg_t, property: *const c_char) -> *const c_char;
    fn zmq_msg_send(msg: *mut zmq_msg_t, s: *mut c_void, flags: c_int) -> c_int;
    fn zmq_msg_recv(msg: *mut zmq_msg_t, s: *mut c_void, flags: c_int) -> c_int;
    fn zmq_msg_close(msg: *mut zmq_msg_t) -> c_int;

    fn zmq_poll(items: *mut zmq_pollitem_t, nitems: c_int, timeout: c_long) -> c_int;

    fn zmq_curve_keypair(public: *mut c_char, secret: *mut c_char) -> c_int;
    fn zmq_z85_encode(dest: *mut c_char, data: *const u8, size: usize) -> *mut c_char;
    fn zmq_z85_decode(dest: *mut u8, string: *const c_char) -> *mut u8;
}

/// errno of the last failed libzmq call on this thread.
pub(crate) fn errno() -> c_int {
    // SAFETY: reads the calling thread's errno; no pointers involved.
    unsafe { zmq_errno() }
}

fn check(rc: c_int) -> RawResult<c_int> {
    if rc < 0 { Err(errno()) } else { Ok(rc) }
}

/// libzmq's message for an errno value (its own codes and the C library's).
pub(crate) fn strerror(errnum: c_int) -> String {
    // SAFETY: zmq_strerror returns a pointer to a static NUL-terminated string for any value.
    unsafe { CStr::from_ptr(zmq_strerror(errnum)) }
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn version() -> (i32, i32, i32) {
    let (mut a, mut b, mut c) = (0, 0, 0);
    // SAFETY: the three out-pointers are valid for writes.
    unsafe { zmq_version(&mut a, &mut b, &mut c) };
    (a, b, c)
}

pub(crate) fn has(capability: &CStr) -> bool {
    // SAFETY: valid NUL-terminated string.
    unsafe { zmq_has(capability.as_ptr()) == 1 }
}

/// An owned libzmq context, terminated on drop.
#[derive(Debug)]
pub(crate) struct RawContext(NonNull<c_void>);

// SAFETY: a libzmq context is thread-safe (documented for all zmq_ctx_* and zmq_socket).
unsafe impl Send for RawContext {}
// SAFETY: as above.
unsafe impl Sync for RawContext {}

impl RawContext {
    pub(crate) fn new() -> RawResult<Self> {
        // SAFETY: no arguments; a null return is handled.
        let ptr = unsafe { zmq_ctx_new() };
        NonNull::new(ptr).map(Self).ok_or_else(errno)
    }
}

impl Drop for RawContext {
    fn drop(&mut self) {
        // Every RawSocket holds an Arc to this context, so all of its sockets are closed and
        // term returns once their linger expires.
        loop {
            // SAFETY: the pointer came from zmq_ctx_new and is terminated exactly once (the
            // loop only repeats when term was interrupted before completing).
            if unsafe { zmq_ctx_term(self.0.as_ptr()) } == 0 || errno() != libc::EINTR {
                break;
            }
        }
    }
}

/// One received frame with the connection metadata libzmq attaches to it.
#[derive(Debug)]
pub(crate) struct RawFrame {
    pub(crate) data: Vec<u8>,
    pub(crate) more: bool,
    pub(crate) user_id: Option<String>,
    pub(crate) peer_address: Option<String>,
}

/// An owned libzmq socket, closed on drop.
#[derive(Debug)]
pub(crate) struct RawSocket {
    ptr: NonNull<c_void>,
    // Declared after `ptr`: the context must outlive the socket (Drop closes first).
    _ctx: Arc<RawContext>,
}

// SAFETY: libzmq sockets may be moved to another thread when no other thread uses them;
// RawSocket is not Sync, so `&RawSocket` never crosses threads.
unsafe impl Send for RawSocket {}

impl Drop for RawSocket {
    fn drop(&mut self) {
        // SAFETY: the pointer came from zmq_socket and is closed exactly once; the context is
        // still alive (held by `_ctx`).
        unsafe { zmq_close(self.ptr.as_ptr()) };
    }
}

impl RawSocket {
    pub(crate) fn new(ctx: &Arc<RawContext>, kind: c_int) -> RawResult<Self> {
        // SAFETY: the context pointer is live (borrowed Arc); a null return is handled.
        let ptr = unsafe { zmq_socket(ctx.0.as_ptr(), kind) };
        let ptr = NonNull::new(ptr).ok_or_else(errno)?;
        Ok(Self {
            ptr,
            _ctx: Arc::clone(ctx),
        })
    }

    pub(crate) fn set_int(&self, opt: c_int, value: c_int) -> RawResult<()> {
        // SAFETY: pointer and length describe a live c_int; libzmq copies the value.
        check(unsafe {
            zmq_setsockopt(
                self.ptr.as_ptr(),
                opt,
                (&raw const value).cast(),
                size_of::<c_int>(),
            )
        })
        .map(drop)
    }

    pub(crate) fn set_bytes(&self, opt: c_int, value: &[u8]) -> RawResult<()> {
        // SAFETY: pointer and length describe a live slice; libzmq copies the bytes.
        check(unsafe { zmq_setsockopt(self.ptr.as_ptr(), opt, value.as_ptr().cast(), value.len()) })
            .map(drop)
    }

    pub(crate) fn get_int(&self, opt: c_int) -> RawResult<c_int> {
        let mut value: c_int = 0;
        let mut len = size_of::<c_int>();
        // SAFETY: pointer and in/out length describe a live, writable c_int.
        check(unsafe {
            zmq_getsockopt(self.ptr.as_ptr(), opt, (&raw mut value).cast(), &mut len)
        })?;
        Ok(value)
    }

    /// Reads a string option into `buf`; returns the number of bytes libzmq wrote (including
    /// the trailing NUL for string options).
    pub(crate) fn get_bytes(&self, opt: c_int, buf: &mut [u8]) -> RawResult<usize> {
        let mut len = buf.len();
        // SAFETY: pointer and in/out length describe a live, writable buffer.
        check(unsafe {
            zmq_getsockopt(self.ptr.as_ptr(), opt, buf.as_mut_ptr().cast(), &mut len)
        })?;
        Ok(len)
    }

    pub(crate) fn bind(&self, endpoint: &CStr) -> RawResult<()> {
        // SAFETY: valid NUL-terminated string; libzmq copies it.
        check(unsafe { zmq_bind(self.ptr.as_ptr(), endpoint.as_ptr()) }).map(drop)
    }

    pub(crate) fn connect(&self, endpoint: &CStr) -> RawResult<()> {
        // SAFETY: valid NUL-terminated string; libzmq copies it.
        check(unsafe { zmq_connect(self.ptr.as_ptr(), endpoint.as_ptr()) }).map(drop)
    }

    pub(crate) fn monitor(&self, endpoint: &CStr, events: c_int) -> RawResult<()> {
        // SAFETY: valid NUL-terminated string; libzmq copies it.
        check(unsafe { zmq_socket_monitor(self.ptr.as_ptr(), endpoint.as_ptr(), events) }).map(drop)
    }

    /// Sends one frame (copying `data` into a libzmq message).
    pub(crate) fn send_frame(&self, data: &[u8], flags: c_int) -> RawResult<()> {
        let mut msg = zmq_msg_t { _opaque: [0; 64] };
        // SAFETY: init_size allocates `data.len()` bytes owned by `msg`, which are filled from a
        // live slice of exactly that length. On success zmq_msg_send takes ownership of the
        // content; on failure the message is still ours and is closed.
        unsafe {
            check(zmq_msg_init_size(&mut msg, data.len()))?;
            if !data.is_empty() {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    zmq_msg_data(&mut msg).cast::<u8>(),
                    data.len(),
                );
            }
            if zmq_msg_send(&mut msg, self.ptr.as_ptr(), flags) < 0 {
                let e = errno();
                zmq_msg_close(&mut msg);
                return Err(e);
            }
        }
        Ok(())
    }

    /// Receives one frame. `metadata` reads the connection properties (`User-Id`,
    /// `Peer-Address`), which are the same for every frame of a message.
    pub(crate) fn recv_frame(&self, flags: c_int, metadata: bool) -> RawResult<RawFrame> {
        let mut msg = zmq_msg_t { _opaque: [0; 64] };
        // SAFETY: `msg` is initialised before use and closed on every path after init. The data
        // pointer/size pair describes the message's own buffer, valid until close; it is copied
        // out before. Property strings returned by zmq_msg_gets live as long as the message and
        // are copied out before close.
        unsafe {
            check(zmq_msg_init(&mut msg))?;
            if zmq_msg_recv(&mut msg, self.ptr.as_ptr(), flags) < 0 {
                let e = errno();
                zmq_msg_close(&mut msg);
                return Err(e);
            }
            let len = zmq_msg_size(&msg);
            let data = if len == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(zmq_msg_data(&mut msg).cast::<u8>(), len).to_vec()
            };
            let property = |name: &CStr| {
                let p = zmq_msg_gets(&msg, name.as_ptr());
                (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
            };
            let (user_id, peer_address) = if metadata {
                (property(c"User-Id"), property(c"Peer-Address"))
            } else {
                (None, None)
            };
            let more = zmq_msg_more(&msg) == 1;
            zmq_msg_close(&mut msg);
            Ok(RawFrame {
                data,
                more,
                user_id,
                peer_address,
            })
        }
    }
}

impl RawSocket {
    /// Sends one message part. On success libzmq has taken the content (the part is left an
    /// empty message); on failure the part is unchanged.
    pub(crate) fn send_msg(&self, part: &mut RawMsg, flags: c_int) -> RawResult<()> {
        // SAFETY: `part` owns an initialised message; zmq_msg_send either takes the content
        // (re-initialising the message empty) or fails leaving it untouched.
        check(unsafe { zmq_msg_send(part.msg.get(), self.ptr.as_ptr(), flags) }).map(drop)
    }

    /// Receives one message part without copying its content; also whether more parts of
    /// the same message follow.
    pub(crate) fn recv_msg(&self, flags: c_int) -> RawResult<(RawMsg, bool)> {
        let part = RawMsg::empty();
        // SAFETY: `part` is initialised; on failure it stays an empty message closed by Drop.
        check(unsafe { zmq_msg_recv(part.msg.get(), self.ptr.as_ptr(), flags) })?;
        // SAFETY: initialised message.
        let more = unsafe { zmq_msg_more(part.msg.get()) } == 1;
        Ok((part, more))
    }
}

/// An owned, initialised `zmq_msg_t`, closed on drop: one message part whose content stays
/// where libzmq (or the `Vec` it was made from) put it.
///
/// `Send` but not `Sync`: sharing the content goes through libzmq's atomic reference count,
/// so shares may live on different threads, but making a share writes the source message's
/// flags, which must not race with another thread using that same message.
pub(crate) struct RawMsg {
    msg: std::cell::UnsafeCell<zmq_msg_t>,
}

// SAFETY: a zmq_msg_t has no thread affinity; shared content is reference counted atomically
// by libzmq, and the free function used here (dropping a Vec) may run on any thread.
unsafe impl Send for RawMsg {}

impl std::fmt::Debug for RawMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawMsg").field("len", &self.len()).finish()
    }
}

/// Free function of [`RawMsg::from_vec`] content.
unsafe extern "C" fn free_boxed_vec(_data: *mut c_void, hint: *mut c_void) {
    // SAFETY: `hint` is the Box<Vec<u8>> leaked in `RawMsg::from_vec`; libzmq calls this
    // exactly once, when the last reference to the content is released.
    drop(unsafe { Box::from_raw(hint.cast::<Vec<u8>>()) });
}

impl RawMsg {
    /// Wraps a blob that a successful `zmq_msg_init*` call initialised.
    fn wrap(msg: zmq_msg_t) -> Self {
        Self {
            msg: std::cell::UnsafeCell::new(msg),
        }
    }

    /// An empty message.
    pub(crate) fn empty() -> Self {
        let mut msg = zmq_msg_t { _opaque: [0; 64] };
        // SAFETY: zmq_msg_init only writes the blob and always succeeds.
        unsafe { zmq_msg_init(&mut msg) };
        Self::wrap(msg)
    }

    /// A message owning `data` without copying it; libzmq frees the Vec when the last
    /// reference (queued shares included) is released.
    pub(crate) fn from_vec(data: Vec<u8>) -> RawResult<Self> {
        if data.is_empty() {
            return Ok(Self::empty());
        }
        let mut msg = zmq_msg_t { _opaque: [0; 64] };
        let mut boxed = Box::new(data);
        let ptr = boxed.as_mut_ptr().cast::<c_void>();
        let len = boxed.len();
        let hint = Box::into_raw(boxed);
        // SAFETY: `ptr`/`len` describe the Vec's buffer, which stays put while the Box owning
        // the Vec lives; ownership of that Box passes to libzmq, which frees it through
        // `free_boxed_vec`. zmq_msg_init_data fails only before taking ownership, and then
        // the Box is reclaimed here.
        let rc =
            unsafe { zmq_msg_init_data(&mut msg, ptr, len, Some(free_boxed_vec), hint.cast()) };
        if rc < 0 {
            let e = errno();
            // SAFETY: never handed to libzmq (init failed); reclaimed exactly once.
            drop(unsafe { Box::from_raw(hint) });
            return Err(e);
        }
        Ok(Self::wrap(msg))
    }

    /// A message holding a copy of `data`.
    pub(crate) fn copy_from(data: &[u8]) -> RawResult<Self> {
        let mut msg = zmq_msg_t { _opaque: [0; 64] };
        // SAFETY: init_size allocates `data.len()` bytes owned by the message, then filled
        // from a live slice of exactly that length.
        unsafe {
            check(zmq_msg_init_size(&mut msg, data.len()))?;
            if !data.is_empty() {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    zmq_msg_data(&mut msg).cast::<u8>(),
                    data.len(),
                );
            }
        }
        Ok(Self::wrap(msg))
    }

    /// Another message with the same content: a reference-count increment for large parts,
    /// a copy of a few dozen bytes for the small ones libzmq stores inline.
    pub(crate) fn share(&self) -> RawResult<Self> {
        let m = Self::empty();
        // SAFETY: both messages are initialised and distinct. `self` is not Sync, so no other
        // thread uses its blob while zmq_msg_copy updates its flags.
        check(unsafe { zmq_msg_copy(m.msg.get(), self.msg.get()) })?;
        Ok(m)
    }

    pub(crate) fn len(&self) -> usize {
        // SAFETY: initialised message.
        unsafe { zmq_msg_size(self.msg.get()) }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        let len = self.len();
        if len == 0 {
            return &[];
        }
        // SAFETY: data pointer and size describe the message's content, which is valid and
        // never written while the message lives; the slice borrows `self`. (Inline content of
        // small messages lives in the blob itself, which does not move while borrowed.)
        unsafe { std::slice::from_raw_parts(zmq_msg_data(self.msg.get()).cast::<u8>(), len) }
    }
}

impl Drop for RawMsg {
    fn drop(&mut self) {
        // SAFETY: every RawMsg is initialised (constructed only after a successful init) and
        // closed exactly once, here.
        unsafe { zmq_msg_close(self.msg.get()) };
    }
}

/// `zmq_poll` over `items` (socket, requested events); returns the ready events per item.
pub(crate) fn poll(items: &[(&RawSocket, c_short)], timeout_ms: c_long) -> RawResult<Vec<c_short>> {
    let mut raw: Vec<zmq_pollitem_t> = items
        .iter()
        .map(|(s, events)| zmq_pollitem_t {
            socket: s.ptr.as_ptr(),
            fd: 0,
            events: *events,
            revents: 0,
        })
        .collect();
    let n = c_int::try_from(raw.len()).map_err(|_| libc::EINVAL)?;
    // SAFETY: `raw` is a live array of `n` items whose sockets are borrowed for the call.
    check(unsafe { zmq_poll(raw.as_mut_ptr(), n, timeout_ms) })?;
    Ok(raw.iter().map(|i| i.revents).collect())
}

/// Fresh CURVE keypair (public, secret) as raw 32-byte keys.
pub(crate) fn curve_keypair() -> RawResult<([u8; 32], [u8; 32])> {
    let mut public = [0 as c_char; 41];
    let mut secret = [0 as c_char; 41];
    // SAFETY: both buffers are the 41 bytes (40 Z85 chars + NUL) the API requires.
    check(unsafe { zmq_curve_keypair(public.as_mut_ptr(), secret.as_mut_ptr()) })?;
    let decode = |z: &[c_char; 41]| {
        let mut out = [0u8; 32];
        // SAFETY: `z` is NUL-terminated 40-char Z85 written by libzmq, which decodes to exactly
        // 32 bytes.
        let p = unsafe { zmq_z85_decode(out.as_mut_ptr(), z.as_ptr()) };
        if p.is_null() {
            Err(libc::EINVAL)
        } else {
            Ok(out)
        }
    };
    let keys = (decode(&public)?, decode(&secret)?);
    zeroize(bytes_of_c_chars(&mut secret));
    Ok(keys)
}

fn bytes_of_c_chars(buf: &mut [c_char]) -> &mut [u8] {
    // SAFETY: c_char and u8 have the same size and alignment, and every bit pattern is valid
    // for both.
    unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), buf.len()) }
}

/// Z85 text for `data`; `None` unless `data.len()` is a multiple of 4.
pub(crate) fn z85_encode(data: &[u8]) -> Option<String> {
    if !data.len().is_multiple_of(4) {
        return None;
    }
    let mut out = vec![0 as c_char; data.len() / 4 * 5 + 1];
    // SAFETY: `out` has room for len*5/4 characters plus NUL, as zmq_z85_encode requires for
    // an input length that is a multiple of 4.
    let p = unsafe { zmq_z85_encode(out.as_mut_ptr(), data.as_ptr(), data.len()) };
    if p.is_null() {
        return None;
    }
    // SAFETY: NUL-terminated by zmq_z85_encode.
    Some(
        unsafe { CStr::from_ptr(out.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Bytes for Z85 `text`; `None` unless the length is a multiple of 5 and every character is
/// in the Z85 alphabet.
pub(crate) fn z85_decode(text: &CStr) -> Option<Vec<u8>> {
    let len = text.to_bytes().len();
    if !len.is_multiple_of(5) {
        return None;
    }
    let mut out = vec![0u8; len / 5 * 4];
    // SAFETY: `text` is NUL-terminated with a length that is a multiple of 5 and `out` holds
    // the len*4/5 bytes it decodes to. libzmq returns null on a character outside Z85.
    let p = unsafe { zmq_z85_decode(out.as_mut_ptr(), text.as_ptr()) };
    (!p.is_null()).then_some(out)
}

/// Overwrites secret material so it does not linger in freed memory.
pub(crate) fn zeroize(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: `b` is a valid, aligned, exclusive reference. The volatile write keeps the
        // compiler from eliding a store to memory that is about to be freed.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}
