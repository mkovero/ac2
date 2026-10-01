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
pub(crate) const ZMQ_LINGER: c_int = 17;
pub(crate) const ZMQ_RECONNECT_IVL: c_int = 18;
pub(crate) const ZMQ_SNDHWM: c_int = 23;
pub(crate) const ZMQ_RCVHWM: c_int = 24;
pub(crate) const ZMQ_LAST_ENDPOINT: c_int = 32;
pub(crate) const ZMQ_ROUTER_MANDATORY: c_int = 33;
pub(crate) const ZMQ_XPUB_VERBOSE: c_int = 40;
pub(crate) const ZMQ_CURVE_SERVER: c_int = 47;
pub(crate) const ZMQ_CURVE_PUBLICKEY: c_int = 48;
pub(crate) const ZMQ_CURVE_SECRETKEY: c_int = 49;
pub(crate) const ZMQ_CURVE_SERVERKEY: c_int = 50;
pub(crate) const ZMQ_ZAP_DOMAIN: c_int = 55;
pub(crate) const ZMQ_HANDSHAKE_IVL: c_int = 66;

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
