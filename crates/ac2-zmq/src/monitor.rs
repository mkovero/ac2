//! Typed socket monitor events (`zmq_socket_monitor`).
//!
//! Refused handshakes are observable here without timing guesses: a ZAP denial arrives as
//! [`MonitorEvent::HandshakeFailedAuth`] on both ends, a mechanism mismatch or wrong server
//! key as [`MonitorEvent::HandshakeFailedProtocol`].

use std::time::{Duration, Instant};

use crate::error::Result;
use crate::socket::{Message, Socket};

/// One connection-level event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MonitorEvent {
    /// Outgoing connection established (before the ZMTP handshake).
    Connected,
    /// Synchronous connect failed; it is retried.
    ConnectDelayed,
    /// Reconnect scheduled after this interval.
    ConnectRetried {
        /// Delay before the next attempt.
        interval: Duration,
    },
    /// Bound and listening.
    Listening,
    /// Bind failed.
    BindFailed {
        /// errno of the failure.
        errno: i32,
    },
    /// Incoming connection accepted (before the ZMTP handshake).
    Accepted,
    /// Accepting an incoming connection failed.
    AcceptFailed {
        /// errno of the failure.
        errno: i32,
    },
    /// Connection closed.
    Closed,
    /// Closing the underlying socket failed.
    CloseFailed {
        /// errno of the failure.
        errno: i32,
    },
    /// Peer disconnected unexpectedly.
    Disconnected,
    /// The monitor was stopped.
    MonitorStopped,
    /// Handshake failed before the security mechanism could tell why (e.g. timeout).
    HandshakeFailedNoDetail {
        /// errno of the failure.
        errno: i32,
    },
    /// ZMTP handshake, including security mechanism and ZAP, completed.
    HandshakeSucceeded,
    /// Handshake failed on a protocol or mechanism error (NULL vs CURVE, wrong server key,
    /// malformed command). `code` is libzmq's `ZMQ_PROTOCOL_ERROR_*` value.
    HandshakeFailedProtocol {
        /// `ZMQ_PROTOCOL_ERROR_*` value.
        code: u32,
    },
    /// The ZAP handler refused the peer. `status` is the ZAP status code (400 for an
    /// unauthorized key).
    HandshakeFailedAuth {
        /// ZAP status code.
        status: u32,
    },
    /// An event this binding does not know.
    Other {
        /// Raw event bit.
        event: u16,
        /// Raw event value.
        value: u32,
    },
}

impl MonitorEvent {
    fn from_raw(event: u16, value: u32) -> Self {
        let errno = i32::try_from(value).unwrap_or(i32::MAX);
        match event {
            0x0001 => Self::Connected,
            0x0002 => Self::ConnectDelayed,
            0x0004 => Self::ConnectRetried {
                interval: Duration::from_millis(u64::from(value)),
            },
            0x0008 => Self::Listening,
            0x0010 => Self::BindFailed { errno },
            0x0020 => Self::Accepted,
            0x0040 => Self::AcceptFailed { errno },
            0x0080 => Self::Closed,
            0x0100 => Self::CloseFailed { errno },
            0x0200 => Self::Disconnected,
            0x0400 => Self::MonitorStopped,
            0x0800 => Self::HandshakeFailedNoDetail { errno },
            0x1000 => Self::HandshakeSucceeded,
            0x2000 => Self::HandshakeFailedProtocol { code: value },
            0x4000 => Self::HandshakeFailedAuth { status: value },
            _ => Self::Other { event, value },
        }
    }

    /// Whether this is one of the three handshake-failure events.
    pub fn is_handshake_failure(&self) -> bool {
        matches!(
            self,
            Self::HandshakeFailedNoDetail { .. }
                | Self::HandshakeFailedProtocol { .. }
                | Self::HandshakeFailedAuth { .. }
        )
    }
}

/// An event and the endpoint it concerns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocketEvent {
    /// What happened.
    pub event: MonitorEvent,
    /// Local or remote endpoint, as libzmq reports it.
    pub endpoint: String,
}

impl SocketEvent {
    /// Monitor messages are `[u16 event | u32 value]` (native endian) then the endpoint.
    fn parse(m: &Message) -> Option<Self> {
        let [head, endpoint] = m.frames() else {
            return None;
        };
        let head: &[u8; 6] = head.as_slice().try_into().ok()?;
        Some(Self {
            event: MonitorEvent::from_raw(
                u16::from_ne_bytes([head[0], head[1]]),
                u32::from_ne_bytes([head[2], head[3], head[4], head[5]]),
            ),
            endpoint: String::from_utf8_lossy(endpoint).into_owned(),
        })
    }
}

/// Reader of one socket's events (see [`Socket::monitor`]). Like a socket, used by one thread
/// at a time; include [`Monitor::socket`] in a [`crate::poll`] to wait on events.
#[derive(Debug)]
pub struct Monitor {
    pair: Socket,
}

impl Monitor {
    pub(crate) fn new(pair: Socket) -> Self {
        Self { pair }
    }

    /// The underlying socket, for polling.
    pub fn socket(&self) -> &Socket {
        &self.pair
    }

    /// The next event if one is queued.
    pub fn try_recv(&self) -> Result<Option<SocketEvent>> {
        while let Some(m) = self.pair.try_recv()? {
            if let Some(ev) = SocketEvent::parse(&m) {
                return Ok(Some(ev));
            }
        }
        Ok(None)
    }

    /// Waits up to `timeout` for the next event matching `want`; skips the others.
    pub fn wait_for(
        &self,
        timeout: Duration,
        mut want: impl FnMut(&MonitorEvent) -> bool,
    ) -> Result<Option<SocketEvent>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(m) = self.pair.recv_timeout(left)? else {
                return Ok(None);
            };
            if let Some(ev) = SocketEvent::parse(&m)
                && want(&ev.event)
            {
                return Ok(Some(ev));
            }
        }
    }
}
