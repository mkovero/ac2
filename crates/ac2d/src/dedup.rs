//! Per-client request-id dedup: a request id executes once. A retry while it runs waits for
//! the same reply; a retry after it finished gets the stored reply.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use ac2_proto::units::{ClientId, RequestId};

use crate::config::DedupLimits;

/// One request id of a client.
#[derive(Debug)]
enum Slot {
    /// Still running: the routing ids of every copy of the request received, each owed the
    /// reply when it is known.
    Pending(Vec<Vec<u8>>),
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
        self.slots.values().any(|s| matches!(s, Slot::Pending(_)))
    }
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

/// Remembered replies per client.
#[derive(Debug)]
pub(crate) struct Dedup {
    clients: HashMap<ClientId, Window>,
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

    /// Admits request `id` from `client`, received on `routing_id`. A new id is reserved
    /// before anything executes, so a copy arriving while it runs (a retry after a lost or
    /// slow reply) attaches to it instead of executing again.
    pub(crate) fn admit(
        &mut self,
        client: &ClientId,
        id: RequestId,
        routing_id: &[u8],
        now: Instant,
    ) -> Admit {
        let limits = self.limits;
        let w = self.clients.entry(client.clone()).or_default();
        Self::evict(w, limits, now);
        match w.slots.get_mut(&id) {
            Some(Slot::Done(reply)) => Admit::Stored(reply.clone()),
            Some(Slot::Pending(waiters)) => {
                waiters.push(routing_id.to_vec());
                Admit::Attached
            }
            None => {
                w.slots.insert(id, Slot::Pending(vec![routing_id.to_vec()]));
                Admit::Execute
            }
        }
    }

    /// Stores the reply to `id`, reserved by the request received on `routing_id`, and
    /// returns the routing ids owed it, one per copy of the request received. Empty when that
    /// reservation is gone (the client was forgotten meanwhile, and its name may since have
    /// reserved the same id from another connection): the reply is then not stored, and only
    /// the asking connection is owed it.
    pub(crate) fn complete(
        &mut self,
        client: &ClientId,
        id: RequestId,
        routing_id: &[u8],
        reply: &[u8],
        now: Instant,
    ) -> Vec<Vec<u8>> {
        let limits = self.limits;
        let Some(w) = self.clients.get_mut(client) else {
            return Vec::new();
        };
        let ours = matches!(
            w.slots.get(&id),
            Some(Slot::Pending(waiters)) if waiters.first().is_some_and(|r| r == routing_id)
        );
        if !ours {
            return Vec::new();
        }
        let waiters = match w.slots.insert(id, Slot::Done(reply.to_vec())) {
            Some(Slot::Pending(waiters)) => waiters,
            _ => Vec::new(),
        };
        w.order.push_back((id, now));
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

    /// Drops every reply and reservation of `client`. A revoked name can be given to a
    /// different key, whose requests must neither be answered with replies meant for the old
    /// one nor attach to the old one's running work. Work already running still answers the
    /// connection that asked, but its reply is not stored (see [`Self::complete`]).
    pub(crate) fn forget(&mut self, client: &ClientId) {
        self.clients.remove(client);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn done(d: &mut Dedup, c: &ClientId, id: u64, reply: u8, t: Instant) {
        assert_eq!(d.admit(c, RequestId(id), b"r", t), Admit::Execute);
        assert_eq!(
            d.complete(c, RequestId(id), b"r", &[reply], t),
            vec![b"r".to_vec()]
        );
    }

    #[test]
    fn window_by_count_and_age() {
        let mut d = Dedup::new(DedupLimits {
            ids: 2,
            age: Duration::from_secs(30),
        });
        let c = ClientId("a".into());
        let t0 = Instant::now();
        done(&mut d, &c, 1, 1, t0);
        done(&mut d, &c, 2, 2, t0);
        assert_eq!(d.admit(&c, RequestId(1), b"r", t0), Admit::Stored(vec![1]));
        done(&mut d, &c, 3, 3, t0);
        assert_eq!(d.admit(&c, RequestId(3), b"r", t0), Admit::Stored(vec![3]));
        assert_eq!(d.admit(&c, RequestId(1), b"r", t0), Admit::Execute);
        let late = t0 + Duration::from_secs(31);
        assert_eq!(d.admit(&c, RequestId(3), b"r", late), Admit::Execute);
        assert_eq!(
            d.admit(&ClientId("b".into()), RequestId(3), b"r", t0),
            Admit::Execute
        );
    }

    #[test]
    fn a_copy_while_pending_attaches_and_is_answered_too() {
        let mut d = Dedup::new(DedupLimits {
            ids: 1,
            age: Duration::from_secs(30),
        });
        let c = ClientId("a".into());
        let t0 = Instant::now();
        assert_eq!(d.admit(&c, RequestId(1), b"x", t0), Admit::Execute);
        // Neither other answered ids nor time evict a request that has not replied.
        done(&mut d, &c, 2, 2, t0);
        done(&mut d, &c, 3, 3, t0);
        let late = t0 + Duration::from_secs(300);
        assert_eq!(d.admit(&c, RequestId(1), b"y", late), Admit::Attached);
        assert_eq!(
            d.complete(&c, RequestId(1), b"x", &[9], late),
            vec![b"x".to_vec(), b"y".to_vec()]
        );
        assert_eq!(
            d.admit(&c, RequestId(1), b"z", late),
            Admit::Stored(vec![9])
        );
        // A second completion of the same id owes nobody and changes nothing.
        assert!(d.complete(&c, RequestId(1), b"x", &[7], late).is_empty());
        assert_eq!(
            d.admit(&c, RequestId(1), b"z", late),
            Admit::Stored(vec![9])
        );
    }

    #[test]
    fn forget_drops_reservations() {
        let mut d = Dedup::new(DedupLimits {
            ids: 8,
            age: Duration::from_secs(30),
        });
        let c = ClientId("a".into());
        let t0 = Instant::now();
        assert_eq!(d.admit(&c, RequestId(1), b"old", t0), Admit::Execute);
        d.forget(&c);
        // The name's next holder executes its own request 1; the old one's late reply
        // neither completes that reservation nor reaches the new connection.
        assert_eq!(d.admit(&c, RequestId(1), b"new", t0), Admit::Execute);
        assert!(d.complete(&c, RequestId(1), b"old", &[1], t0).is_empty());
        assert_eq!(d.admit(&c, RequestId(1), b"new", t0), Admit::Attached);
        assert_eq!(
            d.complete(&c, RequestId(1), b"new", &[2], t0),
            vec![b"new".to_vec(), b"new".to_vec()]
        );
        assert_eq!(
            d.admit(&c, RequestId(1), b"new", t0),
            Admit::Stored(vec![2])
        );
    }
}
