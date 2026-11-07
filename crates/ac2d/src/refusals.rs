//! Refused requests, warned about once per client.
//!
//! A client on another protocol version retries at once and reconnects for each try, so its
//! routing id is new every time and a warning per request buries the rest of the journal. The
//! client is known by its CURVE key instead; one without a key (a local connection) by the
//! reason alone.

use std::collections::HashSet;

use ac2_zmq::PublicKey;

/// Distinct refusals remembered before the set starts over: a bound on memory against a
/// stream of new keys, at the price of one more warning each after it.
const CAP: usize = 256;

#[derive(Debug, Default)]
pub(crate) struct Refusals {
    seen: HashSet<(Option<PublicKey>, String)>,
}

impl Refusals {
    /// Whether this is the first refusal of `client` for `why`: the first is a warning, the
    /// repeats are debug lines.
    pub(crate) fn first(&mut self, client: Option<PublicKey>, why: &str) -> bool {
        let k = (client, why.to_owned());
        if self.seen.contains(&k) {
            return false;
        }
        if self.seen.len() >= CAP {
            self.seen.clear();
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
        assert!(r.first(key(1), "protocol version 83"));
        assert!(!r.first(key(1), "protocol version 83"));
        assert!(r.first(key(2), "protocol version 83"));
        assert!(r.first(key(1), "protocol version 82"));
        assert!(r.first(None, "protocol version 83"));
        assert!(!r.first(None, "protocol version 83"));
    }

    #[test]
    fn bounded() {
        let mut r = Refusals::default();
        for i in 0..=CAP {
            assert!(r.first(None, &i.to_string()));
        }
        assert!(r.seen.len() <= CAP);
    }
}
