//! Who a request came from and where its reply goes, as the socket it arrived on says.
//!
//! The daemon serves two socket pairs: the local one (ipc or loopback, no authentication)
//! for its whole life, and the network one (CURVE) while the network is attached. Loopback
//! is also every ssh tunnel and port proxy, so neither an address nor a name decides
//! "local"; only the pair a message came in on does.

use ac2_proto::units::ClientId;
use ac2_zmq::PublicKey;

use crate::util::hex;

/// Counts attaches of the network pair (each enable and each automatic retry). A reply
/// addressed to an older one is dropped: libzmq hands out routing ids per socket, so a
/// client of the new pair may hold the id a client of the old one had.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct NetEpoch(pub(crate) u64);

/// The pair a connection is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Origin {
    Local,
    Network(NetEpoch),
}

/// Where a reply goes: the pair and the routing id on it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Route {
    pub(crate) origin: Origin,
    pub(crate) id: Vec<u8>,
}

const LOCAL: u8 = b'L';
const NETWORK: u8 = b'N';

impl Route {
    /// The route as one frame of the internal pipe.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(9 + self.id.len());
        match self.origin {
            Origin::Local => b.push(LOCAL),
            Origin::Network(e) => {
                b.push(NETWORK);
                b.extend_from_slice(&e.0.to_le_bytes());
            }
        }
        b.extend_from_slice(&self.id);
        b
    }

    /// [`Self::encode`] read back.
    pub(crate) fn decode(b: &[u8]) -> Option<Self> {
        let (&kind, rest) = b.split_first()?;
        match kind {
            LOCAL => Some(Self {
                origin: Origin::Local,
                id: rest.to_vec(),
            }),
            NETWORK => {
                let (e, id) = rest.split_first_chunk::<8>()?;
                Some(Self {
                    origin: Origin::Network(NetEpoch(u64::from_le_bytes(*e))),
                    id: id.to_vec(),
                })
            }
            _ => None,
        }
    }
}

/// The authenticated peer of a network (CURVE) connection, as the ZAP handler granted it
/// at the handshake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CurvePeer {
    /// The authorized name the key had at the handshake.
    pub(crate) name: String,
    /// The key the connection authenticated with.
    pub(crate) key: PublicKey,
}

/// Who sent a request. Leases, the audit and the dedup window compare on it, so a remote
/// key authorized under a name that looks like a local client's id never passes as one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Peer {
    /// On the local pair: one connection, named after its routing id.
    Local(ClientId),
    /// On the network pair: the authorized name and the key it connected with.
    Curve { id: ClientId, key: PublicKey },
}

impl Peer {
    /// The local connection with routing id `rid`.
    pub(crate) fn local(rid: &[u8]) -> Self {
        Self::Local(ClientId::local(&hex(rid)))
    }

    /// The id clients see (lease owner, audit, hello).
    pub(crate) fn id(&self) -> &ClientId {
        match self {
            Self::Local(id) | Self::Curve { id, .. } => id,
        }
    }

    /// The name and key of a network peer.
    pub(crate) fn curve(&self) -> Option<(&str, &PublicKey)> {
        match self {
            Self::Local(_) => None,
            Self::Curve { id, key } => Some((id.0.as_str(), key)),
        }
    }

    /// Whether it came in on the local pair: only such a peer may name files by path.
    pub(crate) fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }
}

impl From<CurvePeer> for Peer {
    fn from(c: CurvePeer) -> Self {
        Self::Curve {
            id: ClientId(c.name),
            key: c.key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_survives_the_pipe() {
        for origin in [Origin::Local, Origin::Network(NetEpoch(7))] {
            for id in [Vec::new(), b"\0abc".to_vec()] {
                let r = Route {
                    origin,
                    id: id.clone(),
                };
                assert_eq!(Route::decode(&r.encode()), Some(r));
            }
        }
        assert_eq!(Route::decode(b""), None);
        assert_eq!(Route::decode(b"N123"), None);
    }

    #[test]
    fn a_curve_name_like_a_local_id_is_not_local() {
        let local = Peer::local(b"\x01");
        let key = PublicKey::from_bytes([1; 32]);
        let curve = Peer::from(CurvePeer {
            name: local.id().0.clone(),
            key,
        });
        assert_eq!(local.id(), curve.id());
        assert_ne!(local, curve);
        assert!(local.is_local() && !curve.is_local());
    }
}
