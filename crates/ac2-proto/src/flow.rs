//! Flow control on the data socket: the daemon's ROUTER and one DEALER per client
//! connection (`docs/protocol.md` §5.8).
//!
//! A client states a byte window; the daemon keeps at most that many frame bytes in flight
//! towards it and the client returns credit for what it read. A frame that waits for credit
//! is replaced by the topic's newer one, so a slow link receives the newest frame of every
//! topic in turn and its delay stays the window divided by the link rate. Events,
//! keepalives and these notices travel outside the window in a bound of their own
//! ([`EVENT_BOUND`]): an event can be neither replaced nor dropped, and a keepalive must
//! reach a peer whose window frames fill.

use serde::{Deserialize, Serialize};

use crate::PROTO_VERSION;
use crate::ctrl::{CtrlError, check_version};
use crate::topic::{Subscription, Topic};

/// Window a client states unless it chooses another, bytes. Four of the largest frames (an
/// impulse response view, ≈ 13 kB) fit, so a frame never waits for the one before it on a
/// fast link, and on a starved one (2 Mbit/s) the standing delay is ≈ 0.26 s.
pub const DATA_WINDOW_DEFAULT: u32 = 64 * 1024;
/// Smallest window the daemon honours: above the largest frame a session sends, so every
/// topic fits the window on its own. A frame larger than the window still goes, alone.
pub const DATA_WINDOW_MIN: u32 = 20 * 1024;
/// Largest window the daemon honours. With [`FRAME_CHARGE_MIN`] it bounds the frames in
/// flight per peer, which sizes the daemon's send queue so it never refuses a credited one.
pub const DATA_WINDOW_MAX: u32 = 256 * 1024;
/// Least a frame costs of the window, bytes: a frame message of fewer bytes (a small delta)
/// is charged this much, on both ends, so a window never holds more than
/// `window ÷ FRAME_CHARGE_MIN` frames.
pub const FRAME_CHARGE_MIN: u32 = 64;
/// Events, keepalives and notices queued per peer, unacknowledged. Past it a peer's events
/// stop, and once it has room again it is sent `resync_required` before anything else.
pub const EVENT_BOUND: u32 = 256;
/// First part of a notice: `[flow][msgpack notice]`.
pub const FLOW_TOPIC: &[u8] = b"flow";

/// What a frame message of `bytes` (every part) costs of the window.
pub fn frame_charge(bytes: usize) -> u32 {
    u32::try_from(bytes)
        .unwrap_or(u32::MAX)
        .max(FRAME_CHARGE_MIN)
}

/// Whether a data-socket message whose first part is `topic` travels outside the window
/// (an event, a keepalive, a notice) rather than as a frame.
pub fn outside_window(topic: &[u8]) -> bool {
    topic == FLOW_TOPIC || matches!(Topic::parse(topic), Ok(Topic::Evt | Topic::Ka))
}

/// A message from a client on its data DEALER.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataRequest {
    /// Protocol version.
    pub v: u16,
    /// What it says.
    pub msg: DataUp,
}

impl DataRequest {
    /// `msg` at this build's version.
    pub fn new(msg: DataUp) -> Self {
        Self {
            v: PROTO_VERSION,
            msg,
        }
    }
}

/// What a client tells the daemon on its data DEALER.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DataUp {
    /// The window this connection grants, bytes (clamped to [`DATA_WINDOW_MIN`] …
    /// [`DATA_WINDOW_MAX`]); [`DATA_WINDOW_DEFAULT`] until stated.
    Window {
        /// Bytes.
        bytes: u32,
    },
    /// Receive the topics under `sub`; the newest frame of each goes to this connection at
    /// once, as a keyframe. Subscribing again to a subscription held changes nothing.
    Subscribe {
        /// Subscription.
        sub: Subscription,
    },
    /// Stop receiving `sub`.
    Unsubscribe {
        /// Subscription.
        sub: Subscription,
    },
    /// The next frame of `topic` to this connection goes as a keyframe (a delta chain
    /// broke); one request per topic per 250 ms is honoured.
    Keyframe {
        /// Topic.
        topic: Topic,
    },
    /// Read since the last credit: frame bytes (each frame at least
    /// [`FRAME_CHARGE_MIN`]) and messages outside the window.
    Credit {
        /// Frame bytes.
        bytes: u32,
        /// Events, keepalives and notices.
        events: u32,
    },
}

/// What the daemon tells one connection on `flow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FlowNotice {
    /// Events were dropped past [`EVENT_BOUND`]: take a new `state.snapshot`.
    ResyncRequired,
    /// The daemon holds no state for this connection (it was dropped as stalled): state the
    /// window and every subscription again.
    Rejoin,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoticeBody {
    v: u16,
    msg: FlowNotice,
}

/// Encode a data request.
pub fn encode_data_request(r: &DataRequest) -> Result<Vec<u8>, CtrlError> {
    rmp_serde::to_vec_named(r).map_err(|e| CtrlError::Encode(e.to_string()))
}

/// Decode a data request; the version is checked before the body.
pub fn decode_data_request(b: &[u8]) -> Result<DataRequest, CtrlError> {
    check_version(b)?;
    rmp_serde::from_slice(b).map_err(|e| CtrlError::Malformed(e.to_string()))
}

/// A notice as its two parts, `[flow][body]`.
pub fn encode_notice(n: FlowNotice) -> Result<Vec<Vec<u8>>, CtrlError> {
    let body = rmp_serde::to_vec_named(&NoticeBody {
        v: PROTO_VERSION,
        msg: n,
    })
    .map_err(|e| CtrlError::Encode(e.to_string()))?;
    Ok(vec![FLOW_TOPIC.to_vec(), body])
}

/// Decode a notice's body (the part after `flow`).
pub fn decode_notice(body: &[u8]) -> Result<FlowNotice, CtrlError> {
    check_version(body)?;
    let b: NoticeBody =
        rmp_serde::from_slice(body).map_err(|e| CtrlError::Malformed(e.to_string()))?;
    Ok(b.msg)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::topic::Stream;
    use crate::units::MeasId;

    #[test]
    fn requests_round_trip() {
        let t = Topic::Data {
            meas: MeasId(3),
            stream: Stream::Tf,
        };
        for msg in [
            DataUp::Window { bytes: 65_536 },
            DataUp::Subscribe {
                sub: Subscription::Meas(MeasId(12)),
            },
            DataUp::Unsubscribe {
                sub: Subscription::AllData,
            },
            DataUp::Subscribe {
                sub: Subscription::Topic(Topic::Evt),
            },
            DataUp::Keyframe { topic: t },
            DataUp::Credit {
                bytes: 9_000,
                events: 3,
            },
        ] {
            let r = DataRequest::new(msg);
            let b = encode_data_request(&r).unwrap();
            assert_eq!(decode_data_request(&b).unwrap(), r);
        }
        let mut old = DataRequest::new(DataUp::Window { bytes: 1 });
        old.v = PROTO_VERSION - 1;
        let b = encode_data_request(&old).unwrap();
        assert!(matches!(
            decode_data_request(&b),
            Err(CtrlError::VersionMismatch { .. })
        ));
        for n in [FlowNotice::ResyncRequired, FlowNotice::Rejoin] {
            let p = encode_notice(n).unwrap();
            assert_eq!(p[0], FLOW_TOPIC);
            assert_eq!(decode_notice(&p[1]).unwrap(), n);
        }
    }

    #[test]
    fn charges_and_classes() {
        assert_eq!(frame_charge(10), FRAME_CHARGE_MIN);
        assert_eq!(frame_charge(13_000), 13_000);
        assert!(outside_window(b"evt"));
        assert!(outside_window(b"ka"));
        assert!(outside_window(FLOW_TOPIC));
        assert!(!outside_window(b"d/1/tf"));
        assert!(!outside_window(b"session/levels"));
    }
}
