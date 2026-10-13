//! Session file work off the control thread: `file.save`, `file.load`, `file.new`,
//! `file.import`, `file.download_begin` and `file.list`.
//!
//! The control thread also refreshes leases, stops the generator and answers every other
//! client; a big session on a slow disk must not hold any of that up (an unrefreshed lease
//! mutes the output). So a file command runs in three steps: the control thread checks it
//! and captures what it needs (a save's content is an owned snapshot, so later edits cannot
//! tear it), a worker thread does the disk and archive work, and the result comes back as
//! [`ControlMsg::FileDone`] for a serial commit on the control thread, which answers the
//! request then (as `delay.find` and the other worker-backed commands are answered).
//!
//! While one of these is pending, state may change; the policy:
//! - One file operation at a time. Another `file.save` / `file.load` / `file.new` /
//!   `file.recover` / `file.salvage` / `file.import` / `file.download_begin` is refused
//!   (`refused`, nothing changed) until it is answered. `file.list` only reads, and runs
//!   beside anything.
//! - A save writes the content as it was when it began; edits made meanwhile stay unsaved.
//! - A load or new commits only onto the working state it began on: when the content or its
//!   saved standing changed meanwhile, it is refused (`conflict`, nothing changed) and the
//!   client asks again (the unsaved-changes guard then sees the edits).
//! - A retry of the request still pending never reaches here: the request's reservation
//!   (`Dedup::admit`) attaches it, and the one result answers every copy.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use ac2_proto::model::SessionOrigin;
use ac2_proto::units::{ClientId, RequestId, TransferId, WallNs};
use ac2_proto::{Command, ErrorCode, ProtoError, ReplyBody};
use ac2_traces::session::{Manifest, Session as SessionData};

use super::working::Staged;
use super::{Control, ControlMsg, PendingReply};
use crate::util::perr;

/// How long shutdown waits for a file operation under way to finish writing.
const FLUSH_BOUND: Duration = Duration::from_secs(10);

/// What the commit of a file operation needs, captured when it began.
pub(super) enum Pending {
    List,
    Save {
        name: String,
        dir: PathBuf,
        origin: SessionOrigin,
        /// The content hash of the snapshot written.
        hash: u64,
    },
    /// `file.load` or `file.new`.
    Replace {
        client: ClientId,
        discard: bool,
        from: ReplaceFrom,
        /// [`Control::standing`] when it began.
        standing: (u64, u64),
    },
    Import {
        transfer: TransferId,
        name: String,
        target: PathBuf,
    },
    Download {
        client: ClientId,
        stem: String,
        /// A working-state download: the content hash and time it was packed at.
        working: Option<(u64, WallNs)>,
    },
}

/// Where a replace's new working state comes from.
pub(super) enum ReplaceFrom {
    /// `file.load` of a session directory.
    Dir {
        name: String,
        dir: PathBuf,
        origin: SessionOrigin,
    },
    /// `file.load` of an upload, unpacked into `stage`.
    Upload {
        transfer: TransferId,
        file: String,
        stage: PathBuf,
    },
    /// `file.new`.
    Empty,
}

/// A replace's worker result: the session read (with its manifest; `None` for
/// `file.new`) and the recovery copy written.
pub(crate) struct ReplaceRead {
    pub(super) read: Result<(Option<Manifest>, SessionData), ProtoError>,
    pub(super) kept: Option<Staged>,
}

/// What a worker reports back.
pub(crate) enum FileDone {
    List(Result<ReplyBody, ProtoError>),
    Save(Result<Manifest, ProtoError>),
    Replace(ReplaceRead),
    Import(Result<Manifest, ProtoError>),
    Download(Result<Vec<u8>, ProtoError>),
}

/// The disk part of a file operation, run on its worker thread.
pub(super) type Work = Box<dyn FnOnce() -> FileDone + Send>;

/// A file operation under way.
pub(super) struct FileJob {
    command: &'static str,
    reply: PendingReply,
    pending: Pending,
    /// Disconnects when the worker ends.
    ended: Receiver<()>,
}

/// The file operations under way, by token.
#[derive(Default)]
pub(super) struct FileJobs(HashMap<u64, FileJob>);

impl Control {
    /// Refuses while a file operation other than `file.list` is under way (module docs).
    pub(super) fn file_idle(&self, command: &str) -> Result<(), ProtoError> {
        match self
            .file_jobs
            .0
            .values()
            .find(|j| !matches!(j.pending, Pending::List))
        {
            None => Ok(()),
            Some(j) => Err(perr(
                ErrorCode::Refused,
                format!(
                    "{} is under way; {command} waits until it is answered",
                    j.command
                ),
            )),
        }
    }

    /// Begins a file operation whose reply follows its worker's result. `Ok`: pending, the
    /// reply comes later; `Err`: refused before anything began.
    pub(super) fn start_file_job(
        &mut self,
        routing_id: &[u8],
        client: &ClientId,
        id: RequestId,
        cmd: Command,
    ) -> Result<(), ProtoError> {
        let command = cmd.name();
        let (pending, work) = match cmd {
            Command::FileList => self.begin_list(),
            Command::FileSave { session, overwrite } => {
                self.file_idle(command)?;
                self.begin_save(&session, overwrite)?
            }
            Command::FileLoad { session, discard } => {
                self.file_idle(command)?;
                self.begin_load(client, &session, discard)?
            }
            Command::FileNew { discard } => {
                self.file_idle(command)?;
                self.begin_new(client, discard)?
            }
            Command::FileImport {
                transfer,
                name,
                overwrite,
            } => {
                self.file_idle(command)?;
                self.begin_import(client, transfer, name, overwrite)?
            }
            Command::FileDownloadBegin { session } => {
                self.file_idle(command)?;
                self.begin_download(client, session.as_ref())?
            }
            other => {
                return Err(perr(
                    ErrorCode::Internal,
                    format!("{} is not a file operation", other.name()),
                ));
            }
        };
        let token = self.next_token;
        self.next_token += 1;
        let to = self.s.to_self.clone();
        let (alive, ended) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("ac2d-file".into())
            .spawn(move || {
                let _alive = alive;
                let done = work();
                let _ = to.send(ControlMsg::FileDone {
                    token,
                    done: Box::new(done),
                });
            })
            .map_err(|e| perr(ErrorCode::Internal, format!("cannot start {command}: {e}")))?;
        self.file_jobs.0.insert(
            token,
            FileJob {
                command,
                reply: PendingReply {
                    routing_id: routing_id.to_vec(),
                    client: client.clone(),
                    id,
                },
                pending,
                ended,
            },
        );
        Ok(())
    }

    /// Worker `token` reported back: commits and answers.
    pub(super) fn file_done(&mut self, token: u64, done: FileDone) {
        let Some(job) = self.file_jobs.0.remove(&token) else {
            return;
        };
        let result = match (job.pending, done) {
            (Pending::List, FileDone::List(r)) => r,
            (
                Pending::Save {
                    name,
                    dir,
                    origin,
                    hash,
                },
                FileDone::Save(r),
            ) => self.finish_save(&name, &dir, origin, hash, r),
            (
                Pending::Replace {
                    client,
                    discard,
                    from,
                    standing,
                },
                FileDone::Replace(r),
            ) => self.finish_replace(&client, discard, from, standing, r),
            (
                Pending::Import {
                    transfer,
                    name,
                    target,
                },
                FileDone::Import(r),
            ) => self.finish_import(transfer, &name, &target, r),
            (
                Pending::Download {
                    client,
                    stem,
                    working,
                },
                FileDone::Download(r),
            ) => self.finish_download(&client, &stem, working, r),
            (_, done) => {
                if let FileDone::Replace(ReplaceRead { kept: Some(k), .. }) = done {
                    k.discard();
                }
                Err(perr(
                    ErrorCode::Internal,
                    format!("{}: a result of another operation", job.command),
                ))
            }
        };
        let r = job.reply;
        self.answer(&r.routing_id, &r.client, r.id, result, Instant::now());
    }

    /// Shutdown: a file operation under way finishes writing first, within a bound.
    pub(super) fn flush_file_jobs(&mut self) {
        let deadline = Instant::now() + FLUSH_BOUND;
        for (_, j) in self.file_jobs.0.drain() {
            let left = deadline.saturating_duration_since(Instant::now());
            if j.ended.recv_timeout(left) == Err(RecvTimeoutError::Timeout) {
                tracing::warn!(
                    "{} did not finish within {} s of shutdown",
                    j.command,
                    FLUSH_BOUND.as_secs()
                );
            }
        }
    }
}
