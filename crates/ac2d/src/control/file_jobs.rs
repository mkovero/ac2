//! Session file work off the control thread: `file.save`, `file.load`, `file.new`,
//! `file.recover`, `file.salvage`, `file.import`, `file.download_begin` and `file.list`.
//! (The small writes of the working state's session — `working.json`, the recovery copy
//! put in place — run on the autosave's write thread: [`super::working`].)
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
//!   (`refused`, nothing changed) until it is answered — except that an operator's never
//!   waits on a viewer's download: the viewer's is cut (answered `refused`, its worker left
//!   to remove its staging) and the operator's begins. A viewer begins a download at most
//!   once per [`VIEWER_DOWNLOAD_EVERY`] per key, so it cannot keep the slot taken.
//! - `file.list` only reads and runs beside anything; one at a time, and a request that
//!   comes while it runs is answered with its result.
//! - A job answers only the role it began as: when its requester's role is taken down
//!   (demoted, revoked) the worker stops before it changes a session on disk or packs one,
//!   and whatever it read is discarded (staging removed, no transfer offered, nothing
//!   committed); the requester is refused.
//! - A save writes the content as it was when it began; edits made meanwhile stay unsaved.
//! - A load, new or recover commits only onto the working state it began on: when the
//!   content or its saved standing changed meanwhile, it is refused (`conflict`, nothing
//!   changed) and the client asks again (the unsaved-changes guard then sees the edits).
//! - A salvage only adds traces: it imports what its worker read onto the state as it is
//!   when the read comes back.
//! - A retry of the request still pending never reaches here: the request's reservation
//!   (`Dedup::admit`) attaches it, and the one result answers every copy.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use ac2_proto::model::{ClientRole, SessionOrigin};
use ac2_proto::units::{RequestId, TransferId, WallNs};
use ac2_proto::{Command, ProtoError, ReplyBody};
use ac2_traces::session::{Manifest, Session as SessionData};
use ac2_zmq::PublicKey;

use super::files::SalvageRead;
use super::working::Staged;
use super::{Control, ControlMsg, PendingReply};
use crate::route::{Peer, Route};
use crate::util::{internal, refused};

/// How long shutdown waits for a file operation under way to finish writing.
const FLUSH_BOUND: Duration = Duration::from_secs(10);

/// A viewer key begins at most one working-state download in this long: each packs the
/// whole working session onto the session disk and holds the file slot meanwhile. An app
/// exports when its user asks, never this often.
pub(crate) const VIEWER_DOWNLOAD_EVERY: Duration = Duration::from_secs(5);

/// Set when the requester's role is taken down while its job runs.
#[derive(Clone, Default)]
pub(super) struct Cancel(Arc<AtomicBool>);

impl Cancel {
    fn set(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_set(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// `Err` once set: called by a worker before it changes a session on disk or packs one.
    pub(super) fn check(&self) -> Result<(), ProtoError> {
        if self.is_set() {
            Err(role_changed("the operation"))
        } else {
            Ok(())
        }
    }
}

fn role_changed(command: &str) -> ProtoError {
    refused(format!(
        "this client's role changed while {command} ran; nothing was done or handed out"
    ))
}

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
        client: Peer,
        discard: bool,
        from: ReplaceFrom,
        /// [`Control::standing`] when it began.
        standing: (u64, u64),
    },
    /// `file.recover`.
    Recover {
        client: Peer,
        /// [`Control::standing`] when it began.
        standing: (u64, u64),
    },
    /// `file.salvage`; the upload it read, removed when it is answered well.
    Salvage {
        transfer: Option<TransferId>,
    },
    Import {
        transfer: TransferId,
        name: String,
        target: PathBuf,
    },
    Download {
        client: Peer,
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

/// A replace's (or recover's) worker result: the session read (with its manifest; `None`
/// for `file.new` and `file.recover`) and the recovery copy written.
pub(crate) struct ReplaceRead {
    pub(super) read: Result<(Option<Manifest>, SessionData), ProtoError>,
    pub(super) kept: Option<Staged>,
}

/// What a worker reports back.
// One message per worker job, moved once through the channel and never stored in bulk:
// boxing the larger variants would buy nothing.
#[allow(clippy::large_enum_variant)]
pub(crate) enum FileDone {
    List(Result<ReplyBody, ProtoError>),
    Save(Result<Manifest, ProtoError>),
    Replace(ReplaceRead),
    Salvage(Result<SalvageRead, ProtoError>),
    Import(Result<Manifest, ProtoError>),
    Download(Result<Vec<u8>, ProtoError>),
}

/// The disk part of a file operation, run on its worker thread.
pub(super) type Work = Box<dyn FnOnce(&Cancel) -> FileDone + Send>;

/// A file operation under way.
pub(super) struct FileJob {
    command: &'static str,
    reply: PendingReply,
    /// `file.list` requests that came while this one ran: answered with its result.
    attached: Vec<PendingReply>,
    /// The requester's role when it began.
    role: Option<ClientRole>,
    cancel: Cancel,
    pending: Pending,
    /// Disconnects when the worker ends.
    ended: Receiver<()>,
}

/// The file operations under way, by token.
#[derive(Default)]
pub(super) struct FileJobs {
    jobs: HashMap<u64, FileJob>,
    /// Workers of viewers' downloads an operator's command cut, still removing their
    /// staging: shutdown waits for them as for the rest.
    cut: Vec<Receiver<()>>,
    /// When each viewer key last began a download.
    viewer_downloads: HashMap<PublicKey, Instant>,
}

impl FileJobs {
    /// Downloads being packed for viewers; for viewer `key` only when given.
    pub(super) fn viewer_downloads(&self, key: Option<&PublicKey>) -> usize {
        self.jobs
            .values()
            .filter(|j| {
                matches!(j.pending, Pending::Download { .. })
                    && j.role == Some(ClientRole::Viewer)
                    && key.is_none_or(|k| j.reply.client.curve().is_some_and(|(_, c)| c == k))
            })
            .count()
    }

    /// `key`'s role was taken down: its jobs stop before they change or pack anything.
    pub(super) fn cancel_key(&self, key: &PublicKey) {
        for j in self.jobs.values() {
            if !matches!(j.pending, Pending::List)
                && j.reply.client.curve().is_some_and(|(_, k)| k == key)
            {
                j.cancel.set();
            }
        }
    }
}

impl Control {
    /// Takes the file slot for `command` from `role` (module docs): free, or held by a
    /// viewer's download an operator cuts; refused otherwise.
    fn take_file_slot(&mut self, command: &str, role: ClientRole) -> Result<(), ProtoError> {
        let Some((&token, j)) = self
            .file_jobs
            .jobs
            .iter()
            .find(|(_, j)| !matches!(j.pending, Pending::List))
        else {
            return Ok(());
        };
        if role != ClientRole::Operator || j.role != Some(ClientRole::Viewer) {
            return Err(refused(format!(
                "{} is under way; {command} waits until it is answered",
                j.command
            )));
        }
        let Some(cut) = self.file_jobs.jobs.remove(&token) else {
            return Ok(());
        };
        cut.cancel.set();
        tracing::info!(
            "{} of {} cut for an operator's {command}",
            cut.command,
            cut.reply.client.id().0
        );
        self.file_jobs.cut.push(cut.ended);
        let r = cut.reply;
        let e = refused(format!(
            "an operator's {command} came first; ask for {} again",
            cut.command
        ));
        self.answer(&r.route, &r.client, r.id, Err(e), Instant::now());
        Ok(())
    }

    /// A viewer key begins one download per [`VIEWER_DOWNLOAD_EVERY`].
    fn viewer_download_rate(&self, client: &Peer, now: Instant) -> Result<(), ProtoError> {
        let Some((_, key)) = client.curve() else {
            return Ok(());
        };
        match self.file_jobs.viewer_downloads.get(key) {
            Some(at) if now.saturating_duration_since(*at) < VIEWER_DOWNLOAD_EVERY => {
                let left = VIEWER_DOWNLOAD_EVERY.saturating_sub(now.saturating_duration_since(*at));
                Err(refused(format!(
                    "a viewer begins one download every {} s: try again in {} s",
                    VIEWER_DOWNLOAD_EVERY.as_secs(),
                    left.as_secs() + 1
                )))
            }
            _ => Ok(()),
        }
    }

    /// Begins a file operation whose reply follows its worker's result. `Ok`: pending, the
    /// reply comes later; `Err`: refused before anything began.
    pub(super) fn start_file_job(
        &mut self,
        route: &Route,
        client: &Peer,
        id: RequestId,
        cmd: Command,
        role: ClientRole,
    ) -> Result<(), ProtoError> {
        let command = cmd.name();
        let reply = PendingReply {
            route: route.clone(),
            client: client.clone(),
            id,
        };
        let now = Instant::now();
        let viewer_download =
            role == ClientRole::Viewer && matches!(cmd, Command::FileDownloadBegin { .. });
        match cmd {
            Command::FileList => {
                if let Some(j) = self
                    .file_jobs
                    .jobs
                    .values_mut()
                    .find(|j| matches!(j.pending, Pending::List))
                {
                    j.attached.push(reply);
                    return Ok(());
                }
            }
            _ => {
                if viewer_download {
                    self.viewer_download_rate(client, now)?;
                }
                self.take_file_slot(command, role)?;
            }
        }
        let (pending, work) = match cmd {
            Command::FileList => self.begin_list(),
            Command::FileSave { session, overwrite } => {
                self.begin_save(client, &session, overwrite)?
            }
            Command::FileLoad { session, discard } => self.begin_load(client, &session, discard)?,
            Command::FileNew { discard } => self.begin_new(client, discard)?,
            Command::FileRecover => self.begin_recover(client)?,
            Command::FileSalvage { session } => self.begin_salvage(client, &session)?,
            Command::FileImport {
                transfer,
                name,
                overwrite,
            } => self.begin_import(client, transfer, name, overwrite)?,
            Command::FileDownloadBegin { session } => {
                self.begin_download(client, session.as_ref())?
            }
            other => {
                return Err(internal(format!(
                    "{} is not a file operation",
                    other.name()
                )));
            }
        };
        let token = self.ids.token();
        let to = self.s.to_self.clone();
        let (alive, ended) = mpsc::channel::<()>();
        let cancel = Cancel::default();
        let theirs = cancel.clone();
        std::thread::Builder::new()
            .name("ac2d-file".into())
            .spawn(move || {
                let _alive = alive;
                let done = work(&theirs);
                let _ = to.send(ControlMsg::FileDone {
                    token,
                    done: Box::new(done),
                });
            })
            .map_err(|e| internal(format!("cannot start {command}: {e}")))?;
        if viewer_download && let Some((_, key)) = client.curve() {
            let keys = &mut self.file_jobs.viewer_downloads;
            keys.retain(|_, at| now.saturating_duration_since(*at) < VIEWER_DOWNLOAD_EVERY);
            keys.insert(*key, now);
        }
        self.file_jobs.jobs.insert(
            token,
            FileJob {
                command,
                reply,
                attached: Vec::new(),
                role: Some(role),
                cancel,
                pending,
                ended,
            },
        );
        Ok(())
    }

    /// Worker `token` reported back: commits and answers.
    pub(super) fn file_done(&mut self, token: u64, done: FileDone) {
        let Some(job) = self.file_jobs.jobs.remove(&token) else {
            return;
        };
        if let (Pending::List, FileDone::List(r)) = (&job.pending, &done) {
            let now = Instant::now();
            for w in std::iter::once(job.reply).chain(job.attached) {
                // Every role may list; a key revoked meanwhile is told nothing.
                let r = match self.peer_role(&w.client) {
                    Some(_) => r.clone(),
                    None => Err(role_changed(job.command)),
                };
                self.answer(&w.route, &w.client, w.id, r, now);
            }
            return;
        }
        if job.cancel.is_set() || self.peer_role(&job.reply.client) != job.role {
            if let FileDone::Replace(ReplaceRead { kept: Some(k), .. }) = done {
                k.discard();
            }
            tracing::info!(
                "{} of {} discarded: its role changed while it ran",
                job.command,
                job.reply.client.id().0
            );
            let r = job.reply;
            let e = role_changed(job.command);
            self.answer(&r.route, &r.client, r.id, Err(e), Instant::now());
            return;
        }
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
            (Pending::Recover { client, standing }, FileDone::Replace(r)) => {
                self.finish_recover(&client, standing, r)
            }
            (Pending::Salvage { transfer }, FileDone::Salvage(r)) => {
                self.finish_salvage(transfer, r)
            }
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
                Err(internal(format!(
                    "{}: a result of another operation",
                    job.command
                )))
            }
        };
        let r = job.reply;
        self.answer(&r.route, &r.client, r.id, result, Instant::now());
    }

    /// Shutdown: a file operation under way finishes writing first, within a bound.
    pub(super) fn flush_file_jobs(&mut self) {
        let deadline = Instant::now() + FLUSH_BOUND;
        let cut = self.file_jobs.cut.drain(..).map(|e| ("a cut download", e));
        let jobs = self
            .file_jobs
            .jobs
            .drain()
            .map(|(_, j)| (j.command, j.ended));
        for (command, ended) in jobs.chain(cut).collect::<Vec<_>>() {
            let left = deadline.saturating_duration_since(Instant::now());
            if ended.recv_timeout(left) == Err(RecvTimeoutError::Timeout) {
                tracing::warn!(
                    "{command} did not finish within {} s of shutdown",
                    FLUSH_BOUND.as_secs()
                );
            }
        }
    }
}
