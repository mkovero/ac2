//! Per-client request-id dedup: a request id executes once. A retry while it runs waits for
//! the same reply; a retry after it finished gets the stored reply, unless the request only
//! reads ([`Retry::Runs`]): that one runs again.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use ac2_proto::units::RequestId;

use crate::config::DedupLimits;
use crate::route::{Peer, Route};

/// One request id of a client.
#[derive(Debug)]
enum Slot {
    /// Still running: the routes of every copy of the request received, each owed the
    /// reply when it is known, and what a retry after it finished gets.
    Pending(Vec<Route>, Retry),
    /// Answered: the encoded reply.
    Done(Vec<u8>),
}

#[derive(Debug, Default)]
struct Window {
    /// Answered ids in the order they were answered; pending ids are not in it, so neither
    /// the count nor the age limit can drop a request that has not replied yet.
    order: VecDeque<(RequestId, Instant)>,
    slots: HashMap<RequestId, Slot>,
}

impl Window {
    fn pending(&self) -> bool {
        self.slots.values().any(|s| matches!(s, Slot::Pending(..)))
    }
}

/// What a retry of a request gets once it has been answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Retry {
    /// The stored reply: running it again would act twice.
    Stored,
    /// A new run: it only reads, so running it again acts on nothing, and its reply (which
    /// may be megabytes) is not kept for the window.
    Runs,
}

/// What to do with an incoming request.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Admit {
    /// A new id, now reserved: execute it and [`Dedup::complete`] it.
    Execute,
    /// The id is running; this copy gets the reply when it completes. Do nothing now.
    Attached,
    /// The id was answered: send this stored reply.
    Stored(Vec<u8>),
}

/// Remembered replies per client: a local connection by its route, a network one by its
/// key, so a client that retries a request across a reconnect (a new routing id, perhaps
/// a new network pair) gets the stored reply instead of running it twice.
#[derive(Debug)]
pub(crate) struct Dedup {
    clients: HashMap<Peer, Window>,
    limits: DedupLimits,
}

impl Dedup {
    pub(crate) fn new(limits: DedupLimits) -> Self {
        Self {
            clients: HashMap::new(),
            limits,
        }
    }

    fn evict(w: &mut Window, limits: DedupLimits, now: Instant) {
        while w.order.len() > limits.ids
            || w.order
                .front()
                .is_some_and(|(_, t)| now.saturating_duration_since(*t) > limits.age)
        {
            if let Some((id, _)) = w.order.pop_front() {
                w.slots.remove(&id);
            }
        }
    }

    /// Admits request `id` from `client`, received on `route`. A new id is reserved
    /// before anything executes, so a copy arriving while it runs (a retry after a lost or
    /// slow reply) attaches to it instead of executing again.
    pub(crate) fn admit(
        &mut self,
        client: &Peer,
        id: RequestId,
        route: &Route,
        now: Instant,
        retry: Retry,
    ) -> Admit {
        let limits = self.limits;
        let w = self.clients.entry(client.clone()).or_default();
        Self::evict(w, limits, now);
        match w.slots.get_mut(&id) {
            Some(Slot::Done(reply)) => Admit::Stored(reply.clone()),
            Some(Slot::Pending(waiters, _)) => {
                waiters.push(route.clone());
                Admit::Attached
            }
            None => {
                w.slots
                    .insert(id, Slot::Pending(vec![route.clone()], retry));
                Admit::Execute
            }
        }
    }

    /// Stores the reply to `id`, reserved by the request received on `route`, and
    /// returns the routes owed it, one per copy of the request received. Empty when that
    /// reservation is gone (the client was forgotten meanwhile, and its name may since have
    /// reserved the same id from another connection): the reply is then not stored, and only
    /// the asking connection is owed it.
    pub(crate) fn complete(
        &mut self,
        client: &Peer,
        id: RequestId,
        route: &Route,
        reply: &[u8],
        now: Instant,
    ) -> Vec<Route> {
        let limits = self.limits;
        let Some(w) = self.clients.get_mut(client) else {
            return Vec::new();
        };
        let retry = match w.slots.get(&id) {
            Some(Slot::Pending(waiters, retry)) if waiters.first().is_some_and(|r| r == route) => {
                *retry
            }
            _ => return Vec::new(),
        };
        let waiters = if retry == Retry::Runs {
            // Every copy received while it ran is answered; a later one runs anew.
            match w.slots.remove(&id) {
                Some(Slot::Pending(waiters, _)) => waiters,
                _ => Vec::new(),
            }
        } else {
            let waiters = match w.slots.insert(id, Slot::Done(reply.to_vec())) {
                Some(Slot::Pending(waiters, _)) => waiters,
                _ => Vec::new(),
            };
            w.order.push_back((id, now));
            waiters
        };
        Self::evict(w, limits, now);
        // Forget clients with nothing pending and nothing recent, so departed peers do not
        // accumulate.
        self.clients.retain(|_, w| {
            w.pending()
                || w.order
                    .back()
                    .is_some_and(|(_, t)| now.saturating_duration_since(*t) <= limits.age)
        });
        waiters
    }

    /// Drops every reply and reservation of the network client `name`, revoked. Work
    /// already running still answers the connection that asked, but its reply is not
    /// stored (see [`Self::complete`]).
    pub(crate) fn forget_name(&mut self, name: &str) {
        self.clients
            .retain(|p, _| p.curve().is_none_or(|(n, _)| n != name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::units::ClientId;

    fn r(id: &[u8]) -> Route {
        Route {
            origin: crate::route::Origin::Local,
            id: id.to_vec(),
        }
    }

    fn peer(name: &str) -> Peer {
        Peer::Curve {
            id: ClientId(name.into()),
            key: ac2_zmq::PublicKey::from_bytes([1; 32]),
        }
    }
    use std::time::Duration;

    fn done(d: &mut Dedup, c: &Peer, id: u64, reply: u8, t: Instant) {
        assert_eq!(
            d.admit(c, RequestId(id), &r(b"r"), t, Retry::Stored),
            Admit::Execute
        );
        assert_eq!(
            d.complete(c, RequestId(id), &r(b"r"), &[reply], t),
            vec![r(b"r")]
        );
    }

    #[test]
    fn a_read_answers_every_copy_and_is_not_stored() {
        let mut d = Dedup::new(DedupLimits {
            ids: 4,
            age: Duration::from_secs(30),
        });
        let c = peer("a");
        let t0 = Instant::now();
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"x"), t0, Retry::Runs),
            Admit::Execute
        );
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"y"), t0, Retry::Runs),
            Admit::Attached
        );
        assert_eq!(
            d.complete(&c, RequestId(1), &r(b"x"), &[5], t0),
            vec![r(b"x"), r(b"y")]
        );
        // Nothing kept: a retry runs again, and a client that only read is forgotten.
        assert!(d.clients.is_empty());
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"z"), t0, Retry::Runs),
            Admit::Execute
        );
    }

    #[test]
    fn window_by_count_and_age() {
        let mut d = Dedup::new(DedupLimits {
            ids: 2,
            age: Duration::from_secs(30),
        });
        let c = peer("a");
        let t0 = Instant::now();
        done(&mut d, &c, 1, 1, t0);
        done(&mut d, &c, 2, 2, t0);
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"r"), t0, Retry::Stored),
            Admit::Stored(vec![1])
        );
        done(&mut d, &c, 3, 3, t0);
        assert_eq!(
            d.admit(&c, RequestId(3), &r(b"r"), t0, Retry::Stored),
            Admit::Stored(vec![3])
        );
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"r"), t0, Retry::Stored),
            Admit::Execute
        );
        let late = t0 + Duration::from_secs(31);
        assert_eq!(
            d.admit(&c, RequestId(3), &r(b"r"), late, Retry::Stored),
            Admit::Execute
        );
        assert_eq!(
            d.admit(&peer("b"), RequestId(3), &r(b"r"), t0, Retry::Stored),
            Admit::Execute
        );
    }

    #[test]
    fn a_copy_while_pending_attaches_and_is_answered_too() {
        let mut d = Dedup::new(DedupLimits {
            ids: 1,
            age: Duration::from_secs(30),
        });
        let c = peer("a");
        let t0 = Instant::now();
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"x"), t0, Retry::Stored),
            Admit::Execute
        );
        // Neither other answered ids nor time evict a request that has not replied.
        done(&mut d, &c, 2, 2, t0);
        done(&mut d, &c, 3, 3, t0);
        let late = t0 + Duration::from_secs(300);
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"y"), late, Retry::Stored),
            Admit::Attached
        );
        assert_eq!(
            d.complete(&c, RequestId(1), &r(b"x"), &[9], late),
            vec![r(b"x"), r(b"y")]
        );
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"z"), late, Retry::Stored),
            Admit::Stored(vec![9])
        );
        // A second completion of the same id owes nobody and changes nothing.
        assert!(
            d.complete(&c, RequestId(1), &r(b"x"), &[7], late)
                .is_empty()
        );
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"z"), late, Retry::Stored),
            Admit::Stored(vec![9])
        );
    }

    #[test]
    fn forget_drops_reservations() {
        let mut d = Dedup::new(DedupLimits {
            ids: 8,
            age: Duration::from_secs(30),
        });
        let c = peer("a");
        let t0 = Instant::now();
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"old"), t0, Retry::Stored),
            Admit::Execute
        );
        d.forget_name("a");
        // The name's next holder executes its own request 1; the old one's late reply
        // neither completes that reservation nor reaches the new connection.
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"new"), t0, Retry::Stored),
            Admit::Execute
        );
        assert!(
            d.complete(&c, RequestId(1), &r(b"old"), &[1], t0)
                .is_empty()
        );
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"new"), t0, Retry::Stored),
            Admit::Attached
        );
        assert_eq!(
            d.complete(&c, RequestId(1), &r(b"new"), &[2], t0),
            vec![r(b"new"), r(b"new")]
        );
        assert_eq!(
            d.admit(&c, RequestId(1), &r(b"new"), t0, Retry::Stored),
            Admit::Stored(vec![2])
        );
    }

    #[test]
    fn a_network_client_retrying_on_a_new_pair_gets_the_stored_reply() {
        use crate::route::{NetEpoch, Origin};
        let mut d = Dedup::new(DedupLimits::default());
        let c = peer("a");
        let on = |epoch, id: &[u8]| Route {
            origin: Origin::Network(NetEpoch(epoch)),
            id: id.to_vec(),
        };
        let t0 = Instant::now();
        assert_eq!(
            d.admit(&c, RequestId(4), &on(1, b"x"), t0, Retry::Stored),
            Admit::Execute
        );
        assert_eq!(
            d.complete(&c, RequestId(4), &on(1, b"x"), &[5], t0),
            vec![on(1, b"x")]
        );
        assert_eq!(
            d.admit(&c, RequestId(4), &on(2, b"y"), t0, Retry::Stored),
            Admit::Stored(vec![5])
        );
        // A local connection is its route: another one is another client.
        let local = Peer::local(b"x");
        assert_eq!(
            d.admit(&local, RequestId(4), &r(b"x"), t0, Retry::Stored),
            Admit::Execute
        );
    }
}
