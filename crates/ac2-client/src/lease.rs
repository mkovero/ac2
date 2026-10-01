//! Stimulus lease (Q6): acquire, refresh in the background while held, release on drop.
//!
//! The daemon fades out and disarms 1.5 s after the last refresh, so a client that dies or
//! loses the network can never leave a stimulus running. This helper refreshes every
//! 0.5 s with a short per-try deadline, so one lost message is resent well inside the
//! expiry window.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ac2_proto::model::{Generator, GeneratorDesired, Lease};
use ac2_proto::units::LeaseToken;
use ac2_proto::{Command, ErrorCode, ReplyBody};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::client::{Client, Retry};
use crate::error::ClientError;
use crate::expect_body;

/// Refresh period.
pub const REFRESH_EVERY: Duration = Duration::from_millis(500);

/// Per-try deadline of a refresh: two tries fit in one refresh period.
const REFRESH_RETRY: Retry = Retry {
    timeout: Duration::from_millis(250),
    retries: 1,
};

/// What happens to the output when the lease is dropped without [`StimulusLease::end`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDrop {
    /// `gen.release` (stops, disarms, releases).
    Release,
    /// `gen.stop` then `gen.release`: the foreground-CLI rule that output never outlives the
    /// command, even if the token was just invalidated daemon-side.
    StopAndRelease,
}

/// Why the lease is no longer held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseLost {
    /// The daemon refused a refresh (expired or taken over with force).
    Refused {
        /// Error code.
        code: ErrorCode,
        /// Message.
        msg: String,
    },
}

/// A held stimulus lease.
#[derive(Debug)]
pub struct StimulusLease {
    client: Client,
    token: LeaseToken,
    expires_in: Duration,
    on_drop: OnDrop,
    lost: watch::Receiver<Option<LeaseLost>>,
    ended: Arc<AtomicBool>,
    refresher: JoinHandle<()>,
}

impl Client {
    /// `gen.acquire`, then refresh in the background until the lease is ended or dropped.
    pub async fn acquire_lease(
        &self,
        force: bool,
        on_drop: OnDrop,
    ) -> Result<StimulusLease, ClientError> {
        let op = "gen.acquire";
        let r = self.call(Command::GenAcquire { force }).await?;
        let lease: Lease = expect_body!(op, r, ReplyBody::Lease(l) => l)?;
        let (lost_tx, lost_rx) = watch::channel(None);
        let ended = Arc::new(AtomicBool::new(false));
        let refresher = tokio::spawn(refresh_loop(
            self.clone(),
            lease.lease_token,
            lost_tx,
            ended.clone(),
        ));
        Ok(StimulusLease {
            client: self.clone(),
            token: lease.lease_token,
            expires_in: Duration::from_millis(u64::from(lease.expires_in_ms)),
            on_drop,
            lost: lost_rx,
            ended,
            refresher,
        })
    }
}

async fn refresh_loop(
    client: Client,
    token: LeaseToken,
    lost: watch::Sender<Option<LeaseLost>>,
    ended: Arc<AtomicBool>,
) {
    let mut tick = tokio::time::interval(REFRESH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    loop {
        tick.tick().await;
        if ended.load(Ordering::Acquire) {
            return;
        }
        match client
            .call_with(
                Command::GenRefresh { lease_token: token },
                None,
                REFRESH_RETRY,
            )
            .await
        {
            Ok(_) => {}
            Err(ClientError::Daemon(e)) => {
                lost.send_replace(Some(LeaseLost::Refused {
                    code: e.code,
                    msg: e.msg,
                }));
                return;
            }
            // Timeouts: the next period tries again; the daemon's expiry is the backstop.
            Err(ClientError::Closed) => return,
            Err(_) => {}
        }
    }
}

impl StimulusLease {
    /// The token.
    pub fn token(&self) -> LeaseToken {
        self.token
    }

    /// Expiry the daemon granted.
    pub fn expires_in(&self) -> Duration {
        self.expires_in
    }

    /// Why the lease was lost, if it was.
    pub fn lost(&self) -> Option<LeaseLost> {
        self.lost.borrow().clone()
    }

    /// Resolves when the lease is lost.
    pub async fn wait_lost(&mut self) -> LeaseLost {
        loop {
            if let Some(l) = self.lost.borrow_and_update().clone() {
                return l;
            }
            if self.lost.changed().await.is_err() {
                // The refresher ended without a loss: only after `end`. Never resolve.
                std::future::pending::<()>().await;
            }
        }
    }

    /// `gen.set` with the full desired state (also refreshes the lease).
    pub async fn set(&self, desired: GeneratorDesired) -> Result<Generator, ClientError> {
        let op = "gen.set";
        let r = self
            .client
            .call(Command::GenSet {
                lease_token: self.token,
                desired,
            })
            .await?;
        expect_body!(op, r, ReplyBody::Generator(g) => g)
    }

    /// Stops output (per [`OnDrop`]) and releases the lease, waiting for the replies.
    pub async fn end(self) -> Result<(), ClientError> {
        self.ended.store(true, Ordering::Release);
        self.refresher.abort();
        // After a loss the output may be someone else's: never stop it from here.
        let stop = if self.on_drop == OnDrop::StopAndRelease && self.lost().is_none() {
            self.client.call(Command::GenStop).await.map(drop)
        } else {
            Ok(())
        };
        let release = if self.lost().is_none() {
            self.client
                .call(Command::GenRelease {
                    lease_token: self.token,
                })
                .await
                .map(drop)
        } else {
            Ok(())
        };
        stop.and(release)
    }
}

impl Drop for StimulusLease {
    fn drop(&mut self) {
        self.refresher.abort();
        if self.ended.swap(true, Ordering::AcqRel) {
            return;
        }
        // No await in drop: queue the requests; the I/O thread sends them before it stops.
        let core = self.client.core();
        if self.on_drop == OnDrop::StopAndRelease && self.lost().is_none() {
            core.send_nowait(Command::GenStop);
        }
        if self.lost().is_none() {
            core.send_nowait(Command::GenRelease {
                lease_token: self.token,
            });
        }
    }
}
