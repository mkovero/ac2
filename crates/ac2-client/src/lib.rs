//! Async client for the ac2 daemon: connect, call, subscribe, mirrored state.
//!
//! - [`Client::connect`] opens the ctrl DEALER and two data SUBs (one for `evt` + `ka`, one for
//!   measurement frames), says `hello` and refuses another protocol version.
//! - [`Client::call`] sends a typed [`ac2_proto::Command`] with a fresh request id, waits with
//!   a deadline and resends the *same* id on timeout (the daemon deduplicates ids, so a
//!   retry never executes twice). [`Client::call_expect`] adds an `expect_rev` guard.
//! - The mirror ([`Client::view`], [`Client::watch`]) follows the Q5 sync procedure: see
//!   [`mirror`].
//! - [`Client::subscribe`] / [`Client::latest`] give the newest frame per topic by draining the
//!   data socket, with frame age and STALE; [`Client::grid`] caches `grid.get`.
//! - [`Client::acquire_lease`] returns a [`StimulusLease`] refreshed in the background and
//!   released on drop.
//!
//! libzmq sockets are confined to one I/O thread per client (and the data SUB behind a
//! mutex); no tokio file-descriptor integration is used.

mod client;
pub mod data;
pub mod endpoint;
mod error;
mod io;
pub mod keys;
mod lease;
pub mod mirror;

#[cfg(feature = "test-support")]
pub mod fake;

pub use client::{Client, ClientConfig, Retry, body_name};
pub use data::{Latest, TopicFrame, frame_age};
pub use endpoint::{Endpoints, RemoteAddr};
pub use error::{ClientError, code_name};
pub use keys::{KeyDir, PinStatus, fingerprint};
pub use lease::{LeaseLost, OnDrop, StimulusLease};
pub use mirror::{MirrorView, Phase};
