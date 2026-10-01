//! Per-client request-id dedup: a retried request id returns the stored reply and never
//! executes twice.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use ac2_proto::units::{ClientId, RequestId};

use crate::config::DedupLimits;

#[derive(Debug, Default)]
struct Window {
    order: VecDeque<(RequestId, Instant)>,
    replies: HashMap<RequestId, Vec<u8>>,
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
                w.replies.remove(&id);
            }
        }
    }

    /// The stored reply of `id`, if it is still inside the window.
    pub(crate) fn get(&mut self, client: &ClientId, id: RequestId, now: Instant) -> Option<&[u8]> {
        let limits = self.limits;
        let w = self.clients.get_mut(client)?;
        Self::evict(w, limits, now);
        w.replies.get(&id).map(Vec::as_slice)
    }

    /// Remembers the reply to `id`.
    pub(crate) fn insert(
        &mut self,
        client: &ClientId,
        id: RequestId,
        reply: Vec<u8>,
        now: Instant,
    ) {
        let limits = self.limits;
        let w = self.clients.entry(client.clone()).or_default();
        if w.replies.insert(id, reply).is_none() {
            w.order.push_back((id, now));
        }
        Self::evict(w, limits, now);
        // Forget clients whose window has emptied, so departed peers do not accumulate.
        self.clients.retain(|_, w| {
            w.order
                .back()
                .is_some_and(|(_, t)| now.saturating_duration_since(*t) <= limits.age)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn window_by_count_and_age() {
        let mut d = Dedup::new(DedupLimits {
            ids: 2,
            age: Duration::from_secs(30),
        });
        let c = ClientId("a".into());
        let t0 = Instant::now();
        d.insert(&c, RequestId(1), vec![1], t0);
        d.insert(&c, RequestId(2), vec![2], t0);
        assert_eq!(d.get(&c, RequestId(1), t0), Some(&[1u8][..]));
        d.insert(&c, RequestId(3), vec![3], t0);
        assert_eq!(d.get(&c, RequestId(1), t0), None);
        assert_eq!(d.get(&c, RequestId(3), t0), Some(&[3u8][..]));
        assert_eq!(d.get(&c, RequestId(3), t0 + Duration::from_secs(31)), None);
        assert_eq!(d.get(&ClientId("b".into()), RequestId(3), t0), None);
    }
}
