//! Waiting for a file operation ([`Command::is_file_operation`]): the daemon answers it
//! when its worker has packed, unpacked or written the session, which for a large session
//! on a slow disk takes far longer than any reply deadline. Giving up after the usual
//! tries would leave the daemon working for nobody, holding its one file slot meanwhile.
//! So the call waits as long as the daemon is alive (its keepalives arrive, 4 a second,
//! from the same incarnation), and a daemon that falls silent or restarts is still
//! reported within [`ClientConfig::dead_after`] whatever the per-try deadline.
//!
//! The same request id is resent once per try: the daemon runs an id once, so a copy that
//! arrives while the job runs waits for its reply, and one that arrives after the job
//! answered gets the stored reply — which is how a reply lost on a reconnect still comes.
//!
//! [`Command::is_file_operation`]: ac2_proto::Command::is_file_operation
//! [`ClientConfig::dead_after`]: super::ClientConfig::dead_after

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_proto::{Reply, ReplyBody, Warning};
use tokio::sync::{oneshot, watch};

use super::{CallCore, REFUSAL_POLL, Retry};
use crate::error::ClientError;
use crate::mirror::MirrorView;

/// What a file operation's wait watches.
#[derive(Debug)]
pub(crate) struct Liveness {
    /// The mirror view: its keepalive time and incarnation.
    pub(crate) view: watch::Receiver<Arc<MirrorView>>,
    /// Silence that counts as a dead daemon.
    pub(crate) dead_after: Duration,
}

impl CallCore {
    /// The reply to file operation `op` (sent once already as `bytes`), awaited while the
    /// daemon is alive; resent each `retry.timeout`, after which `waiting` hears how long
    /// it has waited.
    pub(super) async fn finish_while_alive(
        &self,
        op: &'static str,
        bytes: &[u8],
        rx: &mut oneshot::Receiver<Result<Reply, ClientError>>,
        retry: Retry,
        live: &Liveness,
        waiting: &mut (dyn FnMut(Duration) + Send),
    ) -> Result<(ReplyBody, Vec<Warning>), ClientError> {
        let start = tokio::time::Instant::now();
        let mut first = live.view.borrow().incarnation;
        let mut resend = start + retry.timeout;
        loop {
            // Liveness is looked at every slice, so a short silence limit holds under a
            // long per-try deadline too.
            let until = resend.min(tokio::time::Instant::now() + REFUSAL_POLL);
            if let Some(r) = self.reply_by(rx, until).await {
                return r;
            }
            let v = live.view.borrow().clone();
            let silent = v.silent_for(Instant::now());
            if silent >= live.dead_after {
                return Err(ClientError::Silent { op, silent });
            }
            match (first, v.incarnation) {
                (Some(a), Some(b)) if a != b => return Err(ClientError::Restarted { op }),
                (None, b) => first = b,
                _ => {}
            }
            if tokio::time::Instant::now() >= resend {
                self.io.send(bytes)?;
                resend += retry.timeout;
                waiting(start.elapsed());
            }
        }
    }
}
