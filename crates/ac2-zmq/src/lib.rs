//! ac2's ZeroMQ layer and the only crate that links libzmq.
//!
//! libzmq 4.3.5 and libsodium are built from source as static libraries (CURVE enabled; see
//! `build.rs` and `docs/design/spike-zmq-curve.md`). On top of a minimal, typed binding
//! ([`Context`], [`Socket`], [`poll`], [`Socket::monitor`], CURVE keys and Z85) this crate
//! provides the security building blocks — [`SecureContext`] with its ZAP handler and
//! [`AuthorizedKeys`] — and the socket patterns ac2 relies on ([`drain_latest`],
//! [`SubscriptionTracker`]).
//!
//! The daemon's I/O-thread architecture is not here: one thread owns ROUTER + XPUB and polls
//! them with an inproc PULL that other threads feed; that lives in `ac2d`.
//!
//! # Security rule
//!
//! libzmq accepts every CURVE client that knows the server key when no ZAP handler is bound
//! to the context. Server-side CURVE is therefore only reachable via
//! [`SecureContext::curve_server_socket`], which exists only with a running handler and keeps
//! it alive; see [`zap`](crate::zap).
//!
//! All `unsafe` code is confined to one private module (`raw`).

#![deny(unsafe_code)]

mod authorized;
mod curve;
mod error;
mod monitor;
mod pattern;
mod poll;
#[allow(
    unsafe_code,
    reason = "the FFI boundary; every block carries a SAFETY comment"
)]
mod raw;
mod socket;
pub mod zap;

pub use authorized::{AuthorizedKeys, KeyStoreError, MAX_NAME_LEN};
pub use curve::{CurveClient, KeyPair, PublicKey, SecretKey, z85_decode, z85_encode};
pub use error::{Error, Result};
pub use monitor::{Monitor, MonitorEvent, SocketEvent};
pub use pattern::{Drained, Latest, SubscriptionEvent, SubscriptionTracker, drain_latest};
pub use poll::{Interest, PollItem, poll};
pub use socket::{Context, Message, Part, Socket, SocketType, TcpLiveness};
pub use zap::{
    AuthorizedHandle, DenyReason, Mechanism, SecureContext, Verdict, ZapDecision, ZapExit,
};

/// Version `(major, minor, patch)` of the linked libzmq.
pub fn libzmq_version() -> (i32, i32, i32) {
    raw::version()
}

/// Whether the linked libzmq supports a capability (`"curve"`, `"ipc"`, ...).
pub fn libzmq_has(capability: &str) -> bool {
    std::ffi::CString::new(capability).is_ok_and(|c| raw::has(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_libzmq_4_3_5_with_curve() {
        assert_eq!(libzmq_version(), (4, 3, 5));
        assert!(libzmq_has("curve"), "libzmq built without CURVE");
        #[cfg(unix)]
        assert!(libzmq_has("ipc"));
    }

    #[test]
    fn z85_and_keys_roundtrip() -> Result<()> {
        // RFC 32 test vector.
        let bytes = [0x86, 0x4F, 0xD2, 0x6F, 0xB5, 0x59, 0xF7, 0x5B];
        assert_eq!(z85_encode(&bytes)?, "HelloWorld");
        assert_eq!(z85_decode("HelloWorld")?, bytes);
        assert!(z85_encode(&[1, 2, 3]).is_err());
        assert!(z85_decode("Hell").is_err());
        assert!(
            z85_decode("Hell\"").is_err(),
            "quote is outside the Z85 alphabet"
        );

        let kp = KeyPair::generate()?;
        let text = kp.public.to_z85();
        assert_eq!(text.len(), 40);
        assert_eq!(PublicKey::from_z85(&text)?, kp.public);
        assert_eq!(SecretKey::from_z85(&kp.secret.to_z85())?, kp.secret);
        assert_eq!(PublicKey::from_z85("short"), Err(Error::InvalidKey));
        assert!(!format!("{:?}", kp).contains(&kp.secret.to_z85()));
        Ok(())
    }

    #[test]
    fn errors_are_typed() -> Result<()> {
        let ctx = Context::new()?;
        let pull = ctx.socket(SocketType::Pull)?;
        assert_eq!(pull.try_recv()?, None);
        let push = ctx.socket(SocketType::Push)?;
        // No peer: a PUSH cannot queue anything.
        assert_eq!(push.try_send(&[b"x"]), Err(Error::WouldBlock));
        let a = ctx.socket(SocketType::Pull)?;
        a.bind("inproc://errors-are-typed")?;
        let b = ctx.socket(SocketType::Pull)?;
        assert_eq!(
            b.bind("inproc://errors-are-typed"),
            Err(Error::AddressInUse)
        );
        assert!(matches!(
            pull.subscribe(b""),
            Err(Error::InvalidArgument(_))
        ));
        assert!(matches!(pull.bind("bogus://x"), Err(Error::Zmq { .. })));
        assert!(matches!(
            pull.bind("inproc://a\0b"),
            Err(Error::InvalidArgument(_))
        ));
        Ok(())
    }
}
