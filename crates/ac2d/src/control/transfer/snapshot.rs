//! A state too large for one ctrl reply: `state.snapshot` holds its bytes as a download
//! (`snapshot_transfer`) and the client fetches them in chunks; `state.since` whose events
//! would not fit answers `resync_required`, and the snapshot that follows goes this way if
//! it must.
//!
//! Snapshot transfers are held apart from the session transfers and outside their quota
//! (and the viewers' share of it): resyncing is what every client must be able to do, a
//! viewer included, and none may take an operator's slot for it. They are bounded instead
//! by their own rules: the bytes of one rev are shared by every client that asked at it, a
//! client holds at most [`PER_CLIENT`] and a newer one replaces its oldest, and each is
//! dropped [`HOLD`] after its last chunk (or its reply, if never fetched).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_proto::event::StateSnapshot;
use ac2_proto::model::Transfer;
use ac2_proto::units::{Blob, Rev, TransferId};
use ac2_proto::{
    ErrorCode, ErrorDetail, Event, MAX_CTRL_BYTES, ProtoError, ReplyBody, SnapshotTransfer,
};

use super::super::Control;
use super::CHUNK_BYTES;
use crate::route::Peer;
use crate::util::{internal, invalid, perr_detail};

/// A snapshot transfer is dropped this long after its last chunk, or after its reply.
pub(crate) const HOLD: Duration = Duration::from_secs(30);
/// Snapshot transfers one client holds: a retried `state.snapshot` runs again, and holding
/// two keeps the first reply's transfer valid when the retry ran it across a rev change.
pub(crate) const PER_CLIENT: usize = 2;
/// What a reply takes around its body (version, id, result tags, warnings): a body that
/// leaves less than this of [`MAX_CTRL_BYTES`] goes as a transfer.
const ENVELOPE_BYTES: usize = 4 << 10;
/// The largest body that goes inline in a reply.
pub(crate) const INLINE_BYTES: usize = MAX_CTRL_BYTES - ENVELOPE_BYTES;

/// One snapshot's bytes, shared by every client that asked at its rev.
#[derive(Debug)]
struct Bytes {
    data: Vec<u8>,
    sha256: String,
}

#[derive(Debug)]
struct Held {
    client: Peer,
    rev: Rev,
    /// Encoded as a network client is sent it (host paths cut to their last component).
    redacted: bool,
    bytes: Arc<Bytes>,
    born: Instant,
    last: Instant,
}

/// The snapshot transfers under way.
#[derive(Debug, Default)]
pub(crate) struct Snapshots {
    map: HashMap<TransferId, Held>,
}

impl Snapshots {
    /// Drops what no client fetched for [`HOLD`].
    pub(crate) fn tick(&mut self, now: Instant) {
        self.prune(now);
    }

    fn prune(&mut self, now: Instant) {
        self.map
            .retain(|_, h| now.saturating_duration_since(h.last) < HOLD);
    }

    fn shared(&self, rev: Rev, redacted: bool) -> Option<Arc<Bytes>> {
        self.map
            .values()
            .find(|h| h.rev == rev && h.redacted == redacted)
            .map(|h| h.bytes.clone())
    }

    fn add(&mut self, id: TransferId, client: &Peer, rev: Rev, redacted: bool, bytes: Arc<Bytes>) {
        let now = Instant::now();
        self.prune(now);
        loop {
            let mine = self.map.iter().filter(|(_, h)| h.client == *client);
            if mine.clone().count() < PER_CLIENT {
                break;
            }
            let oldest = mine.min_by_key(|(_, h)| h.born).map(|(id, _)| *id);
            if let Some(o) = oldest {
                self.map.remove(&o);
            }
        }
        self.map.insert(
            id,
            Held {
                client: client.clone(),
                rev,
                redacted,
                bytes,
                born: now,
                last: now,
            },
        );
    }

    /// `client`'s snapshot transfer `id`, touched; `None` when it is not one.
    fn get(&mut self, client: &Peer, id: TransferId) -> Option<&Held> {
        let now = Instant::now();
        self.prune(now);
        match self.map.get_mut(&id) {
            Some(h) if h.client == *client => {
                h.last = now;
                Some(h)
            }
            _ => None,
        }
    }

    /// `file.download_chunk` of a snapshot transfer; `None` when `id` is not one of
    /// `client`'s.
    pub(super) fn chunk(
        &mut self,
        client: &Peer,
        id: TransferId,
        offset: u64,
    ) -> Option<Result<ReplyBody, ProtoError>> {
        let data = &self.get(client, id)?.bytes.data;
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        Some(if start > data.len() {
            Err(invalid(format!(
                "download {id} is {} bytes; offset {offset}",
                data.len()
            )))
        } else {
            let end = (start + CHUNK_BYTES as usize).min(data.len());
            Ok(ReplyBody::Chunk(Blob(data[start..end].to_vec())))
        })
    }

    /// `file.download_end` of a snapshot transfer: dropped; `None` when `id` is not one of
    /// `client`'s. A snapshot is no session file: `saved_as` names nothing it could become.
    pub(super) fn end(
        &mut self,
        client: &Peer,
        id: TransferId,
        saved_as: Option<&str>,
    ) -> Option<Result<(), ProtoError>> {
        self.get(client, id)?;
        if saved_as.is_some() {
            return Some(Err(invalid(format!(
                "transfer {id} is a state snapshot, not a session: end it without saved_as"
            ))));
        }
        self.map.remove(&id);
        Some(Ok(()))
    }

    /// Drops every snapshot transfer of a network key whose role was taken down.
    pub(super) fn forget_key(&mut self, key: &ac2_zmq::PublicKey) {
        self.map
            .retain(|_, h| h.client.curve().is_none_or(|(_, k)| k != key));
    }
}

impl Control {
    /// `state.snapshot`: inline when it fits a ctrl reply, else held as a snapshot transfer.
    pub(in crate::control) fn state_snapshot(
        &mut self,
        client: &Peer,
    ) -> Result<ReplyBody, ProtoError> {
        let rev = self.store.rev();
        let redacted = client.curve().is_some();
        let bytes = match self.transfers.snapshots.shared(rev, redacted) {
            // Another client asked at this rev: too large then, the same bytes now.
            Some(b) => b,
            None => {
                let mut snap: StateSnapshot = self.store.snapshot(self.s.incarnation);
                if redacted {
                    super::super::redact::snapshot(&mut snap);
                }
                let data = ac2_proto::encode_snapshot(&snap)
                    .map_err(|e| internal(format!("snapshot not encodable: {e}")))?;
                if data.len() <= INLINE_BYTES {
                    return Ok(ReplyBody::Snapshot(Box::new(snap)));
                }
                tracing::info!(
                    "snapshot at rev {} is {} bytes, over one ctrl reply: sent as a transfer",
                    rev.0,
                    data.len()
                );
                let sha256 = ac2_archive::sha256_hex(&data);
                Arc::new(Bytes { data, sha256 })
            }
        };
        let id = self.transfers.next_id();
        let transfer = Transfer {
            transfer: id,
            file_name: SnapshotTransfer::file_name(rev),
            bytes: bytes.data.len() as u64,
            sha256: bytes.sha256.clone(),
            chunk_bytes: CHUNK_BYTES,
        };
        self.transfers
            .snapshots
            .add(id, client, rev, redacted, bytes);
        Ok(ReplyBody::SnapshotTransfer(SnapshotTransfer {
            rev,
            transfer,
        }))
    }

    /// `state.since`: the events after `rev`, or `resync_required` when they are no longer
    /// replayable or would not fit one ctrl reply (the snapshot goes as a transfer if it
    /// must; no event list is ever cut short).
    pub(in crate::control) fn state_since(&mut self, rev: Rev) -> Result<ReplyBody, ProtoError> {
        let resync = |oldest: Rev, why: String| {
            perr_detail(
                ErrorCode::ResyncRequired,
                why,
                ErrorDetail::Resync { oldest },
            )
        };
        let evs = self.store.since(rev, Instant::now()).map_err(|oldest| {
            resync(
                oldest,
                format!(
                    "events after rev {} are no longer replayable; take a snapshot",
                    rev.0
                ),
            )
        })?;
        let bytes = events_bytes(&evs);
        if bytes > INLINE_BYTES {
            let oldest = evs.first().map_or(Rev(rev.0 + 1), |e| e.rev);
            return Err(resync(
                oldest,
                format!(
                    "the {} events after rev {} are {bytes} bytes, over one ctrl reply; take \
                     a snapshot",
                    evs.len(),
                    rev.0
                ),
            ));
        }
        Ok(ReplyBody::Events(evs))
    }
}

/// The encoded size of `evs` as a reply's event list: each event's bytes and the list's
/// header. An event that does not encode counts as too large, so the client takes a
/// snapshot rather than a list without it.
fn events_bytes(evs: &[Event]) -> usize {
    const LIST_HEADER: usize = 5;
    evs.iter().fold(LIST_HEADER, |n, e| {
        n.saturating_add(ac2_proto::encode_event(e).map_or(usize::MAX, |b| b.len()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(n: u8) -> Arc<Bytes> {
        Arc::new(Bytes {
            data: vec![n; 3],
            sha256: String::new(),
        })
    }

    /// A client holds two; a third replaces its oldest, and other clients' are untouched.
    #[test]
    fn a_client_holds_two_and_a_third_replaces_the_older() {
        let mut s = Snapshots::default();
        let (a, b) = (Peer::local(b"a"), Peer::local(b"b"));
        s.add(TransferId(1), &a, Rev(1), false, bytes(1));
        s.add(TransferId(2), &b, Rev(1), false, bytes(1));
        s.add(TransferId(3), &a, Rev(2), false, bytes(2));
        s.add(TransferId(4), &a, Rev(3), false, bytes(3));
        assert!(s.get(&a, TransferId(1)).is_none(), "replaced");
        assert!(s.get(&a, TransferId(3)).is_some());
        assert!(s.get(&a, TransferId(4)).is_some());
        assert!(s.get(&b, TransferId(2)).is_some());
        assert!(s.get(&b, TransferId(3)).is_none(), "another client's");
    }

    /// Dropped a hold after its last chunk, however recently it began.
    #[test]
    fn a_snapshot_transfer_is_dropped_after_its_hold() {
        let mut s = Snapshots::default();
        let a = Peer::local(b"a");
        s.add(TransferId(1), &a, Rev(1), false, bytes(1));
        if let Some(old) = Instant::now().checked_sub(HOLD) {
            s.map.get_mut(&TransferId(1)).expect("held").last = old;
            assert!(s.get(&a, TransferId(1)).is_none());
        }
    }

    /// One rev's bytes are shared, redacted apart from the local pair's.
    #[test]
    fn bytes_are_shared_per_rev_and_redaction() {
        let mut s = Snapshots::default();
        let (a, b) = (Peer::local(b"a"), Peer::local(b"b"));
        s.add(TransferId(1), &a, Rev(7), false, bytes(1));
        let shared = s.shared(Rev(7), false).expect("shared");
        s.add(TransferId(2), &b, Rev(7), false, shared.clone());
        assert_eq!(Arc::strong_count(&shared), 3);
        assert!(s.shared(Rev(7), true).is_none());
        assert!(s.shared(Rev(8), false).is_none());
    }
}
