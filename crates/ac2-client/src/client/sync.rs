//! The sync task: applies events and keepalives to the mirror, takes snapshots, and asks
//! `hello` again when the daemon tells the data connection to rejoin (its role may have
//! changed).

use std::sync::{Arc, Mutex};
use std::time::Instant;

use ac2_proto::{Command, ErrorCode, ReplyBody, Welcome};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::{CallCore, hello, view_of};
use crate::error::ClientError;
use crate::io::SyncIn;
use crate::mirror::{Mirror, MirrorView, Need, RETRY_INTERVAL};

/// The `hello` a rejoin asks for. It runs on a task of its own, so events and keepalives
/// go on being applied while the daemon is slow to answer; one that fails on the way is
/// asked again [`RETRY_INTERVAL`] after the failure, one the daemon refuses is not.
#[derive(Default)]
struct Rejoin {
    /// When the next `hello` is due: until a welcome arrives, the role shown may be the one
    /// the key no longer has.
    due: Option<Instant>,
    asking: Option<JoinHandle<Result<Welcome, ClientError>>>,
    /// A failure of this stretch was logged.
    logged: bool,
}

impl Rejoin {
    fn ticking(&self) -> bool {
        self.due.is_some()
    }

    fn start_if_due(&mut self, core: &Arc<CallCore>, name: &str, now: Instant) {
        if self.asking.is_some() || !self.due.is_some_and(|at| now >= at) {
            return;
        }
        self.due = None;
        let core = core.clone();
        let name = name.to_owned();
        self.asking = Some(tokio::spawn(async move { hello(&core, &name).await }));
    }

    /// The answer to the `hello` asked: the welcome to show, if one came.
    fn answered(
        &mut self,
        r: Result<Result<Welcome, ClientError>, tokio::task::JoinError>,
        now: Instant,
    ) -> Option<Welcome> {
        self.asking = None;
        match r {
            Ok(Ok(w)) => {
                self.logged = false;
                Some(w)
            }
            // Lost on the way: the daemon may answer the next.
            Ok(Err(e @ (ClientError::Zmq(_) | ClientError::Timeout { .. }))) => {
                if !self.logged {
                    tracing::warn!("rejoin: `hello` failed ({e}); asked again until answered");
                    self.logged = true;
                }
                self.due.get_or_insert(now + RETRY_INTERVAL);
                None
            }
            // The daemon's answer (a revoked key, a refusal): asking again changes nothing,
            // and the client shows what it shows for such a daemon.
            Ok(Err(e)) => {
                tracing::warn!("rejoin: `hello` refused ({e}); not asked again");
                self.due = None;
                self.logged = false;
                None
            }
            Err(_) => {
                self.due = None;
                None
            }
        }
    }
}

impl Drop for Rejoin {
    fn drop(&mut self) {
        if let Some(t) = self.asking.take() {
            t.abort();
        }
    }
}

pub(super) async fn run_sync(
    core: Arc<CallCore>,
    mut rx: mpsc::UnboundedReceiver<SyncIn>,
    view_tx: watch::Sender<Arc<MirrorView>>,
    mut mirror: Mirror,
    name: String,
    welcome: Arc<Mutex<Welcome>>,
) {
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut rejoin = Rejoin::default();
    loop {
        // Ticks only while the mirror or a rejoin has something to retry or check; otherwise
        // the task sleeps until the next keepalive or event.
        let ticking = mirror.needs_tick() || rejoin.ticking();
        let now = Instant::now();
        let (mut need, received) = tokio::select! {
            msg = rx.recv() => match msg {
                None => return,
                Some(SyncIn::Event(e)) => (mirror.on_event(*e, now), true),
                Some(SyncIn::Ka { stamp, meta, at, local_wall_ns }) => {
                    (mirror.on_ka(&stamp, *meta, at, local_wall_ns), true)
                }
                Some(SyncIn::Resync) => (mirror.on_resync_required(now), true),
                // Only the welcome: the state itself is unchanged by a role.
                Some(SyncIn::Rejoined) => {
                    rejoin.due = Some(now);
                    (None, true)
                }
            },
            r = async {
                match rejoin.asking.as_mut() {
                    Some(t) => t.await,
                    None => std::future::pending().await,
                }
            }, if rejoin.asking.is_some() => {
                match rejoin.answered(r, Instant::now()) {
                    Some(w) => {
                        if let Ok(mut slot) = welcome.lock() {
                            *slot = w;
                        }
                        (None, true)
                    }
                    None => (None, false),
                }
            }
            _ = tick.tick(), if ticking => (mirror.on_tick(Instant::now()), false),
        };
        rejoin.start_if_due(&core, &name, Instant::now());
        if !received && need.is_none() {
            continue;
        }
        while let Some(n) = need.take() {
            need = match n {
                Need::Rehello => match hello(&core, &name).await {
                    Ok(w) => {
                        if let Ok(mut slot) = welcome.lock() {
                            *slot = w;
                        }
                        Some(Need::Snapshot)
                    }
                    Err(_) => {
                        mirror.on_request_failed(Instant::now());
                        None
                    }
                },
                Need::Snapshot => match crate::snapshot::take(&core, core.retry, &mut || {
                    mirror.on_answer(Instant::now());
                    publish(&view_tx, &mirror, &welcome);
                })
                .await
                {
                    Ok(s) => mirror.on_snapshot(s, Instant::now()),
                    Err(_) => {
                        mirror.on_request_failed(Instant::now());
                        None
                    }
                },
                Need::Since(rev) => {
                    match core.call(Command::StateSince { rev }, core.retry).await {
                        Ok(ReplyBody::Events(evs)) => mirror.on_since(evs, Instant::now()),
                        Err(e) if e.code() == Some(ErrorCode::ResyncRequired) => {
                            mirror.on_resync_required(Instant::now())
                        }
                        _ => {
                            mirror.on_request_failed(Instant::now());
                            None
                        }
                    }
                }
            };
        }
        // Every message changes something visible (liveness, clock, rev); publishing on
        // each keeps watchers simple. Keepalives come at 4 Hz, events at commit rate.
        // Liveness itself is derived by watchers from `last_ka`, so idle ticks publish
        // nothing.
        publish(&view_tx, &mirror, &welcome);
    }
}

fn publish(view_tx: &watch::Sender<Arc<MirrorView>>, mirror: &Mirror, welcome: &Mutex<Welcome>) {
    let v = match welcome.lock() {
        Ok(w) => view_of(mirror, &w),
        Err(_) => mirror.view(),
    };
    view_tx.send_replace(Arc::new(v));
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
