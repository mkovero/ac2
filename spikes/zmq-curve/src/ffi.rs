//! Raw libzmq 4.3 C API, only the subset the spike uses. Values mirror `zmq.h`.

use std::ffi::{c_char, c_int, c_long, c_short, c_void};

pub const ZMQ_HAUSNUMERO: c_int = 156_384_712;
pub const ETERM: c_int = ZMQ_HAUSNUMERO + 53;

pub const ZMQ_PUB: c_int = 1;
pub const ZMQ_SUB: c_int = 2;
pub const ZMQ_REP: c_int = 4;
pub const ZMQ_DEALER: c_int = 5;
pub const ZMQ_ROUTER: c_int = 6;
pub const ZMQ_PULL: c_int = 7;
pub const ZMQ_PUSH: c_int = 8;
pub const ZMQ_XPUB: c_int = 9;
pub const ZMQ_PAIR: c_int = 0;

pub const ZMQ_ROUTING_ID: c_int = 5;
pub const ZMQ_SUBSCRIBE: c_int = 6;
pub const ZMQ_UNSUBSCRIBE: c_int = 7;
pub const ZMQ_SNDBUF: c_int = 11;
pub const ZMQ_RCVBUF: c_int = 12;
pub const ZMQ_RCVMORE: c_int = 13;
pub const ZMQ_LINGER: c_int = 17;
pub const ZMQ_RECONNECT_IVL: c_int = 18;
pub const ZMQ_SNDHWM: c_int = 23;
pub const ZMQ_RCVHWM: c_int = 24;
pub const ZMQ_LAST_ENDPOINT: c_int = 32;
pub const ZMQ_ROUTER_MANDATORY: c_int = 33;
pub const ZMQ_XPUB_VERBOSE: c_int = 40;
pub const ZMQ_MECHANISM: c_int = 43;
pub const ZMQ_CURVE_SERVER: c_int = 47;
pub const ZMQ_CURVE_PUBLICKEY: c_int = 48;
pub const ZMQ_CURVE_SECRETKEY: c_int = 49;
pub const ZMQ_CURVE_SERVERKEY: c_int = 50;
pub const ZMQ_CONFLATE: c_int = 54;
pub const ZMQ_ZAP_DOMAIN: c_int = 55;
pub const ZMQ_HANDSHAKE_IVL: c_int = 66;

pub const ZMQ_DONTWAIT: c_int = 1;
pub const ZMQ_SNDMORE: c_int = 2;

pub const ZMQ_POLLIN: c_short = 1;

pub const ZMQ_EVENT_ALL: c_int = 0xFFFF;
pub const ZMQ_EVENT_ACCEPTED: u16 = 0x0020;
pub const ZMQ_EVENT_DISCONNECTED: u16 = 0x0200;
pub const ZMQ_EVENT_HANDSHAKE_FAILED_NO_DETAIL: u16 = 0x0800;
pub const ZMQ_EVENT_HANDSHAKE_SUCCEEDED: u16 = 0x1000;
pub const ZMQ_EVENT_HANDSHAKE_FAILED_PROTOCOL: u16 = 0x2000;
pub const ZMQ_EVENT_HANDSHAKE_FAILED_AUTH: u16 = 0x4000;

/// `zmq_msg_t` is an opaque 64-byte, pointer-aligned blob.
#[repr(C, align(8))]
#[derive(Debug)]
pub struct zmq_msg_t {
    _opaque: [u8; 64],
}

impl zmq_msg_t {
    pub const fn zeroed() -> Self {
        Self { _opaque: [0; 64] }
    }
}

#[cfg(windows)]
pub type zmq_fd_t = usize;
#[cfg(not(windows))]
pub type zmq_fd_t = c_int;

#[repr(C)]
#[derive(Debug)]
pub struct zmq_pollitem_t {
    pub socket: *mut c_void,
    pub fd: zmq_fd_t,
    pub events: c_short,
    pub revents: c_short,
}

unsafe extern "C" {
    pub fn zmq_errno() -> c_int;
    pub fn zmq_strerror(errnum: c_int) -> *const c_char;
    pub fn zmq_version(major: *mut c_int, minor: *mut c_int, patch: *mut c_int);
    pub fn zmq_has(capability: *const c_char) -> c_int;

    pub fn zmq_ctx_new() -> *mut c_void;
    pub fn zmq_ctx_term(ctx: *mut c_void) -> c_int;

    pub fn zmq_socket(ctx: *mut c_void, kind: c_int) -> *mut c_void;
    pub fn zmq_close(s: *mut c_void) -> c_int;
    pub fn zmq_setsockopt(s: *mut c_void, opt: c_int, val: *const c_void, len: usize) -> c_int;
    pub fn zmq_getsockopt(s: *mut c_void, opt: c_int, val: *mut c_void, len: *mut usize) -> c_int;
    pub fn zmq_bind(s: *mut c_void, addr: *const c_char) -> c_int;
    pub fn zmq_connect(s: *mut c_void, addr: *const c_char) -> c_int;
    pub fn zmq_socket_monitor(s: *mut c_void, addr: *const c_char, events: c_int) -> c_int;

    pub fn zmq_msg_init(msg: *mut zmq_msg_t) -> c_int;
    pub fn zmq_msg_init_size(msg: *mut zmq_msg_t, size: usize) -> c_int;
    pub fn zmq_msg_data(msg: *mut zmq_msg_t) -> *mut c_void;
    pub fn zmq_msg_size(msg: *const zmq_msg_t) -> usize;
    pub fn zmq_msg_more(msg: *const zmq_msg_t) -> c_int;
    pub fn zmq_msg_gets(msg: *const zmq_msg_t, property: *const c_char) -> *const c_char;
    pub fn zmq_msg_send(msg: *mut zmq_msg_t, s: *mut c_void, flags: c_int) -> c_int;
    pub fn zmq_msg_recv(msg: *mut zmq_msg_t, s: *mut c_void, flags: c_int) -> c_int;
    pub fn zmq_msg_close(msg: *mut zmq_msg_t) -> c_int;

    pub fn zmq_poll(items: *mut zmq_pollitem_t, nitems: c_int, timeout: c_long) -> c_int;

    pub fn zmq_curve_keypair(public: *mut c_char, secret: *mut c_char) -> c_int;
    pub fn zmq_z85_encode(dest: *mut c_char, data: *const u8, size: usize) -> *mut c_char;
    pub fn zmq_z85_decode(dest: *mut u8, string: *const c_char) -> *mut u8;
}
