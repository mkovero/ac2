//! Data socket: XPUB in the daemon (sees subscriptions), SUB in clients with a latest-wins
//! drain.

use std::collections::BTreeMap;

use crate::ctrl::{ServerSecurity, apply_server};
use crate::ffi;
use crate::proto::{Frame, decode_frame};
use crate::zmq::{Context, Received, Result, Socket, SocketType};

/// A subscription change reported by XPUB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubEvent {
    Subscribe(Vec<u8>),
    Unsubscribe(Vec<u8>),
}

impl SubEvent {
    /// XPUB delivers `[0x01 | 0x00][topic prefix]` as a single frame.
    pub fn parse(m: &Received) -> Option<Self> {
        let [f] = m.frames.as_slice() else {
            return None;
        };
        match f.split_first()? {
            (1, t) => Some(Self::Subscribe(t.to_vec())),
            (0, t) => Some(Self::Unsubscribe(t.to_vec())),
            _ => None,
        }
    }
}

/// Daemon data socket. `XPUB_VERBOSE` passes every subscribe (not only the first per topic),
/// so the daemon can count interest per topic and react to new subscribers (e.g. re-send
/// the latest frame).
pub fn bind_xpub(ctx: &Context, bind: &str, sec: &ServerSecurity, sndhwm: i32) -> Result<Socket> {
    let s = ctx.socket(SocketType::XPub)?;
    apply_server(&s, sec)?;
    s.set_int(ffi::ZMQ_XPUB_VERBOSE, 1)?;
    s.set_int(ffi::ZMQ_SNDHWM, sndhwm)?;
    s.bind(bind)?;
    Ok(s)
}

/// Result of one client drain pass.
#[derive(Debug, Default)]
pub struct Drained {
    /// Newest frame per topic (by `seq`) among everything that was queued.
    pub latest: BTreeMap<String, Frame>,
    /// Frames read in this pass, including the superseded ones.
    pub read: usize,
    /// Frames that failed validation and were dropped.
    pub malformed: usize,
    /// Oldest `seq` read in this pass (how stale the discarded backlog was).
    pub oldest_seq: Option<u64>,
}

/// Read everything currently queued without blocking and keep only the newest frame per
/// topic. Run before each render: a backlog built up while the client was stalled is
/// discarded in one pass instead of being replayed frame by frame.
pub fn drain_latest(sub: &Socket) -> Result<Drained> {
    let mut d = Drained::default();
    while let Some(m) = sub.try_recv()? {
        d.read += 1;
        let Ok(f) = decode_frame(&m.frames) else {
            d.malformed += 1;
            continue;
        };
        d.oldest_seq = Some(d.oldest_seq.map_or(f.header.seq, |o| o.min(f.header.seq)));
        match d.latest.get(&f.topic) {
            Some(old) if old.header.seq >= f.header.seq => {}
            _ => {
                d.latest.insert(f.topic.clone(), f);
            }
        }
    }
    Ok(d)
}
