//! Log lines for ZAP decisions (network mode), rate-limited per refused key and address.
//!
//! A refused CURVE client sees only silence: libzmq drops the handshake and the client
//! retries. The daemon's log is the one place the operator can see who knocked, so every
//! refused key is logged with its fingerprint (what the client shows) and its key (what goes
//! into `authorized_clients`). The client reconnects every few hundred milliseconds, so each
//! key and address is logged at most once per [`REFUSAL_LOG_EVERY`], with the count of the
//! refusals in between.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac2_zmq::{DenyReason, Mechanism, PublicKey, Verdict, ZapDecision};

/// At most one log line per refused key and address in this interval.
pub const REFUSAL_LOG_EVERY: Duration = Duration::from_secs(10);

/// Distinct (key, address) pairs tracked; beyond this, pairs share one overflow slot so a
/// peer cycling through keys cannot grow the table or the log without bound.
const MAX_TRACKED: usize = 1024;

#[derive(Clone, Copy, Debug)]
struct Slot {
    last: Instant,
    suppressed: u64,
}

/// Who was refused: the key (none when the peer did not use CURVE) and the address.
type Peer = (Option<PublicKey>, String);

/// Decides which refusals get a log line.
#[derive(Debug)]
pub struct RefusalLimiter {
    every: Duration,
    cap: usize,
    slots: HashMap<Peer, Slot>,
    overflow: Option<Slot>,
}

impl RefusalLimiter {
    /// At most one line per peer per `every`.
    pub fn new(every: Duration) -> Self {
        Self::with_cap(every, MAX_TRACKED)
    }

    fn with_cap(every: Duration, cap: usize) -> Self {
        Self {
            every,
            cap,
            slots: HashMap::new(),
            overflow: None,
        }
    }

    /// A refusal of `key` from `address` at `now`: `Some(n)` when it is to be logged, `n`
    /// being the refusals of the same peer that were not logged since its last line.
    pub fn admit(&mut self, key: Option<PublicKey>, address: &str, now: Instant) -> Option<u64> {
        let every = self.every;
        let due = |s: &mut Slot| {
            if now.duration_since(s.last) >= every {
                let n = s.suppressed;
                *s = Slot {
                    last: now,
                    suppressed: 0,
                };
                Some(n)
            } else {
                s.suppressed += 1;
                None
            }
        };
        let peer = (key, address.to_owned());
        if let Some(s) = self.slots.get_mut(&peer) {
            return due(s);
        }
        if self.slots.len() >= self.cap {
            self.slots
                .retain(|_, s| now.duration_since(s.last) < every || s.suppressed > 0);
        }
        if self.slots.len() >= self.cap {
            return match &mut self.overflow {
                Some(s) => due(s),
                None => {
                    self.overflow = Some(Slot {
                        last: now,
                        suppressed: 0,
                    });
                    Some(0)
                }
            };
        }
        self.slots.insert(
            peer,
            Slot {
                last: now,
                suppressed: 0,
            },
        );
        Some(0)
    }
}

/// Refused peers kept for clients to see (`server.info`): enough to find a new client's
/// key among a few retrying peers, bounded against a peer cycling through keys.
pub const REFUSED_KEPT: usize = 32;

/// One refused peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// Its key; `None` when it did not use CURVE.
    pub key: Option<PublicKey>,
    /// Its address.
    pub address: String,
    /// Refusals since the daemon started.
    pub count: u64,
    /// The latest, wall-clock ns.
    pub last_at_ns: u64,
}

/// The peers refused lately, newest first; shared between the ZAP thread and the control
/// thread.
#[derive(Clone, Debug, Default)]
pub struct RefusedList(Arc<Mutex<VecDeque<Refused>>>);

impl RefusedList {
    fn note(&self, key: Option<PublicKey>, address: &str, at_ns: u64) {
        let mut l = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = match l.iter().position(|r| r.key == key && r.address == address) {
            Some(i) => l.remove(i).map_or(0, |r| r.count),
            None => 0,
        };
        l.push_front(Refused {
            key,
            address: address.to_owned(),
            count: count + 1,
            last_at_ns: at_ns,
        });
        l.truncate(REFUSED_KEPT);
    }

    /// The list, newest first.
    pub fn snapshot(&self) -> Vec<Refused> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// Forgets the refusals of `key` (it was just authorized).
    pub fn forget(&self, key: &PublicKey) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|r| r.key.as_ref() != Some(key));
    }
}

/// The audit hook of the daemon's ZAP handler: accepted clients at info, refused ones at
/// warn, rate-limited.
pub struct AuthLog {
    authorized_file: PathBuf,
    limiter: Mutex<RefusalLimiter>,
    refused: RefusedList,
}

impl AuthLog {
    /// Names `authorized_file` in refusal lines (where a key must go to be accepted).
    pub fn new(authorized_file: &Path) -> Self {
        Self {
            authorized_file: authorized_file.to_owned(),
            limiter: Mutex::new(RefusalLimiter::new(REFUSAL_LOG_EVERY)),
            refused: RefusedList::default(),
        }
    }

    /// The peers it refused lately (shared: it keeps filling).
    pub fn refused(&self) -> RefusedList {
        self.refused.clone()
    }

    /// Logs `d` (called on the ZAP thread for every handshake).
    pub fn record(&self, d: &ZapDecision) {
        let address = if d.address.is_empty() {
            "a local peer"
        } else {
            d.address.as_str()
        };
        let reason = match &d.verdict {
            Verdict::Allowed { user_id } => {
                tracing::info!(
                    target: "ac2d::auth",
                    "accepted client {user_id:?}{} from {address}",
                    d.client_key
                        .map(|k| format!(" (fingerprint {})", k.fingerprint()))
                        .unwrap_or_default()
                );
                return;
            }
            Verdict::Denied(r) => *r,
        };
        self.refused
            .note(d.client_key, &d.address, crate::util::wall_ns());
        let admitted = self
            .limiter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .admit(d.client_key, &d.address, Instant::now());
        let Some(suppressed) = admitted else {
            return;
        };
        let line = refusal_line(d, reason, address, &self.authorized_file, suppressed);
        tracing::warn!(target: "ac2d::auth", "{line}");
    }
}

/// The text of one refusal line.
fn refusal_line(
    d: &ZapDecision,
    reason: DenyReason,
    address: &str,
    authorized_file: &Path,
    suppressed: u64,
) -> String {
    let mut s = match (reason, d.client_key) {
        (DenyReason::UnknownKey, Some(k)) => format!(
            "refused client key fingerprint {} from {address}: not in {}; if the client shows \
             this fingerprint, authorize it by adding the line `<name> {}` there and restarting \
             ac2d",
            k.fingerprint(),
            authorized_file.display(),
            k.to_z85()
        ),
        (DenyReason::NotCurve, _) => format!(
            "refused a client without CURVE ({}) from {address}: network mode accepts paired \
             clients only",
            match &d.mechanism {
                Mechanism::Null => "NULL",
                Mechanism::Plain => "PLAIN",
                Mechanism::Curve => "CURVE",
                Mechanism::Other(m) => m.as_str(),
            }
        ),
        (r, k) => format!(
            "refused a client{} from {address}: {}",
            k.map(|k| format!(" key fingerprint {}", k.fingerprint()))
                .unwrap_or_default(),
            match r {
                DenyReason::MalformedRequest => "malformed authentication request",
                DenyReason::WrongDomain => "wrong authentication domain",
                DenyReason::Internal => "internal error while deciding (refused, failing closed)",
                DenyReason::UnknownKey | DenyReason::NotCurve => "not authorized",
            }
        ),
    };
    if suppressed > 0 {
        s.push_str(&format!(
            " ({suppressed} more refusal(s) of it in the last {} s not logged)",
            REFUSAL_LOG_EVERY.as_secs()
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(b: u8) -> Option<PublicKey> {
        Some(PublicKey::from_bytes([b; 32]))
    }

    #[test]
    fn one_line_per_key_and_address_per_interval() {
        let t0 = Instant::now();
        let s = |ms: u64| t0 + Duration::from_millis(ms);
        let mut l = RefusalLimiter::new(Duration::from_secs(10));
        assert_eq!(l.admit(key(1), "10.0.0.2", s(0)), Some(0));
        // A retrying client: quiet until the interval has passed, then the count.
        for ms in [300, 600, 9_999] {
            assert_eq!(l.admit(key(1), "10.0.0.2", s(ms)), None);
        }
        // Another key, or the same key from another address, is its own peer.
        assert_eq!(l.admit(key(2), "10.0.0.2", s(700)), Some(0));
        assert_eq!(l.admit(key(1), "10.0.0.3", s(800)), Some(0));
        assert_eq!(l.admit(None, "10.0.0.2", s(900)), Some(0));
        assert_eq!(l.admit(key(1), "10.0.0.2", s(10_000)), Some(3));
        assert_eq!(l.admit(key(1), "10.0.0.2", s(10_001)), None);
        assert_eq!(l.admit(key(1), "10.0.0.2", s(25_000)), Some(1));
        assert_eq!(l.admit(key(1), "10.0.0.2", s(40_000)), Some(0));
    }

    #[test]
    fn refused_list_counts_per_peer_newest_first_and_is_bounded() {
        let l = RefusedList::default();
        l.note(key(1), "a", 10);
        l.note(key(2), "a", 20);
        l.note(key(1), "a", 30);
        let s = l.snapshot();
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].key, s[0].count, s[0].last_at_ns), (key(1), 2, 30));
        assert_eq!((s[1].key, s[1].count), (key(2), 1));
        for b in 0..100u8 {
            l.note(key(b), "b", 40);
        }
        assert_eq!(l.snapshot().len(), REFUSED_KEPT);
        l.forget(&PublicKey::from_bytes([99; 32]));
        assert!(l.snapshot().iter().all(|r| r.key != key(99)));
    }

    #[test]
    fn many_keys_share_one_overflow_slot() {
        let t0 = Instant::now();
        let mut l = RefusalLimiter::with_cap(Duration::from_secs(10), 2);
        assert_eq!(l.admit(key(1), "a", t0), Some(0));
        assert_eq!(l.admit(key(2), "a", t0), Some(0));
        assert_eq!(l.admit(key(3), "a", t0), Some(0), "first overflow line");
        assert_eq!(l.admit(key(4), "a", t0), None);
        assert_eq!(l.admit(key(5), "a", t0), None);
        assert_eq!(l.slots.len(), 2);
        // Once the tracked peers are quiet for an interval their slots are reused.
        let later = t0 + Duration::from_secs(11);
        assert_eq!(l.admit(key(6), "a", later), Some(0));
        assert!(l.slots.contains_key(&(key(6), "a".to_owned())));
    }

    #[test]
    fn refusal_line_names_fingerprint_address_key_and_file() {
        let k = PublicKey::from_bytes([0; 32]);
        let d = ZapDecision {
            domain: "ac2".into(),
            address: "192.168.9.25".into(),
            mechanism: Mechanism::Curve,
            client_key: Some(k),
            verdict: Verdict::Denied(DenyReason::UnknownKey),
        };
        let line = refusal_line(
            &d,
            DenyReason::UnknownKey,
            "192.168.9.25",
            Path::new("/etc/ac2/authorized_clients"),
            4,
        );
        assert!(
            line.starts_with(
                "refused client key fingerprint 6668-7aad-f862-bd77-6c8f from 192.168.9.25: \
                 not in /etc/ac2/authorized_clients;"
            ),
            "{line}"
        );
        assert!(line.contains(&format!("`<name> {}`", k.to_z85())), "{line}");
        assert!(line.ends_with("(4 more refusal(s) of it in the last 10 s not logged)"));
    }
}
