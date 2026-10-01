//! `zmq_poll` over several sockets.

use std::ffi::{c_long, c_short};
use std::time::{Duration, Instant};

use crate::error::{Result, zmq};
use crate::raw;
use crate::socket::Socket;

/// What to wait for on one socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interest {
    /// A message can be received.
    Readable,
    /// A message can be sent without blocking. XPUB is always writable (it drops per peer
    /// instead of blocking), so this says nothing about slow subscribers.
    Writable,
    /// Either.
    Both,
}

impl Interest {
    fn raw(self) -> c_short {
        match self {
            Self::Readable => raw::ZMQ_POLLIN,
            Self::Writable => raw::ZMQ_POLLOUT,
            Self::Both => raw::ZMQ_POLLIN | raw::ZMQ_POLLOUT,
        }
    }
}

/// One socket in a [`poll`] call, and its readiness afterwards.
#[derive(Debug)]
pub struct PollItem<'s> {
    socket: &'s Socket,
    interest: Interest,
    ready: c_short,
}

impl<'s> PollItem<'s> {
    /// Waits for `interest` on `socket`.
    pub fn new(socket: &'s Socket, interest: Interest) -> Self {
        Self {
            socket,
            interest,
            ready: 0,
        }
    }

    /// Waits for `socket` to become readable.
    pub fn readable(socket: &'s Socket) -> Self {
        Self::new(socket, Interest::Readable)
    }

    /// After [`poll`]: a message can be received.
    pub fn is_readable(&self) -> bool {
        self.ready & raw::ZMQ_POLLIN != 0
    }

    /// After [`poll`]: a message can be sent.
    pub fn is_writable(&self) -> bool {
        self.ready & raw::ZMQ_POLLOUT != 0
    }
}

/// Waits until at least one item is ready or `timeout` passes (`None` = indefinitely).
/// Returns the number of ready items and updates each item's readiness. Interrupted waits
/// are resumed with the remaining time.
pub fn poll(items: &mut [PollItem<'_>], timeout: Option<Duration>) -> Result<usize> {
    let raw_items: Vec<_> = items
        .iter()
        .map(|i| (i.socket.raw(), i.interest.raw()))
        .collect();
    let deadline = timeout.map(|t| Instant::now() + t);
    loop {
        let ms = match deadline {
            None => -1,
            Some(d) => {
                let left = d.saturating_duration_since(Instant::now());
                // Round up so a sub-millisecond remainder still waits instead of spinning.
                let ms = left.as_micros().div_ceil(1000);
                c_long::try_from(ms).unwrap_or(c_long::MAX)
            }
        };
        match zmq(raw::poll(&raw_items, ms)) {
            Ok(ready) => {
                let mut n = 0;
                for (item, r) in items.iter_mut().zip(ready) {
                    item.ready = r;
                    n += usize::from(r != 0);
                }
                return Ok(n);
            }
            Err(e) if e.is_interrupted() => {}
            Err(e) => return Err(e),
        }
    }
}
