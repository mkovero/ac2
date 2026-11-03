//! The fake daemon's data ROUTER: one peer per connection with its subscriptions, and the
//! keyframe requests it makes. Credit and windows are read but not enforced: the fake sends
//! every message at once, as a fast link would.

use std::collections::HashMap;

use ac2_proto::{DataUp, Subscription, Topic, decode_data_request};
use ac2_zmq::Socket;

/// Every data peer and what it subscribes to.
#[derive(Debug, Default)]
pub(super) struct DataPeers {
    peers: HashMap<Vec<u8>, Vec<Subscription>>,
}

impl DataPeers {
    /// Reads every queued request; returns the keyframe requests among them.
    pub(super) fn recv(&mut self, sock: &Socket) -> u32 {
        let mut asks = 0;
        while let Ok(Some(m)) = sock.try_recv() {
            let frames = m.into_frames();
            let [rid, body] = frames.as_slice() else {
                continue;
            };
            let Ok(req) = decode_data_request(body) else {
                continue;
            };
            let subs = self.peers.entry(rid.clone()).or_default();
            match req.msg {
                DataUp::Subscribe { sub } => {
                    if !subs.contains(&sub) {
                        subs.push(sub);
                    }
                }
                DataUp::Unsubscribe { sub } => subs.retain(|s| *s != sub),
                DataUp::Keyframe { .. } => asks += 1,
                DataUp::Window { .. } | DataUp::Credit { .. } => {}
            }
        }
        asks
    }

    /// `parts` to every peer that receives its topic (a topic that does not parse to every
    /// peer, so a damaged frame reaches the client under test). The daemon's keepalives to
    /// quiet peers that do not subscribe to `ka` are not modelled.
    pub(super) fn send(&mut self, sock: &Socket, parts: &[Vec<u8>]) {
        let Some(first) = parts.first() else {
            return;
        };
        let topic = Topic::parse(first).ok();
        let mut gone = Vec::new();
        for (rid, subs) in &self.peers {
            let wants = match topic {
                None => true,
                Some(t) => subs.iter().any(|s| s.matches(&t)),
            };
            if !wants {
                continue;
            }
            let mut v: Vec<&[u8]> = Vec::with_capacity(parts.len() + 1);
            v.push(rid);
            v.extend(parts.iter().map(Vec::as_slice));
            if let Err(ac2_zmq::Error::HostUnreachable) = sock.try_send(&v) {
                gone.push(rid.clone());
            }
        }
        for rid in gone {
            self.peers.remove(&rid);
        }
    }
}
