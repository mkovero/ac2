//! Refused requests, warned about once per client.
//!
//! A client on another protocol version retries at once and reconnects for each try, so its
//! routing id is new every time and a warning per request buries the rest of the journal. The
//! client is known by its CURVE key instead; one without a key (a local connection) by the
//! reason alone. The reason is a fixed word per kind of refusal, never text the client
//! chose: a client must not be able to name a new reason per request.

use std::collections::HashSet;

use ac2_zmq::PublicKey;

/// Distinct refusals remembered: a bound on memory. Past it nothing is forgotten and every
/// further refusal is a debug line, so a stream of new keys or reasons can neither grow the
/// set nor start it over to have its warnings repeated.
const CAP: usize = 256;

/// Most characters of a decoder's message in a log line: it quotes what it could not read.
pub(crate) const LOGGED_ERROR_CHARS: usize = 160;

#[derive(Debug, Default)]
pub(crate) struct Refusals {
    seen: HashSet<(Option<PublicKey>, &'static str)>,
    full_said: bool,
}

impl Refusals {
    /// Whether this is the first refusal of `client` for `why`: the first is a warning, the
    /// repeats are debug lines.
    pub(crate) fn first(&mut self, client: Option<PublicKey>, why: &'static str) -> bool {
        let k = (client, why);
        if self.seen.contains(&k) {
            return false;
        }
        if self.seen.len() >= CAP {
            if !self.full_said {
                self.full_said = true;
                tracing::warn!("{CAP} kinds of refusal warned about; further ones at debug");
            }
            return false;
        }
        self.seen.insert(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(b: u8) -> Option<PublicKey> {
        Some(PublicKey::from_bytes([b; 32]))
    }

    #[test]
    fn once_per_client_and_reason() {
        let mut r = Refusals::default();
        assert!(r.first(key(1), "version"));
        assert!(!r.first(key(1), "version"));
        assert!(r.first(key(2), "version"));
        assert!(r.first(key(1), "revoked"));
        assert!(r.first(None, "version"));
        assert!(!r.first(None, "version"));
    }

    /// Full, it stays full: what it warned about is never warned about again.
    #[test]
    fn bounded_and_never_starts_over() {
        let mut r = Refusals::default();
        for i in 0..CAP as u64 {
            let mut b = [1u8; 32];
            b[..8].copy_from_slice(&i.to_le_bytes());
            assert!(r.first(Some(PublicKey::from_bytes(b)), "version"));
        }
        assert!(!r.first(key(200), "revoked"), "full: debug only");
        let mut first = [1u8; 32];
        first[..8].copy_from_slice(&0u64.to_le_bytes());
        assert!(
            !r.first(Some(PublicKey::from_bytes(first)), "version"),
            "never warned twice"
        );
        assert_eq!(r.seen.len(), CAP);
    }
}
