//! Typed errors for every fallible call of this crate.

use std::ffi::c_int;
use std::fmt;

use crate::raw;
use crate::zap::ZapExit;

/// An error from libzmq or from this crate's own checks.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The operation would block (`EAGAIN`): nothing queued on a non-blocking receive, or the
    /// peer's queue is at its high-water mark on a non-blocking send.
    WouldBlock,
    /// A ROUTER with `router_mandatory` was asked to route to a peer that is not connected
    /// (`EHOSTUNREACH`).
    HostUnreachable,
    /// The context is being terminated (`ETERM`).
    ContextTerminated,
    /// The endpoint is already bound (`EADDRINUSE`).
    AddressInUse,
    /// Any other libzmq failure.
    Zmq {
        /// errno value as libzmq reported it.
        errno: c_int,
        /// libzmq's description of `errno`.
        message: String,
    },
    /// An argument was rejected before reaching libzmq.
    InvalidArgument(&'static str),
    /// Text was not valid Z85, or decoded to the wrong length for a CURVE key.
    InvalidKey,
    /// The ZAP handler of a [`crate::SecureContext`] is no longer answering; no CURVE server
    /// socket may be created and existing ones must be treated as compromised.
    ZapHandlerExited(ZapExit),
    /// A thread could not be spawned.
    Spawn(String),
}

impl Error {
    pub(crate) fn from_errno(errno: c_int) -> Self {
        match errno {
            libc::EAGAIN => Self::WouldBlock,
            libc::EHOSTUNREACH => Self::HostUnreachable,
            libc::EADDRINUSE => Self::AddressInUse,
            raw::ETERM => Self::ContextTerminated,
            _ => Self::Zmq {
                errno,
                message: raw::strerror(errno),
            },
        }
    }

    /// Whether this is [`Error::WouldBlock`].
    pub fn is_would_block(&self) -> bool {
        matches!(self, Self::WouldBlock)
    }

    pub(crate) fn is_interrupted(&self) -> bool {
        matches!(self, Self::Zmq { errno, .. } if *errno == libc::EINTR)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WouldBlock => f.write_str("operation would block"),
            Self::HostUnreachable => f.write_str("peer is not connected"),
            Self::ContextTerminated => f.write_str("context terminated"),
            Self::AddressInUse => f.write_str("address already in use"),
            Self::Zmq { errno, message } => write!(f, "{message} (errno {errno})"),
            Self::InvalidArgument(what) => write!(f, "invalid argument: {what}"),
            Self::InvalidKey => f.write_str("not a Z85-encoded 32-byte CURVE key"),
            Self::ZapHandlerExited(why) => write!(f, "ZAP handler exited: {why}"),
            Self::Spawn(e) => write!(f, "cannot spawn thread: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Converts a raw libzmq result.
pub(crate) fn zmq<T>(r: raw::RawResult<T>) -> Result<T> {
    r.map_err(Error::from_errno)
}
