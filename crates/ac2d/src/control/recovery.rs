//! An open session whose audio stopped: the stopped stream is closed off this thread and
//! the same configuration reopened, attempt after attempt, until it opens or a client
//! closes the session (`docs/design/audio-recovery.md`).

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use ac2_proto::ProtoError;
use ac2_proto::event::Change;
use ac2_proto::model::{
    AudioStopped, BackendKind, GenAction, OpenSession, Recovery as WireRecovery, Session, StopCause,
};
use ac2_proto::units::{SessionEpoch, WallNs};

use super::{Control, ControlMsg};
use crate::session::Runtime;
use crate::util::wall_ns;

/// Waits between failed attempts: doubling from 1 s, so a device that comes straight back
/// (a server restarted by hand, a reset interface) is found within a second or two, capped
/// at 30 s, so a rig left waiting for hours costs a JACK client open twice a minute and the
/// operator who fixes it waits at most half a minute.
pub(crate) fn backoff(failed_attempt: u32) -> Duration {
    const FIRST: Duration = Duration::from_secs(1);
    const CAP: Duration = Duration::from_secs(30);
    let doublings = failed_attempt.saturating_sub(1).min(5);
    (FIRST * (1u32 << doublings)).min(CAP)
}

/// How long the first attempt waits for the stopped stream to close: a healthy host closes
/// one in milliseconds, and opening beside it would make a second client of the same
/// device. A hung server never answers the close, and its outcome is no reason to wait
/// longer: the open then hangs or fails on its own.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// What a closing caller waits for at most before it lets the close finish on its own.
pub(crate) const CLOSE_BOUND: Duration = Duration::from_secs(2);

/// While at the cap, a failure that says the same as the last one logged is logged only
/// every this many attempts (every 5 min at 30 s).
const QUIET_REPEATS: u32 = 10;

/// Closes `rt` on a thread of its own: stopping a JACK client of a hung server blocks until
/// the server dies, and neither the control loop nor the audio path may wait on that. The
/// receiver hears once it is closed.
pub(crate) fn close_off_thread(rt: Runtime) -> Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("ac2d-close".into())
        .spawn(move || {
            rt.close();
            let _ = tx.send(());
        });
    if let Err(e) = spawned {
        // No thread to close it on: the receiver reports it closed at once (its sender is
        // gone); the stream goes with the failed closure's drop.
        tracing::error!("cannot start a thread to close the audio stream: {e}");
    }
    rx
}

/// Closes `rt`, waiting at most `bound`; a close still pending after that finishes (or
/// hangs) on its own thread.
pub(crate) fn close_bounded(rt: Runtime, bound: Duration) {
    let epoch = rt.epoch.0;
    if close_off_thread(rt).recv_timeout(bound) == Err(RecvTimeoutError::Timeout) {
        tracing::warn!(
            "the audio stream of epoch {epoch} did not close within {} s; it closes on its own \
             thread when the audio host answers",
            bound.as_secs()
        );
    }
}

/// Where the attempts stand.
enum Phase {
    /// Attempt `token` runs on its own thread.
    Opening { token: u64 },
    /// The next attempt begins at this instant.
    Waiting { at: Instant },
}

/// The configuration being reopened and how it goes.
pub(crate) struct Recovery {
    /// The epoch the stopped session shows; a reopened stream gets the next.
    epoch: SessionEpoch,
    open: OpenSession,
    routes: Vec<u16>,
    since: WallNs,
    cause: StopCause,
    attempt: u32,
    phase: Phase,
    /// The last failure logged: its wait and text.
    logged: Option<(Duration, String, u32)>,
}

impl Recovery {
    /// When the control loop must wake to start the next attempt.
    pub(crate) fn due(&self) -> Option<Instant> {
        match self.phase {
            Phase::Waiting { at } => Some(at),
            Phase::Opening { .. } => None,
        }
    }
}

fn cause_text(c: StopCause) -> &'static str {
    match c {
        StopCause::NotDelivering { .. } => "the device stopped delivering",
        StopCause::HostEnded => "the audio host ended the stream",
        StopCause::DeviceChanged => "the device changed and did not reopen",
    }
}

impl Control {
    /// The stream of `epoch` stopped: closes it off this thread, pauses the measurements,
    /// disarms the generator and starts reopening the configuration.
    pub(super) fn audio_stopped(&mut self, epoch: SessionEpoch, since: WallNs, cause: StopCause) {
        // Only the current stream's report counts; one from a stream already replaced is old.
        if self.session.as_ref().is_none_or(|r| r.epoch != epoch) {
            return;
        }
        let Some(rt) = self.session.take() else {
            return;
        };
        tracing::warn!(
            "audio stopped ({}); reopening the session's configuration until it opens or a \
             client closes the session",
            cause_text(cause)
        );
        let open = rt.open.clone();
        let routes = rt.routes.clone();
        self.wind_down(ac2_proto::model::RecordingEnd::AudioStopped);
        self.disarm_for_outage();
        let closed = close_off_thread(rt);
        self.recovery = Some(Recovery {
            epoch,
            open,
            routes,
            since,
            cause,
            attempt: 0,
            phase: Phase::Waiting { at: Instant::now() },
            logged: None,
        });
        self.start_attempt(Some(closed));
    }

    /// A device change whose reopen failed with `e` (the stream already closed): the
    /// attempts carry on from there.
    pub(super) fn reopen_failed(
        &mut self,
        open: OpenSession,
        routes: Vec<u16>,
        epoch: SessionEpoch,
        e: &ProtoError,
    ) {
        self.disarm_for_outage();
        self.recovery = Some(Recovery {
            epoch,
            open,
            routes,
            since: WallNs(wall_ns()),
            cause: StopCause::DeviceChanged,
            attempt: 1,
            phase: Phase::Opening { token: 0 },
            logged: None,
        });
        self.attempt_failed(e);
    }

    /// The generator never comes back armed (principle 9): an outage disarms it at once.
    fn disarm_for_outage(&mut self) {
        let g = self.store.state().generator.clone();
        self.gate.close();
        if g.armed || g.firing {
            let mut g = g;
            g.armed = false;
            g.firing = false;
            self.audit(&mut g, GenAction::Stop, None);
            self.commit(Change::Generator(g));
        }
    }

    /// Starts the next attempt on a thread of its own; `closed`: the stopped stream's close,
    /// waited for a little first.
    pub(super) fn start_attempt(&mut self, closed: Option<Receiver<()>>) {
        let token = self.next_token;
        self.next_token += 1;
        let Some(r) = self.recovery.as_mut() else {
            return;
        };
        let epoch = SessionEpoch(r.epoch.0 + 1);
        r.attempt += 1;
        r.phase = Phase::Opening { token };
        let attempt = r.attempt;
        let config = r.open.config.clone();
        let routes = r.routes.clone();
        let kind: BackendKind = r.open.backend;
        let backend = match self.backend_for(Some(kind)) {
            Ok(b) => b,
            Err(e) => {
                self.attempt_failed(&e);
                return;
            }
        };
        let to_self = self.s.to_self.clone();
        let max_level = self.s.max_level;
        let fps = self.s.fps;
        let spawned = std::thread::Builder::new()
            .name("ac2d-reopen".into())
            .spawn(move || {
                if let Some(c) = closed {
                    let _ = c.recv_timeout(CLOSE_WAIT);
                }
                let result = Runtime::open(
                    &*backend,
                    &config,
                    &routes,
                    max_level,
                    epoch,
                    to_self.clone(),
                    fps,
                );
                // A daemon gone meanwhile drops the stream with the message.
                let _ = to_self.send(ControlMsg::Reopened {
                    token,
                    result: Box::new(result),
                });
            });
        if let Err(e) = spawned {
            self.attempt_failed(&crate::util::perr(
                ac2_proto::ErrorCode::Internal,
                format!("cannot start a thread to reopen the session: {e}"),
            ));
            return;
        }
        self.commit_stopped(WireRecovery::Opening {
            attempt,
            started: WallNs(wall_ns()),
        });
    }

    /// Attempt `token` finished.
    pub(super) fn reopened(&mut self, token: u64, result: Result<Runtime, ProtoError>) {
        let current = self
            .recovery
            .as_ref()
            .is_some_and(|r| matches!(r.phase, Phase::Opening { token: t } if t == token));
        if !current {
            // The session was closed or opened anew meanwhile.
            if let Ok(rt) = result {
                drop(close_off_thread(rt));
            }
            return;
        }
        match result {
            Ok(mut rt) => {
                let Some(r) = self.recovery.take() else {
                    return;
                };
                rt.open.replay = r.open.replay.clone();
                let out_s = (wall_ns().saturating_sub(r.since.0)) as f64 / 1e9;
                tracing::warn!(
                    "audio back: the session reopened on attempt {} after {out_s:.1} s without \
                     audio (epoch {}); measurements restart, the generator stays disarmed",
                    r.attempt,
                    rt.epoch.0
                );
                let s = Session {
                    epoch: rt.epoch,
                    open: Some(rt.open.clone()),
                    stopped: None,
                };
                self.session = Some(rt);
                self.commit(Change::Session(s));
                self.after_open();
                self.release_replay();
            }
            Err(e) => self.attempt_failed(&e),
        }
    }

    /// The current attempt failed with `e`: the next waits its backoff.
    fn attempt_failed(&mut self, e: &ProtoError) {
        let Some(r) = self.recovery.as_mut() else {
            return;
        };
        let wait = backoff(r.attempt);
        r.phase = Phase::Waiting {
            at: Instant::now() + wait,
        };
        let attempt = r.attempt;
        // Logged once per backoff step; at the cap only a new error or every so often.
        let quiet = r.logged.as_ref().is_some_and(|(w, m, at)| {
            *w == wait && *m == e.msg && attempt.saturating_sub(*at) < QUIET_REPEATS
        });
        if !quiet {
            tracing::warn!(
                "reopen attempt {attempt} failed: {}; next in {} s",
                e.msg,
                wait.as_secs()
            );
            r.logged = Some((wait, e.msg.clone(), attempt));
        }
        let next_at = WallNs(wall_ns().saturating_add(wait.as_nanos() as u64));
        self.commit_stopped(WireRecovery::Waiting {
            attempt,
            error: e.msg.clone(),
            next_at,
        });
    }

    /// A session file loaded while the audio is stopped: the stopped session shows the
    /// load's `epoch`, and comes back without generator routes like any loaded session.
    pub(super) fn rebase_recovery(&mut self, epoch: SessionEpoch) {
        let Some(r) = self.recovery.as_mut() else {
            return;
        };
        r.epoch = epoch;
        r.routes.clear();
        if let Some(st) = self.store.state().session.stopped.clone() {
            self.commit_stopped(st.recovery);
        }
    }

    fn commit_stopped(&mut self, recovery: WireRecovery) {
        let Some(r) = self.recovery.as_ref() else {
            return;
        };
        let s = Session {
            epoch: r.epoch,
            open: Some(r.open.clone()),
            stopped: Some(AudioStopped {
                since: r.since,
                cause: r.cause,
                recovery,
            }),
        };
        self.commit(Change::Session(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_one_second_to_thirty() {
        let s: Vec<u64> = (1..=8).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(s, [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(backoff(u32::MAX).as_secs(), 30);
    }
}
