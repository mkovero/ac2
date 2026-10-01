//! Small helpers: clocks, randomness, typed errors.

use std::time::{SystemTime, UNIX_EPOCH};

use ac2_proto::{ErrorCode, ErrorDetail, ProtoError};

/// Daemon wall clock, Unix nanoseconds.
pub(crate) fn wall_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // The OS RNG failing means the platform is unusable for key material anyway; fall back
    // to a time-derived value so ids stay distinct (tokens are never derived from this path
    // silently: it is logged).
    if let Err(e) = getrandom::fill(&mut b) {
        tracing::error!("OS random source failed ({e}); using a clock-derived value");
        let t = wall_ns().to_le_bytes();
        for (i, v) in b.iter_mut().enumerate() {
            *v = t[i % t.len()] ^ (i as u8).wrapping_mul(151);
        }
    }
    b
}

/// A random u64 (incarnation, seeds).
pub(crate) fn random_u64() -> u64 {
    u64::from_le_bytes(random_bytes::<8>())
}

/// A random u128 (lease tokens).
pub(crate) fn random_u128() -> u128 {
    u128::from_le_bytes(random_bytes::<16>())
}

/// A typed protocol error.
pub(crate) fn perr(code: ErrorCode, msg: impl Into<String>) -> ProtoError {
    ProtoError {
        code,
        msg: msg.into(),
        detail: None,
    }
}

/// A typed protocol error with detail.
pub(crate) fn perr_detail(
    code: ErrorCode,
    msg: impl Into<String>,
    detail: ErrorDetail,
) -> ProtoError {
    ProtoError {
        code,
        msg: msg.into(),
        detail: Some(detail),
    }
}

/// Lower-case hex of `b`.
pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
