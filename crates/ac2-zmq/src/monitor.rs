//! Typed socket monitor events (`zmq_socket_monitor`).
//!
//! Refused handshakes are observable here without timing guesses: a ZAP denial arrives as
//! [`MonitorEvent::HandshakeFailedAuth`] on both ends, a mechanism mismatch or wrong server
//! key as [`MonitorEvent::HandshakeFailedProtocol`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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

/// What a monitored socket and its [`Monitor`] share.
///
/// libzmq reports a socket's events from the context's I/O thread with a blocking send on a
/// PAIR, among them the `Closed` of a connect still in progress when the socket closes (a peer
/// whose host does not answer SYNs). With the reader gone that send never returns, and the I/O
/// thread stops serving every socket of the context. So the reader stays open until the
/// socket has stopped its monitor: a monitor dropped first parks its reader here, and the
/// socket stops monitoring the next time it is used (or closes), then lets the reader go.
/// libzmq's monitor calls are made on the socket's own thread only.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    /// A reader is parked: checked on every use of the socket, so lock-free.
    parked_flag: AtomicBool,
    state: Mutex<WatchState>,
}

#[derive(Debug, Default)]
struct WatchState {
    /// The socket stopped this monitor (it closed, or a new monitor replaced this one).
    stopped: bool,
    parked: Option<Socket>,
}

pub(crate) type SharedWatch = Arc<Watch>;

impl Watch {
    fn lock(&self) -> MutexGuard<'_, WatchState> {
        // Two plain fields: a panic while held cannot leave them inconsistent.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the monitor was dropped and the socket should stop it.
    pub(crate) fn parked(&self) -> bool {
        self.parked_flag.load(Ordering::Acquire)
    }

    /// The socket's side: its monitor is stopped, so the parked reader may close.
    pub(crate) fn stopped(&self) {
        let parked = {
            let mut w = self.lock();
            w.stopped = true;
            w.parked.take()
        };
        drop(parked);
    }
}

/// Reader of one socket's events (see [`Socket::monitor`]). Like a socket, used by one thread
/// at a time; include [`Monitor::socket`] in a [`crate::poll`] to wait on events.
///
/// Dropped while its socket lives on, it keeps its reader (and the events queued for it)
/// until the socket is next used: sent on, received from, polled, or closed.
#[derive(Debug)]
pub struct Monitor {
    /// `Some` until dropped.
    pair: Option<Socket>,
    watch: SharedWatch,
}

impl Drop for Monitor {
    fn drop(&mut self) {
        let Some(pair) = self.pair.take() else {
            return;
        };
        let mut w = self.watch.lock();
        if !w.stopped {
            w.parked = Some(pair);
            self.watch.parked_flag.store(true, Ordering::Release);
        }
    }
}

impl Monitor {
    pub(crate) fn new(pair: Socket, watch: SharedWatch) -> Self {
        Self {
            pair: Some(pair),
            watch,
        }
    }

    /// The underlying socket, for polling.
    pub fn socket(&self) -> &Socket {
        self.pair
            .as_ref()
            .expect("a monitor holds its reader until dropped")
    }

    /// The next event if one is queued.
    pub fn try_recv(&self) -> Result<Option<SocketEvent>> {
        while let Some(m) = self.socket().try_recv()? {
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
            let Some(m) = self.socket().recv_timeout(left)? else {
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
